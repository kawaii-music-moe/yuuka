//! Repo / UserScope 契約（§7.3・§12.2 の凍結契約5）。
//!
//! `UserScope` は構築時に `UserId` を必ず束縛する。リポジトリのメソッドは
//! `&UserScope` を取ることで「user_id 無しクエリ」を型で不能化する。
//! 横断スキャン（全ユーザー跨ぎ）は `CronScan` に隔離し、通常 repo には現れないようにする。

use crate::error::RepoError;
use crate::ids::{BotId, UserId};

/// データ分離スコープ。構築時に `UserId` を束縛する（生成後に user_id を差し替え不可）。
#[derive(Debug, Clone)]
pub struct UserScope {
    user_id: UserId,
    bot_id: BotId,
}

impl UserScope {
    /// スコープを束縛して生成する。以後この scope 越しのクエリは必ず user_id を持つ。
    #[must_use]
    pub fn new(user_id: UserId, bot_id: BotId) -> Self {
        Self { user_id, bot_id }
    }

    #[must_use]
    pub fn user_id(&self) -> &UserId {
        &self.user_id
    }

    #[must_use]
    pub fn bot_id(&self) -> &BotId {
        &self.bot_id
    }
}

/// スコープ束縛リポジトリの基底契約。ドメイン別 repo（Todo/Finance/…）は Phase 1 で
/// このトレイトを土台に `&UserScope` を取るメソッドを実装する。
///
/// ここでは「全 repo が UserScope を要求する」不変条件を型で凍結する。
/// `async fn in trait`（RPITIT・Rust 1.96 stable）を使い `#[async_trait]` 不要。
pub trait ScopedRepo: Send + Sync {
    /// この repo が期待する分離スコープが妥当か（欠落なら `UserScopeMissing`）を検査する。
    /// 既定実装は「scope が存在すれば妥当」。ドメイン repo は必要に応じて上書きする。
    ///
    /// # Errors
    /// スコープが不正な場合 [`RepoError::UserScopeMissing`] を返す。
    fn validate_scope(&self, _scope: &UserScope) -> Result<(), RepoError> {
        Ok(())
    }
}

/// 横断スキャン契約（全ユーザー跨ぎ）。通常 repo から**隔離**し、cron/バッチ専用にする。
///
/// これにより「うっかり全ユーザーを読むクエリ」が通常のデータアクセス経路に紛れ込まない。
pub trait CronScan: Send + Sync {}
