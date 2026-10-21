//! yuuka-gemini — reqwest 薄ラッパ（classic `generateContent` v1beta・function calling loop）。
//!
//! 現行 `src/gemini.ts` の Gemini API 面（generateContent 呼び出し・リトライ・FC ループ・
//! 完了是正）を型付きで 1:1 移植する。**オーケストレーション**（systemInstruction 組立・
//! シナプス想起・ターンプランナー・会話ログ保存）は上位層（bot/WS/services）の責務であり、
//! 本クレートは純粋な API クライアント＋ツール往復ループに徹する（§8.2）。
//!
//! DAG: `gemini → core`（Tool/ToolProvider/GeminiError の凍結契約のみ）。

pub mod breaker;
pub mod client;
pub mod fc_loop;
pub mod settings;
mod stream;
pub mod wire;
pub use settings::{init_llm_defaults, LlmSettings};

pub use breaker::CircuitBreaker;
pub use client::{
    ClientOptions, GeminiClient, GenerateBackend, OnText, ALLOWED_MODELS, DEFAULT_MODEL,
};
pub use fc_loop::{
    claims_action_completed, run_function_calling_loop, LoopOptions, LoopResult, Status, StatusCb,
    TextCb, ToolOutcomeRecord, ToolOutcomeSink, ToolOutcomeStatus, COMPLETION_CORRECTION_PROMPT,
};
pub use wire::{
    Candidate, Content, FunctionCall, FunctionCallingMode, FunctionDeclaration, FunctionResponse,
    GenerateContentResponse, GenerationConfig, InlineData, Part, Role, SystemInstruction, Tool,
    ToolConfig,
};

