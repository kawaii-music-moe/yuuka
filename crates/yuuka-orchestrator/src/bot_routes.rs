//! Bot インスタンス管理 Web-API（Node `botRoutes` の CRUD 部分・全て auth:user）。
//!
//! 一覧 / 作成 / 削除 / プロフィール編集。共有 3 本は [`crate::share_routes`]、sync-discord（Discord live
//! 依存）は後続。応答の Bot ビューは Node `botViewSchema` のホワイトリストに一致させ、暗号文トークン等の
//! 機密列は構造的に出さない（`has_token`/`has_gemini_key` の有無と稼働状態のみ）。
//!
//! **ランタイムのシーム**: 稼働状態（running/connected/shared）と削除時の停止は [`BotViewRuntime`] ポート
//! 越しに扱う。Discord 未配線時は既定 [`NullBotViewRuntime`]（非稼働・停止 no-op）へ縮退する。
//! `discord_application_id` はトークン復号（[`SystemCrypto::decrypt_text`]）→先頭 base64url セグメントで
//! 導出する（crypto 未注入・復号失敗時は同期済み値のみ／なければ null）。

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde_json::{json, Value};
use yuuka_core::CapabilitySet;
use yuuka_crypto::SystemCrypto;
use yuuka_web::{has_bot_access, AppState, AuthenticatedUser, Db};

use crate::bot_repo::{self, BotDetail};
use crate::discord_live::{DiscordLive, NullDiscordLive};
use crate::preset::{self, BotPresetId};

/// Bot ランタイムのシーム（稼働状態の問い合わせ + カスタム Bot の停止・Node の `client`/`customClients`）。
pub trait BotViewRuntime: Send + Sync {
    /// 共有デフォルト Bot が接続済みか（`client.isReady()`）。
    fn default_ready(&self) -> bool;
    /// 指定 Bot の専用クライアントが起動しているか（`customClients.has(id)`）。
    fn custom_running(&self, bot_id: &str) -> bool;
    /// 指定 Bot の専用クライアントが接続済みか（`client.isReady()`）。
    fn custom_ready(&self, bot_id: &str) -> bool;
    /// 専用クライアントを停止する（削除時・`stopCustomBot`）。
    fn stop_custom(&self, bot_id: &str);
}

/// Discord 未配線時の既定ランタイム（非稼働・停止 no-op）。
pub struct NullBotViewRuntime;

impl BotViewRuntime for NullBotViewRuntime {
    fn default_ready(&self) -> bool {
        false
    }
    fn custom_running(&self, _bot_id: &str) -> bool {
        false
    }
    fn custom_ready(&self, _bot_id: &str) -> bool {
        false
    }
    fn stop_custom(&self, _bot_id: &str) {}
}

/// app-id 導出用の crypto（省略可・Extension で運ぶための newtype）。
#[derive(Clone)]
struct ViewCrypto(Option<Arc<SystemCrypto>>);

/// Bot 管理ルータ（既定 [`NullBotViewRuntime`]・crypto なし）。
pub fn routes() -> Router<AppState> {
    routes_with(Arc::new(NullBotViewRuntime), None)
}

/// ランタイム/crypto を注入して Bot 管理ルータを組む（Discord/crypto 配線時に使う）。
pub fn routes_with(
    runtime: Arc<dyn BotViewRuntime>,
    crypto: Option<Arc<SystemCrypto>>,
) -> Router<AppState> {
    Router::new()
        .route(
            "/api/bots",
            get(list_bots).post(create_bot).delete(delete_bot),
        )
        .route("/api/bots/profile", post(update_profile))
        .route("/api/bots/sync-discord", post(sync_discord))
        .layer(Extension(runtime))
        .layer(Extension(ViewCrypto(crypto)))
        // sync-discord の Discord ライブ照会シーム（gateway 未配線時は Null＝Bot ユーザー無し）。
        .layer(Extension(
            Arc::new(NullDiscordLive) as Arc<dyn DiscordLive>,
        ))
}

// ─── GET /api/bots ───────────────────────────────────────────────────────────

