//! schedule ルートハンドラ（`/api/schedules*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未指定・system_default は 403、未アクセスは 404）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! 登録・削除は [`ScheduleService`] を通し、エージェントのツールと同じく Google カレンダーにも
//! 反映する（issue #127 / D03・[`routes_with_calendar`] で同期先を渡す）。
//!
//! 方針（M-12・全ドメイン共通）: delete の該当無は Node パリティで **200 `{success:false}`**
//! を返す（404 にしない・`deletedId` は返さない）。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use yuuka_google::CalendarEventsPort;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{NewSchedule, ScheduleData, ScheduleListData};
use crate::repo::ScheduleRepo;
use crate::service::{AddOptions, ScheduleService};

/// `GET /api/schedules` のクエリ既定日数（Node parity）。
const DEFAULT_DAYS: i64 = 7;

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    #[serde(default)]
    days: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct IdInput {
    id: i64,
}

/// 登録・削除で使う Google カレンダー同期先（`None` なら同期しない）。
#[derive(Clone)]
struct ScheduleCalendar(Option<Arc<dyn CalendarEventsPort>>);

/// schedule ドメインのルータ（`AppState` 上でマージされる・Google 同期なし）。
pub fn routes() -> Router<AppState> {
    routes_with_calendar(None)
}

/// [`routes`] の Google カレンダー同期付き版（エージェントの `addSchedule`/`deleteSchedule` と同じ
/// 同期先を渡す）。
pub fn routes_with_calendar(calendar: Option<Arc<dyn CalendarEventsPort>>) -> Router<AppState> {
    Router::new()
        .route("/api/schedules", get(list))
        .route("/api/schedules/add", post(add))
        .route("/api/schedules/delete", post(delete))
        .layer(Extension(ScheduleCalendar(calendar)))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Envelope<ScheduleListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let days = q.days.unwrap_or(DEFAULT_DAYS);
    let schedules = ScheduleRepo::new(&db).list_upcoming(&scope, days).await?;
    Ok(Json(Envelope::ok(ScheduleListData { schedules })))
}

async fn add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(ScheduleCalendar(calendar)): Extension<ScheduleCalendar>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<NewSchedule>,
) -> Result<Json<Envelope<ScheduleData>>, ApiError> {
    // タイトル・開始日時の必須検証は PWA・ツールと共通のサービスで行う（D02 #126）。
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let (schedule, _sync) = ScheduleService::new(db, calendar)
        .add(&scope, input, AddOptions::default())
        .await?;
    Ok(Json(Envelope::ok(ScheduleData { schedule })))
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(ScheduleCalendar(calendar)): Extension<ScheduleCalendar>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = ScheduleService::new(db, calendar)
        .delete(&scope, input.id)
        .await?
        .is_some();
    Ok(Json(Envelope::bare(ok)))
}
