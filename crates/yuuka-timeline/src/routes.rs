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
//! 参照スコープ = `timeline_records` のコア CRUD（day list / add / delete）＋
//! `day_plan_blocks` の CRUD（plan add / update / delete・day list に blocks 同梱）＋
//! cross-domain 副作用（`type=expense` の expenses 二重登録・`type=task_done` の todos 完了）＋
//! メディア（`POST /api/timeline/media` base64 アップロード・`GET /api/timeline/media/{filename}`
//! 認証付き配信）。tool 経由の Discord 添付 URL からのメディア取得のみ deferred（reqwest 依存）。

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{
    DayPlanBlockData, NewDayPlanBlock, NewTimelineMedia, NewTimelineRecord, TimelineDayData,
    TimelineRecordData, UpdatePlanBlock,
};
use crate::media;
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
        .route("/api/timeline/media", post(media_upload))
        .route("/api/timeline/media/{filename}", get(media_serve))
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
    let repo = TimelineRepo::new(&db);

    // Node parity: type=expense は expenses へ二重登録（amount 必須・category は既定「その他」）。
    let record = if input.r#type == "expense" {
        // Node: `amount = Number(b.amount)` が falsy（0/NaN/未指定）は 400。
        let amount = input.amount.unwrap_or(0.0);
        if amount == 0.0 {
            return Err(ApiError(WebError::Validation("amount is required".to_owned())));
        }
        // Node: `typeof b.category === "string" ? b.category : "その他"`（空文字はそのまま採用）。
        let category = input
            .category
            .clone()
            .unwrap_or_else(|| "その他".to_owned());
        repo.add_expense_record(&scope, &input, amount, category).await?
    } else {
        // task_done の todos 完了は repo.add がトランザクション内で処理する。
        repo.add(&scope, input).await?
    };
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

/// `POST /api/timeline/media` — base64 メディアを保存し `type=media` 記録を作る（Node media ハンドラ）。
///
/// 保存先は `config.media_dir`。MIME 不正・保存失敗は Node と同じく **400 `{success:false, message}`**
/// （`saveMediaFile` の throw に対応）。`State<AppState>` から DB と media_dir の双方を取る。
async fn media_upload(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    ScopedJson { bot_id, value: input }: ScopedJson<NewTimelineMedia>,
) -> Result<Json<Envelope<TimelineRecordData>>, ApiError> {
    // Node: `!date || !base64 || !mimeType`（空文字含む）は 400「date / base64 / mimeType が必要です。」
    if input.date.trim().is_empty()
        || input.base64.trim().is_empty()
        || input.mime_type.trim().is_empty()
    {
        return Err(ApiError(WebError::Validation(
            "date / base64 / mimeType are required".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &state.db, bot_id.as_deref()).await?;
    // メディア保存（MIME 検証・書き込み）。失敗は Node 同様 400（生の内部エラーは出さない）。
    let media_path =
        media::save_media_base64(&state.config.media_dir, &input.base64, &input.mime_type, &input.date)
            .await
            .map_err(|msg| ApiError(WebError::Validation(msg)))?;
    let media_type = media::media_type_of(&input.mime_type).to_owned();
    let record_fields = NewTimelineRecord {
        date: input.date,
        r#type: "media".to_owned(),
        recorded_at: input.recorded_at,
        title: input.title,
        content: input.content,
        todo_id: None,
        amount: None,
        category: None,
        location: input.location,
    };
    let record = TimelineRepo::new(&state.db)
        .add_media_record(&scope, &record_fields, media_path, media_type)
        .await?;
    Ok(Json(Envelope::ok(TimelineRecordData { record })))
}

/// `GET /api/timeline/media/{filename}` — 認証付きメディア配信（Node media 配信ハンドラ）。
///
/// path traversal 対策（[`media::resolve_media_path`]）で不正名/未存在は **404 "Not Found"**。存在時は
/// `Content-Type`（拡張子から）+ `Cache-Control: private, max-age=86400` を付けてバイト列を返す。
async fn media_serve(
    _user: AuthenticatedUser,
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> Response {
    let Some(full) = media::resolve_media_path(&state.config.media_dir, &filename) else {
        return not_found();
    };
    match tokio::fs::read(&full).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, media::content_type_for(&filename))
            .header(header::CACHE_CONTROL, "private, max-age=86400")
            .body(Body::from(bytes))
            .unwrap_or_else(|_| not_found()),
        Err(_) => not_found(),
    }
}

/// メディア未存在/不正名の 404（Node `ctx.res.writeHead(404); ctx.res.end("Not Found")`）。
fn not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Not Found"))
        .unwrap_or_default()
}
