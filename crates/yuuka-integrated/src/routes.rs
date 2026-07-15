//! integrated ルートハンドラ（全て `AuthenticatedUser` extractor で auth:"user" を型強制）。
//!
//! レスポンス本文は Node `integratedRoutes.ts` の `sendJson` とバイト単位一致（フラット
//! `{success, ...}`・日本語 verbatim）。想定内の業務エラー（403/404/409/502）は明示的な JSON
//! レスポンスで返し、想定外の [`DbError`](yuuka_core::DbError) のみ `?` で [`ApiError`] に写像（500）。
//!
//! **Bot ライフサイクルは [`BotLifecycle`](crate::BotLifecycle) シーム越し**。Discord gateway 未配線時は
//! [`NullBotLifecycle`](crate::NullBotLifecycle) に縮退し、`run_status` は `{false,false}`・`start` は
//! `false` を返す（gateway 無しでは Discord Bot が本当に起動できない正直な縮退）。**DB 効果（stopped
//! フラグ・許可付与・Google 割当・floor 進行）は常に完全に働く**。start/restart は `set_bot_stopped(false)`
//! を先に反映してから `start`（Null は `false`）→ 502 を返す。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{json, Value};
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

use crate::repo::{self, js_int};
use crate::IntegratedRuntime;

// ─── 統合オーバービュー（1 コールでページ全体を構成） ──────────────────────────

#[allow(clippy::too_many_lines)]
pub(crate) async fn overview(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<IntegratedRuntime>>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;

    // 対象 Bot = 共有秘書(system_default) + 所有 Bot。system_default はヘルス表示のみ。
    let system_default = repo::get_bot(db, "system_default").await?;
    let owned: Vec<repo::BotRow> = repo::list_bots_owned_by(db, user_id)
        .await?
        .into_iter()
        .filter(|b| b.id != "system_default")
        .collect();

    let mut records: Vec<(repo::BotRow, bool)> = Vec::new();
    if let Some(sd) = system_default {
        records.push((sd, true));
    }
    for b in owned {
        records.push((b, false));
    }

    let mut bots = Vec::with_capacity(records.len());
    for (bot, is_system_default) in records {
        let caps = repo::parse_capabilities(bot.capabilities.as_deref());
        let status = rt.lifecycle.run_status(&bot.id);
        let granted_mcp_ids = repo::list_server_ids_for_bot(db, &bot.id, user_id).await?;
        let granted_credentials = repo::list_credential_names_for_bot(db, &bot.id, user_id).await?;
        let google_setting = yuuka_google::repo::get_bot_google_mode(db, &bot.id)
            .await?
            .to_json();
        bots.push(json!({
            "id": bot.id,
            "name": bot.name,
            "is_system_default": is_system_default,
            "preset": repo::preset_id_for(&caps),
            "suspended": bot.suspended,
            "stopped": bot.stopped,
            "has_token": bot.has_token,
            "running": status.running,
            "connected": status.connected,
            "discord_username": bot.discord_username,
            "discord_avatar_url": bot.discord_avatar_url,
            "granted_mcp_ids": granted_mcp_ids,
            "granted_credentials": granted_credentials,
            "google_setting": google_setting,
        }));
    }

    let mcp_servers: Vec<Value> = repo::list_servers_for_owner(db, user_id)
        .await?
        .into_iter()
        .map(|s| {
            let tools = repo::parse_tools_len(s.tools_cache.as_deref());
            json!({
                "id": s.id,
                "name": s.name,
                "endpoint_url": s.endpoint_url,
                "enabled": s.enabled,
                "has_auth": s.has_auth,
                "tools": tools,
            })
        })
        .collect();

    let credentials: Vec<Value> = repo::list_credential_services(db, user_id)
        .await?
        .into_iter()
        .map(|c| {
            json!({
                "service_name": c.service_name,
                "username": c.username,
                "url": c.url,
                "updated_at": c.updated_at,
            })
        })
        .collect();

    let google_accounts = yuuka_google::repo::list_accounts_safe(db, user_id).await?;

    Ok(json_ok(json!({
        "success": true,
        "bots": bots,
        "mcpServers": mcp_servers,
        "credentials": credentials,
        "googleAccounts": google_accounts,
    })))
}

// ─── Bot 起動 ──────────────────────────────────────────────────────────────────

