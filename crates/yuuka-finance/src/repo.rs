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

use crate::dto::{Expense, NewExpense};

/// 返却列（クリーンビュー・内部列 user_id/bot_id は含めない）。
const EXPENSE_COLUMNS: &str =
    "id, type, amount, category, memo, date, time, source, created_at";

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
