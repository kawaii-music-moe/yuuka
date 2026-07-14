//! todo ドメインの Native ツール（現行 `src/functions/todoFunctions.ts` の移植・§9.4）。
//!
//! **Phase 2c 参照実装**: gemini FC ループ → RegistrySnapshot → NativeProvider → 本ツール →
//! `TodoRepo` の縦スライスを end-to-end で成立させる雛形。core の凍結 [`Tool`] を実装し、
//! `tools(db)` が `Vec<Arc<dyn Tool>>` を返す（assembly 層が `NativeProvider` へ登録する。
//! ドメインは yuuka-tools に依存しない＝依存の向きを保つ）。
//!
//! **wire 契約の非対称に注意**: HTTP route の body は camelCase（`dueDate`）だが、**tool 引数は
//! snake_case**（`due_date`・Node の Gemini 宣言と一致）。ツール名は Node の system prompt が
//! 参照する **bare 名**（`addTodo` 等・namespace 無し）を使う（MCP/WASM のみ namespace 接頭辞）。
//!
//! 移植済み: addTodo / listTodos / completeTodo / deleteTodo（コア 4 経路）。
//! 未移植（後続・repo 拡張要）: addSubtask / updateTodo / タグ系 / 優先度整理 / ルーチン停止 等
//! 11 ツール、および自動タグ付与（scheduleAutoTagging＝バックグラウンドサービス）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope};
use yuuka_web::Db;

