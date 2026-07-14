//! Bot 属性（プリセット）管理 Web-API（Node `botAttributeRoutes` の preset 部分）。
//!
//! プリセット一覧 / Bot 属性（プリセット）変更 / Admin のプリセット表示名・レート制限既定値。
//! 増分 7 の [`crate::preset`] を消費する純 DB ルータ（seam/crypto 不要）。
//! **残（別増分）**: usage（利用量集計）・modules（enabled_modules）・assistant-*（gemini-key/persona/
//! guilds/members/roles/guild-note〔暗号・ギルド設定サブシステム〕）。

use std::sync::Arc;

use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use yuuka_crypto::SystemCrypto;
use yuuka_web::{AdminUser, AppState, AuthenticatedUser, Db};

use crate::bot_repo::{self, EncryptedTriplet};
use crate::module_catalog;
use crate::preset::{self, BotPresetId};

/// Gemini キー暗号化用 crypto（省略可・Extension で運ぶ newtype）。
#[derive(Clone)]
struct AttrCrypto(Option<Arc<SystemCrypto>>);

/// レート制限既定値（Node `RATE_LIMIT_DEFAULTS`・`system_settings` キーと既定値）。
const RATE_LIMITS: [(&str, &str, i64); 3] = [
    ("userPerMinute", "mcp_rate_user_per_minute", 5),
    ("userPerDay", "mcp_rate_user_per_day", 100),
    ("guildPerDay", "mcp_rate_guild_per_day", 1000),
];

#[derive(Debug, Deserialize)]
struct BotIdQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

/// Bot 属性ルータ（既定 crypto なし・`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    routes_with(None)
}

/// crypto を注入して Bot 属性ルータを組む（Gemini キー暗号化・main で SystemCrypto を渡す）。
pub fn routes_with(crypto: Option<Arc<SystemCrypto>>) -> Router<AppState> {
    Router::new()
        .route("/api/bots/presets", get(list_presets))
        .route("/api/bots/attributes", post(change_attributes))
        .route(
            "/api/bots/assistant/guild-note",
            get(guild_note_get).post(guild_note_set),
        )
        .route("/api/bots/assistant/persona", post(set_persona))
        .route("/api/bots/assistant/guilds", post(set_guild))
        .route("/api/bots/assistant/members", post(set_member))
        .route("/api/bots/assistant/roles", post(set_role))
        .route("/api/bots/assistant/gemini-key", post(set_gemini_key))
        .route("/api/bots/modules", get(get_modules).post(set_modules))
        .route("/api/bots/usage", get(get_usage))
        .route(
            "/api/admin/bot-attribute-settings",
            get(admin_get).post(admin_set),
        )
        .layer(Extension(AttrCrypto(crypto)))
}

// ─── GET /api/bots/usage（API 利用量サマリ・アクセス権のある全ユーザー・読み取り専用） ──

#[derive(Debug, Deserialize)]
struct UsageQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    /// 日数（文字列で受け、JS `parseInt` 相当で寛容に解釈する）。
    #[serde(default)]
    days: Option<String>,
}

/// JS `parseInt(s, 10)` の先頭数値抽出（trim → 任意符号 → 先頭連続数字）。数字が無ければ `None`（NaN）。
fn parse_int_prefix(s: &str) -> Option<i64> {
    let mut chars = s.trim().chars().peekable();
    let mut sign = 1_i64;
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
        return None;
    }
    digits.parse::<i64>().ok().map(|n| sign * n)
}

async fn get_usage(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<UsageQuery>,
) -> Response {
    // botId をアクセス認可つきで解決（未指定・アクセス不可は system_default へフォールバック＝
    // 他人の Bot の利用量を覗かせない・Node `resolveBotId`）。書き込みは無いので owner 限定にしない。
    let bot_id = match yuuka_web::resolve_scope(&user.0, &db, q.bot_id.as_deref()).await {
        Ok(scope) => scope.bot_id().as_str().to_owned(),
        Err(_) => return server_error(),
    };
    // days = parseInt(days || "14"); 有限>0 なら min(,90)、それ以外は 14（Node）。
    let days = match q.days.as_deref().and_then(parse_int_prefix) {
        Some(n) if n > 0 => n.min(90),
        _ => 14,
    };
    let usage = match crate::message_log::get_bot_usage_series(&db, &bot_id, days).await {
        Ok(u) => u,
        Err(_) => return server_error(),
    };
    let rate_limits = match rate_limits_json(&db).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    let series: Vec<Value> = usage
        .series
        .iter()
        .map(|p| {
            json!({ "date": p.date, "requests": p.requests, "responses": p.responses })
        })
        .collect();
    Json(json!({
        "success": true,
        "days": days,
        "series": series,
        "totals": { "requests": usage.total_requests, "responses": usage.total_responses },
        "rate_limits": rate_limits,
    }))
    .into_response()
}

// ─── GET/POST /api/bots/modules（有効モジュール・アクセス権のある全ユーザー） ──

#[derive(Debug, Deserialize)]
struct ModulesQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

