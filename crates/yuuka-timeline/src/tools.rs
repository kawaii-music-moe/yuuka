//! timeline ドメインの Native ツール（現行 `src/functions/timelineFunctions.ts` の移植・§9.4）。
//!
//! **雛形**: `yuuka-todo::tools`（構造・命名・ok/fail JSON・bare ツール名・UserScope・
//! DbError→`ToolError::Execution`・`tools(db) -> Result<Vec<Arc<dyn Tool>>>` を踏襲）。
//!
//! **wire 契約の非対称**: HTTP route の body は camelCase（`recordedAt`）だが、**tool 引数は
//! snake_case**（Node の Gemini 宣言と一致）。ツール名は Node system prompt が参照する **bare 名**。
//!
//! 移植済み: addTimelineRecord（`TimelineRepo::add` に対応するプレーン記録＝memo/task_done/location）。
//! これは routes.rs / dto.rs（M-8）が凍結した「timeline_records のコア INSERT」に一致する。
//!
//! 未移植（対応 repo が無いため deferred）:
//! - `listDayPlan` / `createDayPlanBlock` / `deleteDayPlanBlock`: 別表 `day_plan_blocks` を操作する
//!   （`addDayPlanBlock`/`listDayPlanBlocks`/`deleteDayPlanBlock`）。当クレートに repo が無い。
//!   `TimelineRepo::delete` は `timeline_records` の削除であり block 削除ではないため流用しない。
//! - addTimelineRecord の cross-domain 副作用（`type=expense` の家計簿二重登録＝finance
//!   `addExpenseRecord`／`type=media` の `saveMediaFile`／`type=task_done` の todos `completeTodo`）は
//!   他ドメイン依存のため落とす。プレーン記録（todo_id 参照の passthrough 含む）のみを記録する。
//!   支出の記録は finance の `addExpense` ツールを使う。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope};
use yuuka_web::Db;

use crate::dto::NewTimelineRecord;
use crate::repo::TimelineRepo;

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![Arc::new(AddTimelineRecordTool {
        name: ToolName::checked("addTimelineRecord".to_owned())?,
        db,
    })])
}

// ─── 共通ヘルパ（todo/tools.rs と同一規約） ──────────────────────────────────────

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

/// 数値（`Math.trunc` 相当）または数値文字列を i64 へ（Gemini は number を f64 で送りうる）。
fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    let v = args.get(key)?;
    if let Some(i) = v.as_i64() {
        return Some(i);
    }
    if let Some(f) = v.as_f64() {
        if f.is_finite() {
            return Some(f.trunc() as i64);
        }
    }
    v.as_str().and_then(|s| s.trim().parse::<i64>().ok())
}