use crate::dto::{normalize_priority, NewTodo, PriorityUpdate, TodoUpdate};
use crate::repo::{effective_progress, TodoRepo};

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(AddTodoTool {
            name: ToolName::checked("addTodo".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(ListTodosTool {
            name: ToolName::checked("listTodos".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(CompleteTodoTool {
            name: ToolName::checked("completeTodo".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(DeleteTodoTool {
            name: ToolName::checked("deleteTodo".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(AddSubtaskTool {
            name: ToolName::checked("addSubtask".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(UpdateTodoTool {
            name: ToolName::checked("updateTodo".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(UpdateTaskProgressTool {
            name: ToolName::checked("updateTaskProgress".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(GetTaskDetailTool {
            name: ToolName::checked("getTaskDetail".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(ListTodoTagsTool {
            name: ToolName::checked("listTodoTags".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(ListTasksByTagTool {
            name: ToolName::checked("listTasksByTag".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(GetTaskUsageGuideTool {
            name: ToolName::checked("getTaskUsageGuide".to_owned())?,
        }),
        Arc::new(EditTodoTagsTool {
            name: ToolName::checked("editTodoTags".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(StopTodoRoutineTool {
            name: ToolName::checked("stopTodoRoutine".to_owned())?,
            db,
        }),
    ])
}

/// タグ配列を正規化する（Node `normalizeTags`＝文字列化・trim・空除去・重複除去・最大 8 件）。
fn normalize_tags(value: &Value) -> Vec<String> {
    let Some(arr) = value.as_array() else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for raw in arr {
        let Some(s) = raw.as_str() else { continue };
        let tag = s.trim();
        if tag.is_empty() || !seen.insert(tag.to_owned()) {
            continue;
        }
        out.push(tag.to_owned());
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// タスク管理の使い方ガイド本文（Node `TASK_USAGE_GUIDE_MD`・公開ページ `/tasks/guide` と同内容）。
const TASK_USAGE_GUIDE_MD: &str = "## タスク管理の使い方\n\
### 1. タスクの基本\n\
「やること」を登録して、期限・優先度・進捗とともに管理できます。「〇〇をタスクに追加して」で登録、「〇〇終わった」で完了にできます。\n\
- **期限・開始日**: 期限と開始日を設定でき、両方あるタスクはガントチャートにバーで表示されます。\n\
- **優先度**: 🔴高 / 🟡中 / 🔵低。「タスクを整理して」でAIが優先度を提案します（確定は承認後）。\n\
- **いつかやる**: 期限も開始日も決めていないタスクは「🕗 いつかやる」にまとまります。\n\
### 2. サブタスクと進捗\n\
- **サブタスク**: 大きなタスクを小さな手順に分解できます（1段まで）。親の進捗は「完了サブタスク数 ÷ 全体」で自動計算。\n\
- **進捗**: サブタスクのないタスクは 0〜100% で更新でき、メモとともに履歴に残ります。\n\
### 3. タグでグループ分け\n\
- **自動タグ付け**: 追加・更新時に内容からAIが自動でタグを付けます。\n\
- **手動修正**: 「#3のタグを『買い物』に変えて」「『緊急』タグを足して」「『仮』タグを外して」のように直せます。\n\
- **グループ表示**: 「タスクをグループ別に見せて」でタグごとに確認できます（タグ無しは「未分類」）。\n\
### 4. ルーチン（繰り返し）タスク\n\
「毎週月曜の朝に〇〇」のように伝えると繰り返しタスクとして登録され、期日が来ると自動で次回ぶんへ更新されます。\n\
- **終わり方を決めて登録**: 「年末まで毎週」（終了日）や「毎日5回だけ」（回数）を指定すると自動で止まります。\n\
- **あとから終了**: 「もう毎週の〇〇はやらなくていい」で繰り返しを終了（タスク自体は単発として残ります）。\n\
- **リマインド**: 期限が近づくと自動でDM／チャンネルに通知されます。\n\
### 5. 表示モード\n\
- **一覧**: 優先度・期限順。「全て / 未完了 / 完了済み」で絞り込み。\n\
- **ガント**: 開始日〜期限をバーで時系列表示。";

// ─── 共通ヘルパ ───────────────────────────────────────────────────────────────

/// `{success:true, message, ...extra}`（Node `ok(msg, extra)`）。
fn ok_payload(message: impl Into<String>, extra: Value) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("success".to_owned(), Value::Bool(true));
    obj.insert("message".to_owned(), Value::String(message.into()));
    if let Value::Object(map) = extra {
        for (k, v) in map {
            obj.insert(k, v);
        }
    }
    Value::Object(obj)
}

/// `{success:false, message}`（Node `fail(msg)`）。実行エラーではなく「妥当だが失敗」な結果。
fn fail_payload(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

/// ctx からデータ分離スコープを組む。
fn scope_of(ctx: &ToolContext) -> UserScope {
    UserScope::new(ctx.user_id.clone(), ctx.bot_id.clone())
}

/// `asOptionalString`（trim 後空なら None）。
fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// `asTodoId`（数値、または数値文字列を i64 へ）。
fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    let v = args.get(key)?;
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

/// 文字列引数を trim して取り出す（**空文字も `Some("")` で返す**・Node `typeof x === "string" ? x.trim() : undefined`）。
///
/// `arg_str` と違い空文字を除去しない。`updateTodo` の due_date/start_date は空文字＝クリア指示として
/// 有効値のため、キーが文字列で存在したか（＝指定されたか）を区別する必要がある。
fn arg_raw_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_owned())
}

/// 期限ラベル（Node `dueLabel`）。空なら空文字、日時は raw を採用（LLM 向け案内文のため簡約）。
fn due_label(due_date: Option<&str>) -> String {
    match due_date.map(str::trim).filter(|s| !s.is_empty()) {
        Some(d) => format!(" (期限: {d})"),
        None => String::new(),
    }
}

/// 最小 cron 妥当性（5 フィールド・空でない）。厳密な範囲検証は後続（Node `isValidCron` の簡約）。
fn is_valid_cron_basic(rule: &str) -> bool {
    let fields: Vec<&str> = rule.split_whitespace().collect();
    fields.len() == 5 && fields.iter().all(|f| !f.is_empty())
}

/// DbError をツール実行エラーへ（握り潰さず Gemini へ `{success:false}` として返る・§8.4）。
fn exec_err(e: DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

fn priority_label(priority: Option<&str>) -> &'static str {
    match priority {
        Some("high") => "高",
        Some("medium") => "中",
        Some("low") => "低",
        _ => "なし",
    }
}

// ─── addTodo ─────────────────────────────────────────────────────────────────

struct AddTodoTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for AddTodoTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "新しいタスク（やること）をToDoリストに1件追加する。\
                「〜をやることに追加して」等の登録依頼で呼ぶ。タグは追加後に自動付与されるので指定不要。\
                優先度はユーザーが明示した時だけ指定。繰り返す“ルーチン”は repeat_rule（cron式）を入れ、\
                その場合 due_date に初回期日も必ず入れる。サブタスク登録は代わりに addSubtask を使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "タスクのタイトル（短い体言止め推奨）" },
                    "description": { "type": "string", "description": "詳しい説明（任意）" },
                    "due_date": { "type": "string", "description": "締め切り。YYYY-MM-DD または YYYY-MM-DDTHH:MM:SS。ルーチン指定時は初回期日として必須（任意）" },
                    "start_date": { "type": "string", "description": "始める日。YYYY-MM-DD または YYYY-MM-DDTHH:MM:SS（任意）" },
                    "priority": { "type": "string", "description": "優先度 'high'|'medium'|'low'。ユーザーが明示した時だけ（任意）" },
                    "repeat_rule": { "type": "string", "description": "ルーチンの周期を cron 式で（分 時 日 月 曜日。例 '0 9 * * 1'=毎週月曜）。単発では指定しない（任意）" },
                    "repeat_until": { "type": "string", "description": "ルーチン終了日 YYYY-MM-DD（任意）" },
                    "repeat_count": { "type": "number", "description": "ルーチン実行回数（初回含む）（任意）" }
                },
                "required": ["title"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(title) = arg_str(&args, "title") else {
            return Ok(fail_payload("タイトルを指定してください。"));
        };

        let repeat_rule = arg_str(&args, "repeat_rule");
        let due_date = arg_str(&args, "due_date");
        if let Some(rule) = &repeat_rule {
            if !is_valid_cron_basic(rule) {
                return Ok(fail_payload(
                    "repeat_rule は cron式（分 時 日 月 曜日。例 '0 9 * * 1'=毎週月曜）で指定してください。",
                ));
            }
            if due_date.is_none() {
                return Ok(fail_payload(
                    "ルーチンタスクには初回の due_date（期日）も指定してください。",
                ));
            }
        }
        // repeat_count: 有限・正のみ採用し floor。
        let repeat_count = args
            .get("repeat_count")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && *n > 0.0)
            .map(|n| n.floor() as i64);

        let priority = args.get("priority").and_then(normalize_priority);
        let new = NewTodo {
            title: title.clone(),
            description: arg_str(&args, "description"),
            due_date,
            start_date: arg_str(&args, "start_date"),
            priority: priority.clone(),
            tags: Vec::new(),
            parent_id: None,
            // ルーチン列は repeat_rule がある時のみ有効（Node addTodo と同じ）。
            repeat_until: repeat_rule.as_ref().and(arg_str(&args, "repeat_until")),
            repeat_count: repeat_rule.as_ref().and(repeat_count),
            repeat_rule,
        };

        let todo = TodoRepo::new(&self.db)
            .add(&scope_of(ctx), new)
            .await
            .map_err(exec_err)?;

        // NOTE: 自動タグ付与（scheduleAutoTagging）はバックグラウンドサービス。後続で services 側へ。
        let message = format!(
            "ToDo「{}」を追加しました (ID: #{}、優先度: {})。タグはバックグラウンドで自動付与されます。",
            todo.title,
            todo.id,
            priority_label(todo.priority.as_deref()),
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "todo": todo }),
        )))
    }
}

// ─── listTodos ───────────────────────────────────────────────────────────────

struct ListTodosTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListTodosTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "ToDo一覧を親タスク＋サブタスク（ネスト）で取得する。\
                status は 'open'（既定）/'done'/'all'。tag でタグ絞り込み可。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "'open'（既定）/'done'/'all'" },
                    "tag": { "type": "string", "description": "タグでの絞り込み（任意）" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // status: open/done は絞り込み、all は絞り込みなし（None）。既定は open。
        let status = match arg_str(&args, "status").as_deref() {
            Some("all") => None,
            Some("done") => Some("done".to_owned()),
            _ => Some("open".to_owned()),
        };
        let tag = arg_str(&args, "tag");

        let todos = TodoRepo::new(&self.db)
            .list_tree(&scope_of(ctx), status, tag.clone())
            .await
            .map_err(exec_err)?;

        if todos.is_empty() {
            let msg = match &tag {
                Some(t) => format!(
                    "タグ「{t}」のToDoはありません。listTodoTags で存在するタグを確認できます。"
                ),
                None => "該当するToDoはありません。".to_owned(),
            };
            return Ok(ToolOutcome::from_payload(ok_payload(
                msg,
                json!({ "todos": [] }),
            )));
        }

        let msg = format!(
            "ToDo一覧 (親タスク {}件{})",
            todos.len(),
            tag.as_ref()
                .map(|t| format!("、タグ: {t}"))
                .unwrap_or_default(),
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            msg,
            json!({ "todos": todos }),
        )))
    }
}

// ─── completeTodo ────────────────────────────────────────────────────────────

struct CompleteTodoTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for CompleteTodoTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "指定した ToDo を完了にする。todo_id は listTodos で確認できる ID。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": { "todo_id": { "type": "number", "description": "完了にする ToDo の ID" } },
                "required": ["todo_id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(id) = arg_i64(&args, "todo_id") else {
            return Ok(fail_payload("todo_id を指定してください。"));
        };
        match TodoRepo::new(&self.db)
            .complete(&scope_of(ctx), id)
            .await
            .map_err(exec_err)?
        {
            Some(todo) => Ok(ToolOutcome::from_payload(ok_payload(
                format!("ToDo「{}」(#{}) を完了にしました✅", todo.title, todo.id),
                json!({ "todo": todo }),
            ))),
            None => Ok(fail_payload(format!("ToDo #{id} が見つかりません。"))),
        }
    }
}

// ─── deleteTodo ──────────────────────────────────────────────────────────────

struct DeleteTodoTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for DeleteTodoTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "指定した ToDo を削除する。todo_id は listTodos で確認できる ID。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": { "todo_id": { "type": "number", "description": "削除する ToDo の ID" } },
                "required": ["todo_id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(id) = arg_i64(&args, "todo_id") else {
            return Ok(fail_payload("todo_id を指定してください。"));
        };
        let deleted = TodoRepo::new(&self.db)
            .delete(&scope_of(ctx), id)
            .await
            .map_err(exec_err)?;
        if deleted {
            Ok(ToolOutcome::from_payload(ok_payload(
                format!("ToDo #{id} を削除しました🗑️"),
                json!({}),
            )))
        } else {
            Ok(fail_payload(format!("ToDo #{id} が見つかりません。")))
        }
    }
}

// ─── addSubtask ──────────────────────────────────────────────────────────────

struct AddSubtaskTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for AddSubtaskTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "あるタスクの中の小さな手順（サブタスク）を1件追加する。\
                「#3のサブタスクに〜を追加」等、タスクを分解した小タスクを登録する依頼で呼ぶ。\
                親タスクの進捗は『完了サブタスク数 ÷ 全サブタスク数』で自動計算される。\
                サブタスクの下にさらにサブタスクは作れない（1段まで）。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "parent_todo_id": { "type": "number", "description": "どのタスクの下に入れるか。親タスクのID（#番号）" },
                    "title": { "type": "string", "description": "サブタスクのタイトル（短い体言止め推奨）" },
                    "description": { "type": "string", "description": "サブタスクの詳しい説明（任意）" },
                    "due_date": { "type": "string", "description": "締め切り。YYYY-MM-DD または YYYY-MM-DDTHH:MM:SS（任意）" },
                    "start_date": { "type": "string", "description": "始める日。YYYY-MM-DD または YYYY-MM-DDTHH:MM:SS（任意）" }
                },
                "required": ["parent_todo_id", "title"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(title) = arg_str(&args, "title") else {
            return Ok(fail_payload("サブタスクのタイトルを指定してください。"));
        };
        let parent_id = arg_i64(&args, "parent_todo_id");
        let scope = scope_of(ctx);
        let repo = TodoRepo::new(&self.db);
        // 親タスクが存在し、スコープ内であること。
        let parent = match parent_id {
            Some(pid) => repo.get(&scope, pid).await.map_err(exec_err)?,
            None => None,
        };
        let Some(parent) = parent else {
            return Ok(fail_payload(format!(
                "親タスク #{} が見つかりません。",
                args.get("parent_todo_id").unwrap_or(&Value::Null)
            )));
        };

        let new = NewTodo {
            title,
            description: arg_str(&args, "description"),
            due_date: arg_str(&args, "due_date"),
            start_date: arg_str(&args, "start_date"),
            priority: None,
            tags: Vec::new(),
            parent_id: Some(parent.id),
            repeat_rule: None,
            repeat_until: None,
            repeat_count: None,
        };
        let subtask = repo.add(&scope, new).await.map_err(exec_err)?;

        // 親（付け替えの可能性を考慮し subtask.parent_id 基準）の兄弟サブタスク完了数を返す。
        let effective_parent = subtask.parent_id.unwrap_or(parent.id);
        let siblings = repo
            .list_subtasks_tree(&scope, effective_parent)
            .await
            .map_err(exec_err)?;
        let done = siblings.iter().filter(|s| s.status == "done").count();
        let message = format!(
            "サブタスク「{}」(#{}) を タスク#{} に追加しました。{}（このタスクのサブタスク: {}/{} 完了）",
            subtask.title,
            subtask.id,
            effective_parent,
            due_label(subtask.due_date.as_deref()),
            done,
            siblings.len(),
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "subtask": subtask, "parent_todo_id": effective_parent }),
        )))
    }
}

// ─── updateTodo ──────────────────────────────────────────────────────────────

struct UpdateTodoTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for UpdateTodoTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description:
                "既にあるタスクの中身を変える（タイトル・説明・締め切り・開始日・優先度・状態）。\
                「#2の期限を金曜にして」等の依頼で呼ぶ。変える項目だけを指定する。\
                進捗（何%まで進んだか）を変えたい時は代わりに updateTaskProgress を使う。"
                    .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "todo_id": { "type": "number", "description": "変えるタスクのID（#番号）" },
                    "title": { "type": "string", "description": "新しいタイトル（任意）" },
                    "description": { "type": "string", "description": "新しい説明文（任意）" },
                    "due_date": { "type": "string", "description": "新しい締め切り YYYY-MM-DD 等。空文字を渡すと締め切りを消す（任意）" },
                    "start_date": { "type": "string", "description": "新しい開始日 YYYY-MM-DD 等。空文字を渡すと開始日を消す（任意）" },
                    "priority": { "type": "string", "description": "新しい優先度 'high'|'medium'|'low'（任意）" },
                    "status": { "type": "string", "description": "新しい状態 'open'（未完了へ戻す）|'done'（完了）（任意）" }
                },
                "required": ["todo_id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(todo_id) = arg_i64(&args, "todo_id") else {
            return Ok(fail_payload("todo_id を指定してください。"));
        };
        // status は 'open'/'done' のみ採用。
        let status = arg_str(&args, "status").filter(|s| s == "open" || s == "done");
        // priority は指定時のみ検証（不正値はエラー・クリアは updateTodo では不可）。
        let priority_arg = arg_str(&args, "priority");
        let priority = match &priority_arg {
            Some(p) => match normalize_priority(&Value::String(p.clone())) {
                Some(valid) => PriorityUpdate::Set(valid),
                None => {
                    return Ok(fail_payload(
                        "優先度は 'high' | 'medium' | 'low' のいずれかで指定してください。",
                    ));
                }
            },
            None => PriorityUpdate::Unchanged,
        };
        let title = arg_str(&args, "title");
        let description = arg_str(&args, "description");
        // due_date/start_date は空文字（クリア指示）も有効値として扱う（present 判定は raw）。
        let due_date = arg_raw_str(&args, "due_date");
        let start_date = arg_raw_str(&args, "start_date");

        if title.is_none()
            && description.is_none()
            && due_date.is_none()
            && start_date.is_none()
            && priority_arg.is_none()
            && status.is_none()
        {
            return Ok(fail_payload("変更する項目を1つ以上指定してください。"));
        }

        let update = TodoUpdate {
            id: todo_id,
            title,
            description,
            due_date,
            start_date,
            priority,
            status,
        };
        match TodoRepo::new(&self.db)
            .update(&scope_of(ctx), todo_id, update)
            .await
            .map_err(exec_err)?
        {
            Some(todo) => {
                let message = format!("ToDo「{}」(#{}) を更新しました📝", todo.title, todo.id);
                Ok(ToolOutcome::from_payload(ok_payload(
                    message,
                    json!({ "todo": todo }),
                )))
            }
            None => Ok(fail_payload(format!("ToDo #{todo_id} が見つかりません。"))),
        }
    }
}

// ─── updateTaskProgress ──────────────────────────────────────────────────────

struct UpdateTaskProgressTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for UpdateTaskProgressTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "タスクの進み具合（0〜100%）を更新し、メモを履歴に残す。\
                「○○は半分終わった」等の報告で呼ぶ。100にすると自動で完了になる。\
                サブタスクを持つ親タスクは進捗が自動計算されるため、ここでは更新できない。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "todo_id": { "type": "number", "description": "進捗を更新するタスクのID（#番号）" },
                    "progress": { "type": "number", "description": "進み具合 0〜100の整数（%）。100にすると完了扱い" },
                    "note": { "type": "string", "description": "進捗メモ（履歴に残る・任意）" }
                },
                "required": ["todo_id", "progress"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // Node `typeof args.progress === "number"`: JSON 数値（整数/小数）のみ・文字列は不可。
        let progress = args
            .get("progress")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)));
        let (Some(todo_id), Some(progress)) = (arg_i64(&args, "todo_id"), progress) else {
            return Ok(fail_payload(
                "todo_id と progress（0〜100の数値）を指定してください。",
            ));
        };
        let scope = scope_of(ctx);
        let repo = TodoRepo::new(&self.db);
        let Some(todo) = repo.get(&scope, todo_id).await.map_err(exec_err)? else {
            return Ok(fail_payload(format!(
                "タスク #{todo_id} が見つかりません。"
            )));
        };
        // サブタスクを持つ親は進捗が自動算出のため弾く（Node パリティ）。
        let subtasks = repo
            .list_subtasks_tree(&scope, todo_id)
            .await
            .map_err(exec_err)?;
        if !subtasks.is_empty() {
            let done = subtasks.iter().filter(|s| s.status == "done").count();
            return Ok(fail_payload(format!(
                "タスク#{}「{}」はサブタスクを {} 件持つため、進捗は『完了サブタスク数/全体』(現在 {}/{}) で自動算出されます。該当サブタスクを完了/進捗更新してください。",
                todo_id, todo.title, subtasks.len(), done, subtasks.len(),
            )));
        }
        let note = arg_str(&args, "note");
        match repo
            .update_progress(&scope, todo_id, progress, note.clone())
            .await
            .map_err(exec_err)?
        {
            Some(updated) => {
                let done_suffix = if updated.status == "done" {
                    "（完了にしました✅）"
                } else {
                    ""
                };
                let note_suffix = note.map(|n| format!("（メモ: {n}）")).unwrap_or_default();
                let message = format!(
                    "タスク「{}」(#{}) の進捗を {}% に更新しました📊{note_suffix}{done_suffix}",
                    updated.title, updated.id, updated.progress,
                );
                Ok(ToolOutcome::from_payload(ok_payload(
                    message,
                    json!({ "todo": updated }),
                )))
            }
            None => Ok(fail_payload(format!(
                "タスク #{todo_id} の進捗更新に失敗しました。"
            ))),
        }
    }
}

// ─── getTaskDetail ───────────────────────────────────────────────────────────

struct GetTaskDetailTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for GetTaskDetailTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "1つのタスクの詳しい中身を取り出す（サブタスク一覧・計算後の進捗・進捗の更新履歴）。\
                「#3の進捗の経緯を見せて」等の依頼で呼ぶ。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "todo_id": { "type": "number", "description": "詳しく見るタスクのID（#番号）" }
                },
                "required": ["todo_id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(todo_id) = arg_i64(&args, "todo_id") else {
            return Ok(fail_payload("todo_id を指定してください。"));
        };
        let scope = scope_of(ctx);
        let repo = TodoRepo::new(&self.db);
        let Some(todo) = repo.get(&scope, todo_id).await.map_err(exec_err)? else {
            return Ok(fail_payload(format!(
                "タスク #{todo_id} が見つかりません。"
            )));
        };
        let subtasks = repo
            .list_subtasks_tree(&scope, todo_id)
            .await
            .map_err(exec_err)?;
        let logs = repo
            .list_progress_logs(&scope, todo_id)
            .await
            .map_err(exec_err)?;
        let eff = effective_progress(&todo, &subtasks);
        let done = subtasks.iter().filter(|s| s.status == "done").count();
        let message = format!(
            "タスク「{}」(#{}) の詳細です。進捗 {}%、サブタスク {}/{} 完了、進捗履歴 {} 件。",
            todo.title,
            todo.id,
            eff,
            done,
            subtasks.len(),
            logs.len(),
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({
                "todo": todo,
                "effective_progress": eff,
                "subtasks": subtasks,
                "progress_logs": logs,
            }),
        )))
    }
}

