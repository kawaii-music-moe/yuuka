//! yuuka-settings — ユーザー設定 API（`/api/settings/*`・全ルート auth:"user"）。Node
//! `settingsRoutes.ts` パリティ。
//!
//! 認可は yuuka-web の [`AuthenticatedUser`](yuuka_web::AuthenticatedUser) extractor で型強制。ルート固有の
//! 実行時依存（セッション再発行ストア・保存時暗号・Bot runtime シーム）は [`SettingsRuntime`] にまとめ
//! `Extension` レイヤで注入する（`AuthRuntime`/`AdminRuntime` と同方式・web→auth 循環回避）。
//!
//! **本増分でカバー**: profile（ユーザー名 + セッション再発行）・password（検証 + ポリシー + 全セッション/
//! デスクトップトークン失効 + 監査 + 再発行）・delete-account（検証 + 唯一 admin ガード + 所有 Bot 停止
//! シーム + 削除 + 失効 + 監査）・user（部分更新）・gemini（形式検証 + 暗号化）・backup（フォルダ ID 抽出 +
//! 設定保存）。
//!
//! **意図的に deferred（別増分・documented シーム）**: `GET /api/status`（多ドメイン集計 + Google
//! カレンダーキャッシュ）・`GET/POST /api/settings/discord`（Bot トークン設定 + 再起動フロー）・Google
//! OAuth 一式（`google/oauth/*`・`calendars`・`backup/trigger`）は Google Drive/Calendar HTTP サブシステム
//! に依存するため本crateには含めない。

use std::sync::Arc;

use axum::routing::{get, post};
use axum::{Extension, Router};
use yuuka_admin::BotRuntime;
use yuuka_auth::SessionStore;
use yuuka_crypto::SystemCrypto;
use yuuka_google::{BackupPort, CalendarPort, GoogleOAuthPort, OAuthStateStore};
use yuuka_web::AppState;

mod discord;
mod dto;
mod google;
mod repo;
mod routes;
mod status;

/// 設定ルートが使う実行時依存（`Extension` で各ハンドラへ注入）。
pub struct SettingsRuntime {
    /// セッション発行/一括失効に使う共有ストア（`CompositeAuth`/`AuthRuntime` と同一）。
    sessions: SessionStore,
    /// セッション TTL（秒・Cookie `Max-Age` と Redis EX に使う）。
    session_ttl_secs: u64,
    /// Gemini API キー / Discord トークンの暗号化に使う（未設定＝暗号鍵無しなら暗号化更新は 500 に縮退）。
    crypto: Option<Arc<SystemCrypto>>,
    /// 所有 Bot 停止/再起動の runtime シーム（Discord gateway 未配線時は `NullBotRuntime`）。
    bots: Arc<dyn BotRuntime>,
    /// Google OAuth2 認可フローのシーム（未配線時は `NullGoogleOAuth`＝未設定扱い）。
    oauth: Arc<dyn GoogleOAuthPort>,
    /// Google Calendar 取得/無効化のシーム（未配線時は `NullCalendar`＝空一覧）。
    calendar: Arc<dyn CalendarPort>,
    /// 手動バックアップのシーム（未配線時は `NullBackup`＝常に失敗＝500）。
    backup: Arc<dyn BackupPort>,
    /// OAuth の CSRF state nonce ストア（in-memory・web 再起動を跨ぐよう main で 1 度生成）。
    oauth_state: Arc<OAuthStateStore>,
}

impl SettingsRuntime {
    /// 実行時依存を束ねる。`session_ttl_days` から Cookie/Redis TTL（秒）を導出する。
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sessions: SessionStore,
        session_ttl_days: u32,
        crypto: Option<Arc<SystemCrypto>>,
        bots: Arc<dyn BotRuntime>,
        oauth: Arc<dyn GoogleOAuthPort>,
        calendar: Arc<dyn CalendarPort>,
        backup: Arc<dyn BackupPort>,
        oauth_state: Arc<OAuthStateStore>,
    ) -> Self {
        Self {
            sessions,
            session_ttl_secs: u64::from(session_ttl_days) * 24 * 60 * 60,
            crypto,
            bots,
            oauth,
            calendar,
            backup,
            oauth_state,
        }
    }
}

/// 設定ルータ（`AppState` 上でマージされる）。ルート固有依存を `Extension` で載せる。
pub fn routes(runtime: Arc<SettingsRuntime>) -> Router<AppState> {
    Router::new()
        .route("/api/settings/profile", post(routes::profile))
        .route("/api/settings/password", post(routes::password))
        .route("/api/settings/delete-account", post(routes::delete_account))
        .route("/api/settings/user", post(routes::user_settings))
        .route("/api/settings/gemini", post(routes::gemini))
        .route("/api/settings/backup", post(routes::backup))
        .route("/api/status", get(status::status))
        .route(
            "/api/settings/discord",
            get(discord::get_discord).post(discord::post_discord),
        )
        .route(
            "/api/settings/google/oauth/url",
            get(google::oauth_url),
        )
        .route(
            "/api/settings/google/oauth/callback",
            get(google::oauth_callback),
        )
        .route("/api/settings/calendars", post(google::calendars))
        .route(
            "/api/settings/backup/trigger",
            post(google::backup_trigger),
        )
        .layer(Extension(runtime))
}
