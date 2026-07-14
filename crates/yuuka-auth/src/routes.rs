//! 認証発行 HTTP ルート（Node `src/server/routes/authRoutes.ts` §5.4 パリティ）。
//!
//! `/api/setup/status`・`/api/setup`・`/api/register`・`/api/register/verify`・`/api/login`・
//! `/api/logout`・`/api/users`。セッション**発行**（[`SessionStore::create`]）はここが担う（`/api/me`
//! 等の検証は yuuka-web/CompositeAuth）。ルート固有の依存（セッション発行ハンドル・暗号・保留登録・
//! DM ポート・レート制限）は [`AuthRuntime`] にまとめ、`Extension` レイヤで注入する（`AppState` に
//! 逆依存を作らない＝yuuka-web→yuuka-auth の循環回避）。
//!
//! 認証不要ルート（setup/login/register）は Cookie を持たないため CSRF ガードを通過する
//! （`csrf_guard` は Cookie 認証 × cross-site の POST のみ 403）。

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::extract::{ConnectInfo, Extension, State};
use axum::http::header::SET_COOKIE;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use yuuka_crypto::{Encrypted, SystemCrypto};
use yuuka_types::SessionUser;
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

use crate::pending::{PendingRegistration, PendingStore, RegistrationDm, VerifyResult};
use crate::ratelimit::RateLimiter;
use crate::{audit, invite, password_policy, users, SessionStore};

/// 新規ユーザーの既定 Gemini モデル（Node と一致）。
const GEMINI_DEFAULT_MODEL: &str = "gemini-3.1-flash-lite";
/// Cookie 名（HTTPS 本番）。`__Host-` プレフィクスは `Secure; Path=/`・`Domain` 無しを要求する。
const COOKIE_HOST: &str = "__Host-yuuka-session";
/// Cookie 名（非 HTTPS 開発/移行期）。
const COOKIE_DEV: &str = "yuuka-session";

/// 認証発行ルートが使う実行時依存（`Extension` で各ハンドラへ注入）。
///
/// `AppState`（yuuka-web 所有）に入れると web→auth の循環になるため分離する。
pub struct AuthRuntime {
    /// セッション発行/失効に使う共有ストア（`CompositeAuth` と同一 Redis + in-memory を共有）。
    sessions: SessionStore,
    /// セッション TTL（秒・Cookie `Max-Age` と Redis EX に使う）。
    session_ttl_secs: u64,
    /// Gemini API キー暗号化に使う（`YUUKA_ENCRYPTION_SECRET` 未設定なら `None`＝setup/verify が 500）。
    crypto: Option<Arc<SystemCrypto>>,
    /// DM チャレンジ登録の保留ストア。
    pending: PendingStore,
    /// 確認コード DM 配信ポート（Discord live まで `NullRegistrationDm`＝register は 502）。
    dm: Arc<dyn RegistrationDm>,
    /// ログイン試行・登録送信のレート制限。
    rate: RateLimiter,
    /// 初期 admin に昇格する Discord ID（`createUser` のロール判定に渡す）。
    admin_discord_ids: Vec<String>,
}

impl AuthRuntime {
    /// 実行時依存を束ねる。`session_ttl_days` から Cookie/Redis TTL（秒）を導出する。
    #[must_use]
    pub fn new(
        sessions: SessionStore,
        session_ttl_days: u32,
        crypto: Option<Arc<SystemCrypto>>,
        dm: Arc<dyn RegistrationDm>,
        admin_discord_ids: Vec<String>,
    ) -> Self {
        Self {
            sessions,
            session_ttl_secs: u64::from(session_ttl_days) * 24 * 60 * 60,
            crypto,
            pending: PendingStore::new(),
            dm,
            rate: RateLimiter::new(),
            admin_discord_ids,
        }
    }
}

/// 認証発行ルータ（`AppState` 上でマージされる）。ルート固有依存を `Extension` で載せる。
pub fn routes(runtime: Arc<AuthRuntime>) -> Router<AppState> {
    Router::new()
        .route("/api/setup/status", get(setup_status))
        .route("/api/setup", post(setup))
        .route("/api/register", post(register))
        .route("/api/register/verify", post(register_verify))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/users", get(users_route))
        .layer(Extension(runtime))
}

