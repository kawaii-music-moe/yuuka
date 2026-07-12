//! timeline ルートハンドラ（`/api/timeline/*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! 方針（M-12・全ドメイン共通）: delete の該当無は Node パリティで **200 `{success:false}`**
//! を返す（404 にしない・`deletedId` は返さない）。
//!
//! T1 参照スコープ = `timeline_records` のコア CRUD（day list / add / delete）＋
//! `day_plan_blocks` の CRUD（plan add / update / delete・day list に blocks 同梱）。
//! media 保存/配信・expense/task_done の cross-domain 副作用は deferred。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{
    DayPlanBlockData, NewDayPlanBlock, NewTimelineRecord, TimelineDayData, TimelineRecordData,
    UpdatePlanBlock,
};
use crate::repo::TimelineRepo;

#[derive(Debug, Deserialize)]
struct DayQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdInput {
    id: i64,
}

/// timeline ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/timeline/day", get(day))
        .route("/api/timeline/plan", post(plan_add))
        .route("/api/timeline/plan/update", post(plan_update))
        .route("/api/timeline/plan/delete", post(plan_delete))
        .route("/api/timeline/record", post(add))
        .route("/api/timeline/record/delete", post(delete))
}

async fn day(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<DayQuery>,
) -> Result<Json<Envelope<TimelineDayData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    // Node 同様、date 未指定・空文字は本日（UTC）にフォールバックする（repo が SQL で畳む）。
    let date = q.date.filter(|d| !d.trim().is_empty());
    let repo = TimelineRepo::new(&db);
    // Node は blocks / records の両方を返す（同じ resolve 済み date を両クエリへ渡す）。
    let blocks = repo.list_plans(&scope, date.clone()).await?;
    let records = repo.list(&scope, date).await?;
    Ok(Json(Envelope::ok(TimelineDayData { blocks, records })))
}

async fn plan_add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson { bot_id, value: input }: ScopedJson<NewDayPlanBlock>,
) -> Result<Json<Envelope<DayPlanBlockData>>, ApiError> {
    // Node parity: date / type / title のいずれか欠落（空文字含む）は 400。
    if input.date.trim().is_empty() {
        return Err(ApiError(WebError::Validation("date is required".to_owned())));
    }
    if input.r#type.trim().is_empty() {
        return Err(ApiError(WebError::Validation("type is required".to_owned())));
    }
    if input.title.trim().is_empty() {
        return Err(ApiError(WebError::Validation("title is required".to_owned())));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let block = TimelineRepo::new(&db).add_plan(&scope, input).await?;
    Ok(Json(Envelope::ok(DayPlanBlockData { block })))
}

async fn plan_update(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson { bot_id, value: input }: ScopedJson<UpdatePlanBlock>,
) -> Result<Json<Envelope<DayPlanBlockData>>, ApiError> {
    // Node parity: `Number(b.id)` が falsy（0/欠落）は 400。
    if input.id == 0 {
        return Err(ApiError(WebError::Validation("id is required".to_owned())));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    match TimelineRepo::new(&db).update_plan(&scope, input).await? {
        // 該当無は Node 同様 404（delete の 200 `{success:false}` とは異なる・Node parity）。
        Some(block) => Ok(Json(Envelope::ok(DayPlanBlockData { block }))),
        None => Err(ApiError(WebError::NotFound)),
    }
}

async fn plan_delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson { bot_id, value: input }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    // Node parity: `Number(b.id)` が falsy（0/欠落）は 400。
    if input.id == 0 {
        return Err(ApiError(WebError::Validation("id is required".to_owned())));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = TimelineRepo::new(&db).delete_plan(&scope, input.id).await?;
    Ok(Json(Envelope::bare(ok)))
}

async fn add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson { bot_id, value: input }: ScopedJson<NewTimelineRecord>,
) -> Result<Json<Envelope<TimelineRecordData>>, ApiError> {
    if input.date.trim().is_empty() {
        return Err(ApiError(WebError::Validation("date is required".to_owned())));
    }
    if input.r#type.trim().is_empty() {
        return Err(ApiError(WebError::Validation("type is required".to_owned())));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let record = TimelineRepo::new(&db).add(&scope, input).await?;
    Ok(Json(Envelope::ok(TimelineRecordData { record })))
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson { bot_id, value: input }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = TimelineRepo::new(&db).delete(&scope, input.id).await?;
    Ok(Json(Envelope::bare(ok)))
}
