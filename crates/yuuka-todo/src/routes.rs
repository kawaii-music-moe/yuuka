//! todo ルートハンドラ（`/api/tasks*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! 方針（全ドメイン共通）: mutation の該当無は **404**（Node は complete/delete で 200 を
//! 返すが、Rust はより厳密に 404。golden test 段階で最終確定する）。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::Envelope;
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{DeletedData, NewTodo, TaskData, TaskListData};
use crate::repo::TodoRepo;

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    /// `"pending"`（→open）/`"done"`/その他（既定 all）。
    #[serde(default)]
    status: Option<String>,
    /// タグ絞り込み（空文字は無視）。
    #[serde(default)]
    tag: Option<String>,
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

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Envelope<TaskListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    // Node: pending→open / done→done / それ以外（既定）→ all（絞り込みなし）。
    let status = match q.status.as_deref() {
        Some("pending") => Some("open".to_owned()),
        Some("done") => Some("done".to_owned()),
        _ => None,
    };
    let tag = q.tag.filter(|t| !t.is_empty());
    let tasks = TodoRepo::new(&db).list_tree(&scope, status, tag).await?;
    Ok(Json(Envelope::ok(TaskListData { tasks })))
}

async fn add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<NewTodo>,
) -> Result<Json<Envelope<TaskData>>, ApiError> {
    if input.title.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "title is required".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let task = TodoRepo::new(&db).add(&scope, input).await?;
    Ok(Json(Envelope::ok(TaskData { task })))
}

async fn complete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<TaskData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    match TodoRepo::new(&db).complete(&scope, input.id).await? {
        Some(task) => Ok(Json(Envelope::ok(TaskData { task }))),
        None => Err(ApiError(WebError::NotFound)),
    }
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<DeletedData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    if TodoRepo::new(&db).delete(&scope, input.id).await? {
        Ok(Json(Envelope::ok(DeletedData {
            deleted_id: input.id,
        })))
    } else {
        Err(ApiError(WebError::NotFound))
    }
}
