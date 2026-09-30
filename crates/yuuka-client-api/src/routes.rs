//! `/api/client/*` ルートハンドラ（PWA・全ルート `auth:"user"`・Node `clientRoutes.ts` パリティ）。
//!
//! PWA は常に秘書 Bot（`system_default`）に束縛される（Node `const BOT_ID = "system_default"`）。
//! `?botId=` は存在しない（一般ダッシュボードの `resolve_scope`/`ScopedJson` は使わない）。
//!
//! チャット送信（`POST /api/client/chat/messages`・issue #41）は事前チェック（Gemini キー・
//! 同時ターン・レート制限）を同期で行った後、ターン本体（[`ChatEngine::secretary_turn_pwa`]・
//! `source='pwa'`）を `tokio::spawn` でバックグラウンド実行し、`202 Accepted` を即座に返す
//! （issue #41 PR #75 レビュー・P1: 非同期配信）。クライアントは `GET /api/client/chat/messages`
//! （[`chat_history`]）をポーリングして完了を検知する。詳細は [`chat_send`] のドキュメント参照。
//!
//! リッチ返信（embeds/files）は [`chat_history`] の応答へ含め、ファイル本体は
//! `GET /api/client/chat/attachments/:id`（[`chat_attachment`]・所有者スコープ）で配信する
//! （issue #41 PR #75 レビュー・P2）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::{get, patch};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use tokio::time::timeout;
use yuuka_core::{BotId, GuildId, UserId, UserScope};
use yuuka_discord::{rate_limit_message, IncomingChat, RateLimiter, RichEmbed, StatusSink};
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
    CalendarEventView, ChatEmbedFieldView, ChatEmbedView, ChatFileView, ChatMessageView,
    ChatSendAccepted, ChatSendInput, FinanceSummaryView, NewTodoInput, NewTransactionInput,
    SettingsUpdate, SettingsView, SharedNoteUpdate, SharedNoteView, StatusView, TodoPatch,
    TodoView, TransactionView,
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
/// 上限（Bot 設定のレート制限）は Discord/WS と共有され、PWA 経由で迂回することはできない
/// （main.rs で `InMemoryRateLimiter` を 1 インスタンスだけ作り Discord/PWA 双方へ渡す・
/// issue #41 PR #75 レビュー・P1）。
const PWA_RATE_LIMIT_GUILD: &str = "pwa";
/// チャット送信 1 ターン（バックグラウンド実行分）のタイムアウト。[`chat_send`] のドキュメント参照。
const CHAT_TURN_TIMEOUT: Duration = Duration::from_secs(180);
/// バックグラウンドターンが `Err` で終わった際に保存するフォールバック応答文言。
const FALLBACK_ERROR_TEXT: &str = "⚠️ 応答の生成中にエラーが発生しました。もう一度お試しください。";
/// バックグラウンドターンが [`CHAT_TURN_TIMEOUT`] を超えた際に保存するフォールバック応答文言。
const FALLBACK_TIMEOUT_TEXT: &str =
    "⚠️ 応答の生成に時間がかかりすぎました。もう一度お試しください。";

/// `/api/client/*` ルータ（`AppState` 上で他ドメインと同様に `merge` される）。
///
/// `engine`/`rate_limiter` はチャット送信（`POST /api/client/chat/messages`）専用で、
/// `Extension` として内包する（supervisor 側で実インスタンスを注入・`yuuka_finance::routes_with` と
/// 同型の DI）。`InFlightTurns`（同時ターン防止・issue #41 PR #75 レビュー・P1）はこのルータ内で
/// 完結する状態のため、ここで新規生成して内包する（呼び出し側からの注入は不要）。
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
        .route("/api/client/chat/attachments/{id}", get(chat_attachment))
        .layer(Extension(engine))
        .layer(Extension(rate_limiter))
        .layer(Extension(InFlightTurns::default()))
}

// ─── 同時ターン防止（issue #41 PR #75 レビュー・P1: 非同期配信） ──────────────────

