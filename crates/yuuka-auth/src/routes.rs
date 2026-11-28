//! OIDC プロバイダーを唯一の本人認証元として使う認証ルート（標準は sk-account）。
//!
//! ブラウザをプロバイダーの認可エンドポイントへ送り、戻ってきた一回限りの code をバックチャネルで
//! 交換して ID トークンを検証した後、Yuuka 自身のホスト専用 Cookie セッションを発行する。
//! 流れと検証は [`crate::oidc`] に置き、ここは HTTP の入口と Cookie だけを扱う。
//!
//! ログイン CSRF（攻撃者自身の code を持つ callback へ被害者を送り込み、攻撃者の
//! アカウントでログインさせる）を防ぐため、`start` で発行した `state` を短命 Cookie と
//! 認可 URL の両方に載せ、`callback` で一致を確かめてから code を交換する。
//!
//! プロバイダーでログアウトすると `POST /api/auth/oidc/backchannel-logout` に logout_token が届くので、
//! 同じプロバイダーのセッション（`sid`）から作った Yuuka のセッションを消す。

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{Extension, Form, FromRequestParts, Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use yuuka_types::SessionUser;
use yuuka_web::{AdminUser, ApiError, AppState, AuthenticatedUser};

use yuuka_core::RegistrationMode;

use crate::discord_link::DiscordLinkClient;
use crate::oidc::{LoginError, LogoutTarget, OidcClient};
use crate::users::{LoginIdentity, LoginPolicy};
use crate::{users, SessionStore};

const COOKIE_HOST: &str = "__Host-yuuka-session";
const COOKIE_DEV: &str = "yuuka-session";
const STATE_COOKIE_HOST: &str = "__Host-yuuka-oidc-state";
const STATE_COOKIE_DEV: &str = "yuuka-oidc-state";

pub struct AuthRuntime {
    sessions: SessionStore,
    session_ttl_secs: u64,
    policy: LoginPolicy,
    oidc: Option<Arc<OidcClient>>,
    /// ログイン開始時に受け取った招待コード（ログインの `state` → コード・受け取った時刻）。
    pending_invites: Mutex<HashMap<String, (String, Instant)>>,
    /// Discord の後付け連携（`DISCORD_OAUTH_CLIENT_*` 未設定なら None）。
    discord_link: Option<Arc<DiscordLinkClient>>,
}

/// 保留できる招待コードの上限（OIDC の保留ログインと同じ考え方で、溢れたら古いものから捨てる）。
const MAX_PENDING_INVITES: usize = 1024;

impl AuthRuntime {
    /// `oidc` が `None`（`OIDC_CLIENT_SECRET` 未設定）ならログインできない。
    ///
    /// 初期 admin は `admin_discord_ids` だけで決まる。`ADMIN_OIDC_SUBJECTS`・`ADMIN_EMAILS`・
    /// `REGISTRATION_MODE` も使うときは [`Self::with_login_policy`] で差し替える。
    #[must_use]
    pub fn new(
        sessions: SessionStore,
        session_ttl_days: u32,
        admin_discord_ids: Vec<String>,
        oidc: Option<Arc<OidcClient>>,
    ) -> Self {
        Self {
            sessions,
            session_ttl_secs: u64::from(session_ttl_days) * 24 * 60 * 60,
            policy: LoginPolicy {
                admin_discord_ids,
                ..LoginPolicy::default()
            },
            oidc,
            pending_invites: Mutex::new(HashMap::new()),
            discord_link: None,
        }
    }

    /// Discord の後付け連携を有効にする（§5）。
    #[must_use]
    pub fn with_discord_link(mut self, link: Option<DiscordLinkClient>) -> Self {
        self.discord_link = link.map(Arc::new);
        self
    }

    /// 初期 admin の指定と新規登録の扱いを差し替える。
    #[must_use]
    pub fn with_login_policy(mut self, policy: LoginPolicy) -> Self {
        if !policy.has_admin() {
            tracing::warn!("ADMIN_DISCORD_IDS・ADMIN_OIDC_SUBJECTS・ADMIN_EMAILS がどれも未設定です。新規利用者に管理者権限は付与されません。初回ログイン前に管理者を指定してください。");
        }
        match policy.registration {
            RegistrationMode::Closed => {
                tracing::info!("REGISTRATION_MODE=closed: 既存の利用者だけがログインできます。");
            }
            RegistrationMode::Invite => {
                tracing::info!("REGISTRATION_MODE=invite: 新規登録には招待コードが要ります（管理画面で発行）。");
            }
            RegistrationMode::Open => {}
        }
        self.policy = policy;
        self
    }

    fn remember_invite(&self, login_state: &str, code: String) {
        let mut pending = self
            .pending_invites
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        pending.retain(|_, (_, at)| now.duration_since(*at) < crate::oidc::LOGIN_TTL);
        if pending.len() >= MAX_PENDING_INVITES {
            if let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(k, _)| k.clone())
            {
                pending.remove(&oldest);
            }
        }
        pending.insert(login_state.to_owned(), (code, now));
    }

    fn take_invite(&self, login_state: &str) -> Option<String> {
        self.pending_invites
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(login_state)
            .filter(|(_, at)| at.elapsed() < crate::oidc::LOGIN_TTL)
            .map(|(code, _)| code)
    }
}

