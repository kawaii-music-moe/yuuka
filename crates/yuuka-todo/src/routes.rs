//! todo ルートハンドラ（`/api/tasks*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! `bot_id` は `?botId=` クエリ（既定 `system_default`）から。`UserScope` を束ねて repo に渡す。
//! 本ルータは [`crate::routes`] を通じ supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::{BotId, UserId, UserScope, WebError};
use yuuka_types::Envelope;
use yuuka_web::{ApiError, AppState, AuthenticatedUser, Db};

use crate::dto::{DeletedData, NewTodo, TaskData, TaskListData};
use crate::repo::TodoRepo;

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdInput {
    id: i64,
}

/// todo ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/tasks", get(list))
        .route("/api/tasks/add", post(add))
        .route("/api/tasks/complete", post(complete))
        .route("/api/tasks/delete", post(delete))
}

fn scope_of(user: &AuthenticatedUser, q: &BotQuery) -> UserScope {
    let uid = UserId::new(user.0.discord_id.clone());
    let bid = q
        .bot_id
        .clone()
        .map_or_else(BotId::system_default, BotId::new);
    UserScope::new(uid, bid)
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<TaskListData>>, ApiError> {
    let tasks = TodoRepo::new(&db).list(&scope_of(&user, &q)).await?;
    Ok(Json(Envelope::ok(TaskListData { tasks })))
}

async fn add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(input): Json<NewTodo>,
) -> Result<Json<Envelope<TaskData>>, ApiError> {
    if input.title.trim().is_empty() {
        return Err(ApiError(WebError::Validation("title is required".to_owned())));
    }
    let task = TodoRepo::new(&db).add(&scope_of(&user, &q), input).await?;
    Ok(Json(Envelope::ok(TaskData { task })))
}

async fn complete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(input): Json<IdInput>,
) -> Result<Json<Envelope<TaskData>>, ApiError> {
    // 注: Node の completeTodo は該当無でも 200。ここでは 404 を返す（より厳密）。golden test 時に要判断。
    match TodoRepo::new(&db)
        .complete(&scope_of(&user, &q), input.id)
        .await?
    {
        Some(task) => Ok(Json(Envelope::ok(TaskData { task }))),
        None => Err(ApiError(WebError::NotFound)),
    }
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(input): Json<IdInput>,
) -> Result<Json<Envelope<DeletedData>>, ApiError> {
    if TodoRepo::new(&db)
        .delete(&scope_of(&user, &q), input.id)
        .await?
    {
        Ok(Json(Envelope::ok(DeletedData {
            deleted_id: input.id,
        })))
    } else {
        Err(ApiError(WebError::NotFound))
    }
}
