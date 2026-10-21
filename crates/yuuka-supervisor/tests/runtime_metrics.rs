//! 実利用メトリクス（R23・#120）の配線統合テスト。
//!
//! 本番と同じく 1 つの [`MetricsRegistry`] を [`ChatEngine::with_metrics`] へ注入し、秘書ターンを実経路
//! （想起 → FC ループのツール往復 → 応答）で 1 回通すと、定期ログ（`MetricsLogService`）が出す
//! snapshot に会話数・計画数・ツール数・想起ヒット・応答時間/想起レイテンシが現れることを確かめる。
//! Gemini は scripted fake backend（ネットワーク不要）、DB は実 SQLite、シナプスは実エンジン。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use secrecy::SecretString;
use serde_json::json;
use yuuka_core::{
    BotId, GeminiError, Tool, ToolContext, ToolError, ToolExposure, ToolName, ToolOutcome, UserId,
};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::{BotStatus, IncomingChat, StatusSink};
use yuuka_gemini::{
    Content, FunctionDeclaration, GenerateBackend, GenerateContentResponse, ToolConfig,
};
use yuuka_orchestrator::{ChatEngine, GeminiFactory};
use yuuka_services::MetricsRegistry;
use yuuka_synapse::{FormationContext, Scope, SynapseEngine, DEFAULT_DIM};
use yuuka_tools::{NativeProvider, ToolRegistry};
use yuuka_web::Db;

/// 想起させる記憶（クエリと同文にして確実に 1st Hop へ載せる）。
const MEMORY: &str = "好きな食べ物はカレーです";

/// 事前計画（R06）には軽い計画を返し、本編は 1 回目にツール呼び出し、2 回目に最終テキストを返す
/// scripted backend。
struct ScriptedBackend {
    responses: Mutex<VecDeque<GenerateContentResponse>>,
}

#[async_trait]
impl GenerateBackend for ScriptedBackend {
    async fn generate(
        &self,
        system_instruction: Option<&str>,
        _declarations: &[FunctionDeclaration],
        _contents: &[Content],
        _tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        if system_instruction
            .is_some_and(|s| s.starts_with(yuuka_orchestrator::turn_plan::PLANNER_INSTRUCTION))
        {
            return Ok(serde_json::from_value(json!({"candidates":[{"content":{"role":"model","parts":[
                {"text":"{\"steps\":[\"ToDo に追加する\"],\"tools\":[\"addTodo\"],\"weightMs\":500}"}
            ]}}]}))
            .expect("valid plan response"));
        }
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted backend ran out of responses"))
    }
}

struct ScriptedFactory;

impl GeminiFactory for ScriptedFactory {
    fn build(
        &self,
        _model: &str,
        _api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        let script = [
            json!({"candidates":[{"content":{"role":"model","parts":[
                {"functionCall":{"name":"addTodo","args":{"title":"カレーの材料"},"id":"c1"}}
            ]}}]}),
            json!({"candidates":[{"content":{"role":"model","parts":[
                {"text":"ToDo に追加しました。"}
            ]}}]}),
        ];
        Ok(Arc::new(ScriptedBackend {
            responses: Mutex::new(
                script
                    .into_iter()
                    .map(|v| serde_json::from_value(v).expect("valid response"))
                    .collect(),
            ),
        }))
    }
}

/// 常に成功する秘書ツール（事前計画はツールが 2 つ以上のときだけ走るので 2 つ登録する）。
struct OkTool(&'static str);

#[async_trait]
impl Tool for OkTool {
    fn declaration(&self) -> yuuka_core::tool::FunctionDeclaration {
        yuuka_core::tool::FunctionDeclaration {
            name: ToolName::checked(self.0.to_owned()).expect("valid name"),
            description: String::new(),
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
        Ok(ToolOutcome::from_payload(json!({ "success": true })))
    }
}

#[tokio::test]
async fn secretary_turn_feeds_the_shared_metrics_registry() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("metrics.sqlite");
    drop(rusqlite::Connection::open(&path).expect("seed file"));
    let db = Db::open(&path).expect("open db");

    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("metrics-test-secret".to_owned())).unwrap());
    let enc = crypto.encrypt_text("fake-gemini-key").expect("encrypt");
    rusqlite::Connection::open(&path)
        .expect("open seed")
        .execute(
            "INSERT INTO users (discord_id, username, salt, role, \
             gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag, gemini_model) \
             VALUES ('u1', 'yuu', '00', 'user', ?1, ?2, ?3, 'gemini-3.1-flash-lite')",
            rusqlite::params![enc.encrypted, enc.iv, enc.auth_tag],
        )
        .expect("seed user");

    // 個人の秘書 Bot（システム Bot は想起しない）のスコープへ記憶を 1 件索引する。
    let bot_id = BotId::new("bot_mine");
    let (mut synapse, _) = SynapseEngine::boot(&path, DEFAULT_DIM);
    synapse.index(
        Scope {
            user_id: "u1".into(),
            bot_id: bot_id.as_str().into(),
            guild_id: None,
        },
        1,
        None,
        MEMORY.into(),
        FormationContext::default(),
    );

    let registry = ToolRegistry::new().with_provider(Arc::new(
        NativeProvider::new()
            .with_tool(Arc::new(OkTool("addTodo")))
            .and_then(|p| p.with_tool(Arc::new(OkTool("getTodos"))))
            .expect("register"),
    ));
    let metrics = Arc::new(MetricsRegistry::new());
    let engine = ChatEngine::new(
        db,
        Some(crypto),
        registry,
        Arc::new(ScriptedFactory),
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        Some(Arc::new(tokio::sync::Mutex::new(synapse))),
    )
    .with_metrics(metrics.clone());

    let before = metrics.snapshot_line();
    assert_eq!(
        before, "counters{} latency{}",
        "ターン前は何も記録されていない"
    );

    let status: StatusSink = Arc::new(|_: BotStatus| {});
    let reply = engine
        .secretary_turn(
            &bot_id,
            &UserId::new("u1"),
            IncomingChat {
                text: MEMORY.to_owned(),
                ..IncomingChat::default()
            },
            &status,
        )
        .await
        .expect("turn ok");
    assert_eq!(reply.text, "ToDo に追加しました。");

    let line = metrics.snapshot_line();
    for expected in [
        "messages=1",
        "plan_attempts=1",
        "tool_calls=1",
        "recall_hit=1",
        "assemble(count=1,",
        "response(count=1,",
    ] {
        assert!(line.contains(expected), "{expected} がない: {line}");
    }
    assert!(
        !line.contains("recall_empty"),
        "ヒット時は空振りを数えない: {line}"
    );
    assert!(
        !line.contains("async_predicted"),
        "軽い計画・配信経路なしのターンは途中応答へ回さない: {line}"
    );
}