async fn list_bots(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(rt): Extension<Arc<dyn BotViewRuntime>>,
    Extension(ViewCrypto(crypto)): Extension<ViewCrypto>,
) -> Response {
    let bots = match bot_repo::list_bots_for_user(&db, &user.0.discord_id).await {
        Ok(b) => b,
        Err(_) => return server_error(),
    };
    let (sec_name, mcp_name) = match preset_display_names(&db).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    let views: Vec<Value> = bots
        .iter()
        .map(|b| bot_view(b, rt.as_ref(), crypto.as_deref(), &sec_name, &mcp_name))
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "success": true, "bots": views })),
    )
        .into_response()
}

// ─── POST /api/bots（作成） ──────────────────────────────────────────────────

async fn create_bot(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(rt): Extension<Arc<dyn BotViewRuntime>>,
    Extension(ViewCrypto(crypto)): Extension<ViewCrypto>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let name = str_field(&body, "name");
    if name.is_empty() {
        return bad_request("Botの名前は必須です。");
    }
    // プリセット（未指定は secretary・未知は 400）。
    let preset_input = body
        .get("preset")
        .and_then(Value::as_str)
        .unwrap_or("secretary");
    let Some(preset) = BotPresetId::from_id(preset_input) else {
        return bad_request("不明なプリセットです。");
    };

    let bot_id = new_bot_id();
    if let Err(_e) = bot_repo::create_bot(&db, &bot_id, &uid, &name).await {
        return server_error();
    }
    if preset != BotPresetId::Secretary {
        if let Err(_e) = preset::apply_bot_preset(&db, &bot_id, preset).await {
            return server_error();
        }
        yuuka_auth::audit::add_audit_log(
            &db,
            &uid,
            "bot.capabilities_change",
            Some(&bot_id),
            Some(&format!("create:{}", preset.as_str())),
        )
        .await;
    }

    // preset 適用後の最新行でビューを組む。
    let created = match bot_repo::get_bot_detail(&db, &bot_id).await {
        Ok(Some(b)) => b,
        Ok(None) => return server_error(),
        Err(_) => return server_error(),
    };
    let (sec_name, mcp_name) = match preset_display_names(&db).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    let message = if preset == BotPresetId::McpAssistant {
        "Botを作成しました。汎用モードの利用にはBot専用のGemini APIキーの設定が必要です（Bot設定から設定してください）。"
    } else {
        "Botを作成しました。"
    };
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "bot": bot_view(&created, rt.as_ref(), crypto.as_deref(), &sec_name, &mcp_name),
            "message": message,
        })),
    )
        .into_response()
}

// ─── DELETE /api/bots ────────────────────────────────────────────────────────

async fn delete_bot(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(rt): Extension<Arc<dyn BotViewRuntime>>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot_id = str_field(&body, "botId");
    if bot_id.is_empty() {
        return bad_request("Bot IDが必要です。");
    }
    if bot_id.starts_with("bot_default_") || bot_id == "system_default" {
        return bad_request("デフォルトのBotは削除できません。");
    }
    let bot = match bot_repo::get_bot_detail(&db, &bot_id).await {
        Ok(b) => b,
        Err(_) => return server_error(),
    };
    // Node: `!bot || bot.user_id !== uid` は 403（不在も所有外もまとめて 403）。
    let is_owner = bot.is_some_and(|b| b.user_id == uid);
    if !is_owner {
        return status_json(StatusCode::FORBIDDEN, "Botの所有者のみが削除できます。");
    }
    // 稼働中の専用クライアントを停止（シーム・no-op 縮退）。
    rt.stop_custom(&bot_id);
    let ok = match bot_repo::delete_bot(&db, &bot_id).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    (
        StatusCode::OK,
        Json(json!({ "success": ok, "message": "Botを削除しました。" })),
    )
        .into_response()
}

// ─── POST /api/bots/profile ──────────────────────────────────────────────────