/// 同一ユーザーにつき同時に 1 ターンだけを許可するゲート。PWA は常に `BOT_ID` 固定（`system_default`）
/// なので user_id のみで足りる（`bot_id × user_id` の複合キーにする必要が無い）。
///
/// **設計選択: 直列化ではなく即時 409 拒否。** [`yuuka_discord::TurnGate`]（Discord 経路の
/// bot×channel 直列化・待って後で実行する）とは異なり、ここでは待たせず即座に拒否する。
/// チャット UI で「前の返信がまだ来ていないのに次を送った」は二重送信/連打である可能性が高く、
/// 無言でバックグラウンドへ積み上げるより、即座にエラーを返して「応答を待ってから再送してください」
/// と伝える方がユーザーにとって分かりやすく、無制限にターンが積み上がる（＝サーバーリソースを
/// 使い続ける）事態も避けられる。
///
/// # 「応答が見えた」時点でゲートは塞がない（issue #77 の PR #83 フォロー: 409 の誤発火の解消）
/// ターンの終端行（応答/通知）の永続化は [`ChatEngine::secretary_turn_pwa`] の内部で行われ、RAII の
/// [`InFlightGuard`] は永続化**より後**（バックグラウンドタスクが戻る時点）に drop される。この差の間に
/// ポーラーが応答を見て次を送ると、ターンは実質完了しているのに 409 が返る誤発火が起きていた
/// （PWA は応答が見えた直後に次の送信を許すので実 UX でも起きる）。
///
/// **ガードを永続化より前に外す案は採らない**（逆向きの窓）: ガードが外れた後・応答が保存される前に
/// 次の送信が通ると、次のユーザー発言が古い応答**より前**に保存され、次ターンの `sinceId` が古い応答より
/// 小さくなるため、次ターンのポーラーが古い応答を自分の返信と取り違える（文脈にも入らない）。二重ターンの
/// 実害になる。
///
/// そこで、ガードの解放を早める代わりに「ゲートが塞がっているか」を**永続化から導出**する。
/// 枠（[`Slot`]）は受理した発言の `since_id`（[`accept_pwa_user_message`](message_log::accept_pwa_user_message)
/// の返り値）を記録し、[`acquire`](Self::acquire) は枠が埋まっていても
/// [`message_log::has_pwa_assistant_reply_after`] で「`since_id` より後の終端行が既にコミット済み」と
/// 確認できれば、そのターンは完了済みとみなして枠を引き継ぐ。ポーラーが応答を見た時点で終端行は
/// コミット済みなので、以降の読み取りは必ずそれを見る＝**ポーラーが応答を観測した後の送信が 409 になる
/// ことは無い**（タイミング非依存）。逆向きの窓は存在しない: 引き継ぎは終端行がコミット済みと確認できた
/// ときだけで、それ以前（実行中・`since_id` 未確定・読み取り失敗）は従来どおり 409 で閉じる
/// （fail-closed）。旧ターンの [`InFlightGuard`] は後から drop されるが、トークン照合により引き継いだ
/// 新しい枠は解放しない。RAII ガードは panic/失敗時（終端行が書けない経路）の解放としてそのまま残る。
#[derive(Clone, Default)]
struct InFlightTurns(Arc<StdMutex<InFlightState>>);

#[derive(Default)]
struct InFlightState {
    /// 枠の所有者を識別する単調増加トークン（[`InFlightGuard`] が自分の枠だけを解放するために使う）。
    next_token: u64,
    slots: HashMap<String, Slot>,
}

/// 進行中ターンの枠。
struct Slot {
    token: u64,
    /// 受理（ユーザー発言の永続化）済みなら、そのターンの `sinceId`。`None` は受理前
    /// （事前チェック〜永続化の間）で、この間は必ず「進行中」として扱う。
    since_id: Option<i64>,
}

impl InFlightState {
    fn insert(&mut self, key: &str) -> u64 {
        self.next_token += 1;
        let token = self.next_token;
        self.slots.insert(
            key.to_owned(),
            Slot {
                token,
                since_id: None,
            },
        );
        token
    }
}

impl InFlightTurns {
    fn lock(&self) -> std::sync::MutexGuard<'_, InFlightState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn guard(&self, key: &str, token: u64) -> InFlightGuard {
        InFlightGuard {
            state: self.0.clone(),
            key: key.to_owned(),
            token,
        }
    }

    /// `key`（= user_id）のターンを開始できるか試みる。空いていれば枠を確保して [`InFlightGuard`] を返す
    /// （drop で自動的に解放される・成功・失敗・タイムアウト・panic のいずれの終了経路でも解放を保証する）。
    /// 枠が埋まっていても、保持するターンが既に終端行を永続化済みなら引き継ぐ（型のドキュメント参照）。
    /// 保持者が実行中・受理前・確認の読み取り失敗のときは `None`（呼び出し側は 409 を返す）。
    async fn acquire(&self, db: &Db, key: &str) -> Option<InFlightGuard> {
        let (holder_token, since_id) = {
            let mut state = self.lock();
            match state.slots.get(key) {
                None => {
                    let token = state.insert(key);
                    drop(state);
                    return Some(self.guard(key, token));
                }
                Some(slot) => (slot.token, slot.since_id?),
            }
        };
        // ロックを跨いで await しない。確認後に再ロックし、枠が「確認した保持者のまま」か「その間に保持者の
        // ガードが drop されて空になった」場合だけ確保する。別のリクエストが先に引き継いでいれば
        // （トークンが変わっている）負けなので、同時に複数のリクエストが引き継ごうとしても勝てるのは 1 つだけ。
        match message_log::has_pwa_assistant_reply_after(db, key, BOT_ID, since_id).await {
            Ok(true) => {
                let mut state = self.lock();
                let still_ours = state
                    .slots
                    .get(key)
                    .is_none_or(|slot| slot.token == holder_token);
                if still_ours {
                    let token = state.insert(key);
                    drop(state);
                    Some(self.guard(key, token))
                } else {
                    None
                }
            }
            Ok(false) => None,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    user_id = key,
                    "client-api: 進行中ターンの終端確認に失敗（409 として扱う）"
                );
                None
            }
        }
    }
}

/// [`InFlightTurns::acquire`] の RAII ガード。drop で自分の枠を解放する
/// （引き継がれて既に別のターンの枠になっていれば何もしない）。
struct InFlightGuard {
    state: Arc<StdMutex<InFlightState>>,
    key: String,
    token: u64,
}

