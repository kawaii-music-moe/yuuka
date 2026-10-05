//! admin データアクセス（`adminRoutes.ts` が呼ぶ Node repo 群のパリティ）。
//!
//! admin 操作は **user スコープを持たない**グローバル操作（全ユーザー・全 Bot を跨ぐ）ため、
//! ドメイン repo（`UserScope` 束縛）とは異なり直接クエリする。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（`BEGIN IMMEDIATE`）へ送る。返却ビューは秘密値を含まない列だけを SELECT する。

use rusqlite::params;
use yuuka_core::DbError;
use yuuka_crypto::Encrypted;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

use crate::dto::{AdminBotView, AdminStats, AdminUserView, AuditLogView, InviteCodeView};

/// システム全体の集計（Node `GET /api/admin/stats`）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn stats(db: &Db) -> Result<AdminStats, DbError> {
    db.read
        .read(move |conn| {
            let count = |sql: &str| -> Result<i64, rusqlite::Error> {
                conn.query_row(sql, [], |row| row.get::<_, i64>(0))
            };
            Ok(AdminStats {
                total_users: count("SELECT COUNT(*) FROM users").map_err(map_sqlite)?,
                total_bots: count("SELECT COUNT(*) FROM bots").map_err(map_sqlite)?,
                suspended_bots: count("SELECT COUNT(*) FROM bots WHERE suspended = 1")
                    .map_err(map_sqlite)?,
                total_invite_codes: count("SELECT COUNT(*) FROM invite_codes")
                    .map_err(map_sqlite)?,
                used_invite_codes: count(
                    "SELECT COUNT(*) FROM invite_codes WHERE used_by IS NOT NULL",
                )
                .map_err(map_sqlite)?,
                available_invite_codes: count(
                    "SELECT COUNT(*) FROM invite_codes WHERE used_by IS NULL AND revoked_at IS NULL",
                )
                .map_err(map_sqlite)?,
            })
        })
        .await
}

/// `system_settings` の 1 値を読む（未登録は `None`・Node `getSystemSetting`）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn get_system_setting(db: &Db, key: &str) -> Result<Option<String>, DbError> {
    let key = key.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT value FROM system_settings WHERE key = ?1")
                .map_err(map_sqlite)?;
            let mut rows = stmt
                .query_map(params![key], |row| row.get::<_, String>(0))
                .map_err(map_sqlite)?;
            match rows.next() {
                Some(v) => Ok(Some(v.map_err(map_sqlite)?)),
                None => Ok(None),
            }
        })
        .await
}

