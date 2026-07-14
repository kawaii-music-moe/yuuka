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
            db: db.clone(),
        }),
        Arc::new(GetMonthlySummaryTool {
            name: ToolName::checked("getMonthlySummary".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(GetCategoryBreakdownTool {
            name: ToolName::checked("getCategoryBreakdown".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(GetBudgetLimitsTool {
            name: ToolName::checked("getBudgetLimits".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(SetBudgetLimitTool {
            name: ToolName::checked("setBudgetLimit".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(DeleteBudgetLimitTool {
            name: ToolName::checked("deleteBudgetLimit".to_owned())?,
            db,
        }),
    ])
}

/// 支出カテゴリの許容一覧（Node `expenseRepo.CATEGORIES`）。setBudgetLimit の検証に使う。
const CATEGORIES: [&str; 9] = [
    "食費", "日用品", "交通費", "光熱費", "通信費", "医療費", "娯楽", "衣服", "その他",
];

/// カテゴリ一覧を `, ` 連結（Node `CATEGORY_LIST`・エラーメッセージ用）。
fn category_list() -> String {
    CATEGORIES.join(", ")
}

/// `¥` + 3 桁区切り（Node `formatCurrency` 相当・LLM 向け案内文用）。
fn format_yen(n: i64) -> String {
    let neg = n < 0;
    let digits = n.unsigned_abs().to_string();
    let len = digits.len();
    let mut grouped = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i != 0 && (len - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{}¥{grouped}", if neg { "-" } else { "" })
}

/// `asOptionalInt`（number のみ・trunc・Node は文字列を受けない）。年月・金額に使う。
fn arg_int(args: &Value, key: &str) -> Option<i64> {
    args.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f.trunc() as i64)))
}

/// 集計対象の年月を解決する（引数が無ければ当月）。
async fn resolve_year_month(repo: &ExpenseRepo<'_>, args: &Value) -> Result<(i64, i64), DbError> {
    let (cur_year, cur_month) = repo.current_year_month().await?;
    let year = arg_int(args, "year").unwrap_or(cur_year);
    let month = arg_int(args, "month").unwrap_or(cur_month);
    Ok((year, month))
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

/// 'YYYY年M月' ラベル（Node `currentMonthLabel`）。
fn month_label(year: i64, month: i64) -> String {
    format!("{year}年{month}月")
}

// ─── getMonthlySummary ───────────────────────────────────────────────────────

struct GetMonthlySummaryTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for GetMonthlySummaryTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "指定した月の収支まとめ（収入合計・支出合計・差額・支出のカテゴリ別内訳）を返す。\
                「今月いくら使った？」等で呼ぶ。年と月を省くと今月のまとめを返す。\
                カテゴリの細かい内訳だけ見たい時は getCategoryBreakdown を使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "year": { "type": "number", "description": "年。4桁（例: 2026）。省略=今年" },
                    "month": { "type": "number", "description": "月。1〜12。省略=今月" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let scope = scope_of(ctx);
        let repo = ExpenseRepo::new(&self.db);
        let (year, month) = resolve_year_month(&repo, &args).await.map_err(exec_err)?;
        let label = month_label(year, month);
        let income = repo.monthly_total(&scope, "income", year, month).await.map_err(exec_err)?;
        let expense = repo.monthly_total(&scope, "expense", year, month).await.map_err(exec_err)?;
        let balance = income - expense;
        let breakdown = repo
            .monthly_category_breakdown(&scope, year, month, "expense")
            .await
            .map_err(exec_err)?;

        if income == 0 && expense == 0 {
            return Ok(ToolOutcome::from_payload(ok_payload(
                format!("{label}の収支記録はありません。"),
                json!({ "income": 0, "expense": 0, "balance": 0, "breakdown": [] }),
            )));
        }
        let mut lines = vec![
            format!("📈 収入: {}", format_yen(income)),
            format!("📉 支出: {}", format_yen(expense)),
            format!("💰 収支差: {}", format_yen(balance)),
        ];
        if !breakdown.is_empty() {
            lines.push("───────────".to_owned());
            lines.push("支出の内訳:".to_owned());
            for c in &breakdown {
                lines.push(format!("  {}: {} ({}件)", c.category, format_yen(c.total), c.count));
            }
        }
        Ok(ToolOutcome::from_payload(ok_payload(
            lines.join("\n"),
            json!({ "income": income, "expense": expense, "balance": balance, "breakdown": breakdown }),
        )))
    }
}

// ─── getCategoryBreakdown ────────────────────────────────────────────────────

struct GetCategoryBreakdownTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for GetCategoryBreakdownTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "指定した月のカテゴリ別の内訳（金額・件数・割合）を返す。\
                type で支出（既定）か収入かを切り替える。\
                収支の合計や差額も見たい時は getMonthlySummary を使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "year": { "type": "number", "description": "年。4桁。省略=今年" },
                    "month": { "type": "number", "description": "月。1〜12。省略=今月" },
                    "type": { "type": "string", "description": "'expense'（既定）|'income'" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let etype = if args.get("type").and_then(Value::as_str) == Some("income") {
            "income"
        } else {
            "expense"
        };
        let type_label = if etype == "income" { "収入" } else { "支出" };
        let scope = scope_of(ctx);
        let repo = ExpenseRepo::new(&self.db);
        let (year, month) = resolve_year_month(&repo, &args).await.map_err(exec_err)?;
        let label = month_label(year, month);
        let breakdown = repo
            .monthly_category_breakdown(&scope, year, month, etype)
            .await
            .map_err(exec_err)?;
        let total = repo.monthly_total(&scope, etype, year, month).await.map_err(exec_err)?;

        if breakdown.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                format!("{label}の{type_label}記録はありません。"),
                json!({ "type": etype, "total": 0, "breakdown": [] }),
            )));
        }
        let lines: Vec<String> = breakdown
            .iter()
            .map(|c| {
                let ratio = if total > 0 {
                    format!("{:.1}", (c.total as f64 / total as f64) * 100.0)
                } else {
                    "0".to_owned()
                };
                format!("{}: {} ({}%, {}件)", c.category, format_yen(c.total), ratio, c.count)
            })
            .collect();
        let message = format!(
            "{label}の{type_label}カテゴリ別内訳:\n{}\n合計: {}",
            lines.join("\n"),
            format_yen(total),
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "type": etype, "total": total, "breakdown": breakdown }),
        )))
    }
}