impl InFlightGuard {
    /// 受理したユーザー発言の `since_id` を枠へ記録する（以後、この枠は「`since_id` より後の終端行が
    /// コミットされた時点で完了」と判定できる）。
    fn mark_accepted(&self, since_id: i64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(slot) = state.slots.get_mut(&self.key) {
            if slot.token == self.token {
                slot.since_id = Some(since_id);
            }
        }
    }

    /// この枠がまだ自分のものか（完了済みとして新しいターンに引き継がれていないか）。
    fn is_current(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .slots
            .get(&self.key)
            .is_some_and(|slot| slot.token == self.token)
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.slots.get(&self.key).map(|s| s.token) == Some(self.token) {
            state.slots.remove(&self.key);
        }
    }
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
    Ok(Json(settings_view(&db, uid, current).await?))
}

/// 永続化済みの状態から [`SettingsView`] を組み立てる（GET/PUT 共通・issue #39）。
///
/// PUT の応答も書き込み後に DB を読み直してここで組み立てるため、応答が「保存されていない値」を
/// 報告することはない。`gemini_model` が NULL/空の行は既定モデル（`DEFAULT_MODEL`）として表示する。
/// 行が無い場合（GET のみ）は既定値・未連携として返す。
async fn settings_view(
    db: &Db,
    uid: &str,
    current: Option<users::UserAgentSettings>,
) -> Result<SettingsView, ClientApiError> {
    let model = current
        .as_ref()
        .and_then(|c| c.model.clone())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| yuuka_gemini::DEFAULT_MODEL.to_owned());
    let google_connected = current.map(|c| c.google_connected).unwrap_or(false);
    let persona = active_persona_prompt(db, uid).await?;
    Ok(SettingsView {
        google_connected,
        // issue #47: Node はここに生の calendarId を返していた。人間可読な代替（連携先の表示名）が
        // 無いため、内部 ID を漏らすくらいなら省略する（クライアントは未設定時の案内文にフォール
        // バックする・`client/pwa/src/pages/SettingsPage.vue`）。
        google_account: None,
        model,
        persona,
    })
}

/// `users` 行が無いユーザーの設定更新を拒否するエラー（issue #39）。
///
/// `users` 行は認証（`yuuka-auth`）が `password_hash`/`salt` 付きで作る唯一の正で、PWA 設定の
/// 書き込みが仮の認証情報で行を作ることはしない。行が無いまま黙ってスキップして「保存した」と
/// 応答するのは不正確なので、明示的に 404 を返す。
fn user_settings_not_found() -> ClientApiError {
    ClientApiError::not_found("user settings not found")
}

/// `PUT /api/client/settings`。
///
/// `model`（許可リスト検証・issue #39）と `persona` のみを受け付ける。旧クライアントが送る
/// `maxTokens`/`temperature` は [`SettingsUpdate`] に対応フィールドが無く、無視される。
/// 検証（400）→ 行の存在確認（404）→ 書き込み → DB 再読込の応答、の順で、いずれかで失敗した場合に
/// 部分更新（モデルだけ保存・persona だけ失敗等）や未保存値の報告が起きない。
async fn settings_put(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<SettingsUpdate>,
) -> Result<Json<SettingsView>, ClientApiError> {
    let uid = user.0.discord_id.clone();

    let requested_model = match body
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
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
            Some(m.to_owned())
        }
        None => None,
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

    // `users` 行が無ければ何も書かずに 404（モデル更新を黙ってスキップして変更後の値を返す
    // Node 版の挙動は、保存されていない値を報告するため廃止・issue #39）。
    if users::get_agent_settings(&db, &uid).await?.is_none() {
        return Err(user_settings_not_found());
    }

    if let Some(model) = &requested_model {
        // 存在確認と UPDATE の間で行が消えた場合（`false`）も未保存扱いにしない。
        if !users::set_gemini_model(&db, &uid, model).await? {
            return Err(user_settings_not_found());
        }
    }
    if let Some(persona_text) = &body.persona {
        save_active_persona_prompt(&db, &uid, persona_text).await?;
    }

    // 応答は入力のエコーではなく、書き込み後の永続状態から組み立てる。
    let after = users::get_agent_settings(&db, &uid)
        .await?
        .ok_or_else(user_settings_not_found)?;
    Ok(Json(settings_view(&db, &uid, Some(after)).await?))
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

// ─── chat（履歴・送信・添付） ───────────────────────────────────────────────────

/// `GET /api/client/chat/messages`（チャット履歴・issue #41）。
///
/// `POST /api/client/chat/messages`（[`chat_send`]）が `202 Accepted` を返した後、クライアントは
/// このエンドポイントをポーリングして完了を検知する（issue #41 PR #75 レビュー・P1）。アシスタント
/// 応答には埋め込み（`embeds`）とファイル添付（`files`・実体は `GET .../chat/attachments/:id`）を
/// 含める（issue #41 PR #75 レビュー・P2）。
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
        let embeds = parse_stored_embeds(row.rich_content.as_deref());
        let files = row
            .attachments
            .iter()
            .map(|a| ChatFileView {
                id: a.id.to_string(),
                name: a.name.clone(),
                mime_type: a.mime_type.clone(),
                url: format!("/api/client/chat/attachments/{}", a.id),
            })
            .collect();
        out.push(ChatMessageView {
            id: row.id.to_string(),
            role,
            content: row.content,
            created_at,
            references,
            embeds,
            files,
        });
    }
    Ok(Json(out))
}

