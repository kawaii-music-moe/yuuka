//! finance ルートハンドラ（`/api/expenses*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! 参照スコープ = コア CRUD の list/add に加え、予算上限（budget_limits）・支払い予定
//! （planned_payments・消込含む）・月次集計（`GET /api/expenses` の total/incomeTotal/
//! breakdown/trend）。receipt OCR（upload-receipt）は Gemini vision 経路の supervisor 層配線
//! が必要なため deferred。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{
    BudgetLimitListData, DeleteBudgetLimit, ExpenseData, ExpenseListData, NewExpense,
    NewPlannedPayment, PlannedPaymentData, PlannedPaymentId, PlannedPaymentListData,
    SetBudgetLimit, SettlePlanData,
};
use crate::repo::ExpenseRepo;

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    /// 集計対象の年（未指定は当月）。Node は `parseInt(query.year || now.getFullYear())`。
    #[serde(default)]
    year: Option<i64>,
    /// 集計対象の月 1-12（未指定は当月）。Node は `parseInt(query.month || now.getMonth()+1)`。
    #[serde(default)]
    month: Option<i64>,
}

/// `GET /api/expenses/plans` の query（`botId` + `includePaid`）。
#[derive(Debug, Deserialize)]
struct PlansQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
    /// `?includePaid=true` のみ真（Node は文字列 `"true"` の厳密一致）。
    #[serde(default, rename = "includePaid")]
    include_paid: Option<String>,
}

/// finance ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/expenses", get(list))
        .route("/api/expenses/add", post(add))
        .route(
            "/api/expenses/budget-limits",
            get(list_budget_limits).post(set_budget_limit),
        )
        .route(
            "/api/expenses/budget-limits/delete",
            post(delete_budget_limit),
        )
        .route("/api/expenses/plans", get(list_plans))
        .route("/api/expenses/plans/add", post(add_plan))
        .route("/api/expenses/plans/pay", post(pay_plan))
        .route("/api/expenses/plans/delete", post(delete_plan))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<ExpenseListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let repo = ExpenseRepo::new(&db);
    // 集計対象月: 未指定なら当月（Node の getFullYear/getMonth+1 = サーバローカル暦）。
    let (cur_year, cur_month) = repo.current_year_month().await?;
    let year = q.year.unwrap_or(cur_year);
    let month = q.month.unwrap_or(cur_month);
    let expenses = repo.list(&scope).await?;
    let total = repo.monthly_total(&scope, "expense", year, month).await?;
    let income_total = repo.monthly_total(&scope, "income", year, month).await?;
    let breakdown = repo
        .monthly_category_breakdown(&scope, year, month, "expense")
        .await?;
    let trend = repo.monthly_trend(&scope, 6).await?;
    Ok(Json(Envelope::ok(ExpenseListData {
        expenses,
        total,
        income_total,
        breakdown,
        trend,
    })))
}

async fn add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<NewExpense>,
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

// ── 予算上限（budget_limits） ────────────────────────────────────────────────

async fn list_budget_limits(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<BudgetLimitListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let limits = ExpenseRepo::new(&db).list_budget_limits(&scope).await?;
    Ok(Json(Envelope::ok(BudgetLimitListData { limits })))
}

