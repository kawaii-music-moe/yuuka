//! classic `generateContent` v1beta の wire 型（§8.2.2）。
//!
//! **camelCase 厳密固定**（Interactions API の snake_case 語彙 `function_result`/`call_id`
//! を混入させない）。Part は oneof を「内部タグ無し・各フィールド Option」で表現する
//! （Gemini は 1 Part に 1 フィールドのみ載せてくる、が最も堅牢）。

use serde::{Deserialize, Serialize};

/// メッセージのロール。`role:"function"` も tool 結果返送時に来る。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Model,
    Function,
}

/// 1 ターン分のコンテンツ（role + parts）。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl Content {
    /// `role:"user"` のテキスト単一 part を作る。
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            parts: vec![Part::text(text)],
        }
    }

    /// `role:"model"` のテキスト単一 part を作る。
    #[must_use]
    pub fn model_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Model,
            parts: vec![Part::text(text)],
        }
    }

    /// この content の全 text part を連結して返す（`response.text()` 相当）。
    /// 思考要約 part（`thought: true`）はユーザー向けテキストに含めない。
    #[must_use]
    pub fn collect_text(&self) -> String {
        let mut out = String::new();
        for p in &self.parts {
            if p.extra.get("thought").and_then(serde_json::Value::as_bool) == Some(true) {
                continue;
            }
            if let Some(t) = &p.text {
                out.push_str(t);
            }
        }
        out
    }

    /// この content に含まれる functionCall を複製して集める。
    #[must_use]
    pub fn function_calls(&self) -> Vec<FunctionCall> {
        self.parts
            .iter()
            .filter_map(|p| p.function_call.clone())
            .collect()
    }
}

/// oneof の Part。1 Part に 1 フィールドのみ入る前提で各フィールドを Option 化する。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<InlineData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_response: Option<FunctionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_data: Option<FileData>,
    /// 上記以外のフィールド（`thoughtSignature`/`thought` 等）を往復で温存する。
    /// Gemini 3 系は functionCall part の `thoughtSignature` を次リクエストで
    /// 返送しないと 400 を返すため、未知フィールドの脱落は許されない
    /// （Node は `candidate.content` を生 JSON のまま返送しており、そのパリティ）。
    #[serde(flatten, skip_serializing_if = "serde_json::Map::is_empty", default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Part {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn inline_data(mime_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            inline_data: Some(InlineData {
                mime_type: mime_type.into(),
                data: data.into(),
            }),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn function_response(resp: FunctionResponse) -> Self {
        Self {
            function_response: Some(resp),
            ..Default::default()
        }
    }
}

/// base64 埋め込みメディア（レシート画像・音声メモ）。`data` は base64 文字列。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InlineData {
    pub mime_type: String,
    pub data: String,
}

/// Files API 経由メディア参照（20MB 超・将来対応）。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileData {
    pub mime_type: String,
    pub file_uri: String,
}

/// モデルが要求したツール呼び出し。`id` は並行呼び出しの相関に使う。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FunctionCall {
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// ツール実行結果の返送。`id` は対応する `FunctionCall.id` を写す（並行相関）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FunctionResponse {
    pub name: String,
    pub response: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// `tools[].functionDeclarations[]`。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub function_declarations: Vec<FunctionDeclaration>,
}

/// Gemini へ送る関数宣言。プラグイン由来はフル JSON Schema を `parametersJsonSchema` に載せる。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDeclaration {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters_json_schema: Option<serde_json::Value>,
}

/// `toolConfig.functionCallingConfig`。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfig {
    pub function_calling_config: FunctionCallingConfig,
}

