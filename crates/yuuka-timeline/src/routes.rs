//! timeline ルートハンドラ（`/api/timeline/*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! 方針（全ドメイン共通）: mutation の該当無は **404**（Node は delete で 200 を返すが、
//! Rust はより厳密に 404。golden test 段階で最終確定する）。
//!
//! T1 参照スコープ = `timeline_records` のコア CRUD（day list / add / delete）。
//! day_plan_blocks・media 保存/配信・expense/task_done の cross-domain 副作用は deferred。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::Envelope;
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db};

use crate::dto::{NewTimelineRecord, TimelineDayData, TimelineDeletedData, TimelineRecordData};
use crate::repo::TimelineRepo;

#[derive(Debug, Deserialize)]
struct DayQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdInput {
    id: i64,
}

/// timeline ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/timeline/day", get(day))
        .route("/api/timeline/record", post(add))
        .route("/api/timeline/record/delete", post(delete))
}

async fn day(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<DayQuery>,
) -> Result<Json<Envelope<TimelineDayData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let date = match q.date {
        Some(d) if !d.trim().is_empty() => d,
        _ => return Err(ApiError(WebError::Validation("date is required".to_owned()))),
    };
    let records = TimelineRepo::new(&db).list(&scope, date).await?;
    Ok(Json(Envelope::ok(TimelineDayData { records })))
}

async fn add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(input): Json<NewTimelineRecord>,
) -> Result<Json<Envelope<TimelineRecordData>>, ApiError> {
    if input.date.trim().is_empty() {
        return Err(ApiError(WebError::Validation("date is required".to_owned())));
    }
    if input.r#type.trim().is_empty() {
        return Err(ApiError(WebError::Validation("type is required".to_owned())));
    }
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let record = TimelineRepo::new(&db).add(&scope, input).await?;
    Ok(Json(Envelope::ok(TimelineRecordData { record })))
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(input): Json<IdInput>,
) -> Result<Json<Envelope<TimelineDeletedData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    if TimelineRepo::new(&db).delete(&scope, input.id).await? {
        Ok(Json(Envelope::ok(TimelineDeletedData {
            deleted_id: input.id,
        })))
    } else {
        Err(ApiError(WebError::NotFound))
    }
}
