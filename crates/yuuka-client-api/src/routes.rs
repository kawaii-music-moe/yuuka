//! `/api/client/*` ルートハンドラ（PWA・全ルート `auth:"user"`・Node `clientRoutes.ts` パリティ）。
//!
//! PWA は常に秘書 Bot（`system_default`）に束縛される（Node `const BOT_ID = "system_default"`）。
//! `?botId=` は存在しない（一般ダッシュボードの `resolve_scope`/`ScopedJson` は使わない）。
//!
//! チャット送信（`POST /api/client/chat/messages`・issue #41）は [`ChatEngine::secretary_turn_pwa`]
//! （`source='pwa'`）を通し、WS/Discord 秘書経路と同じ [`yuuka_discord::RateLimiter`]/
//! [`ChatEngine::user_has_gemini_key`] の事前チェックを Gemini 呼び出し前に行う（詳細は [`chat_send`]）。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use tokio::time::timeout;
use yuuka_core::{BotId, GuildId, UserId, UserScope};
use yuuka_discord::{rate_limit_message, IncomingChat, RateLimiter, StatusSink};
use yuuka_finance::dto::{Expense, NewExpense};
use yuuka_finance::repo::ExpenseRepo;
use yuuka_gemini::ALLOWED_MODELS;
use yuuka_orchestrator::{context_note, message_log, ChatEngine};
use yuuka_persona::dto::{SavePersona, PERSONA_MAX_LENGTH};
use yuuka_persona::repo::PersonaRepo;
use yuuka_schedule::repo::ScheduleRepo;
use yuuka_todo::dto::{NewTodo, Todo, TodoUpdate};
use yuuka_todo::repo::TodoRepo;
use yuuka_web::{AppState, AuthenticatedUser, Db};

use crate::date_util::{
    day_start, local_to_utc_iso, next_day_start, parse_local_date, parse_local_month,
};
use crate::dto::{
    CalendarEventView, ChatMessageView, ChatSendInput, FinanceSummaryView, NewTodoInput,
    NewTransactionInput, SettingsUpdate, SettingsView, SharedNoteUpdate, SharedNoteView,
    StatusView, TodoPatch, TodoView, TransactionView,
};
use crate::error::ClientApiError;
use crate::references::infer_reference;
use crate::users;

/// PWA が束縛される Bot（Node `const BOT_ID = "system_default"`）。
const BOT_ID: &str = "system_default";
/// チャット履歴取得の上限（`message_log::list_pwa_messages` が内部で `[1,200]` にクランプする）。
const CHAT_HISTORY_LIMIT: i64 = 200;
/// チャット送信レート制限の guildId スロット（issue #41・`yuuka-finance` の `"web"` と同型）。
///
/// [`InMemoryRateLimiter`](yuuka_orchestrator::InMemoryRateLimiter)（実装）のユーザー分/日単位の窓は
/// `bot_id × user_id` のみで消費されるため、この文字列は「ギルド日次上限」バケットを他経路
/// （Discord の実ギルド ID・finance の `"web"`）と分けるためのラベルに過ぎない。ユーザー単位の
/// 上限（Bot 設定のレート制限）は Discord/WS と共有され、PWA 経由で迂回することはできない。
const PWA_RATE_LIMIT_GUILD: &str = "pwa";
/// チャット送信 1 ターンのタイムアウト（issue #41）。[`chat_send`] のドキュメント参照。
const CHAT_TURN_TIMEOUT: Duration = Duration::from_secs(180);

/// `/api/client/*` ルータ（`AppState` 上で他ドメインと同様に `merge` される）。
///
/// `engine`/`rate_limiter` はチャット送信（`POST /api/client/chat/messages`）専用で、
/// `Extension` として内包する（supervisor 側で実インスタンスを注入・`yuuka_finance::routes_with` と
/// 同型の DI）。他のハンドラは参照しない。
pub fn routes_with(
    engine: Arc<ChatEngine>,
    rate_limiter: Arc<dyn RateLimiter>,
) -> Router<AppState> {
    Router::new()
        .route("/api/client/status", get(status))
        .route("/api/client/settings", get(settings_get).put(settings_put))
        .route("/api/client/shared-note", get(note_get).put(note_put))
        .route("/api/client/todos", get(todos_list).post(todos_add))
        .route("/api/client/todos/{id}", patch(todos_patch))
        .route("/api/client/calendar/events", get(calendar_events))
        .route("/api/client/finance/summary", get(finance_summary))
        .route(
            "/api/client/finance/transactions",
            get(finance_transactions).post(finance_add),
        )
        .route(
            "/api/client/chat/messages",
            get(chat_history).post(chat_send),
        )
        .layer(Extension(engine))
        .layer(Extension(rate_limiter))
}

