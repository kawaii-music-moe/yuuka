//! yuuka-botassistant — 汎用モード（guild-assistant）ツール。Node `botAssistantFunctions` の
//! ノート系（個人/共有メモ帳）を移植する。
//!
//! **露出**: 秘書経路では出さず、汎用モード（`TurnMode::GuildAssistant`）+ 能力 `memory` でのみ
//! 露出する（[`ToolExposure`](yuuka_core::tool::ToolExposure) を tool ごとに上書き）。assembly 層
//! （`build_tool_registry`）が `tools(db)` を集約し、`process_guild` の snapshot が経路×能力で選別する。
//!
//! 移植済み: getMyNote/setMyNote/appendMyNote（個人ノート）・getGuildNote/setGuildNote/
//! appendGuildNote（共有ノート）。**残（後続）**: メンバー管理（addBotMember/listBotMembers/
//! removeBotMember）・会話要約（summarizeConversationTopic＝LLM 側）。

mod repo;
mod tools;

pub use tools::tools;
