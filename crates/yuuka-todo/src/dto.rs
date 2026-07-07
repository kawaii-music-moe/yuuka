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
///
/// **wire 契約**: 既存フロント／Node は body を **camelCase**（`dueDate`/`startDate`/`parentId`）で
/// 送る（[`taskApi.ts`] `add`、Node `todoRoutes.ts` の `body.dueDate` 等）。`rename_all` 欠落時は
/// これらが `#[serde(default)]` で無音で `None` に落ちるため、入力 DTO に camelCase を強制する。
/// **出力ビュー [`Todo`] は snake_case のまま**（フロント受信型と一致）。
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct NewTodo {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    /// 優先度。旧 UI 互換で **数値 `0`/`1`/`2` も文字列 `"low"`/`"medium"`/`"high"` も受理**し、
    /// `"high"`/`"medium"`/`"low"` へ正規化する（Node `normalizePriority` と厳密一致・不正値と
    /// `""`/`null` は `None`＝未設定として扱い 400 にしない）。
    #[serde(default, deserialize_with = "deserialize_priority")]
    pub priority: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub parent_id: Option<i64>,
}

/// `priority` を Node `normalizePriority`（todoRoutes.ts）と同一規則で正規化する。
///
/// 数値 `2`/`1`/`0` → `"high"`/`"medium"`/`"low"`、文字列は同名のみ採用。それ以外（`""`・
/// 未知文字列・`null`・bool 等）は `None`。Node は不正値を `undefined`（＝未設定）に畳み 400 に
/// しないため、ここでも拒否せず `None` に倒す。
fn deserialize_priority<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Number(n) => n.as_i64().and_then(normalize_priority_num),
        serde_json::Value::String(s) => normalize_priority_str(&s),
        _ => None,
    })
}

/// 数値優先度（旧 UI: 0/1/2）を正規文字列へ。範囲外は `None`。
fn normalize_priority_num(n: i64) -> Option<String> {
    match n {
        2 => Some("high".to_owned()),
        1 => Some("medium".to_owned()),
        0 => Some("low".to_owned()),
        _ => None,
    }
}

/// 文字列優先度を正規化する。既知の 3 値のみ採用し、他は `None`。
fn normalize_priority_str(s: &str) -> Option<String> {
    match s {
        "high" => Some("high".to_owned()),
        "medium" => Some("medium".to_owned()),
        "low" => Some("low".to_owned()),
        _ => None,
    }
}

/// 親タスク＋サブタスク（ネスト）＋算出進捗（`GET /api/tasks` のツリー要素・H-3）。
///
/// Node `TodoWithSubtasks`（todoRepo.ts）と一致。**clean view 原則を継承**し内部列
/// （`user_id`/`bot_id`/`linked_payment_id`/`due_reminded`）は持たない（Node は生 row で漏らすが
/// Rust は構造的フェイルクローズ＝安全側の意図的差分）。`subtasks` は ORDER_CLAUSE 順で入れ子。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct TodoWithSubtasks {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub due_date: Option<String>,
    pub start_date: Option<String>,
    pub priority: Option<String>,
    pub tags: Vec<String>,
    pub status: String,
    pub progress: i64,
    pub parent_id: Option<i64>,
    pub repeat_rule: Option<String>,
    pub repeat_until: Option<String>,
    pub repeat_count: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    /// 子タスク（再帰・深さ無制限）。
    pub subtasks: Vec<TodoWithSubtasks>,
    /// 算出進捗 0-100（葉の完了率をボトムアップ集計。子なしは `done?100:progress`）。
    pub effective_progress: i64,
}

/// `GET /api/tasks` のペイロード（`Envelope<TaskListData>` = `{success, tasks}`）。
///
/// Node `listTodoTree` と一致し、**親タスクのみ**を `subtasks` ネスト付きで返す。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct TaskListData {
    pub tasks: Vec<TodoWithSubtasks>,
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
