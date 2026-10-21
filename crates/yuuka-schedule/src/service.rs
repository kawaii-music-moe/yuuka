//! 予定の登録・削除の共通経路（Google カレンダー同期を含む）。
//!
//! エージェントのツール（`addSchedule`/`deleteSchedule`）・管理画面の `/api/schedules/*`・
//! PWA の `/api/client/calendar/events` はすべてここを通す。入口ごとに `ScheduleRepo` を直接
//! 呼ぶと、Google への反映がツール経由のときだけ起きる食い違いが出るため（issue #127 / D03）。
//!
//! 予定は常に Yuuka に保存し、エージェントに Google アカウントが連携されているときだけ Google にも
//! 登録・削除する。Google 側の失敗で Yuuka 側の操作は取り消さない（結果の [`GoogleSync`] で伝える）。
//!
//! 必須項目（タイトル・開始日時）の検証もここで行い、[`DomainError::Invalid`] で返す（D02 #126。
//! ToDo・家計・ノートの共通サービスと同じ形）。日時の書式の解釈（PWA の `datetime-local`、ツールの
//! ISO 8601）は入力形式の違いなので各入口に残す。

use std::sync::Arc;

use yuuka_core::UserScope;
use yuuka_google::{CalendarEventsPort, GoogleEventInput, LinkedGoogleAccount};
use yuuka_web::{Db, DomainError};

use crate::dto::{NewSchedule, Schedule};
use crate::repo::ScheduleRepo;

/// Google カレンダー同期の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleSync {
    /// Google にも反映した。
    Synced,
    /// 連携しているが反映に失敗した。
    Failed,
    /// Google と連携していない（削除では「同期されていない予定」）。
    NotLinked,
    /// `local_only` 指定で送らなかった。
    LocalOnly,
}

impl GoogleSync {
    /// ツール結果の `google_sync` に載せる値。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Synced => "synced",
            Self::Failed => "failed",
            Self::NotLinked => "not_linked",
            Self::LocalOnly => "local_only",
        }
    }
}

/// 登録時の Google 同期の指定。
#[derive(Debug, Clone, Default)]
pub struct AddOptions {
    /// 登録先の Google カレンダー（`None` ならアカウントの既定カレンダー）。
    pub calendar_id: Option<String>,
    /// `true` なら Google に送らない。
    pub local_only: bool,
}

/// 予定の登録・削除（Google 同期込み）。`calendar` が `None` なら同期しない。
#[derive(Clone)]
pub struct ScheduleService {
    db: Db,
    calendar: Option<Arc<dyn CalendarEventsPort>>,
}

impl ScheduleService {
    #[must_use]
    pub fn new(db: Db, calendar: Option<Arc<dyn CalendarEventsPort>>) -> Self {
        Self { db, calendar }
    }

    /// 予定を Yuuka に保存し、連携していれば Google にも登録する。タイトルと開始日時は必須。
    ///
    /// # Errors
    /// 必須項目が空なら [`DomainError::Invalid`]、Yuuka 側の保存・紐付けの DB 失敗時
    /// [`DomainError::Db`]（Google 側の失敗は [`GoogleSync::Failed`]）。
    pub async fn add(
        &self,
        scope: &UserScope,
        new: NewSchedule,
        opts: AddOptions,
    ) -> Result<(Schedule, GoogleSync), DomainError> {
        if new.title.trim().is_empty() {
            return Err(DomainError::Invalid("title is required".to_owned()));
        }
        if new.start_at.trim().is_empty() {
            return Err(DomainError::Invalid("startAt is required".to_owned()));
        }
        let repo = ScheduleRepo::new(&self.db);
        let schedule = repo.add(scope, new).await?;
        if opts.local_only {
            return Ok((schedule, GoogleSync::LocalOnly));
        }
        let Some((calendar, account)) = self.linked_account(scope).await else {
            return Ok((schedule, GoogleSync::NotLinked));
        };
        let calendar_id = opts
            .calendar_id
            .unwrap_or_else(|| account.default_calendar_id.clone());
        let input = GoogleEventInput {
            title: schedule.title.clone(),
            description: schedule.description.clone(),
            start_local: schedule.start_at.clone(),
            end_local: schedule.end_at.clone(),
        };
        let sync = match calendar
            .insert_event(account.account_id, &calendar_id, &input)
            .await
        {
            Ok(event_id) => {
                repo.set_google_link(scope, schedule.id, &event_id, &calendar_id)
                    .await?;
                GoogleSync::Synced
            }
            Err(e) => {
                tracing::warn!(schedule_id = schedule.id, error = ?e, "予定の Google カレンダー登録に失敗しました");
                GoogleSync::Failed
            }
        };
        Ok((schedule, sync))
    }

    /// 予定を削除する。Google と紐付いていれば先に向こうを消す（失敗しても Yuuka 側は消す）。
    /// スコープ内に該当が無ければ `None`。
    ///
    /// # Errors
    /// DB 失敗時 [`DomainError::Db`]。
    pub async fn delete(
        &self,
        scope: &UserScope,
        id: i64,
    ) -> Result<Option<GoogleSync>, DomainError> {
        let repo = ScheduleRepo::new(&self.db);
        // 先に取得して所有（スコープ）を確認する。他人の予定の Google イベントには触れない。
        if repo.get(scope, id).await?.is_none() {
            return Ok(None);
        }
        let sync = match repo.google_link(scope, id).await? {
            None => GoogleSync::NotLinked,
            Some((event_id, calendar_id)) => match self.linked_account(scope).await {
                None => GoogleSync::Failed,
                Some((calendar, account)) => {
                    let calendar_id =
                        calendar_id.unwrap_or_else(|| account.default_calendar_id.clone());
                    match calendar
                        .delete_event(account.account_id, &calendar_id, &event_id)
                        .await
                    {
                        Ok(()) => GoogleSync::Synced,
                        Err(e) => {
                            tracing::warn!(schedule_id = id, error = ?e, "予定の Google カレンダー削除に失敗しました");
                            GoogleSync::Failed
                        }
                    }
                }
            },
        };
        Ok(repo.delete(scope, id).await?.then_some(sync))
    }

    /// 予定の書き込みに使う Google アカウント（同期が無効・未連携なら `None`）。
    async fn linked_account(
        &self,
        scope: &UserScope,
    ) -> Option<(&Arc<dyn CalendarEventsPort>, LinkedGoogleAccount)> {
        let calendar = self.calendar.as_ref()?;
        let account = calendar
            .account_for(scope.user_id().as_str(), scope.bot_id().as_str())
            .await?;
        Some((calendar, account))
    }
}
