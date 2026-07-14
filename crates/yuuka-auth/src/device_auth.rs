//! デスクトップクライアントの OAuth デバイスフロー（RFC 8628 型・Node `desktopAuthService.ts` +
//! `deviceAuthRoutes.ts` パリティ）。
//!
//! - `POST /api/auth/device/code`（auth:none）＝device_code / user_code を発行する。
//! - `POST /api/auth/device/approve`（auth:user）＝ログイン済み本人が user_code を承認する。
//! - `POST /api/auth/device/token`（auth:none）＝アプリが device_code をトークンへ交換する（ポーリング）。
//!
//! **device_code の一時状態はインメモリ**で持つ（Node は Redis＋インメモリフォールバック。Rust は
//! Redis を意図的に非移植＝インメモリフォールバック相当。device_code は短命〔既定 600s〕で揮発して
//! 十分・web 再起動での消失は Node のプロセス再起動と同じく許容）。**発行済みトークンのみ SQLite**
//! （`desktop_tokens`）へ sha256 で永続化する（[`desktop::add_desktop_token`]）。store は main で 1 度
//! 生成して router へ注入し、web 再起動を跨いで保持する（webhook の crypto と同方式の Extension）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use yuuka_db::map_sqlite;
use yuuka_web::{AppState, AuthenticatedUser, Db, ScopedJson};

use crate::audit::add_audit_log;
use crate::token::generate_token;
use crate::{desktop, sha256_hex, DESKTOP_TOKEN_TTL_DAYS};

/// ポーリング最小間隔（秒）。token エンドポイントが `interval` として返す（Node `POLL_INTERVAL_SEC`）。
const POLL_INTERVAL_SECS: u64 = 5;

/// device_code の既定 TTL（秒）。Node `config.desktopDeviceCodeTtlSec`（既定 `DESKTOP_DEVICE_CODE_TTL_SEC=600`）。
const DEVICE_CODE_TTL_SECS: u64 = 600;

/// Crockford Base32（紛らわしい I/L/O/U を除く）。user_code の文字空間（Node `CROCKFORD`）。
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// device_name の最大長（Node `device_name.slice(0, 200)`）。
const MAX_DEVICE_NAME: usize = 200;

/// device_code レコード（インメモリ・Node `DeviceAuthRecord`）。
#[derive(Clone)]
struct DeviceRecord {
    user_code: String,
    approved: bool,
    device_name: Option<String>,
    approved_user: Option<String>,
    expires_at: Instant,
    /// slow_down 判定用: 直近に token ポーリングした時刻。
    last_polled_at: Option<Instant>,
}

/// store の可変状態（sha256(device_code)→レコード + user_code→hash の逆引き）。
struct StoreState {
    by_hash: HashMap<String, DeviceRecord>,
    by_user_code: HashMap<String, String>,
}

/// device_code の一時状態を保持するインメモリ store（router へ Extension で注入・web 再起動を跨ぐ）。
pub struct DeviceAuthStore {
    state: Mutex<StoreState>,
    /// 承認 URL のベース（Node `verificationBase`＝`base_url` or `http://host:port`）。
    verification_base: String,
}