// ─── getBudgetLimits ─────────────────────────────────────────────────────────

struct GetBudgetLimitsTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for GetBudgetLimitsTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "カテゴリごとの毎月の予算上限と、今月の使用額・使用率を返す。\
                予算を新しく決めたり変えたい時は setBudgetLimit を使う。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let scope = scope_of(ctx);
        let repo = ExpenseRepo::new(&self.db);
        let limits = repo.list_budget_limits(&scope).await.map_err(exec_err)?;
        if limits.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                "予算上限が設定されているカテゴリはありません。setBudgetLimit で設定できます。",
                json!({ "limits": [] }),
            )));
        }
        // 当月の支出内訳から消化額を突き合わせる。
        let (year, month) = repo.current_year_month().await.map_err(exec_err)?;
        let breakdown = repo
            .monthly_category_breakdown(&scope, year, month, "expense")
            .await
            .map_err(exec_err)?;
        let entries: Vec<Value> = limits
            .iter()
            .map(|l| {
                let spent = breakdown
                    .iter()
                    .find(|c| c.category == l.category)
                    .map_or(0, |c| c.total);
                let ratio = if l.limit_amount > 0 {
                    ((spent as f64 / l.limit_amount as f64) * 100.0).round() as i64
                } else {
                    0
                };
                json!({
                    "category": l.category,
                    "limit_amount": l.limit_amount,
                    "spent": spent,
                    "ratio_percent": ratio,
                })
            })
            .collect();
        let lines: Vec<String> = entries
            .iter()
            .map(|e| {
                let ratio = e["ratio_percent"].as_i64().unwrap_or(0);
                let warn = if ratio >= 100 {
                    " ⚠️超過"
                } else if ratio >= 80 {
                    " ⚠️"
                } else {
                    ""
                };
                format!(
                    "{}: {}（今月の消化: {} / {}%{}）",
                    e["category"].as_str().unwrap_or(""),
                    format_yen(e["limit_amount"].as_i64().unwrap_or(0)),
                    format_yen(e["spent"].as_i64().unwrap_or(0)),
                    ratio,
                    warn,
                )
            })
            .collect();
        Ok(ToolOutcome::from_payload(ok_payload(
            format!("設定済み予算上限と今月の消化状況:\n{}", lines.join("\n")),
            json!({ "limits": entries }),
        )))
    }
}

// ─── setBudgetLimit ──────────────────────────────────────────────────────────

struct SetBudgetLimitTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for SetBudgetLimitTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: format!(
                "カテゴリの毎月の予算上限を決める、または変える。「食費の予算を3万円にして」等で呼ぶ。\
                 カテゴリは次から選ぶ: {}。今の予算や残りを見るだけなら getBudgetLimits を使う。",
                category_list()
            ),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": format!("予算を決めるカテゴリ。次のどれか: {}", category_list()) },
                    "limit_amount": { "type": "number", "description": "毎月の予算上限。円単位の1以上の整数（例: 30000）" }
                },
                "required": ["category", "limit_amount"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let category = arg_str(&args, "category");
        let valid = category
            .as_deref()
            .is_some_and(|c| CATEGORIES.contains(&c));
        if !valid {
            let raw = args.get("category").and_then(Value::as_str).unwrap_or("");
            return Ok(fail_payload(format!(
                "無効なカテゴリです: {raw}。有効なカテゴリ: {}",
                category_list()
            )));
        }
        let limit_amount = arg_int(&args, "limit_amount");
        let Some(limit_amount) = limit_amount.filter(|n| *n > 0) else {
            return Ok(fail_payload(
                "limit_amount は1以上の整数（円）で指定してください。",
            ));
        };
        let category = category.unwrap_or_default();
        ExpenseRepo::new(&self.db)
            .upsert_budget_limit(&scope_of(ctx), category.clone(), limit_amount)
            .await
            .map_err(exec_err)?;
        Ok(ToolOutcome::from_payload(ok_payload(
            format!(
                "{category} の月次予算上限を {} に設定しました。",
                format_yen(limit_amount)
            ),
            json!({}),
        )))
    }
}