async fn set_budget_limit(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<SetBudgetLimit>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    // Node parity: category は必須（空文字は truthy 判定で許容だが空白のみは拒否）、
    // limitAmount は 0 以上の数値（0 は許容）。i64 デシリアライズで非数/非整数は既に弾かれる。
    if input.category.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "category and limitAmount are required".to_owned(),
        )));
    }
    if input.limit_amount < 0 {
        return Err(ApiError(WebError::Validation(
            "limitAmount must be a number >= 0".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    ExpenseRepo::new(&db)
        .upsert_budget_limit(&scope, input.category.clone(), input.limit_amount)
        .await?;
    // Node: `${category} の予算上限を ¥{limitAmount.toLocaleString()} に設定しました。`
    let message = format!(
        "{} の予算上限を ¥{} に設定しました。",
        input.category,
        format_thousands(input.limit_amount)
    );
    Ok(Json(Envelope::ok_with_message(EmptyData {}, message)))
}

async fn delete_budget_limit(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<DeleteBudgetLimit>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    // Node parity: category は必須（`!category` で 400）。
    if input.category.trim().is_empty() {
        return Err(ApiError(WebError::Validation(
            "category is required".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    // Node は削除の成否に関わらず success:true + 定型メッセージを返す。
    ExpenseRepo::new(&db)
        .delete_budget_limit(&scope, input.category.clone())
        .await?;
    let message = format!("{} の予算上限を削除しました。", input.category);
    Ok(Json(Envelope::ok_with_message(EmptyData {}, message)))
}

// ── 支払い予定・消込（planned_payments） ─────────────────────────────────────

async fn list_plans(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<PlansQuery>,
) -> Result<Json<Envelope<PlannedPaymentListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    // Node: `includePaid === "true"`（厳密一致）。それ以外は pending のみ。
    let include_paid = q.include_paid.as_deref() == Some("true");
    let plans = ExpenseRepo::new(&db)
        .list_plans(&scope, include_paid)
        .await?;
    Ok(Json(Envelope::ok(PlannedPaymentListData { plans })))
}

async fn add_plan(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<NewPlannedPayment>,
) -> Result<Json<Envelope<PlannedPaymentData>>, ApiError> {
    // Node parity: 期日は dueDate ?? plannedDate（旧 UI 互換）。空文字は falsy 扱い。
    let due = pick_nonempty(input.due_date.as_deref())
        .or_else(|| pick_nonempty(input.planned_date.as_deref()));
    // title/amount/category/期日 は必須（amount は 0 も falsy で 400）。
    let due = match due {
        Some(d)
            if !input.title.trim().is_empty()
                && input.amount != 0
                && !input.category.trim().is_empty() =>
        {
            d
        }
        _ => {
            return Err(ApiError(WebError::Validation(
                "title, amount, category and due date are required".to_owned(),
            )));
        }
    };
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let plan = ExpenseRepo::new(&db).add_plan(&scope, input, due).await?;
    Ok(Json(Envelope::ok(PlannedPaymentData { plan })))
}

async fn pay_plan(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<PlannedPaymentId>,
) -> Result<Json<Envelope<SettlePlanData>>, ApiError> {
    // Node parity: `!id`（0 含む）は 400。
    if input.id == 0 {
        return Err(ApiError(WebError::Validation("id is required".to_owned())));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let repo = ExpenseRepo::new(&db);

    // 存在しなければ 404、pending 以外は 400（消込済/キャンセル済）。
    let plan = match repo.get_plan(&scope, input.id).await? {
        Some(p) => p,
        None => return Err(ApiError(WebError::NotFound)),
    };
    if plan.status != "pending" {
        return Err(ApiError(WebError::Validation(
            "already settled or cancelled".to_owned(),
        )));
    }

    // 実支払いを Expense として記録し消込・紐付き ToDo を自動完了（1 トランザクション）。
    let (expense_id, completed_todos) = repo.settle_plan(&scope, &plan).await?;
    let expense = repo
        .get(&scope, expense_id)
        .await?
        .ok_or(ApiError(WebError::Internal))?;

    // Node: `「{title}」の支払いを消込しました。（紐付く ToDo N 件を自動完了）`
    let message = if completed_todos > 0 {
        format!(
            "「{}」の支払いを消込しました。（紐付くToDo {}件を自動完了）",
            plan.title, completed_todos
        )
    } else {
        format!("「{}」の支払いを消込しました。", plan.title)
    };
    Ok(Json(Envelope::ok_with_message(
        SettlePlanData {
            expense,
            completed_todos,
        },
        message,
    )))
}

async fn delete_plan(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<PlannedPaymentId>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    // Node parity: `!id`（0 含む）は 400。成否は bare `{success: ok}` で返す。
    if input.id == 0 {
        return Err(ApiError(WebError::Validation("id is required".to_owned())));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = ExpenseRepo::new(&db).cancel_plan(&scope, input.id).await?;
    Ok(Json(Envelope::bare(ok)))
}

/// 空／空白のみでない文字列だけを `Some` にする（Node の `typeof x === "string" && x` の
/// truthy 判定相当。空文字は期日として採用しない）。
fn pick_nonempty(value: Option<&str>) -> Option<String> {
    match value {
        Some(s) if !s.is_empty() => Some(s.to_owned()),
        _ => None,
    }
}

/// 整数を 3 桁区切りにする（Node `Number.prototype.toLocaleString()` の桁区切り相当）。
///
/// 予算上限設定メッセージの `¥50,000` 等を Node と一致させる。非負整数のみを扱う
/// （呼び出し側で `limit_amount >= 0` を検証済み）。
fn format_thousands(n: i64) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        let remaining = len - i;
        if i != 0 && remaining.is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