// ─── listTodoTags ────────────────────────────────────────────────────────────

struct ListTodoTagsTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListTodoTagsTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "未完了タスクに付いているタグの一覧と、それぞれの件数を取り出す。\
                「どんなタグがある？」等で呼ぶ。listTodos でタグ絞り込みに使うタグ名の確認にも使う。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let all = TodoRepo::new(&self.db)
            .open_tags(&scope_of(ctx))
            .await
            .map_err(exec_err)?;
        // タグごとの件数を数え、件数降順→タグ名昇順で安定ソート。
        let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for tag in all {
            *counts.entry(tag).or_insert(0) += 1;
        }
        if counts.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                "タグの付いた未完了ToDoはありません。",
                json!({ "tags": [] }),
            )));
        }
        let mut tags: Vec<(String, i64)> = counts.into_iter().collect();
        tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let lines: Vec<String> = tags
            .iter()
            .map(|(t, c)| format!("🏷️ {t} ({c}件)"))
            .collect();
        let payload: Vec<Value> = tags
            .iter()
            .map(|(t, c)| json!({ "tag": t, "count": c }))
            .collect();
        Ok(ToolOutcome::from_payload(ok_payload(
            format!(
                "タグ一覧 ({}種類):\n{}\n特定タグのToDoは listTodos の tag 引数で絞り込めます。",
                tags.len(),
                lines.join("\n")
            ),
            json!({ "tags": payload }),
        )))
    }
}

