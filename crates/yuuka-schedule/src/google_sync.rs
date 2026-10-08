//! Google カレンダー → Yuuka の予定の取り込み（旧 Node `syncGoogleCalendarToLocal` の移植）。
//!
//! 取り込みはアカウントを明示的に割り当てたエージェントだけが対象（呼び出し側が
//! [`CalendarEventsPort::import_account_for`] で判定する）。期間内の Google の予定を
//! アカウントの全カレンダーから読み、Yuuka 側を次のように合わせる:
//!
//! - 紐付け済みの予定 → 内容が変わっていれば更新。
//! - 未紐付け → 同じタイトル・開始日時の未紐付けの予定があれば紐付け、無ければ新規作成。
//! - 紐付け済みなのに Google 側から消えた予定 → Yuuka からも削除。
//!
//! 削除の判定は「読み込みに成功したカレンダー」かつ「同じ期間」の予定に限る（旧版は期間外の
//! 未来の予定や読み込みに失敗したカレンダーの予定まで消しうる作りだった）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use yuuka_core::{DbError, UserScope};
use yuuka_google::{CalendarEventsPort, GoogleError};
use yuuka_web::Db;

use crate::repo::{GoogleScheduleFields, ScheduleRepo};

/// 取り込みの既定期間（今日の 1 日前 〜 30 日後・旧版と同じ）。
pub const DEFAULT_SYNC_DAYS: i64 = 30;

/// 取り込みの結果（ログ用）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullSummary {
    pub created: usize,
    pub updated: usize,
    pub linked: usize,
    pub deleted: usize,
}

/// 取り込みの失敗。
#[derive(Debug)]
pub enum PullError {
    Google(GoogleError),
    Db(DbError),
}

impl std::fmt::Display for PullError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Google(e) => write!(f, "google: {e:?}"),
            Self::Db(e) => write!(f, "db: {e}"),
        }
    }
}

impl From<DbError> for PullError {
    fn from(e: DbError) -> Self {
        Self::Db(e)
    }
}

/// `scope`（ユーザー × エージェント）へ、アカウント `account_id` の Google の予定を取り込む。
/// `now_local` は現在のローカル日時（期間の基準）。
///
/// # Errors
/// カレンダー一覧を取得できない・DB 失敗時 [`PullError`]。個々のカレンダーの読み込み失敗は
/// そのカレンダーをスキップする（削除判定からも外す）。
pub async fn pull_from_google(
    db: &Db,
    port: &Arc<dyn CalendarEventsPort>,
    scope: &UserScope,
    account_id: i64,
    now_local: chrono::NaiveDateTime,
    days: i64,
) -> Result<PullSummary, PullError> {
    let fmt = |dt: chrono::NaiveDateTime| dt.format("%Y-%m-%d %H:%M:%S").to_string();
    let from = fmt(now_local - chrono::Duration::days(1));
    let to = fmt(now_local + chrono::Duration::days(days));

    let calendars = port
        .calendars(account_id)
        .await
        .map_err(PullError::Google)?;
    let repo = ScheduleRepo::new(db);
    let mut local: HashMap<String, _> = repo
        .google_linked_between(scope, &from, &to)
        .await?
        .into_iter()
        .map(|s| (s.google_event_id.clone(), s))
        .collect();

    let mut summary = PullSummary::default();
    let mut fetched_calendars = HashSet::new();
    for calendar in &calendars {
        let events = match port.list_events(account_id, &calendar.id, &from, &to).await {
            Ok(events) => events,
            Err(e) => {
                tracing::warn!(calendar = %calendar.id, error = ?e, "Google カレンダーの予定を読めませんでした（スキップ）");
                continue;
            }
        };
        fetched_calendars.insert(calendar.id.clone());
        for event in events {
            let fields = GoogleScheduleFields {
                title: event.title,
                description: event.description,
                start_at: event.start_local,
                end_at: event.end_local,
                google_calendar_id: calendar.id.clone(),
            };
            if let Some(existing) = local.remove(&event.id) {
                let changed = existing.title != fields.title
                    || existing.description.as_deref().unwrap_or_default() != fields.description
                    || existing.start_at != fields.start_at
                    || existing.end_at != fields.end_at
                    || existing.google_calendar_id.as_deref()
                        != Some(fields.google_calendar_id.as_str());
                if changed {
                    repo.apply_google_update(scope, existing.id, fields).await?;
                    summary.updated += 1;
                }
            } else if let Some(id) = repo
                .find_unlinked(scope, &fields.title, &fields.start_at)
                .await?
            {
                repo.set_google_link(scope, id, &event.id, &fields.google_calendar_id)
                    .await?;
                summary.linked += 1;
            } else {
                repo.insert_from_google(scope, &event.id, fields).await?;
                summary.created += 1;
            }
        }
    }

    // Google 側から消えた予定（読み込めたカレンダーのものだけ）を Yuuka からも消す。
    for gone in local.into_values() {
        let from_fetched = gone
            .google_calendar_id
            .as_deref()
            .is_some_and(|id| fetched_calendars.contains(id));
        if from_fetched && repo.delete(scope, gone.id).await? {
            summary.deleted += 1;
        }
    }
    Ok(summary)
}