// ─── リクエストボディ（全て camelCase・欠落は空文字扱いで Node の !field チェックに写す） ───

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupBody {
    #[serde(default)]
    discord_id: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    gemini_api_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterBody {
    #[serde(default)]
    discord_id: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    invite_code: String,
    #[serde(default)]
    gemini_api_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyBody {
    #[serde(default)]
    discord_id: String,
    #[serde(default)]
    code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginBody {
    #[serde(default)]
    discord_id: String,
    #[serde(default)]
    password: String,
}

// ─── ハンドラ ───────────────────────────────────────────────────────────────

/// `GET /api/setup/status`（auth: none）。初回セットアップが必要か + 規約 URL を返す。
async fn setup_status(State(state): State<AppState>) -> Response {
    let need_setup = match users::count_users(&state.db).await {
        Ok(n) => n == 0,
        Err(e) => return ApiError::from(e).into_response(),
    };
    // legal URL は system_settings（admin 保存値）を優先し、無ければ config へ（Node publicLegalUrls）。
    let (privacy_policy_url, terms_url) = yuuka_web::public_legal_urls(&state).await;
    Json(json!({
        "needSetup": need_setup,
        "privacyPolicyUrl": privacy_policy_url,
        "termsUrl": terms_url,
    }))
    .into_response()
}

/// `POST /api/setup`（auth: none）。最初のユーザー（＝管理者）を登録し、自動ログインする。
async fn setup(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    Json(body): Json<SetupBody>,
) -> Response {
    // 既にセットアップ済みなら拒否。
    match users::count_users(&state.db).await {
        Ok(0) => {}
        Ok(_) => {
            return err(
                StatusCode::BAD_REQUEST,
                "システムは既にセットアップされています。",
            )
        }
        Err(e) => return ApiError::from(e).into_response(),
    }

    if body.discord_id.is_empty()
        || body.username.is_empty()
        || body.password.is_empty()
        || body.gemini_api_key.is_empty()
    {
        return err(
            StatusCode::BAD_REQUEST,
            "すべてのフィールド（Discord ID、ユーザーネーム、パスワード、Gemini API Key）を入力してください。",
        );
    }
    if let Err(msg) = password_policy::validate_password(&body.password) {
        return err(StatusCode::BAD_REQUEST, msg);
    }
    let clean_discord_id = body.discord_id.trim();
    let clean_username = body.username.trim();
    if !is_valid_discord_id(clean_discord_id) {
        return err(
            StatusCode::BAD_REQUEST,
            "Discord ID の形式が不正です（17〜20桁の数字）。",
        );
    }
    if clean_username.encode_utf16().count() > 64 {
        return err(
            StatusCode::BAD_REQUEST,
            "ユーザーネームは64文字以内で入力してください。",
        );
    }
    // 暗号が未構成なら **DB 書き込み前に** fail-closed する。create_user を先に走らせると、その後の
    // Gemini キー暗号化で 500 になった際に「admin 行だけ commit 済み（count!=0）→ setup が二度と
    // 通らない」半端状態でロックされる（手動 DB 削除以外で復旧不能）。暗号設定は運用ミスなので、
    // ここで弾いて何も書かない（configured の通常経路では発火しない）。
    if rt.crypto.is_none() {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "サーバの暗号化設定が未構成です（YUUKA_ENCRYPTION_SECRET）。",
        );
    }

    // 1) 管理者ユーザー作成（初回なので admin）。
    let role = match users::create_user(
        &state.db,
        clean_discord_id,
        clean_username,
        &body.password,
        &rt.admin_discord_ids,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return ApiError::from(e).into_response(),
    };

    // 2) Gemini API キーを暗号化保存。
    let enc = match encrypt_gemini(&rt, body.gemini_api_key.trim()) {
        Ok(e) => e,
        Err(msg) => return err(StatusCode::INTERNAL_SERVER_ERROR, msg),
    };
    if let Err(e) =
        users::update_user_gemini_settings(&state.db, clean_discord_id, &enc, GEMINI_DEFAULT_MODEL)
            .await
    {
        return ApiError::from(e).into_response();
    }

    // 3) セッション発行 + 自動ログイン。
    let su = SessionUser {
        discord_id: clean_discord_id.to_owned(),
        username: clean_username.to_owned(),
        role,
    };
    let token = match rt.sessions.create(&su, rt.session_ttl_secs).await {
        Ok(t) => t,
        Err(_) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "セッションの発行に失敗しました。",
            )
        }
    };
    audit::add_audit_log(
        &state.db,
        clean_discord_id,
        "auth.register",
        Some("initial_setup"),
        None,
    )
    .await;

    let cookie = build_session_cookie(state.config.https, &token, ttl_max_age(rt.session_ttl_secs));
    json_with_cookie(
        StatusCode::OK,
        json!({"success": true, "message": "管理者登録が完了しました。続いてデフォルトBotを設定してください。"}),
        cookie,
    )
}

