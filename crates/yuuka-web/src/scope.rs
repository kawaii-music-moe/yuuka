//! `UserScope` の解決（**全ドメイン共通**・bot アクセス認可を含む）。
//!
//! 認証済みユーザーと `?botId=` から `UserScope` を組み立てる際、**bot への
//! アクセス権を必ず検証**する（既存 Node `resolveBotId`/`hasBotAccess` と一致）。
//! これを各ドメインが自前で書くと認可漏れが8ドメインに増幅するため、ここに一元化し
//! `yuuka_web::resolve_scope` として全ドメインに使わせる（レビュー H-1）。
//!
//! システムデフォルト Bot（`system_default`）はシステムの案内役で、個人データを持たない
//! （issue #138）。そのため Bot 未指定・`system_default` は [`ScopeError::BotRequired`]（403）、
//! アクセスできない Bot は [`ScopeError::BotNotFound`]（404）にし、システム Bot へは落とさない。

use axum::response::{IntoResponse, Response};
use rusqlite::{params, OptionalExtension};
use yuuka_core::{BotId, DbError, UserId, UserScope, WebError};
use yuuka_db::map_sqlite;
use yuuka_types::SessionUser;

use crate::error::ApiError;
use crate::state::Db;

/// [`resolve_scope`] の失敗。
#[derive(Debug, thiserror::Error)]
pub enum ScopeError {
    /// `botId` 未指定・空・`system_default`（個人データはユーザーの Bot にだけ置く）。
    #[error("bot required")]
    BotRequired,
    /// 指定 Bot が未知、またはアクセスできない（他人の未共有 Bot）。
    #[error("bot not found")]
    BotNotFound,
    /// bot アクセス確認のクエリ失敗。
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<ScopeError> for ApiError {
    fn from(e: ScopeError) -> Self {
        match e {
            ScopeError::BotRequired => Self(WebError::Forbidden),
            ScopeError::BotNotFound => Self(WebError::NotFound),
            ScopeError::Db(e) => e.into(),
        }
    }
}

impl IntoResponse for ScopeError {
    fn into_response(self) -> Response {
        ApiError::from(self).into_response()
    }
}

/// 認証ユーザーが指定 bot にアクセスできるか。
///
/// 既存 Node `hasBotAccess`（botRepo.ts）と同一クエリ: 自分がオーナー、または
/// `bot_shares` に `status='active'` の共有がある場合に可。`system_default` も特別扱いせず、
/// `bots` 行の持ち主（システム Bot のトークンを設定した管理者）だけが可。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn has_bot_access(db: &Db, user_id: &str, bot_id: &str) -> Result<bool, DbError> {
    Ok(bot_owner_if_accessible(db, user_id, bot_id)
        .await?
        .is_some())
}

