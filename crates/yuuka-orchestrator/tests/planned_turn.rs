//! R06〜R08 のターン統合テスト（fake Gemini backend・実 SQLite・ネットワーク不要）。
//!
//! 検証: 事前計画を参考情報と完了是正の許可リストに使い、ツールの宣言は絞らない（R06）／重い計画は途中応答を先に返し、最終結果を
//! 後続配信する（R07）／計画と途中応答を `MetricsSink` の `plan_attempts`・`async_predicted` で数える
//! （#181）／返信元チェーンを systemInstruction へ入れ、応答を入力へ紐づけて保存する（R08）。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use secrecy::SecretString;
use serde_json::json;
use tokio::sync::{Notify, Semaphore};
use yuuka_core::{
    BotId, GeminiError, MetricsSink, Tool, ToolContext, ToolError, ToolExposure, ToolName,
    ToolOutcome, UserId,
};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::{
    BotStatus, IncomingChat, ReplyAncestor, StatusSink, TurnDelivery, TurnProcessor, TurnReply,
};
use yuuka_gemini::{
    Content, FunctionDeclaration, GenerateBackend, GenerateContentResponse, ToolConfig,
};
use yuuka_orchestrator::turn_plan::PLANNER_INSTRUCTION;
use yuuka_orchestrator::{message_log, ChatEngine, GeminiFactory};
use yuuka_tools::{NativeProvider, ToolRegistry};
use yuuka_web::Db;

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const USER: &str = "u-plan";
const BOT: &str = "bot_mine";

/// migrations 適用済みの一時 DB と、暗号化した Gemini キーを持つ利用者。
fn seeded_db() -> (Db, std::path::PathBuf, Arc<SystemCrypto>) {
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "yuuka_planned_turn_{}_{n}.sqlite",
        std::process::id()
    ));
    drop(rusqlite::Connection::open(&path).expect("seed file"));
    let db = Db::open(&path).expect("open db");
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("planned-turn-secret".to_owned())).unwrap());
    let enc = crypto.encrypt_text("fake-gemini-key").expect("encrypt");
    rusqlite::Connection::open(&path)
        .expect("open seed")
        .execute(
            "INSERT INTO users (discord_id, username, salt, role, \
             gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag, gemini_model) \
             VALUES (?1, 'yuu', '00', 'user', ?2, ?3, ?4, 'gemini-3.1-flash-lite')",
            rusqlite::params![USER, enc.encrypted, enc.iv, enc.auth_tag],
        )
        .expect("seed user");
    (db, path, crypto)
}

