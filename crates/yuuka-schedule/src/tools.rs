//! schedule ドメインの Native ツール（現行 `src/functions/scheduleFunctions.ts` の移植）。
//!
//! yuuka-todo の tools.rs を雛形に、`ScheduleRepo`（add/get/list_upcoming/delete）へ素直に
//! 対応するツールだけを移植する。core の凍結 [`Tool`] を実装し、`tools(db)` が
//! `Vec<Arc<dyn Tool>>` を返す（assembly 層が `NativeProvider` へ登録する。ドメインは
//! yuuka-tools に依存しない＝依存の向きを保つ）。
//!
//! **wire 契約の非対称に注意**: HTTP route の body は camelCase（`startAt`）だが、**tool 引数は
//! snake_case**（`start_at`・Node の Gemini 宣言と一致）。ツール名は Node の system prompt が
//! 参照する **bare 名**（`addSchedule` 等・namespace 無し）を使う。
//!
//! 移植済み: addSchedule / listSchedules / deleteSchedule（コア 3 経路）。
//! **移植時に落とした Node の副作用（deferred）**:
//! - Google カレンダー双方向同期（createCalendarEvent/deleteCalendarEvent/syncGoogleCalendarToLocal・
//!   addSchedule の `calendar_id`/`local_only` 引数）＝ services 未実装・repo 非対応のため後続。
//! - `getUserRemindDefaultMinutes`（ユーザー別リマインド既定）＝ repo `add` の既定 10 分に集約。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope};
use yuuka_web::Db;

use crate::dto::NewSchedule;
use crate::repo::ScheduleRepo;

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(AddScheduleTool {
            name: ToolName::checked("addSchedule".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(ListSchedulesTool {
            name: ToolName::checked("listSchedules".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(DeleteScheduleTool {
            name: ToolName::checked("deleteSchedule".to_owned())?,
            db,
        }),
    ])
}

// ─── 共通ヘルパ（todo/tools.rs と同一規約） ────────────────────────────────────

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

/// 数値、または数値文字列を i64 へ（Node `Number(...)` の整数版・`Number.isInteger` 相当）。
fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    let v = args.get(key)?;
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

/// DbError をツール実行エラーへ（握り潰さず Gemini へ `{success:false}` として返る・§8.4）。
fn exec_err(e: DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

// ─── addSchedule ─────────────────────────────────────────────────────────────

struct AddScheduleTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for AddScheduleTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "日時の決まった予定をカレンダーに登録する。\
                例:「来週月曜10時に打ち合わせ」「5/28に歯医者」。\
                ただ「n分後に教えて」だけなら代わりに addReminder を使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "予定の名前（例:「歯医者」「定例会議」）" },
                    "start_at": { "type": "string", "description": "開始する日時。形式: ISO 8601（例: 2026-05-28T10:00:00）" },
                    "end_at": { "type": "string", "description": "終了する日時。形式: ISO 8601。省略可" },
                    "remind_before_minutes": { "type": "number", "description": "開始の何分前に知らせるか（分単位）。省略=既定値" },
                    "description": { "type": "string", "description": "予定の補足メモ。省略可" }
                },
                "required": ["title", "start_at"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let (Some(title), Some(start_at)) = (arg_str(&args, "title"), arg_str(&args, "start_at"))
        else {
            return Ok(fail_payload("title と start_at は必須です。"));
        };

        let new = NewSchedule {
            title,
            start_at,
            end_at: arg_str(&args, "end_at"),
            // 未指定は repo の既定（10 分）に委ねる（Node getUserRemindDefaultMinutes の簡約）。
            remind_before_minutes: arg_i64(&args, "remind_before_minutes"),
            description: arg_str(&args, "description"),
        };

        let schedule = ScheduleRepo::new(&self.db)
            .add(&scope_of(ctx), new)
            .await
            .map_err(exec_err)?;

        let remind_label = if schedule.remind_before_minutes > 0 {
            format!("、{}分前にリマインド", schedule.remind_before_minutes)
        } else {
            String::new()
        };
        // NOTE: Google カレンダー同期（syncMessage）はサービス未実装のため付与しない（deferred）。
        let message = format!(
            "予定「{}」を登録しました ({}{remind_label})",
            schedule.title, schedule.start_at,
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "schedule": schedule }),
        )))
    }
}

// ─── listSchedules ───────────────────────────────────────────────────────────

