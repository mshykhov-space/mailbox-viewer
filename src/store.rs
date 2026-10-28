use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::gmail::Message;

#[derive(Clone, Debug)]
pub struct Share {
    pub id: String,
    pub label: String,
    pub alias: String,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub last_viewed_at: Option<i64>,
    pub view_count: i64,
}

impl Share {
    pub fn is_active(&self, now: i64) -> bool {
        self.revoked_at.is_none() && self.expires_at.is_none_or(|expires_at| expires_at > now)
    }
}

pub struct CreatedShare {
    pub share: Share,
    pub token: String,
}

pub struct Store {
    conn: Mutex<Connection>,
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).context("creating state directory")?;
        }
        let conn = Connection::open(path).context("opening state database")?;
        Self::from_connection(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS shares (
                id TEXT PRIMARY KEY,
                token_hash TEXT NOT NULL UNIQUE,
                label TEXT NOT NULL DEFAULT '',
                alias TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                revoked_at INTEGER,
                expires_at INTEGER,
                last_viewed_at INTEGER,
                view_count INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS shares_alias ON shares(alias);
            CREATE TABLE IF NOT EXISTS messages (
                alias TEXT NOT NULL,
                id TEXT NOT NULL,
                thread_id TEXT NOT NULL,
                from_addr TEXT NOT NULL,
                subject TEXT NOT NULL,
                received_at INTEGER NOT NULL,
                body TEXT NOT NULL,
                PRIMARY KEY (alias, id)
            );
            CREATE TABLE IF NOT EXISTS alias_cache (
                alias TEXT PRIMARY KEY,
                fetched_at INTEGER NOT NULL
            );",
        )
        .context("initializing schema")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn create_share(
        &self,
        alias: &str,
        label: &str,
        expires_at: Option<i64>,
    ) -> Result<CreatedShare> {
        let token = generate_token()?;
        let share = Share {
            id: generate_id()?,
            label: label.to_string(),
            alias: alias.to_string(),
            created_at: now_secs(),
            revoked_at: None,
            expires_at,
            last_viewed_at: None,
            view_count: 0,
        };
        self.lock()
            .execute(
                "INSERT INTO shares (id, token_hash, label, alias, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    share.id,
                    hash_token(&token),
                    share.label,
                    share.alias,
                    share.created_at,
                    share.expires_at
                ],
            )
            .context("inserting share")?;
        Ok(CreatedShare { share, token })
    }

    pub fn list_shares(&self) -> Result<Vec<Share>> {
        let conn = self.lock();
        let mut statement = conn.prepare(
            "SELECT id, label, alias, created_at, revoked_at, expires_at, last_viewed_at, view_count
             FROM shares ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map([], row_to_share)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_share(&self, id: &str) -> Result<Option<Share>> {
        let conn = self.lock();
        let share = conn
            .query_row(
                "SELECT id, label, alias, created_at, revoked_at, expires_at, last_viewed_at, view_count
                 FROM shares WHERE id = ?1",
                params![id],
                row_to_share,
            )
            .optional()?;
        Ok(share)
    }

    pub fn find_by_token(&self, token: &str) -> Result<Option<Share>> {
        let conn = self.lock();
        let share = conn
            .query_row(
                "SELECT id, label, alias, created_at, revoked_at, expires_at, last_viewed_at, view_count
                 FROM shares WHERE token_hash = ?1",
                params![hash_token(token)],
                row_to_share,
            )
            .optional()?;
        Ok(share)
    }

    pub fn revoke_share(&self, id: &str) -> Result<bool> {
        let changed = self
            .lock()
            .execute(
                "UPDATE shares SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
                params![id, now_secs()],
            )
            .context("revoking share")?;
        Ok(changed > 0)
    }

    pub fn rotate_share(&self, id: &str) -> Result<Option<(Share, String)>> {
        let Some(share) = self.find_share(id)? else {
            return Ok(None);
        };
        if share.revoked_at.is_some() {
            return Err(anyhow!("share is revoked"));
        }
        let token = generate_token()?;
        self.lock()
            .execute(
                "UPDATE shares SET token_hash = ?2 WHERE id = ?1",
                params![id, hash_token(&token)],
            )
            .context("rotating share token")?;
        Ok(Some((share, token)))
    }

    pub fn touch_share(&self, id: &str) -> Result<()> {
        self.lock()
            .execute(
                "UPDATE shares SET last_viewed_at = ?2, view_count = view_count + 1 WHERE id = ?1",
                params![id, now_secs()],
            )
            .context("updating share usage")?;
        Ok(())
    }

    pub fn store_messages(&self, alias: &str, messages: &[Message]) -> Result<()> {
        let mut conn = self.lock();
        let transaction = conn.transaction()?;
        transaction.execute("DELETE FROM messages WHERE alias = ?1", params![alias])?;
        {
            let mut statement = transaction.prepare(
                "INSERT OR REPLACE INTO messages
                    (alias, id, thread_id, from_addr, subject, received_at, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for message in messages {
                statement.execute(params![
                    alias,
                    message.id,
                    message.thread_id,
                    message.from,
                    message.subject,
                    message.received_at,
                    message.body
                ])?;
            }
        }
        transaction.execute(
            "INSERT INTO alias_cache (alias, fetched_at) VALUES (?1, ?2)
             ON CONFLICT(alias) DO UPDATE SET fetched_at = excluded.fetched_at",
            params![alias, now_secs()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn cached_messages(&self, alias: &str, ttl: Duration) -> Result<Option<Vec<Message>>> {
        let conn = self.lock();
        let fetched_at: Option<i64> = conn
            .query_row(
                "SELECT fetched_at FROM alias_cache WHERE alias = ?1",
                params![alias],
                |row| row.get(0),
            )
            .optional()?;
        let Some(fetched_at) = fetched_at else {
            return Ok(None);
        };
        if now_secs() - fetched_at >= ttl.as_secs() as i64 {
            return Ok(None);
        }
        let mut statement = conn.prepare(
            "SELECT id, thread_id, from_addr, subject, received_at, body
             FROM messages WHERE alias = ?1 ORDER BY received_at DESC",
        )?;
        let rows = statement.query_map(params![alias], |row| {
            Ok(Message {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                from: row.get(2)?,
                subject: row.get(3)?,
                received_at: row.get(4)?,
                body: row.get(5)?,
            })
        })?;
        Ok(Some(rows.collect::<rusqlite::Result<Vec<_>>>()?))
    }
}

fn row_to_share(row: &rusqlite::Row<'_>) -> rusqlite::Result<Share> {
    Ok(Share {
        id: row.get(0)?,
        label: row.get(1)?,
        alias: row.get(2)?,
        created_at: row.get(3)?,
        revoked_at: row.get(4)?,
        expires_at: row.get(5)?,
        last_viewed_at: row.get(6)?,
        view_count: row.get(7)?,
    })
}

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn generate_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| anyhow!("random source failed: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn generate_id() -> Result<String> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| anyhow!("random source failed: {error}"))?;
    Ok(hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, received_at: i64) -> Message {
        Message {
            id: id.to_string(),
            thread_id: format!("t{id}"),
            from: "sender@example.com".to_string(),
            subject: "Subject".to_string(),
            received_at,
            body: "Body".to_string(),
        }
    }

    #[test]
    fn revoke_disables_token_immediately() {
        let store = Store::open_in_memory().unwrap();
        let created = store.create_share("a@icloud.com", "test", None).unwrap();
        assert!(store.find_by_token(&created.token).unwrap().is_some());
        assert!(store.revoke_share(&created.share.id).unwrap());
        let share = store.find_by_token(&created.token).unwrap().unwrap();
        assert!(!share.is_active(now_secs()));
    }

    #[test]
    fn rotate_invalidates_old_token() {
        let store = Store::open_in_memory().unwrap();
        let created = store.create_share("a@icloud.com", "test", None).unwrap();
        let (_, token) = store.rotate_share(&created.share.id).unwrap().unwrap();
        assert!(store.find_by_token(&created.token).unwrap().is_none() || created.token == token);
        assert!(store.find_by_token(&token).unwrap().is_some());
    }

    #[test]
    fn expired_share_is_inactive() {
        let store = Store::open_in_memory().unwrap();
        let created = store
            .create_share("a@icloud.com", "test", Some(now_secs() - 1))
            .unwrap();
        assert!(!created.share.is_active(now_secs()));
    }

    #[test]
    fn caches_messages_per_alias() {
        let store = Store::open_in_memory().unwrap();
        store
            .store_messages("a@icloud.com", &[message("1", 10)])
            .unwrap();
        store
            .store_messages("b@icloud.com", &[message("2", 20)])
            .unwrap();
        let cached = store
            .cached_messages("a@icloud.com", Duration::from_secs(60))
            .unwrap()
            .unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].id, "1");
        assert!(
            store
                .cached_messages("a@icloud.com", Duration::ZERO)
                .unwrap()
                .is_none()
        );
    }
}
