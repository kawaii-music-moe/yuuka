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

use crate::dto::{NewTodo, TaskData, TaskDetailData, TaskListData, TodoProgress, TodoUpdate};
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

/// GET 系の共通クエリ（`?botId=` のみ）。gantt/someday はこれで足りる。
#[derive(Debug, Deserialize)]
struct ScopeQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

/// `GET /api/tasks/detail` のクエリ（`?botId=&id=`）。
///
/// `id` は Node が `Number(searchParams.get("id"))` で数値化するため、クエリ文字列として受けて
/// 自前で数値化する（`?id=abc` を 422 でなく Node と同じ 400 に落とすため・[`parse_id_query`]）。
#[derive(Debug, Deserialize)]
struct DetailQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    #[serde(default)]
    id: Option<String>,
}

/// todo ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/tasks", get(list))
        .route("/api/tasks/gantt", get(gantt))
        .route("/api/tasks/someday", get(someday))
        .route("/api/tasks/detail", get(detail))
        .route("/api/tasks/add", post(add))
        .route("/api/tasks/update", post(update))
        .route("/api/tasks/progress", post(progress))
        .route("/api/tasks/complete", post(complete))
        .route("/api/tasks/delete", post(delete))
}

/// Node `Number(raw)` 相当で `id` クエリを数値化する（`!id`＝0/NaN/未指定は `None`）。
///
/// Node は `Number(null)`=0 / `Number("")`=0 / `Number("abc")`=NaN を `!id` で 400 に落とす。
/// `f64` 経由で「数値だが 0」も弾き、非数値・空・未指定を一律 `None` にする。
fn parse_id_query(raw: Option<&str>) -> Option<i64> {
    let text = raw?.trim();
    if text.is_empty() {
        return None;
    }
    // Node の Number() は浮動小数も受けるため一旦 f64 で解釈し、0・非有限を弾いてから丸める。
    let n: f64 = text.parse().ok()?;
    if n == 0.0 || !n.is_finite() {
        return None;
    }
    Some(n as i64)
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

/// `GET /api/tasks/gantt` — 開始日 or 期限を持つ親タスク（サブタスク付き・Node `listGanttTasks`）。
async fn gantt(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<Envelope<TaskListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let tasks = TodoRepo::new(&db).list_gantt(&scope).await?;
    Ok(Json(Envelope::ok(TaskListData { tasks })))
}

/// `GET /api/tasks/someday` — 開始日・期限とも未設定の親タスク（Node `listSomedayTasks`）。
async fn someday(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<Envelope<TaskListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let tasks = TodoRepo::new(&db).list_someday(&scope).await?;
    Ok(Json(Envelope::ok(TaskListData { tasks })))
}

/// `GET /api/tasks/detail?id=` — 単一タスク詳細（サブタスク・算出進捗・進捗ログ）。
///
/// Node パリティ: `id` 未指定/非数値/0 → **400**、タスク不在 → **404**。応答は
/// `{success, task, subtasks, effectiveProgress, progressLogs}`。`task` は**フラットな単一 todo**、
/// `subtasks`/`effectiveProgress` は兄弟キー、`effectiveProgress` は `task`＋`subtasks` から算出。
async fn detail(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<DetailQuery>,
) -> Result<Json<Envelope<TaskDetailData>>, ApiError> {
    let Some(id) = parse_id_query(q.id.as_deref()) else {
        return Err(ApiError(WebError::Validation("id is required".to_owned())));
    };
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let repo = TodoRepo::new(&db);
    let Some(task) = repo.get(&scope, id).await? else {
        return Err(ApiError(WebError::NotFound));
    };
    let subtasks = repo.list_subtasks_tree(&scope, id).await?;
    let progress_logs = repo.list_progress_logs(&scope, id).await?;
    // Node computeEffectiveProgress: 子なしは done→100 / 未完→progress、子ありは葉の完了率。
    let effective_progress = crate::repo::effective_progress(&task, &subtasks);
    Ok(Json(Envelope::ok(TaskDetailData {
        task,
        subtasks,
        effective_progress,
        progress_logs,
    })))
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

/// `POST /api/tasks/update` — タイトル/説明/期限/開始日/優先度/ステータスの部分更新。
///
/// Node パリティ: `id` 未指定/0 → **400**、タスク不在 → **404**、成功で `{success, task}`。
/// 指定フィールドのみ更新（`dueDate`/`startDate` は空文字でクリア・repo 側で畳む）。
async fn update(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<TodoUpdate>,
) -> Result<Json<Envelope<TaskData>>, ApiError> {
    // Node: `const id = Number(body.id); if (!id) 400`（0/未指定を弾く。DTO で数値化済み）。
    if input.id == 0 {
        return Err(ApiError(WebError::Validation("id is required".to_owned())));
    }
    let id = input.id;
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    match TodoRepo::new(&db).update(&scope, id, input).await? {
        Some(task) => Ok(Json(Envelope::ok(TaskData { task }))),
        None => Err(ApiError(WebError::NotFound)),
    }
}

/// `POST /api/tasks/progress` — 手動進捗（0-100）更新＋進捗ログ追記。
///
/// Node パリティ: `id`/`progress` 未指定 → **400**、**サブタスクを持つ親は 409**（進捗は子から
/// 自動算出のため手動不可）、タスク不在 → **404**、成功で `{success, task}`。`progress` は repo で
/// 0-100 にクランプ。
async fn progress(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<TodoProgress>,
) -> Result<Json<Envelope<TaskData>>, ApiError> {
    // Node: `if (!id || !Number.isFinite(progress)) 400`。DTO で id/progress を i64 に数値化済み。
    if input.id == 0 {
        return Err(ApiError(WebError::Validation(
            "id and progress are required".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let repo = TodoRepo::new(&db);
    // Node: サブタスクを持つ親は進捗が子から算出されるため手動更新不可（409）。
    if !repo.list_subtasks_tree(&scope, input.id).await?.is_empty() {
        return Err(ApiError(WebError::Conflict));
    }
    match repo
        .update_progress(&scope, input.id, input.progress, input.note)
        .await?
    {
        Some(task) => Ok(Json(Envelope::ok(TaskData { task }))),
        None => Err(ApiError(WebError::NotFound)),
    }
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