/// `message_logs.rich_content`（[`RichEmbed`] 配列の JSON）を wire 表現へ写像する。パース失敗
/// （想定外の手動データ変更等）は空配列へデグレードする（履歴取得全体を 500 にしない・issue #41
/// PR #75 レビュー・P2）。
fn parse_stored_embeds(raw: Option<&str>) -> Vec<ChatEmbedView> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<RichEmbed>>(raw) {
        Ok(embeds) => embeds
            .into_iter()
            .map(|e| ChatEmbedView {
                title: e.title,
                description: e.description,
                color: e.color,
                fields: e
                    .fields
                    .into_iter()
                    .map(|f| ChatEmbedFieldView {
                        name: f.name,
                        value: f.value,
                        inline: f.inline,
                    })
                    .collect(),
                footer: e.footer,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "client-api: rich_content の JSON パースに失敗（空扱い）");
            Vec::new()
        }
    }
}

/// `POST /api/client/chat/messages`（チャット送信・issue #41）。
///
/// Node の `POST /api/client/chat/messages` は `processMessage` を直接呼んでおり、WS/Discord
/// 秘書経路が持つ事前チェック（レート制限・Gemini キー事前判定）を欠いていた（#41）。ここでは
/// Gemini を呼ぶ**前**に、WS（`crates/yuuka-supervisor/src/ws.rs`）と同様の事前チェックを**同期**で
/// 行い、該当すれば即座にエラーを返す:
///
/// 1. content が空 → 400。
/// 2. [`ChatEngine::user_has_gemini_key`] — 未設定なら 400（WS の `error/no_gemini_key` 相当）。
/// 3. [`InFlightTurns::acquire`] — 同一ユーザーの前のターンがまだ進行中なら 409（二重送信/連打の
///    抑止・[`InFlightTurns`] の doc 参照）。レート制限を消費する**前**に弾くため、拒否されたリクエスト
///    はクォータを消費しない。
/// 4. `rate_limiter.consume`（[`yuuka_discord::RateLimiter`]・Discord 汎用モード/finance receipt と
///    同じ [`yuuka_orchestrator::InMemoryRateLimiter`] シーム）— 超過なら 429。ユーザー単位の窓は
///    `bot_id × user_id` のみで消費されるため、Discord/WS で使い切った分は PWA でも消費済みのまま。
///
/// # 非同期配信（issue #41 PR #75 レビュー・P1）
/// 上記の同期チェックを通過すると、ユーザー発言を**永続化してから**（issue #77）、ターン本体（[`ChatEngine::secretary_turn_pwa`]・`source='pwa'`）を
/// `tokio::spawn` でバックグラウンド実行し（[`run_pwa_turn_in_background`]）、**待たずに** `202
/// Accepted` + [`ChatSendAccepted`]（`sinceId`）を返す。同期 `fetch` で最大 180 秒待つ旧設計は、
/// リバースプロキシ（issue #41 が言及する Cloudflare Tunnel 等）のタイムアウトが 180 秒より短いと
/// 整形された JSON 504 すら届かず素の接続切断になる問題があった。202 は事前チェックの時点で即座に
/// 返るため、この問題が起きない。
///
/// ## ポーリング契約
/// クライアントは `sinceId` を保持しつつ [`chat_history`]（`GET /api/client/chat/messages`）を
/// バックオフ付きで再取得し、`id > sinceId` かつ `role:"agent"` の行が現れたら完了とみなしてポーリング
/// を止める（新しい GET エンドポイントを増やさず、既存の履歴取得だけで完結する設計）。バックグラウンド
/// ターンが失敗/タイムアウトしても [`run_pwa_turn_in_background`] が必ず終端状態（フォールバックの
/// エラー応答）を保存するため、クライアントは有限時間でポーリングを終えられる。
///
/// ## 再起動（fail-closed・issue #77）
/// `202` の後にサーバーが再起動/異常終了すると、バックグラウンドターンと `InFlightTurns`（メモリ内）は
/// 失われ、上記の終端保存も走らない。ターンは**再開しない**（ツール呼び出しの副作用が二重実行されうる
/// ため）。代わりに (1) 発言は `202` の前に永続化済みで失われず、(2) 次回起動時の回復スイープ
/// （[`crate::recover_orphaned_chat_turns`]）が「最新行がユーザー発言のまま」の会話へ通知行を書き、
/// クライアントのポーリングを終端させる。クライアントの契約（`202` + `sinceId` + ポーリング）は不変。
async fn chat_send(
    user: AuthenticatedUser,
    Extension(engine): Extension<Arc<ChatEngine>>,
    Extension(rate_limiter): Extension<Arc<dyn RateLimiter>>,
    Extension(in_flight): Extension<InFlightTurns>,
    State(db): State<Db>,
    Json(body): Json<ChatSendInput>,
) -> Result<(StatusCode, Json<ChatSendAccepted>), ClientApiError> {
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
    // 事前チェック 1/3: Gemini キー未設定（WS `error/no_gemini_key` と同じ判定・Node には無かった）。
    if !engine.user_has_gemini_key(uid).await {
        return Err(ClientApiError {
            status: StatusCode::BAD_REQUEST,
            message: "Gemini APIキーが未設定です。管理画面から設定してください。".to_owned(),
        });
    }

    // 事前チェック 2/3: 同時ターン防止（[`InFlightTurns`] の doc 参照）。
    let Some(guard) = in_flight.acquire(&db, uid).await else {
        return Err(ClientApiError::conflict(
            "前のメッセージへの応答がまだ処理中です。応答が届いてから送信してください。",
        ));
    };

    let bot_id = BotId::new(BOT_ID);
    let user_id = UserId::new(uid.to_owned());

    // 事前チェック 3/3: レート制限。
    let decision = rate_limiter
        .consume(&bot_id, &GuildId::new(PWA_RATE_LIMIT_GUILD), &user_id)
        .await;
    if let Some(exceeded) = decision.exceeded {
        // `guard` はここで drop され即座に解放される（拒否されたリクエストが in-flight を占有し
        // 続けない）。
        return Err(ClientApiError {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: rate_limit_message(exceeded),
        });
    }

    // 受理の永続化（issue #77）: `202` を返す**前**にユーザー発言を保存する。以後にプロセスが落ちても
    // 受理済みの送信は履歴に残り、起動時の回復スイープ（[`crate::recovery`]）が終端の通知行を書ける
    // （エンジンのターンはユーザー発言を保存しない・[`ChatEngine::secretary_turn_pwa`] の前提）。
    // `since_id` は保存の直前の直近 `message_logs.id`（同一トランザクションで読む）なので、以後
    // id > since_id の行はすべてこのターンに属する。
    let accepted = message_log::accept_pwa_user_message(&db, uid, BOT_ID, &content).await?;
    let since_id = accepted.since_id;
    // 枠へ `sinceId` を記録する。以後、`since_id` より後の終端行がコミットされた時点でこの枠は
    // 「完了済み」と判定され、ガードの drop を待たずに次の送信へ引き継げる（[`InFlightTurns`] の doc 参照）。
    guard.mark_accepted(since_id);

    let incoming = IncomingChat {
        text: content,
        ..IncomingChat::default()
    };
    tokio::spawn(run_pwa_turn_in_background(PwaTurnJob {
        guard,
        engine,
        db,
        bot_id,
        user_id,
        incoming,
        since_id,
        turn_timeout: CHAT_TURN_TIMEOUT,
    }));

    Ok((
        StatusCode::ACCEPTED,
        Json(ChatSendAccepted {
            status: "pending",
            since_id: since_id.to_string(),
        }),
    ))
}

