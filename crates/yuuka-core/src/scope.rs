//! Repo / UserScope 契約（§7.3・§12.2 の凍結契約5）。
//!
//! `UserScope` は構築時に `UserId` を必ず束縛する。リポジトリのメソッドは
//! `&UserScope` を取ることで「user_id 無しクエリ」を型で不能化する。
//! 横断スキャン（全ユーザー跨ぎ）は `CronScan` に隔離し、通常 repo には現れないようにする。

use crate::error::RepoError;
use crate::ids::{BotId, UserId};

/// データ分離スコープ。構築時に `UserId` を束縛する（生成後に user_id を差し替え不可）。
///
/// `bot_owner_id` は「解決された bot のオーナー」（`bots.user_id`）。共有 bot（`bot_shares`）を
/// 別ユーザーが操作したときでも、**bot 単位で共有される設定（ペルソナ・MCP 等）はオーナーの
/// 名前空間へ正規化（owner-canonical）**するために使う。`system_default`（共有秘書）や、bot 未束縛
/// の経路では `None`＝発話ユーザー自身が設定オーナー（従来どおり user 単位で独立）。
#[derive(Debug, Clone)]
pub struct UserScope {
    user_id: UserId,
    bot_id: BotId,
    bot_owner_id: Option<UserId>,
}

impl UserScope {
    /// スコープを束縛して生成する（bot オーナー未解決＝`system_default`／user 単位）。以後この
    /// scope 越しのクエリは必ず user_id を持つ。
    #[must_use]
    pub fn new(user_id: UserId, bot_id: BotId) -> Self {
        Self {
            user_id,
            bot_id,
            bot_owner_id: None,
        }
    }

    /// bot オーナーを束ねて生成する（共有 bot の owner-canonical 解決用）。`resolve_scope` が
    /// `system_default` 以外のアクセス可能な bot を解決したときに使う。
    #[must_use]
    pub fn with_owner(user_id: UserId, bot_id: BotId, bot_owner_id: UserId) -> Self {
        Self {
            user_id,
            bot_id,
            bot_owner_id: Some(bot_owner_id),
        }
    }

    #[must_use]
    pub fn user_id(&self) -> &UserId {
        &self.user_id
    }

    #[must_use]
    pub fn bot_id(&self) -> &BotId {
        &self.bot_id
    }

    /// 解決された bot のオーナー（`bots.user_id`）。`system_default`／未束縛では `None`。
    #[must_use]
    pub fn bot_owner_id(&self) -> Option<&UserId> {
        self.bot_owner_id.as_ref()
    }

    /// **bot 単位で共有される設定のオーナーキー**。共有 bot ではオーナー、それ以外
    /// （`system_default`・未束縛）では発話ユーザー自身。ペルソナ／MCP 等の owner-canonical
    /// なクエリ・書き込みはこのキーで行い、共有 bot の設定を全ユーザーで同期する。
    #[must_use]
    pub fn config_owner_id(&self) -> &UserId {
        self.bot_owner_id.as_ref().unwrap_or(&self.user_id)
    }
}

/// スコープ束縛リポジトリの基底契約。ドメイン別 repo（Todo/Finance/…）は Phase 1 で
/// このトレイトを土台に実装する。
///
/// **分離キーの型強制は各ドメイン repo の「メソッド署名」で行う**: すべての通常クエリ
/// メソッドが第一引数に `&UserScope` を取ることで「user_id 無しクエリ」を型で不能化する
/// （基底トレイトは宣言していないメソッドの署名までは強制できないため、これは Phase 1 の
/// 各 repo が守る規約であり、本トレイトはその規約のマーカー＋実行時フックを提供する）。
/// 横断（全ユーザー跨ぎ）アクセスは通常経路から隔離され、[`CronScan`] ＋ [`CrossUserAccess`]
/// 証憑でのみ到達できる。`async fn in trait`（RPITIT・Rust 1.96 stable）を使い `#[async_trait]` 不要。
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

/// 横断（全ユーザー跨ぎ）アクセスの証憑トークン。cron/バッチのブートストラップでのみ
/// 構築でき、通常のリクエスト経路（`UserScope` 由来）では作れない。
///
/// [`CronScan`] のメソッド（Phase 1 で追加）はこれを引数に取ることで、「横断アクセスは
/// ここでしか起きない」ことを型で可視化し **grep 可能**にする（通常 repo に紛れ込まない）。
/// 構築点は `for_scheduled_task` の呼び出し箇所に限定され、監査で追跡できる。
#[derive(Debug, Clone, Copy)]
pub struct CrossUserAccess {
    _private: (),
}

impl CrossUserAccess {
    /// cron/バッチのスケジュール実行起点でのみ構築する（横断アクセスの明示的な起点）。
    #[must_use]
    pub fn for_scheduled_task() -> Self {
        Self { _private: () }
    }
}

/// 横断スキャン契約（全ユーザー跨ぎ）。通常 repo から**隔離**し、cron/バッチ専用にする。
///
/// Phase 1 で追加する各メソッドは必ず [`CrossUserAccess`] 証憑を引数に取ること
/// （`UserScope` 経路から誤って呼べない＝「うっかり全ユーザーを読むクエリ」が通常の
/// データアクセス経路に紛れ込まない）。
pub trait CronScan: Send + Sync {}