fn scope_for(user_id: &str) -> UserScope {
    UserScope::new(UserId::new(user_id.to_owned()), BotId::new(BOT_ID))
}

// ─── status ──────────────────────────────────────────────────────────────────

async fn status(_user: AuthenticatedUser) -> Json<StatusView> {
    Json(StatusView {
        status: "ok",
        service: "yuuka",
        checked_at: now_iso(),
    })
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ─── settings ────────────────────────────────────────────────────────────────

async fn settings_get(
    user: AuthenticatedUser,
    State(db): State<Db>,
) -> Result<Json<SettingsView>, ClientApiError> {
    let uid = user.0.discord_id.as_str();
    let current = users::get_agent_settings(&db, uid).await?;
    let model = current
        .as_ref()
        .and_then(|c| c.model.clone())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| yuuka_gemini::DEFAULT_MODEL.to_owned());
    let google_connected = current.map(|c| c.google_connected).unwrap_or(false);
    let persona = active_persona_prompt(&db, uid).await?;
    Ok(Json(SettingsView {
        google_connected,
        // issue #47: Node はここに生の calendarId を返していた。人間可読な代替（連携先の表示名）が
        // 無いため、内部 ID を漏らすくらいなら省略する（クライアントは未設定時の案内文にフォール
        // バックする・`client/pwa/src/pages/SettingsPage.vue`）。
        google_account: None,
        model,
        max_tokens: 2048,
        temperature: 0.7,
        persona,
    }))
}

async fn settings_put(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<SettingsUpdate>,
) -> Result<Json<SettingsView>, ClientApiError> {
    let uid = user.0.discord_id.clone();
    let current = users::get_agent_settings(&db, &uid).await?;

    let requested = body
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let model = match requested {
        Some(m) => {
            // issue #39: Gemini 以外のモデル名を無検証で保存すると、以後そのユーザーの
            // Discord を含む全会話が Gemini 呼び出し失敗で壊れる。唯一の正の許可リスト
            // （`yuuka_gemini::ALLOWED_MODELS`）に対して検証し、そうでなければ 400 で拒否する。
            if !ALLOWED_MODELS.contains(&m) {
                return Err(ClientApiError::bad_request(format!(
                    "model must be one of: {}",
                    ALLOWED_MODELS.join(", ")
                )));
            }
            m.to_owned()
        }
        None => current
            .as_ref()
            .and_then(|c| c.model.clone())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| yuuka_gemini::DEFAULT_MODEL.to_owned()),
    };

    // persona は先に検証だけ済ませる。モデルを書いた後に persona 側で失敗すると
    // 「モデルだけ更新された部分状態」になる（issue #47: Node の `validatePersonaInput` 例外が
    // まさにこれで 500 になっていた）。
    if let Some(persona_text) = &body.persona {
        if persona_text.encode_utf16().count() > PERSONA_MAX_LENGTH {
            return Err(ClientApiError::bad_request(format!(
                "persona prompt exceeds {PERSONA_MAX_LENGTH} chars"
            )));
        }
    }

    if current.is_some() {
        users::set_gemini_model(&db, &uid, &model).await?;
    }
    if let Some(persona_text) = &body.persona {
        save_active_persona_prompt(&db, &uid, persona_text).await?;
    }

    let google_connected = current.map(|c| c.google_connected).unwrap_or(false);
    let persona = active_persona_prompt(&db, &uid).await?;
    Ok(Json(SettingsView {
        google_connected,
        google_account: None,
        model,
        // Node と同じく永続化しない（入力のエコーのみ・再読み込みで既定値へ戻る)。
        max_tokens: js_number_or(body.max_tokens.as_ref(), 2048.0) as i64,
        temperature: js_number_or(body.temperature.as_ref(), 0.7),
        persona,
    }))
}