impl DeviceAuthStore {
    /// 承認 URL ベースから空の store を作る（main で 1 度だけ）。
    #[must_use]
    pub fn new(verification_base: String) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(StoreState {
                by_hash: HashMap::new(),
                by_user_code: HashMap::new(),
            }),
            verification_base,
        })
    }

    fn lock(&self) -> MutexGuard<'_, StoreState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 期限切れレコードを掃除する（各操作の先頭で呼ぶ・背景タスク無しの遅延 GC）。
    fn sweep(state: &mut StoreState, now: Instant) {
        let expired: Vec<String> = state
            .by_hash
            .iter()
            .filter(|(_, r)| r.expires_at <= now)
            .map(|(h, _)| h.clone())
            .collect();
        for h in expired {
            if let Some(r) = state.by_hash.remove(&h) {
                state.by_user_code.remove(&r.user_code);
            }
        }
    }

    /// device_code / user_code を発行する（Node `createDeviceCode`）。
    fn create(&self, device_name: Option<String>) -> Result<serde_json::Value, getrandom::Error> {
        let device_code = generate_token()?;
        let hash = sha256_hex(&device_code);
        let now = Instant::now();
        let expires_at = now + Duration::from_secs(DEVICE_CODE_TTL_SECS);

        let mut state = self.lock();
        Self::sweep(&mut state, now);
        // user_code 衝突回避。空間 32^8 ≫ 生存件数なので実質 1 回で決まる。**loop-until-unique** ＝
        // 万一の衝突でも既存インデックスを上書きしない（dangling インデックスを作らない・Node の
        // break-after-5 上書きより堅牢）。
        let mut user_code = generate_user_code()?;
        while state.by_user_code.contains_key(&user_code) {
            user_code = generate_user_code()?;
        }
        state.by_user_code.insert(user_code.clone(), hash.clone());
        state.by_hash.insert(
            hash,
            DeviceRecord {
                user_code: user_code.clone(),
                approved: false,
                device_name,
                approved_user: None,
                expires_at,
                last_polled_at: None,
            },
        );
        drop(state);

        let base = &self.verification_base;
        // user_code は [0-9A-Z-] のみで URL セーフ＝encodeURIComponent は恒等。
        Ok(json!({
            "device_code": device_code,
            "user_code": user_code,
            "verification_uri": format!("{base}/device"),
            "verification_uri_complete": format!("{base}/device?code={user_code}"),
            "interval": POLL_INTERVAL_SECS,
            "expires_in": DEVICE_CODE_TTL_SECS,
        }))
    }

    /// user_code を承認する（Node `approveDeviceCode`・インメモリなので `persist_failed` は起きない）。
    fn approve(&self, user_code: &str, approved_user: &str) -> ApproveOutcome {
        let normalized = user_code.trim().to_uppercase();
        let now = Instant::now();
        let mut state = self.lock();
        Self::sweep(&mut state, now);
        let Some(hash) = state.by_user_code.get(&normalized).cloned() else {
            return ApproveOutcome::NotFound;
        };
        let Some(rec) = state.by_hash.get_mut(&hash) else {
            return ApproveOutcome::Expired;
        };
        if rec.expires_at <= now {
            return ApproveOutcome::Expired;
        }
        rec.approved = true;
        rec.approved_user = Some(approved_user.to_owned());
        ApproveOutcome::Ok {
            device_name: rec.device_name.clone(),
        }
    }

    /// device_code のポーリング判定（Node `exchangeDeviceToken` の状態遷移部・DB を触らない同期処理）。
    /// トークン発行（DB 書き込み）はロックを外した後にルート側で行う。
    fn poll(&self, device_code: &str) -> PollDecision {
        let hash = sha256_hex(device_code);
        let now = Instant::now();
        let mut state = self.lock();
        Self::sweep(&mut state, now);
        let Some(rec) = state.by_hash.get_mut(&hash) else {
            return PollDecision::Expired;
        };
        if rec.expires_at <= now {
            return PollDecision::Expired;
        }
        // slow_down: interval より短い間隔での連続ポーリングを抑制する。
        if let Some(last) = rec.last_polled_at {
            if now.duration_since(last) < Duration::from_secs(POLL_INTERVAL_SECS) {
                return PollDecision::SlowDown;
            }
        }
        rec.last_polled_at = Some(now);
        if !rec.approved {
            return PollDecision::Pending;
        }
        let Some(approved_user) = rec.approved_user.clone() else {
            return PollDecision::Pending;
        };
        let user_code = rec.user_code.clone();
        let device_name = rec.device_name.clone();
        // 使い切りをロック下で原子的に確定する（TOCTOU＝同一 device_code からの二重トークン発行を封じる。
        // 以降の並行 poll は Expired を見る。Node は DB 書き込み後に削除するため窓が空くのを是正。DB 発行
        // 失敗時はコードを消費済みとして扱い、クライアントはフローをやり直す〔安全側＝二重発行しない〕）。
        state.by_hash.remove(&hash);
        state.by_user_code.remove(&user_code);
        PollDecision::Approved {
            approved_user,
            device_name,
        }
    }

}

/// `approve` の結果（Node `ApproveResult`。`persist_failed` はインメモリでは発生しない）。
enum ApproveOutcome {
    Ok { device_name: Option<String> },
    NotFound,
    Expired,
}

/// `poll` の判定（トークン発行前の同期的な状態）。
enum PollDecision {
    Expired,
    SlowDown,
    Pending,
    Approved {
        approved_user: String,
        device_name: Option<String>,
    },
}

