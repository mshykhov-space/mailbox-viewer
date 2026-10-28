use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::gmail::MailSource;
use crate::pages;
use crate::store::{Share, Store, now_secs};

pub struct AppState {
    pub admin_key: String,
    pub public_base_url: String,
    pub cache_ttl: Duration,
    pub message_limit: usize,
    pub store: Arc<Store>,
    pub mail: Arc<dyn MailSource>,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(not_found))
        .route("/health", get(health))
        .route("/m/{token}", get(public_page))
        .route("/api/shares", post(create_share).get(list_shares))
        .route("/api/shares/{id}", delete(revoke_share))
        .route("/api/shares/{id}/rotate", post(rotate_share))
        .with_state(state)
        .layer(middleware::map_response(security_headers))
}

async fn security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    );
    response
}

async fn health() -> &'static str {
    "ok"
}

async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(pages::state_page(
            "Страница не найдена",
            "Проверьте ссылку.",
        )),
    )
        .into_response()
}

async fn public_page(State(state): State<Arc<AppState>>, Path(token): Path<String>) -> Response {
    let Ok(Some(share)) = state.store.find_by_token(&token) else {
        return not_found().await;
    };
    if !share.is_active(now_secs()) {
        return (
            StatusCode::GONE,
            Html(pages::state_page(
                "Доступ закрыт",
                "Владелец отозвал эту ссылку или истёк срок действия.",
            )),
        )
            .into_response();
    }

    let (messages, stale) = match state
        .mail
        .messages_for(&share.alias, state.message_limit)
        .await
    {
        Ok(messages) => {
            if let Err(error) = state.store.store_messages(&share.alias, &messages) {
                tracing::warn!(alias = %share.alias, %error, "failed to cache messages");
            }
            (messages, false)
        }
        Err(error) => {
            tracing::warn!(alias = %share.alias, %error, "Gmail fetch failed");
            match state
                .store
                .cached_messages(&share.alias, Duration::from_secs(u32::MAX as u64))
            {
                Ok(Some(messages)) => (messages, true),
                _ => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Html(pages::state_page(
                            "Почта недоступна",
                            "Не удалось прочитать письма. Попробуйте обновить страницу позже.",
                        )),
                    )
                        .into_response();
                }
            }
        }
    };

    if let Err(error) = state.store.touch_share(&share.id) {
        tracing::warn!(share = %share.id, %error, "failed to update share usage");
    }
    Html(pages::inbox_page(&share, &messages, stale)).into_response()
}

#[derive(Deserialize)]
struct CreateShareRequest {
    alias: String,
    #[serde(default)]
    label: String,
    expires_in_days: Option<i64>,
}

#[derive(Serialize)]
struct ShareView {
    id: String,
    alias: String,
    label: String,
    active: bool,
    created_at: i64,
    expires_at: Option<i64>,
    revoked_at: Option<i64>,
    last_viewed_at: Option<i64>,
    view_count: i64,
    url: Option<String>,
}

#[derive(Serialize)]
struct CreatedShareView {
    #[serde(flatten)]
    share: ShareView,
    token: String,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

async fn create_share(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    payload: Result<Json<CreateShareRequest>, JsonRejection>,
) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => return bad_request(rejection.body_text()),
    };
    let alias = match normalize_alias(&request.alias) {
        Ok(alias) => alias,
        Err(error) => return bad_request(error),
    };
    if let Some(days) = request.expires_in_days
        && !(1..=3650).contains(&days)
    {
        return bad_request("expires_in_days must be between 1 and 3650".to_string());
    }
    let expires_at = request
        .expires_in_days
        .map(|days| now_secs() + days * 86_400);
    let label = request.label.trim().chars().take(120).collect::<String>();
    match state.store.create_share(&alias, &label, expires_at) {
        Ok(created) => {
            tracing::info!(share = %created.share.id, alias = %alias, "share created");
            let view = ShareView::new(&created.share, &state.public_base_url, Some(&created.token));
            Json(CreatedShareView {
                share: view,
                token: created.token,
            })
            .into_response()
        }
        Err(error) => internal_error(error),
    }
}

async fn list_shares(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    match state.store.list_shares() {
        Ok(shares) => Json(
            shares
                .iter()
                .map(|share| ShareView::new(share, &state.public_base_url, None))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => internal_error(error),
    }
}

async fn revoke_share(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    match state.store.revoke_share(&id) {
        Ok(true) => {
            tracing::info!(share = %id, "share revoked");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ErrorBody {
                error: "share not found or already revoked".into(),
            }),
        )
            .into_response(),
        Err(error) => internal_error(error),
    }
}

async fn rotate_share(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    match state.store.rotate_share(&id) {
        Ok(Some((share, token))) => {
            tracing::info!(share = %share.id, "share token rotated");
            let view = ShareView::new(&share, &state.public_base_url, Some(&token));
            Json(CreatedShareView { share: view, token }).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorBody {
                error: "share not found".into(),
            }),
        )
            .into_response(),
        Err(error) => {
            let message = error.to_string();
            if message == "share is revoked" {
                return (StatusCode::CONFLICT, Json(ErrorBody { error: message })).into_response();
            }
            internal_error(error)
        }
    }
}

impl ShareView {
    fn new(share: &Share, base_url: &str, token: Option<&str>) -> Self {
        Self {
            id: share.id.clone(),
            alias: share.alias.clone(),
            label: share.label.clone(),
            active: share.is_active(now_secs()),
            created_at: share.created_at,
            expires_at: share.expires_at,
            revoked_at: share.revoked_at,
            last_viewed_at: share.last_viewed_at,
            view_count: share.view_count,
            url: token.map(|token| format!("{base_url}/m/{token}")),
        }
    }
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let provided = headers
        .get("x-admin-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if constant_time_eq(provided.as_bytes(), state.admin_key.as_bytes()) {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                error: "invalid admin key".into(),
            }),
        )
            .into_response())
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

pub fn normalize_alias(value: &str) -> Result<String, String> {
    let alias = value.trim().to_ascii_lowercase();
    if !(6..=254).contains(&alias.len()) {
        return Err("alias length must be between 6 and 254 characters".into());
    }
    let mut parts = alias.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err("alias must contain exactly one @".into());
    };
    let local_ok = !local.is_empty()
        && local
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+'));
    let domain_ok = domain.contains('.')
        && domain
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'));
    if !local_ok || !domain_ok {
        return Err("alias contains invalid characters".into());
    }
    Ok(alias)
}

fn bad_request(message: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(ErrorBody { error: message })).into_response()
}

fn internal_error(error: anyhow::Error) -> Response {
    tracing::error!(%error, "internal error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorBody {
            error: "internal error".into(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_aliases() {
        assert_eq!(
            normalize_alias("  A.B@iCloud.com ").unwrap(),
            "a.b@icloud.com"
        );
        assert!(normalize_alias("noatsign").is_err());
        assert!(normalize_alias("a@@b.c").is_err());
        assert!(normalize_alias("a b@c.com").is_err());
        assert!(normalize_alias("a@localhost").is_err());
    }

    #[test]
    fn compares_keys_without_early_exit() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secre"));
    }
}