/// DbError をツール実行エラーへ（握り潰さず Gemini へ `{success:false}` として返る・§8.4）。
fn exec_err(e: DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

// ─── addTimelineRecord ───────────────────────────────────────────────────────

struct AddTimelineRecordTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for AddTimelineRecordTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "その日の出来事・記録をタイムラインに1件追加する（メモ・タスク完了・場所チェックイン等）。\
                type は 'memo'（テキストメモ）/'task_done'（タスク完了の記録。todo_id で対象を指し示す）/\
                'location'（場所・チェックイン）。支出の記録は addExpense を使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "日付 'YYYY-MM-DD'" },
                    "type": { "type": "string", "description": "'memo' | 'task_done' | 'location'" },
                    "title": { "type": "string", "description": "見出し（任意）" },
                    "content": { "type": "string", "description": "本文・メモ（任意）" },
                    "recorded_at": { "type": "string", "description": "記録日時 'YYYY-MM-DD HH:MM:SS'（省略=現在時刻）（任意）" },
                    "todo_id": { "type": "number", "description": "type='task_done' の時、関連づけるタスクID（任意）" },
                    "location": { "type": "string", "description": "場所名（任意）" }
                },
                "required": ["date", "type"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // Node: `typeof date !== "string" || typeof type !== "string"` → 必須エラー。
        // arg_str は trim 後空も None にする（route の date/type 空文字検証と一致）。
        let (Some(date), Some(r#type)) = (arg_str(&args, "date"), arg_str(&args, "type")) else {
            return Ok(fail_payload("date / type は必須です。"));
        };

        // 内部列（expense_id/expense_category/media_path/media_type）は DTO に存在せず設定不可（M-8）。
        // expense 二重登録・media 保存・task_done の todo 完了は cross-domain のため deferred。
        let new = NewTimelineRecord {
            date,
            r#type,
            recorded_at: arg_str(&args, "recorded_at"),
            title: arg_str(&args, "title"),
            content: arg_str(&args, "content"),
            todo_id: arg_i64(&args, "todo_id"),
            // amount は expense ブランチ専用（deferred）。プレーン記録では書かない（Node parity）。
            amount: None,
            location: arg_str(&args, "location"),
        };

        let record = TimelineRepo::new(&self.db)
            .add(&scope_of(ctx), new)
            .await
            .map_err(exec_err)?;

        Ok(ToolOutcome::from_payload(ok_payload(
            "記録しました。",
            json!({ "record": record }),
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use yuuka_core::{BotId, UserId};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // lib.rs の #[cfg(test)] と同一の実効スキーマ（timeline_records）。
    const RECORDS_DDL: &str = "CREATE TABLE timeline_records (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        date TEXT NOT NULL,
        recorded_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
        type TEXT NOT NULL DEFAULT 'memo',
        title TEXT,
        content TEXT,
        todo_id INTEGER,
        expense_id INTEGER,
        amount REAL,
        expense_category TEXT,
        media_path TEXT,
        media_type TEXT,
        location TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_timeline_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(RECORDS_DDL).unwrap();
        }
        Db::open(&path).unwrap()
    }

    fn ctx() -> ToolContext {
        ToolContext::new(BotId::system_default(), UserId::new("userA"))
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    fn find<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> &'a Arc<dyn Tool> {
        tools
            .iter()
            .find(|t| t.declaration().name.as_str() == name)
            .unwrap()
    }

    #[tokio::test]
    async fn add_records_and_persists() {
        let db = seed_db();
        let tools = tools(db.clone()).unwrap();

        // 宣言名は bare（Node system prompt と一致）。
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.declaration().name.to_string())
            .collect();
        assert!(names.contains(&"addTimelineRecord".to_owned()));
        assert!(!names.iter().any(|n| n.contains(':')), "native は bare 名");

        // add（snake_case 引数）。プレーン記録（memo）。
        let add = find(&tools, "addTimelineRecord");
        let out = add
            .call(
                &ctx(),
                json!({"date": "2026-07-06", "type": "memo", "title": "朝ごはん", "content": "パン"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["message"], "記録しました。");
        assert_eq!(out.payload["record"]["title"], "朝ごはん");
        assert_eq!(out.payload["record"]["type"], "memo");
        // 内部列は露出しない（クリーンビュー）。
        assert!(out.payload["record"]["user_id"].is_null());
        assert!(out.payload["record"]["bot_id"].is_null());

        // repo 経由で往復確認（当日リストに 1 件見える）。
        let listed = TimelineRepo::new(&db)
            .list(&scope("userA"), Some("2026-07-06".to_owned()))
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title.as_deref(), Some("朝ごはん"));
    }

    #[tokio::test]
    async fn task_done_passes_todo_id_without_completing_todo() {
        // task_done の todo 完了連携は deferred。プレーン記録として todo_id を passthrough する。
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addTimelineRecord");

        let out = add
            .call(
                &ctx(),
                json!({"date": "2026-07-06", "type": "task_done", "todo_id": 42}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["record"]["type"], "task_done");
        assert_eq!(out.payload["record"]["todo_id"], 42);
    }

    #[tokio::test]
    async fn add_validates_date_and_type() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addTimelineRecord");

        // date 欠落 → fail。
        let out = add.call(&ctx(), json!({"type": "memo"})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // type 欠落 → fail。
        let out = add.call(&ctx(), json!({"date": "2026-07-06"})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // 空文字 date → fail（route の空文字検証と一致）。
        let out = add
            .call(&ctx(), json!({"date": "  ", "type": "memo"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn scope_isolation_across_users() {
        let db = seed_db();
        let tools = tools(db.clone()).unwrap();
        let add = find(&tools, "addTimelineRecord");

        add.call(
            &ctx(),
            json!({"date": "2026-07-06", "type": "memo", "title": "A の記録"}),
        )
        .await
        .unwrap();

        // 別ユーザーには見えない（分離キーを型で強制）。
        let listed = TimelineRepo::new(&db)
            .list(&scope("userB"), Some("2026-07-06".to_owned()))
            .await
            .unwrap();
        assert!(listed.is_empty());
    }
}
