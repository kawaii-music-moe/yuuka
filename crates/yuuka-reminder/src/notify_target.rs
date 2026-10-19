//! ユーザー既定の通知先（`users.notify_target_type` / `notify_target_id`・現行 `userRepo.getUserNotifyTarget`）。
//!
//! 送信先を省略したリマインド登録（tool / HTTP・[`crate::repo::ReminderRepo::add`]）と、送信先を
//! 持たない常駐通知（ToDo 期限・予定・誕生日・マクロ定期実行など・yuuka-services）が同じ規則で
//! 既定送信先を解決するための共通読み出し。

use rusqlite::{params, Connection, OptionalExtension};
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// ユーザー設定の既定送信先。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserNotifyTarget {
    /// DM（ユーザー行が無い・チャンネル ID 未設定の場合も含む）。
    Dm,
    /// 既定のチャンネル ID。
    Channel(String),
}

/// 既定送信先を同じ接続（トランザクション内を含む）で読む。
///
/// `channel` かつ ID が空でなければチャンネル、それ以外は DM（現行 `getUserNotifyTarget` と同じ）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub(crate) fn query(conn: &Connection, user_id: &str) -> Result<UserNotifyTarget, DbError> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT notify_target_type, notify_target_id FROM users WHERE discord_id = ?1",
            params![user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite)?;
    Ok(match row {
        Some((kind, Some(id))) if kind == "channel" && !id.is_empty() => {
            UserNotifyTarget::Channel(id)
        }
        _ => UserNotifyTarget::Dm,
    })
}

/// ユーザーの既定送信先を返す（読みプール経由）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn user_notify_target(db: &Db, user_id: &str) -> Result<UserNotifyTarget, DbError> {
    let uid = user_id.to_owned();
    db.read.read(move |conn| query(conn, &uid)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (
                discord_id TEXT PRIMARY KEY,
                notify_target_type TEXT NOT NULL DEFAULT 'dm',
                notify_target_id TEXT
            );
            INSERT INTO users VALUES ('dm_user', 'dm', NULL);
            INSERT INTO users VALUES ('chan_user', 'channel', '111');
            INSERT INTO users VALUES ('chan_no_id', 'channel', NULL);
            INSERT INTO users VALUES ('chan_empty', 'channel', '');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn channel_with_id_is_channel() {
        assert_eq!(
            query(&conn(), "chan_user").unwrap(),
            UserNotifyTarget::Channel("111".to_owned())
        );
    }

    #[test]
    fn dm_missing_user_and_channel_without_id_fall_back_to_dm() {
        let conn = conn();
        for user in ["dm_user", "chan_no_id", "chan_empty", "nobody"] {
            assert_eq!(query(&conn, user).unwrap(), UserNotifyTarget::Dm, "{user}");
        }
    }
}
