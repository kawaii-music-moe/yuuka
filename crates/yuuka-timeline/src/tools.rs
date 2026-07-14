//! timeline ドメインの Native ツール（現行 `src/functions/timelineFunctions.ts` の移植・§9.4）。
//!
//! **雛形**: `yuuka-todo::tools`（構造・命名・ok/fail JSON・bare ツール名・UserScope・
//! DbError→`ToolError::Execution`・`tools(db) -> Result<Vec<Arc<dyn Tool>>>` を踏襲）。
//!
//! **wire 契約の非対称**: HTTP route の body は camelCase（`recordedAt`）だが、**tool 引数は
//! snake_case**（Node の Gemini 宣言と一致）。ツール名は Node system prompt が参照する **bare 名**。
//!
//! 移植済み: addTimelineRecord（プレーン記録 memo/location に加え、cross-domain 副作用も対応）。
//! - `type=expense`: `TimelineRepo::add_expense_record` で expenses へ二次登録し `expense_id` 連結
//!   （Node `¥{amount} を記録しました。`・amount 必須・category は `expense_category ?? "その他"`）。
//! - `type=task_done`: `TimelineRepo::add` がトランザクション内で紐付き todos を完了（Node `completeTodo`）。
//!
//! 未移植（対応 repo が無い／他ドメイン依存で deferred）:
//! - `listDayPlan` / `createDayPlanBlock` / `deleteDayPlanBlock`: 別表 `day_plan_blocks` を操作する
//!   （route 側は移植済みだが tool 宣言は未移植）。
//! - `type=media` の `saveMediaFile`（Discord 添付 URL 取得・保存）は media 機能（`/api/timeline/media*`）
//!   と同時に移植（deferred）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope};
use yuuka_web::Db;

use crate::dto::{NewDayPlanBlock, NewTimelineRecord};
use crate::repo::TimelineRepo;

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(AddTimelineRecordTool {
            name: ToolName::checked("addTimelineRecord".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(ListDayPlanTool {
            name: ToolName::checked("listDayPlan".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(CreateDayPlanBlockTool {
            name: ToolName::checked("createDayPlanBlock".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(DeleteDayPlanBlockTool {
            name: ToolName::checked("deleteDayPlanBlock".to_owned())?,
            db,
        }),
    ])
}

/// 計画ブロックの type 許容値（Node `'task'|'transit'|'event'|'free'`）。
const PLAN_BLOCK_TYPES: [&str; 4] = ["task", "transit", "event", "free"];

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

/// 数値または数値文字列を f64 へ（Node `Number(args.amount)` 相当・金額用）。
fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    let v = args.get(key)?;
    if let Some(f) = v.as_f64() {
        return Some(f);
    }
    v.as_str().and_then(|s| s.trim().parse::<f64>().ok())
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
            description: "その日の出来事・記録をタイムラインに1件追加する。\
                type は 'memo'（テキストメモ）/'expense'（支出。家計簿にも自動記録される。amount 必須）/\
                'task_done'（タスク完了。todo_id で対象を指すとそのタスクも同時に完了になる）/\
                'location'（場所・チェックイン）。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "日付 'YYYY-MM-DD'" },
                    "type": { "type": "string", "description": "'memo' | 'expense' | 'task_done' | 'location'" },
                    "title": { "type": "string", "description": "見出し（任意）" },
                    "content": { "type": "string", "description": "本文・メモ（任意）" },
                    "recorded_at": { "type": "string", "description": "記録日時 'YYYY-MM-DD HH:MM:SS'（省略=現在時刻）（任意）" },
                    "todo_id": { "type": "number", "description": "type='task_done' の時、完了するタスクID（任意）" },
                    "amount": { "type": "number", "description": "type='expense' の時、金額（円・必須）" },
                    "expense_category": { "type": "string", "description": "type='expense' の時、カテゴリ（食費/交通費/娯楽 等・省略時は「その他」）" },
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

        let repo = TimelineRepo::new(&self.db);
        let scope = scope_of(ctx);

        // type=expense: 家計簿（expenses）へ二次登録し expense_id を連結（Node addExpenseRecord）。
        if r#type == "expense" {
            let amount = match arg_f64(&args, "amount") {
                Some(a) if a != 0.0 => a,
                _ => return Ok(fail_payload("expense には amount が必要です。")),
            };
            let category =
                arg_str(&args, "expense_category").unwrap_or_else(|| "その他".to_owned());
            let input = NewTimelineRecord {
                date,
                r#type,
                recorded_at: arg_str(&args, "recorded_at"),
                title: arg_str(&args, "title"),
                content: None,
                todo_id: None,
                amount: None,
                category: None,
                location: arg_str(&args, "location"),
            };
            let record = repo
                .add_expense_record(&scope, &input, amount, category)
                .await
                .map_err(exec_err)?;
            // Node: `¥${amount} を記録しました。`（整数は "1500"・小数は "1500.5" と JS Number 表示に一致）。
            return Ok(ToolOutcome::from_payload(ok_payload(
                format!("¥{amount} を記録しました。"),
                json!({ "record": record }),
            )));
        }

        // memo/location/task_done: プレーン記録。task_done + todo_id は repo.add が todos を完了する。
        // 内部列（expense_id/expense_category/media_path/media_type）は DTO に存在せず設定不可（M-8）。
        // media 保存（Discord 添付）は deferred。
        let new = NewTimelineRecord {
            date,
            r#type,
            recorded_at: arg_str(&args, "recorded_at"),
            title: arg_str(&args, "title"),
            content: arg_str(&args, "content"),
            todo_id: arg_i64(&args, "todo_id"),
            amount: None,
            category: None,
            location: arg_str(&args, "location"),
        };

        let record = repo.add(&scope, new).await.map_err(exec_err)?;

        Ok(ToolOutcome::from_payload(ok_payload(
            "記録しました。",
            json!({ "record": record }),
        )))
    }
}

// ─── listDayPlan ─────────────────────────────────────────────────────────────

struct ListDayPlanTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListDayPlanTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "指定日のタイムライン（計画ブロック + 記録）を取得する。\
                「今日の予定は？」等で呼ぶ。date を省くと今日。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "日付 'YYYY-MM-DD'。省略すると今日" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let scope = scope_of(ctx);
        let repo = TimelineRepo::new(&self.db);
        let date = arg_str(&args, "date");
        let blocks = repo
            .list_plans(&scope, date.clone())
            .await
            .map_err(exec_err)?;
        let records = repo.list(&scope, date.clone()).await.map_err(exec_err)?;
        // 表示用ラベル（date 省略時は "今日" と示す・実データは DB の date('now')）。
        let label = date.clone().unwrap_or_else(|| "今日".to_owned());
        if blocks.is_empty() && records.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                format!("{label} の計画・記録はまだありません。"),
                json!({ "date": date, "blocks": [], "records": [] }),
            )));
        }
        Ok(ToolOutcome::from_payload(ok_payload(
            format!(
                "{label} の計画 {} 件・記録 {} 件です。",
                blocks.len(),
                records.len()
            ),
            json!({ "date": date, "blocks": blocks, "records": records }),
        )))
    }
}