async fn get_modules(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<ModulesQuery>,
) -> Response {
    let uid = user.0.discord_id;
    // GET は `?botId=` で受ける（Node は body→query だが GET のボディは実質未使用）。
    let bot_id = q.bot_id.clone().unwrap_or_default();
    if bot_id.is_empty() || !access_ok(&db, &uid, &bot_id).await {
        return not_found_bot();
    }
    let caps = match resolve_caps(&db, &bot_id).await {
        Ok(c) => c,
        Err(_) => return server_error(),
    };
    let override_json = match bot_repo::get_user_modules(&db, &bot_id, &uid).await {
        Ok(o) => o,
        Err(_) => return server_error(),
    };
    let has_override = override_json.is_some();
    // override あり: `parse(override) ?? 空集合`。無し: Bot 既定（None=全有効）。
    let enabled = if has_override {
        Some(parse_enabled_modules(override_json.as_deref()).unwrap_or_default())
    } else {
        let default_json = match bot_repo::bot_enabled_modules(&db, &bot_id).await {
            Ok(d) => d,
            Err(_) => return server_error(),
        };
        parse_enabled_modules(default_json.as_deref())
    };

    let modules: Vec<Value> = module_catalog::SELECTABLE_MODULES
        .iter()
        .filter(|m| m.cap == "core" || caps.has(m.cap))
        .map(|m| {
            let enabled_flag = enabled.as_ref().is_none_or(|set| set.contains(m.id));
            json!({
                "id": m.id,
                "cap": m.cap,
                "label": m.label,
                "description": m.description,
                "settingsKey": m.settings_key,
                "enabled": enabled_flag,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "has_override": has_override,
            "all_enabled": enabled.is_none(),
            "modules": modules,
        })),
    )
        .into_response()
}

async fn set_modules(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot_id = body
        .get("botId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if bot_id.is_empty() || !access_ok(&db, &uid, &bot_id).await {
        return not_found_bot();
    }
    let raw = body.get("enabledModules");
    // null / "all" は上書き解除（Bot 既定へ）。
    if matches!(raw, None | Some(Value::Null)) || raw.and_then(Value::as_str) == Some("all") {
        if let Err(_e) = bot_repo::set_user_modules(&db, &bot_id, &uid, None).await {
            return server_error();
        }
        return ok_message(
            "個別設定を解除し、既定に戻しました（次のメッセージ処理から反映されます）。",
        );
    }
    let Some(arr) = raw.and_then(Value::as_array) else {
        return bad_request("enabledModules は配列または null が必要です。");
    };
    let caps = match resolve_caps(&db, &bot_id).await {
        Ok(c) => c,
        Err(_) => return server_error(),
    };
    // 既知 selectable かつ当該 Bot の capability 配下の ID のみ（重複排除・順序保持）。
    let mut accepted: Vec<String> = Vec::new();
    for id in arr.iter().filter_map(Value::as_str) {
        if accepted.iter().any(|a| a == id) {
            continue;
        }
        if !module_catalog::is_known_selectable(id) {
            continue;
        }
        let cap_ok = module_catalog::find(id).is_some_and(|m| m.cap == "core" || caps.has(m.cap));
        if cap_ok {
            accepted.push(id.to_owned());
        }
    }
    if let Err(_e) = bot_repo::set_user_modules(&db, &bot_id, &uid, Some(accepted.clone())).await {
        return server_error();
    }
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "enabledModules": accepted,
            "message": "有効な機能を更新しました（次のメッセージ処理から反映されます）。",
        })),
    )
        .into_response()
}

/// アクセス権チェック（system_default は常に可・Node `hasBotAccess`）。エラー時は不可扱い。
async fn access_ok(db: &Db, uid: &str, bot_id: &str) -> bool {
    yuuka_web::has_bot_access(db, uid, bot_id)
        .await
        .unwrap_or(false)
}

/// Bot の capability 集合（Node `resolveBotCapabilities`＝bot 存在時 parse・不在は秘書相当フル）。
async fn resolve_caps(
    db: &Db,
    bot_id: &str,
) -> Result<yuuka_core::CapabilitySet, yuuka_core::DbError> {
    Ok(bot_repo::get_bot(db, bot_id)
        .await?
        .map_or_else(bot_repo::secretary_full_capabilities, |b| {
            b.capability_set()
        }))
}

/// 有効モジュール JSON をパースする（Node `parseEnabledModules`）。`None`=全有効。
/// 配列は既知 selectable ID のみ採用・非配列/パース不能/None 入力は `None`（安全側で全有効）。
fn parse_enabled_modules(raw: Option<&str>) -> Option<std::collections::HashSet<String>> {
    let raw = raw?;
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(arr)) => Some(
            arr.iter()
                .filter_map(Value::as_str)
                .filter(|id| module_catalog::is_known_selectable(id))
                .map(str::to_owned)
                .collect(),
        ),
        _ => None,
    }
}

fn not_found_bot() -> Response {
    status_json(StatusCode::NOT_FOUND, "Botが見つかりません。")
}

// ─── POST /api/bots/assistant/gemini-key（Bot 専用 Gemini キー・owner/Admin） ─

async fn set_gemini_key(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(AttrCrypto(crypto)): Extension<AttrCrypto>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot = match owned_bot_from_body(&db, &uid, &body).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let api_key = body.get("apiKey").and_then(Value::as_str).unwrap_or("");

    // 空欄はクリア（キー未設定の Bot は応答停止）。
    if api_key.trim().is_empty() {
        if let Err(_e) = bot_repo::update_bot_gemini_key(&db, &bot.id, None).await {
            return server_error();
        }
        // 監査: target=bot.id・detail="cleared"（Node addAuditLog の 3/4 引数）。
        yuuka_auth::audit::add_audit_log(
            &db,
            &uid,
            "bot.gemini_key_change",
            Some(&bot.id),
            Some("cleared"),
        )
        .await;
        return ok_message(
            "Bot専用APIキーを削除しました。キーが設定されるまでこのBotは応答しません。",
        );
    }
    // マスク済み（未変更）。
    if api_key.starts_with("••••") {
        return ok_message("APIキーは変更されていません。");
    }
    // 形式検証。
    if !is_likely_gemini_key(api_key.trim()) {
        return bad_request(
            "Gemini APIキーの形式が正しくありません。「AIza」で始まるキーを入力してください（Google AI Studio で取得）。",
        );
    }
    // 暗号化して保存。
    let Some(crypto) = crypto.as_deref() else {
        return server_error();
    };
    let enc = match crypto.encrypt_text(api_key.trim()) {
        Ok(e) => EncryptedTriplet {
            encrypted: e.encrypted,
            iv: e.iv,
            tag: e.auth_tag,
        },
        Err(_) => return server_error(),
    };
    if let Err(_e) = bot_repo::update_bot_gemini_key(&db, &bot.id, Some(enc)).await {
        return server_error();
    }
    yuuka_auth::audit::add_audit_log(
        &db,
        &uid,
        "bot.gemini_key_change",
        Some(&bot.id),
        Some("updated"),
    )
    .await;
    ok_message("Bot専用のGemini APIキーを保存しました。")
}

