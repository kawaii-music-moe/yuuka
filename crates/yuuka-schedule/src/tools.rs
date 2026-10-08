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
//!
//! ## Google カレンダー同期（[`tools_with_calendar`] で [`CalendarEventsPort`] を渡したとき）
//! 予定は常に Yuuka に保存し、エージェントに Google アカウントが連携されているときだけ Google にも
//! 登録・削除する（`local_only` なら送らない）。結果の `google_sync`（`synced`/`failed`/`not_linked`/
//! `local_only`）と本文で、実際に同期できたかをエージェントへ正確に伝える（同期していないのに
//! 「同期しました」と言わせない）。listSchedules はアカウントを明示指定したエージェントなら、
//! 一覧の前に Google の予定を取り込む（[`crate::google_sync`]）。
//!
//! 未移植: `getUserRemindDefaultMinutes`（ユーザー別リマインド既定）＝ repo `add` の既定 10 分に集約。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope};
use yuuka_web::Db;

use yuuka_google::{CalendarEventsPort, GoogleEventInput};

use crate::datetime::normalize_local_datetime;
use crate::dto::NewSchedule;
use crate::google_sync::{pull_from_google, DEFAULT_SYNC_DAYS};
use crate::repo::ScheduleRepo;

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    tools_with_calendar(db, None)
}

/// [`tools`] の Google カレンダー同期付き版（`calendar` が `None` なら同期しない）。
///
/// # Errors
/// [`tools`] と同じ。
pub fn tools_with_calendar(
    db: Db,
    calendar: Option<Arc<dyn CalendarEventsPort>>,
) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(AddScheduleTool {
            name: ToolName::checked("addSchedule".to_owned())?,
            db: db.clone(),
            calendar: calendar.clone(),
        }),
        Arc::new(ListSchedulesTool {
            name: ToolName::checked("listSchedules".to_owned())?,
            db: db.clone(),
            calendar: calendar.clone(),
        }),
        Arc::new(DeleteScheduleTool {
            name: ToolName::checked("deleteSchedule".to_owned())?,
            db,
            calendar,
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
    calendar: Option<Arc<dyn CalendarEventsPort>>,
}

#[async_trait]
impl Tool for AddScheduleTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "日時の決まった予定をカレンダーに登録する。\
                例:「来週月曜10時に打ち合わせ」「5/28に歯医者」。\
                Googleカレンダーが連携されていれば、そちらにも同じ予定を登録する（結果の google_sync で成否が分かる）。\
                カレンダーを汚したくない単発のタイマー的な予定なら local_only を true にする。\
                ただ「n分後に教えて」だけなら代わりに addReminder を使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "予定の名前（例:「歯医者」「定例会議」）" },
                    "start_at": { "type": "string", "description": "開始する日時。形式: ISO 8601（例: 2026-05-28T10:00:00）" },
                    "end_at": { "type": "string", "description": "終了する日時。形式: ISO 8601。省略可" },
                    "remind_before_minutes": { "type": "number", "description": "開始の何分前に知らせるか（分単位）。省略=既定値" },
                    "description": { "type": "string", "description": "予定の補足メモ。省略可" },
                    "calendar_id": { "type": "string", "description": "登録先のGoogleカレンダーのID。省略可（省略時は既定のカレンダー）。複数ある時は内容に一番合うものを選ぶ" },
                    "local_only": { "type": "boolean", "description": "true にするとGoogleカレンダーに送らず、Yuuka 内にだけ登録する" }
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
        // 解釈できない日時はそのまま保存せず、エージェントに書式を直させる。
        if normalize_local_datetime(&start_at).is_none() {
            return Ok(fail_payload(
                "start_at の日時を解釈できません。ISO 8601（例: 2026-05-28T10:00:00）で指定してください。",
            ));
        }
        if arg_str(&args, "end_at").is_some_and(|end| normalize_local_datetime(&end).is_none()) {
            return Ok(fail_payload(
                "end_at の日時を解釈できません。ISO 8601（例: 2026-05-28T11:00:00）で指定してください。",
            ));
        }

        let new = NewSchedule {
            title,
            start_at,
            end_at: arg_str(&args, "end_at"),
            // 未指定は repo の既定（10 分）に委ねる（Node getUserRemindDefaultMinutes の簡約）。
            remind_before_minutes: arg_i64(&args, "remind_before_minutes"),
            description: arg_str(&args, "description"),
        };

        let scope = scope_of(ctx);
        let repo = ScheduleRepo::new(&self.db);
        let schedule = repo.add(&scope, new).await.map_err(exec_err)?;

        // Yuuka に保存してから、連携していれば Google にも登録する（失敗しても Yuuka の予定は残す）。
        let local_only = args.get("local_only").and_then(Value::as_bool) == Some(true);
        let sync = if local_only {
            GoogleSync::LocalOnly
        } else {
            match self.linked_account(ctx).await {
                None => GoogleSync::NotLinked,
                Some((calendar, account)) => {
                    let calendar_id = arg_str(&args, "calendar_id")
                        .unwrap_or_else(|| account.default_calendar_id.clone());
                    let input = GoogleEventInput {
                        title: schedule.title.clone(),
                        description: schedule.description.clone(),
                        start_local: schedule.start_at.clone(),
                        end_local: schedule.end_at.clone(),
                    };
                    match calendar
                        .insert_event(account.account_id, &calendar_id, &input)
                        .await
                    {
                        Ok(event_id) => {
                            repo.set_google_link(&scope, schedule.id, &event_id, &calendar_id)
                                .await
                                .map_err(exec_err)?;
                            GoogleSync::Synced
                        }
                        Err(e) => {
                            tracing::warn!(schedule_id = schedule.id, error = ?e, "予定の Google カレンダー登録に失敗しました");
                            GoogleSync::Failed
                        }
                    }
                }
            }
        };

        let remind_label = if schedule.remind_before_minutes > 0 {
            format!("、{}分前にリマインド", schedule.remind_before_minutes)
        } else {
            String::new()
        };
        let sync_note = match sync {
            GoogleSync::Synced => "Googleカレンダーにも同期しました📅",
            GoogleSync::Failed => "Googleカレンダーへの同期に失敗しました（Yuuka には登録済み）",
            GoogleSync::NotLinked => {
                "Googleカレンダーとは連携していないため、Yuuka 内にだけ登録しました"
            }
            GoogleSync::LocalOnly => "Googleカレンダーには送らず、Yuuka 内にだけ登録しました",
        };
        let message = format!(
            "予定「{}」を登録しました ({}{remind_label})。{sync_note}",
            schedule.title, schedule.start_at,
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "schedule": schedule, "google_sync": sync.as_str() }),
        )))
    }
}

