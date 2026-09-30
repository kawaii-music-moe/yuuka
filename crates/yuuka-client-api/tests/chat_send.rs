//! `POST`/`GET /api/client/chat/messages` の HTTP 統合テスト（issue #41 PR #75 レビュー・fake Gemini
//! backend・実 SQLite）。
//!
//! 検証: 認証必須・事前チェック（空 content・レート制限・Gemini キー未設定）は同期のまま 4xx を返す・
//! `202 Accepted` が即座に返る（バックグラウンドターンを待たない）・ポーリング（`GET` 履歴）が完了応答へ
//! 到達する・バックグラウンドターン失敗が終端状態（エラー応答）として保存されポーリングが止まる・
//! 同一ユーザーの同時ターンは 409。ネットワーク不要（`GeminiFactory` を fake backend に差し替え、
//! 実 API は一切呼ばない）。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use secrecy::SecretString;
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tower::ServiceExt;
use yuuka_core::{AuthError, GeminiError};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::RateLimiter;
use yuuka_gemini::{
    Content, FunctionDeclaration, GenerateBackend, GenerateContentResponse, ToolConfig,
};
use yuuka_orchestrator::{message_log, ChatEngine, GeminiFactory, InMemoryRateLimiter};
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

/// Cookie トークンが既知ユーザーのものだけ認証する fake バックエンド。
struct FakeAuth;

#[async_trait]
impl AuthBackend for FakeAuth {
    async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok((token == "u1" || token == "u2").then(|| SessionUser {
            discord_id: token.to_owned(),
            username: token.to_owned(),
            role: Role::User,
        }))
    }

    async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok(None)
    }
}

/// 固定テキストを 1 度だけ返す fake backend（tools 無しの単発ターン）。
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

fn text_response(text: &str) -> GenerateContentResponse {
    serde_json::from_value(json!({
        "candidates": [ { "content": { "role": "model", "parts": [ { "text": text } ] } } ]
    }))
    .expect("valid response")
}

impl GeminiFactory for FakeFactory {
    fn build(
        &self,
        _model: &str,
        _api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        let mut q = VecDeque::new();
        q.push_back(text_response(&self.text));
        Ok(Arc::new(FakeBackend {
            responses: Mutex::new(q),
        }))
    }
}

/// `generate()` が `release`（許可数 0 の Semaphore）に許可が追加されるまで pending のまま返らない fake backend。バックグラウンド
/// ターンが「確実にまだ進行中」の状態を、スケジューリングの偶然に頼らず決定的に作るために使う
/// （issue #41 PR #75 レビュー・P1: 202 の即時性・同時ターン 409 のテスト）。
struct GatedBackend {
    release: Arc<Semaphore>,
    text: String,
}

#[async_trait]
impl GenerateBackend for GatedBackend {
    async fn generate(
        &self,
        _system_instruction: Option<&str>,
        _declarations: &[FunctionDeclaration],
        _contents: &[Content],
        _tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        self.release
            .acquire()
            .await
            .expect("semaphore open")
            .forget();
        Ok(text_response(&self.text))
    }
}

struct GatedFactory {
    release: Arc<Semaphore>,
    text: String,
}

