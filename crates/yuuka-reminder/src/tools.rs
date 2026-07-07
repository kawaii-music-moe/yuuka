//! reminder ドメインの Native ツール（現行 `src/functions/reminderFunctions.ts` の移植・§9.4）。
//!
//! **参照実装は yuuka-todo/src/tools.rs**。core の凍結 [`Tool`] を実装し、`tools(db)` が
//! `Vec<Arc<dyn Tool>>` を返す（assembly 層が `NativeProvider` へ登録する。ドメインは
//! yuuka-tools に依存しない＝依存の向きを保つ）。
//!
//! **wire 契約の非対称に注意**: tool 引数は Node の Gemini 宣言と同じ **snake_case**
//! （`trigger_at`/`repeat_rule`/`include_all`/`reminder_id`）。ツール名は Node の
//! system prompt が参照する **bare 名**（`addReminder` 等・namespace 無し）を使う。
//!
//! 移植済み: addReminder / listReminders / cancelReminder（Node reminderFunctions の全 3 ツール）。
//! 各ツールが依存する副機能のうち repo が持たないものは deferred（repo.rs / lib.rs の deferred
//! 記述と同一）: trigger_at の ISO/Date 正規化（`toDbDateTime`）・過去日時の次回時刻補正・
//! cron 式の厳密検証（cron-parser。ここでは todo と同じ 5 フィールドの簡易検証）・
//! 既定送信先解決（`getUserNotifyTarget`＝userRepo 跨ぎ。未指定時は repo が `"dm"` へ）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope};
use yuuka_web::Db;

