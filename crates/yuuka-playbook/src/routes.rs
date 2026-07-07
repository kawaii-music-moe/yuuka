//! playbook ルートハンドラ（`/api/playbooks*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! スコープ = コア CRUD（list/save/delete）。schedules/runs（cron・定期実行・履歴）は
//! ドメイン固有機能として deferred。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{NewPlaybook, PlaybookData, PlaybookListData};
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

/// playbook ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/playbooks", get(list))
        .route("/api/playbooks/save", post(save))
        .route("/api/playbooks/delete", post(delete))
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
    ScopedJson { bot_id, value: input }: ScopedJson<NewPlaybook>,
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
    ScopedJson { bot_id, value: input }: ScopedJson<NameInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    if input.name.trim().is_empty() {
        return Err(ApiError(WebError::Validation("name is required".to_owned())));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    // Node parity: `{success: <削除できたか>}`（削除した name は返さない・該当無も 200）。
    let ok = PlaybookRepo::new(&db).delete(&scope, input.name).await?;
    Ok(Json(Envelope::bare(ok)))
}