/// 人間可読な user_code（Crockford Base32 8 文字・`XXXX-XXXX`・Node `generateUserCode`）。
fn generate_user_code() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes)?;
    let mut s = String::with_capacity(9);
    for (i, b) in bytes.iter().enumerate() {
        if i == 4 {
            s.push('-');
        }
        let idx = (*b % 32) as usize;
        let c = CROCKFORD.get(idx).copied().unwrap_or(b'0');
        s.push(char::from(c));
    }
    Ok(s)
}

/// `users` から username / role を引く（承認済みユーザーの実在確認 + 応答用）。無ければ `None`。
async fn lookup_user(db: &Db, discord_id: &str) -> Result<Option<(String, String)>, yuuka_core::DbError> {
    let uid = discord_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT username, role FROM users WHERE discord_id = ?1",
                params![uid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

// ─── ルート ──────────────────────────────────────────────────────────────────

/// デバイスフローのルータ（store を Extension で内包・main で 1 度組んで貫通する）。
pub fn device_auth_routes(store: Arc<DeviceAuthStore>) -> Router<AppState> {
    Router::new()
        .route("/api/auth/device/code", post(code_handler))
        .route("/api/auth/device/approve", post(approve_handler))
        .route("/api/auth/device/token", post(token_handler))
        .layer(Extension(store))
}

// body フィールドは `Value` で受け、文字列以外は [`as_opt_string`] で None へ落とす（Node の
// `typeof x === "string" ? x : undefined` 相当）。こうしないと serde の Option<String> が非文字列で
// 失敗し、ScopedJson の汎用「invalid request body」400 になってエンドポイント固有応答に届かない。
#[derive(Debug, Deserialize)]
struct CodeBody {
    #[serde(default)]
    device_name: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ApproveBody {
    #[serde(default)]
    user_code: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct TokenBody {
    #[serde(default)]
    device_code: Option<Value>,
}

/// JSON 値から文字列だけを取り出す（非文字列/欠落は `None`・Node `typeof === "string"` パリティ）。
fn as_opt_string(v: Option<Value>) -> Option<String> {
    v.and_then(|x| x.as_str().map(str::to_owned))
}

fn server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "success": false, "message": "内部エラーが発生しました。" })),
    )
        .into_response()
}

/// §1.1: device_code / user_code を発行する（auth:none）。
async fn code_handler(
    Extension(store): Extension<Arc<DeviceAuthStore>>,
    ScopedJson { value: body, .. }: ScopedJson<CodeBody>,
) -> Response {
    // device_name は文字列のみ採用し先頭 200 文字に制限（Node `typeof===string ? slice(0,200)`）。
    let device_name = as_opt_string(body.device_name)
        .map(|s| s.chars().take(MAX_DEVICE_NAME).collect::<String>());
    match store.create(device_name) {
        Ok(payload) => (StatusCode::OK, Json(payload)).into_response(),
        Err(_) => server_error(),
    }
}

/// §1.2: ログイン済み本人が user_code を承認する（auth:user）。
async fn approve_handler(
    user: AuthenticatedUser,
    Extension(store): Extension<Arc<DeviceAuthStore>>,
    State(db): State<Db>,
    ScopedJson { value: body, .. }: ScopedJson<ApproveBody>,
) -> Response {
    let user_code = as_opt_string(body.user_code).unwrap_or_default();
    if user_code.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "message": "ユーザーコードが必要です。" })),
        )
            .into_response();
    }
    match store.approve(&user_code, &user.0.discord_id) {
        ApproveOutcome::Ok { device_name } => {
            // 監査はベストエフォート（承認自体は既にインメモリに反映済み）。
            add_audit_log(
                &db,
                &user.0.discord_id,
                "desktop.device_approve",
                device_name.as_deref(),
                None,
            )
            .await;
            (
                StatusCode::OK,
                Json(json!({ "success": true, "device_name": device_name })),
            )
                .into_response()
        }
        ApproveOutcome::Expired => (
            StatusCode::GONE,
            Json(json!({
                "success": false,
                "message": "このコードは期限切れです。アプリで再度お試しください。"
            })),
        )
            .into_response(),
        ApproveOutcome::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "message": "コードが見つかりません。入力を確認してください。"
            })),
        )
            .into_response(),
    }
}

