//! credential ルートハンドラ（`/api/credentials*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! **機密ドメイン**: 返す DTO は暗号化列・鍵材料を持たない（[`crate::dto`] のクリーンビュー）。
//!
//! 方針（M-12・全ドメイン共通）: delete の該当無は Node パリティで **200 `{success:false}`**
//! を返す（404 にしない・削除した service_name は返さない）。
//!
//! **deferred（コア CRUD 外・後回し）**: `POST /api/credentials/register`（secretService の
//! ユーザー鍵暗号化 Argon2id+AES-256-GCM が必要・本クレート外）、GET 一覧の
//! `bot_credential_access` 許可フィルタ、grant/revoke 連携。詳細は lib.rs docstring。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{Credential, CredentialListData, DeleteCredential};
use crate::repo::CredentialRepo;

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

/// credential ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/credentials", get(list))
        .route("/api/credentials/delete", post(delete))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<CredentialListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let credentials: Vec<Credential> = CredentialRepo::new(&db).list(&scope).await?;
    Ok(Json(Envelope::ok(CredentialListData { credentials })))
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<DeleteCredential>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    if input.service_name.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "serviceName is required".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = CredentialRepo::new(&db)
        .delete(&scope, &input.service_name)
        .await?;
    Ok(Json(Envelope::bare(ok)))
}