impl AddScheduleTool {
    /// 予定の書き込みに使う Google アカウント（同期が無効・未連携なら `None`）。
    async fn linked_account(
        &self,
        ctx: &ToolContext,
    ) -> Option<(
        &Arc<dyn CalendarEventsPort>,
        yuuka_google::LinkedGoogleAccount,
    )> {
        let calendar = self.calendar.as_ref()?;
        let account = calendar
            .account_for(ctx.user_id.as_str(), ctx.bot_id.as_str())
            .await?;
        Some((calendar, account))
    }
}

/// Google カレンダー同期の結果（ツール結果の `google_sync`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GoogleSync {
    /// Google にも反映した。
    Synced,
    /// 連携しているが反映に失敗した。
    Failed,
    /// Google と連携していない。
    NotLinked,
    /// `local_only` 指定で送らなかった。
    LocalOnly,
}

impl GoogleSync {
    fn as_str(self) -> &'static str {
        match self {
            Self::Synced => "synced",
            Self::Failed => "failed",
            Self::NotLinked => "not_linked",
            Self::LocalOnly => "local_only",
        }
    }
}

// ─── listSchedules ───────────────────────────────────────────────────────────

struct ListSchedulesTool {
    name: ToolName,
    db: Db,
    calendar: Option<Arc<dyn CalendarEventsPort>>,
}

#[async_trait]
impl Tool for ListSchedulesTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description:
                "これから先の予定の一覧を表示する。例:「今週の予定は?」「直近の予定を見せて」。\
                 Googleカレンダーの取り込みが有効なエージェントでは、表示の前に Google の最新の予定を取り込む。"
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
        let scope = scope_of(ctx);

        // 取り込みが有効（アカウントを明示指定）なエージェントは、表示前に Google の予定を取り込む。
        // 失敗しても Yuuka 内の予定は表示する。
        if let Some(calendar) = &self.calendar {
            if let Some(account_id) = calendar
                .import_account_for(ctx.user_id.as_str(), ctx.bot_id.as_str())
                .await
            {
                let now = chrono::Local::now().naive_local();
                if let Err(e) = pull_from_google(
                    &self.db,
                    calendar,
                    &scope,
                    account_id,
                    now,
                    days.max(DEFAULT_SYNC_DAYS),
                )
                .await
                {
                    tracing::warn!(error = %e, "予定一覧の前の Google カレンダー取り込みに失敗しました");
                }
            }
        }

        let schedules = ScheduleRepo::new(&self.db)
            .list_upcoming(&scope, days)
            .await
            .map_err(exec_err)?;

        if schedules.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                format!("今後{days}日間の予定はありません。"),
                json!({ "schedules": [] }),
            )));
        }

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
    calendar: Option<Arc<dyn CalendarEventsPort>>,
}

