//! Bot 属性（プリセット）管理 Web-API（Node `botAttributeRoutes` の preset 部分）。
//!
//! プリセット一覧 / Bot 属性（プリセット）変更 / Admin のプリセット表示名・レート制限既定値。
//! 増分 7 の [`crate::preset`] を消費する純 DB ルータ（seam/crypto 不要）。
//! **残（別増分）**: usage（利用量集計）・modules（enabled_modules）・assistant-*（gemini-key/persona/
//! guilds/members/roles/guild-note〔暗号・ギルド設定サブシステム〕）。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use yuuka_web::{AdminUser, AppState, AuthenticatedUser, Db};

use crate::bot_repo;
use crate::preset::{self, BotPresetId};

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

/// Bot 属性ルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/bots/presets", get(list_presets))
        .route("/api/bots/attributes", post(change_attributes))
        .route(
            "/api/bots/assistant/guild-note",
            get(guild_note_get).post(guild_note_set),
        )
        .route("/api/bots/assistant/persona", post(set_persona))
        .route(
            "/api/admin/bot-attribute-settings",
            get(admin_get).post(admin_set),
        )
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
}