use crate::dto::{NewReminder, Reminder};
use crate::repo::ReminderRepo;

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(AddReminderTool {
            name: ToolName::checked("addReminder".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(ListRemindersTool {
            name: ToolName::checked("listReminders".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(CancelReminderTool {
            name: ToolName::checked("cancelReminder".to_owned())?,
            db,
        }),
    ])
}

// ─── 共通ヘルパ ───────────────────────────────────────────────────────────────

/// `{success:true, message, ...extra}`（Node `ok`/各 handler の成功応答）。
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

/// `{success:false, message}`。実行エラーではなく「妥当だが失敗」な結果。
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

/// 数値、または数値文字列を i64 へ（todo の `arg_i64` を踏襲）。
fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    let v = args.get(key)?;
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

/// `asOptionalTarget`（`'dm' | 'channel'` 以外は None）。
fn arg_target(args: &Value, key: &str) -> Option<String> {
    match args.get(key).and_then(Value::as_str) {
        Some("dm") => Some("dm".to_owned()),
        Some("channel") => Some("channel".to_owned()),
        _ => None,
    }
}

/// DbError をツール実行エラーへ（握り潰さず Gemini へ `{success:false}` として返る・§8.4）。
fn exec_err(e: DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

/// 最小 cron 妥当性（5 フィールド・空でない）。厳密検証（cron-parser）は deferred（todo と同じ簡約）。
fn is_valid_cron_basic(rule: &str) -> bool {
    let fields: Vec<&str> = rule.split_whitespace().collect();
    fields.len() == 5 && fields.iter().all(|f| !f.is_empty())
}

/// `s` が `mask` に一致するか（mask の `'d'`=数字、それ以外=リテラル。長さ一致必須）。
fn matches_mask(s: &str, mask: &str) -> bool {
    s.len() == mask.len()
        && s.bytes().zip(mask.bytes()).all(|(c, m)| {
            if m == b'd' {
                c.is_ascii_digit()
            } else {
                c == m
            }
        })
}

/// 日時 'YYYY-MM-DD HH:MM:SS'（または T 区切り）を表示用 'YYYY-MM-DD HH:MM' に整形する。
/// 形式に合わなければ元の値を返す（Node `displayDateTime` パリティ）。
fn display_date_time(value: &str) -> String {
    let norm = value.trim().replace('T', " ");
    let date = norm.get(0..10);
    let sep = norm.as_bytes().get(10).copied();
    let time = norm.get(11..16);
    if let (Some(date), Some(sep), Some(time)) = (date, sep, time) {
        if matches_mask(date, "dddd-dd-dd") && sep.is_ascii_whitespace() && matches_mask(time, "dd:dd")
        {
            return format!("{date} {time}");
        }
    }
    value.to_owned()
}

/// 送信先の表示ラベル（Node `targetLabel`）。
fn target_label(target_type: &str, target_id: Option<&str>) -> String {
    if target_type == "channel" {
        match target_id.filter(|s| !s.is_empty()) {
            Some(id) => format!("チャンネル <#{id}>"),
            None => "チャンネル（既定送信先）".to_owned(),
        }
    } else {
        "DM".to_owned()
    }
}

/// ステータスの表示絵文字（Node `statusEmoji`）。
fn status_emoji(status: &str) -> &'static str {
    match status {
        "pending" => "⏳",
        "sent" => "✅",
        _ => "🚫",
    }
}

/// ステータスの表示ラベル（Node `statusLabel`）。
fn status_label(status: &str) -> &'static str {
    match status {
        "pending" => "送信待ち",
        "sent" => "送信済み",
        _ => "キャンセル済み",
    }
}

/// 一覧の1行表示（Node `reminderLine`）。
fn reminder_line(r: &Reminder) -> String {
    let repeat = match &r.repeat_rule {
        Some(rule) => format!(" 🔁 繰り返し({rule})"),
        None => String::new(),
    };
    format!(
        "{} #{} {} 「{}」→ {}{}",
        status_emoji(&r.status),
        r.id,
        display_date_time(&r.trigger_at),
        r.message,
        target_label(&r.target_type, r.target_id.as_deref()),
        repeat,
    )
}

/// LLM へ返すリマインドの共通整形（Node `toReminderEntry`。`reminder_id` キーに注意）。
fn reminder_entry(r: &Reminder) -> Value {
    json!({
        "reminder_id": r.id,
        "message": r.message.clone(),
        "trigger_at": r.trigger_at.clone(),
        "repeat_rule": r.repeat_rule.clone(),
        "target_type": r.target_type.clone(),
        "target_id": r.target_id.clone(),
        "status": r.status.clone(),
        "source": r.source.clone(),
    })
}

// ─── addReminder ─────────────────────────────────────────────────────────────

struct AddReminderTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for AddReminderTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "決めた日時にDiscordへお知らせを送るリマインドを登録する。\
                「明日の15時に〜を思い出させて」「30分後に教えて」等。「30分後」などは今の時刻を基準に日時へ直す。\
                毎週・毎日など繰り返したい時は repeat_rule に cron式を入れ（例 毎週月曜9時='0 9 * * 1'）、\
                trigger_at には1回目の日時を入れる。1回だけなら repeat_rule は入れない。\
                送り先 target はユーザーがはっきり指定した時だけ入れる（省略で既定の送り先）。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "お知らせで届ける本文（例 '会議の資料を準備する'）" },
                    "trigger_at": { "type": "string", "description": "送る日時 YYYY-MM-DDTHH:MM:SS。「明日の朝」等は今の時刻を基準に直す。繰り返しの時は1回目の日時" },
                    "repeat_rule": { "type": "string", "description": "繰り返す時だけ入れる cron式（分 時 日 月 曜日。例 '0 9 * * 1'=毎週月曜9時）。1回だけなら省略（任意）" },
                    "target": { "type": "string", "description": "送り先 'dm'（ダイレクトメッセージ）/'channel'（チャンネル）。ユーザーが明示した時だけ（任意）" }
                },
                "required": ["message", "trigger_at"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(message) = arg_str(&args, "message") else {
            return Ok(fail_payload("リマインドのメッセージを指定してください。"));
        };
        let Some(trigger_at) = arg_str(&args, "trigger_at") else {
            return Ok(fail_payload(
                "送信日時 trigger_at (ISO 8601形式) を指定してください。",
            ));
        };

        // 繰り返しの cron 式は簡易検証（厳密検証・過去日時の次回時刻補正は deferred）。
        let repeat_rule = arg_str(&args, "repeat_rule");
        if let Some(rule) = &repeat_rule {
            if !is_valid_cron_basic(rule) {
                return Ok(fail_payload(format!(
                    "repeat_rule のcron式が不正です: {rule}（例: 毎週月曜9時 = '0 9 * * 1'）"
                )));
            }
        }

        // 送信先: 明示指定のみ採用（既定送信先解決 getUserNotifyTarget は deferred。
        // target_type 未指定時は repo が "dm" へ既定化する）。target_id は既定送信先由来のため None。
        let new = NewReminder {
            message,
            trigger_at,
            repeat_rule,
            target_type: arg_target(&args, "target"),
            target_id: None,
        };

        let reminder = ReminderRepo::new(&self.db)
            .add(&scope_of(ctx), new)
            .await
            .map_err(exec_err)?;

        let repeat_note = match &reminder.repeat_rule {
            Some(rule) => format!("、繰り返し: cron '{rule}'（送信後に次回へ自動再設定）"),
            None => String::new(),
        };
        let message = format!(
            "リマインドを登録しました⏰ (ID: #{})\n送信日時: {} → {}{}",
            reminder.id,
            display_date_time(&reminder.trigger_at),
            target_label(&reminder.target_type, reminder.target_id.as_deref()),
            repeat_note,
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "reminder": reminder_entry(&reminder) }),
        )))
    }
}

