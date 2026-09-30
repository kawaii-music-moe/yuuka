//! `POST /api/client/chat/messages` の HTTP 統合テスト（issue #41・fake Gemini backend・実 SQLite）。
//!
//! 検証: 認証必須・レート制限拒否・Gemini キー未設定拒否・ハッピーパス（`source='pwa'` へ永続化）。
//! ネットワーク不要（`GeminiFactory` を fake backend に差し替え、実 API は一切呼ばない）。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use secrecy::SecretString;
use serde_json::{json, Value};
use tower::ServiceExt;
use yuuka_core::{AuthError, GeminiError};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::RateLimiter;
use yuuka_gemini::{
    Content, FunctionDeclaration, GenerateBackend, GenerateContentResponse, ToolConfig,
};
use yuuka_orchestrator::{ChatEngine, GeminiFactory, InMemoryRateLimiter};
use yuuka_tools::ToolRegistry;
use yuuka_types::{Role, SessionUser};
use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// migrations 適用済みの一時 DB（本番 open は CREATE しないので先にファイルを作る）。
fn fresh_db() -> (Db, std::path::PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "yuuka_client_api_chat_it_{}_{n}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    {
        rusqlite::Connection::open(&path).expect("seed file");
    }
    let db = Db::open(&path).expect("open db");
    (db, path)
}

/// Cookie トークン `"u1"` のみ `discord_id="u1"` として認証する fake バックエンド。
struct FakeAuth;

#[async_trait]
impl AuthBackend for FakeAuth {
    async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok((token == "u1").then(|| SessionUser {
            discord_id: "u1".to_owned(),
            username: "u1".to_owned(),
            role: Role::User,
        }))
    }

    async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok(None)
    }
}

/// 固定テキストを 1 度だけ返す fake backend（tools 無しの単発ターン・
/// `crates/yuuka-orchestrator/tests/secretary_turn.rs` と同じ意匠）。
struct FakeBackend {
    responses: Mutex<VecDeque<GenerateContentResponse>>,
}

#[async_trait]
impl GenerateBackend for FakeBackend {
    async fn generate(
        &self,
        _system_instruction: Option<&str>,
        _declarations: &[FunctionDeclaration],
        _contents: &[Content],
        _tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("fake backend ran out of responses"))
    }
}

/// 固定テキストを返す backend を毎ターン新規生成するファクトリ（実 Gemini API は一切呼ばない）。
struct FakeFactory {
    text: String,
}

impl GeminiFactory for FakeFactory {
    fn build(
        &self,
        _model: &str,
        _api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        let resp: GenerateContentResponse = serde_json::from_value(json!({
            "candidates": [ { "content": { "role": "model", "parts": [ { "text": self.text } ] } } ]
        }))
        .expect("valid response");
        let mut q = VecDeque::new();
        q.push_back(resp);
        Ok(Arc::new(FakeBackend {
            responses: Mutex::new(q),
        }))
    }
}

/// システム鍵 crypto でユーザーの Gemini キーを暗号化して `users` 行を seed する。
fn seed_user_with_key(path: &std::path::Path, crypto: &SystemCrypto, discord_id: &str) {
    let enc = crypto.encrypt_text("fake-gemini-key").expect("encrypt");
    let conn = rusqlite::Connection::open(path).expect("open seed");
    conn.execute(
        "INSERT INTO users (discord_id, username, password_hash, salt, role, \
         gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag, gemini_model) \
         VALUES (?1, 'yuu', 'x', '00', 'user', ?2, ?3, ?4, 'gemini-3.1-flash-lite')",
        rusqlite::params![discord_id, enc.encrypted, enc.iv, enc.auth_tag],
    )
    .expect("seed user");
}

/// `users` 行だけ seed（Gemini キー無し）。
fn seed_user_no_key(path: &std::path::Path, discord_id: &str) {
    let conn = rusqlite::Connection::open(path).expect("open seed");
    conn.execute(
        "INSERT INTO users (discord_id, username, password_hash, salt, role) \
         VALUES (?1, 'yuu', 'x', '00', 'user')",
        rusqlite::params![discord_id],
    )
    .expect("seed user");
}

fn engine_with(db: Db, crypto: Option<Arc<SystemCrypto>>, text: &str) -> Arc<ChatEngine> {
    Arc::new(ChatEngine::new(
        db,
        crypto,
        ToolRegistry::new(),
        Arc::new(FakeFactory {
            text: text.to_owned(),
        }),
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        None,
    ))
}