// ─── deleteBudgetLimit ───────────────────────────────────────────────────────

struct DeleteBudgetLimitTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for DeleteBudgetLimitTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "カテゴリの毎月の予算上限を消す。「食費の予算設定を消して」等で呼ぶ。\
                金額を変えたいだけなら setBudgetLimit を使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": "予算上限を消すカテゴリ" }
                },
                "required": ["category"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(category) = arg_str(&args, "category") else {
            return Ok(fail_payload("category を指定してください。"));
        };
        let deleted = ExpenseRepo::new(&self.db)
            .delete_budget_limit(&scope_of(ctx), category.clone())
            .await
            .map_err(exec_err)?;
        if !deleted {
            return Ok(fail_payload(format!(
                "{category} には予算上限が設定されていません。"
            )));
        }
        Ok(ToolOutcome::from_payload(ok_payload(
            format!("{category} の予算上限を削除しました。"),
            json!({}),
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

    // budget_limits を FK 無しで先に作る（V17 の users FK を避ける・todo tools テストと同方針）。
    const BUDGET_LIMITS_DDL: &str = "CREATE TABLE budget_limits (
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        category TEXT NOT NULL,
        limit_amount INTEGER NOT NULL DEFAULT 50000,
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        PRIMARY KEY (user_id, bot_id, category)
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
            conn.execute_batch(BUDGET_LIMITS_DDL).unwrap();
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

    #[tokio::test]
    async fn monthly_summary_breakdown_and_budget_tools() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addExpense");
        let summary = find(&tools, "getMonthlySummary");
        let breakdown = find(&tools, "getCategoryBreakdown");
        let get_budget = find(&tools, "getBudgetLimits");
        let set_budget = find(&tools, "setBudgetLimit");
        let del_budget = find(&tools, "deleteBudgetLimit");

        // 当月の支出/収入を記録（date 省略=今日）。
        add.call(&ctx(), json!({"amount": 3000, "category": "食費"})).await.unwrap();
        add.call(&ctx(), json!({"amount": 2000, "category": "食費"})).await.unwrap();
        add.call(&ctx(), json!({"amount": 1000, "category": "娯楽"})).await.unwrap();
        add.call(&ctx(), json!({"amount": 50000, "category": "給与", "type": "income"}))
            .await
            .unwrap();

        // getMonthlySummary: 収入50000/支出6000/差44000。
        let out = summary.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["income"], 50000);
        assert_eq!(out.payload["expense"], 6000);
        assert_eq!(out.payload["balance"], 44000);
        assert_eq!(out.payload["breakdown"].as_array().unwrap().len(), 2);

        // getCategoryBreakdown（支出既定）: 食費5000（合計6000の内訳）。
        let out = breakdown.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["type"], "expense");
        assert_eq!(out.payload["total"], 6000);
        let bd = out.payload["breakdown"].as_array().unwrap();
        assert_eq!(bd[0]["category"], "食費");
        assert_eq!(bd[0]["total"], 5000);
        // income 指定。
        let out = breakdown.call(&ctx(), json!({"type": "income"})).await.unwrap();
        assert_eq!(out.payload["total"], 50000);

        // setBudgetLimit: 無効カテゴリ拒否・非正数拒否・正常設定。
        let out = set_budget.call(&ctx(), json!({"category": "宇宙旅行", "limit_amount": 10000})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        let out = set_budget.call(&ctx(), json!({"category": "食費", "limit_amount": 0})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        let out = set_budget.call(&ctx(), json!({"category": "食費", "limit_amount": 4000})).await.unwrap();
        assert_eq!(out.payload["success"], true);

        // getBudgetLimits: 食費 消化5000/上限4000 → 125%（超過）。
        let out = get_budget.call(&ctx(), json!({})).await.unwrap();
        let limits = out.payload["limits"].as_array().unwrap();
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0]["category"], "食費");
        assert_eq!(limits[0]["spent"], 5000);
        assert_eq!(limits[0]["ratio_percent"], 125);

        // deleteBudgetLimit: 設定済みは削除成功、未設定は fail。
        let out = del_budget.call(&ctx(), json!({"category": "食費"})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        let out = del_budget.call(&ctx(), json!({"category": "食費"})).await.unwrap();
        assert_eq!(out.payload["success"], false);
    }
}
