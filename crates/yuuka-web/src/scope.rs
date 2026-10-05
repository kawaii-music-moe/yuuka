//! `UserScope` の解決（**全ドメイン共通**・bot アクセス認可を含む）。
//!
//! 認証済みユーザーと `?botId=` から `UserScope` を組み立てる際、**bot への
//! アクセス権を必ず検証**する（既存 Node `resolveBotId`/`hasBotAccess` と一致）。
//! これを各ドメインが自前で書くと認可漏れが8ドメインに増幅するため、ここに一元化し
//! `yuuka_web::resolve_scope` として全ドメインに使わせる（レビュー H-1）。

use rusqlite::{params, OptionalExtension};
use yuuka_core::{BotId, DbError, UserId, UserScope};
use yuuka_db::map_sqlite;
use yuuka_types::SessionUser;

use crate::state::Db;

/// 認証ユーザーが指定 bot にアクセスできるか（`system_default` は常に可）。
///
/// 既存 Node `hasBotAccess`（botRepo.ts）と同一クエリ: 自分がオーナー、または
/// `bot_shares` に `status='active'` の共有がある場合に可。
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

/// `?botId=` を認可検証しつつ `UserScope` へ解決する（全ドメイン共通の入口）。
///
/// 未指定・空・`system_default`・**アクセス不可**はいずれも `system_default` に
/// フォールバックする（Node `resolveBotId` と一致）。これにより「他人の bot を
/// `?botId=` で指定してデータ空間へ横断アクセス」を構造的に封じる。
///
/// アクセス可能な実 bot（`system_default` 以外）では **オーナー ID を [`UserScope`] に束ねる**
/// （[`UserScope::with_owner`]）。これにより共有 bot のペルソナ・MCP 等が
/// [`UserScope::config_owner_id`] 経由でオーナーの名前空間へ正規化され、共有相手（A/B）間で
/// 同期される。`system_default`（共有秘書）はオーナー無し＝発話ユーザー単位で独立（従来どおり）。
///
/// # Errors
/// bot アクセス確認のクエリ失敗時 [`DbError`]。
pub async fn resolve_scope(
    user: &SessionUser,
    db: &Db,
    raw_bot_id: Option<&str>,
) -> Result<UserScope, DbError> {
    let uid = UserId::new(user.discord_id.clone());
    match raw_bot_id {
        Some(b) if !b.is_empty() && b != "system_default" => {
            match bot_owner_if_accessible(db, user.discord_id.as_str(), b).await? {
                Some(owner) => Ok(UserScope::with_owner(
                    uid,
                    BotId::new(b),
                    UserId::new(owner),
                )),
                // アクセス不可・未知 bot は system_default へフォールバック（横断参照を封じる）。
                None => Ok(UserScope::new(uid, BotId::system_default())),
            }
        }
        _ => Ok(UserScope::new(uid, BotId::system_default())),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_scope;
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
                    .unwrap()
                    .bot_id()
                    .as_str()
                    .to_owned()
            }
        };

        // 未指定・system_default は system_default。
        assert_eq!(s(None).await, "system_default");
        assert_eq!(s(Some("system_default")).await, "system_default");
        // 所有 bot は許可。
        assert_eq!(s(Some("mybot")).await, "mybot");
        // active 共有 bot は許可。
        assert_eq!(s(Some("otherbot")).await, "otherbot");
        // アクセス不可 bot は system_default にフォールバック（クロス bot 参照を封じる）。
        assert_eq!(s(Some("nobot")).await, "system_default");
        // 未知 bot もフォールバック。
        assert_eq!(s(Some("ghost")).await, "system_default");
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

        // system_default はオーナー無し＝発話ユーザー単位（従来どおり独立）。
        let s_default = resolve_scope(&owner, &db, None).await.unwrap();
        assert_eq!(s_default.bot_owner_id(), None);
        assert_eq!(s_default.config_owner_id().as_str(), "owner");

        // アクセス不可 bot は system_default フォールバック＝オーナー無し。
        let s_denied = resolve_scope(&owner, &db, Some("nobot")).await.unwrap();
        assert_eq!(s_denied.bot_id().as_str(), "system_default");
        assert_eq!(s_denied.bot_owner_id(), None);
    }
}