/// Gemini API キーらしさ（Node `isLikelyGeminiKey` = `^AIza[0-9A-Za-z_-]{30,}$`）。
fn is_likely_gemini_key(value: &str) -> bool {
    match value.strip_prefix("AIza") {
        Some(rest) => {
            rest.len() >= 30
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        }
        None => false,
    }
}

fn ok_message(message: &str) -> Response {
    (
        StatusCode::OK,
        Json(json!({ "success": true, "message": message })),
    )
        .into_response()
}

/// `action === "remove" ? remove : add`（Node）。
fn is_remove(body: &Value) -> bool {
    body.get("action").and_then(Value::as_str) == Some("remove")
}

async fn owned_bot_from_body(
    db: &Db,
    uid: &str,
    body: &Value,
) -> Result<bot_repo::BotRow, Response> {
    let bot_id = body.get("botId").and_then(Value::as_str).unwrap_or("");
    require_owned_bot(db, uid, bot_id).await
}

// ─── POST /api/bots/assistant/guilds（応答許可ギルド・owner/Admin） ──────────

async fn set_guild(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot = match owned_bot_from_body(&db, &uid, &body).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let guild_id = trimmed(&body, "guildId");
    let remove = is_remove(&body);
    if !is_snowflake(&guild_id) {
        return bad_request("ギルドID（数字）を入力してください。");
    }
    let ok = match if remove {
        bot_repo::remove_allowed_guild(&db, &bot.id, &guild_id).await
    } else {
        bot_repo::add_allowed_guild(&db, &bot.id, &guild_id).await
    } {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    audit_change(
        &db,
        &uid,
        "bot.guild_allow_change",
        &format!("{}:{}", guild_id, action_word(remove)),
    )
    .await;
    let guilds = match bot_repo::list_allowed_guilds(&db, &bot.id).await {
        Ok(g) => g,
        Err(_) => return server_error(),
    };
    let list: Vec<Value> = guilds
        .iter()
        .map(|g| json!({ "bot_id": g.bot_id, "guild_id": g.guild_id, "created_at": g.created_at }))
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "guilds": list,
            "message": change_message(ok, remove, "応答許可ギルドへ追加しました。", "応答許可ギルドから削除しました。"),
        })),
    )
        .into_response()
}

// ─── POST /api/bots/assistant/members（利用メンバー・owner/Admin） ───────────

async fn set_member(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot = match owned_bot_from_body(&db, &uid, &body).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let guild_id = trimmed(&body, "guildId");
    let member_id = trimmed(&body, "userId");
    let remove = is_remove(&body);
    if !is_snowflake(&guild_id) || !is_snowflake(&member_id) {
        return bad_request("ギルドIDとユーザーID（数字）を入力してください。");
    }
    let ok = match if remove {
        bot_repo::remove_bot_member(&db, &bot.id, &guild_id, &member_id).await
    } else {
        bot_repo::add_bot_member(&db, &bot.id, &guild_id, &member_id, &uid).await
    } {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    let action = if remove {
        "bot.member_remove"
    } else {
        "bot.member_add"
    };
    audit_change(
        &db,
        &uid,
        action,
        &format!("{}:{}:{}", bot.id, guild_id, member_id),
    )
    .await;
    let members = match bot_repo::list_bot_members(&db, &bot.id).await {
        Ok(m) => m,
        Err(_) => return server_error(),
    };
    let list: Vec<Value> = members
        .iter()
        .map(|m| {
            json!({
                "bot_id": m.bot_id, "guild_id": m.guild_id, "user_id": m.user_id,
                "added_by": m.added_by, "created_at": m.created_at,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "members": list,
            "message": change_message(ok, remove, "利用メンバーへ追加しました。", "利用メンバーから削除しました。"),
        })),
    )
        .into_response()
}

// ─── POST /api/bots/assistant/roles（利用可能ロール・owner/Admin） ───────────

async fn set_role(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot = match owned_bot_from_body(&db, &uid, &body).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let guild_id = trimmed(&body, "guildId");
    let role_id = trimmed(&body, "roleId");
    let role_name = body.get("roleName").and_then(Value::as_str);
    let remove = is_remove(&body);
    if !is_snowflake(&guild_id) || !is_snowflake(&role_id) {
        return bad_request("ギルドIDとロールID（数字）を入力してください。");
    }
    let ok = match if remove {
        bot_repo::remove_allowed_role(&db, &bot.id, &guild_id, &role_id).await
    } else {
        bot_repo::add_allowed_role(&db, &bot.id, &guild_id, &role_id, &uid, role_name).await
    } {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    let action = if remove {
        "bot.role_remove"
    } else {
        "bot.role_add"
    };
    audit_change(
        &db,
        &uid,
        action,
        &format!("{}:{}:{}", bot.id, guild_id, role_id),
    )
    .await;
    let roles = match bot_repo::list_allowed_roles(&db, &bot.id).await {
        Ok(r) => r,
        Err(_) => return server_error(),
    };
    let list: Vec<Value> = roles
        .iter()
        .map(|r| {
            json!({
                "bot_id": r.bot_id, "guild_id": r.guild_id, "role_id": r.role_id,
                "role_name": r.role_name, "added_by": r.added_by, "created_at": r.created_at,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "roles": list,
            "message": change_message(ok, remove, "利用可能ロールへ追加しました。", "利用可能ロールから削除しました。"),
        })),
    )
        .into_response()
}

fn trimmed(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned()
}

fn action_word(remove: bool) -> &'static str {
    if remove {
        "remove"
    } else {
        "add"
    }
}

fn change_message(
    ok: bool,
    remove: bool,
    add_msg: &'static str,
    remove_msg: &'static str,
) -> &'static str {
    if !ok {
        "変更はありませんでした。"
    } else if remove {
        remove_msg
    } else {
        add_msg
    }
}