/// [`run_pwa_turn_in_background`] の引数束（clippy `too_many_arguments` 回避・フィールドは全て
/// 呼び出しに必要な情報でグルーピングの意味も持つ）。
struct PwaTurnJob {
    /// [`InFlightGuard`] をこのジョブの生存期間ずっと保持する（drop で in-flight から自動解放）。
    guard: InFlightGuard,
    engine: Arc<ChatEngine>,
    db: Db,
    bot_id: BotId,
    user_id: UserId,
    incoming: IncomingChat,
    since_id: i64,
    turn_timeout: Duration,
}

/// [`chat_send`] からバックグラウンド実行される 1 ターン本体（issue #41 PR #75 レビュー・P1）。
///
/// `job.guard`（[`InFlightGuard`]）をこの関数の実行中ずっと保持することで、ターンが確定する
/// （成功・失敗・タイムアウトいずれか）までは同一ユーザーの新規送信が 409 で弾かれる。この関数を
/// 抜けると（return 経路によらず）`job` ごと drop され、同時に in-flight から解放される。終端行の
/// 永続化からこの drop までの間は、[`InFlightTurns::acquire`] が永続化を根拠に枠を引き継ぐため、
/// 応答を見たポーラーの次の送信が 409 になることは無い（[`InFlightTurns`] の doc 参照）。
///
/// **終端状態の保証**: どの結果でも、返る前に「`since_id` より後のアシスタント行」が履歴に存在する状態にする
/// （[`chat_history`] をポーリングするクライアントが必ず有限時間で完了/エラーを受け取れるように）。
///
/// - 通常の成功: `engine.secretary_turn_pwa` が内部でアシスタント応答を（embeds/files 込みで）永続化済み
///   （ユーザー発言は [`chat_send`] が `202` の前に保存済み）のため、追加の保存はしない。
/// - ⚠️ 定型応答（レート制限・サーバー混雑・鍵未設定）やペルソナ入りエラー報告: エンジンは `Ok(reply)` を
///   返すが**履歴には保存しない**（LLM 文脈を汚染しないための既存の不変条件）。そのままではクライアントが
///   永遠に待つため、`reply.text` を通知行（[`message_log::add_pwa_notice`]・LLM コンテキストからは除外）
///   として保存する。
/// - 失敗（`Err`）・タイムアウト: フォールバック文言を同じく通知行として保存する。
///
/// 保存前に [`message_log::has_pwa_assistant_reply_after`] で既存の応答が無いことを確認する（タイムアウトで
/// `timeout()` がキャンセルした future が、キャンセル直前にぎりぎり保存を終えていた場合の二重保存を避ける）。
async fn run_pwa_turn_in_background(job: PwaTurnJob) {
    let PwaTurnJob {
        guard,
        engine,
        db,
        bot_id,
        user_id,
        incoming,
        since_id,
        turn_timeout,
    } = job;

    // PWA は status（thinking/writing）フレームを消費する経路を持たない（ポーリングのみ）ため no-op。
    let status: StatusSink = Arc::new(|_| {});
    let outcome = timeout(
        turn_timeout,
        engine.secretary_turn_pwa(&bot_id, &user_id, incoming, &status),
    )
    .await;

    let (uid, bid) = (user_id.as_str(), bot_id.as_str());
    // 履歴に応答が無かった場合に通知行として保存する文言。
    let notice_text: String = match outcome {
        Ok(Ok(reply)) if !reply.text.trim().is_empty() => reply.text,
        Ok(Ok(_)) => FALLBACK_ERROR_TEXT.to_owned(),
        Ok(Err(e)) => {
            tracing::warn!(
                error = %e,
                user_id = uid,
                "client-api: チャット送信ターンが失敗（バックグラウンド）"
            );
            FALLBACK_ERROR_TEXT.to_owned()
        }
        Err(_) => {
            tracing::warn!(
                timeout = ?turn_timeout,
                user_id = uid,
                "client-api: チャット送信がタイムアウト（バックグラウンド）"
            );
            FALLBACK_TIMEOUT_TEXT.to_owned()
        }
    };

    match message_log::has_pwa_assistant_reply_after(&db, uid, bid, since_id).await {
        // 通常の成功、またはキャンセル直前に保存が完了していた（レアなタイムアウト境界）。
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => tracing::warn!(
            error = %e,
            user_id = uid,
            "client-api: 応答存在確認に失敗（通知行の保存を試みる）"
        ),
    }
    // 枠が既に新しいターンへ引き継がれていたなら、それはこのターンの終端行が保存済みだった場合に限られる
    // （[`InFlightTurns`] の doc 参照）。存在確認だけが読み取りエラーで失敗した場合でも、次のターンの
    // 発言より後に重複の通知行を書かない（次ターンのポーラーが取り違える）。
    if !guard.is_current() {
        return;
    }
    if let Err(e) = message_log::add_pwa_notice(&db, uid, bid, &notice_text).await {
        tracing::error!(
            error = %e,
            user_id = uid,
            "client-api: 通知行の保存に失敗（クライアントは最大待機時間までポーリングを続ける）"
        );
    }
}

