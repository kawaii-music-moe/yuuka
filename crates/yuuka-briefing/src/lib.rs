//! yuuka-briefing — 朝ブリーフィング/日報/週報の配信設定ツール + 配信設定 Web-API（秘書経路）。
//! Node `briefingFunctions` / `deliveryRoutes` を移植する。
//!
//! 移植済み（tool）: configureReport（日報/週報の配信設定を部分更新）・getBriefingConfig（現在設定の
//! 読取）・configureBriefing（SSRF ガード + ニュースフィード配列の部分更新）。
//! 移植済み（Web-API・`deliveryRoutes`）: `GET/POST /api/briefing-config`・`POST /api/briefing/test`・
//! `GET/POST /api/report-configs`・`POST /api/report-configs/test`。`/test` の実配信は
//! [`DeliveryRunner`] シーム越しに委譲し、サービス本体（天気/RSS/Discord 送信）未配線のうちは
//! 既定 [`NullDeliveryRunner`]（常に未配信）へ縮退する。
//! **残（後続）**: runBriefingNow / DeliveryRunner の実装（briefing/report サービス本体）。
//!
//! 定時配信（cron→DM/channel）は `yuuka-services` の `BriefingService` が担う。本 crate は横断走査
//! （[`list_enabled_briefings`]）・生成（[`build_briefing`]）・テキスト整形（[`render_briefing_text`]）
//! の各プリミティブを提供し、通知ポート（`yuuka_services::Notifier`）への配線と cron due 判定は
//! services 側が持つ（本 crate は services に依存しない＝非循環）。

mod repo;
mod routes;
mod service;
mod tools;

pub use repo::{list_enabled_briefings, EnabledBriefing};
pub use routes::{routes, routes_with, DeliveryRunner, NullDeliveryRunner};
pub use service::{build_briefing, render_briefing_text, BriefingContent};
pub use tools::tools;
