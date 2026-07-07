//! todo ルートハンドラ（`/api/tasks*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! 方針（M-12・全ドメイン共通）: mutation の該当無応答は Node パリティ。complete/delete は
//! **200 `{success:<bool>}`** を返す（該当無でも 404 にしない・delete は `deletedId` を返さない）。

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{NewTodo, TaskData, TaskListData};
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
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    // Node parity: 完了できれば `{success:true, task}`、該当無は 404 ではなく `200 {success:false}`。
    Ok(match TodoRepo::new(&db).complete(&scope, input.id).await? {
        Some(task) => Json(Envelope::ok(TaskData { task })).into_response(),
        None => Json(Envelope::<EmptyData>::bare(false)).into_response(),
    })
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    // Node parity: `{success: <削除できたか>}`（`deletedId` は返さない・該当無も 200）。
    let ok = TodoRepo::new(&db).delete(&scope, input.id).await?;
    Ok(Json(Envelope::bare(ok)))
}
