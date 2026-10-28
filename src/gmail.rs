use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::Value;

use crate::text::html_to_text;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub subject: String,
    pub received_at: i64,
    pub body: String,
}

#[async_trait]
pub trait MailSource: Send + Sync {
    async fn messages_for(&self, alias: &str, limit: usize) -> Result<Vec<Message>>;
}

pub struct GmailClient {
    http: Client,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    token: Mutex<Option<(String, Instant)>>,
}

impl GmailClient {
    pub fn new(client_id: String, client_secret: String, refresh_token: String) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            client_id,
            client_secret,
            refresh_token,
            token: Mutex::new(None),
        })
    }

    async fn access_token(&self) -> Result<String> {
        {
            let guard = self
                .token
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((token, expires_at)) = guard.as_ref()
                && Instant::now() < *expires_at
            {
                return Ok(token.clone());
            }
        }
        let response = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", self.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .context("requesting Gmail access token")?;
        if !response.status().is_success() {
            return Err(anyhow!("token endpoint returned {}", response.status()));
        }
        let body: TokenResponse = response.json().await.context("decoding token response")?;
        let ttl = Duration::from_secs(body.expires_in.saturating_sub(60).max(30));
        let mut guard = self
            .token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some((body.access_token.clone(), Instant::now() + ttl));
        Ok(body.access_token)
    }

    async fn get_json(&self, url: Url) -> Result<Value> {
        let token = self.access_token().await?;
        let response = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .context("Gmail API request")?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!("Gmail API returned {status}"));
        }
        response.json().await.context("decoding Gmail response")
    }
}

#[async_trait]
impl MailSource for GmailClient {
    async fn messages_for(&self, alias: &str, limit: usize) -> Result<Vec<Message>> {
        let mut list_url = Url::parse(&format!("{GMAIL_BASE}/messages")).expect("static URL");
        list_url
            .query_pairs_mut()
            .append_pair("q", &format!("to:\"{alias}\""))
            .append_pair("maxResults", &limit.to_string());
        let list = self.get_json(list_url).await?;
        let ids: Vec<String> = list
            .get("messages")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let mut messages = Vec::with_capacity(ids.len());
        for id in ids {
            let mut url = Url::parse(&format!("{GMAIL_BASE}/messages/{id}")).expect("static URL");
            url.query_pairs_mut().append_pair("format", "full");
            let message = self.get_json(url).await?;
            messages.push(parse_message(&message)?);
        }
        messages.sort_by_key(|message| std::cmp::Reverse(message.received_at));
        Ok(messages)
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

fn parse_message(value: &Value) -> Result<Message> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("message without id"))?
        .to_string();
    let thread_id = value
        .get("threadId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let received_at = value
        .get("internalDate")
        .and_then(Value::as_str)
        .and_then(|millis| millis.parse::<i64>().ok())
        .map(|millis| millis / 1000)
        .unwrap_or_default();
    let payload = value.get("payload");
    let header = |name: &str| {
        payload
            .and_then(|payload| payload.get("headers"))
            .and_then(Value::as_array)
            .and_then(|headers| {
                headers.iter().find(|header| {
                    header
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|value| value.eq_ignore_ascii_case(name))
                })
            })
            .and_then(|header| header.get("value"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let body = payload.map(extract_body).unwrap_or_default();
    Ok(Message {
        id,
        thread_id,
        from: header("From"),
        subject: header("Subject"),
        received_at,
        body: body.trim().to_string(),
    })
}

fn extract_body(payload: &Value) -> String {
    let mut plain = String::new();
    let mut html = String::new();
    collect_parts(payload, &mut plain, &mut html);
    if !plain.trim().is_empty() {
        plain
    } else {
        html_to_text(&html)
    }
}

fn collect_parts(payload: &Value, plain: &mut String, html: &mut String) {
    if let Some(parts) = payload.get("parts").and_then(Value::as_array) {
        for part in parts {
            collect_parts(part, plain, html);
        }
        return;
    }
    let mime = payload
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let data = payload
        .get("body")
        .and_then(|body| body.get("data"))
        .and_then(Value::as_str);
    let Some(data) = data else { return };
    let decoded = URL_SAFE_NO_PAD
        .decode(data)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    match mime {
        "text/plain" => {
            plain.push_str(&decoded);
            plain.push('\n');
        }
        "text/html" => {
            html.push_str(&decoded);
            html.push('\n');
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn encoded(text: &str) -> String {
        URL_SAFE_NO_PAD.encode(text.as_bytes())
    }

    #[test]
    fn parses_plain_part() {
        let value = json!({
            "id": "abc",
            "threadId": "t1",
            "internalDate": "1700000000000",
            "payload": {
                "mimeType": "multipart/alternative",
                "headers": [
                    {"name": "From", "value": "a@b.c"},
                    {"name": "Subject", "value": "Code 123"}
                ],
                "parts": [
                    {"mimeType": "text/plain", "body": {"data": encoded("Your code is 123")}},
                    {"mimeType": "text/html", "body": {"data": encoded("<b>Your code is 123</b>")}}
                ]
            }
        });
        let message = parse_message(&value).unwrap();
        assert_eq!(message.body, "Your code is 123");
        assert_eq!(message.subject, "Code 123");
        assert_eq!(message.received_at, 1_700_000_000);
    }

    #[test]
    fn falls_back_to_html_text() {
        let value = json!({
            "id": "abc",
            "payload": {
                "mimeType": "text/html",
                "headers": [],
                "body": {"data": encoded("<p>Hello</p><p>World</p>")}
            }
        });
        assert_eq!(parse_message(&value).unwrap().body, "Hello\nWorld");
    }
}
