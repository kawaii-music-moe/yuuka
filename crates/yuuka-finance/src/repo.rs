//! `ExpenseRepo` — `UserScope` 束縛のデータアクセス（金銭データ・分離キーを型で強制）。
//!
//! 全クエリは `WHERE user_id = ? AND bot_id = ?` を必須にし、`&UserScope` を取ることで
//! 「user_id/bot_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。金銭データのためスコープを厳守する。

use rusqlite::{params, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{
    BudgetLimit, CategoryTotal, Expense, MonthlyTrendPoint, NewExpense, NewPlannedPayment,
    PlannedPayment,
};

/// 返却列（クリーンビュー・内部列 user_id/bot_id は含めない）。
const EXPENSE_COLUMNS: &str =
    "id, type, amount, category, memo, date, time, source, created_at";

/// 支払い予定の返却列（クリーンビュー・内部列 user_id/bot_id は含めない）。
const PLAN_COLUMNS: &str = "id, title, amount, category, memo, due_date, repeat_rule, status, \
     settled_expense_id, linked_todo_id, linked_reminder_id, created_at, updated_at";

/// 一覧のデフォルト取得件数（Node financeRoutes: listRecentExpenses(.., 30)）。
const DEFAULT_LIMIT: i64 = 30;

/// finance リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct ExpenseRepo<'a> {
    read: &'a ReadPool,
    writer: &'a WriterHandle,
}

impl ScopedRepo for ExpenseRepo<'_> {}

