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

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use yuuka_core::WebError;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{Persona, PersonaData, PersonaListData, SavePersona, PERSONA_MAX_LENGTH};
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
        .route("/api/personas/marketplace", get(marketplace_list))
        .route("/api/personas/marketplace/{id}", get(marketplace_get))
        .route("/api/personas/import", post(import_persona))
        .route("/api/personas/publish", post(publish_persona))
        .route("/api/personas/activate", post(activate_persona))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<PersonaListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let repo = PersonaRepo::new(&db);
    let personas = repo.list(&scope).await?;
    // M-11: 当該スコープの適用中ペルソナ ID（Node `getActivePersonaIdForBot`）。
    let active_persona_id = repo.active_persona_id(&scope).await?;
    Ok(Json(Envelope::ok(PersonaListData {
        personas,
        active_persona_id,
        // i64 へ落として wire に載せる（DTO 由来の定数上限）。
        max_length: i64::try_from(PERSONA_MAX_LENGTH).unwrap_or(i64::MAX),
    })))
}

/// 作成／更新（`id` 指定かつスコープ内実在で更新、それ以外は新規作成）。
async fn save(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<SavePersona>,
) -> Result<Json<Envelope<PersonaData>>, ApiError> {
    if input.name.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "name is required".to_owned(),
        )));
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
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = PersonaRepo::new(&db).delete(&scope, input.id).await?;
    Ok(Json(Envelope::bare(ok)))
}

// ─── マーケットプレイス（公開ペルソナの閲覧・auth:user だが owner を跨ぐ公開読み取り） ──────

/// 公開ペルソナ一覧（Node `GET /api/personas/marketplace`＝`{success, personas}`）。
/// `_user` は auth ゲートのためだけに取る（owner スコープはかけない＝全公開ペルソナを見せる）。
async fn marketplace_list(
    _user: AuthenticatedUser,
    State(db): State<Db>,
) -> Result<Json<Value>, ApiError> {
    let personas = PersonaRepo::new(&db).list_public().await?;
    Ok(Json(json!({ "success": true, "personas": personas })))
}

/// 公開ペルソナの全文プレビュー（Node `GET /api/personas/marketplace/:id`）。
/// id が整数でない／非公開／不在はいずれも **404 `{success:false, message}`**（Node パリティ・
/// `Number.isInteger` 不成立も未発見扱い）。
async fn marketplace_get(
    _user: AuthenticatedUser,
    State(db): State<Db>,
    Path(id): Path<String>,
) -> Response {
    let Ok(id) = id.parse::<i64>() else {
        return not_found_public();
    };
    match PersonaRepo::new(&db).get_public(id).await {
        Ok(Some(persona)) => {
            (StatusCode::OK, Json(json!({ "success": true, "persona": persona }))).into_response()
        }
        Ok(None) => not_found_public(),
        Err(_) => internal_error(),
    }
}

fn not_found_public() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "success": false, "message": "公開ペルソナが見つかりません。" })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ImportInput {
    /// Node `Number(ctx.body.id)` 相当に寛容に受ける（数値 or 数値文字列・整数以外は 400）。
    #[serde(default)]
    id: Option<Value>,
}

/// JSON 値から整数 id を取り出す（Node `Number(x)` + `Number.isInteger`＝整数 JSON 数値 or 整数文字列）。
fn as_int_id(v: Option<Value>) -> Option<i64> {
    match v {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// 公開ペルソナを自分の所有として独立コピーする（Node `POST /api/personas/import`）。
/// id が整数でない → 400「id は必須です。」・非公開/不在 → 404・成功 → 200 {persona, message}。
async fn import_persona(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<ImportInput>,
) -> Response {
    let Some(id) = as_int_id(input.id) else {
        return missing_id();
    };
    // persona は owner=user 単位（bot_id は不使用だが共通の解決経路を通す）。
    let scope = match resolve_scope(&user.0, &db, bot_id.as_deref()).await {
        Ok(s) => s,
        Err(_) => return internal_error(),
    };
    match PersonaRepo::new(&db).import_public(&scope, id).await {
        Ok(Some(persona)) => {
            let message = format!(
                "ペルソナ「{}」をインポートしました。「適用」すると会話に反映されます。",
                persona.name
            );
            (
                StatusCode::OK,
                Json(json!({ "success": true, "persona": persona, "message": message })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "message": "公開ペルソナが見つかりません（非公開化された可能性があります）。"
            })),
        )
            .into_response(),
        Err(_) => internal_error(),
    }
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "success": false, "message": "内部エラーが発生しました。" })),
    )
        .into_response()
}