/// `Number(x) || default`（JS の falsy フォールバック）を近似する。
fn js_number_or(value: Option<&serde_json::Value>, default: f64) -> f64 {
    let n = match value {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    match n {
        Some(v) if v != 0.0 && v.is_finite() => v,
        _ => default,
    }
}

/// 秘書 Bot の適用中ペルソナ本文（未設定は空文字・Node `getPersonaPrompt`）。
async fn active_persona_prompt(db: &Db, user_id: &str) -> Result<String, ClientApiError> {
    let scope = scope_for(user_id);
    let repo = PersonaRepo::new(db);
    let Some(id) = repo.active_persona_id(&scope).await? else {
        return Ok(String::new());
    };
    Ok(repo
        .get(&scope, id)
        .await?
        .map(|p| p.prompt)
        .unwrap_or_default())
}

/// 秘書 Bot の適用中ペルソナへ `prompt` を保存する（Node `savePersonaPrompt`）。
///
/// 適用中ペルソナがあれば `name` は据え置きで `prompt` のみ更新、無ければ `prompt` が空でない
/// 場合のみ新規ペルソナ（`"PWA persona"`）を作って適用する。
async fn save_active_persona_prompt(
    db: &Db,
    user_id: &str,
    prompt: &str,
) -> Result<(), ClientApiError> {
    let scope = scope_for(user_id);
    let repo = PersonaRepo::new(db);
    if let Some(id) = repo.active_persona_id(&scope).await? {
        if let Some(existing) = repo.get(&scope, id).await? {
            repo.update(
                &scope,
                id,
                SavePersona {
                    id: None,
                    name: existing.name,
                    prompt: prompt.to_owned(),
                },
            )
            .await?;
            return Ok(());
        }
    }
    if !prompt.trim().is_empty() {
        let created = repo
            .add(
                &scope,
                SavePersona {
                    id: None,
                    name: "PWA persona".to_owned(),
                    prompt: prompt.to_owned(),
                },
            )
            .await?;
        repo.set_active(&scope, Some(created.id)).await?;
    }
    Ok(())
}

// ─── shared-note ─────────────────────────────────────────────────────────────

async fn note_get(
    user: AuthenticatedUser,
    State(db): State<Db>,
) -> Result<Json<SharedNoteView>, ClientApiError> {
    let uid = user.0.discord_id.as_str();
    match context_note::get_note(&db, uid, BOT_ID).await? {
        Some(note) => Ok(Json(SharedNoteView {
            id: "shared-note",
            title: note.title,
            body: note.body,
            updated_at: local_to_utc_iso(&note.updated_at).unwrap_or(note.updated_at),
        })),
        None => Ok(Json(SharedNoteView {
            id: "shared-note",
            title: "Shared note".to_owned(),
            body: String::new(),
            updated_at: now_iso(),
        })),
    }
}

async fn note_put(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<SharedNoteUpdate>,
) -> Result<Json<SharedNoteView>, ClientApiError> {
    let uid = user.0.discord_id.as_str();
    let body_text = body.body.unwrap_or_default();
    if body_text.encode_utf16().count() > context_note::CONTEXT_NOTE_MAX_LENGTH {
        return Err(ClientApiError::bad_request(format!(
            "shared note exceeds {} chars",
            context_note::CONTEXT_NOTE_MAX_LENGTH
        )));
    }
    let title = body.title.unwrap_or_default();
    // issue #47: Node は body.title を受け取りながら永続化せずエコーするだけだった。
    context_note::set_note(&db, uid, BOT_ID, &title, &body_text).await?;
    let note = context_note::get_note(&db, uid, BOT_ID)
        .await?
        .ok_or_else(|| ClientApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal".to_owned(),
        })?;
    Ok(Json(SharedNoteView {
        id: "shared-note",
        title: note.title,
        body: note.body,
        updated_at: local_to_utc_iso(&note.updated_at).unwrap_or(note.updated_at),
    }))
}

// ─── todos ───────────────────────────────────────────────────────────────────

fn map_todo(t: Todo) -> TodoView {
    TodoView {
        id: t.id.to_string(),
        title: t.title,
        due_date: t.due_date,
        completed: t.status == "done",
        list: t.list,
    }
}