impl<'a> ExpenseRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// スコープ内の直近収支を日付降順で返す（既定 30 件）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(&self, scope: &UserScope) -> Result<Vec<Expense>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {EXPENSE_COLUMNS} FROM expenses \
                     WHERE user_id = ?1 AND bot_id = ?2 \
                     ORDER BY date DESC, created_at DESC, id DESC LIMIT ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid, DEFAULT_LIMIT], row_to_expense)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一収支を取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, id: i64) -> Result<Option<Expense>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {EXPENSE_COLUMNS} FROM expenses \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, uid, bid], row_to_expense)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// 収支を記録し、作成後の行を返す（source は常に `manual`）。
    ///
    /// date/time 未指定時は SQLite の localtime を採用（Node 既定と一致）。
    /// type は `income` 以外を `expense` に正規化する（Node financeRoutes parity）。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add(&self, scope: &UserScope, input: NewExpense) -> Result<Expense, DbError> {
        let (uid, bid) = scope_keys(scope);
        let id = self
            .writer
            .transaction(move |tx| {
                let etype = if input.r#type.as_deref() == Some("income") {
                    "income"
                } else {
                    "expense"
                };
                // date/time は未指定なら SQLite localtime を採用（Node の getFullYear 等と一致）。
                tx.execute(
                    "INSERT INTO expenses \
                       (user_id, bot_id, type, amount, category, memo, date, time, source, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, \
                             COALESCE(?7, date('now', 'localtime')), \
                             COALESCE(?8, time('now', 'localtime')), \
                             'manual', datetime('now', 'localtime'))",
                    params![
                        uid,
                        bid,
                        etype,
                        input.amount,
                        input.category,
                        input.description,
                        input.date,
                        input.time,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted expense not found".to_owned()))
    }

    /// 収支を削除する（削除できたら `true`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM expenses WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
                        params![id, uid, bid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }

    // ── 月次集計（§3.4.1・Node expenseRepo getMonthly*） ─────────────────────

    /// 現在のローカル暦の `(year, month)`（1-12）を返す（集計の既定月に使う）。
    ///
    /// Node は `new Date().getFullYear()/getMonth()+1`（サーバローカル時刻）で当月を決める。
    /// Rust も SQLite の `'now','localtime'` で同じローカル暦日境界に合わせる。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn current_year_month(&self) -> Result<(i64, i64), DbError> {
        self.read
            .read(move |conn| {
                conn.query_row(
                    "SELECT CAST(strftime('%Y', 'now', 'localtime') AS INTEGER), \
                            CAST(strftime('%m', 'now', 'localtime') AS INTEGER)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(map_sqlite)
            })
            .await
    }

    /// 指定月・指定 type の合計金額を返す（`COALESCE(SUM(amount),0)`）。
    ///
    /// Node `getMonthlyTotal`（`date LIKE 'YYYY-MM%'`）パリティ。`etype` は `"expense"`/`"income"`。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn monthly_total(
        &self,
        scope: &UserScope,
        etype: &str,
        year: i64,
        month: i64,
    ) -> Result<i64, DbError> {
        let (uid, bid) = scope_keys(scope);
        let etype = etype.to_owned();
        let like = format!("{}-{:02}%", year, month);
        self.read
            .read(move |conn| {
                conn.query_row(
                    "SELECT COALESCE(SUM(amount), 0) FROM expenses \
                     WHERE user_id = ?1 AND bot_id = ?2 AND type = ?3 AND date LIKE ?4",
                    params![uid, bid, etype, like],
                    |row| row.get(0),
                )
                .map_err(map_sqlite)
            })
            .await
    }

    /// 指定月の支出カテゴリ別集計を金額の大きい順に返す（Node `getMonthlyCategoryBreakdown`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn monthly_category_breakdown(
        &self,
        scope: &UserScope,
        year: i64,
        month: i64,
        etype: &str,
    ) -> Result<Vec<CategoryTotal>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let like = format!("{}-{:02}%", year, month);
        let etype = etype.to_owned();
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT category, SUM(amount) AS total, COUNT(*) AS count \
                         FROM expenses \
                         WHERE user_id = ?1 AND bot_id = ?2 AND type = ?3 AND date LIKE ?4 \
                         GROUP BY category ORDER BY total DESC",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid, etype, like], |row| {
                        Ok(CategoryTotal {
                            category: row.get("category")?,
                            total: row.get("total")?,
                            count: row.get("count")?,
                        })
                    })
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// 当月を含む過去 `months` ヶ月の収支推移を古い順に返す（Node `getMonthlyTrend`）。
    ///
    /// 記録の無い月も `income/expense = 0` で埋め、常に `months` 件返す。月ラベルは
    /// ローカル暦から生成し（`new Date(y, m-i, 1)` 相当のロールオーバー整数計算）、
    /// クエリは最古ラベル以降を `GROUP BY substr(date,1,7)` で集計する。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn monthly_trend(
        &self,
        scope: &UserScope,
        months: i64,
    ) -> Result<Vec<MonthlyTrendPoint>, DbError> {
        let n = months.max(1);
        let (cur_year, cur_month) = self.current_year_month().await?;
        // 当月を含む過去 n ヶ月の 'YYYY-MM' ラベルを古い順に生成（0-based 月インデックスで
        // ロールオーバー: idx = year*12 + (month-1) - i）。
        let base = cur_year * 12 + (cur_month - 1);
        let labels: Vec<String> = (0..n)
            .rev()
            .map(|i| {
                let idx = base - i;
                let y = idx.div_euclid(12);
                let m = idx.rem_euclid(12) + 1;
                format!("{}-{:02}", y, m)
            })
            .collect();
        // labels は n>=1 で必ず 1 件以上だが、indexing_slicing 回避のため first() で取り出す。
        let Some(oldest) = labels.first().cloned() else {
            return Ok(Vec::new());
        };

        let (uid, bid) = scope_keys(scope);
        let rows: Vec<MonthlyTrendPoint> = self
            .read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT substr(date, 1, 7) AS month, \
                                COALESCE(SUM(CASE WHEN type = 'income' THEN amount ELSE 0 END), 0) AS income, \
                                COALESCE(SUM(CASE WHEN type = 'expense' THEN amount ELSE 0 END), 0) AS expense \
                         FROM expenses \
                         WHERE user_id = ?1 AND bot_id = ?2 AND substr(date, 1, 7) >= ?3 \
                         GROUP BY month",
                    )
                    .map_err(map_sqlite)?;
                let mapped = stmt
                    .query_map(params![uid, bid, oldest], |row| {
                        Ok(MonthlyTrendPoint {
                            month: row.get("month")?,
                            income: row.get("income")?,
                            expense: row.get("expense")?,
                        })
                    })
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in mapped {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await?;

        // 欠損月をゼロ埋めして常に n 件を古い順で返す。
        Ok(labels
            .into_iter()
            .map(|label| {
                rows.iter()
                    .find(|r| r.month == label)
                    .cloned()
                    .unwrap_or(MonthlyTrendPoint {
                        month: label,
                        income: 0,
                        expense: 0,
                    })
            })
            .collect())
    }

    // ── 予算上限（budget_limits・§3.4.1） ────────────────────────────────────

    /// スコープ内の予算上限を全カテゴリ分、category 昇順で返す。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_budget_limits(
        &self,
        scope: &UserScope,
    ) -> Result<Vec<BudgetLimit>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT category, limit_amount FROM budget_limits \
                         WHERE user_id = ?1 AND bot_id = ?2 ORDER BY category",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid], row_to_budget_limit)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// カテゴリ別の月次予算上限を設定・更新する（UPSERT）。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn upsert_budget_limit(
        &self,
        scope: &UserScope,
        category: String,
        limit_amount: i64,
    ) -> Result<(), DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO budget_limits (user_id, bot_id, category, limit_amount, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, datetime('now', 'localtime')) \
                     ON CONFLICT(user_id, bot_id, category) DO UPDATE SET \
                       limit_amount = excluded.limit_amount, \
                       updated_at = datetime('now', 'localtime')",
                    params![uid, bid, category, limit_amount],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }

    /// カテゴリ別の予算上限を削除する（削除できたら `true`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete_budget_limit(
        &self,
        scope: &UserScope,
        category: String,
    ) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM budget_limits \
                         WHERE user_id = ?1 AND bot_id = ?2 AND category = ?3",
                        params![uid, bid, category],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }

    // ── 支払い予定・消込（planned_payments・§3.4.3） ─────────────────────────

    /// スコープ内の単一支払い予定を取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get_plan(
        &self,
        scope: &UserScope,
        id: i64,
    ) -> Result<Option<PlannedPayment>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {PLAN_COLUMNS} FROM planned_payments \
                     WHERE user_id = ?1 AND bot_id = ?2 AND id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![uid, bid, id], row_to_plan)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// スコープ内の支払い予定一覧を返す。
    ///
    /// `include_paid=false` は pending のみ（期日昇順・id 昇順）。
    /// `include_paid=true` は全ステータス（pending 優先 → 期日昇順・id 昇順）。
    /// Node `listPlannedPayments`（既定 pending / `status:'all'`）と一致。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_plans(
        &self,
        scope: &UserScope,
        include_paid: bool,
    ) -> Result<Vec<PlannedPayment>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = if include_paid {
                    format!(
                        "SELECT {PLAN_COLUMNS} FROM planned_payments \
                         WHERE user_id = ?1 AND bot_id = ?2 \
                         ORDER BY CASE status WHEN 'pending' THEN 0 ELSE 1 END, \
                                  due_date ASC, id ASC"
                    )
                } else {
                    format!(
                        "SELECT {PLAN_COLUMNS} FROM planned_payments \
                         WHERE user_id = ?1 AND bot_id = ?2 AND status = 'pending' \
                         ORDER BY due_date ASC, id ASC"
                    )
                };
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid], row_to_plan)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// 支払い予定を登録し、作成後の行を返す。
    ///
    /// `due_date` は呼び出し側で解決済み（Node の `dueDate ?? plannedDate`）。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add_plan(
        &self,
        scope: &UserScope,
        input: NewPlannedPayment,
        due_date: String,
    ) -> Result<PlannedPayment, DbError> {
        let (uid, bid) = scope_keys(scope);
        let id = self
            .writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO planned_payments \
                       (user_id, bot_id, title, amount, category, memo, due_date, repeat_rule) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        uid,
                        bid,
                        input.title,
                        input.amount,
                        input.category,
                        input.description,
                        due_date,
                        input.repeat_rule,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get_plan(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted planned_payment not found".to_owned()))
    }

    /// 支払い予定をキャンセルする（pending のみ対象・キャンセルできたら `true`）。
    ///
    /// Node `cancelPlannedPayment` parity（`plans/delete` は成否 bool だけを返す）。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn cancel_plan(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "UPDATE planned_payments \
                         SET status = 'cancelled', updated_at = datetime('now', 'localtime') \
                         WHERE user_id = ?1 AND bot_id = ?2 AND id = ?3 AND status = 'pending'",
                        params![uid, bid, id],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }

    /// pending の支払い予定を消込する（§3.4.3）。
    ///
    /// Node `plans/pay` の手順を1つの writer トランザクションで再現する:
    /// (1) 実支払いを Expense（source=manual・type=expense）として記録、
    /// (2) 予定を `status='settled'` + `settled_expense_id` へ更新、
    /// (3) 紐付く未完了 ToDo（`linked_payment_id` = 予定 id・status='open'）を `done` へ自動完了。
    /// Node は繰り返し予定の次回自動生成を **HTTP 経路では行わない**（cron/LLM 経路のみ）ため、
    /// ここでも `advance_recurring` 相当は行わない（厳密 parity）。
    ///
    /// 返り値: 記録した Expense の id と、自動完了した ToDo 件数。
    ///
    /// # Errors
    /// 各更新失敗時 [`DbError`]。
    pub async fn settle_plan(
        &self,
        scope: &UserScope,
        plan: &PlannedPayment,
    ) -> Result<(i64, i64), DbError> {
        let (uid, bid) = scope_keys(scope);
        let plan_id = plan.id;
        let amount = plan.amount;
        let category = plan.category.clone();
        let title = plan.title.clone();
        self.writer
            .transaction(move |tx| {
                // (1) 実支払いを Expense として記録（Node addExpense: source=manual/type=expense、
                //     memo=plan.title、date/time は localtime 補完）。
                tx.execute(
                    "INSERT INTO expenses \
                       (user_id, bot_id, type, amount, category, memo, date, time, source, created_at) \
                     VALUES (?1, ?2, 'expense', ?3, ?4, ?5, \
                             date('now', 'localtime'), time('now', 'localtime'), \
                             'manual', datetime('now', 'localtime'))",
                    params![uid, bid, amount, category, title],
                )
                .map_err(map_sqlite)?;
                let expense_id = tx.last_insert_rowid();

                // (2) 予定を消込（pending のみ・settled_expense_id を記録）。
                tx.execute(
                    "UPDATE planned_payments \
                     SET status = 'settled', settled_expense_id = ?1, \
                         updated_at = datetime('now', 'localtime') \
                     WHERE user_id = ?2 AND bot_id = ?3 AND id = ?4 AND status = 'pending'",
                    params![expense_id, uid, bid, plan_id],
                )
                .map_err(map_sqlite)?;

                // (3) 紐付く未完了 ToDo を自動完了（§3.4.3 手順4）。件数を返す。
                let completed = tx
                    .execute(
                        "UPDATE todos \
                         SET status = 'done', updated_at = datetime('now', 'localtime') \
                         WHERE user_id = ?1 AND bot_id = ?2 \
                           AND linked_payment_id = ?3 AND status = 'open'",
                        params![uid, bid, plan_id],
                    )
                    .map_err(map_sqlite)?;

                Ok((expense_id, i64::try_from(completed).unwrap_or(i64::MAX)))
            })
            .await
    }
}