async fn audit_change(db: &Db, uid: &str, action: &str, target: &str) {
    yuuka_auth::audit::add_audit_log(db, uid, action, Some(target), None).await;
}

// ─── POST /api/bots/assistant/persona（Bot 単位ペルソナ・owner/Admin） ───────

async fn set_persona(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotIdQuery>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot_id = body
        .get("botId")
        .and_then(Value::as_str)
        .or(q.bot_id.as_deref())
        .unwrap_or("")
        .to_owned();
    let bot = match require_owned_bot(&db, &uid, &bot_id).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };

    // personaId: null/未指定/空文字は解除・それ以外は整数必須。
    let raw = body.get("personaId");
    let persona_id: Option<i64> = match raw {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(v) => match int_value(v) {
            Some(id) => Some(id),
            None => return bad_request("personaId が不正です。"),
        },
    };

    // 設定時は owner 所有 or 公開のみ許可。
    if let Some(pid) = persona_id {
        let meta = match bot_repo::get_persona_owner_public(&db, pid).await {
            Ok(m) => m,
            Err(_) => return server_error(),
        };
        let allowed = meta.is_some_and(|(owner, is_public)| owner == bot.owner_id || is_public);
        if !allowed {
            return status_json(
                StatusCode::FORBIDDEN,
                "Bot作成者が所有するペルソナ、または公開ペルソナのみ設定できます。",
            );
        }
    }

    let ok = match bot_repo::set_bot_persona(&db, &bot.id, persona_id).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    if ok {
        yuuka_auth::audit::add_audit_log(
            &db,
            &uid,
            "bot.persona_change",
            Some(&bot.id),
            Some(&persona_id.map_or_else(|| "cleared".to_owned(), |id| id.to_string())),
        )
        .await;
    }
    let message = if !ok {
        "ペルソナの設定に失敗しました。"
    } else if persona_id.is_none() {
        "ペルソナ設定を解除しました（デフォルトに戻ります）。"
    } else {
        "Botのペルソナを設定しました。"
    };
    (
        StatusCode::OK,
        Json(json!({ "success": ok, "message": message })),
    )
        .into_response()
}

/// `Number(x)` が整数か（数値の整数・整数文字列のみ・Node `Number.isInteger`）。
fn int_value(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// ギルド共有ノートの最大長（Node `BOT_NOTE_MAX_LENGTH`）。
const BOT_NOTE_MAX_LENGTH: usize = 10000;

#[derive(Debug, Deserialize)]
struct GuildNoteQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    #[serde(default, rename = "guildId")]
    guild_id: Option<String>,
}

// ─── GET/POST /api/bots/assistant/guild-note（owner/Admin） ──────────────────

async fn guild_note_get(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<GuildNoteQuery>,
) -> Response {
    let bot =
        match require_owned_bot(&db, &user.0.discord_id, q.bot_id.as_deref().unwrap_or("")).await {
            Ok(b) => b,
            Err(resp) => return resp,
        };
    let guild_id = q.guild_id.unwrap_or_default();
    if !is_snowflake(&guild_id) {
        return bad_request("guildId が必要です。");
    }
    let content = match bot_repo::bot_guild_note(&db, &bot.id, &guild_id).await {
        Ok(c) => c,
        Err(_) => return server_error(),
    };
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "content": content,
            "max_length": BOT_NOTE_MAX_LENGTH,
        })),
    )
        .into_response()
}

async fn guild_note_set(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<GuildNoteQuery>,
    Json(body): Json<Value>,
) -> Response {
    let bot_id = body
        .get("botId")
        .and_then(Value::as_str)
        .or(q.bot_id.as_deref())
        .unwrap_or("")
        .to_owned();
    let bot = match require_owned_bot(&db, &user.0.discord_id, &bot_id).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let guild_id = body
        .get("guildId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned();
    let content = body.get("content").and_then(Value::as_str).unwrap_or("");
    if !is_snowflake(&guild_id) {
        return bad_request("guildId が必要です。");
    }
    // 長さは Node `content.length`（UTF-16 単位）で判定。
    let len = content.encode_utf16().count();
    if len > BOT_NOTE_MAX_LENGTH {
        return bad_request(&format!(
            "共有ノートは{}文字以内です（現在: {}文字）",
            format_commas(BOT_NOTE_MAX_LENGTH),
            format_commas(len)
        ));
    }
    if let Err(_e) = bot_repo::set_bot_guild_note(&db, &bot.id, &guild_id, content).await {
        return status_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            "共有ノートの保存に失敗しました。",
        );
    }
    (
        StatusCode::OK,
        Json(json!({ "success": true, "message": "共有ノートを保存しました。" })),
    )
        .into_response()
}

