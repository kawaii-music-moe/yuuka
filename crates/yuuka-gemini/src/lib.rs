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
pub mod wire;

pub use breaker::CircuitBreaker;
pub use client::{ClientOptions, GeminiClient, GenerateBackend, DEFAULT_MODEL};
pub use fc_loop::{
    claims_action_completed, run_function_calling_loop, LoopOptions, LoopResult, Status, StatusCb,
    COMPLETION_CORRECTION_PROMPT,
};
pub use wire::{
    Candidate, Content, FunctionCall, FunctionCallingMode, FunctionDeclaration, FunctionResponse,
    GenerateContentResponse, GenerationConfig, InlineData, Part, Role, SystemInstruction, Tool,
    ToolConfig,
};

#[cfg(test)]
mod loop_tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

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
            self.invoked
                .lock()
                .unwrap()
                .push((name.as_str().to_owned(), args));
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
}