/// `POST /api/register`（auth: none）。招待コード必須。本人確認コードを DM して保留する（G1 対策）。
async fn register(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    client_ip: ClientIp,
    Json(body): Json<RegisterBody>,
) -> Response {
    if body.discord_id.is_empty()
        || body.username.is_empty()
        || body.password.is_empty()
        || body.invite_code.is_empty()
        || body.gemini_api_key.is_empty()
    {
        return err(
            StatusCode::BAD_REQUEST,
            "すべてのフィールド（Discord ID、ユーザーネーム、パスワード、招待コード、Gemini API Key）を入力してください。",
        );
    }
    let clean_discord_id = body.discord_id.trim();
    let clean_username = body.username.trim();
    if !is_valid_discord_id(clean_discord_id) {
        return err(
            StatusCode::BAD_REQUEST,
            "Discord ID の形式が不正です（17〜20桁の数字）。",
        );
    }
    if clean_username.encode_utf16().count() > 64 {
        return err(
            StatusCode::BAD_REQUEST,
            "ユーザーネームは64文字以内で入力してください。",
        );
    }
    match users::get_user_by_discord_id(&state.db, clean_discord_id).await {
        Ok(Some(_)) => {
            return err(
                StatusCode::BAD_REQUEST,
                "このDiscord IDは既に登録されています。",
            )
        }
        Ok(None) => {}
        Err(e) => return ApiError::from(e).into_response(),
    }
    if let Err(msg) = password_policy::validate_password(&body.password) {
        return err(StatusCode::BAD_REQUEST, msg);
    }
    // 招待コードは事前検証のみ（消費は本人確認後）。無効なら DM を送らない。
    match invite::is_valid_code(&state.db, body.invite_code.trim()).await {
        Ok(true) => {}
        Ok(false) => {
            return err(
                StatusCode::BAD_REQUEST,
                "無効な、または使用済みの招待コードです。",
            )
        }
        Err(e) => return ApiError::from(e).into_response(),
    }
    // DM スパム・ID 列挙の防止（(IP, discordId) 単位）。
    let send_key = format!("{}|{}", client_ip.0, clean_discord_id);
    if !rt.rate.allow_register_send(&send_key) {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "確認コードの送信回数が上限に達しました。しばらくしてから再度お試しください。",
        );
    }
    // G1 対策（DM チャレンジ）: 主張 ID 宛にワンタイムコードを DM し、本人確認後にのみ作成する。
    let code = match rt.pending.create(
        clean_discord_id,
        PendingRegistration {
            username: clean_username.to_owned(),
            password: body.password.clone(),
            gemini_api_key: body.gemini_api_key.trim().to_owned(),
            invite_code: body.invite_code.trim().to_owned(),
        },
    ) {
        Ok(c) => c,
        Err(_) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "確認コードの生成に失敗しました。",
            )
        }
    };
    if !rt.dm.send_registration_code(clean_discord_id, &code).await {
        return err(
            StatusCode::BAD_GATEWAY,
            "確認コードのDM送信に失敗しました。Botと同じDiscordサーバーに参加し、DMの受信を許可した上で再度お試しください。",
        );
    }
    Json(json!({
        "success": true,
        "pending": true,
        "message": "確認コードをDiscordのDMに送信しました。10分以内にコードを入力して登録を完了してください。"
    }))
    .into_response()
}