/// `GET /api/client/chat/attachments/:id`（issue #41 PR #75 レビュー・P2）。
///
/// ファイル添付（グラフ PNG 等・`secretary_turn_pwa` の `TurnReply::files`）の実バイトを配信する。
/// [`message_log::get_pwa_attachment_for_user`] が `message_logs.user_id` を発話者本人（かつ
/// `source='pwa'`）に限定するため、他ユーザーの添付は 404 になる（認証済みかつ所有者スコープ）。
/// 画像は `inline`（チャット内に直接表示）、それ以外は `attachment`（ダウンロード）として返す。
async fn chat_attachment(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Path(id): Path<String>,
) -> Result<Response, ClientApiError> {
    let Ok(id) = id.parse::<i64>() else {
        return Err(ClientApiError::not_found("Not found"));
    };
    let uid = user.0.discord_id.as_str();
    let Some(att) = message_log::get_pwa_attachment_for_user(&db, id, uid).await? else {
        return Err(ClientApiError::not_found("Not found"));
    };
    let disposition_kind = if att.mime_type.starts_with("image/") {
        "inline"
    } else {
        "attachment"
    };
    let disposition = format!(
        "{disposition_kind}; filename=\"{}\"",
        sanitize_filename(&att.name)
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, att.mime_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from(att.bytes))
        .map_err(|_| ClientApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal".to_owned(),
        })
}

/// `Content-Disposition` ヘッダへ埋め込む前に危険な文字（引用符・改行）を取り除く（添付ファイル名は
/// 現状 `attachment.{png,jpg,bin}` 固定だが、将来の拡張に備えた防御的サニタイズ）。
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '"' | '\r' | '\n'))
        .collect()
}

#[cfg(test)]
mod tests {
    //! `run_pwa_turn_in_background`（非公開）を router を介さず直接検証するユニットテスト
    //! （issue #41 PR #75 レビュー・P1）。HTTP レベルの結合テストは `tests/chat_send.rs`。
    //!
    //! ここでは特に「タイムアウト時にフォールバック応答が終端状態として保存される」ことを、
    //! 実 180 秒を待たずに短いタイムアウトで検証する（`CHAT_TURN_TIMEOUT` は本番用の固定値のため
    //! HTTP 経由ではこの経路を現実的な時間で再現できない）。
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::atomic::{AtomicU64, Ordering};