impl ToolConfig {
    /// mode=ANY（+ 任意で許可ツール限定）の toolConfig を作る（完了是正で使う）。
    #[must_use]
    pub fn force_any(allowed_function_names: Option<Vec<String>>) -> Self {
        Self {
            function_calling_config: FunctionCallingConfig {
                mode: FunctionCallingMode::Any,
                allowed_function_names: allowed_function_names.filter(|v| !v.is_empty()),
            },
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCallingConfig {
    pub mode: FunctionCallingMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_function_names: Option<Vec<String>>,
}

/// AUTO（既定）/ ANY（完了是正のみ）/ NONE / VALIDATED（Preview・不使用）。
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FunctionCallingMode {
    Auto,
    Any,
    None,
    Validated,
}

/// `systemInstruction` 用の role 無し Content（role を送るとエラーになる実装があるため分離）。
#[derive(Serialize, Clone, Debug)]
pub struct SystemInstruction {
    pub parts: Vec<Part>,
}

impl SystemInstruction {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![Part::text(text)],
        }
    }
}

/// 生成パラメータ（temperature 等）。現状は response schema / temperature のみ最小定義。
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<serde_json::Value>,
}

/// `POST …:generateContent` のリクエスト body。
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<SystemInstruction>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<ToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
}

/// レスポンス。finishReason が SAFETY 等のとき content.parts が無いことがあるため
/// `content` は Option（無ガード .parts でパニックしない・現行 2026-07-07 障害の教訓）。
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentResponse {
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub usage_metadata: Option<UsageMetadata>,
    #[serde(default)]
    pub prompt_feedback: Option<PromptFeedback>,
}