/// `POST /api/register/verify`（auth: none）。DM コードを検証し、実ユーザーを作成する（G1 対策）。
async fn register_verify(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    Json(body): Json<VerifyBody>,
) -> Response {
    if body.discord_id.is_empty() || body.code.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "Discord ID と確認コードを入力してください。",
        );
    }
    // 暗号未構成なら、保留コードや招待コードを消費する前に fail-closed（setup と同じ半端回避）。
    if rt.crypto.is_none() {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "サーバの暗号化設定が未構成です（YUUKA_ENCRYPTION_SECRET）。",
        );
    }
    let clean_discord_id = body.discord_id.trim();

    let reg = match rt.pending.verify(clean_discord_id, body.code.trim()) {
        VerifyResult::Ok(reg) => *reg,
        VerifyResult::NotFound => {
            return err(
                StatusCode::BAD_REQUEST,
                "登録手続きが見つかりません。最初からやり直してください。",
            )
        }
        VerifyResult::Expired => {
            return err(
                StatusCode::BAD_REQUEST,
                "確認コードの有効期限が切れました。最初からやり直してください。",
            )
        }
        VerifyResult::TooManyAttempts => {
            return err(
                StatusCode::BAD_REQUEST,
                "確認コードの試行回数が上限に達しました。最初からやり直してください。",
            )
        }
        VerifyResult::CodeMismatch => {
            return err(StatusCode::BAD_REQUEST, "確認コードが正しくありません。")
        }
    };

    // 確認中に他経路で同 ID が登録された場合の保護。
    match users::get_user_by_discord_id(&state.db, clean_discord_id).await {
        Ok(Some(_)) => {
            return err(
                StatusCode::BAD_REQUEST,
                "このDiscord IDは既に登録されています。",
            )
        }
        Ok(None) => {}
        Err(e) => return ApiError::from(e).into_response(),
    }
    // 招待コードをアトミックに消費（本人確認後）。
    match invite::validate_and_consume_code(&state.db, &reg.invite_code, clean_discord_id).await {
        Ok(true) => {}
        Ok(false) => {
            return err(
                StatusCode::BAD_REQUEST,
                "無効な、または使用済みの招待コードです。",
            )
        }
        Err(e) => return ApiError::from(e).into_response(),
    }

    if let Err(e) = users::create_user(
        &state.db,
        clean_discord_id,
        &reg.username,
        &reg.password,
        &rt.admin_discord_ids,
    )
    .await
    {
        return ApiError::from(e).into_response();
    }
    let enc = match encrypt_gemini(&rt, &reg.gemini_api_key) {
        Ok(e) => e,
        Err(msg) => return err(StatusCode::INTERNAL_SERVER_ERROR, msg),
    };
    if let Err(e) =
        users::update_user_gemini_settings(&state.db, clean_discord_id, &enc, GEMINI_DEFAULT_MODEL)
            .await
    {
        return ApiError::from(e).into_response();
    }
    audit::add_audit_log(&state.db, clean_discord_id, "auth.register", None, None).await;

    Json(json!({"success": true, "message": "登録が完了しました！ログインしてください。"}))
        .into_response()
}

/// `POST /api/login`（auth: none）。レート制限つきの資格情報ログイン。成功で Cookie セッションを発行。
async fn login(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    client_ip: ClientIp,
    Json(body): Json<LoginBody>,
) -> Response {
    if body.discord_id.is_empty() || body.password.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "Discord ID とパスワードを入力してください。",
        );
    }
    let clean_discord_id = body.discord_id.trim().to_owned();
    // レート制限鍵は (IP, アカウント) 単位（1 IP から全アカウントを巻き込まない）。
    let rl_key = format!("{}|{}", client_ip.0, clean_discord_id);

    if let Some(remain) = rt.rate.login_locked_secs(&rl_key) {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            &format!("ログイン試行回数が上限に達しました。{remain}秒後に再試行してください。"),
        );
    }

    let user = match users::get_user_by_discord_id(&state.db, &clean_discord_id).await {
        Ok(u) => u,
        Err(e) => return ApiError::from(e).into_response(),
    };
    // タイミングオラクル対策: 不在でも一定の bcrypt 比較時間を消費する。
    let stored_hash = user.as_ref().map(|u| u.password_hash.clone());
    let password_ok =
        users::verify_password_constant_time(body.password.clone(), stored_hash).await;

    if let (Some(u), true) = (user.as_ref(), password_ok) {
        rt.rate.clear_login(&rl_key);
        let su = SessionUser {
            discord_id: u.discord_id.clone(),
            username: u.username.clone(),
            role: u.role_enum(),
        };
        let token = match rt.sessions.create(&su, rt.session_ttl_secs).await {
            Ok(t) => t,
            Err(_) => {
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "セッションの発行に失敗しました。",
                )
            }
        };
        audit::add_audit_log(&state.db, &clean_discord_id, "auth.login", None, None).await;
        let cookie =
            build_session_cookie(state.config.https, &token, ttl_max_age(rt.session_ttl_secs));
        return json_with_cookie(
            StatusCode::OK,
            json!({"success": true, "message": "ログインに成功しました！"}),
            cookie,
        );
    }

    // 失敗。ロック期間中は resetAt を延長せず新しい窓のみ開始する（Node と一致）。
    rt.rate.record_login_failure(&rl_key);
    if user.is_some() {
        audit::add_audit_log(
            &state.db,
            &clean_discord_id,
            "auth.login_failed",
            None,
            None,
        )
        .await;
    }
    err(
        StatusCode::UNAUTHORIZED,
        "Discord ID またはパスワードが正しくありません。",
    )
}

