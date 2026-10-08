//! Google カレンダー → Yuuka の予定の定期取り込み（15 分ごと）。
//!
//! 取り込みの実体（Google 通信・予定の突き合わせ）は `services → google/schedule` の依存を増やさない
//! よう、services 所有のポート [`CalendarSyncRunner`] を supervisor で橋渡しする（[`crate::backup`] と
//! 同方式）。未配線時は [`NullCalendarSyncRunner`]（何もしない）。

use async_trait::async_trait;

use crate::context::ServiceContext;
use crate::schedule::{CronService, Schedule};

/// 取り込みの間隔（秒）。
const SYNC_INTERVAL_SECS: u64 = 15 * 60;

/// アカウントを明示指定したエージェント全部について、Google の予定を取り込むポート。
#[async_trait]
pub trait CalendarSyncRunner: Send + Sync {
    /// 1 周期分の取り込みを行う（個々の失敗は内部でログして続ける）。
    async fn sync_all(&self);
}

/// 未配線時の縮退（何もしない）。
pub struct NullCalendarSyncRunner;

#[async_trait]
impl CalendarSyncRunner for NullCalendarSyncRunner {
    async fn sync_all(&self) {}
}

/// Google カレンダーの定期取り込みサービス。
pub struct CalendarSyncService;

#[async_trait]
impl CronService for CalendarSyncService {
    fn name(&self) -> &'static str {
        "google_calendar_sync"
    }

    fn schedule(&self) -> Schedule {
        Schedule::FixedSecs(SYNC_INTERVAL_SECS)
    }

    async fn tick(&self, ctx: &ServiceContext) {
        ctx.calendar_sync.sync_all().await;
    }
}
