//! ルーチン（繰り返し）タスク自動生成（§3.2 v16・現行 `todoRecurrenceService.ts`）。
//!
//! 毎日 0:10＋起動直後に、期日を過ぎた repeat_rule 付きルーチンを走査し、cron 式で次回期日を
//! 計算して同一行を進める（`advance_routine`＝支払い予定と異なり行を増やさない・山積み防止）。
//! 終了条件（`repeat_count<=1` / 次回が `repeat_until` 超過）を満たしたら `end_routine` で単発化する。

use async_trait::async_trait;
use chrono::Local;
use yuuka_todo::repo::TodoRepo;

use crate::context::ServiceContext;
use crate::cron_util;
use crate::schedule::{CronService, Schedule};

/// ルーチンタスク自動生成サービス。
pub struct TodoRecurrenceService;

#[async_trait]
impl CronService for TodoRecurrenceService {
    fn name(&self) -> &'static str {
        "todo-recurrence"
    }

    fn schedule(&self) -> Schedule {
        // 毎日 0:10（支払い予定 0:05 と時刻をずらして負荷集中を避ける）。
        Schedule::DailyAt {
            hour: 0,
            minute: 10,
        }
    }

    async fn tick(&self, ctx: &ServiceContext) {
        if let Err(e) = run(ctx).await {
            tracing::error!(error = %e, "❌ ルーチンタスクサービスのティック処理でエラー");
        }
    }
}

async fn run(ctx: &ServiceContext) -> Result<(), yuuka_core::DbError> {
    let repo = TodoRepo::new(&ctx.db);
    let overdue = repo.list_overdue_routines(ctx.cross).await?;
    for todo in overdue {
        // repo 側フィルタの保険。
        let (Some(rule), Some(due)) = (todo.repeat_rule.as_deref(), todo.due_date.as_deref())
        else {
            continue;
        };

        // 終了条件2-b: 残り回数が 1 以下なら今回が最終回 → 終了。
        if matches!(todo.repeat_count, Some(c) if c <= 1) {
            end(ctx, &repo, todo.id, &todo.title, &todo.user_id, "回数消化").await;
            continue;
        }

        let Some(next_due) = cron_util::next_recurring_due_date(rule, due, Local::now()) else {
            // cron 式が壊れている場合は行を残して修正を待つ。
            tracing::error!(id = todo.id, rule, user = %todo.user_id, "❌ repeat_rule 解釈失敗のためスキップ");
            continue;
        };

        // 終了条件2-a: 次回期日が終了日を越えるなら終了（現在の1件を最終とする）。
        if matches!(todo.repeat_until.as_deref(), Some(until) if next_due.as_str() > until) {
            end(
                ctx,
                &repo,
                todo.id,
                &todo.title,
                &todo.user_id,
                "終了日到達",
            )
            .await;
            continue;
        }

        let next_count = todo.repeat_count.map(|c| c - 1);
        match repo
            .advance_routine(ctx.cross, todo.id, next_due.clone(), next_count)
            .await
        {
            Ok(true) => tracing::info!(
                id = todo.id, next_due, user = %todo.user_id,
                "🔁 ルーチンタスクを次回へ更新"
            ),
            Ok(false) => {}
            Err(e) => tracing::error!(id = todo.id, error = %e, "❌ ルーチンタスクの更新に失敗"),
        }
    }
    Ok(())
}

/// ルーチンを終了して単発へ戻す（ログ付き）。
async fn end(
    ctx: &ServiceContext,
    repo: &TodoRepo<'_>,
    id: i64,
    title: &str,
    user: &str,
    reason: &str,
) {
    match repo.end_routine(ctx.cross, id).await {
        Ok(()) => tracing::info!(id, title, user, reason, "🏁 ルーチン終了"),
        Err(e) => tracing::error!(id, error = %e, "❌ ルーチン終了処理に失敗"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ctx_null, seeded_db};
    use yuuka_db::map_sqlite;

    const TODOS_DDL: &str = "CREATE TABLE todos (
        id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default', title TEXT NOT NULL, description TEXT,
        due_date TEXT, start_date TEXT, priority TEXT, tags TEXT NOT NULL DEFAULT '[]',
        status TEXT NOT NULL DEFAULT 'open', progress INTEGER NOT NULL DEFAULT 0, parent_id INTEGER,
        linked_payment_id INTEGER, due_reminded INTEGER NOT NULL DEFAULT 0, repeat_rule TEXT,
        repeat_until TEXT, repeat_count INTEGER, created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
    );";

    async fn insert(ctx: &ServiceContext, sql: &str) {
        let sql = sql.to_owned();
        ctx.db
            .writer
            .execute(move |conn| {
                conn.execute_batch(&sql).map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    async fn row(ctx: &ServiceContext) -> (String, Option<String>, Option<i64>, Option<String>) {
        ctx.db
            .read
            .read(|conn| {
                conn.query_row(
                    "SELECT status, due_date, repeat_count, repeat_rule FROM todos WHERE id=1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .map_err(map_sqlite)
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn overdue_routine_advances_to_next_due() {
        let (db, _dir) = seeded_db(TODOS_DDL);
        let ctx = ctx_null(db);
        // 数日前が期日の日次ルーチン（回数無制限）。cron 収束が 1000 回上限内に収まる現実的入力。
        insert(
            &ctx,
            "INSERT INTO todos (user_id,title,due_date,status,progress,due_reminded,repeat_rule) \
             VALUES ('u','routine', date('now','localtime','-3 days'),'done',100,1,'0 0 * * *')",
        )
        .await;

        run(&ctx).await.unwrap();

        let (status, due, _count, rule) = row(&ctx).await;
        // 進捗/状態/リマインドがリセットされ、期日が今日以降へ進む。
        assert_eq!(status, "open");
        let today: String = ctx
            .db
            .read
            .read(|conn| {
                conn.query_row("SELECT date('now','localtime')", [], |r| r.get(0))
                    .map_err(map_sqlite)
            })
            .await
            .unwrap();
        assert!(
            due.as_deref().is_some_and(|d| d >= today.as_str()),
            "due {due:?} >= {today}"
        );
        assert_eq!(rule.as_deref(), Some("0 0 * * *")); // まだ繰り返し中。
    }

    #[tokio::test]
    async fn last_count_ends_routine() {
        let (db, _dir) = seeded_db(TODOS_DDL);
        let ctx = ctx_null(db);
        insert(
            &ctx,
            "INSERT INTO todos (user_id,title,due_date,repeat_rule,repeat_count) \
             VALUES ('u','last','2000-01-01','0 0 * * *',1)",
        )
        .await;

        run(&ctx).await.unwrap();

        let (_status, _due, count, rule) = row(&ctx).await;
        assert_eq!(rule, None); // 単発化。
        assert_eq!(count, None);
    }

    #[tokio::test]
    async fn next_beyond_until_ends_routine() {
        let (db, _dir) = seeded_db(TODOS_DDL);
        let ctx = ctx_null(db);
        // 期日は数日前、終了日は昨日 → 次回（今日以降）は until を越える → 終了。
        insert(&ctx, "INSERT INTO todos (user_id,title,due_date,repeat_rule,repeat_until) \
             VALUES ('u','ending', date('now','localtime','-3 days'),'0 0 * * *', date('now','localtime','-1 day'))").await;

        run(&ctx).await.unwrap();

        let (_status, _due, _count, rule) = row(&ctx).await;
        assert_eq!(rule, None);
    }
}
