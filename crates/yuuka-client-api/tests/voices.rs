//! `/api/client/voices*`（PWA の音声チャットの声）の HTTP 統合テスト（実 SQLite・実ルータ）。
//!
//! 検証: 未認証は 401・作成した声が一覧と本体（生バイト列）で返る・名前と話し方を変えられる・
//! 他人の声は見えず変更も削除もできない（404）・話者条件の大きさが mask と合わなければ 400・
//! `users` 行の無いユーザーには作らない（404）・保存数の上限で 409。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use base64::Engine as _;
use secrecy::SecretString;
use serde_json::{json, Value};
use tower::ServiceExt;
use yuuka_client_api::voices::{MAX_VOICES_PER_USER, SPEAKER_DIM};
use yuuka_core::{AuthError, GeminiError};
use yuuka_discord::RateLimiter;
use yuuka_gemini::GenerateBackend;
use yuuka_orchestrator::{ChatEngine, GeminiFactory, InMemoryRateLimiter};
use yuuka_tools::ToolRegistry;
use yuuka_types::{Role, SessionUser};
use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

static SEQ: AtomicU64 = AtomicU64::new(0);

const ALICE: &str = "alice";
const BOB: &str = "bob";
/// 認証は通るが `users` 行が存在しないユーザー。
const GHOST: &str = "ghost";

fn fresh_db() -> (Db, std::path::PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "yuuka_client_api_voices_it_{}_{n}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    {
        rusqlite::Connection::open(&path).expect("seed file");
    }
    (Db::open(&path).expect("open db"), path)
}

struct FakeAuth;

#[async_trait]
impl AuthBackend for FakeAuth {
    async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok([ALICE, BOB, GHOST].contains(&token).then(|| SessionUser {
            discord_id: token.to_owned(),
            username: token.to_owned(),
            role: Role::User,
        }))
    }

    async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok(None)
    }
}

/// 声のルートはチャットエンジンを使わない。ルータ構築のために渡すだけの、常に失敗するファクトリ。
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

fn seed_user(path: &std::path::Path, discord_id: &str) {
    let conn = rusqlite::Connection::open(path).expect("open seed");
    conn.execute(
        "INSERT INTO users (discord_id, username, salt, role) VALUES (?1, ?1, '00', 'user')",
        rusqlite::params![discord_id],
    )
    .expect("seed user");
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

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("cookie", format!("__Host-yuuka-session={token}"));
    }
    let body = body.map_or_else(Body::empty, |v| Body::from(v.to_string()));
    let resp = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (
        status,
        to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

/// `tokens` 個の話者条件（中身はトークン番号で埋める）。
fn state_bytes(tokens: usize) -> Vec<u8> {
    (0..tokens * SPEAKER_DIM)
        .flat_map(|i| ((i / SPEAKER_DIM) as f32).to_le_bytes())
        .collect()
}

fn create_body(name: &str, tokens: usize) -> Value {
    json!({
        "name": name,
        "caption": "明るく",
        "source": "sample",
        "seconds": 12.5,
        "speakerModel": "irodori-v4.1-small",
        "mask": vec![true; tokens],
        "stateBase64": base64::engine::general_purpose::STANDARD.encode(state_bytes(tokens)),
    })
}

#[tokio::test]
async fn voices_require_authentication() {
    let (db, _path) = fresh_db();
    let app = app(db);
    let (status, _) = call(&app, "GET", "/api/client/voices", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(
        &app,
        "POST",
        "/api/client/voices",
        None,
        Some(create_body("x", 2)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_list_fetch_update_delete() {
    let (db, path) = fresh_db();
    seed_user(&path, ALICE);
    let app = app(db);

    let (status, body) = call(
        &app,
        "POST",
        "/api/client/voices",
        Some(ALICE),
        Some(create_body("  わたしの声  ", 3)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let created = json_of(&body);
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["name"], "わたしの声");
    assert_eq!(created["tokens"], 3);
    assert_eq!(created["mask"], json!([true, true, true]));
    assert_eq!(created["speakerModel"], "irodori-v4.1-small");
    assert!(created.get("stateBase64").is_none());

    let (status, body) = call(&app, "GET", "/api/client/voices", Some(ALICE), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_of(&body).as_array().unwrap().len(), 1);

    let (status, bytes) = call(
        &app,
        "GET",
        &format!("/api/client/voices/{id}/state"),
        Some(ALICE),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, state_bytes(3));

    let (status, body) = call(
        &app,
        "PATCH",
        &format!("/api/client/voices/{id}"),
        Some(ALICE),
        Some(json!({"caption": "ゆっくり"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_of(&body)["caption"], "ゆっくり");
    assert_eq!(json_of(&body)["name"], "わたしの声");

    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/api/client/voices/{id}"),
        Some(ALICE),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = call(&app, "GET", "/api/client/voices", Some(ALICE), None).await;
    assert_eq!(json_of(&body), json!([]));
}

#[tokio::test]
async fn other_users_cannot_see_or_touch_a_voice() {
    let (db, path) = fresh_db();
    seed_user(&path, ALICE);
    seed_user(&path, BOB);
    let app = app(db);
    let (_, body) = call(
        &app,
        "POST",
        "/api/client/voices",
        Some(ALICE),
        Some(create_body("alice", 2)),
    )
    .await;
    let id = json_of(&body)["id"].as_str().unwrap().to_owned();

    let (_, body) = call(&app, "GET", "/api/client/voices", Some(BOB), None).await;
    assert_eq!(json_of(&body), json!([]));
    for (method, uri, body) in [
        ("GET", format!("/api/client/voices/{id}/state"), None),
        (
            "PATCH",
            format!("/api/client/voices/{id}"),
            Some(json!({"name": "bob"})),
        ),
        ("DELETE", format!("/api/client/voices/{id}"), None),
    ] {
        let (status, _) = call(&app, method, &uri, Some(BOB), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
    let (_, body) = call(&app, "GET", "/api/client/voices", Some(ALICE), None).await;
    assert_eq!(json_of(&body)[0]["name"], "alice");
}

#[tokio::test]
async fn rejects_invalid_input() {
    let (db, path) = fresh_db();
    seed_user(&path, ALICE);
    let app = app(db);
    let mut wrong_size = create_body("x", 2);
    wrong_size["mask"] = json!([true, true, true]);
    let mut empty_name = create_body("x", 2);
    empty_name["name"] = json!("   ");
    let mut bad_source = create_body("x", 2);
    bad_source["source"] = json!("upload");
    for body in [wrong_size, empty_name, bad_source] {
        let (status, _) = call(&app, "POST", "/api/client/voices", Some(ALICE), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn user_without_row_gets_404() {
    let (db, _path) = fresh_db();
    let app = app(db);
    let (status, _) = call(
        &app,
        "POST",
        "/api/client/voices",
        Some(GHOST),
        Some(create_body("x", 1)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn limits_the_number_of_voices() {
    let (db, path) = fresh_db();
    seed_user(&path, ALICE);
    let app = app(db);
    for i in 0..MAX_VOICES_PER_USER {
        let (status, _) = call(
            &app,
            "POST",
            "/api/client/voices",
            Some(ALICE),
            Some(create_body(&format!("v{i}"), 1)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let (status, _) = call(
        &app,
        "POST",
        "/api/client/voices",
        Some(ALICE),
        Some(create_body("over", 1)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}
