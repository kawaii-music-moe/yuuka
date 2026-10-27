//! sk-account を唯一の本人認証元として使う認証ルート。
//!
//! Yuuka と sk-account は親ドメインが異なるため Cookie は共有しない。ブラウザを
//! sk-account の認可入口へ送り、一回限り・短命の code をバックチャネルで交換した後、
//! Yuuka 自身のホスト専用 Cookie セッションを発行する。
//!
//! ログイン CSRF（攻撃者自身の code を持つ callback へ被害者を送り込み、攻撃者の
//! アカウントでログインさせる）を防ぐため、`start` で発行した `state` を短命 Cookie と
//! 認可 URL の両方に載せ、`callback` で一致を確かめてから code を交換する。

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Extension, FromRequestParts, Query, State};
use axum::http::header::{LOCATION, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use yuuka_types::SessionUser;
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

use crate::{users, SessionStore};

const COOKIE_HOST: &str = "__Host-yuuka-session";
const COOKIE_DEV: &str = "yuuka-session";
const STATE_COOKIE_HOST: &str = "__Host-yuuka-sk-state";
const STATE_COOKIE_DEV: &str = "yuuka-sk-state";
/// sk-account でのログイン・登録に掛かる時間を見込んだ `state` の有効期間。
const STATE_TTL_SECS: i64 = 10 * 60;

pub struct AuthRuntime {
    sessions: SessionStore,
    session_ttl_secs: u64,
    sk_account_public_url: String,
    sk_account_internal_url: String,
    admin_discord_ids: Vec<String>,
    http: reqwest::Client,
}

impl AuthRuntime {
    #[must_use]
    pub fn new(
        sessions: SessionStore,
        session_ttl_days: u32,
        sk_account_public_url: String,
        sk_account_internal_url: String,
        admin_discord_ids: Vec<String>,
    ) -> Self {
        Self {
            sessions,
            session_ttl_secs: u64::from(session_ttl_days) * 24 * 60 * 60,
            sk_account_public_url: sk_account_public_url.trim_end_matches('/').to_owned(),
            sk_account_internal_url: sk_account_internal_url.trim_end_matches('/').to_owned(),
            admin_discord_ids,
            http: reqwest::Client::new(),
        }
    }
}

pub fn routes(runtime: Arc<AuthRuntime>) -> Router<AppState> {
    Router::new()
        .route("/api/auth/sk/start", get(start))
        .route("/api/auth/sk/callback", get(callback))
        .route("/api/logout", post(logout))
        .route("/api/users", get(users_route))
        .layer(Extension(runtime))
}

async fn start(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
) -> Response {
    let Ok(login_state) = crate::token::generate_token() else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "ログインを開始できません。",
        );
    };
    let Ok(mut authorize_url) = reqwest::Url::parse(&format!(
        "{}/api/services/yuuka/authorize",
        rt.sk_account_public_url
    )) else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "認証先の設定が不正です。",
        );
    };
    authorize_url
        .query_pairs_mut()
        .append_pair("state", &login_state);
    if let Some(callback) = callback_url(&state) {
        authorize_url
            .query_pairs_mut()
            .append_pair("redirect_uri", &callback);
    }
    let mut response = redirect(authorize_url.as_str());
    response.headers_mut().insert(
        SET_COOKIE,
        build_state_cookie(state.config.https, &login_state, STATE_TTL_SECS),
    );
    response
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkAccount {
    id: String,
    discord_user_id: String,
    display_name: String,
}

#[derive(Deserialize)]
struct ExchangeResponse {
    account: SkAccount,
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
    let expected = cookie_value(headers, state_cookie_name(state.config.https));
    let state_matches = match (expected, query.state.as_deref()) {
        (Some(expected), Some(actual)) => constant_time_eq(expected.as_bytes(), actual.as_bytes()),
        _ => false,
    };
    if !state_matches {
        return error(
            StatusCode::BAD_REQUEST,
            "ログインの確認に失敗しました。もう一度ログインしてください。",
        );
    }
    let Some(code) = query.code.filter(|v| !v.is_empty()) else {
        return error(StatusCode::BAD_REQUEST, "認証コードがありません。");
    };
    let exchange_body = match callback_url(state) {
        Some(callback) => json!({ "code": code, "redirect_uri": callback }),
        None => json!({ "code": code }),
    };
    let response = match rt
        .http
        .post(format!(
            "{}/api/services/yuuka/exchange",
            rt.sk_account_internal_url
        ))
        .json(&exchange_body)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(_) => {
            return error(
                StatusCode::UNAUTHORIZED,
                "認証コードが無効または期限切れです。",
            )
        }
        Err(e) => {
            tracing::error!(error = %e, "sk-account code exchange failed");
            return error(StatusCode::BAD_GATEWAY, "sk-accountに接続できません。");
        }
    };
    let exchange = match response.json::<ExchangeResponse>().await {
        Ok(value) => value,
        Err(e) => {
            tracing::error!(error = %e, "invalid sk-account exchange response");
            return error(StatusCode::BAD_GATEWAY, "sk-accountの応答が不正です。");
        }
    };

    let role = match users::upsert_from_sk_account(
        &state.db,
        &exchange.account.id,
        &exchange.account.discord_user_id,
        &exchange.account.display_name,
        &rt.admin_discord_ids,
    )
    .await
    {
        Ok(role) => role,
        Err(e) => return ApiError::from(e).into_response(),
    };
    let user = SessionUser {
        discord_id: exchange.account.discord_user_id,
        username: exchange.account.display_name,
        role,
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
    let cookie = build_session_cookie(state.config.https, &token, ttl_max_age(rt.session_ttl_secs));
    let mut response = redirect("/");
    response.headers_mut().insert(SET_COOKIE, cookie);
    response
}

async fn logout(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    token: SessionCookieToken,
) -> Response {
    if let Some(token) = token.0 {
        rt.sessions.destroy(&token).await;
    }
    let mut response = Json(json!({ "success": true })).into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, build_session_cookie(state.config.https, "", 0));
    response
}

async fn users_route(user: AuthenticatedUser, State(state): State<AppState>) -> Response {
    match users::get_user_by_discord_id(&state.db, &user.0.discord_id).await {
        Ok(Some(found)) => Json(json!({
            "success": true,
            "user": { "discordId": found.discord_id, "username": found.username, "role": found.role }
        }))
        .into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "ユーザーが見つかりません。"),
        Err(e) => ApiError::from(e).into_response(),
    }
}

// Host/Origin ヘッダではなく、運用設定を認可コードの戻り先・対象に使う。
fn callback_url(state: &AppState) -> Option<String> {
    state
        .config
        .base_url
        .as_ref()
        .map(|base| format!("{}/api/auth/sk/callback", base.trim_end_matches('/')))
}

fn redirect(location: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(LOCATION, value);
    }
    response
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
