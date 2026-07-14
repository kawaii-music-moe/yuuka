//! admin ルートハンドラ（全て `AdminUser` extractor で auth:"admin" を型強制）。
//!
//! レスポンス本文は Node `adminRoutes.ts` の `sendJson` と**バイト単位で一致**させる
//! （フラットな `{success, ...}` オブジェクト・日本語メッセージ verbatim）。想定内の業務エラー
//! （400/404）は明示的な JSON レスポンスで返し、想定外の [`DbError`](yuuka_core::DbError) のみ
//! `?` で [`ApiError`] に写像（500・内部詳細は丸められる）。ロール変更・ユーザー削除では
//! `destroy_all_for_user` でセッションを一括失効し、セッション内ロールの陳腐化を防ぐ。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::json;
use yuuka_core::WebError;
use yuuka_web::{AdminUser, ApiError, Db};

use crate::dto::{
    AuditQuery, BotIdInput, CodeInput, RoleInput, SystemSettingsInput, TargetUserInput, TokenInput,
};
use crate::{repo, AdminRuntime};

// ─── デフォルト Bot のトークン更新 ────────────────────────────────────────────

pub(crate) async fn default_bot_token(
    admin: AdminUser,
    Extension(rt): Extension<Arc<AdminRuntime>>,
    State(db): State<Db>,
    Json(body): Json<TokenInput>,
) -> Result<Response, ApiError> {
    let token = body.token.as_deref().map(str::trim).unwrap_or_default();
    if token.is_empty() {
        return Ok(bad_request("トークンを入力してください。"));
    }
    // 1. 暗号化して bots(system_default) へ upsert。
    let Some(crypto) = rt.crypto.as_ref() else {
        // 暗号鍵は起動時 require_encryption_secret で必須化される（防御的に 500）。
        return Err(ApiError(WebError::Internal));
    };
    let enc = crypto
        .encrypt_text(token)
        .map_err(|_| ApiError(WebError::Internal))?;
    repo::upsert_default_bot_token(&db, &admin.0.discord_id, &enc).await?;
    yuuka_auth::audit::add_audit_log(&db, &admin.0.discord_id, "admin.default_bot_token", None, None)
        .await;
    // 2. デフォルト Bot 再起動（runtime シーム）。
    rt.bots.restart_default(token).await;
    Ok(ok_message("デフォルトBotのトークンを更新しました。"))
}

// ─── 統計 ─────────────────────────────────────────────────────────────────────

pub(crate) async fn stats(_admin: AdminUser, State(db): State<Db>) -> Result<Response, ApiError> {
    let stats = repo::stats(&db).await?;
    Ok(json_ok(json!({ "success": true, "stats": stats })))
}

// ─── システム設定 ─────────────────────────────────────────────────────────────

pub(crate) async fn system_settings_get(
    _admin: AdminUser,
    Extension(rt): Extension<Arc<AdminRuntime>>,
    State(db): State<Db>,
) -> Result<Response, ApiError> {
    let privacy = non_empty(repo::get_system_setting(&db, "privacy_policy_url").await?)
        .unwrap_or_else(|| rt.privacy_policy_url.clone());
    let terms = non_empty(repo::get_system_setting(&db, "terms_url").await?)
        .unwrap_or_else(|| rt.terms_url.clone());
    Ok(json_ok(json!({
        "success": true,
        "privacyPolicyUrl": privacy,
        "termsUrl": terms,
    })))
}

pub(crate) async fn system_settings_set(
    _admin: AdminUser,
    State(db): State<Db>,
    Json(body): Json<SystemSettingsInput>,
) -> Result<Response, ApiError> {
    let privacy = body
        .privacy_policy_url
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    let terms = body
        .terms_url
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    // 空欄・"/"始まりの相対パスは許可、それ以外は絶対 URL を要求する（Node `new URL()` 相当）。
    if !is_valid_setting_url(&privacy) {
        return Ok(bad_request("無効なプライバシーポリシーのURL形式です。"));
    }
    if !is_valid_setting_url(&terms) {
        return Ok(bad_request("無効な利用規約のURL形式です。"));
    }
    repo::set_system_setting(&db, "privacy_policy_url", &privacy).await?;
    repo::set_system_setting(&db, "terms_url", &terms).await?;
    Ok(ok_message("システム設定を保存しました。"))
}

// ─── ユーザー管理 ─────────────────────────────────────────────────────────────

pub(crate) async fn users(_admin: AdminUser, State(db): State<Db>) -> Result<Response, ApiError> {
    let users = repo::list_all_users(&db).await?;
    Ok(json_ok(json!({ "success": true, "users": users })))
}

