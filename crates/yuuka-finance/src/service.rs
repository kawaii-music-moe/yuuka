//! 収支の記帳・修正・削除の共通経路（D02 #126）。
//!
//! 管理画面の `POST /api/expenses/add`・PWA の `POST /api/client/finance/transactions`・
//! エージェントのツール（`addExpense`/`updateExpense`/`deleteExpense`）は書き込みをすべてここに
//! 通す。入口ごとに repo を直接呼ぶと、必須項目や金額の扱いが経路ごとに食い違うため。
//!
//! 金額の扱いは入口の用途で 2 つに分ける（どちらもここで判定する）:
//! - [`FinanceService::add_entry`]: 管理画面の記帳。0 以外なら負の金額（調整）も受け付ける
//!   （旧 Node からの管理 API の契約）。
//! - [`FinanceService::add_transaction`]: 利用者の取引登録（PWA・ツール）。1 円以上だけ。
//!
//! 記帳後の予算消化率・消込候補の案内（R16）はツールの会話向けの補足なので `addExpense` に残す。
//! レシート読み取り（D08 #132）は下書きを返すだけで、登録は利用者がここを通して行う。

use yuuka_core::UserScope;
use yuuka_web::{Db, DomainError};

use crate::dto::{Expense, ExpenseUpdate, NewExpense};
use crate::repo::ExpenseRepo;

/// 必須項目が欠けているときの文言（管理 API・PWA の従来の 400 と同じ）。
const REQUIRED_MESSAGE: &str = "amount and category are required";

/// 収支の書き込み（全入口共通）。
pub struct FinanceService<'a> {
    db: &'a Db,
}

impl<'a> FinanceService<'a> {
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// 管理画面の記帳（`source = manual`）。金額は 0 以外（負の調整を含む）、カテゴリ必須。
    ///
    /// # Errors
    /// 入力が不正なら [`DomainError::Invalid`]、DB 失敗は [`DomainError::Db`]。
    pub async fn add_entry(
        &self,
        scope: &UserScope,
        input: NewExpense,
    ) -> Result<Expense, DomainError> {
        if input.amount == 0 {
            return Err(DomainError::Invalid(REQUIRED_MESSAGE.to_owned()));
        }
        check_category(&input.category)?;
        Ok(ExpenseRepo::new(self.db).add(scope, input).await?)
    }

    /// 利用者の取引登録（PWA・`addExpense` ツール）。金額は 1 円以上、カテゴリ必須。
    /// `source` は `receipt_ocr` 以外を `manual` として保存する（D08 #132）。
    ///
    /// # Errors
    /// 入力が不正なら [`DomainError::Invalid`]、DB 失敗は [`DomainError::Db`]。
    pub async fn add_transaction(
        &self,
        scope: &UserScope,
        input: NewExpense,
        source: &str,
    ) -> Result<Expense, DomainError> {
        if input.amount <= 0 {
            return Err(DomainError::Invalid(REQUIRED_MESSAGE.to_owned()));
        }
        check_category(&input.category)?;
        Ok(ExpenseRepo::new(self.db)
            .add_with_source(scope, input, source)
            .await?)
    }

    /// 指定項目だけを修正する。スコープ内に無ければ `None`。金額は 1 円以上、カテゴリは空にできない。
    ///
    /// # Errors
    /// 入力が不正なら [`DomainError::Invalid`]、DB 失敗は [`DomainError::Db`]。
    pub async fn update(
        &self,
        scope: &UserScope,
        id: i64,
        input: ExpenseUpdate,
    ) -> Result<Option<Expense>, DomainError> {
        if input.amount.is_some_and(|amount| amount <= 0) {
            return Err(DomainError::Invalid(
                "amount must be a positive integer".to_owned(),
            ));
        }
        if let Some(category) = &input.category {
            check_category(category)?;
        }
        Ok(ExpenseRepo::new(self.db).update(scope, id, input).await?)
    }

    /// 1 件削除する。削除できたら `true`。
    ///
    /// # Errors
    /// DB 失敗時 [`DomainError::Db`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DomainError> {
        Ok(ExpenseRepo::new(self.db).delete(scope, id).await?)
    }
}

fn check_category(category: &str) -> Result<(), DomainError> {
    if category.trim().is_empty() {
        return Err(DomainError::Invalid(REQUIRED_MESSAGE.to_owned()));
    }
    Ok(())
}