#[cfg(test)]
mod loop_tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::json;
    use yuuka_core::tool::FunctionDeclaration as CoreFnDecl;
    use yuuka_core::{
        BotId, GeminiError, ToolContext, ToolError, ToolName, ToolOutcome, ToolProvider, UserId,
    };

    use super::wire::GenerateContentResponse;
    use super::*;

    /// canned レスポンスを順番に返す fake backend。各 generate 呼び出しの tool_config を記録する。
    struct FakeBackend {
        scripted: Mutex<VecDeque<GenerateContentResponse>>,
        seen_tool_configs: Mutex<Vec<Option<ToolConfig>>>,
    }

    impl FakeBackend {
        fn new(responses: Vec<GenerateContentResponse>) -> Self {
            Self {
                scripted: Mutex::new(responses.into_iter().collect()),
                seen_tool_configs: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl GenerateBackend for FakeBackend {
        async fn generate(
            &self,
            _system_instruction: Option<&str>,
            _declarations: &[FunctionDeclaration],
            _contents: &[Content],
            tool_config: Option<ToolConfig>,
        ) -> Result<GenerateContentResponse, GeminiError> {
            self.seen_tool_configs.lock().unwrap().push(tool_config);
            self.scripted
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(GeminiError::MaxIterations)
        }
    }

    /// `native:add_todo` を 1 つ公開し、呼び出しを記録する fake provider。
    struct FakeProvider {
        invoked: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                invoked: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ToolProvider for FakeProvider {
        fn list(&self, _ctx: &ToolContext) -> Vec<CoreFnDecl> {
            vec![CoreFnDecl {
                name: ToolName::namespaced("native", "add_todo").unwrap(),
                description: "add a todo".to_owned(),
                parameters_json_schema: json!({
                    "$schema": "https://json-schema.org/draft-07/schema",
                    "type": "object",
                    "properties": { "title": { "type": "string" } }
                }),
                requires_confirmation: false,
            }]
        }

        async fn invoke(
            &self,
            name: &ToolName,
            args: serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutcome, ToolError> {
            let fail = args.get("title") == Some(&json!("fail"));
            self.invoked
                .lock()
                .unwrap()
                .push((name.as_str().to_owned(), args));
            // title="fail" はツール側の失敗（Node の `{success:false}` 応答）を返す。
            if fail {
                return Ok(ToolOutcome::from_payload(
                    json!({ "success": false, "message": "失敗" }),
                ));
            }
            Ok(ToolOutcome::from_payload(
                json!({ "success": true, "id": 1 }),
            ))
        }
    }

    fn resp(value: serde_json::Value) -> GenerateContentResponse {
        serde_json::from_value(value).unwrap()
    }

    fn ctx() -> ToolContext {
        ToolContext::new(BotId::system_default(), UserId::new("u1"))
    }

    #[tokio::test]
    async fn dispatches_tool_and_returns_final_text_with_id_correlation() {
        let backend = FakeBackend::new(vec![
            resp(json!({"candidates":[{"content":{"role":"model","parts":[
                {"functionCall":{"name":"native:add_todo","args":{"title":"牛乳を買う"},"id":"c1"}}
            ]}}]})),
            resp(json!({"candidates":[{"content":{"role":"model","parts":[
                {"text":"ToDo を追加しました。"}
            ]}}]})),
        ]);
        let provider = FakeProvider::new();
        let mut contents = vec![Content::user_text("牛乳を買うのを忘れないで")];
        let opts = LoopOptions::default();

        let result =
            run_function_calling_loop(&backend, &provider, "sys", &mut contents, &ctx(), &opts)
                .await
                .unwrap();

        assert_eq!(result.tool_calls, vec!["native:add_todo".to_owned()]);
        assert_eq!(result.text, "ToDo を追加しました。");
        assert!(!result.hit_max_iterations);

        // provider は正しい引数で 1 回呼ばれた。
        let invoked = provider.invoked.lock().unwrap();
        assert_eq!(invoked.len(), 1);
        assert_eq!(invoked[0].0, "native:add_todo");
        assert_eq!(invoked[0].1, json!({"title":"牛乳を買う"}));

        // contents に model の functionCall content と functionResponse（id=c1 相関）が積まれた。
        let last = contents.last().unwrap();
        assert_eq!(last.role, Role::User);
        let fr = last.parts[0].function_response.as_ref().unwrap();
        assert_eq!(fr.name, "native:add_todo");
        assert_eq!(fr.id.as_deref(), Some("c1"));
        assert_eq!(fr.response, json!({"success":true,"id":1}));
    }

    #[tokio::test]
    async fn streams_text_but_hides_completion_claims_until_corrected() {
        // 是正前の「登録しました」は利用者へ流さず、ツール実行後の本文だけを流す。
        let backend = FakeBackend::new(vec![
            resp(json!({"candidates":[{"content":{"role":"model","parts":[
                {"text":"タスクを登録しました。"}
            ]}}]})),
            resp(json!({"candidates":[{"content":{"role":"model","parts":[
                {"text":"手続きを進めます。"},
                {"functionCall":{"name":"native:add_todo","args":{"title":"x"},"id":"c1"}}
            ]}}]})),
            resp(json!({"candidates":[{"content":{"role":"model","parts":[
                {"text":"追加しました。"}
            ]}}]})),
        ]);
        let provider = FakeProvider::new();
        let mut contents = vec![Content::user_text("x を登録して")];
        let streamed = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&streamed);
        let opts = LoopOptions {
            on_text: Some(Arc::new(move |text: &str| {
                sink.lock().unwrap().push(text.to_owned());
            })),
            ..LoopOptions::default()
        };

        let result =
            run_function_calling_loop(&backend, &provider, "sys", &mut contents, &ctx(), &opts)
                .await
                .unwrap();

        assert_eq!(result.text, "追加しました。");
        assert_eq!(
            *streamed.lock().unwrap(),
            vec!["手続きを進めます。".to_owned(), "追加しました。".to_owned()]
        );
    }

    #[tokio::test]
    async fn completion_correction_forces_mode_any() {
        // resp1: 関数を呼ばず「登録しました」と主張 → 是正発火。
        // resp2: 是正後（mode=ANY）にツールを呼ぶ。resp3: 最終テキスト。
        let backend = FakeBackend::new(vec![
            resp(json!({"candidates":[{"content":{"role":"model","parts":[
                {"text":"タスクを登録しました。"}
            ]}}]})),
            resp(json!({"candidates":[{"content":{"role":"model","parts":[
                {"functionCall":{"name":"native:add_todo","args":{"title":"x"},"id":"c2"}}
            ]}}]})),
            resp(json!({"candidates":[{"content":{"role":"model","parts":[
                {"text":"本当に追加しました。"}
            ]}}]})),
        ]);
        let provider = FakeProvider::new();
        let mut contents = vec![Content::user_text("x を登録して")];
        let opts = LoopOptions {
            allowed_tool_names: Some(vec!["native:add_todo".to_owned()]),
            ..LoopOptions::default()
        };

        let result =
            run_function_calling_loop(&backend, &provider, "sys", &mut contents, &ctx(), &opts)
                .await
                .unwrap();

        assert_eq!(result.text, "本当に追加しました。");
        assert_eq!(provider.invoked.lock().unwrap().len(), 1);

        // 2 回目の generate（是正）が mode=ANY + allowedFunctionNames を伴っていること。
        let configs = backend.seen_tool_configs.lock().unwrap();
        assert!(configs[0].is_none(), "初回は AUTO（tool_config なし）");
        let corr = configs[1].as_ref().expect("是正呼び出しに tool_config");
        assert_eq!(corr.function_calling_config.mode, FunctionCallingMode::Any);
        assert_eq!(
            corr.function_calling_config.allowed_function_names,
            Some(vec!["native:add_todo".to_owned()])
        );
        assert!(configs[2].is_none(), "ツール結果返送後は再び AUTO");
    }

    #[tokio::test]
    async fn no_correction_when_text_is_plain() {
        let backend = FakeBackend::new(vec![resp(json!({"candidates":[{"content":{
            "role":"model","parts":[{"text":"こんにちは、今日はいい天気ですね。"}]
        }}]}))]);
        let provider = FakeProvider::new();
        let mut contents = vec![Content::user_text("やあ")];
        let opts = LoopOptions::default();

        let result =
            run_function_calling_loop(&backend, &provider, "sys", &mut contents, &ctx(), &opts)
                .await
                .unwrap();

        assert_eq!(result.text, "こんにちは、今日はいい天気ですね。");
        assert!(provider.invoked.lock().unwrap().is_empty());
        assert_eq!(backend.seen_tool_configs.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stops_at_max_iterations() {
        // 常にツールを呼び続けるモデルを 3 応答分与え、max_iterations=2 で打ち切る。
        let call = json!({"candidates":[{"content":{"role":"model","parts":[
            {"functionCall":{"name":"native:add_todo","args":{},"id":"c"}}
        ]}}]});
        let backend = FakeBackend::new(vec![resp(call.clone()), resp(call.clone()), resp(call)]);
        let provider = FakeProvider::new();
        let mut contents = vec![Content::user_text("loop")];
        let opts = LoopOptions {
            max_iterations: 2,
            ..LoopOptions::default()
        };

        let result =
            run_function_calling_loop(&backend, &provider, "sys", &mut contents, &ctx(), &opts)
                .await
                .unwrap();

        assert!(result.hit_max_iterations);
        assert_eq!(provider.invoked.lock().unwrap().len(), 2);
    }

    /// 記録内容（ツール名・引数・成否・user/bot/guild）を溜める fake 記録器。`fail=true` なら常に失敗する。
    #[derive(Default)]
    struct FakeOutcomeSink {
        fail: bool,
        #[allow(clippy::type_complexity)]
        records: Mutex<
            Vec<(
                String,
                serde_json::Value,
                ToolOutcomeStatus,
                String,
                String,
                Option<String>,
            )>,
        >,
    }

    #[async_trait]
    impl ToolOutcomeSink for FakeOutcomeSink {
        async fn record(
            &self,
            ctx: &ToolContext,
            record: ToolOutcomeRecord<'_>,
        ) -> Result<(), yuuka_core::DbError> {
            self.records.lock().unwrap().push((
                record.tool_name.to_owned(),
                record.args.clone(),
                record.status,
                ctx.user_id.as_str().to_owned(),
                ctx.bot_id.as_str().to_owned(),
                ctx.guild_id.as_ref().map(|g| g.as_str().to_owned()),
            ));
            if self.fail {
                return Err(yuuka_core::DbError::Operation("disk full".to_owned()));
            }
            Ok(())
        }
    }

    /// 1 ターンで成功・失敗のツールを 1 回ずつ呼ぶ応答列（並行呼び出し 2 件 → 最終テキスト）。
    fn success_and_failure_script() -> Vec<GenerateContentResponse> {
        vec![
            resp(json!({"candidates":[{"content":{"role":"model","parts":[
                {"functionCall":{"name":"native:add_todo","args":{"title":"牛乳"},"id":"c1"}},
                {"functionCall":{"name":"native:add_todo","args":{"title":"fail"},"id":"c2"}}
            ]}}]})),
            resp(json!({"candidates":[{"content":{"role":"model","parts":[
                {"text":"1 件は追加できませんでした。"}
            ]}}]})),
        ]
    }

    /// dispatch 毎に実行実績を 1 件ずつ記録し、`{success:false}` 応答は error として残す（R22）。
    #[tokio::test]
    async fn records_tool_outcome_per_dispatch_with_status() {
        let backend = FakeBackend::new(success_and_failure_script());
        let provider = FakeProvider::new();
        let sink = Arc::new(FakeOutcomeSink::default());
        let mut contents = vec![Content::user_text("牛乳と fail を追加して")];
        let opts = LoopOptions {
            outcome_sink: Some(sink.clone()),
            ..LoopOptions::default()
        };
        let mut ctx = ctx();
        ctx.guild_id = Some(yuuka_core::GuildId::new("g1"));

        let result =
            run_function_calling_loop(&backend, &provider, "sys", &mut contents, &ctx, &opts)
                .await
                .unwrap();
        assert_eq!(result.tool_calls.len(), 2);

        let records = sink.records.lock().unwrap();
        assert_eq!(records.len(), 2, "成功・失敗それぞれ 1 件ずつ記録される");
        let bot = BotId::system_default().as_str().to_owned();
        assert_eq!(
            records[0],
            (
                "native:add_todo".to_owned(),
                json!({"title":"牛乳"}),
                ToolOutcomeStatus::Success,
                "u1".to_owned(),
                bot.clone(),
                Some("g1".to_owned()),
            )
        );
        assert_eq!(records[1].0, "native:add_todo");
        assert_eq!(records[1].1, json!({"title":"fail"}));
        assert_eq!(records[1].2, ToolOutcomeStatus::Error);
    }

    /// 記録に失敗してもターンは失敗せず、ツール結果の返送と最終応答はそのまま続く。
    #[tokio::test]
    async fn outcome_sink_failure_does_not_fail_turn() {
        let backend = FakeBackend::new(success_and_failure_script());
        let provider = FakeProvider::new();
        let sink = Arc::new(FakeOutcomeSink {
            fail: true,
            ..FakeOutcomeSink::default()
        });
        let mut contents = vec![Content::user_text("牛乳と fail を追加して")];
        let opts = LoopOptions {
            outcome_sink: Some(sink.clone()),
            ..LoopOptions::default()
        };

        let result =
            run_function_calling_loop(&backend, &provider, "sys", &mut contents, &ctx(), &opts)
                .await
                .unwrap();
        assert_eq!(result.text, "1 件は追加できませんでした。");
        assert_eq!(provider.invoked.lock().unwrap().len(), 2);
        assert_eq!(
            sink.records.lock().unwrap().len(),
            2,
            "失敗しても毎回試みる"
        );
    }

    /// カウンタを溜める fake メトリクス記録先（実体の `MetricsRegistry` は services 側）。
    #[derive(Default)]
    struct FakeMetrics {
        counters: Mutex<std::collections::HashMap<String, u64>>,
    }

    impl yuuka_core::MetricsSink for FakeMetrics {
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

    /// dispatch 毎に `tool_calls` が増える（`{success:false}` の dispatch も数える・Node `incrMetric("tool_calls")`）。
    #[tokio::test]
    async fn counts_tool_calls_metric_per_dispatch() {
        let backend = FakeBackend::new(success_and_failure_script());
        let provider = FakeProvider::new();
        let metrics = Arc::new(FakeMetrics::default());
        let mut contents = vec![Content::user_text("牛乳と fail を追加して")];
        let opts = LoopOptions {
            metrics: Some(metrics.clone()),
            ..LoopOptions::default()
        };

        run_function_calling_loop(&backend, &provider, "sys", &mut contents, &ctx(), &opts)
            .await
            .unwrap();
        assert_eq!(
            metrics.counters.lock().unwrap().get("tool_calls").copied(),
            Some(2)
        );
    }

    #[test]
    fn outcome_status_follows_success_false_only() {
        assert_eq!(
            ToolOutcomeStatus::from_response(&json!({"success": false, "message": "x"})),
            ToolOutcomeStatus::Error
        );
        assert_eq!(
            ToolOutcomeStatus::from_response(&json!({"success": true})),
            ToolOutcomeStatus::Success
        );
        // success を持たない結果（説明文の包み等）は成功扱い（Node `reportedFailure` と同じ）。
        assert_eq!(
            ToolOutcomeStatus::from_response(&json!({"result": "text"})),
            ToolOutcomeStatus::Success
        );
        assert_eq!(ToolOutcomeStatus::Error.as_str(), "error");
        assert_eq!(ToolOutcomeStatus::Success.as_str(), "success");
    }
}
