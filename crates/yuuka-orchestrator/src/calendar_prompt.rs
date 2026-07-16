//! 連携中の Google カレンダー一覧を systemInstruction へ注入するシーム（Node `gemini.ts:149-166`・P2-E-2）。
//!
//! Node `buildSystemInstruction`（秘書モード）は、連携中の Google カレンダー（名前/ID）一覧と
//! デフォルトカレンダー ID の案内を注入し、`addSchedule` の `calendar_id` 選択を促す。無いと予定登録が
//! デフォルトカレンダー実質固定になる（silent 後退）。カレンダー一覧は [`CalendarPort::cached_calendars`]
//! （A3・5min キャッシュ）から、デフォルト ID は primary アカウントの `calendar_id`（Node
//! `getResolvedCalendarId` = `resolveTargetCalendarId(primary account)`）から取る。guild/DM の
//! `buildGuildSystemInstruction` は Node でも非注入のため、本注入は**秘書ターンのみ**。

use std::sync::Arc;

use yuuka_google::CalendarPort;
use yuuka_web::Db;

/// 連携中カレンダー一覧セクションを返す（キャッシュが空なら `""`）。
///
/// Node `isCalendarEnabled` ゲートは `calendars.length > 0` に包含される（未連携＝キャッシュ空 →
/// `cached_calendars` が空 → 非注入）。デフォルト ID は primary アカウントの `calendar_id`（未設定は ""）。
pub async fn build_calendar_section(
    db: &Db,
    calendar: &Arc<dyn CalendarPort>,
    user_id: &str,
) -> String {
    let calendars = calendar.cached_calendars(user_id).await;
    if calendars.is_empty() {
        return String::new();
    }
    // デフォルトカレンダー ID = primary アカウントの calendar_id（Node `getResolvedCalendarId`）。
    // Rust の CalendarPort は user 単位（primary 基準）のため primary 固定。読み取り失敗/未設定は ""。
    let default_id = match yuuka_google::repo::get_primary_account(db, user_id).await {
        Ok(Some(acct)) => acct.calendar_id.unwrap_or_default(),
        _ => String::new(),
    };
    // Node `gemini.ts:157` 逐語（見出し + 案内は 1 行・改行は先頭と末尾のみ）。
    let mut section = String::from(
        "\n# 連携中のGoogleカレンダー一覧\n現在、予定を登録可能なカレンダーは以下の通りです。\
         ユーザーからの予定追加指示の際、その内容や目的に最も適したカレンダーの「ID」を選択し、\
         addSchedule関数の calendar_id 引数に指定して登録してください。\n",
    );
    for cal in &calendars {
        // Node `gemini.ts:159`。
        section.push_str(&format!(
            "- カレンダー名: \"{}\" (ID: \"{}\")\n",
            cal.summary, cal.id
        ));
    }
    // Node `gemini.ts:161`。
    section.push_str(&format!(
        "※もし内容や目的に合うカレンダーがない場合は、デフォルトのカレンダーIDである \"{default_id}\" を使用してください。\n"
    ));
    section
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use yuuka_google::{CalendarSummary, GoogleError};

    /// 一覧を固定で返すテスト用 CalendarPort。
    struct FakeCalendar(Vec<CalendarSummary>);

    #[async_trait::async_trait]
    impl CalendarPort for FakeCalendar {
        async fn cached_calendars(&self, _user_id: &str) -> Vec<CalendarSummary> {
            self.0.clone()
        }
        async fn list_for_account(
            &self,
            _user_id: &str,
            _account_id: i64,
        ) -> Result<Vec<CalendarSummary>, GoogleError> {
            Ok(Vec::new())
        }
        fn invalidate_user(&self, _user_id: &str) {}
        fn invalidate_account(&self, _account_id: i64) {}
    }

    fn cal(id: &str, summary: &str) -> CalendarSummary {
        CalendarSummary {
            id: id.to_owned(),
            summary: summary.to_owned(),
        }
    }

    async fn open_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("t.db");
        // P0-4: 空ファイルを先に作り Db::open の migrations で user_google_accounts を作らせる。
        drop(rusqlite::Connection::open(&path).expect("create empty"));
        let db = Db::open(Path::new(&path)).expect("open db");
        (db, dir)
    }

    async fn seed_user(db: &Db, uid: &str) {
        let uid = uid.to_owned();
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                     VALUES (?1, ?1, 'x', 'x')",
                    rusqlite::params![uid],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .expect("seed user");
    }

    #[tokio::test]
    async fn empty_cache_yields_empty_section() {
        let (db, _dir) = open_db().await;
        let calendar: Arc<dyn CalendarPort> = Arc::new(FakeCalendar(vec![]));
        assert_eq!(build_calendar_section(&db, &calendar, "u1").await, "");
    }

    #[tokio::test]
    async fn lists_calendars_with_empty_default_when_no_account() {
        let (db, _dir) = open_db().await;
        let calendar: Arc<dyn CalendarPort> = Arc::new(FakeCalendar(vec![
            cal("primary@x", "仕事"),
            cal("home@y", "プライベート"),
        ]));
        let section = build_calendar_section(&db, &calendar, "u1").await;
        assert_eq!(
            section,
            "\n# 連携中のGoogleカレンダー一覧\n現在、予定を登録可能なカレンダーは以下の通りです。\
             ユーザーからの予定追加指示の際、その内容や目的に最も適したカレンダーの「ID」を選択し、\
             addSchedule関数の calendar_id 引数に指定して登録してください。\n\
             - カレンダー名: \"仕事\" (ID: \"primary@x\")\n\
             - カレンダー名: \"プライベート\" (ID: \"home@y\")\n\
             ※もし内容や目的に合うカレンダーがない場合は、デフォルトのカレンダーIDである \"\" を使用してください。\n"
        );
    }

    #[tokio::test]
    async fn default_id_comes_from_primary_account_calendar_id() {
        let (db, _dir) = open_db().await;
        seed_user(&db, "u1").await;
        // primary アカウントに calendar_id を設定する。
        db.writer
            .transaction(|tx| {
                tx.execute(
                    "INSERT INTO user_google_accounts \
                     (user_id, calendar_id, is_primary, refresh_token_encrypted, refresh_token_iv, refresh_token_tag) \
                     VALUES ('u1', 'work@group.calendar.google.com', 1, 'x', 'x', 'x')",
                    [],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .expect("seed account");
        let calendar: Arc<dyn CalendarPort> = Arc::new(FakeCalendar(vec![cal(
            "work@group.calendar.google.com",
            "仕事",
        )]));
        let section = build_calendar_section(&db, &calendar, "u1").await;
        assert!(section.contains(
            "※もし内容や目的に合うカレンダーがない場合は、デフォルトのカレンダーIDである \"work@group.calendar.google.com\" を使用してください。\n"
        ));
    }
}
