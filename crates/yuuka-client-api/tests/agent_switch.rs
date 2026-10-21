//! PWA のエージェント切り替え（`/api/client/*?botId=`）の HTTP 統合テスト（実 SQLite・実ルータ）。
//!
//! 検証: `botId` ごとにデータ（Todo・共有ノート・ペルソナ）が分かれる・自分の Bot と `active` 共有の
//! Bot は使える・未指定/空/`system_default`（システム Bot）は 403・未知/他人の未共有/共有解除済みの
//! Bot はシステム Bot へフォールバックせず 404（どちらも何も書かない）。
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
        "INSERT INTO users (discord_id, username, salt, role) \
            VALUES ('u1', 'u1', '00', 'user'), ('u2', 'u2', '00', 'user');
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
    // 管理画面のドメイン API も同じルータに載せ、入口をまたいだ保存・検証を確かめる（D02 #126）。
    yuuka_client_api::routes_with(engine, rate_limiter)
        .merge(yuuka_todo::routes())
        .merge(yuuka_finance::routes())
        .merge(yuuka_schedule::routes())
        .merge(yuuka_personal::routes())
        .with_state(state)
}

/// エージェント（ツール）として `mybot` で呼ぶ文脈。
fn tool_ctx() -> yuuka_core::ToolContext {
    yuuka_core::ToolContext::new(yuuka_core::BotId::new("mybot"), yuuka_core::UserId::new(ME))
}

fn find_tool(tools: &[Arc<dyn yuuka_core::Tool>], name: &str) -> Arc<dyn yuuka_core::Tool> {
    tools
        .iter()
        .find(|t| t.declaration().name.as_str() == name)
        .cloned()
        .unwrap()
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
async fn data_is_separated_per_agent() {
    let (db, _path) = fresh_db();
    let app = app(db.clone());

    assert_eq!(
        add_todo(&app, "?botId=mybot", "my bot task").await,
        StatusCode::CREATED
    );
    assert_eq!(
        add_todo(&app, "?botId=sharedbot", "shared task").await,
        StatusCode::CREATED
    );

    assert_eq!(todo_titles(&app, "?botId=mybot").await, ["my bot task"]);
    assert_eq!(todo_titles(&app, "?botId=sharedbot").await, ["shared task"]);
    let jobs = yuuka_todo::repo::TodoRepo::new(&db)
        .claim_tag_jobs(yuuka_core::CrossUserAccess::for_scheduled_task())
        .await
        .unwrap();
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().all(|job| job.scope.user_id().as_str() == ME));
    assert!(jobs
        .iter()
        .any(|job| job.scope.bot_id().as_str() == "mybot"));
    assert!(jobs
        .iter()
        .any(|job| job.scope.bot_id().as_str() == "sharedbot"));
}

