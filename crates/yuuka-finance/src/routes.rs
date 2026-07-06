//! finance ルートハンドラ（`/api/expenses*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! Phase 1 参照スコープ = コア CRUD の list/add。receipt OCR・予算上限・支払い予定・
//! 月次集計(total/breakdown/trend) は deferred。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::Envelope;
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{ExpenseData, ExpenseListData, NewExpense};
use crate::repo::ExpenseRepo;

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

/// finance ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/expenses", get(list))
        .route("/api/expenses/add", post(add))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<ExpenseListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let expenses = ExpenseRepo::new(&db).list(&scope).await?;
    Ok(Json(Envelope::ok(ExpenseListData { expenses })))
}

async fn add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson { bot_id, value: input }: ScopedJson<NewExpense>,
) -> Result<Json<Envelope<ExpenseData>>, ApiError> {
    // Node parity: amount と category は必須（amount は 0 も金額として不正扱い＝truthy 判定）。
    if input.amount == 0 || input.category.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "amount and category are required".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let expense = ExpenseRepo::new(&db).add(&scope, input).await?;
    Ok(Json(Envelope::ok(ExpenseData { expense })))
}