fn assistant_logs(path: &std::path::Path) -> Vec<(String, Option<String>)> {
    let conn = rusqlite::Connection::open(path).expect("open");
    let mut stmt = conn
        .prepare(
            "SELECT content, reply_to_msg_id FROM message_logs \
             WHERE user_id = ?1 AND role = 'assistant' ORDER BY id",
        )
        .expect("prepare");
    stmt.query_map(rusqlite::params![USER], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
}

fn response(value: serde_json::Value) -> GenerateContentResponse {
    serde_json::from_value(json!({"candidates": [{"content": {"role": "model", "parts": value}}]}))
        .expect("valid response")
}

/// 本編の 1 回の生成で backend が受け取ったもの。
struct MainCall {
    sys: String,
    /// 宣言されたツール名（昇順）。
    names: Vec<String>,
    /// 完了是正（mode=ANY）の `allowedFunctionNames`。是正でない呼び出しは `None`。
    allowed: Option<Vec<String>>,
}

/// 計画の呼び出しには `plan` を返し、本編の呼び出しは [`MainCall`] を記録して台本を返す。
struct PlanningBackend {
    plan: String,
    script: Mutex<VecDeque<GenerateContentResponse>>,
    /// 本編の生成を止める門（途中応答を本編の完了より先に確実に観測するため）。`None` なら止めない。
    gate: Option<Arc<Semaphore>>,
    planner_calls: Mutex<usize>,
    main_calls: Mutex<Vec<MainCall>>,
}

impl PlanningBackend {
    fn new(plan: &str, script: Vec<serde_json::Value>) -> Self {
        Self {
            plan: plan.to_owned(),
            script: Mutex::new(script.into_iter().map(response).collect()),
            gate: None,
            planner_calls: Mutex::new(0),
            main_calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl GenerateBackend for PlanningBackend {
    async fn generate(
        &self,
        system_instruction: Option<&str>,
        declarations: &[FunctionDeclaration],
        _contents: &[Content],
        tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        let sys = system_instruction.unwrap_or_default();
        if sys.starts_with(PLANNER_INSTRUCTION) {
            assert!(declarations.is_empty(), "計画にはツールを渡さない");
            *self.planner_calls.lock().unwrap() += 1;
            return Ok(response(json!([{ "text": self.plan }])));
        }
        if let Some(gate) = &self.gate {
            gate.acquire().await.expect("gate open").forget();
        }
        let mut names: Vec<String> = declarations
            .iter()
            .map(|d| d.name.as_str().to_owned())
            .collect();
        names.sort();
        self.main_calls.lock().unwrap().push(MainCall {
            sys: sys.to_owned(),
            names,
            allowed: tool_config.and_then(|tc| tc.function_calling_config.allowed_function_names),
        });
        Ok(self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("script ran out of responses"))
    }
}

struct SharedFactory(Arc<PlanningBackend>);

impl GeminiFactory for SharedFactory {
    fn build(&self, _: &str, _: SecretString) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        Ok(self.0.clone())
    }
}

/// 実行された名前を記録する秘書ツール。
struct RecordingTool {
    name: &'static str,
    executed: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Tool for RecordingTool {
    fn declaration(&self) -> yuuka_core::tool::FunctionDeclaration {
        yuuka_core::tool::FunctionDeclaration {
            name: ToolName::checked(self.name.to_owned()).expect("valid name"),
            description: format!("{} tool", self.name),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::secretary()
    }

    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        self.executed.lock().unwrap().push(self.name.to_owned());
        Ok(ToolOutcome::from_payload(json!({ "success": true })))
    }
}

/// 名前ごとのカウンタを溜める fake メトリクス（実体の `MetricsRegistry` は services 側）。
#[derive(Default)]
struct CountingMetrics {
    counters: Mutex<HashMap<String, u64>>,
}

impl CountingMetrics {
    fn get(&self, name: &str) -> u64 {
        self.counters
            .lock()
            .unwrap()
            .get(name)
            .copied()
            .unwrap_or(0)
    }
}

impl MetricsSink for CountingMetrics {
    fn incr(&self, name: &str, by: u64) {
        *self
            .counters
            .lock()
            .unwrap()
            .entry(name.to_owned())
            .or_insert(0) += by;
    }

    fn record_latency(&self, _kind: &str, _ms: u64) {}
}

/// 途中応答と最終配信を順に記録する配信ハンドル。
#[derive(Default)]
struct RecordingDelivery {
    events: Mutex<Vec<String>>,
    final_sent: Notify,
}

#[async_trait]
impl TurnDelivery for RecordingDelivery {
    async fn on_interim(&self, text: String) {
        self.events.lock().unwrap().push(format!("interim:{text}"));
    }

    async fn deliver_final(&self, reply: TurnReply) {
        self.events
            .lock()
            .unwrap()
            .push(format!("final:{}", reply.text));
        self.final_sent.notify_one();
    }
}

struct Harness {
    engine: ChatEngine,
    db: Db,
    path: std::path::PathBuf,
    backend: Arc<PlanningBackend>,
    metrics: Arc<CountingMetrics>,
    executed: Arc<Mutex<Vec<String>>>,
}

fn harness(backend: PlanningBackend) -> Harness {
    harness_with_tools(backend, &["addTodo", "deleteTodo"])
}

fn harness_with_tools(backend: PlanningBackend, tools: &[&'static str]) -> Harness {
    let (db, path, crypto) = seeded_db();
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut provider = NativeProvider::new();
    for &name in tools {
        provider
            .register(Arc::new(RecordingTool {
                name,
                executed: executed.clone(),
            }))
            .expect("register");
    }
    let backend = Arc::new(backend);
    let metrics = Arc::new(CountingMetrics::default());
    let engine = ChatEngine::new(
        db.clone(),
        Some(crypto),
        ToolRegistry::new().with_provider(Arc::new(provider)),
        Arc::new(SharedFactory(backend.clone())),
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        None,
    )
    .with_metrics(metrics.clone());
    Harness {
        engine,
        db,
        path,
        backend,
        metrics,
        executed,
    }
}

fn null_sink() -> StatusSink {
    Arc::new(|_: BotStatus| {})
}

async fn secretary(h: &Harness, text: &str) -> TurnReply {
    h.engine
        .secretary_turn(
            &BotId::new(BOT),
            &UserId::new(USER),
            IncomingChat {
                text: text.to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .unwrap()
}

/// R06: 計画は参考情報として注入し、宣言は能力で絞ったカタログ全体のまま。完了だけを主張したときの
/// 是正（mode=ANY）は計画のツールに限る（Node `allowedToolNames`）。配信経路のない同期ターン（PWA と
/// 同じ）は重い計画でも途中応答へ回さず、`async_predicted` を数えない。
#[tokio::test]
async fn plan_guides_the_turn_without_hiding_tools() {
    let h = harness(PlanningBackend::new(
        r#"{"steps":["ToDo を追加する"],"tools":["addTodo"],"weightMs":20000,"interim":"追加しています。"}"#,
        vec![
            json!([{ "text": "牛乳を追加しました。" }]),
            json!([{ "functionCall": { "name": "addTodo", "args": {}, "id": "c1" } }]),
            json!([{ "text": "ToDo に入れました。" }]),
        ],
    ));
    let reply = secretary(&h, "牛乳を ToDo に").await;

    assert_eq!(reply.text, "ToDo に入れました。");
    assert!(!reply.deferred, "同期ターンは後続配信へ回さない");
    let calls = h.backend.main_calls.lock().unwrap();
    assert!(
        calls.iter().all(|c| c.names == ["addTodo", "deleteTodo"]),
        "計画にないツールも宣言する"
    );
    assert!(calls[0].sys.contains("# 今回の処理プラン"));
    assert_eq!(calls[0].allowed, None);
    assert_eq!(
        calls[1].allowed.as_deref(),
        Some(&["addTodo".to_owned()][..]),
        "完了是正は計画のツールに限る"
    );
    assert_eq!(*h.executed.lock().unwrap(), ["addTodo"]);
    assert_eq!(h.metrics.get("plan_attempts"), 1);
    assert_eq!(h.metrics.get("async_predicted"), 0);
    assert_eq!(h.metrics.get("messages"), 1);
}

/// 計画が必要なツールを落としても（`tools: []`）、本編のモデルはそのツールを呼んで実行できる。
#[tokio::test]
async fn tools_missing_from_the_plan_still_run() {
    let h = harness(PlanningBackend::new(
        r#"{"steps":["雑談に答える"],"tools":[],"weightMs":300}"#,
        vec![
            json!([{ "functionCall": { "name": "deleteTodo", "args": {}, "id": "c1" } }]),
            json!([{ "text": "削除しました。" }]),
        ],
    ));
    let reply = secretary(&h, "牛乳の ToDo を消して").await;

    assert_eq!(reply.text, "削除しました。");
    assert_eq!(*h.executed.lock().unwrap(), ["deleteTodo"]);
    assert_eq!(h.metrics.get("plan_attempts"), 1);
}

/// 計画の失敗（JSON でない）は計画なしの従来どおりのターン。`plan_attempts` は数えない。
#[tokio::test]
async fn failed_plan_keeps_the_scoped_catalog() {
    let h = harness(PlanningBackend::new(
        "計画できません",
        vec![json!([{ "text": "こんにちは。" }])],
    ));
    secretary(&h, "こんにちは").await;

    let calls = h.backend.main_calls.lock().unwrap();
    assert_eq!(calls[0].names, ["addTodo", "deleteTodo"]);
    assert!(!calls[0].sys.contains("# 今回の処理プラン"));
    assert_eq!(*h.backend.planner_calls.lock().unwrap(), 1);
    assert_eq!(h.metrics.get("plan_attempts"), 0);
}

/// 選ぶ余地のないカタログ（ツール 1 つ以下）は計画しない（上流呼び出しを増やさない）。
#[tokio::test]
async fn single_tool_catalog_is_not_planned() {
    let h = harness_with_tools(
        PlanningBackend::new(
            r#"{"steps":["表示する"],"tools":["showRichContent"],"weightMs":300}"#,
            vec![json!([{ "text": "こんにちは。" }])],
        ),
        &["showRichContent"],
    );
    secretary(&h, "こんにちは").await;

    assert_eq!(*h.backend.planner_calls.lock().unwrap(), 0);
    assert_eq!(h.metrics.get("plan_attempts"), 0);
    let calls = h.backend.main_calls.lock().unwrap();
    assert_eq!(calls[0].names, ["showRichContent"]);
}

/// R07: Discord の配信経路で重い計画は、計画の途中応答を先に返して会話ログへ残し、本編の完了後に
/// 最終結果を 1 回だけ配信する。`plan_attempts`・`async_predicted` を 1 回ずつ数える。
#[tokio::test]
async fn heavy_plan_delivers_interim_then_final_and_counts_async_predicted() {
    let gate = Arc::new(Semaphore::new(0));
    let mut backend = PlanningBackend::new(
        r#"{"steps":["検索する"],"tools":["addTodo"],"weightMs":20000,"interim":"調べています。"}"#,
        vec![json!([{ "text": "調べた結果です。" }])],
    );
    backend.gate = Some(gate.clone());
    let h = harness(backend);
    let delivery = Arc::new(RecordingDelivery::default());
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let status: StatusSink = {
        let statuses = statuses.clone();
        Arc::new(move |s: BotStatus| statuses.lock().unwrap().push(s))
    };

    let reply = h
        .engine
        .process_secretary(
            &BotId::new(BOT),
            &UserId::new(USER),
            IncomingChat {
                text: "最新のニュースを調べて".to_owned(),
                discord_msg_id: Some("d10".to_owned()),
                channel_id: Some("c1".to_owned()),
                ..IncomingChat::default()
            },
            status,
            delivery.clone(),
        )
        .await
        .unwrap();

    assert!(
        reply.deferred && reply.is_silent(),
        "呼び出し側は返信しない"
    );
    assert_eq!(*delivery.events.lock().unwrap(), ["interim:調べています。"]);
    assert_eq!(
        assistant_logs(&h.path),
        [("調べています。".to_owned(), Some("d10".to_owned()))],
        "途中応答も入力へ紐づけて会話ログへ残す"
    );
    assert_eq!(h.metrics.get("plan_attempts"), 1);
    assert_eq!(h.metrics.get("async_predicted"), 1);

    gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), delivery.final_sent.notified())
        .await
        .expect("final delivered");
    assert_eq!(
        *delivery.events.lock().unwrap(),
        ["interim:調べています。", "final:調べた結果です。"]
    );
    let logs = assistant_logs(&h.path);
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[1].0, "調べた結果です。");
    assert_eq!(logs[1].1.as_deref(), Some("d10"));
    // 配信後に表示を待機へ戻す。
    tokio::time::timeout(Duration::from_secs(5), async {
        while statuses.lock().unwrap().last() != Some(&BotStatus::Idle) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("status reset to idle");
}

/// R08: Discord から取得した返信元と会話ログから復元した返信元を、メッセージ ID で重複を除いて時系列に
/// systemInstruction へ入れる。応答は入力へ紐づけて保存し、`log:<id>` を返す。
#[tokio::test]
async fn reply_chain_reaches_the_prompt_once_in_order_and_reply_is_linked() {
    let h = harness(PlanningBackend::new(
        "{}",
        vec![json!([{ "text": "旅行は来月です。" }])],
    ));
    // 会話ログ: 100「最初の話」← 200「先週話した旅行の件」（200 は 100 への返信）。
    for (id, text, parent) in [
        ("100", "最初の話", None),
        ("200", "先週話した旅行の件", Some("100")),
    ] {
        message_log::add_message_log(&h.db, USER, BOT, "user", text, Some(id), parent)
            .await
            .unwrap();
    }

    let reply = h
        .engine
        .secretary_turn(
            &BotId::new(BOT),
            &UserId::new(USER),
            IncomingChat {
                text: "あれどうなった？".to_owned(),
                discord_msg_id: Some("300".to_owned()),
                reply_to_msg_id: Some("200".to_owned()),
                // Discord からの取得は直近の返信先で切れた（時間切れ）。200 は会話ログにもある。
                reply_chain: vec![ReplyAncestor {
                    message_id: "200".to_owned(),
                    text: "[ゆう] 先週話した旅行の件".to_owned(),
                }],
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .unwrap();

    let sys = h.backend.main_calls.lock().unwrap()[0].sys.clone();
    assert!(sys.contains("# 返信元の会話"));
    assert!(
        sys.contains(r#"["[user] 最初の話","[ゆう] 先週話した旅行の件"]"#),
        "同じ発言は 1 回だけ・古い順: {sys}"
    );
    assert_eq!(sys.matches("先週話した旅行の件").count(), 1);
    let logs = assistant_logs(&h.path);
    assert_eq!(
        logs,
        [("旅行は来月です。".to_owned(), Some("300".to_owned()))]
    );
    let id = reply.message_id.expect("persisted reply id");
    assert!(id.starts_with("log:"), "{id}");
    let chain = message_log::reply_chain(&h.db, USER, BOT, None, None, false, Some(&id))
        .await
        .unwrap();
    let texts: Vec<&str> = chain.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "[user] 最初の話",
            "[user] 先週話した旅行の件",
            "[user] あれどうなった？",
            "[assistant] 旅行は来月です。"
        ]
    );
}
