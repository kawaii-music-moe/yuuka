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
#[derive(Debug, Clone, Default, Deserialize, TS)]
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
    /// ルーチン（繰り返し）cron 式。tool `addTodo` の `repeat_rule` 経路で使う（HTTP add は送らない）。
    #[serde(default)]
    pub repeat_rule: Option<String>,
    /// ルーチン終了日 `YYYY-MM-DD`。`repeat_rule` がある時のみ有効。
    #[serde(default)]
    pub repeat_until: Option<String>,
    /// ルーチン実行回数（初回含む）。`repeat_rule` がある時のみ有効。
    #[serde(default)]
    pub repeat_count: Option<i64>,
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
        serde_json::Value::Number(n) => priority_num_to_i64(&n).and_then(normalize_priority_num),
        serde_json::Value::String(s) => normalize_priority_str(&s),
        _ => None,
    })
}

/// JSON 数値を JS `Number(x)` 相当で整数へ寄せる（`2` も `2.0` も `2`・JS は int/float を区別しない）。
/// 小数部を持つ値・i64 範囲外は `None`（M-7・旧 UI 互換の数値 priority 受理幅）。
fn priority_num_to_i64(n: &serde_json::Number) -> Option<i64> {
    if let Some(i) = n.as_i64() {
        return Some(i);
    }
    let f = n.as_f64()?;
    #[allow(clippy::cast_possible_truncation)]
    if f.fract() == 0.0 && (i64::MIN as f64..=i64::MAX as f64).contains(&f) {
        Some(f as i64)
    } else {
        None
    }
}

/// update 用の優先度 3 値（Node `normalizePriority` + `updateTodo` の `!== undefined` 分岐）。
///
/// - [`PriorityUpdate::Unchanged`]: フィールド未指定 **または** 不正値（未知文字列・`9` 等・bool）。
///   Node `normalizePriority` が `undefined` を返す経路（`updateTodo` は据え置き）。
/// - [`PriorityUpdate::Clear`]: `""` または `null`。Node は `null` を返し repo が priority を NULL 化。
/// - [`PriorityUpdate::Set`]: 数値 `0/1/2` または文字列 `"low"/"medium"/"high"` を正規化した値。
///
/// `#[serde(default)]` により**フィールド欠落時は `Unchanged`**（`Default`）に落ちる。
///
/// wire 上の `priority` は `"high"|"medium"|"low"|null` のままで、本 enum は Rust 内部の
/// 3 値表現に過ぎない（フロントへ露出しないため `TS` は導出しない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PriorityUpdate {
    /// 据え置き（未指定・不正値）。
    #[default]
    Unchanged,
    /// クリア（`null`/`""` → NULL）。
    Clear,
    /// 設定（正規化済み `"high"`/`"medium"`/`"low"`）。
    Set(String),
}

impl<'de> Deserialize<'de> for PriorityUpdate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value {
            // null/空文字は明示クリア（Node normalizePriority が null を返す経路）。
            serde_json::Value::Null => Self::Clear,
            serde_json::Value::String(s) if s.is_empty() => Self::Clear,
            serde_json::Value::Number(n) => priority_num_to_i64(&n)
                .and_then(normalize_priority_num)
                .map_or(Self::Unchanged, Self::Set),
            serde_json::Value::String(s) => {
                normalize_priority_str(&s).map_or(Self::Unchanged, Self::Set)
            }
            // bool/配列/オブジェクト等は Node で undefined 相当 → 据え置き。
            _ => Self::Unchanged,
        })
    }
}

/// tool 引数（JSON 値）の優先度を Node `asOptionalPriority`/`normalizePriority` 規則で正規化する。
///
/// 数値 `0/1/2` → `"low"/"medium"/"high"`、文字列は既知 3 値のみ採用、他は `None`（未設定）。
/// tool `addTodo`/`updateTodo` 経路（snake_case 引数）から使う公開ヘルパ。
#[must_use]
pub fn normalize_priority(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Number(n) => priority_num_to_i64(n).and_then(normalize_priority_num),
        serde_json::Value::String(s) => normalize_priority_str(s),
        _ => None,
    }
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

/// 単一 todo を返すペイロード（add/complete/update。`{success, task}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct TaskData {
    pub task: Todo,
}

/// `task_progress_logs` の 1 レコード（進捗報告の時系列ログ・クリーンビュー）。
///
/// **機密フェイルクローズ**: Node `TaskProgressLogRecord`（[`src/db/todoRepo.ts`]）は
/// `user_id`/`bot_id` を生 row で持つが、本 DTO は [`Todo`]/[`TodoWithSubtasks`] と同じ
/// clean-view 原則（R-13）に従い内部列を**フィールドに存在させない**（安全側の意図的差分）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct TaskProgressLog {
    pub id: i64,
    pub todo_id: i64,
    pub progress: i64,
    pub note: Option<String>,
    pub created_at: String,
}

