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
