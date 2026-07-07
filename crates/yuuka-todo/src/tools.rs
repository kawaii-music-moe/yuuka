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

use crate::dto::{normalize_priority, NewTodo};
use crate::repo::TodoRepo;

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
            db,
        }),
    ])
}

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
            return Ok(ToolOutcome::from_payload(ok_payload(msg, json!({ "todos": [] }))));
        }

        let msg = format!(
            "ToDo一覧 (親タスク {}件{})",
            todos.len(),
            tag.as_ref().map(|t| format!("、タグ: {t}")).unwrap_or_default(),
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
            description: "指定した ToDo を完了にする。todo_id は listTodos で確認できる ID。".to_owned(),
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
            description: "指定した ToDo を削除する。todo_id は listTodos で確認できる ID。".to_owned(),
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

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("yuuka_todo_tools_{}_{seq}.sqlite", std::process::id()));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(TODOS_DDL).unwrap();
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
        let names: Vec<String> = tools.iter().map(|t| t.declaration().name.to_string()).collect();
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

        add.call(&ctx(), json!({"title": "A のタスク"})).await.unwrap();

        // 別ユーザーには見えない。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["todos"].as_array().unwrap().len(), 0);
    }
}
