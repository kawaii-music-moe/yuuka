//! yuuka-orchestrator — 会話オーケストレーション（秘書ターン処理・現行 `gemini.ts` 上位層／P1-2）。
//!
//! [`yuuka_gemini`] の function-calling ループ（純 API クライアント + ツール往復）の**上に**乗る層で、
//! Node `gemini.ts` の `processMessage`（systemInstruction 組立・会話ログ・ペルソナ・ユーザー鍵復号・
//! ツールレジストリ snapshot）を移植する。[`ChatEngine`] は `/ws/chat`（デスクトップ）と Discord の
//! [`yuuka_discord::TurnProcessor`] の**双方から共有**される会話の中核。
//!
//! DAG: `orchestrator → {gemini, tools, discord(DTO), crypto, web(Db), db, core}`。

pub mod engine;
pub mod message_log;
pub mod persona;
pub mod system_prompt;
pub mod user;

pub use engine::{ChatEngine, GeminiFactory, RealGeminiFactory};