// ─── listTasksByTag ──────────────────────────────────────────────────────────

struct ListTasksByTagTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListTasksByTagTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "タスクをタグ（グループ）ごとにまとめて取り出す。\
                「タグごとにまとめて」等で呼ぶ。1つのタスクが複数タグを持つ場合は各グループに現れる。\
                タグの付いていないタスクは『未分類』グループにまとまる。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "'open'（既定）|'done'|'all'" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // status: open（既定）/done は絞り込み、all は無フィルタ。
        let status = match args.get("status").and_then(Value::as_str) {
            Some("done") => Some("done".to_owned()),
            Some("all") => None,
            _ => Some("open".to_owned()),
        };
        let todos = TodoRepo::new(&self.db)
            .list_tree(&scope_of(ctx), status, None)
            .await
            .map_err(exec_err)?;
        if todos.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                "該当するToDoはありません。",
                json!({ "groups": [] }),
            )));
        }
        const UNTAGGED: &str = "未分類";
        // タグ→そのタグを持つトップレベル ToDo。タグ無しは「未分類」。
        let mut buckets: std::collections::HashMap<String, Vec<&crate::dto::TodoWithSubtasks>> =
            std::collections::HashMap::new();
        for todo in &todos {
            let keys: Vec<String> = if todo.tags.is_empty() {
                vec![UNTAGGED.to_owned()]
            } else {
                todo.tags.clone()
            };
            for key in keys {
                buckets.entry(key).or_default().push(todo);
            }
        }
        // 未分類は末尾・それ以外は件数降順→タグ名昇順。
        let mut groups: Vec<(String, Vec<&crate::dto::TodoWithSubtasks>)> =
            buckets.into_iter().collect();
        groups.sort_by(|a, b| match (a.0 == UNTAGGED, b.0 == UNTAGGED) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)),
        });
        let lines: Vec<String> = groups
            .iter()
            .map(|(tag, items)| format!("🏷️ {tag} ({}件)", items.len()))
            .collect();
        let payload: Vec<Value> = groups
            .iter()
            .map(|(tag, items)| json!({ "tag": tag, "items": items }))
            .collect();
        Ok(ToolOutcome::from_payload(ok_payload(
            format!("タグ別のToDo:\n{}", lines.join("\n")),
            json!({ "groups": payload }),
        )))
    }
}

