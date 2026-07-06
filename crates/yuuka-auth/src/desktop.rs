//! Bearer デスクトップトークン認証（SQLite `desktop_tokens` + `users`）。
//!
//! Node `verifyToken`（desktopAuthService.ts）パリティ:
//! 1. `token_hash` 一致・未失効・TTL 内の行を引く。
//! 2. `users` から username/role を解決（無ければ無効）。
//! 3. `last_used_at` を現在時刻へ touch（スライディング更新）。

use rusqlite::{params, OptionalExtension};
use yuuka_core::AuthError;
use yuuka_db::map_sqlite;
use yuuka_types::{Role, SessionUser};
use yuuka_web::Db;

/// `token_hash`（sha256hex）で Bearer トークンを検証し、対応ユーザーを返す。
///
/// 有効期限は `COALESCE(last_used_at, created_at) + ttl_days > now`（Node と一致）。
///
/// # Errors
/// DB 到達不能等は [`AuthError::Backend`]（502 に写像・監視に載せる。401 とは区別）。
pub async fn verify(
    db: &Db,
    token_hash: String,
    ttl_days: i64,
) -> Result<Option<SessionUser>, AuthError> {
    // 1) 有効なトークン行（id, user_id）を引く。
    let hash = token_hash;
    let row: Option<(i64, String)> = db
        .read
        .read(move |conn| {
            conn.query_row(
                "SELECT id, user_id FROM desktop_tokens \
                 WHERE token_hash = ?1 AND revoked = 0 \
                   AND datetime(COALESCE(last_used_at, created_at), '+' || ?2 || ' days') \
                       > datetime('now', 'localtime')",
                params![hash, ttl_days],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
        .map_err(|_| AuthError::Backend)?;
    let Some((token_id, user_id)) = row else {
        return Ok(None);
    };

    // 2) users から username/role を解決（ユーザー不在なら無効扱い）。
    let uid = user_id.clone();
    let user: Option<(String, String)> = db
        .read
        .read(move |conn| {
            conn.query_row(
                "SELECT username, role FROM users WHERE discord_id = ?1",
                params![uid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
        .map_err(|_| AuthError::Backend)?;
    let Some((username, role)) = user else {
        return Ok(None);
    };

    // 3) last_used_at を touch（スライディング）。失敗は致命でないので握って続行。
    if let Err(e) = db
        .writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE desktop_tokens SET last_used_at = datetime('now', 'localtime') WHERE id = ?1",
                params![token_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
    {
        tracing::warn!(error = %e, "desktop token last_used_at の更新に失敗（認証は継続）");
    }

    Ok(Some(SessionUser {
        discord_id: user_id,
        username,
        role: parse_role(&role),
    }))
}

/// `users.role`（'user'|'admin'）を [`Role`] へ写像（未知は user・Node の `|| "user"` 相当）。
fn parse_role(raw: &str) -> Role {
    if raw.eq_ignore_ascii_case("admin") {
        Role::Admin
    } else {
        Role::User
    }
}

#[cfg(test)]
mod tests {
    use super::verify;
    use crate::sha256_hex;
    use yuuka_types::Role;
    use yuuka_web::Db;

    fn seed_db() -> Db {
        let path = std::env::temp_dir()
            .join(format!("yuuka_auth_desktop_{}_{:?}.sqlite", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).expect("seed");
            conn.execute_batch(
                "CREATE TABLE users (discord_id TEXT PRIMARY KEY, username TEXT NOT NULL, \
                    role TEXT NOT NULL DEFAULT 'user');
                 CREATE TABLE desktop_tokens (id INTEGER PRIMARY KEY AUTOINCREMENT, \
                    user_id TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE, device_name TEXT, \
                    created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), \
                    last_used_at TEXT, revoked INTEGER NOT NULL DEFAULT 0);
                 INSERT INTO users(discord_id, username, role) VALUES('123','alice','admin');",
            )
            .expect("ddl");
        }
        Db::open(&path).expect("open")
    }

    #[tokio::test]
    async fn valid_token_resolves_user_and_role() {
        let db = seed_db();
        // 直接挿入（有効・未失効・作成直後）。
        let hash = sha256_hex("tok-good");
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO desktop_tokens(user_id, token_hash) VALUES('123', ?1)",
                    rusqlite::params![hash],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();

        let user = verify(&db, sha256_hex("tok-good"), 90).await.unwrap().expect("user");
        assert_eq!(user.discord_id, "123");
        assert_eq!(user.username, "alice");
        assert_eq!(user.role, Role::Admin);
    }

    #[tokio::test]
    async fn revoked_or_unknown_token_is_none() {
        let db = seed_db();
        let hash = sha256_hex("tok-revoked");
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO desktop_tokens(user_id, token_hash, revoked) VALUES('123', ?1, 1)",
                    rusqlite::params![hash],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(verify(&db, sha256_hex("tok-revoked"), 90).await.unwrap().is_none());
        assert!(verify(&db, sha256_hex("never-issued"), 90).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn expired_token_is_none() {
        let db = seed_db();
        // created_at を 100 日前にして TTL 90 日を超過させる。
        let hash = sha256_hex("tok-old");
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO desktop_tokens(user_id, token_hash, created_at) \
                     VALUES('123', ?1, datetime('now','localtime','-100 days'))",
                    rusqlite::params![hash],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(verify(&db, sha256_hex("tok-old"), 90).await.unwrap().is_none());
    }
}