async fn todos_list(
    user: AuthenticatedUser,
    State(db): State<Db>,
) -> Result<Json<Vec<TodoView>>, ClientApiError> {
    let scope = scope_for(&user.0.discord_id);
    // issue #33: PWA の一覧は open/done を問わず全件（Node `listTodos({status:"all"})`）。
    let items = TodoRepo::new(&db).list_all_flat(&scope).await?;
    Ok(Json(items.into_iter().map(map_todo).collect()))
}

async fn todos_add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<NewTodoInput>,
) -> Result<(StatusCode, Json<TodoView>), ClientApiError> {
    let title = body.title.as_deref().unwrap_or_default().trim().to_owned();
    if title.is_empty() {
        return Err(ClientApiError::bad_request("title is required"));
    }
    let scope = scope_for(&user.0.discord_id);
    let new = NewTodo {
        title,
        due_date: body.due_date.filter(|s| !s.is_empty()),
        // issue #47: PWA が送った list をそのまま使う（Node は保存せず常に "Personal" 固定だった）。
        list: body.list,
        ..Default::default()
    };
    let created = TodoRepo::new(&db).add(&scope, new).await?;
    Ok((StatusCode::CREATED, Json(map_todo(created))))
}

async fn todos_patch(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Path(id): Path<String>,
    Json(body): Json<TodoPatch>,
) -> Result<Json<TodoView>, ClientApiError> {
    let Ok(id) = id.parse::<i64>() else {
        return Err(ClientApiError::not_found("Not found"));
    };
    let scope = scope_for(&user.0.discord_id);
    let status = if body.completed == Some(true) {
        "done"
    } else {
        "open"
    };
    let update = TodoUpdate {
        id,
        status: Some(status.to_owned()),
        ..Default::default()
    };
    match TodoRepo::new(&db).update(&scope, id, update).await? {
        Some(t) => Ok(Json(map_todo(t))),
        None => Err(ClientApiError::not_found("Not found")),
    }
}

// ─── calendar ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CalendarQuery {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

async fn calendar_events(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<CalendarQuery>,
) -> Result<Json<Vec<CalendarEventView>>, ClientApiError> {
    // issue #43: from/to は PWA が送るローカル暦日（YYYY-MM-DD）。サーバーも同じ暦で境界を作る
    // （UTC 変換は行わない・JST 前提のデプロイと SQLite localtime 列に一致させる）。
    let today = chrono::Local::now().date_naive();
    let from = match q.from.as_deref() {
        Some(s) => parse_local_date(s)
            .ok_or_else(|| ClientApiError::bad_request("from must be YYYY-MM-DD"))?,
        None => today,
    };
    let to = match q.to.as_deref() {
        Some(s) => parse_local_date(s)
            .ok_or_else(|| ClientApiError::bad_request("to must be YYYY-MM-DD"))?,
        None => today + chrono::Duration::days(31),
    };

    let scope = scope_for(&user.0.discord_id);
    let rows = ScheduleRepo::new(&db)
        .list_in_range(&scope, &day_start(from), &next_day_start(to))
        .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        // issue #47: 解釈できない start_at が 1 件あっても全体を 500 にせず、その行だけ除外する。
        let Some(starts_at) = local_to_utc_iso(&row.start_at) else {
            tracing::warn!(
                schedule_id = row.id,
                "start_at をパースできないため calendar events から除外"
            );
            continue;
        };
        let ends_at = row
            .end_at
            .as_deref()
            .and_then(local_to_utc_iso)
            .unwrap_or_else(|| starts_at.clone());
        let (calendar, calendar_name) = match row.google_calendar_id.filter(|s| !s.is_empty()) {
            Some(id) => (id, "Google カレンダー".to_owned()),
            None => ("Personal".to_owned(), "Personal".to_owned()),
        };
        out.push(CalendarEventView {
            id: row.id.to_string(),
            title: row.title,
            starts_at,
            ends_at,
            calendar,
            calendar_name,
            color: "#155eef",
        });
    }
    Ok(Json(out))
}

// ─── finance ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct MonthQuery {
    #[serde(default)]
    month: Option<String>,
}

fn resolve_month(raw: Option<&str>) -> Result<(i64, i64), ClientApiError> {
    use chrono::Datelike;
    match raw {
        Some(s) => parse_local_month(s)
            .map(|(y, m)| (i64::from(y), i64::from(m)))
            .ok_or_else(|| ClientApiError::bad_request("month must be YYYY-MM")),
        None => {
            let now = chrono::Local::now();
            Ok((i64::from(now.year()), i64::from(now.month())))
        }
    }
}

