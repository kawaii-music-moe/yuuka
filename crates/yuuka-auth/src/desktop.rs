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

/// 長命デスクトップトークンを 1 本登録する（Node `addDesktopToken`）。生トークンの `sha256hex` と
/// `device_name` を保存し、新規行の id を返す（生トークンは呼び出し側だけが保持・サーバは hash のみ）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`](yuuka_core::DbError)。
pub async fn add_desktop_token(
    db: &Db,
    user_id: &str,
    token_hash: &str,
    device_name: Option<&str>,
) -> Result<i64, yuuka_core::DbError> {
    let (user_id, token_hash) = (user_id.to_owned(), token_hash.to_owned());
    let device_name = device_name.map(str::to_owned);
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO desktop_tokens (user_id, token_hash, device_name) \
                 VALUES (?1, ?2, ?3)",
                params![user_id, token_hash, device_name],
            )
            .map_err(map_sqlite)?;
            Ok(tx.last_insert_rowid())
        })
        .await
}

/// あるユーザーの全デスクトップトークンを物理削除する（Node `revokeAllDesktopTokensForUser`）。
///
/// パスワード変更時に全端末を強制再ログインさせるために呼ぶ（`DELETE FROM desktop_tokens
/// WHERE user_id = ?`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`](yuuka_core::DbError)。
pub async fn revoke_all_for_user(db: &Db, user_id: &str) -> Result<(), yuuka_core::DbError> {
    let user_id = user_id.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "DELETE FROM desktop_tokens WHERE user_id = ?1",
                params![user_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 端末管理一覧の 1 行（`GET /api/devices` 用・`token_hash` は current 判定にのみ使う）。
#[derive(Debug, Clone)]
pub struct DesktopTokenInfo {
    pub id: i64,
    pub device_name: Option<String>,
    pub token_hash: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// 本人の**未失効**デスクトップトークンを列挙する（Node `listDesktopTokensForUser`）。
/// 並びは `COALESCE(last_used_at, created_at) DESC`（最近使った端末が先頭）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`](yuuka_core::DbError)。
pub async fn list_for_user(
    db: &Db,
    user_id: &str,
) -> Result<Vec<DesktopTokenInfo>, yuuka_core::DbError> {
    let uid = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, device_name, token_hash, created_at, last_used_at \
                     FROM desktop_tokens WHERE user_id = ?1 AND revoked = 0 \
                     ORDER BY COALESCE(last_used_at, created_at) DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![uid], |row| {
                    Ok(DesktopTokenInfo {
                        id: row.get(0)?,
                        device_name: row.get(1)?,
                        token_hash: row.get(2)?,
                        created_at: row.get(3)?,
                        last_used_at: row.get(4)?,
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

/// 端末単位の失効（本人スコープ・**soft delete** `revoked = 1`・Node `revokeDesktopToken`）。
/// 失効できたら `true`（既に失効/他人/未存在は `false`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`](yuuka_core::DbError)。
pub async fn revoke(db: &Db, id: i64, user_id: &str) -> Result<bool, yuuka_core::DbError> {
    let uid = user_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE desktop_tokens SET revoked = 1 \
                     WHERE id = ?1 AND user_id = ?2 AND revoked = 0",
                    params![id, uid],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::verify;
    use crate::sha256_hex;
    use yuuka_types::Role;
    use yuuka_web::Db;

    fn seed_db() -> Db {
        let path = std::env::temp_dir().join(format!(
            "yuuka_auth_desktop_{}_{:?}.sqlite",
            std::process::id(),
            std::thread::current().id()
        ));
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

        let user = verify(&db, sha256_hex("tok-good"), 90)
            .await
            .unwrap()
            .expect("user");
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
        assert!(verify(&db, sha256_hex("tok-revoked"), 90)
            .await
            .unwrap()
            .is_none());
        assert!(verify(&db, sha256_hex("never-issued"), 90)
            .await
            .unwrap()
            .is_none());
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
        assert!(verify(&db, sha256_hex("tok-old"), 90)
            .await
            .unwrap()
            .is_none());
    }
}