impl GenerateContentResponse {
    /// 先頭候補の全 text part を連結して返す（`response.text()` 相当・候補無しは空文字）。
    #[must_use]
    pub fn text(&self) -> String {
        self.candidates
            .first()
            .and_then(|c| c.content.as_ref())
            .map(Content::collect_text)
            .unwrap_or_default()
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    #[serde(default)]
    pub content: Option<Content>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

impl Candidate {
    /// この候補が要求している functionCall 群（content 欠落時は空）。
    #[must_use]
    pub fn function_calls(&self) -> Vec<FunctionCall> {
        self.content
            .as_ref()
            .map(Content::function_calls)
            .unwrap_or_default()
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetadata {
    #[serde(default)]
    pub prompt_token_count: Option<u32>,
    #[serde(default)]
    pub candidates_token_count: Option<u32>,
    #[serde(default)]
    pub total_token_count: Option<u32>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PromptFeedback {
    #[serde(default)]
    pub block_reason: Option<String>,
}

// ─── エラー body（RetryInfo 抽出用） ───────────────────────────────────────────

/// `{ "error": { "code", "status", "message", "details":[...] } }` の最小写し。
#[derive(Deserialize, Debug, Default)]
pub struct ErrorEnvelope {
    #[serde(default)]
    pub error: Option<ApiErrorBody>,
}

#[derive(Deserialize, Debug, Default)]
pub struct ApiErrorBody {
    #[serde(default)]
    pub code: Option<u16>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub details: Vec<serde_json::Value>,
}

impl ErrorEnvelope {
    /// `google.rpc.RetryInfo` の `retryDelay`（`"37s"` 形式）を秒数へパースして返す。
    ///
    /// 現行 [`src/gemini.ts:439-449`] と同じ挙動: 見つかれば `seconds+1` 秒を待つよう
    /// 呼び出し側が使う（本メソッドは生の秒数を返す）。
    #[must_use]
    pub fn retry_delay_secs(&self) -> Option<u64> {
        let details = &self.error.as_ref()?.details;
        for d in details {
            let is_retry_info = d
                .get("@type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "type.googleapis.com/google.rpc.RetryInfo");
            if is_retry_info {
                if let Some(delay) = d.get("retryDelay").and_then(|v| v.as_str()) {
                    // "37s" → 37。末尾 's' を落として整数解釈（現行 parseInt 相当）。
                    let trimmed = delay.trim_end_matches('s');
                    if let Ok(secs) = trimmed.parse::<u64>() {
                        return Some(secs);
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_oneof_serializes_only_present_field() {
        let p = Part::text("hi");
        let v = serde_json::to_value(&p).expect("ser");
        assert_eq!(v, serde_json::json!({"text": "hi"}));

        let fr = Part::function_response(FunctionResponse {
            name: "addTodo".into(),
            response: serde_json::json!({"success": true}),
            id: Some("c1".into()),
        });
        let v = serde_json::to_value(&fr).expect("ser");
        assert_eq!(
            v,
            serde_json::json!({"functionResponse": {"name": "addTodo", "response": {"success": true}, "id": "c1"}})
        );
    }

    #[test]
    fn role_and_camelcase_wire_shape() {
        let c = Content {
            role: Role::Function,
            parts: vec![Part::inline_data("image/png", "AAAA")],
        };
        let v = serde_json::to_value(&c).expect("ser");
        assert_eq!(v["role"], "function");
        assert_eq!(v["parts"][0]["inlineData"]["mimeType"], "image/png");
    }

    #[test]
    fn function_calling_mode_screaming_snake() {
        let v = serde_json::to_value(FunctionCallingMode::Any).expect("ser");
        assert_eq!(v, serde_json::json!("ANY"));
        let v = serde_json::to_value(FunctionCallingMode::Validated).expect("ser");
        assert_eq!(v, serde_json::json!("VALIDATED"));
    }

    #[test]
    fn response_survives_missing_parts() {
        // finishReason=SAFETY で content.parts が無い候補でも panic せず空文字。
        let raw = r#"{"candidates":[{"finishReason":"SAFETY"}]}"#;
        let resp: GenerateContentResponse = serde_json::from_str(raw).expect("de");
        assert_eq!(resp.text(), "");
        assert!(resp.candidates[0].function_calls().is_empty());
    }

    #[test]
    fn response_collects_text_and_calls() {
        let raw = r#"{"candidates":[{"content":{"role":"model","parts":[
            {"text":"やって"},{"text":"おきました"},
            {"functionCall":{"name":"addTodo","args":{"title":"x"},"id":"c1"}}
        ]}}]}"#;
        let resp: GenerateContentResponse = serde_json::from_str(raw).expect("de");
        assert_eq!(resp.text(), "やっておきました");
        let calls = resp.candidates[0].function_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "addTodo");
        assert_eq!(calls[0].id.as_deref(), Some("c1"));
    }

    #[test]
    fn retry_info_delay_parsed_from_error_body() {
        let raw = r#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"quota",
            "details":[
                {"@type":"type.googleapis.com/google.rpc.QuotaFailure"},
                {"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"37s"}
            ]}}"#;
        let env: ErrorEnvelope = serde_json::from_str(raw).expect("de");
        assert_eq!(env.retry_delay_secs(), Some(37));
    }

    #[test]
    fn retry_info_absent_returns_none() {
        let raw = r#"{"error":{"code":500,"message":"boom","details":[]}}"#;
        let env: ErrorEnvelope = serde_json::from_str(raw).expect("de");
        assert_eq!(env.retry_delay_secs(), None);
    }

    #[test]
    fn part_roundtrips_thought_signature_on_function_call() {
        // Gemini 3 系: functionCall part の thoughtSignature を次リクエストで返送しないと 400。
        let raw = r#"{"functionCall":{"name":"mcp4_list_debts","args":{"include_paid":false}},
            "thoughtSignature":"sig-abc123"}"#;
        let p: Part = serde_json::from_str(raw).expect("de");
        assert_eq!(
            p.extra.get("thoughtSignature"),
            Some(&serde_json::Value::String("sig-abc123".into()))
        );
        let v = serde_json::to_value(&p).expect("ser");
        assert_eq!(v["thoughtSignature"], "sig-abc123");
        assert_eq!(v["functionCall"]["name"], "mcp4_list_debts");
    }

    #[test]
    fn part_without_extra_serializes_without_extra_keys() {
        let p = Part::text("hi");
        let v = serde_json::to_value(&p).expect("ser");
        assert_eq!(v, serde_json::json!({"text": "hi"}));
    }

    #[test]
    fn collect_text_skips_thought_parts() {
        let raw = r#"{"role":"model","parts":[
            {"text":"(内部思考)","thought":true},
            {"text":"こんにちは"}
        ]}"#;
        let c: Content = serde_json::from_str(raw).expect("de");
        assert_eq!(c.collect_text(), "こんにちは");
    }
}