/// `GET /api/tasks/detail` のペイロード（`{success, task, subtasks, effectiveProgress, progressLogs}`）。
///
/// Node `todoRoutes.ts` の detail ハンドラと一致。`task` は**フラットな単一 todo**（`getTodoById`
/// 相当・`subtasks`/`effective_progress` を内包しない）で、サブツリー・算出進捗・進捗ログは
/// **兄弟キー**として返す。`effectiveProgress`/`progressLogs` は **camelCase**（Node の JSON 出力・
/// フロント `TaskDetailResponse` 型と一致）。
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct TaskDetailData {
    pub task: Todo,
    pub subtasks: Vec<TodoWithSubtasks>,
    /// 算出進捗 0-100（`task`＋`subtasks` から Node `computeEffectiveProgress` で算出）。
    pub effective_progress: i64,
    /// 進捗ログ（新しい順・`listProgressLogs`）。
    pub progress_logs: Vec<TaskProgressLog>,
}

/// todo 更新リクエスト（`POST /api/tasks/update` の body）。
///
/// **wire 契約**: フロント／Node は body を **camelCase**（`dueDate`/`startDate`）で送る
/// （[`taskApi.ts`] `update`、Node `todoRoutes.ts` の `body.dueDate` 等）。`id` は必須。
/// 未指定フィールドは更新しない（`Option::None`＝据え置き）。`due_date`/`start_date` は
/// **空文字でクリア**（NULL 化・Node `updateTodo` パリティ。repo 側で畳む）。
///
/// `priority` は Node `normalizePriority` + `updateTodo` の 3 値（据え置き／クリア／設定）を
/// [`PriorityUpdate`] で厳密に再現する（`null`/`""`→クリア、正規値→設定、不正値・未指定→据え置き）。
#[derive(Debug, Clone, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct TodoUpdate {
    pub id: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    /// 優先度 3 値（据え置き／クリア／設定・[`PriorityUpdate`]）。未指定は `Unchanged`。
    /// wire 上は `"high"|"medium"|"low"|null`（`null` でクリア）。`PriorityUpdate` は `TS` 非導出の
    /// ため、生成 TS 用に型を明示する。
    #[serde(default)]
    #[ts(type = "\"high\" | \"medium\" | \"low\" | null")]
    pub priority: PriorityUpdate,
    /// ステータス。`"open"`/`"done"` のみ採用（他は `None`＝据え置き・Node パリティ）。
    #[serde(default, deserialize_with = "deserialize_status")]
    pub status: Option<String>,
}

/// `status` を `"open"`/`"done"` のみ採用し、他（未知文字列・`null`・数値等）は `None` に畳む
/// （Node `todoRoutes.ts`: `statusRaw === "open" || "done" ? statusRaw : undefined`）。
fn deserialize_status<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::String(s) if s == "open" || s == "done" => Some(s),
        _ => None,
    })
}

/// todo 進捗更新リクエスト（`POST /api/tasks/progress` の body）。
///
/// **wire 契約**: フロント／Node は camelCase 相当だが本 body は `id`/`progress`/`note` のみで
/// snake/camel の差が無い。`id`・`progress` は必須（Node は `Number()` で数値化し `!id ||
/// !Number.isFinite(progress)` を 400 で弾く）。`progress` は repo 側で 0-100 にクランプする。
#[derive(Debug, Clone, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct TodoProgress {
    pub id: i64,
    pub progress: i64,
    #[serde(default)]
    pub note: Option<String>,
}

#[cfg(test)]
mod priority_tests {
    use super::{normalize_priority, PriorityUpdate};
    use serde_json::json;

    #[test]
    fn accepts_whole_number_floats_like_js() {
        // 整数（旧 UI 互換）。
        assert_eq!(normalize_priority(&json!(2)).as_deref(), Some("high"));
        // whole-number float（JS `Number(2.0)===2`）も受理する（M-7）。
        assert_eq!(normalize_priority(&json!(2.0)).as_deref(), Some("high"));
        assert_eq!(normalize_priority(&json!(0.0)).as_deref(), Some("low"));
        // 文字列。
        assert_eq!(
            normalize_priority(&json!("medium")).as_deref(),
            Some("medium")
        );
        // 小数部あり・範囲外・未知は None。
        assert_eq!(normalize_priority(&json!(1.5)), None);
        assert_eq!(normalize_priority(&json!(9)), None);
        assert_eq!(normalize_priority(&json!("urgent")), None);
    }

    #[test]
    fn priority_update_float_is_set() {
        // update 経路でも whole-number float は Set 扱い。
        let pu: PriorityUpdate = serde_json::from_value(json!(1.0)).unwrap();
        assert_eq!(pu, PriorityUpdate::Set("medium".to_owned()));
    }
}
