//! cron スケジュールと監督下で回る長寿命ループ（`SupervisedService` へは supervisor 側の
//! アダプタが橋渡しする・DAG: `services → core` を保つため本 crate は supervisor に依存しない）。
//!
//! 各サービスは [`CronService`] を実装し、[`run_cron`] が「起動時即実行（取りこぼし復帰・§10）→
//! 次回発火まで sleep → tick」を協調停止（`cancel` future）付きで回す。tick は現行 node-cron の
//! コールバックと同じく **自前でエラーを吸収してログ**する（1 回の失敗でループを殺さない）。
//! 逐次 await ループなので Node の `ticking` 多重起動防止フラグは構造的に不要（前 tick 完了まで
//! 次の sleep に入らない）。

use std::future::Future;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Local;

use crate::context::ServiceContext;
use crate::cron_util;

/// cron スケジュール。cron 整列（分境界で発火）と固定間隔（metrics の setInterval）を区別する。
#[derive(Debug, Clone)]
pub enum Schedule {
    /// 毎分（`"* * * * *"`）。
    EveryMinute,
    /// 毎時 0 分（`"0 * * * *"`）。
    Hourly,
    /// 毎日 `hour:minute`（ローカル）。
    DailyAt { hour: u32, minute: u32 },
    /// 任意の 5-field cron 式（ユーザー由来ではなくサービス定義の固定式）。
    Cron(&'static str),
    /// 固定間隔（秒）。cron 非整列（現行 metrics の `setInterval`）。
    FixedSecs(u64),
}

impl Schedule {
    /// cron 整列スケジュールの式（`FixedSecs` は `None`）。
    fn cron_expr(&self) -> Option<String> {
        match self {
            Schedule::EveryMinute => Some("* * * * *".to_owned()),
            Schedule::Hourly => Some("0 * * * *".to_owned()),
            Schedule::DailyAt { hour, minute } => Some(format!("{minute} {hour} * * *")),
            Schedule::Cron(expr) => Some((*expr).to_owned()),
            Schedule::FixedSecs(_) => None,
        }
    }

    /// `now` から次回発火までの待機時間。
    ///
    /// cron 整列は次回ローカル発火時刻まで、`FixedSecs` は固定間隔。式が壊れている等で解けない
    /// 場合は 60 秒後に再評価する（サービス定義の式は健全な前提だが安全側で沈黙しない）。
    fn delay_from(&self, now: chrono::DateTime<Local>) -> Duration {
        match self {
            Schedule::FixedSecs(secs) => Duration::from_secs(*secs),
            _ => match self.cron_expr().and_then(|e| cron_util::next_after(&e, now)) {
                Some(next) => (next - now).to_std().unwrap_or(Duration::from_secs(1)),
                None => Duration::from_secs(60),
            },
        }
    }
}

/// 監督下で回る cron サービス（現行 `src/services/*` の各 `start*`/`stop*`）。
///
/// `tick` は 1 周期の処理で、**失敗は内部でログして握らず返す**（Node のコールバック同様、
/// tick の失敗はループを止めない＝自己復帰。panic した場合のみタスク境界で supervisor が再起動）。
#[async_trait]
pub trait CronService: Send + Sync {
    /// ログ・識別名（`cron:<name>` で監督ログに出る）。
    fn name(&self) -> &'static str;

    /// 発火スケジュール。
    fn schedule(&self) -> Schedule;

    /// 起動直後に一度即実行するか（計画外停止中の取りこぼし復帰・§10）。既定 true。
    fn run_on_start(&self) -> bool {
        true
    }

    /// 1 周期の処理。内部でエラーを吸収してログする。
    async fn tick(&self, ctx: &ServiceContext);
}

/// 1 サービスの長寿命ループ（協調停止 `cancel` 付き）。`cancel` 完了で即座に抜ける。
///
/// supervisor アダプタが `cancel = shutdown.cancelled()` を渡す。戻り値は無く、正常時は
/// `cancel` でのみ終了する（アダプタはこれを `Ok(())`＝意図的停止に写す）。
pub async fn run_cron<C>(svc: &C, ctx: &ServiceContext, cancel: impl Future<Output = ()>)
where
    C: CronService + ?Sized,
{
    tokio::pin!(cancel);

    if svc.run_on_start() {
        svc.tick(ctx).await;
    }

    let schedule = svc.schedule();
    loop {
        let delay = schedule.delay_from(Local::now());
        tokio::select! {
            () = &mut cancel => break,
            () = tokio::time::sleep(delay) => svc.tick(ctx).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_delay_is_within_a_day() {
        let now = Local::now();
        let d = Schedule::DailyAt { hour: 8, minute: 0 }.delay_from(now);
        assert!(d <= Duration::from_secs(24 * 60 * 60 + 1));
    }

    #[test]
    fn fixed_secs_delay_is_exact() {
        let now = Local::now();
        assert_eq!(Schedule::FixedSecs(300).delay_from(now), Duration::from_secs(300));
    }

    #[test]
    fn every_minute_delay_is_under_a_minute() {
        let now = Local::now();
        assert!(Schedule::EveryMinute.delay_from(now) <= Duration::from_secs(60));
    }

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::test_support::ctx_null;

    struct CountingService {
        ticks: Arc<AtomicUsize>,
        on_start: bool,
    }

    #[async_trait]
    impl CronService for CountingService {
        fn name(&self) -> &'static str {
            "counting"
        }
        fn schedule(&self) -> Schedule {
            // 1 分後まで sleep（cancel が先に発火する）。
            Schedule::EveryMinute
        }
        fn run_on_start(&self) -> bool {
            self.on_start
        }
        async fn tick(&self, _ctx: &ServiceContext) {
            self.ticks.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn run_cron_ticks_on_start_then_exits_on_cancel() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let svc = CountingService { ticks: ticks.clone(), on_start: true };
        let ctx = ctx_null(crate::test_support::seeded_db("CREATE TABLE t(x)").0);
        // 即座に ready な cancel。on-start tick 後、次の sleep へ入る前に cancel で抜ける。
        run_cron(&svc, &ctx, std::future::ready(())).await;
        assert_eq!(ticks.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_cron_without_on_start_does_not_tick_before_cancel() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let svc = CountingService { ticks: ticks.clone(), on_start: false };
        let ctx = ctx_null(crate::test_support::seeded_db("CREATE TABLE t(x)").0);
        run_cron(&svc, &ctx, std::future::ready(())).await;
        assert_eq!(ticks.load(Ordering::SeqCst), 0);
    }
}