/// `yuuka_client_api::routes_with` を単体で `AppState` に載せたテスト用アプリ（`build_app` は経由
/// しない・本クレートのルート層だけを検証する）。
fn app(engine: Arc<ChatEngine>, db: Db) -> Router {
    let rate_limiter: Arc<dyn RateLimiter> = Arc::new(InMemoryRateLimiter::new(db.clone()));
    let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), db);
    yuuka_client_api::routes_with(engine, rate_limiter).with_state(state)
}

async fn post_chat(app: &Router, token: Option<&str>, content: &str) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/client/chat/messages")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("cookie", format!("__Host-yuuka-session={token}"));
    }
    let resp = app
        .clone()
        .oneshot(
            builder
                .body(Body::from(json!({ "content": content }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

#[tokio::test]
async fn requires_auth() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    let engine = engine_with(db.clone(), Some(crypto), "unused");
    let app = app(engine, db);

    // Cookie 無し（未認証）→ 401。
    let (status, _) = post_chat(&app, None, "hello").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 無効なトークン（FakeAuth が既知としないもの）→ 401。
    let (status, _) = post_chat(&app, Some("bogus"), "hello").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn empty_content_is_rejected_with_bad_request() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    let engine = engine_with(db.clone(), Some(crypto), "unused");
    let app = app(engine, db);

    let (status, body) = post_chat(&app, Some("u1"), "   ").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "content is required");
}

#[tokio::test]
async fn missing_gemini_key_is_rejected_before_calling_the_engine() {
    let (db, path) = fresh_db();
    seed_user_no_key(&path, "u1");
    // crypto は Some でもキー行が無ければ事前チェックで弾く（WS `error/no_gemini_key` と同じ判定）。
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    let engine = engine_with(db.clone(), Some(crypto), "unused");
    let app = app(engine, db);

    let (status, body) = post_chat(&app, Some("u1"), "こんにちは").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"].as_str().unwrap().contains("Gemini APIキー"),
        "body={body:?}"
    );

    // 事前チェックで弾かれ secretary_turn_pwa まで到達しない＝ユーザー発言すら保存されない。
    let count: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM message_logs WHERE user_id='u1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "事前チェックで弾かれた発言は永続化されない");
}

#[tokio::test]
async fn rate_limit_rejects_after_default_per_minute_quota() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    let engine = engine_with(db.clone(), Some(crypto), "了解しました！");
    let app = app(engine, db);

    // 既定の分あたり上限（5・`InMemoryRateLimiter` の `DEFAULT_USER_PER_MINUTE`）まで送信できる。
    for i in 0..5 {
        let (status, _) = post_chat(&app, Some("u1"), &format!("msg {i}")).await;
        assert_eq!(status, StatusCode::CREATED, "iteration {i}");
    }
    // 6 回目は 429（WS/Discord と同じ `rate_limit_message` 文言）。
    let (status, body) = post_chat(&app, Some("u1"), "one more").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        body["message"].as_str().unwrap().contains("利用ペース"),
        "body={body:?}"
    );
}

#[tokio::test]
async fn happy_path_persists_user_and_assistant_messages_under_pwa_source() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    let engine = engine_with(
        db.clone(),
        Some(crypto),
        "こんにちは、PWAからのメッセージですね！",
    );
    let app = app(engine, db);

    let (status, body) = post_chat(&app, Some("u1"), "こんにちは").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["role"], "agent");
    assert_eq!(body["content"], "こんにちは、PWAからのメッセージですね！");
    assert!(
        body["id"].as_str().unwrap().starts_with("pwa-"),
        "id={:?}",
        body["id"]
    );
    assert!(body["createdAt"].as_str().is_some());

    let conn = rusqlite::Connection::open(&path).unwrap();
    let user_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_logs \
             WHERE user_id='u1' AND role='user' AND content='こんにちは' AND source='pwa'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let assistant_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_logs \
             WHERE user_id='u1' AND role='assistant' \
               AND content='こんにちは、PWAからのメッセージですね！' AND source='pwa'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        user_count, 1,
        "PWA のユーザー発言が source='pwa' で保存される"
    );
    assert_eq!(
        assistant_count, 1,
        "PWA のアシスタント応答が source='pwa' で保存される"
    );

    // GET 履歴にもそのまま反映される（`chat_history` ハンドラとの整合）。
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/client/chat/messages")
                .header("cookie", "__Host-yuuka-session=u1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let history: Value = serde_json::from_slice(&bytes).unwrap();
    let items = history.as_array().unwrap();
    assert_eq!(items.len(), 2, "history={items:?}");
    assert_eq!(items[0]["role"], "user");
    assert_eq!(items[1]["role"], "agent");
}