/// §1.3: アプリが device_code をトークンへ交換する（auth:none・device_code で認可）。
async fn token_handler(
    Extension(store): Extension<Arc<DeviceAuthStore>>,
    State(db): State<Db>,
    ScopedJson { value: body, .. }: ScopedJson<TokenBody>,
) -> Response {
    let device_code = as_opt_string(body.device_code).unwrap_or_default();
    if device_code.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_request" }))).into_response();
    }
    match store.poll(&device_code) {
        PollDecision::Expired => {
            (StatusCode::GONE, Json(json!({ "error": "expired_token" }))).into_response()
        }
        PollDecision::SlowDown => {
            (StatusCode::OK, Json(json!({ "error": "slow_down" }))).into_response()
        }
        PollDecision::Pending => (
            StatusCode::OK,
            Json(json!({ "error": "authorization_pending" })),
        )
            .into_response(),
        PollDecision::Approved {
            approved_user,
            device_name,
        } => {
            // レコードは poll() がロック下で既に消費済み（使い切り・二重発行防止）。
            // 承認済みユーザーが実在するか確認（削除済み等の防御・不在は expired_token）。
            let user = match lookup_user(&db, &approved_user).await {
                Ok(u) => u,
                Err(_) => return server_error(),
            };
            let Some((username, role)) = user else {
                return (StatusCode::GONE, Json(json!({ "error": "expired_token" }))).into_response();
            };
            // 生トークンを 1 本発行して返す（hash のみ保存・平文はサーバに残さない）。
            let raw_token = match generate_token() {
                Ok(t) => t,
                Err(_) => return server_error(),
            };
            let token_hash = sha256_hex(&raw_token);
            let row_id = match desktop::add_desktop_token(
                &db,
                &approved_user,
                &token_hash,
                device_name.as_deref(),
            )
            .await
            {
                Ok(id) => id,
                Err(_) => return server_error(),
            };
            add_audit_log(
                &db,
                &approved_user,
                "desktop.token_issue",
                Some(&row_id.to_string()),
                None,
            )
            .await;

            let role = if role.is_empty() { "user".to_owned() } else { role };
            (
                StatusCode::OK,
                Json(json!({
                    "access_token": raw_token,
                    "token_type": "Bearer",
                    "expires_in": DESKTOP_TOKEN_TTL_DAYS * 24 * 60 * 60,
                    "user": {
                        "discordId": approved_user,
                        "username": username,
                        "role": role,
                    },
                })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct FakeAuth;

    #[async_trait::async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok((token == "owner").then(|| SessionUser {
                discord_id: "owner".to_owned(),
                username: "owner".to_owned(),
                role: Role::User,
            }))
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_deviceauth_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).expect("create empty");
            drop(conn);
        }
        let db = Db::open(&path).expect("open");
        {
            let conn = rusqlite::Connection::open(&path).expect("seed conn");
            conn.execute(
                "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt, role) \
                 VALUES ('owner', 'owner', 'x', 'x', 'user')",
                [],
            )
            .expect("seed user");
        }
        db
    }

    fn app(store: Arc<DeviceAuthStore>) -> axum::Router {
        let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), seed_db());
        super::device_auth_routes(store).with_state(state)
    }

    async fn send(app: &axum::Router, uri: &str, token: &str, body: &str) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if !token.is_empty() {
            builder = builder.header("cookie", format!("__Host-yuuka-session={token}"));
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
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

    #[tokio::test]
    async fn full_flow_code_approve_token() {
        let store = DeviceAuthStore::new("https://ex.test".to_owned());
        let app = app(store);

        // 1) code 発行。
        let (st, j) = send(&app, "/api/auth/device/code", "", r#"{"device_name":"My PC"}"#).await;
        assert_eq!(st, StatusCode::OK);
        let device_code = j["device_code"].as_str().unwrap().to_owned();
        let user_code = j["user_code"].as_str().unwrap().to_owned();
        assert_eq!(j["interval"], serde_json::json!(5));
        assert_eq!(j["expires_in"], serde_json::json!(600));
        assert_eq!(j["verification_uri"], serde_json::json!("https://ex.test/device"));
        assert_eq!(
            j["verification_uri_complete"],
            serde_json::json!(format!("https://ex.test/device?code={user_code}"))
        );
        // user_code は XXXX-XXXX。
        assert_eq!(user_code.len(), 9);
        assert_eq!(user_code.as_bytes()[4], b'-');

        // 2) 承認（auth:user）。小文字入力でも正規化される。
        //    （承認前ポーリング=pending は rapid_poll テストで確認。ここで poll すると slow_down が
        //     絡むため、承認→初回 poll の approved 経路を検証する。）
        let (st, j) = send(
            &app,
            "/api/auth/device/approve",
            "owner",
            &format!(r#"{{"user_code":"{}"}}"#, user_code.to_lowercase()),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["device_name"], serde_json::json!("My PC"));

        // 3) token 交換 → approved（access_token 発行）。承認直後の初回 poll なので slow_down しない。
        let (st, j) = send(
            &app,
            "/api/auth/device/token",
            "",
            &format!(r#"{{"device_code":"{device_code}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(j["access_token"].as_str().unwrap().len() >= 40);
        assert_eq!(j["token_type"], serde_json::json!("Bearer"));
        assert_eq!(j["expires_in"], serde_json::json!(90 * 24 * 60 * 60));
        assert_eq!(j["user"]["discordId"], serde_json::json!("owner"));
        assert_eq!(j["user"]["username"], serde_json::json!("owner"));

        // 4) 使い切り: 同じ device_code は消えて expired_token（410）。
        let (st, j) = send(
            &app,
            "/api/auth/device/token",
            "",
            &format!(r#"{{"device_code":"{device_code}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::GONE);
        assert_eq!(j["error"], serde_json::json!("expired_token"));
    }

    #[tokio::test]
    async fn approve_guards_and_token_edge_cases() {
        let store = DeviceAuthStore::new("http://127.0.0.1:3000".to_owned());
        let app = app(store);

        // approve は auth 必須（未ログイン → 401）。
        let (st, _) = send(&app, "/api/auth/device/approve", "", r#"{"user_code":"AAAA-BBBB"}"#).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        // user_code 空 → 400。
        let (st, _) = send(&app, "/api/auth/device/approve", "owner", r#"{"user_code":""}"#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // 未知の user_code → 404。
        let (st, j) = send(
            &app,
            "/api/auth/device/approve",
            "owner",
            r#"{"user_code":"ZZZZ-ZZZZ"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(j["success"], serde_json::json!(false));

        // token: device_code 欠落 → 400 invalid_request。
        let (st, j) = send(&app, "/api/auth/device/token", "", r#"{}"#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(j["error"], serde_json::json!("invalid_request"));

        // token: 未知 device_code → 410 expired_token。
        let (st, j) = send(
            &app,
            "/api/auth/device/token",
            "",
            r#"{"device_code":"nonexistent"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::GONE);
        assert_eq!(j["error"], serde_json::json!("expired_token"));
    }

    #[tokio::test]
    async fn non_string_body_fields_are_lenient() {
        let store = DeviceAuthStore::new("https://ex.test".to_owned());
        let app = app(store);
        // 非文字列 device_name → 無視して 200 発行（Node `typeof===string` coercion）。
        let (st, j) = send(&app, "/api/auth/device/code", "", r#"{"device_name":123}"#).await;
        assert_eq!(st, StatusCode::OK);
        assert!(j["device_code"].is_string());
        // 非文字列 device_code → エンドポイント固有 400 {error:"invalid_request"}（汎用 400 でない）。
        let (st, j) = send(&app, "/api/auth/device/token", "", r#"{"device_code":123}"#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(j["error"], serde_json::json!("invalid_request"));
        // 非文字列 user_code → 400「ユーザーコードが必要です。」（汎用 invalid request body でない）。
        let (st, j) = send(&app, "/api/auth/device/approve", "owner", r#"{"user_code":123}"#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(j["message"], serde_json::json!("ユーザーコードが必要です。"));
    }

    #[tokio::test]
    async fn rapid_poll_returns_slow_down() {
        let store = DeviceAuthStore::new("https://ex.test".to_owned());
        let app = app(store);
        let (_, j) = send(&app, "/api/auth/device/code", "", r#"{}"#).await;
        let device_code = j["device_code"].as_str().unwrap().to_owned();
        // 初回 poll → pending（last_polled をセット）。
        let (_, j1) = send(
            &app,
            "/api/auth/device/token",
            "",
            &format!(r#"{{"device_code":"{device_code}"}}"#),
        )
        .await;
        assert_eq!(j1["error"], serde_json::json!("authorization_pending"));
        // 即座の 2 回目 → slow_down（interval 未満）。
        let (st, j2) = send(
            &app,
            "/api/auth/device/token",
            "",
            &format!(r#"{{"device_code":"{device_code}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j2["error"], serde_json::json!("slow_down"));
    }
}