// ─── getTaskUsageGuide ───────────────────────────────────────────────────────

struct GetTaskUsageGuideTool {
    name: ToolName,
}

#[async_trait]
impl Tool for GetTaskUsageGuideTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "タスク管理機能の「使い方」を案内する。「タスクの使い方教えて」等で呼ぶ。\
                返る guide_markdown（使い方本文）と guide_url（詳しい説明ページ）を必ず両方ユーザーに伝える。\
                本文はあなた自身の口調・人格に合わせて自然に言い換えて返すこと（要点は省略しない）。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, _ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        // guide_url は BASE_URL 未取得（tools は config 非依存）のためサイト相対を返す（Node の
        // baseUrl 未設定時のフォールバックと一致）。
        Ok(ToolOutcome::from_payload(ok_payload(
            "タスク機能の使い方ガイドです。guide_markdown の内容を、あなた自身の口調・人格に合わせて自然に言い換えてユーザーに伝え、最後に guide_url のリンクも必ず案内してください（要点は省略しないこと）。",
            json!({ "guide_markdown": TASK_USAGE_GUIDE_MD, "guide_url": "/tasks/guide" }),
        )))
    }
}

// ─── editTodoTags ────────────────────────────────────────────────────────────

struct EditTodoTagsTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for EditTodoTagsTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "タスクのタグを手動で直す。mode で 'set'（丸ごと置換・既定）/'add'（追加）\
                /'remove'（指定タグを外す）を選ぶ。tags にはタグ名を配列で渡す（例: ['業務','緊急']）。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "todo_id": { "type": "number", "description": "タグを直すタスクのID（#番号）" },
                    "mode": { "type": "string", "description": "'set'（既定）|'add'|'remove'" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "対象のタグ名（配列）" }
                },
                "required": ["todo_id", "tags"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(todo_id) = arg_i64(&args, "todo_id") else {
            return Ok(fail_payload("todo_id（#番号）を指定してください。"));
        };
        let mode = arg_str(&args, "mode").unwrap_or_else(|| "set".to_owned());
        if mode != "set" && mode != "add" && mode != "remove" {
            return Ok(fail_payload(
                "mode は 'set' | 'add' | 'remove' のいずれかで指定してください。",
            ));
        }
        let input_tags = normalize_tags(args.get("tags").unwrap_or(&Value::Null));
        if mode != "set" && input_tags.is_empty() {
            return Ok(fail_payload(format!(
                "{mode} には tags を1つ以上指定してください。"
            )));
        }
        let scope = scope_of(ctx);
        let repo = TodoRepo::new(&self.db);
        let Some(todo) = repo.get(&scope, todo_id).await.map_err(exec_err)? else {
            return Ok(fail_payload(format!(
                "タスク #{todo_id} が見つかりません。"
            )));
        };
        let current = todo.tags;
        let next: Vec<String> = match mode.as_str() {
            "set" => input_tags,
            "add" => {
                // 既存 + 入力を再正規化（重複除去・最大 8）。
                let merged: Vec<Value> = current
                    .iter()
                    .chain(input_tags.iter())
                    .map(|t| Value::String(t.clone()))
                    .collect();
                normalize_tags(&Value::Array(merged))
            }
            _ => {
                let remove: std::collections::HashSet<&String> = input_tags.iter().collect();
                current
                    .into_iter()
                    .filter(|t| !remove.contains(t))
                    .collect()
            }
        };
        match repo
            .update_tags(&scope, todo_id, next)
            .await
            .map_err(exec_err)?
        {
            Some(updated) => {
                let tag_label = if updated.tags.is_empty() {
                    "（タグなし）".to_owned()
                } else {
                    updated.tags.join("、")
                };
                Ok(ToolOutcome::from_payload(ok_payload(
                    format!(
                        "ToDo「{}」(#{}) のタグを更新しました🏷️ {tag_label}",
                        updated.title, updated.id
                    ),
                    json!({ "todo": updated }),
                )))
            }
            None => Ok(fail_payload(format!(
                "タスク #{todo_id} のタグ更新に失敗しました。"
            ))),
        }
    }
}