pub fn routes(runtime: Arc<AuthRuntime>) -> Router<AppState> {
    Router::new()
        .route("/api/auth/oidc/provider", get(provider))
        .route("/api/auth/oidc/start", get(start))
        .route("/api/auth/oidc/callback", get(callback))
        .route(
            "/api/auth/oidc/backchannel-logout",
            post(backchannel_logout),
        )
        .route("/api/logout", post(logout))
        .route("/api/users", get(users_route))
        .route("/api/admin/invites", get(list_invites).post(create_invite))
        .route("/api/admin/invites/delete", post(delete_invite))
        .route("/api/me/discord", get(discord_status))
        .route("/api/me/discord/link/start", get(discord_link_start))
        .route("/api/me/discord/link/callback", get(discord_link_callback))
        .route("/api/me/discord/unlink", post(discord_unlink))
        .layer(Extension(runtime))
}

/// ログイン画面向け: ログイン先の表示名と、ログインできる状態か。
async fn provider(Extension(rt): Extension<Arc<AuthRuntime>>) -> Response {
    Json(json!({
        "success": true,
        "enabled": rt.oidc.is_some(),
        "label": rt.oidc.as_ref().map(|oidc| oidc.label()),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct StartQuery {
    /// 招待コード（`REGISTRATION_MODE=invite` で初めてログインする人が、招待リンクから持ってくる）。
    invite: Option<String>,
}

async fn start(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    Query(query): Query<StartQuery>,
) -> Response {
    let Some(oidc) = &rt.oidc else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "ログインが設定されていません。",
        );
    };
    let (authorize_url, login_state) = match oidc.start_login().await {
        Ok(started) => started,
        Err(e) => {
            tracing::error!(error = %e.0, "OIDC のログインを開始できない");
            return error(StatusCode::BAD_GATEWAY, "ログイン先に接続できません。");
        }
    };
    if let Some(code) = query.invite.as_deref().and_then(crate::invite::normalize) {
        rt.remember_invite(&login_state, code);
    }
    let mut response = redirect(&authorize_url);
    let max_age = i64::try_from(crate::oidc::LOGIN_TTL.as_secs()).unwrap_or(i64::MAX);
    response.headers_mut().insert(
        SET_COOKIE,
        build_state_cookie(state.config.https, &login_state, max_age),
    );
    response
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    iss: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    Query(query): Query<CallbackQuery>,
    headers: HeaderMap,
) -> Response {
    let https = state.config.https;
    let mut response = exchange_and_login(&state, &rt, query, &headers).await;
    // 成否に関わらず state は一回限りにする。
    response
        .headers_mut()
        .append(SET_COOKIE, build_state_cookie(https, "", 0));
    response
}

async fn exchange_and_login(
    state: &AppState,
    rt: &AuthRuntime,
    query: CallbackQuery,
    headers: &HeaderMap,
) -> Response {
    let Some(oidc) = &rt.oidc else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "ログインが設定されていません。",
        );
    };
    let expected = cookie_value(headers, state_cookie_name(state.config.https));
    let login_state = match (expected, query.state.as_deref()) {
        (Some(expected), Some(actual))
            if constant_time_eq(expected.as_bytes(), actual.as_bytes()) =>
        {
            expected
        }
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "ログインの確認に失敗しました。もう一度ログインしてください。",
            )
        }
    };
    let Some(code) = query.code.filter(|v| !v.is_empty()) else {
        if let Some(e) = query.error {
            tracing::warn!(error = %e, "OIDC プロバイダーがログインを拒否した");
        }
        return error(StatusCode::BAD_REQUEST, "認証コードがありません。");
    };
    let outcome = match oidc
        .finish_login(&login_state, &code, query.iss.as_deref())
        .await
    {
        Ok(outcome) => outcome,
        Err(LoginError::Rejected(message)) => return error(StatusCode::UNAUTHORIZED, message),
        Err(LoginError::Unavailable(e)) => {
            tracing::error!(error = %e.0, "OIDC のログインを完了できない");
            return error(StatusCode::BAD_GATEWAY, "ログイン先に接続できません。");
        }
    };
    let identity = outcome.identity;

    let login = LoginIdentity {
        subject: identity.subject,
        discord_id: identity.discord_id,
        email: identity.email,
        username: identity.display_name,
    };
    let invite = rt.take_invite(&login_state);
    let found =
        match users::upsert_from_oidc(&state.db, &login, &rt.policy, invite.as_deref()).await {
            Ok(Ok(found)) => found,
            Ok(Err(rejection)) => {
                tracing::warn!(reason = ?rejection, "OIDC ログインを断った");
                let status = match rejection {
                    users::LoginRejection::Conflict(_) => StatusCode::CONFLICT,
                    users::LoginRejection::RegistrationClosed
                    | users::LoginRejection::InviteRequired
                    | users::LoginRejection::InviteInvalid => StatusCode::FORBIDDEN,
                };
                return error(status, rejection.message());
            }
            Err(e) => return ApiError::from(e).into_response(),
        };
    let user = SessionUser {
        user_id: found.user_id,
        username: login.username,
        role: found.role,
    };
    let token = match rt.sessions.create(&user, rt.session_ttl_secs).await {
        Ok(token) => token,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "セッションを作成できません。",
            )
        }
    };
    rt.sessions
        .put_oidc_link(
            &crate::sha256_hex(&token),
            &outcome.link,
            rt.session_ttl_secs,
        )
        .await;
    let cookie = build_session_cookie(state.config.https, &token, ttl_max_age(rt.session_ttl_secs));
    let mut response = redirect("/");
    response.headers_mut().insert(SET_COOKIE, cookie);
    response
}