/// id が整数でない/欠落時の 400（Node `!Number.isInteger(id)` 分岐・import/publish 共通）。
fn missing_id() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "success": false, "message": "id は必須です。" })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ActivateInput {
    /// null/欠落/空文字 → 適用解除（既定へ）。それ以外は整数 id（Node `Number(id)`+`isInteger`）。
    #[serde(default)]
    id: Option<Value>,
}

/// (現在の Bot に対する) 適用中ペルソナを切り替える（Node `POST /api/personas/activate`・**bot 単位**）。
/// null/空 → 解除、整数以外 → 400「id が不正です。」、他人のペルソナ → 403、成功 → 200 {message}。
async fn activate_persona(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<ActivateInput>,
) -> Response {
    let scope = match resolve_scope(&user.0, &db, bot_id.as_deref()).await {
        Ok(s) => s,
        Err(_) => return internal_error(),
    };
    let repo = PersonaRepo::new(&db);
    // null/欠落/空文字は「適用解除（既定へ）」（Node `id != null && id !== "" ? Number(id) : null`）。
    let cleared = match &input.id {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        _ => false,
    };
    if cleared {
        return match repo.set_active(&scope, None).await {
            Ok(()) => (
                StatusCode::OK,
                Json(json!({ "success": true, "message": "デフォルトペルソナに戻しました。" })),
            )
                .into_response(),
            Err(_) => internal_error(),
        };
    }
    let Some(id) = as_int_id(input.id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "message": "id が不正です。" })),
        )
            .into_response();
    };
    // 自分のペルソナのみ適用できる（`get` は owner-scoped＝他人/不在は None → 403）。
    let persona = match repo.get(&scope, id).await {
        Ok(p) => p,
        Err(_) => return internal_error(),
    };
    let Some(persona) = persona else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "success": false, "message": "自分のペルソナのみ適用できます。" })),
        )
            .into_response();
    };
    match repo.set_active(&scope, Some(id)).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": format!("ペルソナ「{}」を適用しました。", persona.name)
            })),
        )
            .into_response(),
        Err(_) => internal_error(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishInput {
    #[serde(default)]
    id: Option<Value>,
    /// Node `ctx.body.isPublic === true`（**厳密 true のみ公開**・非 bool/欠落は非公開扱い）。
    #[serde(default)]
    is_public: Option<Value>,
}

/// 公開/非公開の切り替え（Node `POST /api/personas/publish`＝`updatePersona({isPublic})`）。
/// id が整数でない → 400。所有者本人のみ更新でき、非公開化時は推奨 Bot から解除。応答は Node 文言。
async fn publish_persona(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<PublishInput>,
) -> Response {
    let Some(id) = as_int_id(input.id) else {
        return missing_id();
    };
    // Node は厳密 `=== true`。非 bool/欠落は false（非公開）。
    let is_public = input.is_public == Some(Value::Bool(true));
    let scope = match resolve_scope(&user.0, &db, bot_id.as_deref()).await {
        Ok(s) => s,
        Err(_) => return internal_error(),
    };
    match PersonaRepo::new(&db).set_public(&scope, id, is_public).await {
        Ok(ok) => {
            let message = if ok {
                if is_public {
                    "ペルソナをマーケットプレイスに公開しました。"
                } else {
                    "ペルソナを非公開にしました。"
                }
            } else {
                "ペルソナが見つからないか、所有者ではありません。"
            };
            (
                StatusCode::OK,
                Json(json!({ "success": ok, "message": message })),
            )
                .into_response()
        }
        Err(_) => internal_error(),
    }
}