/// requireOwnedBot（botId 必須→404→owner/Admin 403・Node）。エラー時は応答を `Err` で返す。
async fn require_owned_bot(db: &Db, uid: &str, bot_id: &str) -> Result<bot_repo::BotRow, Response> {
    if bot_id.is_empty() {
        return Err(bad_request("botId が必要です。"));
    }
    let bot = match bot_repo::get_bot(db, bot_id).await {
        Ok(Some(b)) => b,
        Ok(None) => return Err(status_json(StatusCode::NOT_FOUND, "Botが見つかりません。")),
        Err(_) => return Err(server_error()),
    };
    let is_admin = match bot_repo::is_admin(db, uid).await {
        Ok(v) => v,
        Err(_) => return Err(server_error()),
    };
    if bot.owner_id != uid && !is_admin {
        return Err(status_json(
            StatusCode::FORBIDDEN,
            "Botの作成者のみが設定を変更できます。",
        ));
    }
    Ok(bot)
}

/// `^\d{5,25}$`（Node `isSnowflake`）。
fn is_snowflake(value: &str) -> bool {
    let len = value.len();
    (5..=25).contains(&len) && value.bytes().all(|b| b.is_ascii_digit())
}

/// 3 桁区切りの 10 進表記（Node `Number.toLocaleString()` 相当・整数のみ）。
fn format_commas(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

// ─── GET /api/bots/presets（プリセット一覧・user） ──────────────────────────

async fn list_presets(_user: AuthenticatedUser, State(db): State<Db>) -> Response {
    match preset::list_presets(&db).await {
        Ok(presets) => (
            StatusCode::OK,
            Json(json!({ "success": true, "presets": presets_json(&presets) })),
        )
            .into_response(),
        Err(_) => server_error(),
    }
}

// ─── POST /api/bots/attributes（プリセット変更・owner/Admin） ────────────────

async fn change_attributes(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotIdQuery>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    // requireOwnedBot: botId 必須 → 404 → 403 の順（Node）。
    let bot_id = body
        .get("botId")
        .and_then(Value::as_str)
        .or(q.bot_id.as_deref())
        .unwrap_or("")
        .to_owned();
    if bot_id.is_empty() {
        return bad_request("botId が必要です。");
    }
    let bot = match bot_repo::get_bot(&db, &bot_id).await {
        Ok(Some(b)) => b,
        Ok(None) => return status_json(StatusCode::NOT_FOUND, "Botが見つかりません。"),
        Err(_) => return server_error(),
    };
    let is_admin = match bot_repo::is_admin(&db, &uid).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    if bot.owner_id != uid && !is_admin {
        return status_json(
            StatusCode::FORBIDDEN,
            "Botの作成者のみが設定を変更できます。",
        );
    }
    if bot.id == "system_default" {
        return bad_request("デフォルトBotの属性は変更できません。");
    }
    let Some(preset) = body
        .get("preset")
        .and_then(Value::as_str)
        .and_then(BotPresetId::from_id)
    else {
        return bad_request("不明なプリセットです。");
    };

    let ok = match preset::apply_bot_preset(&db, &bot.id, preset).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    if ok {
        yuuka_auth::audit::add_audit_log(
            &db,
            &uid,
            "bot.capabilities_change",
            Some(&bot.id),
            Some(preset.as_str()),
        )
        .await;
    }
    let message = if !ok {
        "属性の変更に失敗しました。".to_owned()
    } else if preset == BotPresetId::McpAssistant {
        "Botの属性を変更しました（次のメッセージ処理から反映されます）。 汎用モードの応答にはBot専用のGemini APIキー・応答許可ギルドの設定が必要です。".to_owned()
    } else {
        "Botの属性を変更しました（次のメッセージ処理から反映されます）。".to_owned()
    };
    (
        StatusCode::OK,
        Json(json!({ "success": ok, "message": message })),
    )
        .into_response()
}

// ─── GET/POST /api/admin/bot-attribute-settings（admin） ─────────────────────

async fn admin_get(_admin: AdminUser, State(db): State<Db>) -> Response {
    match settings_payload(&db).await {
        Ok(mut payload) => {
            payload.insert("success".to_owned(), json!(true));
            (StatusCode::OK, Json(Value::Object(payload))).into_response()
        }
        Err(_) => server_error(),
    }
}

async fn admin_set(admin: AdminUser, State(db): State<Db>, Json(body): Json<Value>) -> Response {
    // 表示名（present な string のみ・trim 非空）。
    let display_names = body.get("displayNames");
    for preset in BotPresetId::all() {
        let name = display_names
            .and_then(|d| d.get(preset.as_str()))
            .and_then(Value::as_str);
        if let Some(name) = name {
            if !name.trim().is_empty() {
                if let Err(_e) = preset::set_preset_display_name(&db, preset, name).await {
                    return server_error();
                }
            }
        }
    }

    // レート制限（Number(value) が整数 && >0 のときのみ設定）。
    let rate_limits = body.get("rateLimits");
    for (field, key, _default) in RATE_LIMITS {
        let value = rate_limits.and_then(|r| r.get(field));
        if let Some(parsed) = value.and_then(js_positive_int) {
            if let Err(_e) = preset::set_system_setting(&db, key, &parsed.to_string()).await {
                return server_error();
            }
        }
    }

    yuuka_auth::audit::add_audit_log(
        &db,
        &admin.0.discord_id,
        "admin.bot_attribute_settings_change",
        None,
        None,
    )
    .await;

    match settings_payload(&db).await {
        Ok(mut payload) => {
            payload.insert("success".to_owned(), json!(true));
            payload.insert("message".to_owned(), json!("Bot属性の設定を保存しました。"));
            (StatusCode::OK, Json(Value::Object(payload))).into_response()
        }
        Err(_) => server_error(),
    }
}

// ─── ヘルパ ──────────────────────────────────────────────────────────────────

/// `{ presets, rate_limits }`（success/message は呼び出し側で足す）。
async fn settings_payload(db: &Db) -> Result<serde_json::Map<String, Value>, yuuka_core::DbError> {
    let presets = preset::list_presets(db).await?;
    let mut map = serde_json::Map::new();
    map.insert("presets".to_owned(), presets_json(&presets));
    map.insert("rate_limits".to_owned(), rate_limits_json(db).await?);
    Ok(map)
}

/// プリセット一覧の JSON（Node `listPresets`＝`{ id, displayName, capabilities }`・camelCase）。
fn presets_json(presets: &[preset::PresetView]) -> Value {
    Value::Array(
        presets
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "displayName": p.display_name,
                    "capabilities": p.capabilities,
                })
            })
            .collect(),
    )
}

