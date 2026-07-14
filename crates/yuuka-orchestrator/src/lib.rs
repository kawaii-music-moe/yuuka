//! yuuka-orchestrator — 会話オーケストレーション（秘書ターン処理・現行 `gemini.ts` 上位層／P1-2）。
//!
//! [`yuuka_gemini`] の function-calling ループ（純 API クライアント + ツール往復）の**上に**乗る層で、
//! Node `gemini.ts` の `processMessage`（systemInstruction 組立・会話ログ・ペルソナ・ユーザー鍵復号・
//! ツールレジストリ snapshot）を移植する。[`ChatEngine`] は `/ws/chat`（デスクトップ）と Discord の
//! [`yuuka_discord::TurnProcessor`] の**双方から共有**される会話の中核。
//!
//! DAG: `orchestrator → {gemini, tools, discord(DTO), crypto, web(Db), db, core}`。

pub mod bot_attr_routes;
pub mod bot_repo;
pub mod bot_routes;
pub mod discord_ports;
pub mod engine;
pub mod guild_prompt;
pub mod member_routes;
pub mod message_log;
pub mod persona;
pub mod preset;
pub mod share_routes;
pub mod system_prompt;
pub mod user;

pub use bot_attr_routes::routes as bot_attribute_routes;
pub use bot_routes::{
    routes as bot_management_routes, routes_with as bot_management_routes_with, BotViewRuntime,
    NullBotViewRuntime,
};
pub use discord_ports::{DbBotDirectory, DbMembership, InMemoryRateLimiter};
pub use engine::{ChatEngine, GeminiFactory, RealGeminiFactory};
pub use member_routes::{
    routes as member_request_routes, routes_with as member_request_routes_with, NullMemberDmSender,
};
pub use share_routes::{
    routes as bot_share_routes, routes_with as bot_share_routes_with, NullShareInviteDm,
};
