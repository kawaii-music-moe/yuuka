//! yuuka-briefing — 定期報告（秘書経路）。
//!
//! 旧来の朝報（天気・ニュース）と日報・週報（タスク・予定・支払い・家計・会話の振り返り）を、名前を
//! 付けて複数登録できる「定期報告」にまとめたもの。1 件ごとに配信スケジュール・配信先・載せる内容
//! （[`ReportSection`]）を選ぶ。タスク・予定・支払い予定・家計を含む報告は本人への DM にだけ送る。
//!
//! - 設定: Web-API（[`routes_with`]・`/api/periodic-reports*`）と LLM ツール（[`tools_with_aux_text`]）。
//!   どちらも同じ検証（[`ReportSettings::validate`]）を通して保存する。
//! - 生成: [`build_report_embed`]（天気は Open-Meteo・ニュースは登録 RSS・活動データは DB 集計を
//!   補助 LLM で要約し、使えなければ生データ）。
//! - 配信: 定時配信（cron→DM/channel）は `yuuka-services` の定期報告サービスが [`list_enabled_reports`]
//!   を走査して行う。テスト配信は [`DeliveryRunner`] シーム越し（実装は `yuuka-services` の
//!   `NotifierDeliveryRunner`＝定時配信と同じ生成・送信経路。本 crate は services に依存しない＝非循環）。

mod activity;
mod compose;
mod manage;
mod model;
mod repo;
mod routes;
mod service;
mod tools;

pub use compose::{build_report_embed, REPORT_COLOR};
pub use model::{PeriodicReport, ReportSection, ReportSettings, TargetType};
pub use repo::{find_report, list_enabled_reports, list_reports};
pub use routes::{routes, routes_with, DeliveryRunner, LateDeliveryRunner, NullDeliveryRunner};
pub use tools::{tools, tools_with, tools_with_aux_text};
