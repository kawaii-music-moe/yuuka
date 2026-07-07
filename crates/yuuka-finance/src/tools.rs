//! finance ドメインの Native ツール（現行 `src/functions/financeFunctions.ts` の移植・§9.4）。
//!
//! **雛形**: `yuuka-todo::tools`（構造・命名・ok/fail JSON・bare ツール名・UserScope・
//! DbError→`ToolError::Execution`・`tools(db) -> Result<Vec<Arc<dyn Tool>>>` を踏襲）。
//!
//! **wire 契約の非対称**: HTTP route の body は camelCase だが、**tool 引数は snake_case**
//! （Node の Gemini 宣言と一致）。ツール名は Node system prompt が参照する **bare 名**を使う。
//!
//! 移植済み: addExpense（収支記録・`ExpenseRepo::add`）/ listRecentExpenses（直近一覧・`ExpenseRepo::list`）。
//! これらは repo の add/list に素直に対応する。予算・支払い予定・月次集計・消込は対応 repo が
//! 無いため未移植（deferred）。addExpense の予算消化率・消込候補（他 repo 依存）も同様に落とす。
//! `ExpenseRepo` の get/delete には対応する Node ツールが無い（settle 内部で使うのみ）ため公開しない。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope};
use yuuka_web::Db;

use crate::dto::NewExpense;
use crate::repo::ExpenseRepo;

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(AddExpenseTool {
            name: ToolName::checked("addExpense".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(ListRecentExpensesTool {
            name: ToolName::checked("listRecentExpenses".to_owned())?,
            db,
        }),
    ])
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

/// ctx からデータ分離スコープを組む（金銭データ・スコープ厳守）。
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

/// `asOptionalInt`（数値は `Math.trunc` 相当、数値文字列も許容）。
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

/// `isYmd`（'YYYY-MM-DD' 形式か・indexing を避けた実装）。
fn is_ymd(s: &str) -> bool {
    let mut it = s.split('-');
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(y), Some(m), Some(d), None) => {
            y.len() == 4
                && m.len() == 2
                && d.len() == 2
                && y.chars().all(|c| c.is_ascii_digit())
                && m.chars().all(|c| c.is_ascii_digit())
                && d.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// DbError をツール実行エラーへ（握り潰さず Gemini へ `{success:false}` として返る・§8.4）。
fn exec_err(e: DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

/// 収支タイプの日本語ラベル。
fn type_label(etype: &str) -> &'static str {
    if etype == "income" {
        "収入"
    } else {
        "支出"
    }
}

// ─── addExpense ──────────────────────────────────────────────────────────────

struct AddExpenseTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for AddExpenseTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "収入や支出を家計簿に1件記録する。\
                「1200円使った」「給料が振り込まれた」等の記録依頼で呼ぶ。\
                type は 'expense'（支出・既定）/'income'（収入）。日付を省くと今日で記録する。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "amount": { "type": "number", "description": "金額。円単位の1以上の整数（例: 1200）" },
                    "category": { "type": "string", "description": "カテゴリ。支出なら費目、収入なら内容に合う名前（例: 給与）" },
                    "type": { "type": "string", "description": "収入か支出か。'expense'=支出（既定）/'income'=収入（任意）" },
                    "memo": { "type": "string", "description": "メモ。店名・品目・用途など（任意）" },
                    "date": { "type": "string", "description": "日付。形式 YYYY-MM-DD。省略=今日（任意）" },
                    "time": { "type": "string", "description": "時刻。形式 HH:MM:SS（任意）" }
                },
                "required": ["amount", "category"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(amount) = arg_i64(&args, "amount").filter(|n| *n > 0) else {
            return Ok(fail_payload("amount は1以上の整数（円）で指定してください。"));
        };
        let Some(category) = arg_str(&args, "category") else {
            return Ok(fail_payload("category を指定してください。"));
        };
        let date = arg_str(&args, "date");
        if let Some(d) = &date {
            if !is_ymd(d) {
                return Ok(fail_payload("date は YYYY-MM-DD 形式で指定してください。"));
            }
        }

        let new = NewExpense {
            amount,
            category,
            description: arg_str(&args, "memo"),
            date,
            time: arg_str(&args, "time"),
            // repo が income 以外を expense に正規化する（Node parity）。
            r#type: arg_str(&args, "type"),
        };

        let expense = ExpenseRepo::new(&self.db)
            .add(&scope_of(ctx), new)
            .await
            .map_err(exec_err)?;

        // NOTE: 予算消化率・消込候補（budget_limits / planned_payments）は対応 repo が無いため落とす。
        let memo_suffix = expense
            .memo
            .as_ref()
            .map(|m| format!(" — {m}"))
            .unwrap_or_default();
        let message = format!(
            "{} {}円 ({}) を記録しました{} (ID: #{})",
            type_label(&expense.r#type),
            expense.amount,
            expense.category,
            memo_suffix,
            expense.id,
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "expense": expense }),
        )))
    }
}

