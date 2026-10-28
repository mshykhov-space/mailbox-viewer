use std::env;
use std::time::Duration;

use anyhow::{Context, Result, bail};

#[derive(Clone, Debug)]
pub struct Config {
    pub http_port: u16,
    pub db_path: String,
    pub admin_key: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub google_refresh_token: String,
    pub public_base_url: String,
    pub cache_ttl: Duration,
    pub message_limit: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let admin_key = required("ADMIN_API_KEY")?;
        if admin_key.len() < 24 {
            bail!("ADMIN_API_KEY must be at least 24 characters");
        }
        Ok(Self {
            http_port: optional("HTTP_PORT", "8080").parse().context("HTTP_PORT")?,
            db_path: optional("STATE_DB_PATH", "/state/mailbox-viewer.db"),
            admin_key,
            google_client_id: required("GOOGLE_CLIENT_ID")?,
            google_client_secret: required("GOOGLE_CLIENT_SECRET")?,
            google_refresh_token: required("GOOGLE_REFRESH_TOKEN")?,
            public_base_url: optional("PUBLIC_BASE_URL", "http://localhost:8080")
                .trim_end_matches('/')
                .to_string(),
            cache_ttl: Duration::from_secs(
                optional("CACHE_TTL_SECONDS", "30")
                    .parse()
                    .context("CACHE_TTL_SECONDS")?,
            ),
            message_limit: optional("MESSAGE_LIMIT", "25")
                .parse()
                .context("MESSAGE_LIMIT")?,
        })
    }
}

fn required(name: &str) -> Result<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => bail!("{name} is required"),
    }
}

fn optional(name: &str, default: &str) -> String {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => default.to_string(),
    }
}