pub(crate) async fn bots_start(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<IntegratedRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    let bot_id = body_bot_id(&body);
    let bot = repo::get_bot(db, &bot_id).await?;
    if let Some(resp) = owner_guard(bot.as_ref(), user_id, &bot_id) {
        return Ok(resp);
    }
    if let Some(resp) = suspended_and_token_guards(db, bot.as_ref(), &bot_id).await? {
        return Ok(resp);
    }
    // 希望状態=起動。stopped を解除する（DB 効果は常に働く）。
    repo::set_bot_stopped(db, &bot_id, false).await?;
    let ok = rt.lifecycle.start(&bot_id).await;
    yuuka_auth::audit::add_audit_log(db, user_id, "bot.owner_start", Some(&bot_id), None).await;
    let status = rt.lifecycle.run_status(&bot_id);
    Ok(lifecycle_response(
        ok,
        if ok {
            "起動しました。"
        } else {
            "起動に失敗しました（トークンを確認してください）。"
        },
        status,
    ))
}

// ─── Bot 停止 ──────────────────────────────────────────────────────────────────

pub(crate) async fn bots_stop(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<IntegratedRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    let bot_id = body_bot_id(&body);
    let bot = repo::get_bot(db, &bot_id).await?;
    if let Some(resp) = owner_guard(bot.as_ref(), user_id, &bot_id) {
        return Ok(resp);
    }
    rt.lifecycle.stop(&bot_id).await;
    repo::set_bot_stopped(db, &bot_id, true).await?;
    yuuka_auth::audit::add_audit_log(db, user_id, "bot.owner_stop", Some(&bot_id), None).await;
    Ok(json_ok(json!({
        "success": true,
        "message": "停止しました。",
        "running": false,
        "connected": false,
    })))
}

// ─── Bot 再起動 ────────────────────────────────────────────────────────────────

pub(crate) async fn bots_restart(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<IntegratedRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    let bot_id = body_bot_id(&body);
    let bot = repo::get_bot(db, &bot_id).await?;
    if let Some(resp) = owner_guard(bot.as_ref(), user_id, &bot_id) {
        return Ok(resp);
    }
    if let Some(resp) = suspended_and_token_guards(db, bot.as_ref(), &bot_id).await? {
        return Ok(resp);
    }
    // 再起動は起動状態で終わるため、希望状態=起動として stopped を解除する。
    repo::set_bot_stopped(db, &bot_id, false).await?;
    rt.lifecycle.stop(&bot_id).await;
    let ok = rt.lifecycle.start(&bot_id).await;
    yuuka_auth::audit::add_audit_log(db, user_id, "bot.owner_restart", Some(&bot_id), None).await;
    let status = rt.lifecycle.run_status(&bot_id);
    Ok(lifecycle_response(
        ok,
        if ok {
            "再起動しました。"
        } else {
            "再起動に失敗しました（トークンを確認してください）。"
        },
        status,
    ))
}

// ─── 会話履歴クリア ────────────────────────────────────────────────────────────

pub(crate) async fn bots_clear_history(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    let bot_id = body_bot_id(&body);
    let bot = repo::get_bot(db, &bot_id).await?;
    // 自分が会話できる Bot のみ（system_default は共有秘書）。
    let accessible = bot.is_some()
        && (bot_id == "system_default" || repo::has_bot_access(db, user_id, &bot_id).await?);
    if !accessible {
        return Ok(forbidden("このBotへのアクセス権がありません。"));
    }
    // 汎用モード Bot は会話が owner DM コンテキスト（別キー）に保存されるため、そちらをクリアする。
    let caps = repo::parse_capabilities(bot.as_ref().and_then(|b| b.capabilities.as_deref()));
    let floor_key = if repo::is_guild_assistant(&caps) {
        repo::bot_dm_context_floor_key(user_id, &bot_id)
    } else {
        repo::context_floor_key(user_id, &bot_id)
    };
    repo::clear_context_floor(db, user_id, &bot_id, floor_key).await?;
    yuuka_auth::audit::add_audit_log(db, user_id, "conversation.clear", Some(&bot_id), None).await;
    Ok(json_ok(json!({
        "success": true,
        "message":
            "会話履歴をクリアしました（次のメッセージから新しい会話になります。永続ログは保持されます）。",
    })))
}

// ─── 利用許可トグル: MCP ───────────────────────────────────────────────────────