async fn update_profile(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot_id = str_field(&body, "botId");
    if bot_id.is_empty() {
        return bad_request("botId が必要です。");
    }
    if bot_id == "system_default" {
        return status_json(
            StatusCode::FORBIDDEN,
            "デフォルトBotのプロフィールは変更できません。",
        );
    }
    let bot = match bot_repo::get_bot_detail(&db, &bot_id).await {
        Ok(b) => b,
        Err(_) => return server_error(),
    };
    let is_owner = bot.is_some_and(|b| b.user_id == uid);
    if !is_owner {
        return status_json(StatusCode::FORBIDDEN, "Botの所有者のみが変更できます。");
    }
    let name = str_field(&body, "name");
    if name.is_empty() {
        return bad_request("Botの名前は必須です。");
    }
    let avatar = str_field(&body, "avatarUrl");
    let avatar_opt = if avatar.is_empty() {
        None
    } else {
        Some(avatar.as_str())
    };
    if let Err(_e) = bot_repo::update_bot_profile(&db, &bot_id, &name, avatar_opt).await {
        return server_error();
    }
    (
        StatusCode::OK,
        Json(json!({ "success": true, "message": "Botのプロフィールを更新しました。" })),
    )
        .into_response()
}

// ─── POST /api/bots/sync-discord（Discord プロフィール同期・Discord live 依存） ──

async fn sync_discord(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(live): Extension<Arc<dyn DiscordLive>>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot_id = str_field(&body, "botId");
    if bot_id.is_empty() {
        return bad_request("botId が必要です。");
    }
    // 共有可のアクセス権（オーナー限定ではない・Node `hasBotAccess`）。
    match has_bot_access(&db, &uid, &bot_id).await {
        Ok(true) => {}
        Ok(false) => return status_json(StatusCode::FORBIDDEN, "アクセス権限がありません。"),
        Err(_) => return server_error(),
    }
    // Bot の実在確認。
    match bot_repo::get_bot(&db, &bot_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return status_json(StatusCode::NOT_FOUND, "Bot が見つかりません。"),
        Err(_) => return server_error(),
    }
    // トークン設定有無（起動していないがトークンはある → 400 誘導）。
    let has_token = match bot_repo::bot_discord_token(&db, &bot_id).await {
        Ok(t) => t.is_some(),
        Err(_) => return server_error(),
    };

    // Node: customClients → (token あり&未起動=400) → defaultBotClient → 無ければ 503。
    let bot_user = if let Some(u) = live.custom_bot_user(&bot_id).await {
        Some(u)
    } else if has_token {
        return bad_request("Botクライアントが起動していません。先にBotを起動してください。");
    } else {
        live.default_bot_user().await
    };
    let Some(bot_user) = bot_user else {
        return status_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "Discordクライアントが準備できていません。サーバーを確認してください。",
        );
    };

    if let Err(_e) = bot_repo::update_discord_profile(
        &db,
        &bot_id,
        &bot_user.username,
        &bot_user.avatar_url,
        &bot_user.id,
    )
    .await
    {
        return server_error();
    }
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "discord_username": bot_user.username,
            "discord_avatar_url": bot_user.avatar_url,
            "discord_application_id": bot_user.id,
            "message": format!("Discordプロフィールを同期しました: {}", bot_user.username),
        })),
    )
        .into_response()
}

// ─── ビュー構築 ──────────────────────────────────────────────────────────────

