//! エージェントが予定を書き込む Google カレンダーの一覧を systemInstruction へ注入する（Node
//! `gemini.ts:149-166` 相当）。
//!
//! エージェントに Google アカウントが連携されていれば（[`CalendarEventsPort::account_for`]）、そのアカウントの
//! 書き込み可能なカレンダーと既定のカレンダー ID を示し、`addSchedule` の `calendar_id` 選択を促す。
//! 連携していなければ何も注入しない（予定は Yuuka 内にだけ保存される）。

use std::sync::Arc;

use yuuka_google::CalendarEventsPort;

/// 連携中カレンダー一覧セクションを返す（未連携・一覧が取れなければ `""`）。
pub async fn build_calendar_section(
    calendar: &Arc<dyn CalendarEventsPort>,
    user_id: &str,
    bot_id: &str,
) -> String {
    let Some(account) = calendar.account_for(user_id, bot_id).await else {
        return String::new();
    };
    let calendars = calendar
        .calendars(account.account_id)
        .await
        .unwrap_or_default();
    if calendars.is_empty() {
        return String::new();
    }
    let mut section = String::from(
        "\n# 連携中のGoogleカレンダー一覧\n現在、予定を登録可能なカレンダーは以下の通りです。\
         ユーザーからの予定追加指示の際、その内容や目的に最も適したカレンダーの「ID」を選択し、\
         addSchedule関数の calendar_id 引数に指定して登録してください。\n",
    );
    for cal in &calendars {
        section.push_str(&format!(
            "- カレンダー名: \"{}\" (ID: \"{}\")\n",
            cal.summary, cal.id
        ));
    }
    section.push_str(&format!(
        "※もし内容や目的に合うカレンダーがない場合は、デフォルトのカレンダーIDである \"{}\" を使用してください。\n",
        account.default_calendar_id
    ));
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use yuuka_google::{
        CalendarSummary, GoogleError, GoogleEvent, GoogleEventInput, LinkedGoogleAccount,
    };

    /// 連携の有無とカレンダー一覧を固定で返すテスト用ポート。
    struct FakeCalendar {
        linked: bool,
        calendars: Vec<CalendarSummary>,
    }

    #[async_trait]
    impl CalendarEventsPort for FakeCalendar {
        async fn account_for(&self, _user_id: &str, _bot_id: &str) -> Option<LinkedGoogleAccount> {
            self.linked.then(|| LinkedGoogleAccount {
                account_id: 1,
                default_calendar_id: "work@group.calendar.google.com".to_owned(),
            })
        }
        async fn import_account_for(&self, _user_id: &str, _bot_id: &str) -> Option<i64> {
            None
        }
        async fn calendars(&self, _account_id: i64) -> Result<Vec<CalendarSummary>, GoogleError> {
            Ok(self.calendars.clone())
        }
        async fn insert_event(
            &self,
            _account_id: i64,
            _calendar_id: &str,
            _event: &GoogleEventInput,
        ) -> Result<String, GoogleError> {
            Err(GoogleError::NotConfigured)
        }
        async fn delete_event(
            &self,
            _account_id: i64,
            _calendar_id: &str,
            _event_id: &str,
        ) -> Result<(), GoogleError> {
            Err(GoogleError::NotConfigured)
        }
        async fn list_events(
            &self,
            _account_id: i64,
            _calendar_id: &str,
            _from_local: &str,
            _to_local: &str,
        ) -> Result<Vec<GoogleEvent>, GoogleError> {
            Err(GoogleError::NotConfigured)
        }
    }

    fn cal(id: &str, summary: &str) -> CalendarSummary {
        CalendarSummary {
            id: id.to_owned(),
            summary: summary.to_owned(),
        }
    }

    #[tokio::test]
    async fn unlinked_agent_yields_empty_section() {
        let calendar: Arc<dyn CalendarEventsPort> = Arc::new(FakeCalendar {
            linked: false,
            calendars: vec![cal("primary@x", "仕事")],
        });
        assert_eq!(build_calendar_section(&calendar, "u1", "bot").await, "");
    }

    #[tokio::test]
    async fn linked_agent_lists_calendars_and_default_id() {
        let calendar: Arc<dyn CalendarEventsPort> = Arc::new(FakeCalendar {
            linked: true,
            calendars: vec![cal("primary@x", "仕事"), cal("home@y", "プライベート")],
        });
        let section = build_calendar_section(&calendar, "u1", "bot").await;
        assert!(section.contains("- カレンダー名: \"仕事\" (ID: \"primary@x\")\n"));
        assert!(section.contains("- カレンダー名: \"プライベート\" (ID: \"home@y\")\n"));
        assert!(section.contains("addSchedule関数の calendar_id 引数"));
        assert!(section.contains(
            "デフォルトのカレンダーIDである \"work@group.calendar.google.com\" を使用してください。"
        ));
    }
}