pub(crate) async fn grants_mcp(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    let bot_id = body_bot_id(&body);
    let granted = body.get("granted") == Some(&Value::Bool(true));
    if !is_grant_target_bot(db, user_id, &bot_id).await? {
        return Ok(forbidden("対象Botへの権限がありません。"));
    }
    // serverId = Number(x)・Number.isInteger でなければ getServerById を呼ばず None。
    let server_id = body.get("serverId").and_then(js_int);
    let owner = match server_id {
        Some(id) => repo::get_server_owner(db, id).await?,
        None => None,
    };
    // server が無い or owner が本人でない（システムレベル `Some(None)` も含む）→ 403。
    let owned = matches!(&owner, Some(Some(o)) if o == user_id);
    let Some(server_id) = server_id.filter(|_| owned) else {
        return Ok(forbidden("対象MCPサーバーの所有者ではありません。"));
    };
    if granted {
        repo::grant_mcp_to_bot(db, &bot_id, user_id, server_id).await?;
    } else {
        repo::revoke_mcp_from_bot(db, &bot_id, user_id, server_id).await?;
    }
    Ok(json_ok(json!({ "success": true })))
}

// ─── 利用許可トグル: 認証情報 ──────────────────────────────────────────────────

pub(crate) async fn grants_credential(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    let bot_id = body_bot_id(&body);
    let granted = body.get("granted") == Some(&Value::Bool(true));
    // serviceName: trim + 小文字（非文字列は ""）。
    let raw_service = body
        .get("serviceName")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_default();
    if !is_grant_target_bot(db, user_id, &bot_id).await? {
        return Ok(forbidden("対象Botへの権限がありません。"));
    }
    if raw_service.is_empty() || !repo::credential_exists(db, user_id, &raw_service).await? {
        return Ok(not_found("対象の認証情報が見つかりません。"));
    }
    if granted {
        repo::grant_credential_to_bot(db, &bot_id, user_id, &raw_service).await?;
    } else {
        repo::revoke_credential_from_bot(db, &bot_id, user_id, &raw_service).await?;
    }
    Ok(json_ok(json!({ "success": true })))
}

// ─── 使用 Google アカウント割当（per-bot） ─────────────────────────────────────

pub(crate) async fn grants_google(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    let bot_id = body_bot_id(&body);
    let mode = body
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("primary");
    let bot = repo::get_bot(db, &bot_id).await?;
    // 所有 Bot のみ（system_default は共有秘書のため対象外）。
    let owned = bot.as_ref().is_some_and(|b| b.user_id == *user_id) && bot_id != "system_default";
    if !owned {
        return Ok(forbidden(
            "対象Botへの権限がありません（共有秘書は対象外）。",
        ));
    }
    if mode == "primary" {
        yuuka_google::repo::clear_bot_google_account(db, &bot_id).await?;
        return Ok(json_ok(json!({ "success": true })));
    }
    if mode == "none" {
        yuuka_google::repo::set_bot_google_account(db, &bot_id, None).await?;
        return Ok(json_ok(json!({ "success": true })));
    }
    // mode === "account"。
    let account_id = body.get("accountId").and_then(js_int);
    let acct = match account_id {
        Some(id) => yuuka_google::repo::get_account(db, id).await?,
        None => None,
    };
    let owned = matches!(&acct, Some(a) if a.user_id == *user_id);
    let Some(account_id) = account_id.filter(|_| owned) else {
        return Ok(forbidden(
            "対象Googleアカウントの所有者ではありません。",
        ));
    };
    yuuka_google::repo::set_bot_google_account(db, &bot_id, Some(account_id)).await?;
    Ok(json_ok(json!({ "success": true })))
}

// ─── Google アカウント: primary 変更 ───────────────────────────────────────────

pub(crate) async fn google_accounts_primary(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    let ok = match body.get("accountId").and_then(js_int) {
        Some(id) => yuuka_google::repo::set_primary(db, user_id, id).await?,
        None => false,
    };
    Ok(bool_response(ok, json!({ "success": ok })))
}

// ─── Google アカウント: 削除 ───────────────────────────────────────────────────

pub(crate) async fn google_accounts_delete(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<IntegratedRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    let account_id = body.get("accountId").and_then(js_int);
    let ok = match account_id {
        Some(id) => yuuka_google::repo::delete_account(db, user_id, id).await?,
        None => false,
    };
    if ok {
        if let Some(id) = account_id {
            rt.calendar.invalidate_account(id);
            yuuka_auth::audit::add_audit_log(
                db,
                user_id,
                "google.account_delete",
                Some(&id.to_string()),
                None,
            )
            .await;
        }
    }
    Ok(bool_response(ok, json!({ "success": ok })))
}

// ─── Google アカウント: 同期対象カレンダー更新 ─────────────────────────────────

