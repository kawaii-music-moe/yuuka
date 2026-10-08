//! `GET`/`PUT /api/client/settings` の HTTP 統合テスト（issue #39・実 SQLite・実ルータ）。
//!
//! 検証: 応答に `maxTokens`/`temperature` が含まれない（永続化されない偽の設定値の削除）・旧クライアントが
//! それらを PUT しても 400 にならず無視される・モデルは `ALLOWED_MODELS` 以外を 400 で拒否・`users` 行が
//! 無いユーザーの PUT は「保存した」と偽らず 404（何も書かない）・PUT の応答は永続化された状態と一致し
//! 直後の GET と等しい。
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
use yuuka_gemini::{GenerateBackend, ALLOWED_MODELS, DEFAULT_MODEL};
use yuuka_orchestrator::{ChatEngine, GeminiFactory, InMemoryRateLimiter};
use yuuka_persona::dto::PERSONA_MAX_LENGTH;
use yuuka_tools::ToolRegistry;
use yuuka_types::{Role, SessionUser};
use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// `users` 行を持つユーザー（設定の読み書きが可能）。
const USER_WITH_ROW: &str = "u1";
/// 認証は通るが `users` 行が存在しないユーザー（削除済み・未登録の残存セッション相当）。
const USER_WITHOUT_ROW: &str = "ghost";

fn fresh_db() -> (Db, std::path::PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "yuuka_client_api_settings_it_{}_{n}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    {
        rusqlite::Connection::open(&path).expect("seed file");
    }
    (Db::open(&path).expect("open db"), path)
}

/// Cookie トークンが既知ユーザーのものだけ認証する fake バックエンド。
struct FakeAuth;

#[async_trait]
impl AuthBackend for FakeAuth {
    async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok(
            (token == USER_WITH_ROW || token == USER_WITHOUT_ROW).then(|| SessionUser {
                discord_id: token.to_owned(),
                username: token.to_owned(),
                role: Role::User,
            }),
        )
    }

    async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok(None)
    }
}

/// 設定ルートはチャットエンジンを使わない。ルータ構築のために渡すだけの、常に失敗するファクトリ。
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

fn seed_user(path: &std::path::Path, discord_id: &str, model: Option<&str>) {
    let conn = rusqlite::Connection::open(path).expect("open seed");
    conn.execute(
        "INSERT INTO users (discord_id, username, password_hash, salt, role, gemini_model) \
         VALUES (?1, ?1, 'x', '00', 'user', ?2)",
        rusqlite::params![discord_id, model],
    )
    .expect("seed user");
    // PWA はシステム Bot を扱わないため、設定はユーザー本人の Bot（`bot_<id>`）経由で読み書きする。
    conn.execute(
        "INSERT INTO bots (id, user_id, name) VALUES ('bot_' || ?1, ?1, 'Mine')",
        rusqlite::params![discord_id],
    )
    .expect("seed bot");
}

fn stored_model(path: &std::path::Path, discord_id: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(path).expect("open");
    conn.query_row(
        "SELECT gemini_model FROM users WHERE discord_id = ?1",
        rusqlite::params![discord_id],
        |r| r.get(0),
    )
    .expect("row")
}

