//! yuuka-discord — twilight 0.17 マルチテナント bot（caller 駆動 Shard poll loop・注入ポート）。
//!
//! 現行 `src/bot.ts`（discord.js）を twilight へ置換する（§8.1）。本クレートは **Discord トランスポート
//! ＋テナント監督**に徹し、ターン処理（オーケストレーション）・Bot メタデータ・メンバー制・レート制限
//! といった外部依存は [`ports`] のトレイトとして**注入**する（gemini の `GenerateBackend`・web の
//! `AuthBackend` と同じ規律）。本番は実装を注入し、テストは fake でネットワーク無しに検証する。
//!
//! # 構成
//! - [`ports`]         — 注入トレイト＋provider 中立 DTO（`TurnProcessor`/`BotDirectory`/…）。
//! - [`DiscordManager`] — テナントのトークン解決・クライアント生成・runner/Messenger 組立。
//! - [`run_tenant`]    — 1 テナントの Shard poll loop（絶対制約2の核）。
//! - `message_flow`/`interaction`/`reply`/`presence`/`text`/`idempotent` — 内部実装。
//!
//! # DAG
//! `discord → core`（ids/error/secrets のみ）。db/gemini/services には依存せず、必要な機能は
//! ポート越しに受け取る（循環防止・テスト容易性）。supervisor は [`TenantRunner::run`] を
//! `SupervisedService` でラップして監督する（discord は supervisor に依存しない）。

pub mod ports;

mod idempotent;
mod interaction;
mod manager;
mod message_flow;
mod notify_bridge;
mod presence;
mod reply;
mod tenant;
mod text;

pub use idempotent::MessageDedup;
pub use manager::{DiscordManager, DiscordMessenger, ManagerPorts, Prepared, TenantRunner};
pub use presence::build_presence;
pub use tenant::{default_intents, run_tenant, TenantConfig, TenantStatus};
pub use text::{split_message, to_discord_markdown};

// よく使う契約を crate ルートへ再エクスポート（実装側の import を短くする）。
pub use ports::{
    rate_limit_message, ActionRow, BotDirectory, BotRecord, BotStatus, Button, ButtonStyle,
    DecisionOutcome, DeliverTarget, EmbedField, FileAttachment, IncomingChat, InlineMedia,
    MemberDecision, MemberDmSender, MembershipService, Notifier, PersonaRecord, RateDecision,
    RateExceeded, RateLimiter, RichEmbed, ShareRecord, Speaker, StatusSink, SubmitOutcome,
    TurnDelivery, TurnError, TurnProcessor, TurnReply,
};
