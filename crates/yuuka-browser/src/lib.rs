//! yuuka-browser — browser 系 Gemini ツール（Node `browserModule`/`browserService` パリティ）。
//!
//! 本増分は非対話の 3 本（`searchWeb`/`fetchDynamicPage`/`takePageScreenshot`）。chromium は
//! CLI（`--dump-dom`/`--screenshot`）でのみ起動し、CDP を要する対話セッション 6 本
//! （`browserInteractive*`）と `browserFillCredential`（credential 復号）は後続増分。
//!
//! 依存の向き: supervisor → yuuka-browser → yuuka-core（他ドメイン非依存）。ツール露出は既定
//! （secretary・Node browserModule `cap:"secretary"`）。

mod chromium;
mod interactive;
mod markdown;
mod search;
mod ssrf;
mod tools;

/// 対話ブラウザの per-user セッション管理。supervisor が 1 つ生成し、対話 6 ツールと
/// `browserFillCredential`（yuuka-credential）へ同じ Arc を注入する。
pub use interactive::BrowserManager;
pub use tools::tools;
