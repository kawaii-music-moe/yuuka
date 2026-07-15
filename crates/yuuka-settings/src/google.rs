//! Google 連携ルート（Node `settingsRoutes.ts` パリティ）:
//! `GET /api/settings/google/oauth/url`（認可 URL 生成）・`GET .../callback`（トークン交換 + アカウント保存・
//! auth:none + 手動セッション検証 + 302 リダイレクト）・`POST /api/settings/calendars`（同期対象更新）・
//! `POST /api/settings/backup/trigger`（手動バックアップ）。外部 HTTP はシーム越し（未配線時は縮退）。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::header::LOCATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use yuuka_web::{ApiError, AppState, AuthenticatedUser, OptionalUser};

use crate::SettingsRuntime;

/// OAuth の redirect_uri を構築する（Node `buildOAuthRedirectUri`）。base_url があればそれを使い、無ければ
/// localhost/127.0.0.1 のときのみ Host から構築する（攻撃者操作可能な Host での偽装を防ぐ）。不能なら `None`。
fn build_redirect_uri(base_url: Option<&str>, host_header: Option<&str>) -> Option<String> {
    if let Some(b) = base_url.filter(|s| !s.is_empty()) {
        let trimmed = b.trim_end_matches('/');
        return Some(format!("{trimmed}/api/settings/google/oauth/callback"));
    }
    let host = host_header.unwrap_or_default().to_ascii_lowercase();
    let hostname = host.split(':').next().unwrap_or("");
    if hostname == "localhost" || hostname == "127.0.0.1" {
        return Some(format!("http://{host}/api/settings/google/oauth/callback"));
    }
    None
}

fn host_header(headers: &HeaderMap) -> Option<&str> {
    headers.get("host").and_then(|v| v.to_str().ok())
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

// ─── GET /api/settings/google/oauth/url ──────────────────────────────────────

pub(crate) async fn oauth_url(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<SettingsRuntime>>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if !rt.oauth.is_configured() {
        return Ok(bad_request(
            "システムに Google OAuth2 設定が登録されていません。システム管理者に問い合わせてください。",
        ));
    }
    let Some(redirect_uri) =
        build_redirect_uri(state.config.base_url.as_deref(), host_header(&headers))
    else {
        return Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "message": "OAuth リダイレクトURIを構築できません。システム管理者に BASE_URL の設定を依頼してください。",
            })),
        )
            .into_response());
    };
    // CSRF 対策: セッションユーザーに束縛した一回限りの state nonce を発行する。
    let state_nonce = rt
        .oauth_state
        .create(&user.0.discord_id)
        .map_err(|_| ApiError(yuuka_core::WebError::Internal))?;
    let url = rt.oauth.auth_url(&redirect_uri, &state_nonce);
    Ok(Json(json!({ "success": true, "url": url })).into_response())
}

// ─── GET /api/settings/google/oauth/callback（auth:none・302 応答） ───────────

#[derive(Debug, Deserialize)]
pub(crate) struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

fn redirect(location: &str) -> Response {
    (StatusCode::FOUND, [(LOCATION, location)]).into_response()
}