pub(crate) async fn users_role(
    admin: AdminUser,
    Extension(rt): Extension<Arc<AdminRuntime>>,
    State(db): State<Db>,
    Json(body): Json<RoleInput>,
) -> Result<Response, ApiError> {
    let admin_id = admin.0.discord_id.as_str();
    let target = body.target_user_id.as_deref().unwrap_or_default();
    let role = body.role.as_deref().unwrap_or_default();

    if target.is_empty() || role.is_empty() {
        return Ok(bad_request("targetUserId と role が必要です。"));
    }
    if role != "user" && role != "admin" {
        return Ok(bad_request("role は 'user' または 'admin' のみ指定可能です。"));
    }
    // 自己降格防止。
    if target == admin_id && role == "user" {
        return Ok(bad_request("自分自身の Admin 権限を解除することはできません。"));
    }
    if !repo::update_user_role(&db, target, role).await? {
        return Ok(bad_request(
            "ロールの変更に失敗しました。ユーザーが存在しない可能性があります。",
        ));
    }
    yuuka_auth::audit::add_audit_log(&db, admin_id, "admin.role_change", Some(target), Some(role))
        .await;
    // ロール変更を即時反映するためセッションを失効（再ログインさせる）。
    rt.sessions.destroy_all_for_user(target).await;
    Ok(ok_message(&format!(
        "ユーザー {target} のロールを {role} に変更しました。"
    )))
}

pub(crate) async fn users_delete(
    admin: AdminUser,
    Extension(rt): Extension<Arc<AdminRuntime>>,
    State(db): State<Db>,
    Json(body): Json<TargetUserInput>,
) -> Result<Response, ApiError> {
    let admin_id = admin.0.discord_id.as_str();
    let target = body.target_user_id.as_deref().unwrap_or_default();
    if target.is_empty() {
        return Ok(bad_request("targetUserId が必要です。"));
    }
    if target == admin_id {
        return Ok(bad_request("自分自身は削除できません。"));
    }
    // 対象ユーザーがオーナーの起動中 Bot を停止（system_default を除く・runtime シーム）。
    for bot_id in repo::bot_ids_owned_by(&db, target).await? {
        rt.bots.stop(&bot_id).await;
    }
    if !repo::delete_user(&db, target).await? {
        return Ok(not_found("ユーザーが見つかりません。"));
    }
    rt.sessions.destroy_all_for_user(target).await;
    yuuka_auth::audit::add_audit_log(&db, admin_id, "admin.user_delete", Some(target), None).await;
    Ok(ok_message(&format!(
        "ユーザー {target} を削除しました（関連データも削除されました）。"
    )))
}

// ─── 監査ログ ─────────────────────────────────────────────────────────────────

pub(crate) async fn audit_logs(
    _admin: AdminUser,
    State(db): State<Db>,
    Query(q): Query<AuditQuery>,
) -> Result<Response, ApiError> {
    let action = q.action.as_deref().filter(|s| !s.is_empty());
    // Node parity: parseInt 寛容パース → limit は上限 500、offset は 0 下限。
    let limit = parse_int_or(q.limit.as_deref(), 200).min(500);
    let offset = parse_int_or(q.offset.as_deref(), 0).max(0);
    let logs = repo::list_audit_logs(&db, limit, action, offset).await?;
    let total = repo::count_audit_logs(&db, action).await?;
    Ok(json_ok(json!({ "success": true, "logs": logs, "total": total })))
}

// ─── Bot 管理 ─────────────────────────────────────────────────────────────────

pub(crate) async fn bots(
    _admin: AdminUser,
    Extension(rt): Extension<Arc<AdminRuntime>>,
    State(db): State<Db>,
) -> Result<Response, ApiError> {
    let mut bots = repo::list_all_bots(&db).await?;
    for bot in &mut bots {
        bot.is_running = rt.bots.is_running(&bot.id);
    }
    Ok(json_ok(json!({ "success": true, "bots": bots })))
}

pub(crate) async fn bots_suspend(
    admin: AdminUser,
    Extension(rt): Extension<Arc<AdminRuntime>>,
    State(db): State<Db>,
    Json(body): Json<BotIdInput>,
) -> Result<Response, ApiError> {
    let bot_id = body.bot_id.as_deref().unwrap_or_default();
    if bot_id.is_empty() {
        return Ok(bad_request("botId が必要です。"));
    }
    // 動作中の Bot クライアントを停止（runtime シーム）。
    rt.bots.stop(bot_id).await;
    if !repo::suspend_bot(&db, bot_id).await? {
        return Ok(bad_request("Botの停止処分に失敗しました。"));
    }
    yuuka_auth::audit::add_audit_log(&db, &admin.0.discord_id, "admin.bot_suspend", Some(bot_id), None)
        .await;
    Ok(ok_message(&format!(
        "Bot {bot_id} を停止処分にしました。Discordクライアントは停止されました。"
    )))
}

