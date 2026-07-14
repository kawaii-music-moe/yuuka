//! yuuka-admin — 管理 API（`/api/admin/*`・全ルート auth:"admin"）。Node `adminRoutes.ts` パリティ。
//!
//! 認可は yuuka-web の [`AdminUser`](yuuka_web::AdminUser) extractor で型強制する。ルート固有の実行時
//! 依存（セッション一括失効ストア・保存時暗号・Bot runtime ポート・legal URL 既定）は [`AdminRuntime`]
//! にまとめ、`Extension` レイヤで各ハンドラへ注入する（`AppState`（yuuka-web 所有）に入れると
//! web→auth の循環になるため分離する。認証発行ルータ [`yuuka_auth::AuthRuntime`] と同じ方式）。
//!
//! **Bot runtime シーム**: `default-bot/token` の再起動・`bots/suspend` の停止・`GET /api/admin/bots`
//! の `isRunning` は稼働中 Discord クライアント（Node `customClients`/`restartDefaultBot`/
//! `stopCustomBot`）に触れる。Discord gateway は `YUUKA_RUST_DISCORD` ゲート既定 off のため、runtime
//! 効果は [`BotRuntime`] ポート越しに委譲し、既定は [`NullBotRuntime`]（no-op・`is_running=false`）に
//! 縮退する。**DB 効果（トークン暗号化保存・suspend フラグ・ユーザー削除）は常に完全に働く**。
//! gateway を配線したら実 `BotRuntime`（`DiscordManager` 背後）を注入すれば live 化する。

use std::sync::Arc;

use async_trait::async_trait;
use axum::routing::{delete, get, post};
use axum::{Extension, Router};
use yuuka_auth::SessionStore;
use yuuka_crypto::SystemCrypto;
use yuuka_web::AppState;

mod dto;
mod repo;
mod routes;

pub use dto::{AdminBotView, AdminStats, AdminUserView, AuditLogView, InviteCodeView};

/// 稼働中 Discord クライアントへの runtime 効果ポート（Node `customClients`/`restartDefaultBot`/
/// `stopCustomBot` 相当）。gateway 未配線時は [`NullBotRuntime`] へ縮退する。
#[async_trait]
pub trait BotRuntime: Send + Sync {
    /// Bot が現在稼働中か（Node `customClients.has(botId)`）。
    fn is_running(&self, bot_id: &str) -> bool;
    /// 稼働中の Bot クライアントを停止する（Node `stopCustomBot`）。冪等・未稼働は no-op。
    async fn stop(&self, bot_id: &str);
    /// システムデフォルト Bot を新トークンで再起動する（Node `restartDefaultBot`）。
    async fn restart_default(&self, token: &str);
}

/// Discord gateway 未配線時の縮退実装（no-op・常に非稼働扱い）。
pub struct NullBotRuntime;

#[async_trait]
impl BotRuntime for NullBotRuntime {
    fn is_running(&self, _bot_id: &str) -> bool {
        false
    }
    async fn stop(&self, _bot_id: &str) {}
    async fn restart_default(&self, _token: &str) {}
}

/// 管理ルートが使う実行時依存（`Extension` で各ハンドラへ注入）。
pub struct AdminRuntime {
    /// セッション一括失効に使う共有ストア（ロール変更・削除の即時反映）。
    sessions: SessionStore,
    /// デフォルト Bot トークンの暗号化に使う（未設定＝暗号鍵無しなら token 更新は 500 に縮退）。
    crypto: Option<Arc<SystemCrypto>>,
    /// 稼働中 Bot への runtime 効果（既定は [`NullBotRuntime`]）。
    bots: Arc<dyn BotRuntime>,
    /// `system_settings` 未設定時の privacy/terms URL 既定（config 由来・Node `config.privacyPolicyUrl`）。
    privacy_policy_url: String,
    terms_url: String,
}

impl AdminRuntime {
    /// 実行時依存を束ねる。
    #[must_use]
    pub fn new(
        sessions: SessionStore,
        crypto: Option<Arc<SystemCrypto>>,
        bots: Arc<dyn BotRuntime>,
        privacy_policy_url: String,
        terms_url: String,
    ) -> Self {
        Self {
            sessions,
            crypto,
            bots,
            privacy_policy_url,
            terms_url,
        }
    }
}

/// 管理ルータ（`AppState` 上でマージされる）。ルート固有依存を `Extension` で載せる。
pub fn routes(runtime: Arc<AdminRuntime>) -> Router<AppState> {
    Router::new()
        .route("/api/admin/default-bot/token", post(routes::default_bot_token))
        .route("/api/admin/stats", get(routes::stats))
        .route(
            "/api/admin/system-settings",
            get(routes::system_settings_get).post(routes::system_settings_set),
        )
        .route("/api/admin/users", get(routes::users))
        .route("/api/admin/users/role", post(routes::users_role))
        .route("/api/admin/users/delete", post(routes::users_delete))
        .route("/api/admin/audit-logs", get(routes::audit_logs))
        .route("/api/admin/bots", get(routes::bots))
        .route("/api/admin/bots/suspend", post(routes::bots_suspend))
        .route("/api/admin/bots/unsuspend", post(routes::bots_unsuspend))
        .route(
            "/api/admin/invite-codes",
            get(routes::invite_codes_list).post(routes::invite_codes_create),
        )
        .route(
            "/api/admin/invite-codes/{code}/revoke",
            post(routes::invite_codes_revoke),
        )
        .route(
            "/api/admin/invite-codes/{code}",
            delete(routes::invite_codes_delete),
        )
        .layer(Extension(runtime))
}
