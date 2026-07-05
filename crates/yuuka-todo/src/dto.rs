//! todo ドメインの wire DTO（自クレートに配置し ts-rs で生成）。
//!
//! **機密フェイルクローズ**: `user_id`/`bot_id`/`linked_payment_id`/`due_reminded` 等の
//! 内部列は [`Todo`] の**フィールドに存在させない**（既存 Node の `toTodoEntry` 相当の
//! クリーンビュー）。生成 TS にも現れず漏洩は型的に不可能（R-13）。既存フロントは snake_case。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// クライアントへ返す todo（クリーンビュー・snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct Todo {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub due_date: Option<String>,
    pub start_date: Option<String>,
    /// `"high" | "medium" | "low"` または未設定。
    pub priority: Option<String>,
    /// パース済みタグ（DB は JSON 文字列 `tags` で保持）。
    pub tags: Vec<String>,
    /// `"open" | "done"` 等。
    pub status: String,
    pub progress: i64,
    pub parent_id: Option<i64>,
    pub repeat_rule: Option<String>,
    pub repeat_until: Option<String>,
    pub repeat_count: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// todo 作成リクエスト（`POST /api/tasks/add` の body）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct NewTodo {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub parent_id: Option<i64>,
}

/// `GET /api/tasks` のペイロード（`Envelope<TaskListData>` = `{success, tasks}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct TaskListData {
    pub tasks: Vec<Todo>,
}

/// 単一 todo を返すペイロード（add/complete。`{success, task}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct TaskData {
    pub task: Todo,
}

/// 削除結果（`{success, deletedId}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct DeletedData {
    pub deleted_id: i64,
}