#[derive(Deserialize)]
struct BackchannelLogoutForm {
    logout_token: Option<String>,
}

/// OIDC Back-Channel Logout: プロバイダーでログアウトしたセッションを消す。
async fn backchannel_logout(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    Form(form): Form<BackchannelLogoutForm>,
) -> Response {
    let status = match (&rt.oidc, form.logout_token) {
        (None, _) => StatusCode::NOT_IMPLEMENTED,
        (Some(_), None) => StatusCode::BAD_REQUEST,
        (Some(oidc), Some(logout_token)) => match oidc.verify_logout_token(&logout_token).await {
            None => StatusCode::BAD_REQUEST,
            Some(LogoutTarget::Session(sid)) => {
                rt.sessions.destroy_by_sid(&sid).await;
                StatusCode::OK
            }
            Some(LogoutTarget::Subject(sub)) => {
                match users::user_id_by_subject(&state.db, &sub).await {
                    Ok(Some(user_id)) => {
                        rt.sessions.destroy_all_for_user(&user_id).await;
                        StatusCode::OK
                    }
                    Ok(None) => StatusCode::OK,
                    Err(e) => return ApiError::from(e).into_response(),
                }
            }
        },
    };
    let mut response = status.into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn logout(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    token: SessionCookieToken,
) -> Response {
    if let Some(token) = token.0 {
        let hash = crate::sha256_hex(&token);
        let link = rt.sessions.oidc_link(&hash, rt.session_ttl_secs).await;
        rt.sessions.destroy_hash(&hash).await;
        // プロバイダー側のリフレッシュトークンも失効させる（応答は待たない）。
        if let (Some(oidc), Some(link)) = (rt.oidc.clone(), link) {
            tokio::spawn(async move { oidc.revoke(&link).await });
        }
    }
    let mut response = Json(json!({ "success": true })).into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, build_session_cookie(state.config.https, "", 0));
    response
}