async fn finance_summary(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<MonthQuery>,
) -> Result<Json<FinanceSummaryView>, ClientApiError> {
    let (year, month) = resolve_month(q.month.as_deref())?;
    let scope = scope_for(&user.0.discord_id);
    let repo = ExpenseRepo::new(&db);
    let income = repo.monthly_total(&scope, "income", year, month).await?;
    let expense = repo.monthly_total(&scope, "expense", year, month).await?;
    Ok(Json(FinanceSummaryView {
        income,
        expense,
        balance: income - expense,
        month: format!("{year}-{month:02}"),
    }))
}

fn map_transaction(e: Expense) -> TransactionView {
    TransactionView {
        id: e.id.to_string(),
        date: e.date,
        description: e.memo.unwrap_or_else(|| e.category.clone()),
        category: e.category,
        amount: e.amount,
        kind: e.r#type,
    }
}

async fn finance_transactions(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<MonthQuery>,
) -> Result<Json<Vec<TransactionView>>, ClientApiError> {
    // issue #47: Node は month を無視し常に直近 100 件を返していた。
    let (year, month) = resolve_month(q.month.as_deref())?;
    let scope = scope_for(&user.0.discord_id);
    let rows = ExpenseRepo::new(&db)
        .list_by_month(&scope, year, month)
        .await?;
    Ok(Json(rows.into_iter().map(map_transaction).collect()))
}

fn json_positive_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    let n = match value {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(n as i64)
}

async fn finance_add(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<NewTransactionInput>,
) -> Result<(StatusCode, Json<TransactionView>), ClientApiError> {
    let amount = json_positive_i64(body.amount.as_ref());
    let category = body
        .category
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (Some(amount), Some(category)) = (amount, category) else {
        return Err(ClientApiError::bad_request(
            "amount and category are required",
        ));
    };
    let scope = scope_for(&user.0.discord_id);
    let kind = if body.kind.as_deref() == Some("income") {
        "income"
    } else {
        "expense"
    };
    let new = NewExpense {
        amount,
        category: category.to_owned(),
        description: body.description.filter(|s| !s.is_empty()),
        date: body.date.filter(|s| !s.is_empty()),
        time: None,
        r#type: Some(kind.to_owned()),
    };
    let created = ExpenseRepo::new(&db).add(&scope, new).await?;
    Ok((StatusCode::CREATED, Json(map_transaction(created))))
}

// ─── chat（履歴のみ・送信は issue #41 で別 PR） ────────────────────────────────

async fn chat_history(
    user: AuthenticatedUser,
    State(db): State<Db>,
) -> Result<Json<Vec<ChatMessageView>>, ClientApiError> {
    let uid = user.0.discord_id.as_str();
    let rows = message_log::list_pwa_messages(&db, uid, BOT_ID, CHAT_HISTORY_LIMIT).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let is_assistant = row.role == "assistant";
        let role: &'static str = if is_assistant { "agent" } else { "user" };
        let created_at =
            local_to_utc_iso(&row.created_at).unwrap_or_else(|| row.created_at.clone());
        let references = if is_assistant {
            infer_reference(&row.content).map(|r| vec![r])
        } else {
            None
        };
        out.push(ChatMessageView {
            id: row.id.to_string(),
            role,
            content: row.content,
            created_at,
            references,
        });
    }
    Ok(Json(out))
}

