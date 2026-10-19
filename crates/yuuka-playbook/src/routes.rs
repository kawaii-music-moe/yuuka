//! playbook ルートハンドラ（`/api/playbooks*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未指定・system_default は 403、未アクセスは 404）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! スコープ = コア CRUD（list/save/delete）＋定期実行スケジュール（schedules の
//! list/save/toggle/delete）と実行履歴（runs）。
//!
//! **cron 妥当性検証**: Node `upsertSchedule` と同じく、不正式は 400（「無効なcron式です。」）で
//! 弾く。検証器は実行側（`yuuka-services` の `PlaybookScheduleService`）と同じ
//! [`yuuka_core::cron_util::is_valid`]（登録時と実行時で解釈がずれない）。
//! それ以外（必須検証・playbook 存在検証・所有者検証・レスポンス形状・status）は Node と一致。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::{cron_util, BotId, UserId, UserScope, WebError};
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{
    NewPlaybook, NewSchedule, PlaybookData, PlaybookListData, RunListData, ScheduleData,
    ScheduleIdInput, ScheduleListData, ToggleScheduleInput,
};
use crate::repo::PlaybookRepo;

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    #[serde(default)]
    query: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NameInput {
    name: String,
}

/// `GET /api/playbooks/runs` の query（`?botId=` と任意の `?scheduleId=`）。
#[derive(Debug, Deserialize)]
struct RunsQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    #[serde(default, rename = "scheduleId")]
    schedule_id: Option<i64>,
}

/// playbook ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/playbooks", get(list))
        .route("/api/playbooks/save", post(save))
        .route("/api/playbooks/delete", post(delete))
        .route("/api/playbooks/runs", get(list_runs))
        .route("/api/playbooks/schedules", get(list_schedules))
        .route("/api/playbooks/schedules/save", post(save_schedule))
        .route("/api/playbooks/schedules/delete", post(delete_schedule))
        .route("/api/playbooks/schedules/toggle", post(toggle_schedule))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Envelope<PlaybookListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let playbooks = PlaybookRepo::new(&db).list(&scope, q.query).await?;
    Ok(Json(Envelope::ok(PlaybookListData { playbooks })))
}

async fn save(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<NewPlaybook>,
) -> Result<Json<Envelope<PlaybookData>>, ApiError> {
    if input.name.trim().is_empty()
        || input.title.trim().is_empty()
        || input.steps.trim().is_empty()
    {
        return Err(ApiError(WebError::Validation(
            "name, title, and steps are required".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let playbook = PlaybookRepo::new(&db).save(&scope, input).await?;
    Ok(Json(Envelope::ok(PlaybookData { playbook })))
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<NameInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    if input.name.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "name is required".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    // Node parity: `{success: <削除できたか>}`（削除した name は返さない・該当無も 200）。
    let ok = PlaybookRepo::new(&db).delete(&scope, input.name).await?;
    Ok(Json(Envelope::bare(ok)))
}

// ─── 定期実行スケジュール / 実行履歴 ─────────────────────────────────────────

/// `GET /api/playbooks/schedules` — Node `listSchedules(userId)`（bot 横断・`user_id` スコープ）。
///
/// Node は botId を読まない。repo は scope の `user_id` しか使わないため、bot を解決せず
/// ユーザーだけのスコープで引く（`resolve_scope` は bot 未指定を 403 にする）。
async fn list_schedules(
    user: AuthenticatedUser,
    State(db): State<Db>,
) -> Result<Json<Envelope<ScheduleListData>>, ApiError> {
    let scope = UserScope::new(
        UserId::new(user.0.discord_id.clone()),
        BotId::system_default(),
    );
    let schedules = PlaybookRepo::new(&db).list_schedules(&scope).await?;
    Ok(Json(Envelope::ok(ScheduleListData { schedules })))
}

/// `POST /api/playbooks/schedules/save` — Node `upsertSchedule`。
///
/// 必須検証（`playbookName`/`cronExpression`）と playbook 存在検証に失敗した場合は Node と同じ
/// 日本語メッセージで 400 を返す（`WebError::Validation` は `{success:false, message}` に写像）。
/// 成功時は `{success:true, message, schedule}`（Node と同形）。不正な cron 式は 400（「無効なcron式です。」）。
async fn save_schedule(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<NewSchedule>,
) -> Result<Json<Envelope<ScheduleData>>, ApiError> {
    if input.playbook_name.trim().is_empty() || input.cron_expression.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "playbookNameとcronExpressionは必須です。".to_owned(),
        )));
    }
    // Node は raw の playbookName をメッセージに埋め込む（正規化前）。
    let playbook_name = input.playbook_name.clone();
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    // Node parity: cron 式の検証は playbook 存在検証より先。
    if !cron_util::is_valid(&input.cron_expression) {
        return Err(ApiError(WebError::Validation(
            "無効なcron式です。".to_owned(),
        )));
    }
    match PlaybookRepo::new(&db)
        .upsert_schedule(&scope, input)
        .await?
    {
        Some(schedule) => Ok(Json(Envelope::ok_with_message(
            ScheduleData { schedule },
            format!("スケジュール「{playbook_name}」を保存しました。"),
        ))),
        // Node parity: playbook 不在は 400 + 固定メッセージ。
        None => Err(ApiError(WebError::Validation(format!(
            "マクロ「{playbook_name}」が見つかりません。"
        )))),
    }
}

/// `POST /api/playbooks/schedules/toggle` — Node `toggleSchedule`。
///
/// `id` 欠落は 400（「idは必須です。」）、不在／他人の id は 400（「スケジュールが見つかりません。」）。
/// 成功時は `{success:true, message}`（有効化／無効化で文言が変わる）。
async fn toggle_schedule(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<ToggleScheduleInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let Some(id) = input.id else {
        return Err(ApiError(WebError::Validation("idは必須です。".to_owned())));
    };
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = PlaybookRepo::new(&db)
        .toggle_schedule(&scope, id, input.enabled)
        .await?;
    if ok {
        let message = if input.enabled {
            "スケジュールを有効化しました。"
        } else {
            "スケジュールを無効化しました。"
        };
        Ok(Json(Envelope::ok_with_message(EmptyData {}, message)))
    } else {
        Err(ApiError(WebError::Validation(
            "スケジュールが見つかりません。".to_owned(),
        )))
    }
}

/// `POST /api/playbooks/schedules/delete` — Node `deleteSchedule`。
///
/// `id` 欠落は 400（「idは必須です。」）、不在／他人の id は 400（「スケジュールが見つかりません。」）。
/// 成功時は `{success:true, message}`。
async fn delete_schedule(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<ScheduleIdInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let Some(id) = input.id else {
        return Err(ApiError(WebError::Validation("idは必須です。".to_owned())));
    };
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = PlaybookRepo::new(&db).delete_schedule(&scope, id).await?;
    if ok {
        Ok(Json(Envelope::ok_with_message(
            EmptyData {},
            "スケジュールを削除しました。",
        )))
    } else {
        Err(ApiError(WebError::Validation(
            "スケジュールが見つかりません。".to_owned(),
        )))
    }
}

/// `GET /api/playbooks/runs` — Node `listRuns(userId, botId, scheduleId)`。
///
/// `user_id AND bot_id` スコープ、任意の `scheduleId`、`started_at` 降順・`LIMIT 50`。
async fn list_runs(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<RunsQuery>,
) -> Result<Json<Envelope<RunListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let runs = PlaybookRepo::new(&db)
        .list_runs(&scope, q.schedule_id)
        .await?;
    Ok(Json(Envelope::ok(RunListData { runs })))
}
