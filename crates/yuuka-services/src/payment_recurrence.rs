//! 支払い予定 繰り返し自動生成（§3.4.3・現行 `paymentRecurrenceService.ts`）。
//!
//! 毎日 0:05＋起動直後に、期日を過ぎた pending の繰り返し支払い予定を走査し、cron 式で次回期日を
//! 計算して次回分の pending 行を生成する（`advance_recurring`＝ルーチンタスクと異なり行を増やす）。
//! 期日前通知は本サービスでは行わない（リマインド連携はユーザーが明示設定する方式）。

use async_trait::async_trait;
use chrono::Local;

use crate::context::ServiceContext;
use crate::cron_util;
use crate::planned_payment;
use crate::schedule::{CronService, Schedule};

/// 支払い予定 繰り返し自動生成サービス。
pub struct PaymentRecurrenceService;

#[async_trait]
impl CronService for PaymentRecurrenceService {
    fn name(&self) -> &'static str {
        "payment-recurrence"
    }

    fn schedule(&self) -> Schedule {
        Schedule::DailyAt { hour: 0, minute: 5 }
    }

    async fn tick(&self, ctx: &ServiceContext) {
        if let Err(e) = run(ctx).await {
            tracing::error!(error = %e, "❌ 支払い予定繰り返しサービスのティック処理でエラー");
        }
    }
}

async fn run(ctx: &ServiceContext) -> Result<(), yuuka_core::DbError> {
    let overdue = planned_payment::list_overdue_recurring(&ctx.db, ctx.cross).await?;
    for plan in overdue {
        let Some(rule) = plan.repeat_rule.as_deref() else {
            continue; // repo フィルタの保険。
        };
        let Some(next_due) = cron_util::next_recurring_due_date(rule, &plan.due_date, Local::now())
        else {
            // cron 式が壊れている場合は pending のまま残し修正を待つ。
            tracing::error!(id = plan.id, rule, user = %plan.user_id, "❌ repeat_rule 解釈失敗のためスキップ");
            continue;
        };

        match planned_payment::advance_recurring(&ctx.db, ctx.cross, &plan, &next_due).await {
            Ok(true) => tracing::info!(
                id = plan.id, next_due, user = %plan.user_id,
                "🔁 繰り返し支払い予定を次回へ更新"
            ),
            Ok(false) => {}
            Err(e) => tracing::error!(id = plan.id, error = %e, "❌ 繰り返し支払い予定の処理に失敗"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ctx_null, seeded_db};
    use yuuka_db::map_sqlite;

    const PLANNED_DDL: &str = "CREATE TABLE planned_payments (
        id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, title TEXT NOT NULL,
        amount INTEGER NOT NULL, category TEXT, memo TEXT, due_date TEXT NOT NULL, repeat_rule TEXT,
        status TEXT NOT NULL DEFAULT 'pending',
        created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
    );";

    #[tokio::test]
    async fn overdue_plan_settled_and_next_created() {
        let (db, _dir) = seeded_db(PLANNED_DDL);
        let ctx = ctx_null(db);
        ctx.db
            .writer
            .execute(|conn| {
                conn.execute(
                    "INSERT INTO planned_payments (user_id,title,amount,category,due_date,repeat_rule,status) \
                     VALUES ('u','家賃',80000,'housing','2000-01-01','0 0 1 * *','pending')",
                    [],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();

        run(&ctx).await.unwrap();

        // 元行は settled、次回分 pending が生成され合計 2 行。
        let (total, settled, pending): (i64, i64, i64) = ctx
            .db
            .read
            .read(|conn| {
                conn.query_row(
                    "SELECT COUNT(*), \
                     SUM(status='settled'), SUM(status='pending') FROM planned_payments",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(settled, 1);
        assert_eq!(pending, 1);

        // 生成された次回行は amount/category を引き継ぐ。
        let (amount, next_due): (i64, String) = ctx
            .db
            .read
            .read(|conn| {
                conn.query_row(
                    "SELECT amount, due_date FROM planned_payments WHERE status='pending'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(amount, 80000);
        assert!(next_due.ends_with("-01"), "next_due should be a month start: {next_due}");
    }
}