// ─── listReminders ───────────────────────────────────────────────────────────

struct ListRemindersTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListRemindersTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "登録してあるリマインドの一覧を見せる。\
                ふだんは送信待ちのものだけ返す。送信済み・キャンセル済みも見たい時は include_all を true にする。\
                キャンセルに使う ID もこの一覧で分かる。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "include_all": { "type": "boolean", "description": "送信済み・キャンセル済みも含めるか。省略=false=送信待ちのみ（任意）" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let include_all = args
            .get("include_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let reminders = ReminderRepo::new(&self.db)
            .list(&scope_of(ctx), include_all)
            .await
            .map_err(exec_err)?;

        if reminders.is_empty() {
            let msg = if include_all {
                "リマインドはありません。"
            } else {
                "送信待ちのリマインドはありません。"
            };
            return Ok(ToolOutcome::from_payload(ok_payload(
                msg,
                json!({ "reminders": [] }),
            )));
        }

        let lines: Vec<String> = reminders
            .iter()
            .map(|r| {
                let base = reminder_line(r);
                if include_all {
                    format!("{base} [{}]", status_label(&r.status))
                } else {
                    base
                }
            })
            .collect();
        let msg = format!(
            "リマインド一覧 ({}件{}):\n{}",
            reminders.len(),
            if include_all { "" } else { "、送信待ちのみ" },
            lines.join("\n"),
        );
        let entries: Vec<Value> = reminders.iter().map(reminder_entry).collect();
        Ok(ToolOutcome::from_payload(ok_payload(
            msg,
            json!({ "reminders": entries }),
        )))
    }
}

// ─── cancelReminder ──────────────────────────────────────────────────────────