// ─── stopTodoRoutine ─────────────────────────────────────────────────────────

struct StopTodoRoutineTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for StopTodoRoutineTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "ルーチン（繰り返し）タスクの繰り返しを終了する。「もう毎週の○○はやらなくていい」\
                等で呼ぶ。タスク自体は消えず、今ある1件は単発タスクとして残る（完全削除は deleteTodo）。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "todo_id": { "type": "number", "description": "繰り返しを終了するタスクのID（#番号）" }
                },
                "required": ["todo_id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(todo_id) = arg_i64(&args, "todo_id") else {
            return Ok(fail_payload("todo_id（#番号）を指定してください。"));
        };
        let scope = scope_of(ctx);
        let repo = TodoRepo::new(&self.db);
        match repo.stop_routine(&scope, todo_id).await.map_err(exec_err)? {
            Some(stopped) => Ok(ToolOutcome::from_payload(ok_payload(
                format!(
                    "タスク「{}」(#{}) の繰り返しを終了しました🏁（このタスクは単発として残ります）",
                    stopped.title, stopped.id
                ),
                json!({ "todo": stopped }),
            ))),
            None => {
                // 対象なし = 存在しない or 既にルーチンでない（Node パリティで区別）。
                let msg = if repo.get(&scope, todo_id).await.map_err(exec_err)?.is_some() {
                    format!("タスク #{todo_id} はルーチン（繰り返し）ではありません。")
                } else {
                    format!("タスク #{todo_id} が見つかりません。")
                };
                Ok(fail_payload(msg))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use yuuka_core::{BotId, UserId};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    const TODOS_DDL: &str = "CREATE TABLE todos (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        title TEXT NOT NULL, description TEXT, due_date TEXT, start_date TEXT,
        priority TEXT, tags TEXT NOT NULL DEFAULT '[]', status TEXT NOT NULL DEFAULT 'open',
        progress INTEGER NOT NULL DEFAULT 0, parent_id INTEGER, linked_payment_id INTEGER,
        due_reminded INTEGER NOT NULL DEFAULT 0, repeat_rule TEXT, repeat_until TEXT,
        repeat_count INTEGER, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);";

    // task_progress_logs を FK 無しで先に作る（V17 の users/todos FK を避ける・lib.rs テストと同方針）。
    const PROGRESS_LOGS_DDL: &str = "CREATE TABLE task_progress_logs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        todo_id INTEGER NOT NULL,
        progress INTEGER NOT NULL,
        note TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_todo_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(TODOS_DDL).unwrap();
            conn.execute_batch(PROGRESS_LOGS_DDL).unwrap();
        }
        Db::open(&path).unwrap()
    }

    fn ctx() -> ToolContext {
        ToolContext::new(BotId::system_default(), UserId::new("userA"))
    }

    fn find<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> &'a Arc<dyn Tool> {
        tools
            .iter()
            .find(|t| t.declaration().name.as_str() == name)
            .unwrap()
    }

    #[tokio::test]
    async fn add_list_complete_delete_roundtrip() {
        let db = seed_db();
        let tools = tools(db).unwrap();

        // 宣言名は bare（Node system prompt と一致）。
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.declaration().name.to_string())
            .collect();
        assert!(names.contains(&"addTodo".to_owned()));
        assert!(!names.iter().any(|n| n.contains(':')), "native は bare 名");

        // add（snake_case 引数）。
        let add = find(&tools, "addTodo");
        let out = add
            .call(&ctx(), json!({"title": "牛乳を買う", "priority": "high"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["todo"]["title"], "牛乳を買う");
        assert_eq!(out.payload["todo"]["priority"], "high");
        let id = out.payload["todo"]["id"].as_i64().unwrap();

        // list（open 既定）で 1 件見える。
        let list = find(&tools, "listTodos");
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["todos"].as_array().unwrap().len(), 1);

        // complete。
        let complete = find(&tools, "completeTodo");
        let out = complete.call(&ctx(), json!({"todo_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        // open 絞り込みでは 0 件、done では 1 件。
        let out = list.call(&ctx(), json!({"status": "open"})).await.unwrap();
        assert_eq!(out.payload["todos"].as_array().unwrap().len(), 0);
        let out = list.call(&ctx(), json!({"status": "done"})).await.unwrap();
        assert_eq!(out.payload["todos"].as_array().unwrap().len(), 1);

        // delete。
        let delete = find(&tools, "deleteTodo");
        let out = delete.call(&ctx(), json!({"todo_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        // 二重削除は not-found（success:false）。
        let out = delete.call(&ctx(), json!({"todo_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn add_validates_title_and_routine() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addTodo");

        // title 欠落 → fail。
        let out = add.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // repeat_rule だけで due_date 無し → fail。
        let out = add
            .call(&ctx(), json!({"title": "x", "repeat_rule": "0 9 * * 1"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // repeat_rule + due_date → 成功し repeat 列が入る。
        let out = add
            .call(
                &ctx(),
                json!({"title": "毎週会議", "repeat_rule": "0 9 * * 1", "due_date": "2026-07-13"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["todo"]["repeat_rule"], "0 9 * * 1");
    }

    #[tokio::test]
    async fn scope_isolation_across_users() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addTodo");
        let list = find(&tools, "listTodos");

        add.call(&ctx(), json!({"title": "A のタスク"}))
            .await
            .unwrap();

        // 別ユーザーには見えない。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["todos"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn subtask_update_progress_and_detail_flow() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addTodo");
        let add_sub = find(&tools, "addSubtask");
        let update = find(&tools, "updateTodo");
        let progress = find(&tools, "updateTaskProgress");
        let detail = find(&tools, "getTaskDetail");

        // 親タスク作成。
        let out = add.call(&ctx(), json!({"title": "設計"})).await.unwrap();
        let parent_id = out.payload["todo"]["id"].as_i64().unwrap();

        // サブタスク追加（親の下に付く）。
        let out = add_sub
            .call(
                &ctx(),
                json!({"parent_todo_id": parent_id, "title": "要件定義"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["parent_todo_id"], parent_id);
        let sub_id = out.payload["subtask"]["id"].as_i64().unwrap();
        // 存在しない親 → fail。
        let out = add_sub
            .call(&ctx(), json!({"parent_todo_id": 99999, "title": "x"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // updateTodo: タイトル変更は成功、無変更は fail、不正 priority も fail。
        let out = update
            .call(
                &ctx(),
                json!({"todo_id": sub_id, "title": "要件定義（改）"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["todo"]["title"], "要件定義（改）");
        let out = update
            .call(&ctx(), json!({"todo_id": sub_id}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        let out = update
            .call(&ctx(), json!({"todo_id": sub_id, "priority": "urgent"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // updateTaskProgress: 葉サブタスクは更新可（100 で完了）。
        let out = progress
            .call(
                &ctx(),
                json!({"todo_id": sub_id, "progress": 100, "note": "済"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["todo"]["status"], "done");
        // 親はサブタスクを持つため進捗更新は弾かれる。
        let out = progress
            .call(&ctx(), json!({"todo_id": parent_id, "progress": 50}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // getTaskDetail: 親の effective_progress はサブ 1/1 完了 = 100。
        let out = detail
            .call(&ctx(), json!({"todo_id": parent_id}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["effective_progress"], 100);
        assert_eq!(out.payload["subtasks"].as_array().unwrap().len(), 1);
        // 存在しないタスクの detail → fail。
        let out = detail
            .call(&ctx(), json!({"todo_id": 99999}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn tag_tools_and_usage_guide() {
        let db = seed_db();
        let seed = db.clone();
        let tools = tools(db).unwrap();
        let list_tags = find(&tools, "listTodoTags");
        let by_tag = find(&tools, "listTasksByTag");
        let guide = find(&tools, "getTaskUsageGuide");

        // タグ付き ToDo を repo 直挿し（addTodo ツールは tags=[]・自動タグは deferred のため）。
        let scope = scope_of(&ctx());
        let repo = TodoRepo::new(&seed);
        for (title, tags) in [
            ("買い物", vec!["家事".to_owned(), "緊急".to_owned()]),
            ("掃除", vec!["家事".to_owned()]),
        ] {
            repo.add(
                &scope,
                NewTodo {
                    title: title.to_owned(),
                    description: None,
                    due_date: None,
                    start_date: None,
                    priority: None,
                    tags,
                    parent_id: None,
                    repeat_rule: None,
                    repeat_until: None,
                    repeat_count: None,
                },
            )
            .await
            .unwrap();
        }

        // listTodoTags: 家事×2, 緊急×1（件数降順）。
        let out = list_tags.call(&ctx(), json!({})).await.unwrap();
        let tags = out.payload["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0]["tag"], "家事");
        assert_eq!(tags[0]["count"], 2);

        // listTasksByTag: 家事(2)が先頭・緊急(1)。
        let out = by_tag.call(&ctx(), json!({})).await.unwrap();
        let groups = out.payload["groups"].as_array().unwrap();
        assert_eq!(groups[0]["tag"], "家事");
        assert_eq!(groups[0]["items"].as_array().unwrap().len(), 2);

        // getTaskUsageGuide: 本文とリンクを返す。
        let out = guide.call(&ctx(), json!({})).await.unwrap();
        assert!(out.payload["guide_markdown"]
            .as_str()
            .unwrap()
            .contains("タスク管理の使い方"));
        assert_eq!(out.payload["guide_url"], "/tasks/guide");
    }

    #[tokio::test]
    async fn edit_tags_and_stop_routine() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addTodo");
        let edit_tags = find(&tools, "editTodoTags");
        let stop_routine = find(&tools, "stopTodoRoutine");

        // タスク作成（tags=[]）。
        let out = add.call(&ctx(), json!({"title": "掃除"})).await.unwrap();
        let id = out.payload["todo"]["id"].as_i64().unwrap();

        // set: 丸ごと置換（重複は正規化で1つに）。
        let out = edit_tags
            .call(
                &ctx(),
                json!({"todo_id": id, "tags": ["家事", "家事", "急ぎ"]}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["todo"]["tags"], json!(["家事", "急ぎ"]));
        // add: 追加。
        let out = edit_tags
            .call(
                &ctx(),
                json!({"todo_id": id, "mode": "add", "tags": ["週末"]}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["todo"]["tags"], json!(["家事", "急ぎ", "週末"]));
        // remove: 指定タグを外す。
        let out = edit_tags
            .call(
                &ctx(),
                json!({"todo_id": id, "mode": "remove", "tags": ["急ぎ"]}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["todo"]["tags"], json!(["家事", "週末"]));
        // 不正 mode → fail。
        let out = edit_tags
            .call(&ctx(), json!({"todo_id": id, "mode": "x", "tags": []}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        // add で tags 空 → fail。
        let out = edit_tags
            .call(&ctx(), json!({"todo_id": id, "mode": "add", "tags": []}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // 単発タスクへの stopRoutine → fail（ルーチンではない）。
        let out = stop_routine
            .call(&ctx(), json!({"todo_id": id}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // ルーチンタスクを作って停止。
        let out = add
            .call(
                &ctx(),
                json!({"title": "毎週報告", "repeat_rule": "0 9 * * 1", "due_date": "2026-08-03"}),
            )
            .await
            .unwrap();
        let rid = out.payload["todo"]["id"].as_i64().unwrap();
        let out = stop_routine
            .call(&ctx(), json!({"todo_id": rid}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert!(out.payload["todo"]["repeat_rule"].is_null());
        // 停止後は単発なので再度 stop は fail。
        let out = stop_routine
            .call(&ctx(), json!({"todo_id": rid}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        // 存在しない ID。
        let out = stop_routine
            .call(&ctx(), json!({"todo_id": 99999}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }
}