/// 現在のレート制限設定（`system_settings` 上書き or 既定・Node `getRateLimitSettings`）。
async fn rate_limits_json(db: &Db) -> Result<Value, yuuka_core::DbError> {
    let mut out = serde_json::Map::new();
    for (field, key, default) in RATE_LIMITS {
        let value = match preset::get_system_setting(db, key).await? {
            Some(raw) => raw
                .parse::<i64>()
                .ok()
                .filter(|&n| n > 0)
                .unwrap_or(default),
            None => default,
        };
        out.insert(field.to_owned(), json!(value));
    }
    Ok(Value::Object(out))
}

/// JS `Number(x)` が正の整数か（数値の整数・整数文字列のみ・Node `Number.isInteger && >0`）。
fn js_positive_int(v: &Value) -> Option<i64> {
    let n = match v {
        Value::Number(num) => num.as_i64()?,
        Value::String(s) => s.trim().parse::<i64>().ok()?,
        _ => return None,
    };
    (n > 0).then_some(n)
}

fn status_json(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

fn bad_request(message: &str) -> Response {
    status_json(StatusCode::BAD_REQUEST, message)
}

fn server_error() -> Response {
    status_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        "内部エラーが発生しました。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, WebConfig};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct FakeAuth;

    #[async_trait::async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            let known = ["owner", "admin", "other"];
            Ok(known.contains(&token).then(|| SessionUser {
                discord_id: token.to_owned(),
                username: token.to_owned(),
                role: if token == "admin" {
                    Role::Admin
                } else {
                    Role::User
                },
            }))
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_botattr_test_{}_{seq}.sqlite",
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
            for (uid, role) in [("owner", "user"), ("admin", "admin"), ("other", "user")] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt, role) \
                     VALUES (?1, ?1, 'x', 'x', ?2)",
                    rusqlite::params![uid, role],
                )
                .expect("seed user");
            }
            conn.execute(
                "INSERT INTO bots (id, user_id, name) VALUES ('b1', 'owner', 'TestBot')",
                [],
            )
            .expect("seed bot");
            // owner 所有ペルソナ(1)・他人の公開ペルソナ(2)・他人の非公開ペルソナ(3)。
            conn.execute(
                "INSERT INTO personas (id, owner_id, name, is_public) VALUES \
                 (1, 'owner', 'Mine', 0), (2, 'other', 'Pub', 1), (3, 'other', 'Priv', 0)",
                [],
            )
            .expect("seed personas");
        }
        db
    }

    fn app() -> axum::Router {
        let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), seed_db());
        super::routes().with_state(state)
    }

    async fn send(
        app: &axum::Router,
        method: &str,
        uri: &str,
        token: &str,
        body: &str,
    ) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("cookie", format!("__Host-yuuka-session={token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
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
    async fn presets_list_and_attribute_change() {
        let app = app();
        // presets 一覧（camelCase displayName）。
        let (st, j) = send(&app, "GET", "/api/bots/presets", "owner", "").await;
        assert_eq!(st, StatusCode::OK);
        let presets = j["presets"].as_array().unwrap();
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0]["id"], "secretary");
        assert_eq!(presets[0]["displayName"], "パーソナル秘書");

        // 属性変更ガード: botId 必須。
        let (st, _) = send(&app, "POST", "/api/bots/attributes", "owner", "{}").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // 不在 bot → 404。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/attributes",
            "owner",
            r#"{"botId":"nope","preset":"secretary"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        // 非所有者 → 403。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/attributes",
            "other",
            r#"{"botId":"b1","preset":"secretary"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // 不明プリセット → 400。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/attributes",
            "owner",
            r#"{"botId":"b1","preset":"bogus"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // 汎用モードへ変更 → success + 追記メッセージ。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/attributes",
            "owner",
            r#"{"botId":"b1","preset":"mcp_assistant"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);
        assert!(j["message"].as_str().unwrap().contains("汎用モード"));
    }

    #[tokio::test]
    async fn admin_settings_get_set_and_authz() {
        let app = app();
        // 非 admin は 401/403（AdminUser extractor）。
        let (st, _) = send(
            &app,
            "GET",
            "/api/admin/bot-attribute-settings",
            "owner",
            "",
        )
        .await;
        assert!(st == StatusCode::FORBIDDEN || st == StatusCode::UNAUTHORIZED);

        // admin GET: presets + rate_limits（既定値）。
        let (st, j) = send(
            &app,
            "GET",
            "/api/admin/bot-attribute-settings",
            "admin",
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["rate_limits"]["userPerMinute"], 5);
        assert_eq!(j["rate_limits"]["guildPerDay"], 1000);

        // admin POST: 表示名 + レート制限を更新。
        let (st, j) = send(
            &app,
            "POST",
            "/api/admin/bot-attribute-settings",
            "admin",
            r#"{"displayNames":{"secretary":" 秘書さん "},"rateLimits":{"userPerMinute":9,"userPerDay":0,"guildPerDay":"abc"}}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["message"], "Bot属性の設定を保存しました。");
        // 表示名は trim 反映・userPerMinute=9・userPerDay=0(無効,既定維持)・guildPerDay 非数(維持)。
        assert_eq!(j["rate_limits"]["userPerMinute"], 9);
        assert_eq!(j["rate_limits"]["userPerDay"], 100);
        assert_eq!(j["rate_limits"]["guildPerDay"], 1000);
        let sec = j["presets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "secretary")
            .unwrap();
        assert_eq!(sec["displayName"], "秘書さん");
    }

    #[tokio::test]
    async fn guild_note_get_set() {
        let app = app();
        let gid = "123456789012345678";

        // 未設定は空 + max_length。
        let (st, j) = send(
            &app,
            "GET",
            &format!("/api/bots/assistant/guild-note?botId=b1&guildId={gid}"),
            "owner",
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["content"], "");
        assert_eq!(j["max_length"], 10000);

        // 非所有者は 403。
        let (st, _) = send(
            &app,
            "GET",
            &format!("/api/bots/assistant/guild-note?botId=b1&guildId={gid}"),
            "other",
            "",
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // guildId 不正は 400（requireOwnedBot 通過後）。
        let (st, _) = send(
            &app,
            "GET",
            "/api/bots/assistant/guild-note?botId=b1&guildId=12",
            "owner",
            "",
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // 保存 → 再取得で反映。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/guild-note",
            "owner",
            &format!(r#"{{"botId":"b1","guildId":"{gid}","content":"共有メモ"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["message"], "共有ノートを保存しました。");
        let (_st, j) = send(
            &app,
            "GET",
            &format!("/api/bots/assistant/guild-note?botId=b1&guildId={gid}"),
            "owner",
            "",
        )
        .await;
        assert_eq!(j["content"], "共有メモ");

        // 上限超過は 400（カンマ区切りメッセージ）。
        let long: String = "x".repeat(10001);
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/guild-note",
            "owner",
            &format!(r#"{{"botId":"b1","guildId":"{gid}","content":"{long}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(j["message"].as_str().unwrap().contains("10,000文字以内"));
    }

    #[test]
    fn format_commas_matches_locale() {
        use super::format_commas;
        assert_eq!(format_commas(0), "0");
        assert_eq!(format_commas(10_000), "10,000");
        assert_eq!(format_commas(12_345), "12,345");
        assert_eq!(format_commas(1_234_567), "1,234,567");
    }

    #[tokio::test]
    async fn set_persona_ownership_and_clear() {
        let app = app();
        // 不正 personaId → 400。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/assistant/persona",
            "owner",
            r#"{"botId":"b1","personaId":"abc"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // 他人の非公開ペルソナ(3) → 403。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/assistant/persona",
            "owner",
            r#"{"botId":"b1","personaId":3}"#,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // 存在しないペルソナ → 403。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/assistant/persona",
            "owner",
            r#"{"botId":"b1","personaId":999}"#,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // owner 所有(1) → 設定成功。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/persona",
            "owner",
            r#"{"botId":"b1","personaId":1}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);
        assert_eq!(j["message"], "Botのペルソナを設定しました。");
        // 他人の公開(2) → 許可。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/assistant/persona",
            "owner",
            r#"{"botId":"b1","personaId":2}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        // 解除（null）。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/persona",
            "owner",
            r#"{"botId":"b1","personaId":null}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            j["message"],
            "ペルソナ設定を解除しました（デフォルトに戻ります）。"
        );
        // 非所有者は 403。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/assistant/persona",
            "other",
            r#"{"botId":"b1","personaId":1}"#,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn assistant_allowlists_guilds_members_roles() {
        let app = app();
        let g = "111111111111111111";
        let u = "222222222222222222";
        let r = "333333333333333333";

        // guilds: 不正 → 400。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/assistant/guilds",
            "owner",
            r#"{"botId":"b1","guildId":"12"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // guilds: 追加 → 一覧に出る。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/guilds",
            "owner",
            &format!(r#"{{"botId":"b1","guildId":"{g}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["message"], "応答許可ギルドへ追加しました。");
        assert_eq!(j["guilds"].as_array().unwrap()[0]["guild_id"], g);
        // 二重追加は「変更はありませんでした。」。
        let (_st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/guilds",
            "owner",
            &format!(r#"{{"botId":"b1","guildId":"{g}"}}"#),
        )
        .await;
        assert_eq!(j["message"], "変更はありませんでした。");
        // 削除。
        let (_st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/guilds",
            "owner",
            &format!(r#"{{"botId":"b1","guildId":"{g}","action":"remove"}}"#),
        )
        .await;
        assert_eq!(j["message"], "応答許可ギルドから削除しました。");
        assert_eq!(j["guilds"].as_array().unwrap().len(), 0);
        // 非所有者 403。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/assistant/guilds",
            "other",
            &format!(r#"{{"botId":"b1","guildId":"{g}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // members: 不正（userId 欠落）→ 400。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/assistant/members",
            "owner",
            &format!(r#"{{"botId":"b1","guildId":"{g}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // members: 追加。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/members",
            "owner",
            &format!(r#"{{"botId":"b1","guildId":"{g}","userId":"{u}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["members"].as_array().unwrap()[0]["user_id"], u);

        // roles: 追加（roleName 付き）。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/roles",
            "owner",
            &format!(r#"{{"botId":"b1","guildId":"{g}","roleId":"{r}","roleName":"VIP"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let role0 = &j["roles"].as_array().unwrap()[0];
        assert_eq!(role0["role_id"], r);
        assert_eq!(role0["role_name"], "VIP");
    }

    #[tokio::test]
    async fn gemini_key_set_clear_validate() {
        use secrecy::SecretString;
        use yuuka_crypto::SystemCrypto;
        let crypto = Arc::new(SystemCrypto::new(SecretString::from("test-secret-xyz")).unwrap());
        let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), seed_db());
        let app = super::routes_with(Some(crypto)).with_state(state);

        // 形式不正 → 400。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/assistant/gemini-key",
            "owner",
            r#"{"botId":"b1","apiKey":"notavalidkey"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // マスク済み → 未変更。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/gemini-key",
            "owner",
            r#"{"botId":"b1","apiKey":"••••abcd"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["message"], "APIキーは変更されていません。");
        // 正規キー → 保存。
        let key = format!("AIza{}", "a".repeat(35));
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/gemini-key",
            "owner",
            &format!(r#"{{"botId":"b1","apiKey":"{key}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["message"], "Bot専用のGemini APIキーを保存しました。");
        // 空欄 → クリア。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/assistant/gemini-key",
            "owner",
            r#"{"botId":"b1","apiKey":"  "}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(j["message"].as_str().unwrap().contains("削除しました"));
        // 非所有者 403。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/assistant/gemini-key",
            "other",
            &format!(r#"{{"botId":"b1","apiKey":"{key}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn modules_override_and_defaults() {
        let app = app();

        // アクセス不可（他人の bot）→ 404。
        let (st, _) = send(&app, "GET", "/api/bots/modules?botId=b1", "other", "").await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // 既定（override 無し）→ 全有効。
        let (st, j) = send(&app, "GET", "/api/bots/modules?botId=b1", "owner", "").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["has_override"], false);
        assert_eq!(j["all_enabled"], true);
        let mods = j["modules"].as_array().unwrap();
        assert!(mods.len() >= 14);
        assert!(mods.iter().all(|m| m["enabled"] == true));

        // 上書き設定（todo + note + 不明IDは無視 + 重複除去）。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/modules",
            "owner",
            r#"{"botId":"b1","enabledModules":["todo","note","bogus","todo"]}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["enabledModules"], serde_json::json!(["todo", "note"]));

        // GET に反映（has_override + 個別の enabled）。
        let (_st, j) = send(&app, "GET", "/api/bots/modules?botId=b1", "owner", "").await;
        assert_eq!(j["has_override"], true);
        assert_eq!(j["all_enabled"], false);
        let mods = j["modules"].as_array().unwrap();
        let todo = mods.iter().find(|m| m["id"] == "todo").unwrap();
        let finance = mods.iter().find(|m| m["id"] == "finance").unwrap();
        assert_eq!(todo["enabled"], true);
        assert_eq!(finance["enabled"], false);

        // 非配列 → 400。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/modules",
            "owner",
            r#"{"botId":"b1","enabledModules":"xyz"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // null で解除 → 既定へ。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/modules",
            "owner",
            r#"{"botId":"b1","enabledModules":null}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(j["message"].as_str().unwrap().contains("既定に戻しました"));
        let (_st, j) = send(&app, "GET", "/api/bots/modules?botId=b1", "owner", "").await;
        assert_eq!(j["has_override"], false);
        assert_eq!(j["all_enabled"], true);
    }

    #[tokio::test]
    async fn usage_route_shape_days_clamp_and_auth() {
        let app = app();
        // 既定 days=14・ログ無し system_default（未指定 botId）→ 全0・rate_limits あり。
        let (st, j) = send(&app, "GET", "/api/bots/usage", "owner", "").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["days"], serde_json::json!(14));
        assert_eq!(j["series"].as_array().unwrap().len(), 14);
        assert_eq!(j["totals"]["requests"], serde_json::json!(0));
        assert_eq!(j["totals"]["responses"], serde_json::json!(0));
        assert!(j["rate_limits"].is_object());
        let p0 = &j["series"][0];
        assert!(p0["date"].is_string());
        assert!(p0["requests"].is_number() && p0["responses"].is_number());

        // days=5 → 長さ5。
        let (_, j) = send(&app, "GET", "/api/bots/usage?days=5", "owner", "").await;
        assert_eq!(j["days"], serde_json::json!(5));
        assert_eq!(j["series"].as_array().unwrap().len(), 5);
        // days>90 → 90 にクランプ。
        let (_, j) = send(&app, "GET", "/api/bots/usage?days=365", "owner", "").await;
        assert_eq!(j["days"], serde_json::json!(90));
        assert_eq!(j["series"].as_array().unwrap().len(), 90);
        // 非数値 days → 14（Node parseInt NaN フォールバック）。
        let (_, j) = send(&app, "GET", "/api/bots/usage?days=abc", "owner", "").await;
        assert_eq!(j["days"], serde_json::json!(14));

        // 認証必須（不明トークン → 401）。
        let (st, _) = send(&app, "GET", "/api/bots/usage", "nobody", "").await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn usage_route_surfaces_real_counts() {
        // message_logs を持つ DB を組み、route が実カウントを返すことを確認する。
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_botattr_usage_{}_{seq}.sqlite",
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
            // 未指定 botId → system_default 宛の今日ぶん（user×1, assistant×1）。
            for role in ["user", "assistant"] {
                conn.execute(
                    "INSERT INTO message_logs (user_id, bot_id, role, content) \
                     VALUES ('owner', 'system_default', ?1, 'c')",
                    rusqlite::params![role],
                )
                .expect("seed log");
            }
        }
        let app = super::routes()
            .with_state(AppState::new(Arc::new(FakeAuth), WebConfig::default(), db));

        let (st, j) = send(&app, "GET", "/api/bots/usage?days=1", "owner", "").await;
        assert_eq!(st, StatusCode::OK);
        let series = j["series"].as_array().unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0]["requests"], serde_json::json!(1));
        assert_eq!(series[0]["responses"], serde_json::json!(1));
        assert_eq!(j["totals"]["requests"], serde_json::json!(1));
        assert_eq!(j["totals"]["responses"], serde_json::json!(1));
    }
}