    use async_trait::async_trait;
    use secrecy::SecretString;
    use tokio::sync::Notify;
    use yuuka_core::GeminiError;
    use yuuka_gemini::{Content, FunctionDeclaration, GenerateContentResponse, ToolConfig};
    use yuuka_orchestrator::GeminiFactory;
    use yuuka_tools::ToolRegistry;

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn fresh_db() -> (Db, std::path::PathBuf) {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_client_api_routes_ut_{}_{n}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            rusqlite::Connection::open(&path).expect("seed file");
        }
        (Db::open(&path).expect("open db"), path)
    }

    fn seed_user_with_key(
        path: &std::path::Path,
        crypto: &yuuka_crypto::SystemCrypto,
        discord_id: &str,
    ) {
        let enc = crypto.encrypt_text("fake-gemini-key").expect("encrypt");
        let conn = rusqlite::Connection::open(path).expect("open seed");
        conn.execute(
            "INSERT INTO users (discord_id, username, password_hash, salt, role, \
             gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag, gemini_model) \
             VALUES (?1, 'yuu', 'x', '00', 'user', ?2, ?3, ?4, 'gemini-3.1-flash-lite')",
            rusqlite::params![discord_id, enc.encrypted, enc.iv, enc.auth_tag],
        )
        .expect("seed user");
    }

    /// `generate()` が呼ばれたことだけ通知し、そのまま `Pending` のまま返らない fake backend
    /// （`tokio::time::timeout` によるキャンセルを決定的に発生させるため・実ネットワークは使わない）。
    struct NeverRespondingBackend {
        started: Arc<Notify>,
    }

    #[async_trait]
    impl yuuka_gemini::GenerateBackend for NeverRespondingBackend {
        async fn generate(
            &self,
            _system_instruction: Option<&str>,
            _declarations: &[FunctionDeclaration],
            _contents: &[Content],
            _tool_config: Option<ToolConfig>,
        ) -> Result<GenerateContentResponse, GeminiError> {
            self.started.notify_one();
            // 決して解決しない（外側の timeout が先に発火してキャンセルする）。
            std::future::pending().await
        }
    }

    struct NeverRespondingFactory {
        started: Arc<Notify>,
    }

    impl GeminiFactory for NeverRespondingFactory {
        fn build(
            &self,
            _model: &str,
            _api_key: SecretString,
        ) -> Result<Arc<dyn yuuka_gemini::GenerateBackend>, GeminiError> {
            Ok(Arc::new(NeverRespondingBackend {
                started: self.started.clone(),
            }))
        }
    }

    #[tokio::test]
    async fn run_pwa_turn_in_background_persists_fallback_on_timeout() {
        let (db, path) = fresh_db();
        let crypto = Arc::new(
            yuuka_crypto::SystemCrypto::new(SecretString::from(
                "routes-test-secret-0123456789ab".to_owned(),
            ))
            .unwrap(),
        );
        seed_user_with_key(&path, &crypto, "u1");

        let started = Arc::new(Notify::new());
        let engine = Arc::new(ChatEngine::new(
            db.clone(),
            Some(crypto),
            ToolRegistry::new(),
            Arc::new(NeverRespondingFactory {
                started: started.clone(),
            }),
            None,
            Arc::new(yuuka_mcp::NullMcpClient),
            None,
        ));

        let in_flight = InFlightTurns::default();
        let guard = in_flight
            .acquire(&db, "u1")
            .await
            .expect("first acquire succeeds");

        let bot_id = BotId::new(BOT_ID);
        let user_id = UserId::new("u1".to_owned());
        let incoming = IncomingChat {
            text: "hello".to_owned(),
            ..IncomingChat::default()
        };
        // `chat_send` と同じく、ターンの前にユーザー発言を受理（永続化）しておく（issue #77）。
        let since_id = message_log::accept_pwa_user_message(&db, "u1", BOT_ID, "hello")
            .await
            .unwrap()
            .since_id;
        guard.mark_accepted(since_id);

        // 実 CHAT_TURN_TIMEOUT（180 秒）は現実的な時間で HTTP 経由で再現できないため、この
        // ユニットテストだけ短いタイムアウトを直接注入する。
        run_pwa_turn_in_background(PwaTurnJob {
            guard,
            engine,
            db: db.clone(),
            bot_id,
            user_id,
            incoming,
            since_id,
            turn_timeout: Duration::from_millis(50),
        })
        .await;

        // 終端状態（フォールバック応答）が保存され、ポーリングするクライアントは応答を受け取れる。
        assert!(
            message_log::has_pwa_assistant_reply_after(&db, "u1", BOT_ID, since_id)
                .await
                .unwrap(),
            "タイムアウト後にフォールバックのアシスタント応答が保存されている"
        );
        let history = message_log::list_pwa_messages(&db, "u1", BOT_ID, 50)
            .await
            .unwrap();
        let last = history.last().expect("history is not empty");
        assert_eq!(last.role, "assistant");
        assert_eq!(last.content, FALLBACK_TIMEOUT_TEXT);
        // 受理済みのユーザー発言はターンに再保存されず 1 件のまま（issue #77）。
        assert_eq!(
            history.iter().filter(|m| m.role == "user").count(),
            1,
            "history={history:?}"
        );

        // 関数を抜けた時点で guard は解放されている（新規ターンを開始できる）。
        assert!(
            in_flight.acquire(&db, "u1").await.is_some(),
            "run_pwa_turn_in_background を抜けると in-flight から解放される"
        );
    }

    // ─── 終端行の永続化から導出する in-flight 枠（PR #83 フォロー: 応答直後の 409 誤発火） ─────────

    /// `chat_send` と同じ手順（枠の確保 → 発言の受理 → `sinceId` の記録）で 1 ターンを「進行中」にする。
    async fn start_turn(in_flight: &InFlightTurns, db: &Db) -> (InFlightGuard, i64) {
        let guard = in_flight
            .acquire(db, "u1")
            .await
            .expect("slot is free for a new turn");
        let since_id = message_log::accept_pwa_user_message(db, "u1", BOT_ID, "hi")
            .await
            .unwrap()
            .since_id;
        guard.mark_accepted(since_id);
        (guard, since_id)
    }

    async fn persist_reply(db: &Db) {
        message_log::add_pwa_assistant_reply(db, "u1", BOT_ID, "reply", None, &[])
            .await
            .unwrap();
    }

    async fn persist_notice(db: &Db) {
        message_log::add_pwa_notice(db, "u1", BOT_ID, "notice")
            .await
            .unwrap();
    }

    /// 実行中（終端行が無い）のターンの枠は、受理前でも受理後でも、そして以前のターンの応答が履歴に
    /// あっても引き継げない（逆向きの窓を作らない・fail-closed）。
    #[tokio::test]
    async fn slot_is_not_reclaimed_while_the_turn_has_no_terminal_row() {
        let (db, _path) = fresh_db();
        let in_flight = InFlightTurns::default();

        // 前のターンは完了済み（応答が履歴にある）。
        let (prev, _) = start_turn(&in_flight, &db).await;
        persist_reply(&db).await;
        drop(prev);

        // 受理前（`sinceId` 未確定）: 以前の応答があっても、枠は進行中として扱う。
        let guard = in_flight.acquire(&db, "u1").await.expect("free");
        assert!(in_flight.acquire(&db, "u1").await.is_none(), "受理前は 409");

        // 受理後・終端行なし: `since_id` より後の応答が無いので進行中のまま（以前の応答は数えない）。
        let since_id = message_log::accept_pwa_user_message(&db, "u1", BOT_ID, "next")
            .await
            .unwrap()
            .since_id;
        guard.mark_accepted(since_id);
        assert!(
            in_flight.acquire(&db, "u1").await.is_none(),
            "終端行が無いターンは引き継げない"
        );
        assert!(guard.is_current());

        // 別ユーザーの枠とは独立。
        assert!(in_flight.acquire(&db, "u2").await.is_some());
    }

    /// 回帰: 応答（終端行）がコミット済みなら、ガードが drop される前でも次の送信は枠を得られる
    /// （旧実装は応答の保存 → ガードの drop の間に 409 を返していた）。
    #[tokio::test]
    async fn slot_is_reclaimed_once_the_reply_is_committed_even_if_the_guard_is_still_held() {
        let (db, _path) = fresh_db();
        let in_flight = InFlightTurns::default();
        let (old_guard, _) = start_turn(&in_flight, &db).await;

        // エンジンが応答を保存済み。バックグラウンドタスクはまだ戻っておらず、ガードは保持されたまま。
        persist_reply(&db).await;

        let new_guard = in_flight
            .acquire(&db, "u1")
            .await
            .expect("応答が見える状態では次の送信は 409 にならない");
        assert!(!old_guard.is_current(), "旧ターンの枠は引き継がれた");
        assert!(new_guard.is_current());

        // 旧ターンのガードが後から drop されても、引き継がれた新しい枠は解放されない。
        drop(old_guard);
        assert!(
            in_flight.acquire(&db, "u1").await.is_none(),
            "新しいターンはまだ進行中（受理前）なので 409"
        );

        // 新しいターンの枠は自分のガードでだけ解放される。
        drop(new_guard);
        assert!(in_flight.acquire(&db, "u1").await.is_some());
    }

    /// 通知行（失敗/タイムアウト/⚠️ 定型応答のフォールバック）も終端行として同様に扱う。
    #[tokio::test]
    async fn slot_is_reclaimed_once_a_notice_is_committed_even_if_the_guard_is_still_held() {
        let (db, _path) = fresh_db();
        let in_flight = InFlightTurns::default();
        let (old_guard, _) = start_turn(&in_flight, &db).await;
        persist_notice(&db).await;

        assert!(in_flight.acquire(&db, "u1").await.is_some());
        assert!(!old_guard.is_current());
    }

    /// 完了済みの枠へ同時に複数の送信が来ても、引き継げるのは 1 つだけ（他は 409）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn only_one_of_many_concurrent_sends_reclaims_a_finished_slot() {
        let (db, _path) = fresh_db();
        let in_flight = InFlightTurns::default();
        let (old_guard, _) = start_turn(&in_flight, &db).await;
        persist_reply(&db).await;

        let tasks: Vec<_> = (0..16)
            .map(|_| {
                let (in_flight, db) = (in_flight.clone(), db.clone());
                tokio::spawn(async move { in_flight.acquire(&db, "u1").await })
            })
            .collect();
        let mut winners = 0;
        let mut held = Vec::new();
        for task in tasks {
            if let Some(guard) = task.await.unwrap() {
                winners += 1;
                held.push(guard);
            }
        }
        assert_eq!(winners, 1, "引き継げるのは 1 リクエストだけ");
        drop(old_guard);
        drop(held);
    }
}