#[async_trait]
impl Tool for DeleteScheduleTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "指定したIDの予定を削除する。どの予定か分からない時は先に listSchedules でIDを確認する。\
                Googleカレンダーに同期済みの予定なら、向こうの予定も一緒に削除する（結果の google_sync で成否が分かる）。"
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

        // Google と紐付いていれば先に向こうを消す（失敗しても Yuuka 側は消す）。
        let link = repo.google_link(&scope, id).await.map_err(exec_err)?;
        let sync = match (link, &self.calendar) {
            (None, _) => GoogleSync::NotLinked,
            (Some(_), None) => GoogleSync::Failed,
            (Some((event_id, calendar_id)), Some(calendar)) => {
                match calendar
                    .account_for(ctx.user_id.as_str(), ctx.bot_id.as_str())
                    .await
                {
                    None => GoogleSync::Failed,
                    Some(account) => {
                        let calendar_id =
                            calendar_id.unwrap_or_else(|| account.default_calendar_id.clone());
                        match calendar
                            .delete_event(account.account_id, &calendar_id, &event_id)
                            .await
                        {
                            Ok(()) => GoogleSync::Synced,
                            Err(e) => {
                                tracing::warn!(schedule_id = id, error = ?e, "予定の Google カレンダー削除に失敗しました");
                                GoogleSync::Failed
                            }
                        }
                    }
                }
            }
        };

        let deleted = repo.delete(&scope, id).await.map_err(exec_err)?;
        if !deleted {
            return Ok(fail_payload(format!("予定 #{id} の削除に失敗しました。")));
        }
        let sync_note = match sync {
            GoogleSync::Synced => "Googleカレンダーからも削除しました",
            GoogleSync::Failed => {
                "Googleカレンダー側の予定は削除できませんでした（Yuuka からは削除済み）"
            }
            GoogleSync::NotLinked | GoogleSync::LocalOnly => {
                "Googleカレンダーには同期されていない予定でした"
            }
        };
        Ok(ToolOutcome::from_payload(ok_payload(
            format!("予定 #{id} を削除しました🗑️ {sync_note}"),
            json!({ "google_sync": sync.as_str() }),
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
        let path = std::env::temp_dir().join(format!(
            "yuuka_schedule_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
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
            .call(
                &ctx(),
                json!({"title": "打ち合わせ", "start_at": start_in_days(1)}),
            )
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
        let out = delete
            .call(&ctx(), json!({"schedule_id": id}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        // 二重削除は not-found（success:false）。
        let out = delete
            .call(&ctx(), json!({"schedule_id": id}))
            .await
            .unwrap();
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
    async fn add_normalizes_iso_datetimes_and_rejects_unparsable() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addSchedule");

        // Gemini が返す ISO 8601（T 区切り・オフセット付き）は保存形式へ揃える。
        let out = add
            .call(
                &ctx(),
                json!({"title": "歯医者", "start_at": "2026-05-28T10:00:00", "end_at": "2026-05-28T02:00:00Z"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["schedule"]["start_at"], "2026-05-28 10:00:00");
        assert_eq!(out.payload["schedule"]["end_at"], "2026-05-28 11:00:00");

        // 解釈できない日時は保存せずに fail（エージェントに書式を直させる）。
        let out = add
            .call(&ctx(), json!({"title": "x", "start_at": "明日の10時"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        let out = add
            .call(
                &ctx(),
                json!({"title": "x", "start_at": "2026-05-28T10:00:00", "end_at": "そのうち"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn scope_isolation_across_users() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addSchedule");
        let list = find(&tools, "listSchedules");

        add.call(
            &ctx(),
            json!({"title": "A の予定", "start_at": start_in_days(1)}),
        )
        .await
        .unwrap();

        // 別ユーザーには見えない。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["schedules"].as_array().unwrap().len(), 0);
    }

    // ─── Google カレンダー同期 ───────────────────────────────────────────────

    use std::sync::Mutex;

    use yuuka_google::{CalendarSummary, GoogleError, GoogleEvent, LinkedGoogleAccount};

    /// メモリ上の Google カレンダー（呼び出しを記録し、`events` を一覧として返す）。
    #[derive(Default)]
    struct FakeGoogle {
        linked: bool,
        import: bool,
        fail_insert: bool,
        inserted: Mutex<Vec<(String, GoogleEventInput)>>,
        deleted: Mutex<Vec<(String, String)>>,
        events: Mutex<Vec<GoogleEvent>>,
    }

    #[async_trait]
    impl CalendarEventsPort for FakeGoogle {
        async fn account_for(&self, _user_id: &str, _bot_id: &str) -> Option<LinkedGoogleAccount> {
            self.linked.then(|| LinkedGoogleAccount {
                account_id: 1,
                default_calendar_id: "main@g".to_owned(),
            })
        }
        async fn import_account_for(&self, _user_id: &str, _bot_id: &str) -> Option<i64> {
            self.import.then_some(1)
        }
        async fn calendars(&self, _account_id: i64) -> Result<Vec<CalendarSummary>, GoogleError> {
            Ok(vec![CalendarSummary {
                id: "main@g".to_owned(),
                summary: "メイン".to_owned(),
            }])
        }
        async fn insert_event(
            &self,
            _account_id: i64,
            calendar_id: &str,
            event: &GoogleEventInput,
        ) -> Result<String, GoogleError> {
            if self.fail_insert {
                return Err(GoogleError::Upstream("boom".to_owned()));
            }
            let mut inserted = self.inserted.lock().unwrap();
            inserted.push((calendar_id.to_owned(), event.clone()));
            Ok(format!("ev{}", inserted.len()))
        }
        async fn delete_event(
            &self,
            _account_id: i64,
            calendar_id: &str,
            event_id: &str,
        ) -> Result<(), GoogleError> {
            self.deleted
                .lock()
                .unwrap()
                .push((calendar_id.to_owned(), event_id.to_owned()));
            Ok(())
        }
        async fn list_events(
            &self,
            _account_id: i64,
            _calendar_id: &str,
            _from_local: &str,
            _to_local: &str,
        ) -> Result<Vec<GoogleEvent>, GoogleError> {
            Ok(self.events.lock().unwrap().clone())
        }
    }

    fn google_tools(db: Db, google: Arc<FakeGoogle>) -> Vec<Arc<dyn Tool>> {
        tools_with_calendar(db, Some(google as Arc<dyn CalendarEventsPort>)).unwrap()
    }

    #[tokio::test]
    async fn add_syncs_to_google_only_when_linked() {
        // 連携あり: Google に登録し、紐付けを保存して「同期しました」と返す。
        let db = seed_db();
        let google = Arc::new(FakeGoogle {
            linked: true,
            ..FakeGoogle::default()
        });
        let tools = google_tools(db.clone(), google.clone());
        let add = find(&tools, "addSchedule");
        let out = add
            .call(&ctx(), json!({"title": "歯医者", "start_at": "2026-05-28T10:00:00", "calendar_id": "work@g"}))
            .await
            .unwrap();
        assert_eq!(out.payload["google_sync"], "synced");
        assert!(out.payload["message"]
            .as_str()
            .unwrap()
            .contains("Googleカレンダーにも同期しました"));
        let inserted = google.inserted.lock().unwrap().clone();
        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].0, "work@g");
        assert_eq!(inserted[0].1.start_local, "2026-05-28 10:00:00");
        let id = out.payload["schedule"]["id"].as_i64().unwrap();
        let scope = UserScope::new(UserId::new("userA"), BotId::system_default());
        assert_eq!(
            ScheduleRepo::new(&db)
                .google_link(&scope, id)
                .await
                .unwrap(),
            Some(("ev1".to_owned(), Some("work@g".to_owned())))
        );

        // local_only: 連携していても送らない。
        let out = add
            .call(
                &ctx(),
                json!({"title": "タイマー", "start_at": "2026-05-28T11:00:00", "local_only": true}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["google_sync"], "local_only");
        assert_eq!(google.inserted.lock().unwrap().len(), 1);

        // 連携なし: Yuuka にだけ保存し、同期したとは言わない。
        let google = Arc::new(FakeGoogle::default());
        let tools = google_tools(seed_db(), google.clone());
        let out = find(&tools, "addSchedule")
            .call(
                &ctx(),
                json!({"title": "歯医者", "start_at": "2026-05-28T10:00:00"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["google_sync"], "not_linked");
        assert!(!out.payload["message"]
            .as_str()
            .unwrap()
            .contains("同期しました"));
        assert!(google.inserted.lock().unwrap().is_empty());

        // 連携しているが失敗: Yuuka には残し、失敗したと伝える。
        let google = Arc::new(FakeGoogle {
            linked: true,
            fail_insert: true,
            ..FakeGoogle::default()
        });
        let tools = google_tools(seed_db(), google);
        let out = find(&tools, "addSchedule")
            .call(
                &ctx(),
                json!({"title": "歯医者", "start_at": "2026-05-28T10:00:00"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["google_sync"], "failed");
    }

    #[tokio::test]
    async fn delete_removes_the_google_event_too() {
        let db = seed_db();
        let google = Arc::new(FakeGoogle {
            linked: true,
            ..FakeGoogle::default()
        });
        let tools = google_tools(db, google.clone());
        let out = find(&tools, "addSchedule")
            .call(
                &ctx(),
                json!({"title": "歯医者", "start_at": "2026-05-28T10:00:00"}),
            )
            .await
            .unwrap();
        let id = out.payload["schedule"]["id"].as_i64().unwrap();
        let out = find(&tools, "deleteSchedule")
            .call(&ctx(), json!({"schedule_id": id}))
            .await
            .unwrap();
        assert_eq!(out.payload["google_sync"], "synced");
        assert_eq!(
            google.deleted.lock().unwrap().clone(),
            vec![("main@g".to_owned(), "ev1".to_owned())]
        );
    }

    #[tokio::test]
    async fn list_imports_google_events_for_explicitly_linked_agents() {
        let db = seed_db();
        let now = chrono::Local::now().naive_local();
        let at = |days: i64, hour: u32| {
            (now.date() + chrono::Duration::days(days))
                .and_hms_opt(hour, 0, 0)
                .unwrap()
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        };
        let event = |id: &str, title: &str, start: String| GoogleEvent {
            id: id.to_owned(),
            title: title.to_owned(),
            description: String::new(),
            start_local: start,
            end_local: None,
        };
        let google = Arc::new(FakeGoogle {
            linked: true,
            import: true,
            ..FakeGoogle::default()
        });
        let tools = google_tools(db.clone(), google.clone());
        let list = find(&tools, "listSchedules");

        // 同じタイトル・日時の未紐付けの予定は紐付け、無いものは新規に取り込む。
        let scope = UserScope::new(UserId::new("userA"), BotId::system_default());
        let repo = ScheduleRepo::new(&db);
        repo.add(&scope, new_schedule_at("会議", &at(2, 10)))
            .await
            .unwrap();
        *google.events.lock().unwrap() = vec![
            event("g1", "会議", at(2, 10)),
            event("g2", "旅行", at(3, 9)),
        ];
        let out = list.call(&ctx(), json!({"days": 7})).await.unwrap();
        let titles: Vec<&str> = out.payload["schedules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["会議", "旅行"]);

        // Google 側の変更は反映し、Google から消えた予定は Yuuka からも消す。
        *google.events.lock().unwrap() = vec![event("g1", "会議（変更）", at(2, 11))];
        let out = list.call(&ctx(), json!({"days": 7})).await.unwrap();
        let schedules = out.payload["schedules"].as_array().unwrap();
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0]["title"], "会議（変更）");
        assert_eq!(schedules[0]["start_at"], at(2, 11));

        // 取り込み対象でないエージェントは取り込まない。
        let google = Arc::new(FakeGoogle {
            linked: true,
            ..FakeGoogle::default()
        });
        *google.events.lock().unwrap() = vec![event("g9", "取り込まれない", at(1, 9))];
        let tools = google_tools(seed_db(), google);
        let out = find(&tools, "listSchedules")
            .call(&ctx(), json!({}))
            .await
            .unwrap();
        assert_eq!(out.payload["schedules"].as_array().unwrap().len(), 0);
    }

    fn new_schedule_at(title: &str, start_at: &str) -> NewSchedule {
        NewSchedule {
            title: title.to_owned(),
            start_at: start_at.to_owned(),
            end_at: None,
            remind_before_minutes: None,
            description: None,
        }
    }
}
