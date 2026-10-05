//! 支払い予定（`planned_payments`）の cron 専用アクセス（現行 `plannedPaymentRepo` の
//! `listOverdueRecurringAcrossUsers` / `advanceRecurring`）。
//!
//! 支払い予定はユーザー向け CRUD ドメイン crate が未整備（`planned_payments` は `user_id` のみで
//! `bot_id` を持たない）。cron 専用 SQL を本 crate が直接持つ（ドメイン整備時に repo へ移す）。
//! 横断アクセスは [`CrossUserAccess`] 証憑を要求する。

use rusqlite::types::Value;
use rusqlite::{params, Row};
use yuuka_core::{CrossUserAccess, DbError};
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// 期日超過の繰り返し支払い予定（cron 専用）。`amount` は INTEGER/REAL いずれでも透過に運ぶ。
#[derive(Debug, Clone)]
pub struct OverduePlan {
    pub id: i64,
    pub user_id: String,
    pub title: String,
    pub amount: Value,
    pub category: Option<String>,
    pub memo: Option<String>,
    pub due_date: String,
    pub repeat_rule: Option<String>,
}

/// 期日を過ぎた pending の繰り返し支払い予定を**全ユーザー横断**で返す
/// （現行 `listOverdueRecurringAcrossUsers`）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn list_overdue_recurring(
    db: &Db,
    _cron: CrossUserAccess,
) -> Result<Vec<OverduePlan>, DbError> {
    db.read
        .read(move |conn| {
            let sql = "SELECT id, user_id, title, amount, category, memo, due_date, repeat_rule \
                 FROM planned_payments \
                 WHERE status = 'pending' AND repeat_rule IS NOT NULL \
                 AND date(due_date) < date('now', 'localtime') \
                 ORDER BY due_date ASC, id ASC";
            let mut stmt = conn.prepare(sql).map_err(map_sqlite)?;
            let rows = stmt.query_map([], row_to_plan).map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// 元の pending 行を settled にし、次回期日の新しい pending 行を生成する（現行 `advanceRecurring`）。
/// 新規行を作れたら `true`（ToDo/リマインド紐付けは引き継がない）。
///
/// # Errors
/// トランザクション失敗時 [`DbError`]。
pub async fn advance_recurring(
    db: &Db,
    _cron: CrossUserAccess,
    plan: &OverduePlan,
    next_due: &str,
) -> Result<bool, DbError> {
    let id = plan.id;
    let user_id = plan.user_id.clone();
    let title = plan.title.clone();
    let amount = plan.amount.clone();
    let category = plan.category.clone();
    let memo = plan.memo.clone();
    let repeat_rule = plan.repeat_rule.clone();
    let next_due = next_due.to_owned();

    db.writer
        .transaction(move |tx| {
            // 元の行が pending のままなら処理済みにする（期日超過の自動送り）。
            tx.execute(
                "UPDATE planned_payments SET status = 'settled', \
                 updated_at = datetime('now', 'localtime') WHERE id = ?1 AND status = 'pending'",
                params![id],
            )
            .map_err(map_sqlite)?;

            // 次回分の pending 行を生成。
            let n = tx
                .execute(
                    "INSERT INTO planned_payments \
                     (user_id, title, amount, category, memo, due_date, repeat_rule) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        user_id,
                        title,
                        amount,
                        category,
                        memo,
                        next_due,
                        repeat_rule
                    ],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

fn row_to_plan(row: &Row) -> rusqlite::Result<OverduePlan> {
    Ok(OverduePlan {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        title: row.get("title")?,
        amount: row.get("amount")?,
        category: row.get("category")?,
        memo: row.get("memo")?,
        due_date: row.get("due_date")?,
        repeat_rule: row.get("repeat_rule")?,
    })
}