/// `POST /api/logout`（auth: user）。セッションを失効させ Cookie を削除する。
async fn logout(
    _user: AuthenticatedUser,
    RawSessionToken(token): RawSessionToken,
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
) -> Response {
    if let Some(token) = token {
        rt.sessions.destroy(&token).await;
    }
    let cookie = build_session_cookie(state.config.https, "", 0);
    json_with_cookie(
        StatusCode::OK,
        json!({"success": true, "message": "ログアウトしました。"}),
        cookie,
    )
}

/// `GET /api/users`（auth: user）。互換のため自身のユーザー名のみを返す（Node と一致）。
async fn users_route(user: AuthenticatedUser, State(state): State<AppState>) -> Response {
    match users::get_user_by_discord_id(&state.db, &user.0.discord_id).await {
        Ok(Some(u)) => Json(json!({"success": true, "users": [u.username]})).into_response(),
        Ok(None) => Json(json!({"success": true, "users": []})).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

// ─── ヘルパ ─────────────────────────────────────────────────────────────────

/// Gemini API キーを暗号化する。`Err` は 500 応答に使うユーザー向けメッセージ（Response を
/// 直接返すと `clippy::result_large_err` になるため文言だけを返し、呼び出し側で `err()` 化する）。
fn encrypt_gemini(rt: &AuthRuntime, api_key: &str) -> Result<Encrypted, &'static str> {
    let Some(crypto) = rt.crypto.as_ref() else {
        return Err("サーバの暗号化設定が未構成です（YUUKA_ENCRYPTION_SECRET）。");
    };
    crypto.encrypt_text(api_key).map_err(|e| {
        tracing::error!(error = %e, "Gemini API キーの暗号化に失敗");
        "APIキーの暗号化に失敗しました。"
    })
}

/// `{success:false, message}` のエラー応答を作る（Node `sendJson` 相当）。
fn err(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"success": false, "message": message}))).into_response()
}

/// JSON ボディに `Set-Cookie` を付けて応答する。
fn json_with_cookie(status: StatusCode, body: serde_json::Value, cookie: HeaderValue) -> Response {
    let mut resp = (status, Json(body)).into_response();
    resp.headers_mut().insert(SET_COOKIE, cookie);
    resp
}

/// セッション TTL（秒）から Cookie `Max-Age` を導出する（i64・オーバーフローは飽和）。
fn ttl_max_age(ttl_secs: u64) -> i64 {
    i64::try_from(ttl_secs).unwrap_or(i64::MAX)
}

