//! `/api/client/tts/*`（サーバーでの音声生成への中継）の HTTP 統合テスト（実ルータ・偽の音声生成サーバー）。
//!
//! 検証: 未ログインは 401（中継しない）・未設定は 404・許可した操作だけを中継し、本文と
//! `content-type`/`x-sample-rate` をそのまま返す・音声生成サーバーが落ちていれば 503。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body, Bytes};
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use secrecy::SecretString;
use tower::ServiceExt;
use yuuka_core::{AuthError, GeminiError};
use yuuka_discord::RateLimiter;
use yuuka_gemini::GenerateBackend;
use yuuka_orchestrator::{ChatEngine, GeminiFactory, InMemoryRateLimiter};
use yuuka_tools::ToolRegistry;
use yuuka_types::{Role, SessionUser};
use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

static SEQ: AtomicU64 = AtomicU64::new(0);
const ALICE: &str = "alice";

fn fresh_db() -> Db {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "yuuka_client_api_tts_it_{}_{n}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    rusqlite::Connection::open(&path).expect("seed file");
    Db::open(&path).expect("open db")
}

struct FakeAuth;

#[async_trait]
impl AuthBackend for FakeAuth {
    async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok((token == ALICE).then(|| SessionUser {
            discord_id: token.to_owned(),
            username: token.to_owned(),
            role: Role::User,
        }))
    }

    async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok(None)
    }
}

struct UnusedFactory;

impl GeminiFactory for UnusedFactory {
    fn build(
        &self,
        _model: &str,
        _api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        Err(GeminiError::Timeout)
    }
}

fn app(tts_server_url: Option<String>) -> Router {
    let db = fresh_db();
    let engine = Arc::new(ChatEngine::new(
        db.clone(),
        None,
        ToolRegistry::new(),
        Arc::new(UnusedFactory),
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        None,
    ));
    let rate_limiter: Arc<dyn RateLimiter> = Arc::new(InMemoryRateLimiter::new(db.clone()));
    let config = WebConfig {
        tts_server_url,
        ..WebConfig::default()
    };
    let state = AppState::new(Arc::new(FakeAuth), config, db);
    yuuka_client_api::routes_with(engine, rate_limiter).with_state(state)
}

/// 偽の音声生成サーバー（127.0.0.1 の空きポート）。`/speak` は受け取った本文の前に `pcm:` を付けて返す。
async fn fake_upstream() -> String {
    let router = Router::new()
        .route("/status", get(|| async { r#"{"ok":true}"# }))
        .route(
            "/speak",
            post(|body: Bytes| async move {
                (
                    [
                        ("content-type", "application/octet-stream"),
                        ("x-sample-rate", "48000"),
                    ],
                    [b"pcm:".as_slice(), &body].concat(),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: &'static str,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("cookie", format!("__Host-yuuka-session={token}"));
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let rate = resp
        .headers()
        .get("x-sample-rate")
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, rate, bytes)
}

#[tokio::test]
async fn requires_login_and_configuration() {
    let configured = app(Some(fake_upstream().await));
    let (status, _, _) = call(&configured, "GET", "/api/client/tts/status", None, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let unconfigured = app(None);
    let (status, _, _) = call(
        &unconfigured,
        "GET",
        "/api/client/tts/status",
        Some(ALICE),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn forwards_allowed_operations_with_body_and_headers() {
    let app = app(Some(fake_upstream().await));
    let (status, _, body) = call(&app, "GET", "/api/client/tts/status", Some(ALICE), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, br#"{"ok":true}"#);

    let (status, rate, body) = call(
        &app,
        "POST",
        "/api/client/tts/speak",
        Some(ALICE),
        r#"{"text":"はい。"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rate.as_deref(), Some("48000"));
    assert_eq!(body, "pcm:{\"text\":\"はい。\"}".as_bytes());

    // 許可していない操作・メソッドは中継しない。
    let (status, _, _) = call(&app, "POST", "/api/client/tts/admin", Some(ALICE), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(&app, "GET", "/api/client/tts/speak", Some(ALICE), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upstream_down_is_503() {
    // 何も待ち受けていないポート。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let app = app(Some(format!("http://{addr}")));
    let (status, _, _) = call(&app, "GET", "/api/client/tts/status", Some(ALICE), "").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
