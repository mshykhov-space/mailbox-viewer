use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use mailbox_viewer::gmail::{MailSource, Message};
use mailbox_viewer::store::Store;
use mailbox_viewer::web::{AppState, router};
use tower::ServiceExt;

const ADMIN_KEY: &str = "test-admin-key-that-is-long-enough";

struct StubMail {
    messages: Vec<Message>,
    fail: bool,
}

#[async_trait]
impl MailSource for StubMail {
    async fn messages_for(&self, _alias: &str, _limit: usize) -> anyhow::Result<Vec<Message>> {
        if self.fail {
            anyhow::bail!("gmail is down");
        }
        Ok(self.messages.clone())
    }
}

fn message(id: &str) -> Message {
    Message {
        id: id.to_string(),
        thread_id: format!("t{id}"),
        from: "WhiteBIT <no-reply@whitebit.com>".to_string(),
        subject: format!("Code for {id}"),
        received_at: 1_700_000_000,
        body: format!("Your code is {id}"),
    }
}

fn app(mail: StubMail) -> (Arc<Store>, Router) {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let state = Arc::new(AppState {
        admin_key: ADMIN_KEY.to_string(),
        public_base_url: "https://mailbox.example".to_string(),
        cache_ttl: Duration::from_secs(0),
        message_limit: 20,
        store: store.clone(),
        mail: Arc::new(mail),
    });
    (store, router(state))
}

async fn body(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn admin_request(method: &str, uri: &str, key: &str, json: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-admin-key", key);
    if json.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    builder
        .body(Body::from(json.unwrap_or_default().to_string()))
        .unwrap()
}

async fn create_share(app: &Router, alias: &str) -> (String, String) {
    let response = app
        .clone()
        .oneshot(admin_request(
            "POST",
            "/api/shares",
            ADMIN_KEY,
            Some(&format!("{{\"alias\":\"{alias}\",\"label\":\"Steam\"}}")),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
    (
        value["token"].as_str().unwrap().to_string(),
        value["id"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn creates_share_and_shows_plain_messages() {
    let (_store, app) = app(StubMail {
        messages: vec![message("123456")],
        fail: false,
    });
    let (token, id) = create_share(&app, "a.b@icloud.com").await;
    assert!(!id.is_empty());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/m/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    assert!(html.contains("a.b@icloud.com"));
    assert!(html.contains("Your code is 123456"));
    assert!(html.contains("noindex"));
}

#[tokio::test]
async fn revoked_link_stops_working() {
    let (_store, app) = app(StubMail {
        messages: vec![message("1")],
        fail: false,
    });
    let (token, id) = create_share(&app, "a@icloud.com").await;

    let response = app
        .clone()
        .oneshot(admin_request(
            "DELETE",
            &format!("/api/shares/{id}"),
            ADMIN_KEY,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/m/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
}

#[tokio::test]
async fn rotation_replaces_link() {
    let (_store, app) = app(StubMail {
        messages: vec![message("1")],
        fail: false,
    });
    let (token, id) = create_share(&app, "a@icloud.com").await;

    let response = app
        .clone()
        .oneshot(admin_request(
            "POST",
            &format!("/api/shares/{id}/rotate"),
            ADMIN_KEY,
            None,
        ))
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
    let new_token = value["token"].as_str().unwrap().to_string();
    assert_ne!(new_token, token);

    let old = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/m/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(old.status(), StatusCode::NOT_FOUND);

    let new = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/m/{new_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(new.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_endpoints_require_key() {
    let (_store, app) = app(StubMail {
        messages: vec![],
        fail: false,
    });
    for request in [
        admin_request(
            "POST",
            "/api/shares",
            "wrong-key",
            Some("{\"alias\":\"a@icloud.com\"}"),
        ),
        admin_request("GET", "/api/shares", "wrong-key", None),
        admin_request("POST", "/api/shares/1/rotate", "", None),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn invalid_alias_is_rejected() {
    let (_store, app) = app(StubMail {
        messages: vec![],
        fail: false,
    });
    let response = app
        .clone()
        .oneshot(admin_request(
            "POST",
            "/api/shares",
            ADMIN_KEY,
            Some("{\"alias\":\"not an address\"}"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_token_is_not_found() {
    let (_store, app) = app(StubMail {
        messages: vec![],
        fail: false,
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/m/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn gmail_failure_without_cache_returns_bad_gateway() {
    let (_store, app) = app(StubMail {
        messages: vec![],
        fail: true,
    });
    let (token, _id) = create_share(&app, "a@icloud.com").await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/m/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}
