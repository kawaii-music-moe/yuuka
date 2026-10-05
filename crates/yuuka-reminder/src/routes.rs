//! reminder ルートハンドラ（`/api/reminders*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! 方針: cancel は Node パリティで **404（不在）/ 409（実在するが pending でない）** を区別する。
//! cron 検証・過去日時補正・既定送信先解決は deferred。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::Envelope;
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{NewReminder, ReminderData, ReminderListData};
use crate::repo::ReminderRepo;

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    /// `all=1` / `all=true` で送信済み・キャンセル済みも含める。
    #[serde(default)]
    all: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReminderIdInput {
    reminder_id: i64,
}

/// reminder ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/reminders", get(list))
        .route("/api/reminders/add", post(add))
        .route("/api/reminders/cancel", post(cancel))
        .route("/api/reminders/delete", post(delete))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Envelope<ReminderListData>>, ApiError> {
    let include_all = matches!(q.all.as_deref(), Some("1") | Some("true"));
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let reminders = ReminderRepo::new(&db).list(&scope, include_all).await?;
    Ok(Json(Envelope::ok(ReminderListData { reminders })))
}

async fn add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<NewReminder>,
) -> Result<Json<Envelope<ReminderData>>, ApiError> {
    if input.message.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "message is required".to_owned(),
        )));
    }
    if input.trigger_at.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "trigger_at is required".to_owned(),
        )));
    }
    // #6/B4: 正規化できない日時（Z/オフセット/不正日付）は 400 で弾く（Node `reminderRoutes.ts`・
    // tool `tools.rs` と同一判定）。repo は正規化済み文字列を INSERT するため、字句比較バグを残さない。
    if crate::datetime::to_db_datetime(&input.trigger_at).is_none() {
        return Err(ApiError(WebError::Validation(
            "trigger_at must be a valid date-time".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let reminder = ReminderRepo::new(&db).add(&scope, input).await?;
    Ok(Json(Envelope::ok(ReminderData { reminder })))
}

async fn cancel(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<ReminderIdInput>,
) -> Result<Json<Envelope<ReminderData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let repo = ReminderRepo::new(&db);
    match repo.cancel(&scope, input.reminder_id).await? {
        Some(reminder) => Ok(Json(Envelope::ok(ReminderData { reminder }))),
        // Node parity: cancel 失敗の理由を区別する。実在するが pending でない
        // （送信済み／キャンセル済み）→ 409、まったく存在しない → 404。
        None => match repo.get(&scope, input.reminder_id).await? {
            Some(_) => Err(ApiError(WebError::Conflict)),
            None => Err(ApiError(WebError::NotFound)),
        },
    }
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<ReminderIdInput>,
) -> Result<Json<Envelope<ReminderData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    // 削除前の行を返す（Node に delete route は無いが CRUD 完備・{success, reminder}）。
    let Some(reminder) = ReminderRepo::new(&db)
        .get(&scope, input.reminder_id)
        .await?
    else {
        return Err(ApiError(WebError::NotFound));
    };
    if ReminderRepo::new(&db)
        .delete(&scope, input.reminder_id)
        .await?
    {
        Ok(Json(Envelope::ok(ReminderData { reminder })))
    } else {
        Err(ApiError(WebError::NotFound))
    }
}