/// `system_settings` を upsert する（Node `setSystemSetting`＝`INSERT OR REPLACE`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_system_setting(db: &Db, key: &str, value: &str) -> Result<(), DbError> {
    let key = key.to_owned();
    let value = value.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO system_settings (key, value, updated_at) \
                 VALUES (?1, ?2, datetime('now', 'localtime'))",
                params![key, value],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 全ユーザーを作成日時昇順で返す（Node `listAllUsers`・秘密値なし）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn list_all_users(db: &Db) -> Result<Vec<AdminUserView>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT discord_id, username, role, created_at, updated_at \
                     FROM users ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(AdminUserView {
                        discord_id: row.get(0)?,
                        username: row.get(1)?,
                        role: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
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

/// ユーザーのロールを変更する（Node `updateUserRole`・`role` は `'user'`/`'admin'` のみ）。
/// 変更行があれば `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_user_role(db: &Db, discord_id: &str, role: &str) -> Result<bool, DbError> {
    if role != "user" && role != "admin" {
        return Ok(false);
    }
    let discord_id = discord_id.to_owned();
    let role = role.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE users SET role = ?1, updated_at = datetime('now', 'localtime') \
                     WHERE discord_id = ?2",
                    params![role, discord_id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// ユーザーを削除する（Node `deleteUser`・関連データは FK ON DELETE CASCADE）。削除できれば `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn delete_user(db: &Db, discord_id: &str) -> Result<bool, DbError> {
    let discord_id = discord_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "DELETE FROM users WHERE discord_id = ?1",
                    params![discord_id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 監査ログをページングして返す（Node `listAuditLogs`・action は前方一致・id 降順）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn list_audit_logs(
    db: &Db,
    limit: i64,
    action: Option<&str>,
    offset: i64,
) -> Result<Vec<AuditLogView>, DbError> {
    let action = action.map(str::to_owned);
    db.read
        .read(move |conn| {
            let map_row = |row: &rusqlite::Row<'_>| -> Result<AuditLogView, rusqlite::Error> {
                Ok(AuditLogView {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    action: row.get(2)?,
                    target: row.get(3)?,
                    detail: row.get(4)?,
                    created_at: row.get(5)?,
                })
            };
            const COLS: &str = "id, user_id, action, target, detail, created_at";
            let mut out = Vec::new();
            match action {
                Some(prefix) => {
                    let like = format!("{prefix}%");
                    let sql = format!(
                        "SELECT {COLS} FROM audit_logs WHERE action LIKE ?1 \
                         ORDER BY id DESC LIMIT ?2 OFFSET ?3"
                    );
                    let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                    let rows = stmt
                        .query_map(params![like, limit, offset], map_row)
                        .map_err(map_sqlite)?;
                    for row in rows {
                        out.push(row.map_err(map_sqlite)?);
                    }
                }
                None => {
                    let sql = format!(
                        "SELECT {COLS} FROM audit_logs ORDER BY id DESC LIMIT ?1 OFFSET ?2"
                    );
                    let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                    let rows = stmt
                        .query_map(params![limit, offset], map_row)
                        .map_err(map_sqlite)?;
                    for row in rows {
                        out.push(row.map_err(map_sqlite)?);
                    }
                }
            }
            Ok(out)
        })
        .await
}

/// 監査ログの総件数（Node `countAuditLogs`・action は前方一致）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn count_audit_logs(db: &Db, action: Option<&str>) -> Result<i64, DbError> {
    let action = action.map(str::to_owned);
    db.read
        .read(move |conn| match action {
            Some(prefix) => {
                let like = format!("{prefix}%");
                conn.query_row(
                    "SELECT COUNT(*) FROM audit_logs WHERE action LIKE ?1",
                    params![like],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(map_sqlite)
            }
            None => conn
                .query_row("SELECT COUNT(*) FROM audit_logs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(map_sqlite),
        })
        .await
}

/// 全 Bot をオーナー名付きで返す（Node `GET /api/admin/bots`）。`is_running` は呼び出し側で埋める
/// （稼働状態は DB 外の runtime 情報のため）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn list_all_bots(db: &Db) -> Result<Vec<AdminBotView>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT b.id, b.name, b.user_id, \
                            COALESCE(u.username, '不明') AS owner_username, \
                            b.discord_username, b.discord_avatar_url, b.suspended, \
                            (b.discord_token_encrypted IS NOT NULL) AS has_custom_token, \
                            b.created_at, b.updated_at \
                     FROM bots b LEFT JOIN users u ON u.discord_id = b.user_id \
                     ORDER BY b.created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(AdminBotView {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        user_id: row.get(2)?,
                        owner_username: row.get(3)?,
                        discord_username: row.get(4)?,
                        discord_avatar_url: row.get(5)?,
                        suspended: row.get(6)?,
                        has_custom_token: row.get(7)?,
                        // runtime 情報。既定 false・呼び出し側が BotRuntime で上書きする。
                        is_running: false,
                        created_at: row.get(8)?,
                        updated_at: row.get(9)?,
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

/// `system_default` を除く、指定オーナーの Bot ID 一覧（Node のユーザー削除時 Bot 停止ループ用）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn bot_ids_owned_by(db: &Db, user_id: &str) -> Result<Vec<String>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM bots WHERE user_id = ?1 AND id != 'system_default'")
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id], |row| row.get::<_, String>(0))
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// Bot を停止処分にする（Node `suspendBot`）。変更行があれば `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn suspend_bot(db: &Db, bot_id: &str) -> Result<bool, DbError> {
    set_suspended(db, bot_id, 1).await
}

/// Bot の停止処分を解除する（Node `unsuspendBot`）。変更行があれば `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn unsuspend_bot(db: &Db, bot_id: &str) -> Result<bool, DbError> {
    set_suspended(db, bot_id, 0).await
}

async fn set_suspended(db: &Db, bot_id: &str, suspended: i64) -> Result<bool, DbError> {
    let bot_id = bot_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE bots SET suspended = ?1 WHERE id = ?2",
                    params![suspended, bot_id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// システムデフォルト Bot のトークン（暗号化済み）を upsert する（Node `POST /api/admin/default-bot/token`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn upsert_default_bot_token(
    db: &Db,
    owner_id: &str,
    enc: &Encrypted,
) -> Result<(), DbError> {
    let owner_id = owner_id.to_owned();
    let encrypted = enc.encrypted.clone();
    let iv = enc.iv.clone();
    let tag = enc.auth_tag.clone();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO bots \
                   (id, user_id, name, discord_token_encrypted, discord_token_iv, \
                    discord_token_tag, suspended) \
                 VALUES ('system_default', ?1, 'システムデフォルト', ?2, ?3, ?4, 0) \
                 ON CONFLICT(id) DO UPDATE SET \
                   discord_token_encrypted = excluded.discord_token_encrypted, \
                   discord_token_iv = excluded.discord_token_iv, \
                   discord_token_tag = excluded.discord_token_tag, \
                   suspended = 0, \
                   updated_at = datetime('now', 'localtime')",
                params![owner_id, encrypted, iv, tag],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 招待コード一覧を作成日時降順で返す（Node `listInviteCodes`＝`SELECT *`）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn list_invite_codes(db: &Db) -> Result<Vec<InviteCodeView>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT code, created_by, used_by, used_at, revoked_at, created_at \
                     FROM invite_codes ORDER BY created_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(InviteCodeView {
                        code: row.get(0)?,
                        created_by: row.get(1)?,
                        used_by: row.get(2)?,
                        used_at: row.get(3)?,
                        revoked_at: row.get(4)?,
                        created_at: row.get(5)?,
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

/// 招待コードを作成する（Node `createInviteCode`＝`INSERT OR IGNORE`・重複は無視）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn create_invite_code(db: &Db, code: &str, created_by: &str) -> Result<(), DbError> {
    let code = code.to_owned();
    let created_by = created_by.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT OR IGNORE INTO invite_codes (code, created_by) VALUES (?1, ?2)",
                params![code, created_by],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 未使用の招待コードを無効化する（Node `revokeInviteCode`・記録は残す）。成功時 `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn revoke_invite_code(db: &Db, code: &str) -> Result<bool, DbError> {
    let code = code.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE invite_codes SET revoked_at = datetime('now', 'localtime') \
                     WHERE code = ?1 AND used_by IS NULL AND revoked_at IS NULL",
                    params![code],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 未使用の招待コードを物理削除する（Node `deleteInviteCode`・使用済みは削除不可）。成功時 `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn delete_invite_code(db: &Db, code: &str) -> Result<bool, DbError> {
    let code = code.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "DELETE FROM invite_codes WHERE code = ?1 AND used_by IS NULL",
                    params![code],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use yuuka_crypto::Encrypted;
    use yuuka_web::Db;

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// `Db::open`（= run_migrations）が V17 baseline を丸ごと適用し、admin が触る全表
    /// （users/bots/invite_codes/audit_logs/system_settings）を作る。返り値はハンドルとパス。
    fn fresh_db() -> (Db, PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_admin_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        // Rust は存在しない DB を作らない（P0-4）。raw 接続で空ファイルを作ってから migrate する。
        drop(Connection::open(&path).expect("create db file"));
        let db = Db::open(&path).expect("open db");
        (db, path)
    }

    fn raw(path: &PathBuf) -> Connection {
        Connection::open(path).expect("raw conn")
    }

    fn seed_user(path: &PathBuf, discord_id: &str, role: &str) {
        raw(path)
            .execute(
                "INSERT INTO users (discord_id, username, password_hash, salt, role) \
                 VALUES (?1, ?1, 'x', '00', ?2)",
                params![discord_id, role],
            )
            .expect("seed user");
    }

    fn seed_bot(path: &PathBuf, id: &str, user_id: &str, suspended: i64, has_token: bool) {
        let token = has_token.then_some("deadbeef");
        raw(path)
            .execute(
                "INSERT INTO bots (id, user_id, name, discord_token_encrypted, suspended) \
                 VALUES (?1, ?2, ?1, ?3, ?4)",
                params![id, user_id, token, suspended],
            )
            .expect("seed bot");
    }

    #[tokio::test]
    async fn stats_counts_users_bots_invites() {
        let (db, path) = fresh_db();
        seed_user(&path, "owner", "admin");
        seed_bot(&path, "system_default", "owner", 0, true);
        seed_bot(&path, "b1", "owner", 1, false);
        create_invite_code(&db, "FREE", "owner").await.unwrap();
        create_invite_code(&db, "USED", "owner").await.unwrap();
        // USED を消費済みにする。
        raw(&path)
            .execute(
                "UPDATE invite_codes SET used_by = 'someone' WHERE code = 'USED'",
                [],
            )
            .unwrap();

        let s = stats(&db).await.unwrap();
        assert_eq!(s.total_users, 1);
        assert_eq!(s.total_bots, 2);
        assert_eq!(s.suspended_bots, 1);
        assert_eq!(s.total_invite_codes, 2);
        assert_eq!(s.used_invite_codes, 1);
        assert_eq!(s.available_invite_codes, 1);
    }

    #[tokio::test]
    async fn invite_code_lifecycle() {
        let (db, _path) = fresh_db();
        create_invite_code(&db, "ALPHA", "admin").await.unwrap();
        // 重複作成は無視（INSERT OR IGNORE）— 件数は増えない。
        create_invite_code(&db, "ALPHA", "admin").await.unwrap();
        let codes = list_invite_codes(&db).await.unwrap();
        assert_eq!(codes.len(), 1);
        assert_eq!(codes[0].code, "ALPHA");
        assert!(codes[0].revoked_at.is_none());

        // 無効化 → revoked_at が付く。再無効化は false（既に revoked）。
        assert!(revoke_invite_code(&db, "ALPHA").await.unwrap());
        assert!(!revoke_invite_code(&db, "ALPHA").await.unwrap());
        // revoked でも未使用なら削除可（DELETE は used_by IS NULL のみ条件）。
        assert!(delete_invite_code(&db, "ALPHA").await.unwrap());
        assert!(list_invite_codes(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn used_invite_cannot_be_revoked_or_deleted() {
        let (db, path) = fresh_db();
        create_invite_code(&db, "TAKEN", "admin").await.unwrap();
        raw(&path)
            .execute(
                "UPDATE invite_codes SET used_by = 'u' WHERE code = 'TAKEN'",
                [],
            )
            .unwrap();
        assert!(!revoke_invite_code(&db, "TAKEN").await.unwrap());
        assert!(!delete_invite_code(&db, "TAKEN").await.unwrap());
        // 記録は残る。
        assert_eq!(list_invite_codes(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn user_role_and_delete() {
        let (db, path) = fresh_db();
        seed_user(&path, "alice", "user");
        let users = list_all_users(&db).await.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].role, "user");

        assert!(update_user_role(&db, "alice", "admin").await.unwrap());
        assert_eq!(list_all_users(&db).await.unwrap()[0].role, "admin");
        // 不正な role は false（更新しない）。
        assert!(!update_user_role(&db, "alice", "superuser").await.unwrap());
        // 不在ユーザーは false。
        assert!(!update_user_role(&db, "ghost", "admin").await.unwrap());

        assert!(delete_user(&db, "alice").await.unwrap());
        assert!(!delete_user(&db, "alice").await.unwrap());
        assert!(list_all_users(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn system_settings_roundtrip() {
        let (db, _path) = fresh_db();
        assert!(get_system_setting(&db, "privacy_policy_url")
            .await
            .unwrap()
            .is_none());
        set_system_setting(&db, "privacy_policy_url", "https://example.com/p")
            .await
            .unwrap();
        assert_eq!(
            get_system_setting(&db, "privacy_policy_url").await.unwrap(),
            Some("https://example.com/p".to_owned())
        );
        // upsert（INSERT OR REPLACE）で上書きできる。
        set_system_setting(&db, "privacy_policy_url", "/relative")
            .await
            .unwrap();
        assert_eq!(
            get_system_setting(&db, "privacy_policy_url").await.unwrap(),
            Some("/relative".to_owned())
        );
    }

    #[tokio::test]
    async fn audit_logs_filter_and_paginate() {
        let (db, path) = fresh_db();
        seed_user(&path, "admin1", "admin");
        // add_audit_log（yuuka-auth）で 3 件書く（action 前方一致の検証用）。
        yuuka_auth::audit::add_audit_log(&db, "admin1", "admin.role_change", Some("t"), None).await;
        yuuka_auth::audit::add_audit_log(&db, "admin1", "admin.user_delete", Some("t"), None).await;
        yuuka_auth::audit::add_audit_log(&db, "admin1", "auth.login", None, None).await;

        assert_eq!(count_audit_logs(&db, None).await.unwrap(), 3);
        assert_eq!(count_audit_logs(&db, Some("admin.")).await.unwrap(), 2);

        let all = list_audit_logs(&db, 200, None, 0).await.unwrap();
        assert_eq!(all.len(), 3);
        // id 降順（最新が先頭）。
        assert_eq!(all[0].action, "auth.login");

        let admins = list_audit_logs(&db, 200, Some("admin."), 0).await.unwrap();
        assert_eq!(admins.len(), 2);
        assert!(admins.iter().all(|r| r.action.starts_with("admin.")));

        // OFFSET/LIMIT のページング。
        let page = list_audit_logs(&db, 1, None, 1).await.unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].action, "admin.user_delete");
    }

    #[tokio::test]
    async fn bot_suspend_unsuspend_and_owned_ids() {
        let (db, path) = fresh_db();
        seed_user(&path, "owner", "user");
        seed_user(&path, "other", "user");
        seed_bot(&path, "system_default", "owner", 0, true);
        seed_bot(&path, "b1", "owner", 0, false);
        seed_bot(&path, "b2", "other", 0, false);

        assert!(suspend_bot(&db, "b1").await.unwrap());
        let bots = list_all_bots(&db).await.unwrap();
        let b1 = bots.iter().find(|b| b.id == "b1").unwrap();
        assert_eq!(b1.suspended, 1);
        assert_eq!(b1.owner_username, "owner");
        assert!(!b1.has_custom_token);
        assert!(!b1.is_running); // repo は既定 false（runtime はハンドラで埋める）。
        let sysdef = bots.iter().find(|b| b.id == "system_default").unwrap();
        assert!(sysdef.has_custom_token);

        assert!(unsuspend_bot(&db, "b1").await.unwrap());
        assert!(!suspend_bot(&db, "ghost").await.unwrap());

        // owner の Bot は system_default を除いて b1 のみ。
        let owned = bot_ids_owned_by(&db, "owner").await.unwrap();
        assert_eq!(owned, vec!["b1".to_owned()]);
    }

    #[tokio::test]
    async fn missing_owner_falls_back_to_unknown() {
        let (db, path) = fresh_db();
        // オーナー行を作らずに Bot を入れる（FK OFF + INSERT を同一接続で実行）。
        let conn = raw(&path);
        conn.execute("PRAGMA foreign_keys = OFF", []).unwrap();
        conn.execute(
            "INSERT INTO bots (id, user_id, name, suspended) VALUES ('orphan', 'nonexistent', 'orphan', 0)",
            [],
        )
        .unwrap();
        let bots = list_all_bots(&db).await.unwrap();
        let orphan = bots.iter().find(|b| b.id == "orphan").unwrap();
        assert_eq!(orphan.owner_username, "不明");
    }

    #[tokio::test]
    async fn default_bot_token_upsert() {
        let (db, path) = fresh_db();
        seed_user(&path, "admin1", "admin");
        let enc = Encrypted {
            encrypted: "aa".to_owned(),
            iv: "bb".to_owned(),
            auth_tag: "cc".to_owned(),
        };
        upsert_default_bot_token(&db, "admin1", &enc).await.unwrap();
        let bots = list_all_bots(&db).await.unwrap();
        let sysdef = bots.iter().find(|b| b.id == "system_default").unwrap();
        assert!(sysdef.has_custom_token);
        assert_eq!(sysdef.name, "システムデフォルト");
        assert_eq!(sysdef.user_id, "admin1");

        // 再 upsert（トークン更新）は suspended=0 に戻し、行は 1 件のまま。
        raw(&path)
            .execute(
                "UPDATE bots SET suspended = 1 WHERE id = 'system_default'",
                [],
            )
            .unwrap();
        let enc2 = Encrypted {
            encrypted: "dd".to_owned(),
            iv: "ee".to_owned(),
            auth_tag: "ff".to_owned(),
        };
        upsert_default_bot_token(&db, "admin1", &enc2)
            .await
            .unwrap();
        let bots = list_all_bots(&db).await.unwrap();
        let count = bots.iter().filter(|b| b.id == "system_default").count();
        assert_eq!(count, 1);
        let sysdef = bots.iter().find(|b| b.id == "system_default").unwrap();
        assert_eq!(sysdef.suspended, 0);
    }
}