/// `POST /api/client/chat/messages`（チャット送信・issue #41）。
///
/// Node の `POST /api/client/chat/messages` は `processMessage` を直接呼んでおり、WS/Discord
/// 秘書経路が持つ事前チェック（レート制限・Gemini キー事前判定）を欠いていた（#41）。ここでは
/// Gemini を呼ぶ**前**に、WS（`crates/yuuka-supervisor/src/ws.rs`）と同じ 2 段の事前チェックを行う:
///
/// 1. [`ChatEngine::user_has_gemini_key`] — 未設定なら 400（WS の `error/no_gemini_key` 相当）。
/// 2. `rate_limiter.consume`（[`yuuka_discord::RateLimiter`]・Discord 汎用モード/finance receipt と
///    同じ [`yuuka_orchestrator::InMemoryRateLimiter`] シーム）— 超過なら 429。ユーザー単位の窓は
///    `bot_id × user_id` のみで消費されるため、Discord/WS で使い切った分は PWA でも消費済みのまま
///    （= Bot のレート制限設定を PWA 経由で迂回できない）。
///
/// 通過後は [`ChatEngine::secretary_turn_pwa`]（`source='pwa'`）へ委譲する。history/context の
/// 読み書きは PWA 専用 floor を経由し、Discord/WS の会話と混ざらない（#38 と同種の再発防止）。
///
/// # 同期応答について
/// PWA クライアント（`client/pwa/src/api/http.ts` の `request()`）は `fetch` の完了をそのまま待つ
/// だけで、ポーリング/SSE/WS のような非同期受信経路を持たない。加えて、Rust のオーケストレーション
/// エンジン自体が現状**非同期配信（deferred/`TurnDelivery`）を未実装**で、WS/Discord/レシート OCR
/// いずれも `ChatEngine` を同期呼び出ししている（`crates/yuuka-supervisor/src/ws.rs` のモジュール doc
/// 参照）。本 PR の範囲でここだけ非同期配信を新設するのはスコープを大きく超えるため、本エンドポイントも
/// 同期のまま維持し、[`CHAT_TURN_TIMEOUT`] でターン全体を打ち切る。目的は主にリバースプロキシ
/// （issue #41 が言及する Cloudflare Tunnel 等）のタイムアウトより手前で、素の接続切断ではなく
/// 整形された JSON エラー（504）を返し、ハングしたリクエストでサーバーリソースを専有し続けないこと。
async fn chat_send(
    user: AuthenticatedUser,
    Extension(engine): Extension<Arc<ChatEngine>>,
    Extension(rate_limiter): Extension<Arc<dyn RateLimiter>>,
    Json(body): Json<ChatSendInput>,
) -> Result<(StatusCode, Json<ChatMessageView>), ClientApiError> {
    let content = body
        .content
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if content.is_empty() {
        return Err(ClientApiError::bad_request("content is required"));
    }

    let uid = user.0.discord_id.as_str();
    // 事前チェック 1/2: Gemini キー未設定（WS `error/no_gemini_key` と同じ判定・Node には無かった）。
    if !engine.user_has_gemini_key(uid).await {
        return Err(ClientApiError {
            status: StatusCode::BAD_REQUEST,
            message: "Gemini APIキーが未設定です。管理画面から設定してください。".to_owned(),
        });
    }

    let bot_id = BotId::new(BOT_ID);
    let user_id = UserId::new(uid.to_owned());

    // 事前チェック 2/2: レート制限。
    let decision = rate_limiter
        .consume(&bot_id, &GuildId::new(PWA_RATE_LIMIT_GUILD), &user_id)
        .await;
    if let Some(exceeded) = decision.exceeded {
        return Err(ClientApiError {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: rate_limit_message(exceeded),
        });
    }

    let incoming = IncomingChat {
        text: content,
        ..IncomingChat::default()
    };
    // PWA は status（thinking/writing）フレームを消費しない同期 HTTP 応答のため no-op sink。
    let status: StatusSink = Arc::new(|_| {});
    let reply = match timeout(
        CHAT_TURN_TIMEOUT,
        engine.secretary_turn_pwa(&bot_id, &user_id, incoming, &status),
    )
    .await
    {
        Ok(Ok(reply)) => reply,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "client-api: チャット送信ターンが失敗");
            return Err(ClientApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: "internal".to_owned(),
            });
        }
        Err(_) => {
            tracing::warn!(timeout = ?CHAT_TURN_TIMEOUT, "client-api: チャット送信がタイムアウト");
            return Err(ClientApiError {
                status: StatusCode::GATEWAY_TIMEOUT,
                message: "応答の生成に時間がかかりすぎました。もう一度お試しください。".to_owned(),
            });
        }
    };

    Ok((
        StatusCode::CREATED,
        Json(ChatMessageView {
            id: format!("pwa-{}", chrono::Utc::now().timestamp_millis()),
            role: "agent",
            content: reply.text.clone(),
            created_at: now_iso(),
            references: infer_reference(&reply.text).map(|r| vec![r]),
        }),
    ))
}
