//! schedule ルートハンドラ（`/api/schedules*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! 方針（全ドメイン共通）: mutation の該当無は **404**（Node は delete で 200 を返すが、
//! Rust はより厳密に 404。golden test 段階で最終確定する）。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::Envelope;
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db};

use crate::dto::{NewSchedule, ScheduleData, ScheduleDeletedData, ScheduleListData};
use crate::repo::ScheduleRepo;

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
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdInput {
    id: i64,
}

/// schedule ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/schedules", get(list))
        .route("/api/schedules/add", post(add))
        .route("/api/schedules/delete", post(delete))
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
    Query(q): Query<BotQuery>,
    Json(input): Json<NewSchedule>,
) -> Result<Json<Envelope<ScheduleData>>, ApiError> {
    if input.title.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "title is required".to_owned(),
        )));
    }
    if input.start_at.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "startAt is required".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let schedule = ScheduleRepo::new(&db).add(&scope, input).await?;
    Ok(Json(Envelope::ok(ScheduleData { schedule })))
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(input): Json<IdInput>,
) -> Result<Json<Envelope<ScheduleDeletedData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    if ScheduleRepo::new(&db).delete(&scope, input.id).await? {
        Ok(Json(Envelope::ok(ScheduleDeletedData {
            deleted_id: input.id,
        })))
    } else {
        Err(ApiError(WebError::NotFound))
    }
}