/// Bot ビュー（Node `toBotView`＝`botViewSchema` ホワイトリスト）。機密列は出さない。
fn bot_view(
    bot: &BotDetail,
    rt: &dyn BotViewRuntime,
    crypto: Option<&SystemCrypto>,
    sec_name: &str,
    mcp_name: &str,
) -> Value {
    let caps = bot_repo::parse_capabilities(&bot.capabilities);
    let preset = preset::preset_id_for_capabilities(&caps);
    let (running, connected, shared) = bot_health(rt, bot, &caps);
    let display = match preset {
        BotPresetId::Secretary => sec_name,
        BotPresetId::McpAssistant => mcp_name,
    };
    json!({
        "id": bot.id,
        "user_id": bot.user_id,
        "name": bot.name,
        "recommended_persona_id": bot.recommended_persona_id,
        "persona_id": bot.persona_id,
        "capabilities": bot.capabilities,
        "discord_username": bot.discord_username,
        "discord_avatar_url": bot.discord_avatar_url,
        "discord_application_id": resolve_app_id(bot, crypto),
        "suspended": i64::from(bot.suspended),
        "created_at": bot.created_at,
        "updated_at": bot.updated_at,
        "preset": preset.as_str(),
        "preset_display_name": display,
        "has_gemini_key": bot.has_gemini_key,
        "has_token": bot.has_token,
        "running": running,
        "connected": connected,
        "shared": shared,
    })
}

/// 稼働状態を判定する（Node `botHealth`・running/connected/shared）。
fn bot_health(
    rt: &dyn BotViewRuntime,
    bot: &BotDetail,
    caps: &CapabilitySet,
) -> (bool, bool, bool) {
    if bot.suspended {
        return (false, false, false);
    }
    if bot.id == "system_default" {
        let up = rt.default_ready();
        return (up, up, false);
    }
    if bot.has_token {
        let running = rt.custom_running(&bot.id);
        let connected = running && rt.custom_ready(&bot.id);
        return (running, connected, false);
    }
    // トークン未設定の汎用モード（secretary 無し）は専用接続必須のため停止扱い。
    if !caps.has("secretary") {
        return (false, false, false);
    }
    // トークン未設定の秘書系はデフォルト接続にフォールバック（shared 表示）。
    let up = rt.default_ready();
    (up, up, true)
}

/// application/client ID を解決する（Node `resolveBotApplicationId`）。
/// 同期済み `discord_application_id` を優先し、無ければ保存トークンを復号して先頭 base64url
/// セグメント（=app id）を導く。復号失敗・crypto 未注入・不正形式は `None`（招待カードは未取得表示）。
fn resolve_app_id(bot: &BotDetail, crypto: Option<&SystemCrypto>) -> Option<String> {
    if let Some(id) = bot.discord_application_id.as_deref() {
        if !id.is_empty() {
            return Some(id.to_owned());
        }
    }
    let enc = bot.token_enc.as_ref()?;
    let token = crypto?
        .decrypt_text(&enc.encrypted, &enc.iv, &enc.tag)
        .ok()?;
    let first = token.split('.').next()?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(first)
        .ok()?;
    let s = std::str::from_utf8(&decoded).ok()?;
    // Discord snowflake（17〜20 桁の数値）のみ採用。
    if (17..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()) {
        Some(s.to_owned())
    } else {
        None
    }
}

/// 秘書 / 汎用モードの表示名を 1 度だけ引く（一覧の各 Bot で共有）。
async fn preset_display_names(db: &Db) -> Result<(String, String), yuuka_core::DbError> {
    let sec = preset::preset_display_name(db, BotPresetId::Secretary).await?;
    let mcp = preset::preset_display_name(db, BotPresetId::McpAssistant).await?;
    Ok((sec, mcp))
}

// ─── ヘルパ ──────────────────────────────────────────────────────────────────