struct CancelReminderTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for CancelReminderTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "登録してあるリマインドを取り消す。繰り返しのリマインドも取り消すと以降は送られなくなる。\
                ID が分からない時は先に listReminders で確認してから呼ぶ。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "reminder_id": { "type": "number", "description": "取り消すリマインドの ID（#のあとの番号）" }
                },
                "required": ["reminder_id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(id) = arg_i64(&args, "reminder_id") else {
            return Ok(fail_payload("reminder_id を数値で指定してください。"));
        };

        let scope = scope_of(ctx);
        let repo = ReminderRepo::new(&self.db);
        match repo.cancel(&scope, id).await.map_err(exec_err)? {
            Some(reminder) => Ok(ToolOutcome::from_payload(ok_payload(
                format!(
                    "リマインド「{}」(#{}) をキャンセルしました🚫",
                    reminder.message, reminder.id
                ),
                json!({ "reminder": reminder_entry(&reminder) }),
            ))),
            // 失敗理由を区別（不在 / 既に送信済み・キャンセル済み）。Node と同じく get で実在確認。
            None => match repo.get(&scope, id).await.map_err(exec_err)? {
                None => Ok(fail_payload(format!(
                    "リマインド #{id} が見つかりません。listReminders でIDを確認してください。"
                ))),
                Some(existing) => Ok(fail_payload(format!(
                    "リマインド #{id} は既に{}のためキャンセルできません。",
                    status_label(&existing.status)
                ))),
            },
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

    // lib.rs の #[cfg(test)] REMINDERS_DDL と同一（Node migrations.ts の reminders 表 + bot_id 列）。
    const REMINDERS_DDL: &str = "CREATE TABLE reminders (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        message TEXT NOT NULL,
        trigger_at TEXT NOT NULL,
        repeat_rule TEXT,
        target_type TEXT NOT NULL DEFAULT 'dm',
        target_id TEXT,
        status TEXT NOT NULL DEFAULT 'pending',
        source TEXT NOT NULL DEFAULT 'manual',
        source_id TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_reminder_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(REMINDERS_DDL).unwrap();
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
    async fn add_list_cancel_roundtrip() {
        let db = seed_db();
        let tools = tools(db).unwrap();

        // 宣言名は bare（Node system prompt と一致）。
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.declaration().name.to_string())
            .collect();
        assert!(names.contains(&"addReminder".to_owned()));
        assert!(names.contains(&"listReminders".to_owned()));
        assert!(names.contains(&"cancelReminder".to_owned()));
        assert!(!names.iter().any(|n| n.contains(':')), "native は bare 名");

        // add（snake_case 引数）。
        let add = find(&tools, "addReminder");
        let out = add
            .call(
                &ctx(),
                json!({"message": "水を飲む", "trigger_at": "2999-01-01 09:00:00"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["reminder"]["message"], "水を飲む");
        assert_eq!(out.payload["reminder"]["status"], "pending");
        // toReminderEntry は `reminder_id` キー（`id` ではない）。
        let id = out.payload["reminder"]["reminder_id"].as_i64().unwrap();

        // list（既定=送信待ちのみ）で 1 件見える。
        let list = find(&tools, "listReminders");
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["reminders"].as_array().unwrap().len(), 1);
        assert_eq!(out.payload["reminders"][0]["reminder_id"], id);

        // cancel。
        let cancel = find(&tools, "cancelReminder");
        let out = cancel.call(&ctx(), json!({"reminder_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["reminder"]["status"], "cancelled");

        // 既定一覧からは消える。
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["reminders"].as_array().unwrap().len(), 0);
        // include_all では残る（cancelled）。
        let out = list.call(&ctx(), json!({"include_all": true})).await.unwrap();
        assert_eq!(out.payload["reminders"].as_array().unwrap().len(), 1);
        assert_eq!(out.payload["reminders"][0]["status"], "cancelled");

        // 二重キャンセル（実在するが pending でない）→ success:false。
        let out = cancel.call(&ctx(), json!({"reminder_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        // 不在 ID も success:false。
        let out = cancel
            .call(&ctx(), json!({"reminder_id": 999999}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn add_validates_message_trigger_and_cron() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addReminder");

        // message 欠落 → fail。
        let out = add
            .call(&ctx(), json!({"trigger_at": "2999-01-01 09:00:00"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // trigger_at 欠落 → fail。
        let out = add.call(&ctx(), json!({"message": "x"})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // 不正 cron（5 フィールドでない）→ fail。
        let out = add
            .call(
                &ctx(),
                json!({"message": "x", "trigger_at": "2999-01-01 09:00:00", "repeat_rule": "bad"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // 正しい cron + trigger_at → 成功し repeat_rule が入る。
        let out = add
            .call(
                &ctx(),
                json!({"message": "毎週会議", "trigger_at": "2999-01-01 09:00:00", "repeat_rule": "0 9 * * 1"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["reminder"]["repeat_rule"], "0 9 * * 1");
    }

    #[tokio::test]
    async fn add_channel_target_is_persisted() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addReminder");

        // target=channel は保持。無効値は None（repo が "dm" 既定化）。
        let out = add
            .call(
                &ctx(),
                json!({"message": "x", "trigger_at": "2999-01-01 09:00:00", "target": "channel"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["reminder"]["target_type"], "channel");

        let out = add
            .call(
                &ctx(),
                json!({"message": "y", "trigger_at": "2999-01-01 09:00:00", "target": "bogus"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["reminder"]["target_type"], "dm");
    }

    #[tokio::test]
    async fn scope_isolation_across_users() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addReminder");
        let list = find(&tools, "listReminders");

        add.call(
            &ctx(),
            json!({"message": "A のリマインド", "trigger_at": "2999-01-01 09:00:00"}),
        )
        .await
        .unwrap();

        // 別ユーザーには見えない。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["reminders"].as_array().unwrap().len(), 0);
    }
}
