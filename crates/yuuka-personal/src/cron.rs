//! cron（誕生日リマインド）用の**全ユーザー横断走査**（§3.11.2・現行 contactRepo の
//! `listBirthdayContactsForDate` / `markBirthdayReminded`）。
//!
//! 通常の `UserScope` 経路から隔離するため [`CrossUserAccess`] 証憑を要求する。返す
//! [`BirthdayContact`] は通知に必要な内部列（`user_id`/`bot_id`）を含む。

use rusqlite::{Row, params};
use yuuka_core::{CrossUserAccess, DbError};
use yuuka_db::map_sqlite;

use crate::repo::ContactRepo;

/// 指定日が誕生日で当年未通知の連絡先（cron 専用・内部列を持つ）。
#[derive(Debug, Clone)]
pub struct BirthdayContact {
    pub id: i64,
    pub user_id: String,
    pub bot_id: String,
    pub name: String,
    pub birthday: Option<String>,
    pub relationship: Option<String>,
}

impl ContactRepo<'_> {
    /// 誕生日が `month_day`（`'MM-DD'`）で、当年（`current_year`）未通知の連絡先を
    /// **全ユーザー横断**で返す（現行 `listBirthdayContactsForDate`）。
    ///
    /// `substr(birthday, -5) = month_day`（`'YYYY-MM-DD'`/`'MM-DD'` 両対応）かつ
    /// `birthday_reminded_year IS NULL OR < current_year`。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_birthday_for_date(
        &self,
        _cron: CrossUserAccess,
        month_day: String,
        current_year: i64,
    ) -> Result<Vec<BirthdayContact>, DbError> {
        self.read
            .read(move |conn| {
                let sql = "SELECT id, user_id, bot_id, name, birthday, relationship FROM contacts \
                     WHERE birthday IS NOT NULL AND substr(birthday, -5) = ?1 \
                     AND (birthday_reminded_year IS NULL OR birthday_reminded_year < ?2)";
                let mut stmt = conn.prepare(sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![month_day, current_year], row_to_birthday)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// 連絡先を当年通知済みにする（重複通知防止・現行 `markBirthdayReminded`）。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn mark_birthday_reminded(
        &self,
        _cron: CrossUserAccess,
        id: i64,
        year: i64,
    ) -> Result<(), DbError> {
        self.writer
            .execute(move |conn| {
                conn.execute(
                    "UPDATE contacts SET birthday_reminded_year = ?1 WHERE id = ?2",
                    params![year, id],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }
}

fn row_to_birthday(row: &Row) -> rusqlite::Result<BirthdayContact> {
    Ok(BirthdayContact {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        bot_id: row.get("bot_id")?,
        name: row.get("name")?,
        birthday: row.get("birthday")?,
        relationship: row.get("relationship")?,
    })
}
