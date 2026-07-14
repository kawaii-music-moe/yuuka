//! 誕生日リマインド（§3.11.2・現行 `birthdayReminderService.ts`）。
//!
//! 毎朝 8 時に「翌日が誕生日」の連絡先を検出し、登録ユーザーへ通知する。`birthday_reminded_year`
//! で同一年の重複通知を防止する。送信失敗はマークせず翌日 tick で再試行する。

use async_trait::async_trait;
use chrono::{Datelike, Duration, Local};
use yuuka_core::{BotId, UserId};
use yuuka_personal::repo::ContactRepo;

use crate::context::ServiceContext;
use crate::notifier::Notification;
use crate::schedule::{CronService, Schedule};

/// 誕生日リマインドサービス。
pub struct BirthdayReminderService;

#[async_trait]
impl CronService for BirthdayReminderService {
    fn name(&self) -> &'static str {
        "birthday"
    }

    fn schedule(&self) -> Schedule {
        Schedule::DailyAt { hour: 8, minute: 0 }
    }

    fn run_on_start(&self) -> bool {
        // 現行は cron.schedule のみ（起動直後には実行しない）。
        false
    }

    async fn tick(&self, ctx: &ServiceContext) {
        if let Err(e) = run(ctx).await {
            tracing::error!(error = %e, "[Birthday] 誕生日リマインド処理に失敗しました");
        }
    }
}

async fn run(ctx: &ServiceContext) -> Result<(), yuuka_core::DbError> {
    let now = Local::now();
    let tomorrow = now + Duration::days(1);
    let month_day = tomorrow.format("%m-%d").to_string();
    let current_year = i64::from(now.year());
    let tomorrow_year = tomorrow.year();

    let repo = ContactRepo::new(&ctx.db);
    let contacts = repo
        .list_birthday_for_date(ctx.cross, month_day.clone(), current_year)
        .await?;

    let display_day = month_day.replace('-', "/");
    for contact in contacts {
        let age_note = age_note(contact.birthday.as_deref(), tomorrow_year);
        let relationship = contact
            .relationship
            .as_deref()
            .filter(|r| !r.is_empty())
            .map(|r| format!("（{r}）"))
            .unwrap_or_default();

        let content = format!(
            "🎂 明日 ({display_day}) は **{}**さん{relationship}の誕生日です！{age_note}\nお祝いの準備はいかがですか？",
            contact.name
        );
        let notification = Notification::text(
            UserId::new(contact.user_id.clone()),
            BotId::new(contact.bot_id.clone()),
            content,
        );

        if ctx.notifier.send(notification).await {
            if let Err(e) = repo
                .mark_birthday_reminded(ctx.cross, contact.id, current_year)
                .await
            {
                tracing::error!(id = contact.id, error = %e, "[Birthday] 通知済みマークに失敗");
            } else {
                tracing::info!(user = %contact.user_id, name = %contact.name, "🎂 [Birthday] 誕生日リマインドを送信");
            }
        }
        // 送信失敗時はマークせず翌日 tick で再試行。
    }
    Ok(())
}

/// 年齢注記（`YYYY-MM-DD` で年が分かるときのみ「（N歳になります）」・現行ロジック）。
fn age_note(birthday: Option<&str>, tomorrow_year: i32) -> String {
    let Some(year) = birthday
        .filter(|b| b.as_bytes().get(4) == Some(&b'-'))
        .and_then(|b| b.get(0..4))
        .and_then(|y| y.parse::<i32>().ok())
    else {
        return String::new();
    };
    let age = tomorrow_year - year;
    if age > 0 && age < 130 {
        format!("（{age}歳になります）")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ctx_with, seeded_db, RecordingNotifier};
    use std::sync::Arc;
    use yuuka_db::map_sqlite;

    const CONTACTS_DDL: &str = "CREATE TABLE contacts (
        id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default', name TEXT NOT NULL, birthday TEXT,
        relationship TEXT, notes TEXT, tags TEXT NOT NULL DEFAULT '[]',
        birthday_reminded_year INTEGER,
        created_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
    );";

    #[test]
    fn age_note_only_with_full_year() {
        assert_eq!(age_note(Some("1990-07-10"), 2026), "（36歳になります）");
        assert_eq!(age_note(Some("07-10"), 2026), ""); // 年不明。
        assert_eq!(age_note(None, 2026), "");
        assert_eq!(age_note(Some("3000-01-01"), 2026), ""); // 負の年齢は付けない。
    }

    #[tokio::test]
    async fn tomorrow_birthday_notifies_and_marks() {
        let (db, _dir) = seeded_db(CONTACTS_DDL);
        // 「明日」の MM-DD を SQL 側で計算して birthday に埋める（実行日非依存）。
        db.writer
            .execute(|conn| {
                conn.execute(
                    "INSERT INTO contacts (user_id, bot_id, name, birthday, relationship) \
                     VALUES ('u','system_default','花子', \
                       '1990-' || strftime('%m-%d', 'now','localtime','+1 day'), '友人')",
                    [],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();

        let rec = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, rec.clone());
        run(&ctx).await.unwrap();

        assert_eq!(rec.count(), 1);
        assert!(rec.sent.lock().unwrap()[0].content.contains("花子"));

        // 通知済み年がマークされる。
        let year: Option<i64> = ctx
            .db
            .read
            .read(|conn| {
                conn.query_row(
                    "SELECT birthday_reminded_year FROM contacts WHERE id=1",
                    [],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)
            })
            .await
            .unwrap();
        assert!(year.is_some());
    }

    #[tokio::test]
    async fn already_reminded_this_year_is_skipped() {
        let (db, _dir) = seeded_db(CONTACTS_DDL);
        let this_year = Local::now().year();
        db.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO contacts (user_id, name, birthday, birthday_reminded_year) \
                     VALUES ('u','再送なし', \
                       '1990-' || strftime('%m-%d','now','localtime','+1 day'), ?1)",
                    rusqlite::params![this_year],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();

        let rec = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, rec.clone());
        run(&ctx).await.unwrap();
        assert_eq!(rec.count(), 0);
    }
}