impl GeminiFactory for GatedFactory {
    fn build(
        &self,
        _model: &str,
        _api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        Ok(Arc::new(GatedBackend {
            release: self.release.clone(),
            text: self.text.clone(),
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
         VALUES (?1, ?1, 'x', '00', 'user', ?2, ?3, ?4, 'gemini-3.1-flash-lite')",
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

/// `users` 行を Gemini キーは設定済みだが**復号できない**状態で seed する（crypto=None のエンジンと
/// 組み合わせ、事前チェック（行の存在のみ確認）は通過しつつバックグラウンドターンが確実に失敗する
/// 状況を作る・「バックグラウンド失敗が終端状態として保存される」テスト用）。
fn seed_user_with_key_but_engine_has_no_crypto(path: &std::path::Path, discord_id: &str) {
    let conn = rusqlite::Connection::open(path).expect("open seed");
    conn.execute(
        "INSERT INTO users (discord_id, username, password_hash, salt, role, \
         gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag, gemini_model) \
         VALUES (?1, 'yuu', 'x', '00', 'user', 'enc', 'iv', 'tag', 'gemini-3.1-flash-lite')",
        rusqlite::params![discord_id],
    )
    .expect("seed user");
}

fn engine_with_factory(
    db: Db,
    crypto: Option<Arc<SystemCrypto>>,
    factory: Arc<dyn GeminiFactory>,
) -> Arc<ChatEngine> {
    Arc::new(ChatEngine::new(
        db,
        crypto,
        ToolRegistry::new(),
        factory,
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        None,
    ))
}

fn engine_with(db: Db, crypto: Option<Arc<SystemCrypto>>, text: &str) -> Arc<ChatEngine> {
    engine_with_factory(
        db,
        crypto,
        Arc::new(FakeFactory {
            text: text.to_owned(),
        }),
    )
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

async fn get_history(app: &Router, token: &str) -> Vec<Value> {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/client/chat/messages")
                .header("cookie", format!("__Host-yuuka-session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let history: Value = serde_json::from_slice(&bytes).unwrap();
    history.as_array().unwrap().clone()
}

/// `since_id` より大きい id を持つ `role:"agent"` の行が現れるまで履歴をポーリングする（issue #41
/// PR #75 レビュー・P1 のポーリング契約そのもの）。見つからないまま `max_iters` を使い切ったら panic。
async fn poll_for_reply_after(app: &Router, token: &str, since_id: i64, max_iters: u32) -> Value {
    for _ in 0..max_iters {
        let history = get_history(app, token).await;
        if let Some(msg) = history.iter().find(|m| {
            m["role"] == "agent"
                && m["id"]
                    .as_str()
                    .and_then(|s| s.parse::<i64>().ok())
                    .is_some_and(|id| id > since_id)
        }) {
            return msg.clone();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("since_id={since_id} より後の agent 応答が時間内に現れなかった");
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

    // 事前チェックで弾かれ background ターンまで到達しない＝ユーザー発言すら保存されない。
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
    // 各回、次を送る前に完了を待つ（同時ターン防止ゲートに引っかからないようにする・issue #41
    // PR #75 レビュー・P1 の 409 とは別の関心事なのでここでは直列に送る）。
    for i in 0..5 {
        let (status, body) = post_chat(&app, Some("u1"), &format!("msg {i}")).await;
        assert_eq!(status, StatusCode::ACCEPTED, "iteration {i}: body={body:?}");
        let since_id: i64 = body["sinceId"].as_str().unwrap().parse().unwrap();
        poll_for_reply_after(&app, "u1", since_id, 200).await;
    }
    // 6 回目は 429（WS/Discord と同じ `rate_limit_message` 文言）。事前チェックなので同期で返る。
    let (status, body) = post_chat(&app, Some("u1"), "one more").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        body["message"].as_str().unwrap().contains("利用ペース"),
        "body={body:?}"
    );
}

#[tokio::test]
async fn send_returns_202_immediately_without_waiting_for_the_turn() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    // GatedBackend は release されるまで永遠に generate() から戻らない＝もし chat_send がターン完了を
    // 待つ実装だったらこのテストはタイムアウトする。
    let release = Arc::new(Semaphore::new(0));
    let engine = engine_with_factory(
        db.clone(),
        Some(crypto),
        Arc::new(GatedFactory {
            release: release.clone(),
            text: "ゆっくり返します".to_owned(),
        }),
    );
    let app = app(engine, db);

    let started = Instant::now();
    let (status, body) = post_chat(&app, Some("u1"), "hello").await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["status"], "pending");
    assert!(body["sinceId"].as_str().is_some());
    assert!(
        elapsed < Duration::from_millis(500),
        "202 はバックグラウンドターンの完了を待たずに即座に返るはず（elapsed={elapsed:?}）"
    );

    // 後片付け＋ポーリングが最終的に完了へ到達することも合わせて確認する。
    release.add_permits(1);
    let since_id: i64 = body["sinceId"].as_str().unwrap().parse().unwrap();
    let reply = poll_for_reply_after(&app, "u1", since_id, 200).await;
    assert_eq!(reply["content"], "ゆっくり返します");
}

#[tokio::test]
async fn poll_reaches_the_completed_reply_via_chat_history() {
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
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["status"], "pending");
    let since_id: i64 = body["sinceId"].as_str().unwrap().parse().unwrap();

    let reply = poll_for_reply_after(&app, "u1", since_id, 200).await;
    assert_eq!(reply["role"], "agent");
    assert_eq!(reply["content"], "こんにちは、PWAからのメッセージですね！");
    assert!(reply["id"].as_str().is_some());
    assert!(reply["createdAt"].as_str().is_some());

    // ユーザー発言・アシスタント応答の両方が source='pwa' で永続化されている（`secretary_turn_pwa`
    // が内部で行う・chat_send はここに追加で書き込まない）。
    let conn = rusqlite::Connection::open(&path).unwrap();
    let user_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_logs \
             WHERE user_id='u1' AND role='user' AND content='こんにちは' AND source='pwa'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(user_count, 1);
    let assistant_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_logs \
             WHERE user_id='u1' AND role='assistant' \
               AND content='こんにちは、PWAからのメッセージですね！' AND source='pwa'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(assistant_count, 1);

    // GET 履歴にも 2 件（ユーザー・エージェント）とも反映されている。
    let history = get_history(&app, "u1").await;
    assert_eq!(history.len(), 2, "history={history:?}");
    assert_eq!(history[0]["role"], "user");
    assert_eq!(history[1]["role"], "agent");
}

#[tokio::test]
async fn background_failure_persists_a_terminal_error_reply() {
    let (db, path) = fresh_db();
    // 行は存在する（事前チェック user_has_gemini_key は通過）が、エンジンの crypto が None のため
    // バックグラウンドターン側の鍵復号ステップで確実に失敗する（issue #41 PR #75 レビュー・P1:
    // 「失敗/タイムアウトは終端状態として保存される」ことの検証）。
    seed_user_with_key_but_engine_has_no_crypto(&path, "u1");
    let engine = engine_with(db.clone(), None, "unused");
    let app = app(engine, db);

    let (status, body) = post_chat(&app, Some("u1"), "こんにちは").await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "事前チェックは通過し 202 が返る"
    );
    let since_id: i64 = body["sinceId"].as_str().unwrap().parse().unwrap();

    let reply = poll_for_reply_after(&app, "u1", since_id, 200).await;
    assert_eq!(reply["role"], "agent");
    let content = reply["content"].as_str().unwrap();
    assert!(
        content.contains("エラー"),
        "失敗は固定のエラー文言で終端化される: content={content:?}"
    );

    // 二重に保存されていない（フォールバック応答はちょうど 1 件）。
    let history = get_history(&app, "u1").await;
    let agent_replies = history.iter().filter(|m| m["role"] == "agent").count();
    assert_eq!(agent_replies, 1);
}

#[tokio::test]
async fn concurrent_send_for_the_same_user_is_rejected_with_409() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    seed_user_with_key(&path, &crypto, "u2");
    let release = Arc::new(Semaphore::new(0));
    let engine = engine_with_factory(
        db.clone(),
        Some(crypto),
        Arc::new(GatedFactory {
            release: release.clone(),
            text: "最初の返信".to_owned(),
        }),
    );
    let app = app(engine, db);

    // 1 通目: バックグラウンドターンは GatedBackend が release されるまで確実に完了しない
    // （＝ in-flight ガードは確実にまだ保持されている）。
    let (status_a, body_a) = post_chat(&app, Some("u1"), "1通目").await;
    assert_eq!(status_a, StatusCode::ACCEPTED);

    // 2 通目（同じユーザー・まだ 1 通目の応答待ち）: 409。
    let (status_b, body_b) = post_chat(&app, Some("u1"), "2通目").await;
    assert_eq!(status_b, StatusCode::CONFLICT, "body={body_b:?}");
    assert!(body_b["message"].as_str().unwrap().contains("処理中"));

    // 別ユーザーは同時ターン防止の影響を受けない（キーが user_id のみのため独立）。
    let (status_other, _) = post_chat(&app, Some("u2"), "別ユーザーから").await;
    assert_eq!(
        status_other,
        StatusCode::ACCEPTED,
        "別ユーザーは 409 の影響を受けない"
    );

    // 1 通目を完了させる。
    release.add_permits(2); // u1 と u2 の 2 ターン分（許可は貯まるので順序に依存しない）
    let since_id: i64 = body_a["sinceId"].as_str().unwrap().parse().unwrap();
    poll_for_reply_after(&app, "u1", since_id, 200).await;

    // 完了後は新規送信が再び 202 で受け付けられる（guard が解放されている）。
    let (status_c, _) = post_chat(&app, Some("u1"), "3通目").await;
    assert_eq!(status_c, StatusCode::ACCEPTED);
}

// ─── リッチ返信（embeds/files）の配信（issue #41 PR #75 レビュー・P2） ─────────────────────────

async fn get_raw(
    app: &Router,
    token: Option<&str>,
    uri: &str,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = token {
        builder = builder.header("cookie", format!("__Host-yuuka-session={token}"));
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, content_type, bytes.to_vec())
}

#[tokio::test]
async fn rich_embed_reply_is_delivered_through_polling_and_history() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    // モデルが Embed を ```json ブロックで返す → エンジンが実 Embed へ復元する（既存の embed_recover 経路）。
    let engine = engine_with(
        db.clone(),
        Some(crypto),
        "気温をまとめました。\n```json\n{\"title\":\"天気\",\"description\":\"晴れ\",\"color\":\"#00ff00\",\"fields\":[{\"name\":\"最高\",\"value\":\"30℃\",\"inline\":true}],\"footer\":\"気象庁\"}\n```",
    );
    let app = app(engine, db);

    let (status, body) = post_chat(&app, Some("u1"), "天気を教えて").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let since_id: i64 = body["sinceId"].as_str().unwrap().parse().unwrap();

    let reply = poll_for_reply_after(&app, "u1", since_id, 200).await;
    assert!(reply["content"]
        .as_str()
        .unwrap()
        .contains("気温をまとめました"));
    let embeds = reply["embeds"].as_array().expect("embeds が配信される");
    assert_eq!(embeds.len(), 1);
    assert_eq!(embeds[0]["title"], "天気");
    assert_eq!(embeds[0]["description"], "晴れ");
    assert_eq!(embeds[0]["color"], 0x00ff00);
    assert_eq!(embeds[0]["footer"], "気象庁");
    assert_eq!(embeds[0]["fields"][0]["name"], "最高");
    assert_eq!(embeds[0]["fields"][0]["value"], "30℃");
    assert_eq!(embeds[0]["fields"][0]["inline"], true);
    // ユーザー発言・プレーンな返信には embeds/files キー自体が出ない（旧クライアント互換）。
    let history = get_history(&app, "u1").await;
    assert!(history[0].get("embeds").is_none());
    assert!(history[0].get("files").is_none());
}

#[tokio::test]
async fn attachments_are_listed_and_served_only_to_their_owner() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    seed_user_with_key(&path, &crypto, "u2");
    let engine = engine_with(db.clone(), Some(crypto), "unused");
    // エンジンを介さず、PWA 応答（グラフ PNG + 非画像ファイル）を永続化済みの状態を作る。
    yuuka_orchestrator::message_log::add_pwa_assistant_reply(
        &db,
        "u1",
        "system_default",
        "グラフです",
        None,
        &[
            yuuka_orchestrator::message_log::PwaAttachmentInput {
                name: "attachment.png".to_owned(),
                mime_type: "image/png".to_owned(),
                bytes: vec![0x89, b'P', b'N', b'G', 1, 2, 3],
            },
            yuuka_orchestrator::message_log::PwaAttachmentInput {
                name: "attachment.bin".to_owned(),
                mime_type: "application/octet-stream".to_owned(),
                bytes: vec![9, 8, 7],
            },
        ],
    )
    .await
    .unwrap();
    let app = app(engine, db);

    let history = get_history(&app, "u1").await;
    let files = history[0]["files"].as_array().expect("files が配信される");
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["name"], "attachment.png");
    assert_eq!(files[0]["mimeType"], "image/png");
    let png_url = files[0]["url"].as_str().unwrap().to_owned();
    let bin_url = files[1]["url"].as_str().unwrap().to_owned();
    assert!(png_url.starts_with("/api/client/chat/attachments/"));

    // 所有者は実バイトを取得できる（Content-Type は保存した MIME）。
    let (status, content_type, bytes) = get_raw(&app, Some("u1"), &png_url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("image/png"));
    assert_eq!(bytes, vec![0x89, b'P', b'N', b'G', 1, 2, 3]);
    let (status, content_type, bytes) = get_raw(&app, Some("u1"), &bin_url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("application/octet-stream"));
    assert_eq!(bytes, vec![9, 8, 7]);

    // 他ユーザーは 404（存在の有無も漏らさない）・未認証は 401・不正な id は 404。
    let (status, _, _) = get_raw(&app, Some("u2"), &png_url).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get_raw(&app, None, &png_url).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = get_raw(
        &app,
        Some("u1"),
        "/api/client/chat/attachments/not-a-number",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get_raw(&app, Some("u1"), "/api/client/chat/attachments/999999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // u2 の履歴には u1 の添付は現れない。
    assert!(get_history(&app, "u2").await.is_empty());
}

/// 常に上流 429（レート制限）で失敗する fake backend。エンジンはこれを分類済みの ⚠️ 定型応答
/// （`Ok(reply)`・**履歴には保存しない**）へ畳む。
struct RateLimitedBackend;

#[async_trait]
impl GenerateBackend for RateLimitedBackend {
    async fn generate(
        &self,
        _system_instruction: Option<&str>,
        _declarations: &[FunctionDeclaration],
        _contents: &[Content],
        _tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        Err(GeminiError::RateLimited { retry_after: None })
    }
}

struct RateLimitedFactory;

impl GeminiFactory for RateLimitedFactory {
    fn build(
        &self,
        _model: &str,
        _api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        Ok(Arc::new(RateLimitedBackend))
    }
}

#[tokio::test]
async fn unsaved_warning_reply_is_persisted_as_a_notice_so_polling_terminates() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    // エンジンは 429 を ⚠️ 定型応答（Ok）に畳み、履歴へは保存しない（LLM 文脈を汚染しない不変条件）。
    // 非同期配信ではこのままだとクライアントが永遠に待つため、バックグラウンド側が通知行として保存する。
    let engine = engine_with_factory(db.clone(), Some(crypto), Arc::new(RateLimitedFactory));
    let app = app(engine, db.clone());

    let (status, body) = post_chat(&app, Some("u1"), "こんにちは").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let since_id: i64 = body["sinceId"].as_str().unwrap().parse().unwrap();

    let reply = poll_for_reply_after(&app, "u1", since_id, 200).await;
    assert_eq!(reply["role"], "agent");
    assert!(
        reply["content"].as_str().unwrap().contains("利用制限"),
        "reply={reply:?}"
    );

    // 通知行は履歴には出るが、次ターンの LLM コンテキストには入らない。
    let context =
        yuuka_orchestrator::message_log::recent_pwa_context(&db, "u1", "system_default", 15)
            .await
            .unwrap();
    assert_eq!(context.len(), 1, "context={context:?}");
    assert_eq!(context[0].role, "user");
    let agent_rows = get_history(&app, "u1")
        .await
        .iter()
        .filter(|m| m["role"] == "agent")
        .count();
    assert_eq!(agent_rows, 1, "通知行はちょうど 1 件");
}

// ─── 受理の永続化と再起動後の回復（issue #77） ─────────────────────────────────────────────

fn count_pwa_rows(path: &std::path::Path, user_id: &str, filter: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM message_logs \
                 WHERE user_id = ?1 AND source = 'pwa' AND {filter}"
            ),
            rusqlite::params![user_id],
            |r| r.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn accepted_user_message_is_persisted_before_202_and_not_duplicated_by_the_turn() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    // release されるまでターンは進まない＝この間に見える履歴は「`202` の時点で保存済みのもの」だけ。
    let release = Arc::new(Semaphore::new(0));
    let engine = engine_with_factory(
        db.clone(),
        Some(crypto),
        Arc::new(GatedFactory {
            release: release.clone(),
            text: "お待たせしました".to_owned(),
        }),
    );
    let app = app(engine, db);

    let (status, body) = post_chat(&app, Some("u1"), "  受理された発言  ").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let since_id: i64 = body["sinceId"].as_str().unwrap().parse().unwrap();

    // ターン（生成）は未完了のまま、ユーザー発言は既に永続化されている（trim 済み・source='pwa'）。
    assert_eq!(count_pwa_rows(&path, "u1", "role = 'assistant'"), 0);
    assert_eq!(
        count_pwa_rows(&path, "u1", "role = 'user' AND content = '受理された発言'"),
        1,
        "202 の時点でユーザー発言が保存済み"
    );
    let history = get_history(&app, "u1").await;
    assert_eq!(history.len(), 1, "history={history:?}");
    assert_eq!(history[0]["role"], "user");
    assert_eq!(history[0]["content"], "受理された発言");
    let user_id: i64 = history[0]["id"].as_str().unwrap().parse().unwrap();
    assert!(user_id > since_id, "sinceId はユーザー発言より前の起点");

    // ターン完了後もユーザー発言は 1 件のまま（エンジンが再保存しない）・履歴は user → agent の 2 件。
    release.add_permits(1);
    let reply = poll_for_reply_after(&app, "u1", since_id, 200).await;
    assert_eq!(reply["content"], "お待たせしました");
    assert_eq!(count_pwa_rows(&path, "u1", "role = 'user'"), 1);
    assert_eq!(count_pwa_rows(&path, "u1", "role = 'assistant'"), 1);
    let history = get_history(&app, "u1").await;
    assert_eq!(history.len(), 2, "history={history:?}");
    assert_eq!(history[0]["role"], "user");
    assert_eq!(history[1]["role"], "agent");
}

/// `202` の後にプロセスが落ちた状況を再現する: 1 つ目の「プロセス」のターンは永遠に返らない（＝
/// 再起動で消えたターン）。2 つ目の「プロセス」（同じ DB ファイルを新しく開いたルータ/`InFlightTurns`）
/// が起動時スイープを実行し、クライアントのポーリングが終端することを確認する。
#[tokio::test]
async fn restart_recovery_terminates_polling_for_an_orphaned_turn() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("chat-send-test-secret".to_owned())).unwrap(),
    );
    seed_user_with_key(&path, &crypto, "u1");
    seed_user_with_key(&path, &crypto, "u2");

    // プロセス 1: u1 のターンは決して完了しない（release しない）。u2 は完了済みの会話を持つ。
    let never = Arc::new(Semaphore::new(0));
    let engine_1 = engine_with_factory(
        db.clone(),
        Some(crypto.clone()),
        Arc::new(GatedFactory {
            release: never,
            text: "返らない".to_owned(),
        }),
    );
    let app_1 = app(engine_1, db.clone());
    let (status, body) = post_chat(&app_1, Some("u1"), "再起動で消える質問").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let since_id: i64 = body["sinceId"].as_str().unwrap().parse().unwrap();
    message_log::add_pwa_message_log(&db, "u2", "system_default", "user", "済んだ質問")
        .await
        .unwrap();
    message_log::add_pwa_assistant_reply(&db, "u2", "system_default", "済んだ返事", None, &[])
        .await
        .unwrap();

    // プロセス 2（再起動後）: 同じ DB を新しく開き、in-flight が空のルータを組む。
    let db_2 = Db::open(&path).expect("reopen db");
    let engine_2 = engine_with(db_2.clone(), Some(crypto), "再起動後の返事");
    let app_2 = app(engine_2, db_2.clone());

    // スイープ前: u1 の履歴は未回答のユーザー発言で終わっている（クライアントは待ち続ける状態）。
    let before = get_history(&app_2, "u1").await;
    assert_eq!(before.len(), 1);
    assert_eq!(before[0]["role"], "user");

    // 起動時スイープ: 取り残された u1 の会話だけに通知を 1 件書く（完了済みの u2 には何も書かない）。
    let swept = yuuka_client_api::recover_orphaned_chat_turns(&db_2).await;
    assert_eq!(swept, 1);
    assert_eq!(count_pwa_rows(&path, "u2", "is_notice = 1"), 0);
    assert_eq!(count_pwa_rows(&path, "u1", "is_notice = 1"), 1);

    // クライアントのポーリング契約（`sinceId` より後の agent 行で終了）がそのまま終端する。
    let notice = poll_for_reply_after(&app_2, "u1", since_id, 20).await;
    assert_eq!(notice["role"], "agent");
    assert_eq!(
        notice["content"],
        yuuka_client_api::recovery::RESTART_NOTICE_TEXT
    );
    // 受理済みのユーザー発言は失われず、履歴は user → 通知 の 2 件。
    let history = get_history(&app_2, "u1").await;
    assert_eq!(history.len(), 2, "history={history:?}");
    assert_eq!(history[0]["content"], "再起動で消える質問");

    // 冪等: 2 回目のスイープ（例: さらに再起動）は何も書かない。
    assert_eq!(
        yuuka_client_api::recover_orphaned_chat_turns(&db_2).await,
        0
    );
    assert_eq!(count_pwa_rows(&path, "u1", "is_notice = 1"), 1);

    // 新プロセスは in-flight が空なので、同じユーザーがすぐ再送でき、正常に完了する。通知行は
    // LLM 文脈に入らない。
    let (status, body) = post_chat(&app_2, Some("u1"), "もう一度お願い").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let since_id_2: i64 = body["sinceId"].as_str().unwrap().parse().unwrap();
    let reply = poll_for_reply_after(&app_2, "u1", since_id_2, 200).await;
    assert_eq!(reply["content"], "再起動後の返事");
    let context = message_log::recent_pwa_context(&db_2, "u1", "system_default", 15)
        .await
        .unwrap();
    assert!(
        context
            .iter()
            .all(|c| !c.content.contains("サーバー再起動")),
        "通知行は文脈から除外される: {context:?}"
    );
}