async fn users_route(user: AuthenticatedUser, State(state): State<AppState>) -> Response {
    match users::get_user_by_discord_id(&state.db, &user.0.user_id).await {
        Ok(Some(found)) => Json(json!({
            "success": true,
            "user": { "discordId": found.discord_id, "username": found.username, "role": found.role }
        }))
        .into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "ユーザーが見つかりません。"),
        Err(e) => ApiError::from(e).into_response(),
    }
}

fn redirect(location: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(LOCATION, value);
    }
    response
}

// ─── Discord の後付け連携（§5） ─────────────────────────────────────────────────

/// 連携後に戻すアカウント画面（`?discord=linked|conflict|cancelled|error|unavailable`）。
const ACCOUNT_PAGE: &str = "/admin/account/profile";

fn account_redirect(result: &str) -> Response {
    redirect(&format!("{ACCOUNT_PAGE}?discord={result}"))
}

/// `GET /api/me/discord`：連携の状態。`available` は連携ボタンを出せるか（OAuth2 が設定済みか）。
async fn discord_status(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
) -> Response {
    match users::discord_link_of(&state.db, &user.0.user_id).await {
        Ok(Some(discord_id)) => Json(json!({
            "success": true,
            "linked": discord_id.is_some(),
            "discordId": discord_id,
            "available": rt.discord_link.is_some(),
        }))
        .into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "ユーザーが見つかりません。"),
        Err(e) => ApiError::from(e).into_response(),
    }
}

/// `GET /api/me/discord/link/start`：Discord の認可画面へ送る。
async fn discord_link_start(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<AuthRuntime>>,
) -> Response {
    let Some(link) = &rt.discord_link else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Discord 連携が設定されていません。",
        );
    };
    match link.start(&user.0.user_id) {
        Ok(url) => redirect(&url),
        Err(e) => {
            tracing::error!(error = ?e, "Discord 連携を開始できない");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Discord 連携を開始できません。",
            )
        }
    }
}

#[derive(Deserialize)]
struct LinkCallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

/// `GET /api/me/discord/link/callback`：code を交換して連携し、アカウント画面へ戻す。
async fn discord_link_callback(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    Query(query): Query<LinkCallbackQuery>,
) -> Response {
    let Some(link) = &rt.discord_link else {
        return account_redirect("unavailable");
    };
    let (Some(code), Some(link_state)) = (query.code.filter(|c| !c.is_empty()), query.state) else {
        return account_redirect("cancelled");
    };
    let discord_id = match link.finish(&user.0.user_id, &link_state, &code).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = ?e, "Discord 連携を確かめられない");
            return account_redirect("error");
        }
    };
    match users::link_discord(&state.db, &user.0.user_id, &discord_id).await {
        Ok(Ok(())) => {
            crate::audit::add_audit_log(
                &state.db,
                &user.0.user_id,
                "discord.link",
                Some(&discord_id),
                None,
            )
            .await;
            account_redirect("linked")
        }
        Ok(Err(rejection)) => {
            tracing::warn!(reason = ?rejection, "Discord 連携を断った（統合はしない）");
            account_redirect("conflict")
        }
        Err(e) => {
            tracing::error!(error = %e, "Discord 連携を保存できない");
            account_redirect("error")
        }
    }
}

