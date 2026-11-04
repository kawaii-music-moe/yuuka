//! `/api/client/tts/*`（サーバーでの音声生成への中継）の HTTP 統合テスト（実ルータ・偽の音声生成サーバー）。
//!
//! 検証: 未ログインは 401（中継しない）・未設定は 404・許可した操作だけを中継し、本文と
//! `content-type`/`x-sample-rate` をそのまま返す・音声生成サーバーが落ちていれば 503・`TTS_SIGNING_KEY` が
//! あれば、音声生成サーバーが確かめられる Ed25519 署名を付ける。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body, Bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ring::signature::{KeyPair as _, UnparsedPublicKey, ED25519};
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

/// 署名の鍵（seed 0..31 の base64・`auth.test.ts` の固定の値と同じ）。
const SIGNING_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

fn app(tts_server_url: Option<String>) -> Router {
    app_with_key(tts_server_url, None)
}

fn app_with_key(tts_server_url: Option<String>, tts_signing_key: Option<&str>) -> Router {
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
        tts_signing_key: tts_signing_key.map(SecretString::from),
        ..WebConfig::default()
    };
    let state = AppState::new(Arc::new(FakeAuth), config, db);
    yuuka_client_api::routes_with(engine, rate_limiter).with_state(state)
}

/// 音声生成サーバー（`auth.ts`）と同じ手順で署名を確かめる。
fn signature_is_valid(path: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let (Some(time), Some(nonce), Some(sha), Some(sig)) = (
        get("x-yuuka-tts-time"),
        get("x-yuuka-tts-nonce"),
        get("x-yuuka-tts-body-sha256"),
        get("x-yuuka-tts-signature"),
    ) else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let fresh = time.parse::<u128>().is_ok_and(|t| t.abs_diff(now) < 30_000);
    let body_ok = sha == hex::encode(ring::digest::digest(&ring::digest::SHA256, body));
    let seed = STANDARD.decode(SIGNING_KEY).unwrap();
    let public = ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed)
        .unwrap()
        .public_key()
        .as_ref()
        .to_vec();
    let message = format!("yuuka-tts-v1\nPOST\n{path}\n{time}\n{nonce}\n{sha}");
    let sig_ok = UnparsedPublicKey::new(&ED25519, public)
        .verify(
            message.as_bytes(),
            &STANDARD.decode(sig).unwrap_or_default(),
        )
        .is_ok();
    fresh && body_ok && sig_ok && nonce.len() == 22
}

/// 偽の音声生成サーバー（127.0.0.1 の空きポート）。`/speak` は受け取った本文の前に `pcm:` を付けて返す。
/// `/design` は署名が正しければ 200、そうでなければ 401 を返す。
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
        )
        .route(
            "/design",
            post(|headers: HeaderMap, body: Bytes| async move {
                if signature_is_valid("/design", &headers, &body) {
                    StatusCode::OK
                } else {
                    StatusCode::UNAUTHORIZED
                }
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

#[tokio::test]
async fn signs_requests_when_a_signing_key_is_configured() {
    let upstream = fake_upstream().await;
    let signed = app_with_key(Some(upstream.clone()), Some(SIGNING_KEY));
    let body = r#"{"caption":"","voiceSeed":1}"#;
    let (status, _, _) = call(&signed, "POST", "/api/client/tts/design", Some(ALICE), body).await;
    assert_eq!(status, StatusCode::OK);

    // 鍵が無ければ署名を付けない（音声生成サーバーが断る）。
    let unsigned = app(Some(upstream));
    let (status, _, _) = call(
        &unsigned,
        "POST",
        "/api/client/tts/design",
        Some(ALICE),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
