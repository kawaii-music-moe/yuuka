//! `users` テーブルの認証向け CRUD（Node `src/db/userRepo.ts` パリティ）。
//!
//! 本人認証は sk-account だけが担い、Yuuka は認証情報（パスワード等）を一切持たない。
//! `users.salt` は Argon2id ユーザー鍵導出用の **16 バイト CSPRNG hex**
//! （[`yuuka_crypto::generate_user_salt`]）を生成時に一度だけ設定し、以後変更しない。

use rusqlite::{params, OptionalExtension};
use yuuka_core::DbError;
use yuuka_crypto::generate_user_salt;
use yuuka_db::map_sqlite;
use yuuka_types::Role;
use yuuka_web::Db;

/// 認証に必要な最小のユーザー行。
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// Discord ID（PK）。
    pub discord_id: String,
    /// 表示名。
    pub username: String,
    /// 権限ロール（生文字列 `'user'`/`'admin'`）。
    pub role: String,
}

/// `users.role`（`'user'`/`'admin'`）を [`Role`] へ（未知は `User`）。
#[must_use]
pub fn role_from_str(raw: &str) -> Role {
    if raw.eq_ignore_ascii_case("admin") {
        Role::Admin
    } else {
        Role::User
    }
}

/// [`Role`] を `users.role` 文字列へ（新規作成の INSERT 用）。
fn role_text(role: Role) -> &'static str {
    match role {
        Role::Admin => "admin",
        Role::User => "user",
    }
}

/// Discord ID でユーザーを引く（Node `getUserByDiscordId`）。不在は `Ok(None)`。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_user_by_discord_id(
    db: &Db,
    discord_id: &str,
) -> Result<Option<AuthUser>, DbError> {
    let id = discord_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT discord_id, username, role FROM users WHERE discord_id = ?1",
                params![id],
                |r| {
                    Ok(AuthUser {
                        discord_id: r.get(0)?,
                        username: r.get(1)?,
                        role: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// sk-account の確定済みIDを Yuuka のローカル権限・データ所有者へ結び付ける。
///
/// 利用者は sk-account ID で引き当て、Discord ID との組み合わせも一致する場合だけプロフィール名を
/// 同期する。sk-account に未紐付けのデータ行（旧環境から移した行）は、sk-account が確認した Discord ID
/// が一致すれば一度だけ引き受ける。新規利用者は、最初の利用者または `admin_ids` に含まれる
/// Discord ID だけを admin にする。
/// COUNT→INSERT は単一 writer トランザクション内で行い、初回 admin 判定の競合を閉じる。
///
/// # Errors
/// sk-account ID と Discord ID の組み合わせ不一致・一意制約違反・salt/DB 失敗時 [`DbError`]。
pub async fn upsert_from_sk_account(
    db: &Db,
    sk_account_id: &str,
    discord_id: &str,
    username: &str,
    admin_ids: &[String],
) -> Result<Role, DbError> {
    let salt = generate_user_salt().map_err(|e| DbError::Operation(format!("salt 生成: {e}")))?;
    let sk_account_id = sk_account_id.to_owned();
    let discord_id = discord_id.to_owned();
    let username = username.to_owned();
    let is_admin_id = admin_ids.iter().any(|id| id == &discord_id);
    db.writer
        .transaction(move |tx| {
            let linked: Option<(String, String)> = tx
                .query_row(
                    "SELECT discord_id, role FROM users WHERE sk_account_id = ?1",
                    params![sk_account_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            if let Some((linked_discord, role)) = linked {
                if linked_discord != discord_id {
                    return Err(DbError::Operation(
                        "sk-account ID の所有者が一致しません".to_owned(),
                    ));
                }
                tx.execute(
                    "UPDATE users SET username = ?1, updated_at = datetime('now','localtime') WHERE discord_id = ?2",
                    params![username, discord_id],
                )
                .map_err(map_sqlite)?;
                return Ok(role_from_str(&role));
            }

            let existing: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT role, sk_account_id FROM users WHERE discord_id = ?1",
                    params![discord_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            if let Some((role, other_link)) = existing {
                // 同じ Discord ID が別の sk-account へ結び付いていれば乗っ取りとして拒否する。
                if other_link.is_some_and(|other| other != sk_account_id) {
                    return Err(DbError::Operation(
                        "この Discord ID は別の sk-account に結び付いています".to_owned(),
                    ));
                }
                tx.execute(
                    "UPDATE users SET sk_account_id = ?1, username = ?2, updated_at = datetime('now','localtime') WHERE discord_id = ?3",
                    params![sk_account_id, username, discord_id],
                )
                .map_err(map_sqlite)?;
                return Ok(role_from_str(&role));
            }

            let count: i64 = tx
                .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
                .map_err(map_sqlite)?;
            let role = if count == 0 || is_admin_id {
                Role::Admin
            } else {
                Role::User
            };
            tx.execute(
                "INSERT INTO users (discord_id, username, salt, role, sk_account_id) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![discord_id, username, salt, role_text(role), sk_account_id],
            )
            .map_err(map_sqlite)?;
            Ok(role)
        })
        .await
}