pub(crate) async fn oauth_callback(
    maybe_user: OptionalUser,
    Extension(rt): Extension<Arc<SettingsRuntime>>,
    State(state): State<AppState>,
    Query(q): Query<CallbackQuery>,
    headers: HeaderMap,
) -> Response {
    // コールバック時点のセッション確認（auth:none だが手動検証）。
    let Some(session) = maybe_user.0 else {
        return redirect("/?oauth=error&msg=unauthorized");
    };
    let user_id = session.discord_id;

    if !rt.oauth.is_configured() {
        return redirect("/?oauth=error&msg=missing_config");
    }

    // CSRF 対策: state を消費し、フロー開始時のセッションユーザーと一致するか確認する。
    let state_param = q.state.unwrap_or_default();
    match rt.oauth_state.consume(&state_param) {
        Some(uid) if uid == user_id => {}
        _ => return redirect("/?oauth=error&msg=invalid_state"),
    }

    let Some(code) = q.code.filter(|c| !c.is_empty()) else {
        return redirect("/?oauth=error&msg=missing_code");
    };
    let Some(redirect_uri) =
        build_redirect_uri(state.config.base_url.as_deref(), host_header(&headers))
    else {
        return redirect("/?oauth=error&msg=token_exchange_failed");
    };

    let tokens = match rt.oauth.exchange_code(&redirect_uri, &code).await {
        Ok(t) => t,
        Err(_) => return redirect("/?oauth=error&msg=token_exchange_failed"),
    };

    // email 取得は非致命（失敗は None・Node は握り潰す）。
    let email = match tokens.access_token.as_deref() {
        Some(at) => rt.oauth.fetch_email(at).await,
        None => None,
    };

    let Some(refresh_token) = tokens.refresh_token else {
        // refresh_token が来ない（再同意なし）→ 明示エラー（URL 生成側で prompt=consent 済み）。
        return redirect("/?oauth=error&msg=no_refresh_token");
    };

    // リフレッシュトークンをシステム鍵で暗号化して保存する。
    let Some(crypto) = rt.crypto.as_ref() else {
        return redirect("/?oauth=error&msg=token_exchange_failed");
    };
    let Ok(enc) = crypto.encrypt_text(&refresh_token) else {
        return redirect("/?oauth=error&msg=token_exchange_failed");
    };
    let account_id = match yuuka_google::repo::add_or_update_account(
        &state.db,
        &user_id,
        email.clone(),
        enc.encrypted,
        enc.iv,
        enc.auth_tag,
        email.clone(),
    )
    .await
    {
        Ok(id) => id,
        Err(_) => return redirect("/?oauth=error&msg=token_exchange_failed"),
    };
    rt.calendar.invalidate_account(account_id);
    yuuka_auth::audit::add_audit_log(
        &state.db,
        &user_id,
        "auth.google_link",
        email.as_deref(),
        None,
    )
    .await;
    redirect("/?oauth=success")
}

// ─── POST /api/settings/calendars ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct CalendarsInput {
    calendars: Option<Value>,
}

/// JS `String(x)` 相当（カレンダー ID の緩い文字列化・実運用は常に文字列配列）。
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

pub(crate) async fn calendars(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<SettingsRuntime>>,
    Json(body): Json<CalendarsInput>,
) -> Result<Response, ApiError> {
    let Some(Value::Array(list)) = body.calendars else {
        return Ok(bad_request("カレンダーリストは配列形式で指定してください。"));
    };
    let calendars: Vec<String> = list.iter().map(js_string).collect();
    yuuka_google::repo::set_user_calendars(&state.db, &user.0.discord_id, &calendars).await?;
    rt.calendar.invalidate_user(&user.0.discord_id);
    Ok(Json(json!({ "success": true, "message": "同期対象カレンダーを更新しました。" })).into_response())
}

// ─── POST /api/settings/backup/trigger ───────────────────────────────────────

pub(crate) async fn backup_trigger(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<SettingsRuntime>>,
) -> Response {
    match rt.backup.run_backup(&user.0.discord_id).await {
        Ok(url) => {
            yuuka_auth::audit::add_audit_log(
                &state.db,
                &user.0.discord_id,
                "backup.manual_run",
                None,
                None,
            )
            .await;
            Json(json!({
                "success": true,
                "url": url,
                "message": "手動バックアップが完了しました。",
            }))
            .into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "message": "手動バックアップに失敗しました。Google連携とバックアップ先フォルダの設定をご確認ください。",
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::build_redirect_uri;

    #[test]
    fn redirect_uri_parity() {
        // base_url があれば末尾スラッシュを除去して使う。
        assert_eq!(
            build_redirect_uri(Some("https://yuuka.example/"), Some("evil.example")).as_deref(),
            Some("https://yuuka.example/api/settings/google/oauth/callback")
        );
        // base_url 無し + localhost Host → http で構築。
        assert_eq!(
            build_redirect_uri(None, Some("localhost:3000")).as_deref(),
            Some("http://localhost:3000/api/settings/google/oauth/callback")
        );
        assert_eq!(
            build_redirect_uri(None, Some("127.0.0.1")).as_deref(),
            Some("http://127.0.0.1/api/settings/google/oauth/callback")
        );
        // base_url 無し + 非 localhost Host → 拒否（攻撃者 Host 偽装防止）。
        assert_eq!(build_redirect_uri(None, Some("attacker.example")), None);
        assert_eq!(build_redirect_uri(None, None), None);
    }
}
