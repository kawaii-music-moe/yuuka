//! persona ルートハンドラ（`/api/personas*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]** で束ねて repo に渡す（persona は owner=user 単位の
//! ため実際に効くのは user_id・bot_id は不使用）。本ルータは supervisor 側で共通レイヤ
//! （CSRF/body 上限）配下にマージされる。
//!
//! 参照スコープ = コア CRUD（list/save=create+update/delete）。activate/publish/marketplace/
//! import/recommended-persona/admin は deferred（`bot_active_personas`・`bots` 連携が必要）。
//!
//! 方針（M-12）: delete の該当無は Node パリティで **200 `{success:false}`**（404 にしない・
//! `deletedId` は返さない）。save の該当無更新は Node 同様 404。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{
    Persona, PersonaData, PersonaListData, SavePersona, PERSONA_MAX_LENGTH,
};
use crate::repo::PersonaRepo;

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdInput {
    id: i64,
}

/// persona ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/personas", get(list))
        .route("/api/personas/save", post(save))
        .route("/api/personas/delete", post(delete))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<PersonaListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let personas = PersonaRepo::new(&db).list(&scope).await?;
    Ok(Json(Envelope::ok(PersonaListData {
        personas,
        // i64 へ落として wire に載せる（DTO 由来の定数上限）。
        max_length: i64::try_from(PERSONA_MAX_LENGTH).unwrap_or(i64::MAX),
    })))
}

/// 作成／更新（`id` 指定かつスコープ内実在で更新、それ以外は新規作成）。
async fn save(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson { bot_id, value: input }: ScopedJson<SavePersona>,
) -> Result<Json<Envelope<PersonaData>>, ApiError> {
    if input.name.trim().is_empty() {
        return Err(ApiError(WebError::Validation("name is required".to_owned())));
    }
    // Node は `prompt.length`（UTF-16 code unit 数）で判定する。`chars().count()`
    // だと非BMP文字（絵文字等）を過小評価し過剰許容になるため、UTF-16 単位で数える。
    if input.prompt.encode_utf16().count() > PERSONA_MAX_LENGTH {
        return Err(ApiError(WebError::Validation(format!(
            "prompt exceeds {PERSONA_MAX_LENGTH} chars"
        ))));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let repo = PersonaRepo::new(&db);
    let persona: Persona = match input.id {
        Some(id) => match repo.update(&scope, id, input).await? {
            Some(p) => p,
            None => return Err(ApiError(WebError::NotFound)),
        },
        None => repo.add(&scope, input).await?,
    };
    Ok(Json(Envelope::ok(PersonaData { persona })))
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson { bot_id, value: input }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = PersonaRepo::new(&db).delete(&scope, input.id).await?;
    Ok(Json(Envelope::bare(ok)))
}
