//! yuuka-client-api — PWA クライアント API（`/api/client/*`・issue #33: Node `clientRoutes.ts` の
//! Rust 移植）。
//!
//! `client/pwa`（Vue 3・`client/pwa/src/api/contracts.ts` が唯一の wire 契約）専用の薄いルート層。
//! DB アクセスは既存ドメインクレート（`yuuka-todo`/`yuuka-finance`/`yuuka-schedule`/`yuuka-persona`）
//! の `Repo` と `yuuka-orchestrator`（`message_log`・`context_note`・source 分離済み PWA コンテキスト）
//! を再利用し、本クレート自身は PWA 契約特有の DTO 変換・JST 日境界・バリデーションのみを持つ。
//!
//! **今回未移植**: `POST /api/client/chat/messages`（チャット送信）。詳細は [`routes`] のモジュール doc。
//!
//! DAG: `client-api → web, orchestrator, todo, finance, schedule, persona, gemini, core, db`。

pub mod date_util;
pub mod dto;
pub mod error;
pub mod references;
pub mod routes;
pub mod users;

pub use routes::routes;