fn count(path: &std::path::Path, sql: &str, id: &str) -> i64 {
    let conn = rusqlite::Connection::open(path).expect("open");
    conn.query_row(sql, rusqlite::params![id], |r| r.get(0))
        .expect("count")
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
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let uri = match token {
        Some(token) => format!("/api/client/settings?botId=bot_{token}"),
        None => "/api/client/settings".to_owned(),
    };
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
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

async fn get(app: &Router, token: &str) -> (StatusCode, Value) {
    call(app, "GET", Some(token), None).await
}

async fn put(app: &Router, token: &str, body: Value) -> (StatusCode, Value) {
    call(app, "PUT", Some(token), Some(body)).await
}

fn assert_no_generation_fields(v: &Value) {
    let obj = v.as_object().expect("settings is an object");
    for key in ["maxTokens", "temperature", "max_tokens"] {
        assert!(!obj.contains_key(key), "{key} は応答に含まれない: {v}");
    }
}

#[tokio::test]
async fn settings_require_authentication() {
    let (db, _path) = fresh_db();
    let app = app(db);
    let (status, _) = call(&app, "GET", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(&app, "PUT", None, Some(json!({"model": "gemini-2.5-pro"}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_returns_persisted_state_without_generation_fields() {
    let (db, path) = fresh_db();
    seed_user(&path, USER_WITH_ROW, Some("gemini-2.5-pro"));
    let app = app(db);

    let (status, body) = get(&app, USER_WITH_ROW).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["model"], "gemini-2.5-pro");
    assert_eq!(body["googleConnected"], false);
    assert_eq!(body["persona"], "");
    assert_no_generation_fields(&body);
}

#[tokio::test]
async fn get_falls_back_to_default_model_when_column_is_null() {
    let (db, path) = fresh_db();
    seed_user(&path, USER_WITH_ROW, None);
    let app = app(db);

    let (status, body) = get(&app, USER_WITH_ROW).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["model"], DEFAULT_MODEL);
}

#[tokio::test]
async fn put_persists_model_and_get_returns_it() {
    let (db, path) = fresh_db();
    seed_user(&path, USER_WITH_ROW, Some(DEFAULT_MODEL));
    let app = app(db);

    let (status, put_body) = put(&app, USER_WITH_ROW, json!({ "model": "gemini-2.5-flash" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(put_body["model"], "gemini-2.5-flash");
    assert_no_generation_fields(&put_body);
    assert_eq!(
        stored_model(&path, USER_WITH_ROW).as_deref(),
        Some("gemini-2.5-flash"),
        "DB に保存されている"
    );

    let (status, get_body) = get(&app, USER_WITH_ROW).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(get_body, put_body, "PUT の応答は直後の GET と一致する");
}

#[tokio::test]
async fn put_accepts_every_allowed_model() {
    let (db, path) = fresh_db();
    seed_user(&path, USER_WITH_ROW, None);
    let app = app(db);

    for model in ALLOWED_MODELS {
        let (status, body) = put(&app, USER_WITH_ROW, json!({ "model": model })).await;
        assert_eq!(status, StatusCode::OK, "{model}");
        assert_eq!(body["model"], *model);
        assert_eq!(stored_model(&path, USER_WITH_ROW).as_deref(), Some(*model));
    }
}

#[tokio::test]
async fn put_rejects_model_outside_allowlist_and_keeps_stored_value() {
    let (db, path) = fresh_db();
    seed_user(&path, USER_WITH_ROW, Some("gemini-2.5-pro"));
    let app = app(db);

    for bad in ["GPT-4o", "gpt-4.1", "Claude", "gemini-1.0-nonexistent"] {
        let (status, body) = put(&app, USER_WITH_ROW, json!({ "model": bad })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|m| m.contains("model must be one of")),
            "許可リストを案内するメッセージ: {body}"
        );
    }
    assert_eq!(
        stored_model(&path, USER_WITH_ROW).as_deref(),
        Some("gemini-2.5-pro"),
        "拒否されたモデルは保存されない"
    );
    let (_, body) = get(&app, USER_WITH_ROW).await;
    assert_eq!(body["model"], "gemini-2.5-pro");
}

#[tokio::test]
async fn put_ignores_legacy_generation_keys_from_cached_clients() {
    let (db, path) = fresh_db();
    seed_user(&path, USER_WITH_ROW, Some(DEFAULT_MODEL));
    let app = app(db);

    // 旧 PWA は `AgentSettings` 全体（maxTokens/temperature 含む）を PUT する。
    let (status, put_body) = put(
        &app,
        USER_WITH_ROW,
        json!({
            "googleConnected": true,
            "googleAccount": "someone@example.com",
            "model": "gemini-2.5-pro",
            "maxTokens": 4096,
            "temperature": 1.5,
            "persona": "丁寧に答える",
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "未知キーで 400 にならない: {put_body}"
    );
    assert_eq!(put_body["model"], "gemini-2.5-pro");
    assert_eq!(put_body["persona"], "丁寧に答える");
    assert_eq!(
        put_body["googleConnected"], false,
        "クライアント申告の連携状態は保存・反映されない"
    );
    assert_no_generation_fields(&put_body);

    let (_, get_body) = get(&app, USER_WITH_ROW).await;
    assert_eq!(get_body, put_body);
    assert_no_generation_fields(&get_body);
}

#[tokio::test]
async fn put_without_model_keeps_stored_model() {
    let (db, path) = fresh_db();
    seed_user(&path, USER_WITH_ROW, Some("gemini-2.5-pro"));
    let app = app(db);

    let (status, body) = put(&app, USER_WITH_ROW, json!({ "persona": "簡潔に" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["model"], "gemini-2.5-pro");
    assert_eq!(body["persona"], "簡潔に");
    assert_eq!(
        stored_model(&path, USER_WITH_ROW).as_deref(),
        Some("gemini-2.5-pro")
    );

    let (status, body) = put(&app, USER_WITH_ROW, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["model"], "gemini-2.5-pro");
    assert_eq!(body["persona"], "簡潔に");
}

#[tokio::test]
async fn put_with_too_long_persona_is_rejected_without_partial_model_update() {
    let (db, path) = fresh_db();
    seed_user(&path, USER_WITH_ROW, Some(DEFAULT_MODEL));
    let app = app(db);

    let too_long = "あ".repeat(PERSONA_MAX_LENGTH + 1);
    let (status, _) = put(
        &app,
        USER_WITH_ROW,
        json!({ "model": "gemini-2.5-pro", "persona": too_long }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        stored_model(&path, USER_WITH_ROW).as_deref(),
        Some(DEFAULT_MODEL),
        "persona 検証失敗時にモデルだけ更新されない"
    );
}

#[tokio::test]
async fn put_without_users_row_is_explicit_error_and_writes_nothing() {
    let (db, path) = fresh_db();
    let app = app(db);

    // モデル更新: 「保存した」と偽らず 404。
    let (status, body) = put(&app, USER_WITHOUT_ROW, json!({ "model": "gemini-2.5-pro" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["message"].is_string());
    assert!(
        body.get("model").is_none(),
        "保存されなかった値を応答に含めない: {body}"
    );

    // persona のみ・空 body も同様（行が無いユーザーには何も書かない）。
    let (status, _) = put(&app, USER_WITHOUT_ROW, json!({ "persona": "x" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = put(&app, USER_WITHOUT_ROW, json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 行は作られず、ペルソナも作られず、以後の GET は保存されていない値を返さない。
    assert_eq!(
        count(
            &path,
            "SELECT COUNT(*) FROM users WHERE discord_id = ?1",
            USER_WITHOUT_ROW
        ),
        0,
        "users 行を勝手に作らない"
    );
    assert_eq!(
        count(
            &path,
            "SELECT COUNT(*) FROM personas WHERE owner_id = ?1",
            USER_WITHOUT_ROW
        ),
        0,
        "persona も作られない"
    );
    // `users` 行の無いユーザーは Bot も持てない（外部キー）ため、どの Bot 宛ても 404。
    let (status, _) = get(&app, USER_WITHOUT_ROW).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// PWA のモデル選択肢（`client/pwa/src/api/models.ts`）がサーバーの許可リストと一致する。
/// 差分があると、選べるのに 400 になる／サーバーが許すのに選べないモデルが生まれる（issue #39）。
#[test]
fn pwa_model_list_matches_server_allowlist() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../client/pwa/src/api/models.ts");
    let src = std::fs::read_to_string(&path).expect("read client/pwa/src/api/models.ts");
    let list = src
        .split("GEMINI_MODELS = [")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("GEMINI_MODELS array literal");
    let pwa: Vec<&str> = list.split('\'').skip(1).step_by(2).collect();
    assert_eq!(
        pwa, ALLOWED_MODELS,
        "PWA の GEMINI_MODELS と ALLOWED_MODELS の順序・内容が一致する"
    );

    let default_line = src
        .lines()
        .find(|l| l.contains("DEFAULT_GEMINI_MODEL"))
        .expect("DEFAULT_GEMINI_MODEL");
    assert!(
        default_line.contains(&format!("'{DEFAULT_MODEL}'")),
        "PWA の既定モデルはサーバーの DEFAULT_MODEL と一致する: {default_line}"
    );
}