// ─── createDayPlanBlock ──────────────────────────────────────────────────────

struct CreateDayPlanBlockTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for CreateDayPlanBlockTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description:
                "タイムラインに計画ブロックを1件追加する（その日の予定の枠）。\
                type は 'task'|'transit'|'event'|'free'。start_time/end_time で時間帯を指定できる。"
                    .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "日付 'YYYY-MM-DD'" },
                    "type": { "type": "string", "description": "'task'|'transit'|'event'|'free'" },
                    "title": { "type": "string", "description": "ブロックのタイトル" },
                    "start_time": { "type": "string", "description": "開始時刻 'HH:MM'（任意）" },
                    "end_time": { "type": "string", "description": "終了時刻 'HH:MM'（任意）" },
                    "description": { "type": "string", "description": "補足メモ（任意）" },
                    "todo_id": { "type": "number", "description": "type='task' の時、紐付けるタスクID（任意）" },
                    "transit_from": { "type": "string", "description": "出発地（type='transit' 時・任意）" },
                    "transit_to": { "type": "string", "description": "到着地（type='transit' 時・任意）" },
                    "transit_line": { "type": "string", "description": "路線・交通機関名（任意）" }
                },
                "required": ["date", "type", "title"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let (Some(date), Some(block_type), Some(title)) = (
            arg_str(&args, "date"),
            arg_str(&args, "type"),
            arg_str(&args, "title"),
        ) else {
            return Ok(fail_payload("date / type / title は必須です。"));
        };
        // type を許容値へ正規化（不正値は 'free' へ・Node パリティの安全側）。
        let block_type = if PLAN_BLOCK_TYPES.contains(&block_type.as_str()) {
            block_type
        } else {
            "free".to_owned()
        };
        let input = NewDayPlanBlock {
            date,
            r#type: block_type,
            title: title.clone(),
            description: arg_str(&args, "description"),
            start_time: arg_str(&args, "start_time"),
            end_time: arg_str(&args, "end_time"),
            todo_id: arg_i64(&args, "todo_id"),
            transit_from: arg_str(&args, "transit_from"),
            transit_to: arg_str(&args, "transit_to"),
            transit_line: arg_str(&args, "transit_line"),
            position: None,
        };
        let block = TimelineRepo::new(&self.db)
            .add_plan(&scope_of(ctx), input)
            .await
            .map_err(exec_err)?;
        Ok(ToolOutcome::from_payload(ok_payload(
            format!("計画ブロック「{title}」を追加しました。"),
            json!({ "block": block }),
        )))
    }
}

// ─── deleteDayPlanBlock ──────────────────────────────────────────────────────

struct DeleteDayPlanBlockTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for DeleteDayPlanBlockTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "タイムラインの計画ブロックを削除する。".to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "number", "description": "削除するブロックのID" }
                },
                "required": ["id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(id) = arg_i64(&args, "id") else {
            return Ok(fail_payload("id が必要です。"));
        };
        let ok = TimelineRepo::new(&self.db)
            .delete_plan(&scope_of(ctx), id)
            .await
            .map_err(exec_err)?;
        let message = if ok {
            "削除しました。"
        } else {
            "ブロックが見つかりません。"
        };
        Ok(ToolOutcome::from_payload(if ok {
            ok_payload(message, json!({}))
        } else {
            json!({ "success": false, "message": message })
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use yuuka_core::{BotId, UserId};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // 実効スキーマは `Db::open`（= run_migrations）が V17__baseline.sql を丸ごと適用して作る
    // （timeline_records + cross-domain の expenses/todos を含む・部分 DDL は idx_todos_parent と
    // 衝突するため使わない・lib.rs テストと同方針）。
    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_timeline_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            drop(conn);
        }
        let db = Db::open(&path).unwrap();
        // expenses/todos は users への FK を張るため、テスト対象ユーザーを先に作る。
        {
            let conn = Connection::open(&path).unwrap();
            for uid in ["userA", "userB"] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                     VALUES (?1, ?1, 'x', 'x')",
                    rusqlite::params![uid],
                )
                .unwrap();
            }
        }
        db
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
    async fn task_done_completes_linked_todo() {
        // task_done + todo_id: 記録を作りつつ紐付き todos を done に更新する（Node completeTodo）。
        let db = seed_db();
        // open な todo を 1 件仕込む（id=1）。
        db.writer
            .transaction(|tx| {
                tx.execute(
                    "INSERT INTO todos (id, user_id, bot_id, title, status) \
                     VALUES (1, 'userA', 'system_default', 'やること', 'open')",
                    [],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        let tools = tools(db.clone()).unwrap();
        let add = find(&tools, "addTimelineRecord");

        let out = add
            .call(
                &ctx(),
                json!({"date": "2026-07-06", "type": "task_done", "todo_id": 1}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["record"]["type"], "task_done");
        assert_eq!(out.payload["record"]["todo_id"], 1);

        // 紐付き todo が done になっている（silent 後退の解消）。
        let status: String = db
            .read
            .read(|conn| {
                conn.query_row("SELECT status FROM todos WHERE id = 1", [], |r| r.get(0))
                    .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(status, "done");
    }

    #[tokio::test]
    async fn expense_type_double_registers_to_expenses() {
        // type=expense: expenses へ二次登録し expense_id を連結（Node addExpenseRecord）。
        let db = seed_db();
        let tools = tools(db.clone()).unwrap();
        let add = find(&tools, "addTimelineRecord");

        let out = add
            .call(
                &ctx(),
                json!({
                    "date": "2026-07-06",
                    "type": "expense",
                    "amount": 1500,
                    "expense_category": "食費",
                    "title": "ランチ"
                }),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        // Node: `¥{amount} を記録しました。`（整数は小数点なし）。
        assert_eq!(out.payload["message"], "¥1500 を記録しました。");
        assert_eq!(out.payload["record"]["type"], "expense");
        assert_eq!(out.payload["record"]["expense_category"], "食費");
        assert!(out.payload["record"]["expense_id"].is_i64());

        // expenses に 1 行（source='timeline'・memo=title・amount）が入っている。
        let (amount, source, memo): (i64, String, String) = db
            .read
            .read(|conn| {
                conn.query_row(
                    "SELECT amount, source, memo FROM expenses WHERE user_id = 'userA'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(amount, 1500);
        assert_eq!(source, "timeline");
        assert_eq!(memo, "ランチ");

        // amount 欠落は fail（Node `expense には amount が必要です。`）。
        let out = add
            .call(&ctx(), json!({"date": "2026-07-06", "type": "expense"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
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
        let out = add
            .call(&ctx(), json!({"date": "2026-07-06"}))
            .await
            .unwrap();
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

    #[tokio::test]
    async fn day_plan_create_list_delete() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let create = find(&tools, "createDayPlanBlock");
        let list = find(&tools, "listDayPlan");
        let delete = find(&tools, "deleteDayPlanBlock");

        // 必須欠落 → fail。
        let out = create
            .call(&ctx(), json!({"date": "2026-08-01"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // 作成。
        let out = create
            .call(
                &ctx(),
                json!({"date": "2026-08-01", "type": "event", "title": "会議", "start_time": "10:00"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        let block_id = out.payload["block"]["id"].as_i64().unwrap();
        assert_eq!(out.payload["block"]["type"], "event");

        // 一覧（date 指定）に 1 件。
        let out = list
            .call(&ctx(), json!({"date": "2026-08-01"}))
            .await
            .unwrap();
        assert_eq!(out.payload["blocks"].as_array().unwrap().len(), 1);
        // 別日は空。
        let out = list
            .call(&ctx(), json!({"date": "2026-08-02"}))
            .await
            .unwrap();
        assert_eq!(out.payload["blocks"].as_array().unwrap().len(), 0);

        // 削除。
        let out = delete.call(&ctx(), json!({"id": block_id})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        let out = delete.call(&ctx(), json!({"id": block_id})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        // 別ユーザーには見えない。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list
            .call(&ctx_b, json!({"date": "2026-08-01"}))
            .await
            .unwrap();
        assert_eq!(out.payload["blocks"].as_array().unwrap().len(), 0);
    }
}