/// アクセス可能なら bot のオーナー（`bots.user_id`）を返す（オーナー本人 or `active` 共有）。
///
/// [`has_bot_access`] と同一の認可条件を 1 クエリで満たしつつ、**オーナー ID を取り出す**。
/// 共有 bot の設定（ペルソナ・MCP）を owner-canonical に解決するために `resolve_scope` が使う。
/// アクセス不可・未知 bot は `None`。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
async fn bot_owner_if_accessible(
    db: &Db,
    user_id: &str,
    bot_id: &str,
) -> Result<Option<String>, DbError> {
    let uid = user_id.to_owned();
    let bid = bot_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT b.user_id FROM bots b \
                 LEFT JOIN bot_shares s \
                   ON s.bot_id = b.id AND s.shared_user_id = ?1 AND s.status = 'active' \
                 WHERE b.id = ?2 AND (b.user_id = ?1 OR s.id IS NOT NULL)",
                params![uid, bid],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// `?botId=` を認可検証しつつ `UserScope` へ解決する（全ドメイン共通の入口）。
///
/// 未指定・空・`system_default` は [`ScopeError::BotRequired`]、**アクセス不可**・未知 bot は
/// [`ScopeError::BotNotFound`]。どちらもシステム Bot へはフォールバックしない（システム Bot に
/// 個人データを読み書きさせない・他人の bot を `?botId=` で指定した横断アクセスも封じる）。
///
/// アクセス可能な bot では **オーナー ID を [`UserScope`] に束ねる**（[`UserScope::with_owner`]）。
/// これにより共有 bot のペルソナ・MCP 等が [`UserScope::config_owner_id`] 経由でオーナーの
/// 名前空間へ正規化され、共有相手（A/B）間で同期される。
///
/// # Errors
/// 上記の [`ScopeError`]。
pub async fn resolve_scope(
    user: &SessionUser,
    db: &Db,
    raw_bot_id: Option<&str>,
) -> Result<UserScope, ScopeError> {
    let requested = raw_bot_id
        .map(str::trim)
        .filter(|b| !b.is_empty() && *b != BotId::SYSTEM_DEFAULT)
        .ok_or(ScopeError::BotRequired)?;
    let owner = bot_owner_if_accessible(db, user.discord_id.as_str(), requested)
        .await?
        .ok_or(ScopeError::BotNotFound)?;
    Ok(UserScope::with_owner(
        UserId::new(user.discord_id.clone()),
        BotId::new(requested),
        UserId::new(owner),
    ))
}

#[cfg(test)]
mod tests {
    use super::{has_bot_access, resolve_scope, ScopeError};
    use crate::state::Db;
    use std::sync::atomic::{AtomicU64, Ordering};
    use yuuka_types::{Role, SessionUser};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn db_with_bots() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_scope_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed");
            conn.execute_batch(
                "CREATE TABLE bots(id TEXT PRIMARY KEY, user_id TEXT NOT NULL);
                 CREATE TABLE bot_shares(id INTEGER PRIMARY KEY AUTOINCREMENT, bot_id TEXT, \
                    shared_user_id TEXT, status TEXT);
                 INSERT INTO bots(id, user_id) VALUES('mybot', 'owner');
                 INSERT INTO bots(id, user_id) VALUES('otherbot', 'someone');
                 INSERT INTO bots(id, user_id) VALUES('nobot', 'someone');
                 INSERT INTO bot_shares(bot_id, shared_user_id, status) \
                    VALUES('otherbot', 'owner', 'active');",
            )
            .expect("ddl");
        }
        Db::open(&path).expect("open")
    }

    fn user(id: &str) -> SessionUser {
        SessionUser {
            discord_id: id.to_owned(),
            username: id.to_owned(),
            role: Role::User,
        }
    }

    #[tokio::test]
    async fn resolve_scope_enforces_bot_access() {
        let db = db_with_bots();
        let owner = user("owner");
        let s = |b: Option<&'static str>| {
            let db = db.clone();
            let u = owner.clone();
            async move {
                resolve_scope(&u, &db, b)
                    .await
                    .map(|scope| scope.bot_id().as_str().to_owned())
            }
        };

        // 未指定・空・system_default は BotRequired（システム Bot へ落とさない）。
        assert!(matches!(s(None).await, Err(ScopeError::BotRequired)));
        assert!(matches!(s(Some("")).await, Err(ScopeError::BotRequired)));
        assert!(matches!(
            s(Some("system_default")).await,
            Err(ScopeError::BotRequired)
        ));
        // 所有 bot は許可。
        assert_eq!(s(Some("mybot")).await.unwrap(), "mybot");
        // active 共有 bot は許可。
        assert_eq!(s(Some("otherbot")).await.unwrap(), "otherbot");
        // アクセス不可 bot・未知 bot は BotNotFound（クロス bot 参照を封じる）。
        assert!(matches!(
            s(Some("nobot")).await,
            Err(ScopeError::BotNotFound)
        ));
        assert!(matches!(
            s(Some("ghost")).await,
            Err(ScopeError::BotNotFound)
        ));
    }

    #[tokio::test]
    async fn resolve_scope_binds_bot_owner_for_shared_and_owned() {
        let db = db_with_bots();
        // オーナー本人が所有 bot を解決すると owner が束ねられ、config_owner はオーナー。
        let owner = user("owner");
        let s_owned = resolve_scope(&owner, &db, Some("mybot")).await.unwrap();
        assert_eq!(s_owned.bot_owner_id().map(|u| u.as_str()), Some("owner"));
        assert_eq!(s_owned.config_owner_id().as_str(), "owner");

        // 共有相手（active 共有）が解決すると config_owner は**オーナー**（共有相手ではない）。
        // 'otherbot' の owner は 'someone'、'owner' へ active 共有済み。
        let s_shared = resolve_scope(&owner, &db, Some("otherbot")).await.unwrap();
        assert_eq!(s_shared.user_id().as_str(), "owner");
        assert_eq!(s_shared.bot_owner_id().map(|u| u.as_str()), Some("someone"));
        assert_eq!(s_shared.config_owner_id().as_str(), "someone");
    }

    #[tokio::test]
    async fn has_bot_access_does_not_special_case_system_default() {
        let db = db_with_bots();
        // system_default 行の無い DB では誰もアクセスできない（持ち主だけが可）。
        assert!(!has_bot_access(&db, "owner", "system_default")
            .await
            .unwrap());
        assert!(has_bot_access(&db, "owner", "mybot").await.unwrap());
        assert!(!has_bot_access(&db, "owner", "nobot").await.unwrap());
    }
}