/// `Set-Cookie` ヘッダ値を作る（Node `setSessionCookie` パリティ）。
///
/// HTTPS 本番は `__Host-yuuka-session` + `Secure`、開発は `yuuka-session`。`max_age=0` は削除。
/// トークンは base64url（予約文字なし）なので `HeaderValue` 化は失敗しない。設定系ルート
/// （プロフィール/パスワード変更）のセッション再発行でも再利用するため公開する。
pub fn build_session_cookie(https: bool, token: &str, max_age: i64) -> HeaderValue {
    let name = if https { COOKIE_HOST } else { COOKIE_DEV };
    let secure = if https { "; Secure" } else { "" };
    let cookie =
        format!("{name}={token}; Path=/; HttpOnly{secure}; SameSite=Lax; Max-Age={max_age}");
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// Discord ID（snowflake）形式か（17〜20 桁の数字・Node `isValidDiscordId`）。
fn is_valid_discord_id(s: &str) -> bool {
    (17..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

/// `Cookie` ヘッダからセッショントークンを取り出す（Node `getSessionToken`・auth.rs と同一規則）。
fn extract_cookie_token(parts: &Parts, https: bool) -> Option<String> {
    let raw = parts
        .headers
        .get(axum::http::header::COOKIE)?
        .to_str()
        .ok()?;
    let find = |name: &str| {
        raw.split(';')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.trim(), v.trim()))
            .rfind(|(k, _)| *k == name)
            .map(|(_, v)| v.to_owned())
    };
    if https {
        find(COOKIE_HOST)
    } else {
        find(COOKIE_HOST).or_else(|| find(COOKIE_DEV))
    }
}

/// クライアント実 IP を解決する（Node `getClientIp`）。直前 peer が信頼プロキシのときのみ
/// `X-Forwarded-For` を右端から辿り、最初の非信頼アドレスを採用する。
fn resolve_client_ip(peer: Option<IpAddr>, xff: Option<&str>, trusted: &[IpAddr]) -> String {
    let Some(peer) = peer else {
        return "unknown".to_owned();
    };
    if !trusted.is_empty() && trusted.contains(&peer) {
        if let Some(xff) = xff {
            for part in xff
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .rev()
            {
                match part.parse::<IpAddr>() {
                    Ok(ip) if trusted.contains(&ip) => continue,
                    _ => return part.to_owned(),
                }
            }
        }
    }
    peer.to_string()
}

// ─── 抽出器 ─────────────────────────────────────────────────────────────────

/// レート制限用のクライアント IP（`ConnectInfo` + XFF から解決・失敗しない）。
struct ClientIp(String);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // `into_make_service_with_connect_info::<SocketAddr>()` が peer を extensions に載せる。
        // テスト（Router 直叩き）では不在 → `unknown` に縮退。
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0.ip());
        let xff = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok());
        Ok(Self(resolve_client_ip(
            peer,
            xff,
            &state.config.trusted_proxies,
        )))
    }
}

/// ログアウトで失効させる生セッショントークン（Cookie から抽出・失敗しない）。
struct RawSessionToken(Option<String>);

impl FromRequestParts<AppState> for RawSessionToken {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(extract_cookie_token(parts, state.config.https)))
    }
}

/// 現在のセッショントークン（Cookie から抽出・失敗しない・[`RawSessionToken`] の公開版）。
///
/// 設定系ルート（プロフィール/パスワード変更）が旧セッションを失効させて再発行するために使う。
pub struct SessionCookieToken(pub Option<String>);

impl FromRequestParts<AppState> for SessionCookieToken {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(extract_cookie_token(parts, state.config.https)))
    }
}

#[cfg(test)]
mod tests {
    use super::{is_valid_discord_id, resolve_client_ip};
    use std::net::IpAddr;

    #[test]
    fn discord_id_format() {
        assert!(is_valid_discord_id("123456789012345678")); // 18 桁
        assert!(is_valid_discord_id("12345678901234567")); // 17 桁
        assert!(is_valid_discord_id("12345678901234567890")); // 20 桁
        assert!(!is_valid_discord_id("1234567890123456")); // 16 桁
        assert!(!is_valid_discord_id("123456789012345678901")); // 21 桁
        assert!(!is_valid_discord_id("12345678901234567a")); // 非数字
    }

    #[test]
    fn client_ip_prefers_xff_behind_trusted_proxy() {
        let proxy: IpAddr = "10.0.0.1".parse().unwrap();
        let real: IpAddr = "203.0.113.7".parse().unwrap();
        // peer が信頼プロキシ → XFF 右端から最初の非信頼を採用。
        assert_eq!(
            resolve_client_ip(Some(proxy), Some("203.0.113.7, 10.0.0.1"), &[proxy]),
            real.to_string()
        );
        // peer が非信頼 → XFF は無視して peer を採用（偽装対策）。
        assert_eq!(
            resolve_client_ip(Some(real), Some("1.1.1.1"), &[proxy]),
            real.to_string()
        );
        // peer 不明 → unknown。
        assert_eq!(resolve_client_ip(None, None, &[proxy]), "unknown");
    }
}