pub(crate) async fn bots_unsuspend(
    admin: AdminUser,
    State(db): State<Db>,
    Json(body): Json<BotIdInput>,
) -> Result<Response, ApiError> {
    let bot_id = body.bot_id.as_deref().unwrap_or_default();
    if bot_id.is_empty() {
        return Ok(bad_request("botId が必要です。"));
    }
    if !repo::unsuspend_bot(&db, bot_id).await? {
        return Ok(bad_request("停止処分の解除に失敗しました。"));
    }
    yuuka_auth::audit::add_audit_log(
        &db,
        &admin.0.discord_id,
        "admin.bot_unsuspend",
        Some(bot_id),
        None,
    )
    .await;
    Ok(ok_message(&format!(
        "Bot {bot_id} の停止処分を解除しました。所有者が再起動できるようになりました。"
    )))
}

// ─── 招待コード ───────────────────────────────────────────────────────────────

pub(crate) async fn invite_codes_list(
    _admin: AdminUser,
    State(db): State<Db>,
) -> Result<Response, ApiError> {
    let codes = repo::list_invite_codes(&db).await?;
    Ok(json_ok(json!({ "success": true, "codes": codes })))
}

pub(crate) async fn invite_codes_create(
    admin: AdminUser,
    State(db): State<Db>,
    Json(body): Json<CodeInput>,
) -> Result<Response, ApiError> {
    let code = body.code.as_deref().map(str::trim).unwrap_or_default();
    if code.is_empty() {
        return Ok(bad_request("招待コードを入力してください。"));
    }
    repo::create_invite_code(&db, code, &admin.0.discord_id).await?;
    yuuka_auth::audit::add_audit_log(&db, &admin.0.discord_id, "admin.invite_create", Some(code), None)
        .await;
    Ok(ok_message(&format!("招待コード「{code}」を作成しました。")))
}

pub(crate) async fn invite_codes_revoke(
    admin: AdminUser,
    State(db): State<Db>,
    Path(code): Path<String>,
) -> Result<Response, ApiError> {
    if !repo::revoke_invite_code(&db, &code).await? {
        return Ok(bad_request("未使用の招待コードのみ無効化できます。"));
    }
    yuuka_auth::audit::add_audit_log(&db, &admin.0.discord_id, "admin.invite_revoke", Some(&code), None)
        .await;
    Ok(ok_message(&format!("招待コード「{code}」を無効化しました。")))
}

pub(crate) async fn invite_codes_delete(
    admin: AdminUser,
    State(db): State<Db>,
    Path(code): Path<String>,
) -> Result<Response, ApiError> {
    if !repo::delete_invite_code(&db, &code).await? {
        return Ok(bad_request("未使用の招待コードのみ削除できます。"));
    }
    yuuka_auth::audit::add_audit_log(&db, &admin.0.discord_id, "admin.invite_delete", Some(&code), None)
        .await;
    Ok(ok_message(&format!("招待コード「{code}」を削除しました。")))
}

// ─── レスポンスヘルパ・小ユーティリティ ───────────────────────────────────────

/// 200 で任意の JSON 本文を返す。
fn json_ok(body: serde_json::Value) -> Response {
    (StatusCode::OK, Json(body)).into_response()
}

/// 200 `{success:true, message}`。
fn ok_message(message: &str) -> Response {
    json_ok(json!({ "success": true, "message": message }))
}

/// 400 `{success:false, message}`。
fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

/// 404 `{success:false, message}`。
fn not_found(message: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

/// `Some(非空文字列)` のみ通す（Node の `value || fallback` で空文字がフォールバックする挙動）。
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

/// JS `parseInt` 相当の寛容パース（先頭の符号 + 数字列のみ解釈・失敗時は `default`）。
///
/// clippy の `indexing_slicing` を避けるため文字イテレータで走査する。
fn parse_int_or(value: Option<&str>, default: i64) -> i64 {
    let Some(raw) = value else {
        return default;
    };
    let mut chars = raw.trim_start().chars().peekable();
    let mut sign: i64 = 1;
    match chars.peek() {
        Some('+') => {
            chars.next();
        }
        Some('-') => {
            sign = -1;
            chars.next();
        }
        _ => {}
    }
    let digits: String = chars.take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return default;
    }
    digits.parse::<i64>().map(|n| sign * n).unwrap_or(default)
}

/// system-settings の URL 検証（Node: 空欄・`/` 始まりは許可、それ以外は絶対 URL を要求）。
///
/// `new URL()` の実用近似として `scheme://rest`（scheme は英数 + `+-.`・rest 非空）を要求する。
fn is_valid_setting_url(value: &str) -> bool {
    if value.is_empty() || value.starts_with('/') {
        return true;
    }
    match value.split_once("://") {
        Some((scheme, rest)) => {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                && !rest.is_empty()
        }
        None => false,
    }
}