/// `POST /api/me/discord/unlink`：連携を解除する（OIDC の紐付けがあるときだけ・§9）。
async fn discord_unlink(user: AuthenticatedUser, State(state): State<AppState>) -> Response {
    match users::unlink_discord(&state.db, &user.0.user_id).await {
        Ok(users::UnlinkOutcome::Unlinked) => {
            crate::audit::add_audit_log(&state.db, &user.0.user_id, "discord.unlink", None, None)
                .await;
            Json(json!({ "success": true, "message": "Discord の連携を解除しました。" }))
                .into_response()
        }
        Ok(users::UnlinkOutcome::NotLinked) => {
            error(StatusCode::BAD_REQUEST, "Discord は連携していません。")
        }
        Ok(users::UnlinkOutcome::NoOtherLogin) => error(
            StatusCode::CONFLICT,
            "ほかにログインする手段が無いため、Discord の連携を解除できません。",
        ),
        Err(e) => ApiError::from(e).into_response(),
    }
}

// ─── 招待コード（管理者のみ・§4.1） ───────────────────────────────────────────────

/// `GET /api/admin/invites`：招待コードの一覧。
async fn list_invites(_admin: AdminUser, State(state): State<AppState>) -> Response {
    match crate::invite::list(&state.db).await {
        Ok(invites) => Json(json!({ "success": true, "invites": invites })).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInviteBody {
    note: Option<String>,
    expires_in_days: Option<u32>,
}

/// `POST /api/admin/invites`：招待コードを発行する。
async fn create_invite(
    admin: AdminUser,
    State(state): State<AppState>,
    Json(body): Json<CreateInviteBody>,
) -> Response {
    match crate::invite::create(&state.db, &admin.0.user_id, body.note, body.expires_in_days).await
    {
        Ok(invite) => {
            crate::audit::add_audit_log(
                &state.db,
                &admin.0.user_id,
                "admin.invite_create",
                Some(&invite.code),
                None,
            )
            .await;
            Json(json!({ "success": true, "invite": invite })).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

#[derive(Deserialize)]
struct DeleteInviteBody {
    code: String,
}

/// `POST /api/admin/invites/delete`：招待コードを削除する。
async fn delete_invite(
    admin: AdminUser,
    State(state): State<AppState>,
    Json(body): Json<DeleteInviteBody>,
) -> Response {
    match crate::invite::delete(&state.db, &body.code).await {
        Ok(true) => {
            crate::audit::add_audit_log(
                &state.db,
                &admin.0.user_id,
                "admin.invite_delete",
                Some(&body.code),
                None,
            )
            .await;
            Json(json!({ "success": true })).into_response()
        }
        Ok(false) => error(StatusCode::NOT_FOUND, "招待コードが見つかりません。"),
        Err(e) => ApiError::from(e).into_response(),
    }
}

fn error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

fn ttl_max_age(ttl_secs: u64) -> i64 {
    i64::try_from(ttl_secs).unwrap_or(i64::MAX)
}

#[must_use]
pub fn build_session_cookie(https: bool, token: &str, max_age: i64) -> HeaderValue {
    let name = if https { COOKIE_HOST } else { COOKIE_DEV };
    let secure = if https { "; Secure" } else { "" };
    let cookie =
        format!("{name}={token}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age={max_age}");
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn state_cookie_name(https: bool) -> &'static str {
    if https {
        STATE_COOKIE_HOST
    } else {
        STATE_COOKIE_DEV
    }
}

/// `__Host-` 接頭辞は `Path=/` を要求するため、state Cookie もサイト全体に付ける。
/// 兄弟サブドメインから上書きされない（`__Host-`）ことを経路の絞り込みより優先する。
fn build_state_cookie(https: bool, value: &str, max_age: i64) -> HeaderValue {
    let name = state_cookie_name(https);
    let secure = if https { "; Secure" } else { "" };
    let cookie =
        format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age={max_age}");
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// 長さは秘密ではないので先に比べ、中身は早期終了せずに全バイトを比べる。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

fn cookie_value(headers: &HeaderMap, wanted: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == wanted && !value.is_empty()).then(|| value.to_owned())
    })
}

fn extract_cookie_token(parts: &Parts, https: bool) -> Option<String> {
    cookie_value(&parts.headers, if https { COOKIE_HOST } else { COOKIE_DEV })
}

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
