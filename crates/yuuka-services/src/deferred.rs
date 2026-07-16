//! 予約シーム（本体 deferred）: report / briefing / backup。
//!
//! これらはスケジュール・監督配線・登録は本 Phase で確定させるが、tick 本体は**未整備の基盤に
//! 依存する**ため保留する（discord の凍結シームと同方針＝配線を実証し、後で本体を埋めるだけにする）。
//! 各サービスの tick は保留マーカーを `debug` で出す（既定 OFF・毎分でもスパムしない）。完了時は
//! この tick を実装へ差し替えるだけでよい。
//!
//! （playbook-schedule は本体実装済み＝[`crate::playbook_schedule`]。）
//!
//! 保留理由（各サービスが必要とする未整備基盤）:
//! - **report**（現行 `reportService.ts`）: `report_configs`/`message_logs` repo、Gemini 補助生成
//!   （`generateAuxText` = gemini オーケストレーション上位層）、グラフ埋め込み、Notifier。
//! - **backup**（現行 `backupService.ts`）: ユーザー別 Google Drive OAuth クライアント、SQLite
//!   バックアップ＋zip、`users` の Drive 連携メタ。
//!
//! （**briefing** は本体実装済み＝[`crate::briefing::BriefingService`]。天気/RSS 生成 +
//! テキスト整形は `yuuka-briefing` の各プリミティブを再利用し、cron due 判定 + Notifier 配線を持つ。）

use async_trait::async_trait;

use crate::context::ServiceContext;
use crate::schedule::{CronService, Schedule};

/// 保留サービスを定義する（名前・スケジュール・保留理由メッセージ）。
macro_rules! deferred_service {
    ($ty:ident, $name:literal, $sched:expr, $reason:literal) => {
        #[doc = concat!("予約シーム: ", $name, "（本体 deferred・", $reason, "）。")]
        pub struct $ty;

        #[async_trait]
        impl CronService for $ty {
            fn name(&self) -> &'static str {
                $name
            }
            fn schedule(&self) -> Schedule {
                $sched
            }
            fn run_on_start(&self) -> bool {
                false
            }
            async fn tick(&self, _ctx: &ServiceContext) {
                tracing::debug!(
                    service = $name,
                    reason = $reason,
                    "予約シーム: 本体未実装（依存基盤待ち）"
                );
            }
        }
    };
}

deferred_service!(
    ReportService,
    "report",
    Schedule::EveryMinute,
    "report_configs/message_logs + Gemini 補助生成"
);
deferred_service!(
    BackupService,
    "backup",
    Schedule::Cron("15 * * * *"),
    "ユーザー別 Google Drive クライアント"
);
// playbook-schedule / briefing は本体実装済み
// （[`crate::playbook_schedule::PlaybookScheduleService`] / [`crate::briefing::BriefingService`]）。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_services_have_stable_names_and_schedules() {
        assert_eq!(ReportService.name(), "report");
        assert!(matches!(
            BackupService.schedule(),
            Schedule::Cron("15 * * * *")
        ));
        assert!(!ReportService.run_on_start());
    }
}
