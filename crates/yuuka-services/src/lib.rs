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
//! # 実装済み / 予約シーム
//! - 実装: metrics / clipboard / reminder / todo-recurrence / payment-recurrence / birthday /
//!   **playbook-schedule**（マクロ定期実行・[`turn::PlaybookRunner`] ポート経由で会話エンジンを起動）/
//!   **briefing**（朝報の天気/RSS 定時配信・`yuuka-briefing` の生成プリミティブを再利用）
//! - 予約シーム（本体 deferred）: report / backup
//!   — gemini 補助生成（`generateAuxText`）・Google Drive 等の未整備基盤に依存。

use std::sync::Arc;

pub mod context;
pub mod cron_util;
pub mod metrics;
pub mod notifier;
pub mod schedule;
pub mod turn;

mod birthday;
mod briefing;
mod clipboard;
mod deferred;
mod payment_recurrence;
mod planned_payment;
mod playbook_schedule;
mod reminder;
mod todo_recurrence;

#[cfg(test)]
mod test_support;

pub use context::ServiceContext;
pub use metrics::MetricsRegistry;
pub use notifier::{Notification, Notifier, NotifyTarget, NullNotifier};
pub use schedule::{run_cron, CronService, Schedule};
pub use turn::{NullPlaybookRunner, PlaybookRunner};

/// 全 cron サービスを構築して返す（登録レジストリ）。supervisor はこれを監督下タスクへ変換する。
///
/// 予約シーム（report/backup）は本体 deferred だが**スケジュール上は起動**し、tick で
/// 「未実装（依存基盤待ち）」を warn する（配線を実証し、後で本体を埋めるだけにする）。
#[must_use]
pub fn build_services() -> Vec<Arc<dyn CronService>> {
    vec![
        // ── 実装済み ──
        Arc::new(reminder::ReminderService),
        Arc::new(todo_recurrence::TodoRecurrenceService),
        Arc::new(payment_recurrence::PaymentRecurrenceService),
        Arc::new(birthday::BirthdayReminderService),
        Arc::new(clipboard::ClipboardCleanupService),
        Arc::new(metrics::MetricsLogService),
        Arc::new(playbook_schedule::PlaybookScheduleService),
        Arc::new(briefing::BriefingService),
        // ── 予約シーム（本体 deferred・依存基盤待ち） ──
        Arc::new(deferred::ReportService),
        Arc::new(deferred::BackupService),
    ]
}
