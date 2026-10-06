//! PWA のエージェント切り替え（`/api/client/*?botId=`）の HTTP 統合テスト（実 SQLite・実ルータ）。
//!
//! 検証: `botId` ごとにデータ（Todo・共有ノート・ペルソナ）が分かれ、未指定・空は秘書 Bot
//! （`system_default`）と同じ・自分の Bot と `active` 共有の Bot は使える・未知/他人の未共有/共有解除
//! 済みの Bot は秘書 Bot へフォールバックせず 404（何も書かない）。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use secrecy::SecretString;
use serde_json::{json, Value};
use tower::ServiceExt;
use yuuka_core::{AuthError, GeminiError};
use yuuka_discord::RateLimiter;
use yuuka_gemini::GenerateBackend;
use yuuka_orchestrator::{ChatEngine, GeminiFactory, InMemoryRateLimiter};
use yuuka_tools::ToolRegistry;
use yuuka_types::{Role, SessionUser};
use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// 操作するユーザー。
const ME: &str = "u1";
/// 他人（Bot のオーナー）。
const OTHER: &str = "u2";

fn fresh_db() -> (Db, std::path::PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "yuuka_client_api_agent_switch_it_{}_{n}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    {
        rusqlite::Connection::open(&path).expect("seed file");
    }
    let db = Db::open(&path).expect("open db");
    let conn = rusqlite::Connection::open(&path).expect("open seed");
    conn.execute_batch(
        "INSERT INTO users (discord_id, username, password_hash, salt, role) \
            VALUES ('u1', 'u1', 'x', '00', 'user'), ('u2', 'u2', 'x', '00', 'user');
         INSERT INTO bots (id, user_id, name) VALUES \
            ('mybot', 'u1', 'Mine'), ('otherbot', 'u2', 'Not shared'), \
            ('sharedbot', 'u2', 'Shared'), ('revokedbot', 'u2', 'Revoked');
         INSERT INTO bot_shares (bot_id, owner_id, shared_user_id, status) VALUES \
            ('sharedbot', 'u2', 'u1', 'active'), ('revokedbot', 'u2', 'u1', 'revoked');",
    )
    .expect("seed users/bots");
    (db, path)
}

struct FakeAuth;

#[async_trait]
impl AuthBackend for FakeAuth {
    async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok((token == ME || token == OTHER).then(|| SessionUser {
            discord_id: token.to_owned(),
            username: token.to_owned(),
            role: Role::User,
        }))
    }

    async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok(None)
    }
}

/// このテストはチャットエンジンを使わない。ルータ構築のために渡すだけの、常に失敗するファクトリ。
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

fn app(db: Db) -> Router {
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
    let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), db);
    yuuka_client_api::routes_with(engine, rate_limiter).with_state(state)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("cookie", format!("__Host-yuuka-session={ME}"));
    let body = body.map_or_else(Body::empty, |v| Body::from(v.to_string()));
    let resp = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn todo_titles(app: &Router, query: &str) -> Vec<String> {
    let (status, body) = call(app, "GET", &format!("/api/client/todos{query}"), None).await;
    assert_eq!(status, StatusCode::OK, "{query}: {body}");
    body.as_array()
        .expect("todos array")
        .iter()
        .map(|t| t["title"].as_str().unwrap().to_owned())
        .collect()
}

async fn add_todo(app: &Router, query: &str, title: &str) -> StatusCode {
    call(
        app,
        "POST",
        &format!("/api/client/todos{query}"),
        Some(json!({ "title": title })),
    )
    .await
    .0
}

fn todo_count(path: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(path).expect("open");
    conn.query_row("SELECT COUNT(*) FROM todos", [], |r| r.get(0))
        .expect("count")
}

#[tokio::test]
async fn data_is_separated_per_agent_and_default_is_the_secretary() {
    let (db, _path) = fresh_db();
    let app = app(db);

    assert_eq!(
        add_todo(&app, "", "secretary task").await,
        StatusCode::CREATED
    );
    assert_eq!(
        add_todo(&app, "?botId=mybot", "my bot task").await,
        StatusCode::CREATED
    );
    assert_eq!(
        add_todo(&app, "?botId=sharedbot", "shared task").await,
        StatusCode::CREATED
    );

    assert_eq!(todo_titles(&app, "").await, ["secretary task"]);
    // 空・明示の system_default は未指定と同じ秘書 Bot。
    assert_eq!(todo_titles(&app, "?botId=").await, ["secretary task"]);
    assert_eq!(
        todo_titles(&app, "?botId=system_default").await,
        ["secretary task"]
    );
    assert_eq!(todo_titles(&app, "?botId=mybot").await, ["my bot task"]);
    assert_eq!(todo_titles(&app, "?botId=sharedbot").await, ["shared task"]);
}

#[tokio::test]
async fn inaccessible_agent_is_not_found_instead_of_falling_back_to_the_secretary() {
    let (db, path) = fresh_db();
    let app = app(db);

    for bot in ["otherbot", "revokedbot", "no-such-bot"] {
        let query = format!("?botId={bot}");
        let (status, _) = call(&app, "GET", &format!("/api/client/todos{query}"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {bot}");
        assert_eq!(
            add_todo(&app, &query, "leak").await,
            StatusCode::NOT_FOUND,
            "POST {bot}"
        );
        let (status, _) = call(
            &app,
            "GET",
            &format!("/api/client/chat/messages{query}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "chat {bot}");
    }
    assert_eq!(todo_count(&path), 0, "拒否したリクエストは何も書かない");
}

#[tokio::test]
async fn query_parameters_other_than_bot_id_still_work() {
    let (db, _path) = fresh_db();
    let app = app(db);
    let (status, body) = call(
        &app,
        "GET",
        "/api/client/finance/summary?month=2026-08&botId=mybot",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["month"], "2026-08");
}

#[tokio::test]
async fn shared_note_and_persona_follow_the_selected_agent() {
    let (db, _path) = fresh_db();
    let app = app(db);

    let (status, _) = call(
        &app,
        "PUT",
        "/api/client/shared-note?botId=mybot",
        Some(json!({ "title": "t", "body": "my bot note" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, mine) = call(&app, "GET", "/api/client/shared-note?botId=mybot", None).await;
    let (_, secretary) = call(&app, "GET", "/api/client/shared-note", None).await;
    assert_eq!(mine["body"], "my bot note");
    assert_eq!(secretary["body"], "");

    let (status, saved) = call(
        &app,
        "PUT",
        "/api/client/settings?botId=mybot",
        Some(json!({ "persona": "my bot persona" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["persona"], "my bot persona");
    let (_, secretary) = call(&app, "GET", "/api/client/settings", None).await;
    assert_eq!(secretary["persona"], "");
}