/// `bot_<uuid v4>` を生成する（Node `bot_${crypto.randomUUID()}`）。
fn new_bot_id() -> String {
    let mut b = [0u8; 16];
    // CSPRNG 失敗は実質起こらない（起きたら固定バイトで継続＝衝突は UNIQUE 制約が拾う）。
    let _ = getrandom::getrandom(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10
    format!(
        "bot_{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

fn str_field(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned()
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

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use secrecy::SecretString;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, WebConfig};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct FakeAuth;

    #[async_trait::async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            let known = ["owner", "other"];
            Ok(known.contains(&token).then(|| SessionUser {
                discord_id: token.to_owned(),
                username: token.to_owned(),
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
            "yuuka_botroutes_test_{}_{seq}.sqlite",
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
            for uid in ["owner", "other"] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                     VALUES (?1, ?1, 'x', 'x')",
                    rusqlite::params![uid],
                )
                .expect("seed user");
            }
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
    async fn create_list_profile_delete_flow() {
        let app = app();

        // 名前必須。
        let (st, _) = send(&app, "POST", "/api/bots", "owner", "{}").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // 不明プリセット。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots",
            "owner",
            r#"{"name":"X","preset":"bogus"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // 汎用モードで作成。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots",
            "owner",
            r#"{"name":"MyBot","preset":"mcp_assistant"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let bot = &j["bot"];
        assert_eq!(bot["name"], "MyBot");
        assert_eq!(bot["preset"], "mcp_assistant");
        assert_eq!(bot["preset_display_name"], "汎用モード");
        assert_eq!(bot["suspended"], 0);
        assert_eq!(bot["running"], false);
        assert_eq!(bot["has_token"], false);
        assert!(bot.get("discord_token_encrypted").is_none()); // 機密は出さない。
        assert!(j["message"].as_str().unwrap().contains("汎用モード"));
        let bot_id = bot["id"].as_str().unwrap().to_owned();
        assert!(bot_id.starts_with("bot_"));

        // 一覧に出る（秘書 preset の bot も作って 2 件）。
        send(&app, "POST", "/api/bots", "owner", r#"{"name":"SecBot"}"#).await;
        let (st, j) = send(&app, "GET", "/api/bots", "owner", "").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["bots"].as_array().unwrap().len(), 2);

        // プロフィール更新。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/profile",
            "owner",
            &format!(r#"{{"botId":"{bot_id}","name":"Renamed"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        // 他人は変更不可。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/profile",
            "other",
            &format!(r#"{{"botId":"{bot_id}","name":"Hax"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // 削除ガード。
        let (st, _) = send(
            &app,
            "DELETE",
            "/api/bots",
            "owner",
            r#"{"botId":"system_default"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // 他人は削除不可。
        let (st, _) = send(
            &app,
            "DELETE",
            "/api/bots",
            "other",
            &format!(r#"{{"botId":"{bot_id}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // 所有者削除。
        let (st, j) = send(
            &app,
            "DELETE",
            "/api/bots",
            "owner",
            &format!(r#"{{"botId":"{bot_id}"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);
        // 一覧は 1 件に。
        let (_st, j) = send(&app, "GET", "/api/bots", "owner", "").await;
        assert_eq!(j["bots"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn app_id_prefers_synced_then_derives_from_token() {
        let crypto = SystemCrypto::new(SecretString::from("test-secret-xyz")).unwrap();
        // トークン先頭セグメント = base64url("123456789012345678")（18 桁 snowflake）。
        let app_id = "123456789012345678";
        let seg = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(app_id.as_bytes());
        let token = format!("{seg}.abc.def");
        let enc = crypto.encrypt_text(&token).unwrap();

        let mut bot = BotDetail {
            id: "b1".to_owned(),
            user_id: "owner".to_owned(),
            name: "B".to_owned(),
            recommended_persona_id: None,
            persona_id: None,
            capabilities: r#"["persona","memory","mcp","secretary"]"#.to_owned(),
            discord_username: None,
            discord_avatar_url: None,
            discord_application_id: None,
            suspended: false,
            created_at: String::new(),
            updated_at: String::new(),
            has_token: true,
            has_gemini_key: false,
            token_enc: Some(bot_repo::EncryptedTriplet {
                encrypted: enc.encrypted,
                iv: enc.iv,
                tag: enc.auth_tag,
            }),
        };
        // 同期済み値があればそれを優先。
        bot.discord_application_id = Some("999888777666555444".to_owned());
        assert_eq!(
            resolve_app_id(&bot, Some(&crypto)).as_deref(),
            Some("999888777666555444")
        );
        // 無ければトークンから導出。
        bot.discord_application_id = None;
        assert_eq!(resolve_app_id(&bot, Some(&crypto)).as_deref(), Some(app_id));
        // crypto 未注入なら None。
        assert_eq!(resolve_app_id(&bot, None), None);
    }
}