// ─── listRecentExpenses ──────────────────────────────────────────────────────

struct ListRecentExpensesTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListRecentExpensesTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "最近の収支記録を新しい順に一覧で返す。\
                type で支出だけ・収入だけに絞り込める。count で件数を指定できる（既定10）。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "count": { "type": "number", "description": "取得する件数。省略=10件（任意）" },
                    "type": { "type": "string", "description": "絞り込み。'expense'=支出だけ/'income'=収入だけ。省略=両方（任意）" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // 件数（既定10・最小1）。repo は直近30件を返すため、それ以上は取得できない（SQL 拡張なし）。
        let count = arg_i64(&args, "count").unwrap_or(10).max(1);
        // 収入/支出の絞り込み（exact match のみ・Node parity）。
        let type_filter = match arg_str(&args, "type").as_deref() {
            Some("income") => Some("income".to_owned()),
            Some("expense") => Some("expense".to_owned()),
            _ => None,
        };

        let mut expenses = ExpenseRepo::new(&self.db)
            .list(&scope_of(ctx))
            .await
            .map_err(exec_err)?;

        if let Some(t) = &type_filter {
            expenses.retain(|e| &e.r#type == t);
        }
        // count を上限に切り詰める（repo は既に date 降順）。
        if let Ok(n) = usize::try_from(count) {
            expenses.truncate(n);
        }

        if expenses.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                "収支の記録はありません。",
                json!({ "expenses": [] }),
            )));
        }

        let message = format!("直近の収支記録 ({}件)", expenses.len());
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "expenses": expenses }),
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

    // lib.rs の #[cfg(test)] と同一の実効スキーマ（migrations.ts + bot_id ALTER）。
    const EXPENSES_DDL: &str = "CREATE TABLE expenses (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        type TEXT NOT NULL DEFAULT 'expense',
        amount INTEGER NOT NULL,
        category TEXT NOT NULL,
        memo TEXT,
        date TEXT NOT NULL,
        time TEXT,
        source TEXT NOT NULL DEFAULT 'manual',
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_finance_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(EXPENSES_DDL).unwrap();
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
    async fn add_then_list_roundtrip() {
        let db = seed_db();
        let tools = tools(db).unwrap();

        // 宣言名は bare（Node system prompt と一致）。
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.declaration().name.to_string())
            .collect();
        assert!(names.contains(&"addExpense".to_owned()));
        assert!(names.contains(&"listRecentExpenses".to_owned()));
        assert!(!names.iter().any(|n| n.contains(':')), "native は bare 名");

        // add（snake_case 引数・memo→description、type 正規化）。
        let add = find(&tools, "addExpense");
        let out = add
            .call(
                &ctx(),
                json!({"amount": 1200, "category": "食費", "memo": "ランチ"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["expense"]["amount"], 1200);
        assert_eq!(out.payload["expense"]["category"], "食費");
        assert_eq!(out.payload["expense"]["memo"], "ランチ");
        // 既定は expense / source=manual。内部列は露出しない（金銭データ）。
        assert_eq!(out.payload["expense"]["type"], "expense");
        assert_eq!(out.payload["expense"]["source"], "manual");
        assert!(out.payload["expense"]["user_id"].is_null());
        assert!(out.payload["expense"]["bot_id"].is_null());

        // income を1件追加。
        add.call(&ctx(), json!({"amount": 300000, "category": "給与", "type": "income"}))
            .await
            .unwrap();

        // list（既定）で2件見える。
        let list = find(&tools, "listRecentExpenses");
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["expenses"].as_array().unwrap().len(), 2);

        // type=income で1件に絞れる。
        let out = list.call(&ctx(), json!({"type": "income"})).await.unwrap();
        let arr = out.payload["expenses"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "income");

        // count=1 で1件に切り詰められる。
        let out = list.call(&ctx(), json!({"count": 1})).await.unwrap();
        assert_eq!(out.payload["expenses"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn add_validates_amount_category_and_date() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addExpense");

        // amount 欠落 → fail。
        let out = add.call(&ctx(), json!({"category": "食費"})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // amount <= 0 → fail。
        let out = add
            .call(&ctx(), json!({"amount": 0, "category": "食費"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // category 欠落 → fail。
        let out = add.call(&ctx(), json!({"amount": 100})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // date 不正形式 → fail。
        let out = add
            .call(
                &ctx(),
                json!({"amount": 100, "category": "食費", "date": "2026/07/08"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // date 正形式 → success。
        let out = add
            .call(
                &ctx(),
                json!({"amount": 100, "category": "食費", "date": "2026-07-08"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["expense"]["date"], "2026-07-08");
    }

    #[tokio::test]
    async fn scope_isolation_across_users() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addExpense");
        let list = find(&tools, "listRecentExpenses");

        add.call(&ctx(), json!({"amount": 500, "category": "娯楽"}))
            .await
            .unwrap();

        // 別ユーザーには見えない（金銭データのスコープ厳守）。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["expenses"].as_array().unwrap().len(), 0);
    }
}
