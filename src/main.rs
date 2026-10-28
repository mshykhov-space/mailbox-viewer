use std::sync::Arc;

use anyhow::{Context, Result};
use mailbox_viewer::config::Config;
use mailbox_viewer::gmail::GmailClient;
use mailbox_viewer::store::Store;
use mailbox_viewer::web::{AppState, router};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    let store = Arc::new(Store::open(&config.db_path).context("opening state database")?);
    let mail = Arc::new(GmailClient::new(
        config.google_client_id.clone(),
        config.google_client_secret.clone(),
        config.google_refresh_token.clone(),
    )?);
    let state = Arc::new(AppState {
        admin_key: config.admin_key.clone(),
        public_base_url: config.public_base_url.clone(),
        cache_ttl: config.cache_ttl,
        message_limit: config.message_limit,
        store,
        mail,
    });

    let listener = TcpListener::bind(("0.0.0.0", config.http_port))
        .await
        .with_context(|| format!("binding port {}", config.http_port))?;
    tracing::info!(port = config.http_port, base_url = %config.public_base_url, "mailbox-viewer listening");
    axum::serve(listener, router(state))
        .await
        .context("serving HTTP")?;
    Ok(())
}