#[tokio::test]
async fn system_bot_is_forbidden() {
    let (db, path) = fresh_db();
    let app = app(db);

    // 未指定・空・明示の system_default はすべてシステム Bot 宛て＝ PWA では扱わない。
    for query in ["", "?botId=", "?botId=%20", "?botId=system_default"] {
        let (status, _) = call(&app, "GET", &format!("/api/client/todos{query}"), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "GET todos {query:?}");
        assert_eq!(
            add_todo(&app, query, "leak").await,
            StatusCode::FORBIDDEN,
            "POST todos {query:?}"
        );
        for uri in ["/api/client/chat/messages", "/api/client/settings"] {
            let (status, _) = call(&app, "GET", &format!("{uri}{query}"), None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "GET {uri}{query:?}");
        }
    }
    assert_eq!(todo_count(&path), 0, "拒否したリクエストは何も書かない");
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
    let (_, shared) = call(&app, "GET", "/api/client/shared-note?botId=sharedbot", None).await;
    assert_eq!(mine["body"], "my bot note");
    assert_eq!(shared["body"], "");

    let (status, saved) = call(
        &app,
        "PUT",
        "/api/client/settings?botId=mybot",
        Some(json!({ "persona": "my bot persona" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["persona"], "my bot persona");
    let (_, shared) = call(&app, "GET", "/api/client/settings?botId=sharedbot", None).await;
    assert_eq!(shared["persona"], "");
}

#[tokio::test]
async fn calendar_event_can_be_added_from_the_pwa() {
    let (db, _path) = fresh_db();
    let app = app(db);

    let (status, created) = call(
        &app,
        "POST",
        "/api/client/calendar/events?botId=mybot",
        Some(json!({ "title": "歯医者", "startsAt": "2026-10-06T09:30", "endsAt": "2026-10-06T10:00" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["title"], "歯医者");
    // ローカル暦（JST）09:30 は UTC 00:30。
    assert_eq!(created["startsAt"], "2026-10-06T00:30:00.000Z");
    assert_eq!(created["endsAt"], "2026-10-06T01:00:00.000Z");

    // 選択中エージェントの一覧にだけ出る。
    let (_, mine) = call(
        &app,
        "GET",
        "/api/client/calendar/events?botId=mybot&from=2026-10-01&to=2026-10-31",
        None,
    )
    .await;
    assert_eq!(mine.as_array().unwrap().len(), 1, "{mine}");
    let (_, shared) = call(
        &app,
        "GET",
        "/api/client/calendar/events?botId=sharedbot&from=2026-10-01&to=2026-10-31",
        None,
    )
    .await;
    assert_eq!(shared.as_array().unwrap().len(), 0, "{shared}");

    // 入力不足・不正は 400。
    for body in [
        json!({ "title": "", "startsAt": "2026-10-06T09:30" }),
        json!({ "title": "x" }),
        json!({ "title": "x", "startsAt": "2026-02-30T09:30" }),
        json!({ "title": "x", "startsAt": "2026-10-06T09:30", "endsAt": "2026-10-06T09:00" }),
    ] {
        let (status, _) = call(
            &app,
            "POST",
            "/api/client/calendar/events?botId=mybot",
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
}

#[tokio::test]
async fn receipt_scan_validates_input_before_calling_the_llm() {
    let (db, _path) = fresh_db();
    let app = app(db);
    let image = "aGVsbG8=";

    // 画像・形式の検証。
    for body in [
        json!({ "mimeType": "image/jpeg" }),
        json!({ "imageBase64": image }),
        json!({ "imageBase64": image, "mimeType": "application/pdf" }),
    ] {
        let (status, _) = call(
            &app,
            "POST",
            "/api/client/finance/receipt?botId=mybot",
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    // Gemini キー未設定は案内文付きの 400（LLM は呼ばない）。
    let (status, body) = call(
        &app,
        "POST",
        "/api/client/finance/receipt?botId=mybot",
        Some(json!({ "imageBase64": image, "mimeType": "image/jpeg; charset=binary" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("Gemini APIキー"),
        "{body}"
    );
}

// ─── D02 #126: 管理 API・PWA・ツールの 3 入口が同じ業務サービスを通る ─────────────────

#[tokio::test]
async fn todo_entrances_share_storage_and_validation_but_keep_their_wire_contracts() {
    let (db, _path) = fresh_db();
    let tools = yuuka_todo::tools(db.clone()).unwrap();
    let app = app(db);

    let out = find_tool(&tools, "addTodo")
        .call(&tool_ctx(), json!({ "title": "tool task" }))
        .await
        .unwrap();
    assert_eq!(out.payload["success"], true);

    // 管理画面: `{success, task}`・数値 ID。
    let (status, admin) = call(
        &app,
        "POST",
        "/api/tasks/add",
        Some(json!({ "botId": "mybot", "title": "admin task", "dueDate": "2026-10-20" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{admin}");
    assert_eq!(admin["success"], true);
    let admin_id = admin["task"]["id"].as_i64().unwrap();

    // PWA: 201・素の JSON・文字列 ID・`completed`。
    let (status, pwa) = call(
        &app,
        "POST",
        "/api/client/todos?botId=mybot",
        Some(json!({ "title": "pwa task", "list": "仕事" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{pwa}");
    assert!(pwa["id"].is_string());
    assert_eq!(pwa["completed"], false);
    assert_eq!(pwa["list"], "仕事");
    assert!(pwa.get("success").is_none());

    assert_eq!(
        todo_titles(&app, "?botId=mybot").await.len(),
        3,
        "3 入口の ToDo は同じ一覧に載る"
    );

    // PWA の完了は管理画面の完了と同じ状態になる。戻すと open。
    let patch = format!("/api/client/todos/{admin_id}?botId=mybot");
    let (_, done) = call(&app, "PATCH", &patch, Some(json!({ "completed": true }))).await;
    assert_eq!(done["completed"], true);
    let (_, admin_done) = call(&app, "GET", "/api/tasks?botId=mybot&status=done", None).await;
    assert_eq!(admin_done["tasks"][0]["title"], "admin task");
    let (_, reopened) = call(&app, "PATCH", &patch, Some(json!({ "completed": false }))).await;
    assert_eq!(reopened["completed"], false);

    // 同じ検証が両方の HTTP 入口で行を書く前に効く（文言も同じ）。
    for (uri, input) in [
        ("/api/tasks/add", json!({ "botId": "mybot", "title": " " })),
        ("/api/client/todos?botId=mybot", json!({ "title": " " })),
    ] {
        let (status, error) = call(&app, "POST", uri, Some(input)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {error}");
        assert_eq!(error["message"], "title is required");
    }
    let (status, error) = call(
        &app,
        "POST",
        "/api/tasks/add",
        Some(json!({ "botId": "mybot", "title": "routine", "repeatRule": "0 0 30 2 *" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(todo_titles(&app, "?botId=mybot").await.len(), 3);
}

#[tokio::test]
async fn expense_entrances_share_totals_and_keep_signed_admin_adjustments() {
    let (db, _path) = fresh_db();
    let tools = yuuka_finance::tools(db.clone()).unwrap();
    let app = app(db);
    let add = find_tool(&tools, "addExpense");

    let out = add
        .call(
            &tool_ctx(),
            json!({ "amount": 100, "category": "食費", "date": "2026-10-20" }),
        )
        .await
        .unwrap();
    assert_eq!(out.payload["success"], true);

    // 管理画面は従来どおり負の調整を受け付ける（0 は不可）。
    let (status, admin) = call(
        &app,
        "POST",
        "/api/expenses/add",
        Some(json!({ "botId": "mybot", "amount": -20, "category": "食費", "date": "2026-10-20" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{admin}");
    assert!(admin["expense"]["id"].is_number());

    let (status, pwa) = call(
        &app,
        "POST",
        "/api/client/finance/transactions?botId=mybot",
        Some(json!({ "amount": 500, "category": "給与", "kind": "income", "date": "2026-10-20" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{pwa}");
    assert!(pwa["id"].is_string());
    assert_eq!(pwa["kind"], "income");

    let (_, totals) = call(
        &app,
        "GET",
        "/api/client/finance/summary?botId=mybot&month=2026-10",
        None,
    )
    .await;
    assert_eq!(totals["expense"], 80);
    assert_eq!(totals["income"], 500);
    assert_eq!(totals["balance"], 420);

    // 利用者の取引登録（PWA・ツール）は 1 円以上だけ。どちらも何も書かない。
    for amount in [json!(-20), json!(0), json!(0.5)] {
        let (status, error) = call(
            &app,
            "POST",
            "/api/client/finance/transactions?botId=mybot",
            Some(json!({ "amount": amount, "category": "食費" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{amount}: {error}");
        assert_eq!(error["message"], "amount and category are required");
    }
    let out = add
        .call(&tool_ctx(), json!({ "amount": -20, "category": "食費" }))
        .await
        .unwrap();
    assert_eq!(out.payload["success"], false);
    for (uri, input) in [
        (
            "/api/expenses/add",
            json!({ "botId": "mybot", "amount": 0, "category": "食費" }),
        ),
        (
            "/api/client/finance/transactions?botId=mybot",
            json!({ "amount": 100, "category": " " }),
        ),
    ] {
        let (status, error) = call(&app, "POST", uri, Some(input)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {error}");
        assert_eq!(error["message"], "amount and category are required");
    }
    let (_, rows) = call(
        &app,
        "GET",
        "/api/client/finance/transactions?botId=mybot&month=2026-10",
        None,
    )
    .await;
    assert_eq!(rows.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn note_entrances_keep_the_pwa_title_and_share_the_utf16_limit() {
    let (db, _path) = fresh_db();
    let tools = yuuka_personal::tools(db.clone()).unwrap();
    let app = app(db);
    let set = find_tool(&tools, "setContextNote");

    let (status, pwa) = call(
        &app,
        "PUT",
        "/api/client/shared-note?botId=mybot",
        Some(json!({ "title": "私のノート", "body": "PWA body" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pwa}");
    assert_eq!(pwa["title"], "私のノート");

    let (_, admin) = call(&app, "GET", "/api/context-note?botId=mybot", None).await;
    assert_eq!(admin["success"], true);
    assert_eq!(admin["content"], "PWA body");

    // 管理画面・ツールの保存は本文だけを書き換え、PWA のタイトルは残る。
    let out = set
        .call(&tool_ctx(), json!({ "content": "tool body" }))
        .await
        .unwrap();
    assert_eq!(out.payload["success"], true);
    let (_, note) = call(&app, "GET", "/api/client/shared-note?botId=mybot", None).await;
    assert_eq!(note["title"], "私のノート");
    assert_eq!(note["body"], "tool body");

    // 上限は全入口で UTF-16 の長さ（絵文字 5,001 個 = 10,002）。どの入口でも保存しない。
    let over_limit = "😀".repeat(5_001);
    let out = set
        .call(&tool_ctx(), json!({ "content": over_limit }))
        .await
        .unwrap();
    assert_eq!(out.payload["success"], false);
    assert_eq!(
        out.payload["message"],
        "コンテキストノートは10,000文字以内です（現在: 10,002文字）"
    );
    let (status, error) = call(
        &app,
        "POST",
        "/api/context-note",
        Some(json!({ "botId": "mybot", "content": over_limit })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(
        error["message"],
        "コンテキストノートは10,000文字以内です（現在: 10,002文字）"
    );
    let (status, error) = call(
        &app,
        "PUT",
        "/api/client/shared-note?botId=mybot",
        Some(json!({ "title": "x", "body": over_limit })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["message"], "shared note exceeds 10000 chars");

    let (_, note) = call(&app, "GET", "/api/client/shared-note?botId=mybot", None).await;
    assert_eq!(note["title"], "私のノート");
    assert_eq!(note["body"], "tool body", "拒否した保存はノートを変えない");
}

#[tokio::test]
async fn schedule_entrances_share_validation_and_reminder_defaults() {
    let (db, _path) = fresh_db();
    let tools = yuuka_schedule::tools(db.clone()).unwrap();
    let app = app(db);

    let out = find_tool(&tools, "addSchedule")
        .call(
            &tool_ctx(),
            json!({ "title": "tool schedule", "start_at": "2026-10-20T10:00:00" }),
        )
        .await
        .unwrap();
    assert_eq!(out.payload["success"], true);

    let (status, admin) = call(
        &app,
        "POST",
        "/api/schedules/add",
        Some(json!({ "botId": "mybot", "title": "admin schedule", "startAt": "2026-10-20 11:00:00" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{admin}");
    assert_eq!(admin["schedule"]["remind_before_minutes"], 10);

    let (status, pwa) = call(
        &app,
        "POST",
        "/api/client/calendar/events?botId=mybot",
        Some(json!({ "title": "pwa schedule", "startsAt": "2026-10-20T12:00" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{pwa}");
    assert!(pwa["id"].is_string());

    for (uri, input) in [
        (
            "/api/schedules/add",
            json!({ "botId": "mybot", "title": " ", "startAt": "2026-10-20 13:00:00" }),
        ),
        (
            "/api/client/calendar/events?botId=mybot",
            json!({ "title": " ", "startsAt": "2026-10-20T13:00" }),
        ),
    ] {
        let (status, error) = call(&app, "POST", uri, Some(input)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {error}");
        assert_eq!(error["message"], "title is required");
    }

    let (status, all) = call(
        &app,
        "GET",
        "/api/client/calendar/events?botId=mybot&from=2026-10-20&to=2026-10-20",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(all.as_array().unwrap().len(), 3);
}
