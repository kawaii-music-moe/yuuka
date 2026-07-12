//! finance ドメインの wire DTO（自クレートに配置し ts-rs で生成）。
//!
//! **機密フェイルクローズ**（金銭データ）: `user_id`/`bot_id` 等の内部列は [`Expense`] の
//! **フィールドに存在させない**（既存 Node の `ExpenseRecord` から内部列を除いたクリーンビュー）。
//! 生成 TS にも現れず漏洩は型的に不可能。既存フロントは snake_case。
//!
//! DTO 名にはドメイン接頭辞（`Expense*`）を付け、共有 `generated/` での衝突を避ける。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// クライアントへ返す収支記録（クリーンビュー・snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct Expense {
    pub id: i64,
    /// `"income" | "expense"`。
    pub r#type: String,
    /// 円単位（整数）。
    pub amount: i64,
    pub category: String,
    pub memo: Option<String>,
    /// `'YYYY-MM-DD'`。
    pub date: String,
    /// `'HH:MM:SS'`（任意）。
    pub time: Option<String>,
    /// `"manual" | "receipt_ocr"`。
    pub source: String,
    pub created_at: String,
}

/// 収支作成リクエスト（`POST /api/expenses/add` の body）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct NewExpense {
    /// 円単位（整数・必須）。
    pub amount: i64,
    pub category: String,
    /// Node body の `description`（DB では `memo` 列）。
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub time: Option<String>,
    /// `"income" | "expense"`（未指定は `expense`）。
    #[serde(default)]
    pub r#type: Option<String>,
}

/// `GET /api/expenses` のペイロード（`Envelope<ExpenseListData>` = `{success, expenses}`）。
///
/// Node は同エンドポイントで total/incomeTotal/breakdown/trend も返すが、
/// 集計系は deferred（コア CRUD 縦スライスに限定）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct ExpenseListData {
    pub expenses: Vec<Expense>,
}

/// 単一収支を返すペイロード（add。`{success, expense}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct ExpenseData {
    pub expense: Expense,
}

// ── 予算上限（budget_limits・§3.4.1） ────────────────────────────────────────

/// クライアントへ返す予算上限の1件（Node `BudgetLimit` = `{category, limit_amount}`）。
///
/// 内部列 `user_id`/`bot_id` はクリーンビューとして露出させない（金銭データの分離）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct BudgetLimit {
    pub category: String,
    /// 円単位（整数・0 以上）。
    pub limit_amount: i64,
}

/// `GET /api/expenses/budget-limits` のペイロード（`{success, limits}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct BudgetLimitListData {
    pub limits: Vec<BudgetLimit>,
}

/// 予算上限の設定リクエスト（`POST /api/expenses/budget-limits` の body）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct SetBudgetLimit {
    pub category: String,
    /// 円単位（整数・必須・0 以上）。
    pub limit_amount: i64,
}

/// 予算上限の削除リクエスト（`POST /api/expenses/budget-limits/delete` の body）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct DeleteBudgetLimit {
    pub category: String,
}

// ── 支払い予定・消込（planned_payments・§3.4.3） ─────────────────────────────

/// クライアントへ返す支払い予定の1件（Node `PlannedPaymentRecord` からクリーン化）。
///
/// 内部列 `user_id`/`bot_id` は露出させない（金銭データの分離キー）。それ以外の列は
/// Node の `PlannedPaymentRecord` と同名（snake_case）で維持する。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct PlannedPayment {
    pub id: i64,
    pub title: String,
    /// 円単位（整数）。
    pub amount: i64,
    pub category: String,
    pub memo: Option<String>,
    /// 支払い期日 `'YYYY-MM-DD'`。
    pub due_date: String,
    /// 繰り返し支払いの cron 式。単発は `null`。
    pub repeat_rule: Option<String>,
    /// `"pending" | "settled" | "cancelled"`。
    pub status: String,
    /// 消込した Expense の ID。
    pub settled_expense_id: Option<i64>,
    /// 紐付き ToDo の ID。
    pub linked_todo_id: Option<i64>,
    /// 紐付きリマインドの ID。
    pub linked_reminder_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// `GET /api/expenses/plans` のペイロード（`{success, plans}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct PlannedPaymentListData {
    pub plans: Vec<PlannedPayment>,
}

/// 単一支払い予定を返すペイロード（add。`{success, plan}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct PlannedPaymentData {
    pub plan: PlannedPayment,
}

/// 支払い予定の登録リクエスト（`POST /api/expenses/plans/add` の body）。
///
/// Node parity: `dueDate` 優先・無ければ旧 UI 互換の `plannedDate` を期日として採用。
/// body の `description` は DB `memo` 列へ。全期日・memo・repeatRule はハンドラ側で検証／正規化。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct NewPlannedPayment {
    pub title: String,
    /// 円単位（整数・必須）。
    pub amount: i64,
    pub category: String,
    /// 支払い期日 `'YYYY-MM-DD'`（`plannedDate` 未指定時の優先入力）。
    #[serde(default, rename = "dueDate")]
    pub due_date: Option<String>,
    /// 旧 UI 互換の期日入力（`dueDate` 非存在時のフォールバック）。
    #[serde(default, rename = "plannedDate")]
    pub planned_date: Option<String>,
    /// Node body の `description`（DB では `memo` 列）。
    #[serde(default)]
    pub description: Option<String>,
    /// cron 式（繰り返し支払いの場合のみ）。
    #[serde(default, rename = "repeatRule")]
    pub repeat_rule: Option<String>,
}

/// 支払い予定の ID 指定リクエスト（`plans/pay`・`plans/delete` 共通の body）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct PlannedPaymentId {
    /// 対象の支払い予定 ID。
    pub id: i64,
}

/// `POST /api/expenses/plans/pay` の成功ペイロード（`{success, expense, completedTodos}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct SettlePlanData {
    /// 消込で記録した実支払い（Expense）。
    pub expense: Expense,
    /// 自動完了した紐付き ToDo の件数（Node `completedTodos.length`）。
    #[serde(rename = "completedTodos")]
    pub completed_todos: i64,
}