/// スコープから所有 String キーを取り出す（`spawn_blocking` の `'static` クロージャ用）。
fn scope_keys(scope: &UserScope) -> (String, String) {
    (
        scope.user_id().as_str().to_owned(),
        scope.bot_id().as_str().to_owned(),
    )
}

/// SQLite 行を [`Expense`] へ変換する。
fn row_to_expense(row: &Row) -> rusqlite::Result<Expense> {
    Ok(Expense {
        id: row.get("id")?,
        r#type: row.get("type")?,
        amount: row.get("amount")?,
        category: row.get("category")?,
        memo: row.get("memo")?,
        date: row.get("date")?,
        time: row.get("time")?,
        source: row.get("source")?,
        created_at: row.get("created_at")?,
    })
}

/// SQLite 行を [`BudgetLimit`] へ変換する。
fn row_to_budget_limit(row: &Row) -> rusqlite::Result<BudgetLimit> {
    Ok(BudgetLimit {
        category: row.get("category")?,
        limit_amount: row.get("limit_amount")?,
    })
}

/// SQLite 行を [`PlannedPayment`] へ変換する（クリーンビュー・内部列は含めない）。
fn row_to_plan(row: &Row) -> rusqlite::Result<PlannedPayment> {
    Ok(PlannedPayment {
        id: row.get("id")?,
        title: row.get("title")?,
        amount: row.get("amount")?,
        category: row.get("category")?,
        memo: row.get("memo")?,
        due_date: row.get("due_date")?,
        repeat_rule: row.get("repeat_rule")?,
        status: row.get("status")?,
        settled_expense_id: row.get("settled_expense_id")?,
        linked_todo_id: row.get("linked_todo_id")?,
        linked_reminder_id: row.get("linked_reminder_id")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}