pub(crate) async fn google_accounts_calendars(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<IntegratedRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    let account_id = body.get("accountId").and_then(js_int);
    // calendars: 配列なら string 要素のみ抽出・それ以外は []。
    let calendars: Vec<String> = body
        .get("calendars")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let acct = match account_id {
        Some(id) => yuuka_google::repo::get_account(db, id).await?,
        None => None,
    };
    let Some(acct) = acct.filter(|a| a.user_id == *user_id) else {
        return Ok((StatusCode::FORBIDDEN, Json(json!({ "success": false }))).into_response());
    };
    yuuka_google::repo::update_account_calendars(db, acct.id, &calendars).await?;
    rt.calendar.invalidate_account(acct.id);
    Ok(json_ok(json!({ "success": true })))
}

// ─── Google アカウント: 利用可能カレンダー一覧（Google API から取得） ──────────

pub(crate) async fn google_account_calendars_list(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<IntegratedRuntime>>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let db = &state.db;
    // path param を JS `Number()`+`Number.isInteger` 相当でコアース（非整数は取得せず 403）。
    let account_id = js_int(&Value::String(id));
    let acct = match account_id {
        Some(aid) => yuuka_google::repo::get_account(db, aid).await?,
        None => None,
    };
    let Some(acct) = acct.filter(|a| a.user_id == *user_id) else {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "success": false, "calendars": [] })),
        )
            .into_response());
    };
    // Null シームは常に空一覧を返す（gateway 未配線時の正直な縮退）。
    let calendars = rt
        .calendar
        .list_for_account(user_id, acct.id)
        .await
        .unwrap_or_default();
    Ok(json_ok(json!({ "success": true, "calendars": calendars })))
}

// ─── ガード・ユーティリティ ────────────────────────────────────────────────────

/// body の `botId` を取り出す（Node `typeof x === "string" ? x : ""`）。
fn body_bot_id(body: &Value) -> String {
    body.get("botId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// 起動/停止/再起動の共通 403 ガード（不在・非所有・system_default はいずれも 403）。
fn owner_guard(bot: Option<&repo::BotRow>, user_id: &str, bot_id: &str) -> Option<Response> {
    let owned = bot.is_some_and(|b| b.user_id == user_id);
    if !owned || bot_id == "system_default" {
        return Some(forbidden("このBotを操作する権限がありません。"));
    }
    None
}

/// start/restart の suspend(409) + トークン未設定(409) ガード。ここに来る時点で bot は所有確定。
async fn suspended_and_token_guards(
    db: &yuuka_web::Db,
    bot: Option<&repo::BotRow>,
    bot_id: &str,
) -> Result<Option<Response>, ApiError> {
    if repo::is_bot_suspended(db, bot_id).await? {
        return Ok(Some(conflict("このBotは管理者により停止されています。")));
    }
    if !bot.is_some_and(|b| b.has_token) {
        return Ok(Some(conflict(
            "Discordトークンが未設定です（Bot設定で登録してください）。",
        )));
    }
    Ok(None)
}

/// リソース許可の対象 Bot として有効か（Node `isGrantTargetBot`）。system_default は存在すれば可・
/// それ以外は owner 本人所有のみ可。
async fn is_grant_target_bot(
    db: &yuuka_web::Db,
    user_id: &str,
    bot_id: &str,
) -> Result<bool, ApiError> {
    let bot = repo::get_bot(db, bot_id).await?;
    if bot_id == "system_default" {
        return Ok(bot.is_some());
    }
    Ok(bot.is_some_and(|b| b.user_id == user_id))
}

/// start/restart の成否から `(200|502, {success, message, running, connected})` を組む。
fn lifecycle_response(ok: bool, message: &str, status: crate::BotRunStatus) -> Response {
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };
    (
        code,
        Json(json!({
            "success": ok,
            "message": message,
            "running": status.running,
            "connected": status.connected,
        })),
    )
        .into_response()
}

/// `ok ? 200 : 403` の bool レスポンス（Google primary/delete 用）。
fn bool_response(ok: bool, body: Value) -> Response {
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::FORBIDDEN
    };
    (code, Json(body)).into_response()
}

fn json_ok(body: Value) -> Response {
    (StatusCode::OK, Json(body)).into_response()
}

fn status_json(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

fn forbidden(message: &str) -> Response {
    status_json(StatusCode::FORBIDDEN, message)
}

fn not_found(message: &str) -> Response {
    status_json(StatusCode::NOT_FOUND, message)
}

fn conflict(message: &str) -> Response {
    status_json(StatusCode::CONFLICT, message)
}
