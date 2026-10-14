//! `UserScope` の解決（**全ドメイン共通**・bot アクセス認可を含む）。
//!
//! 認証済みユーザーと `?botId=` から `UserScope` を組み立てる際、**bot への
//! アクセス権を必ず検証**する（既存 Node `resolveBotId`/`hasBotAccess` と一致）。
//! これを各ドメインが自前で書くと認可漏れが8ドメインに増幅するため、ここに一元化し
//! `yuuka_web::resolve_scope` として全ドメインに使わせる（レビュー H-1）。
//!
//! システム Bot（`system_default`）は案内役で個人データを持たないため、[`resolve_scope`] は
//! システム Bot のスコープを作らない（#139）。

use axum::response::{IntoResponse, Response};
use rusqlite::{params, OptionalExtension};
use yuuka_core::{BotId, DbError, UserId, UserScope, WebError};
use yuuka_db::map_sqlite;
use yuuka_types::SessionUser;

use crate::error::ApiError;
use crate::state::Db;

/// 認証ユーザーが指定 bot にアクセスできるか（`system_default` は常に可）。
///
/// 既存 Node `hasBotAccess`（botRepo.ts）と同一クエリ: 自分がオーナー、または
/// `bot_shares` に `status='active'` の共有がある場合に可。個人データのルートはこれではなく
/// [`resolve_scope`]（システム Bot を拒否する）を使う。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn has_bot_access(db: &Db, user_id: &str, bot_id: &str) -> Result<bool, DbError> {
    if bot_id == "system_default" {
        return Ok(true);
    }
    let uid = user_id.to_owned();
    let bid = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM bots b \
                     LEFT JOIN bot_shares s \
                       ON s.bot_id = b.id AND s.shared_user_id = ?1 AND s.status = 'active' \
                     WHERE b.id = ?2 AND (b.user_id = ?1 OR s.id IS NOT NULL)",
                    params![uid, bid],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            Ok(found.is_some())
        })
        .await
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

/// [`resolve_scope`] の失敗。HTTP では [`ApiError`] へ写像する（403 / 404 / 500・502）。
#[derive(Debug)]
pub enum ScopeError {
    /// `botId` が未指定・空・`system_default`（システム Bot は個人データを持たない）→ 403。
    BotRequired,
    /// 指定した Bot にアクセスできない（未知・他人の未共有 Bot）→ 404。
    BotNotFound,
    /// bot アクセス確認のクエリ失敗。
    Db(DbError),
}

impl From<DbError> for ScopeError {
    fn from(e: DbError) -> Self {
        Self::Db(e)
    }
}

impl From<ScopeError> for ApiError {
    fn from(e: ScopeError) -> Self {
        match e {
            ScopeError::BotRequired => Self(WebError::Forbidden),
            ScopeError::BotNotFound => Self(WebError::NotFound),
            ScopeError::Db(e) => Self::from(e),
        }
    }
}

impl IntoResponse for ScopeError {
    fn into_response(self) -> Response {
        ApiError::from(self).into_response()
    }
}

/// `?botId=` を認可検証しつつ `UserScope` へ解決する（全ドメイン共通の入口）。
///
/// 前後の空白を除いた `botId` が未指定・空・`system_default` なら [`ScopeError::BotRequired`]、
/// アクセスできない（未知・他人の未共有）Bot なら [`ScopeError::BotNotFound`]。どちらも
/// システム Bot へはフォールバックしない（PWA 用 API の `client_scope` と同じ方針）。これにより
/// 「他人の bot を `?botId=` で指定してデータ空間へ横断アクセス」も「システム Bot へ個人データを
/// 読み書き」も構造的に封じる。
///
/// 解決した bot では **オーナー ID を [`UserScope`] に束ねる**（[`UserScope::with_owner`]）。
/// これにより共有 bot のペルソナ・MCP 等が [`UserScope::config_owner_id`] 経由でオーナーの
/// 名前空間へ正規化され、共有相手（A/B）間で同期される。
///
/// # Errors
/// 上記の [`ScopeError::BotRequired`] / [`ScopeError::BotNotFound`]、bot アクセス確認のクエリ
/// 失敗時 [`ScopeError::Db`]。
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
    use super::{resolve_scope, ScopeError};
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
            // system_default の行はセットアップした管理者（ここでは 'owner'）が持ち主になる。
            conn.execute_batch(
                "CREATE TABLE bots(id TEXT PRIMARY KEY, user_id TEXT NOT NULL);
                 CREATE TABLE bot_shares(id INTEGER PRIMARY KEY AUTOINCREMENT, bot_id TEXT, \
                    shared_user_id TEXT, status TEXT);
                 INSERT INTO bots(id, user_id) VALUES('system_default', 'owner');
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
            async move { resolve_scope(&u, &db, b).await }
        };
        let bot = |r: Result<yuuka_core::UserScope, ScopeError>| {
            r.expect("resolved").bot_id().as_str().to_owned()
        };

        // 所有 bot は許可。
        assert_eq!(bot(s(Some("mybot")).await), "mybot");
        // active 共有 bot は許可。
        assert_eq!(bot(s(Some("otherbot")).await), "otherbot");
        // 前後の空白は除いて解決する。
        assert_eq!(bot(s(Some(" mybot ")).await), "mybot");
        // 未指定・空・system_default はシステム Bot 宛てなので拒否（system_default の行の持ち主でも）。
        for b in [None, Some(""), Some("  "), Some("system_default")] {
            assert!(
                matches!(s(b).await, Err(ScopeError::BotRequired)),
                "botId={b:?}"
            );
        }
        // アクセス不可 bot・未知 bot は system_default へフォールバックせず not found。
        for b in ["nobot", "ghost"] {
            assert!(
                matches!(s(Some(b)).await, Err(ScopeError::BotNotFound)),
                "botId={b}"
            );
        }
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
}
