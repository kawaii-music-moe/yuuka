//! yuuka-services — cron 常駐タスク群（現行 `src/services/*` の `start*`/`stop*`）。
//!
//! Phase 4。各サービスは [`CronService`] を実装し、[`build_services`] が全サービスをまとめて返す。
//! supervisor はこれを [`crate::run_cron`] 経由で `SupervisedService` へアダプトし、JoinSet 監督下で
//! 回す（panic 隔離＋指数バックオフ再起動・絶対制約2）。**本 crate は supervisor に依存しない**
//! （DAG: `services → web/core/domains`）。
//!
//! # 横断アクセス
//! cron は全ユーザーを跨いで走査する（`listDuePending` 等）。通常の `UserScope` 経路から隔離する
//! ため、各ドメイン crate の **`CronScan` メソッド**（[`CrossUserAccess`](yuuka_core::CrossUserAccess)
//! 証憑必須）を使う。証憑は [`ServiceContext`] が保持し、cron の起点をそこに限定する（grep 可能）。
//!
//! # 実装済み
//! - metrics / clipboard / reminder / todo-recurrence / payment-recurrence / birthday /
//!   **playbook-schedule**（マクロ定期実行・[`turn::PlaybookRunner`] ポート経由で会話エンジンを起動）/
//!   **briefing**（朝報の天気/RSS 定時配信・`yuuka-briefing` の生成プリミティブを再利用）/
//!   **report**（日報/週報の活動データ集約 + 定時配信・LLM 要約は未配線＝Node 自前の生データ
//!   フォールバックを実装）/ **backup**（ユーザー別 Google Drive バックアップの定期実行・実 Drive
//!   アップロードは [`backup::BackupRunner`] ポート経由＝main で `GoogleBackupClient` へ橋渡し）

use std::sync::Arc;

pub mod backup;
pub mod context;
pub mod cron_util;
pub mod metrics;
pub mod notifier;
pub mod schedule;
pub mod turn;

mod birthday;
mod briefing;
mod clipboard;
mod payment_recurrence;
mod planned_payment;
mod playbook_schedule;
mod reminder;
mod report;
mod todo_recurrence;

#[cfg(test)]
mod test_support;

pub use backup::{BackupRunner, NullBackupRunner};
pub use context::ServiceContext;
pub use metrics::MetricsRegistry;
pub use notifier::{Notification, Notifier, NotifyTarget, NullNotifier};
pub use schedule::{run_cron, CronService, Schedule};
pub use turn::{NullPlaybookRunner, PlaybookRunner};

/// 全 cron サービスを構築して返す（登録レジストリ）。supervisor はこれを監督下タスクへ変換する。
///
/// backup は実行ポート（[`backup::BackupRunner`]）を [`ServiceContext`] から得る。未配線時は
/// [`backup::NullBackupRunner`] へ縮退し、走査はするが実行は失敗（Node の Google 未連携と同じ非致命）。
#[must_use]
pub fn build_services() -> Vec<Arc<dyn CronService>> {
    vec![
        Arc::new(reminder::ReminderService),
        Arc::new(todo_recurrence::TodoRecurrenceService),
        Arc::new(payment_recurrence::PaymentRecurrenceService),
        Arc::new(birthday::BirthdayReminderService),
        Arc::new(clipboard::ClipboardCleanupService),
        Arc::new(metrics::MetricsLogService),
        Arc::new(playbook_schedule::PlaybookScheduleService),
        Arc::new(briefing::BriefingService),
        Arc::new(report::ReportService),
        Arc::new(backup::BackupService),
    ]
}
