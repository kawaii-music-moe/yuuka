//! yuuka-client-api — PWA クライアント API（`/api/client/*`・issue #33: Node `clientRoutes.ts` の
//! Rust 移植）。
//!
//! `client/pwa`（Vue 3・`client/pwa/src/api/contracts.ts` が唯一の wire 契約）専用の薄いルート層。
//! DB アクセスは既存ドメインクレート（`yuuka-todo`/`yuuka-finance`/`yuuka-schedule`/`yuuka-persona`）
//! の `Repo` と `yuuka-orchestrator`（`message_log`・`context_note`・source 分離済み PWA コンテキスト・
//! `ChatEngine`）を再利用し、本クレート自身は PWA 契約特有の DTO 変換・JST 日境界・バリデーション・
//! チャット送信の事前チェック配線（issue #41）のみを持つ。
//!
//! チャット送信（`POST /api/client/chat/messages`）の設計は [`routes::routes_with`] のモジュール doc
//! および同関数直下の `chat_send` ドキュメントを参照。`202` 受理後の再起動でターンが失われた場合の
//! 回復（起動時スイープ・issue #77）は [`recovery`]。
//!
//! DAG: `client-api → web, orchestrator, discord(DTO/RateLimiter シーム), todo, finance, schedule, persona, gemini, core, db`。

pub mod date_util;
pub mod dto;
pub mod error;
pub mod recovery;
pub mod references;
pub mod routes;
pub mod users;

pub use recovery::recover_orphaned_chat_turns;
pub use routes::routes_with;