struct ListSchedulesTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListSchedulesTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "これから先の予定の一覧を表示する。例:「今週の予定は?」「直近の予定を見せて」。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "days": { "type": "number", "description": "今日から何日先までの予定を表示するか（日数）。省略=7日" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // 既定 7 日（Node: rawArgs.days !== undefined ? Number(days) : 7）。
        let days = arg_i64(&args, "days").unwrap_or(7);

        let schedules = ScheduleRepo::new(&self.db)
            .list_upcoming(&scope_of(ctx), days)
            .await
            .map_err(exec_err)?;

        if schedules.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                format!("今後{days}日間の予定はありません。"),
                json!({ "schedules": [] }),
            )));
        }

        // クリーンビューは google_event_id を持たないため、アイコンは常に 📌（Node の 📅 分岐は同期列前提）。
        let lines: Vec<String> = schedules
            .iter()
            .map(|s| format!("📌 #{} {} — {}", s.id, s.title, s.start_at))
            .collect();
        let message = format!(
            "今後{days}日間の予定 ({}件):\n{}",
            schedules.len(),
            lines.join("\n"),
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "schedules": schedules }),
        )))
    }
}

// ─── deleteSchedule ──────────────────────────────────────────────────────────

struct DeleteScheduleTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for DeleteScheduleTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "指定したIDの予定を削除する。どの予定か分からない時は先に listSchedules でIDを確認する。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "schedule_id": { "type": "number", "description": "削除する予定のID（listSchedules で表示される番号）" }
                },
                "required": ["schedule_id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(id) = arg_i64(&args, "schedule_id") else {
            return Ok(fail_payload("schedule_id が不正です。"));
        };

        let scope = scope_of(ctx);
        let repo = ScheduleRepo::new(&self.db);

        // Node parity: 先に取得して所有（スコープ）を確認し、無ければ not-found。
        if repo.get(&scope, id).await.map_err(exec_err)?.is_none() {
            return Ok(fail_payload(format!("予定 #{id} が見つかりません。")));
        }

        let deleted = repo.delete(&scope, id).await.map_err(exec_err)?;
        if deleted {
            Ok(ToolOutcome::from_payload(ok_payload(
                format!("予定 #{id} を削除しました🗑️"),
                json!({}),
            )))
        } else {
            Ok(fail_payload(format!("予定 #{id} の削除に失敗しました。")))
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

    // lib.rs のテスト DDL を再利用（bot_id / google_* / created_at 既定を含む）。
    const SCHEDULES_DDL: &str = "CREATE TABLE schedules (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        title TEXT NOT NULL,
        description TEXT,
        start_at TEXT NOT NULL,
        end_at TEXT,
        remind_before_minutes INTEGER NOT NULL DEFAULT 10,
        reminded INTEGER NOT NULL DEFAULT 0,
        google_event_id TEXT,
        google_calendar_id TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("yuuka_schedule_tools_{}_{seq}.sqlite", std::process::id()));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEDULES_DDL).unwrap();
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

    /// list_upcoming の窓（now..now+days）に確実に載る開始時刻を実値化する。
    fn start_in_days(offset: i64) -> String {
        let conn = Connection::open_in_memory().unwrap();
        conn.query_row(
            &format!("SELECT datetime('now','localtime','+{offset} days')"),
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn add_list_delete_roundtrip() {
        let db = seed_db();
        let tools = tools(db).unwrap();

        // 宣言名は bare（Node system prompt と一致）。
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.declaration().name.to_string())
            .collect();
        assert!(names.contains(&"addSchedule".to_owned()));
        assert!(!names.iter().any(|n| n.contains(':')), "native は bare 名");

        // add（snake_case 引数）。remind 未指定は既定 10（Node parity）。
        let add = find(&tools, "addSchedule");
        let out = add
            .call(&ctx(), json!({"title": "打ち合わせ", "start_at": start_in_days(1)}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["schedule"]["title"], "打ち合わせ");
        assert_eq!(out.payload["schedule"]["remind_before_minutes"], 10);
        let id = out.payload["schedule"]["id"].as_i64().unwrap();

        // list（既定 7 日窓）で 1 件見える。
        let list = find(&tools, "listSchedules");
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["schedules"].as_array().unwrap().len(), 1);

        // delete。
        let delete = find(&tools, "deleteSchedule");
        let out = delete.call(&ctx(), json!({"schedule_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        // 二重削除は not-found（success:false）。
        let out = delete.call(&ctx(), json!({"schedule_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // 削除後は list も空。
        let out = list.call(&ctx(), json!({"days": 7})).await.unwrap();
        assert_eq!(out.payload["schedules"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn add_validates_required_and_explicit_remind() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addSchedule");

        // title / start_at 欠落 → fail。
        let out = add.call(&ctx(), json!({"title": "x"})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        let out = add.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // 明示 remind_before_minutes は採用される。
        let out = add
            .call(
                &ctx(),
                json!({"title": "会議", "start_at": start_in_days(1), "remind_before_minutes": 30}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["schedule"]["remind_before_minutes"], 30);
    }

    #[tokio::test]
    async fn scope_isolation_across_users() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addSchedule");
        let list = find(&tools, "listSchedules");

        add.call(&ctx(), json!({"title": "A の予定", "start_at": start_in_days(1)}))
            .await
            .unwrap();

        // 別ユーザーには見えない。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["schedules"].as_array().unwrap().len(), 0);
    }
}
