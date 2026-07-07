//! `GeminiClient` — reqwest 薄クライアント（§8.2.1〜8.2.3）。
//!
//! classic `generateContent` v1beta を叩き、429/`RetryInfo`/5xx/timeout を
//! [`GeminiError`] の variant に落とす。バックオフは `backon` の指数（ジッタ明示）だが、
//! **`RetryInfo.retryDelay` があればそれを最優先**する（現行 `generateWithRetry` の 1:1 移植）。
//! サーキットブレーカを併用して連続失敗時は即 fail（劣化縮退）。

use std::time::Duration;

use async_trait::async_trait;
use backon::{BackoffBuilder, ExponentialBuilder};
use secrecy::{ExposeSecret, SecretString};
use yuuka_core::GeminiError;

use crate::breaker::CircuitBreaker;
use crate::wire::{
    Content, ErrorEnvelope, FunctionDeclaration, GenerateContentRequest, GenerateContentResponse,
    GenerationConfig, SystemInstruction, Tool, ToolConfig,
};

/// 既定モデル（GA・`-preview` は使わない・§8.2.5）。
pub const DEFAULT_MODEL: &str = "gemini-3.1-flash-lite";
const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
/// FC ループ用タイムアウト（現行 120s・[`src/gemini.ts:420`]）。
const LOOP_TIMEOUT: Duration = Duration::from_secs(120);
/// 補助生成用タイムアウト（現行 60s・[`src/services/llmClient.ts:137`]）。
const AUX_TIMEOUT: Duration = Duration::from_secs(60);
/// FC 生成のリトライ上限（現行 `maxRetries=3`）。
const LOOP_MAX_RETRIES: u32 = 3;
/// 補助生成のリトライ上限（現行 `maxRetries=2`・§8.2.6）。
const AUX_MAX_RETRIES: u32 = 2;

/// FC ループがテスト時に差し替え可能な生成バックエンド。
///
/// 本番は [`GeminiClient`]、テストは canned レスポンスを返す fake を注入して
/// ネットワーク無しでループ意味論（往復・並行相関・完了是正）を検証する。
#[async_trait]
pub trait GenerateBackend: Send + Sync {
    /// 1 回の generateContent 呼び出し（内部でリトライ／ブレーカを含む）。
    ///
    /// # Errors
    /// 429/5xx/timeout/transport/decode を [`GeminiError`] で返す（握り潰さない）。
    async fn generate(
        &self,
        system_instruction: Option<&str>,
        declarations: &[FunctionDeclaration],
        contents: &[Content],
        tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError>;
}

/// `GeminiClient` の構築オプション。
#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub base_url: String,
    pub breaker_failure_threshold: u64,
    pub breaker_cooldown: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            breaker_failure_threshold: 5,
            breaker_cooldown: Duration::from_secs(30),
        }
    }
}

/// Gemini REST クライアント。API キーは `secrecy` で保持しログに漏らさない。
pub struct GeminiClient {
    http: reqwest::Client,
    api_key: SecretString,
    model: String,
    base_url: String,
    breaker: CircuitBreaker,
}

impl GeminiClient {
    /// 既定オプションでクライアントを作る。
    ///
    /// # Errors
    /// reqwest クライアントの構築に失敗した場合 [`GeminiError::Transport`]。
    pub fn new(model: impl Into<String>, api_key: SecretString) -> Result<Self, GeminiError> {
        Self::with_options(model, api_key, ClientOptions::default())
    }

    /// オプション指定でクライアントを作る。
    ///
    /// # Errors
    /// reqwest クライアントの構築に失敗した場合 [`GeminiError::Transport`]。
    pub fn with_options(
        model: impl Into<String>,
        api_key: SecretString,
        opts: ClientOptions,
    ) -> Result<Self, GeminiError> {
        let http = reqwest::Client::builder()
            .timeout(LOOP_TIMEOUT)
            .build()
            .map_err(|e| GeminiError::Transport(e.to_string()))?;
        let model = model.into();
        // "models/foo" が渡ってもエンドポイント側で二重付与しないよう正規化。
        let model = model
            .strip_prefix("models/")
            .map(str::to_owned)
            .unwrap_or(model);
        Ok(Self {
            http,
            api_key,
            model,
            base_url: opts.base_url,
            breaker: CircuitBreaker::new(opts.breaker_failure_threshold, opts.breaker_cooldown),
        })
    }

    /// 使用中のモデル名。
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1beta/models/{}:generateContent",
            self.base_url.trim_end_matches('/'),
            self.model
        )
    }

    /// リクエストを組み立ててリトライ付きで送る共通処理。
    async fn send_with_retry(
        &self,
        req: &GenerateContentRequest,
        max_retries: u32,
        per_request_timeout: Duration,
    ) -> Result<GenerateContentResponse, GeminiError> {
        // ブレーカが open（cool-down 中）なら即劣化縮退へ（無駄打ち回避）。
        if !self.breaker.acquire() {
            return Err(GeminiError::ServerError { status: 503 });
        }

        // 指数バックオフ（ジッタ明示・§8.2.3）: 2s → 4s → 8s（上限 60s）。RetryInfo があれば優先。
        let mut backoff = ExponentialBuilder::default()
            .with_min_delay(Duration::from_secs(2))
            .with_max_delay(Duration::from_secs(60))
            .with_factor(2.0)
            .with_jitter()
            .with_max_times(max_retries as usize)
            .build();

        let mut attempt: u32 = 0;
        loop {
            match self.send_once(req, per_request_timeout).await {
                Ok(resp) => {
                    self.breaker.on_success();
                    return Ok(resp);
                }
                Err(err) => {
                    if err.is_retryable() && attempt < max_retries {
                        // RetryInfo(retryDelay) を最優先。現行同様に +1s の安全マージンを足す。
                        // 無ければ指数バックオフ（ジッタ込み）。
                        let wait = err
                            .retry_after()
                            .map(|d| d + Duration::from_secs(1))
                            .or_else(|| backoff.next())
                            .unwrap_or(Duration::from_secs(60));
                        tracing::warn!(
                            attempt = attempt + 1,
                            max_retries,
                            wait_secs = wait.as_secs(),
                            error = %err,
                            "gemini 一時障害・リトライ"
                        );
                        tokio::time::sleep(wait).await;
                        attempt += 1;
                        continue;
                    }
                    // 一過性障害を出し切った（or 非リトライ）。一過性のみブレーカへ計上する
                    // （4xx はクライアント要因なので上流健全性の判定に混ぜない）。
                    if err.is_retryable() {
                        self.breaker.on_failure();
                    }
                    return Err(err);
                }
            }
        }
    }

    /// HTTP 1 回。ステータスを分類し `GeminiError` variant へ写像する。
    async fn send_once(
        &self,
        req: &GenerateContentRequest,
        per_request_timeout: Duration,
    ) -> Result<GenerateContentResponse, GeminiError> {
        let resp = self
            .http
            .post(self.endpoint())
            // ?key= は URL がログに残るためヘッダで渡す（§8.2.2）。
            .header("x-goog-api-key", self.api_key.expose_secret())
            .timeout(per_request_timeout)
            .json(req)
            .send()
            .await
            .map_err(map_transport_error)?;

        let status = resp.status().as_u16();
        if status == 200 {
            let bytes = resp.bytes().await.map_err(map_transport_error)?;
            return serde_json::from_slice::<GenerateContentResponse>(&bytes)
                .map_err(|source| GeminiError::Decode { source });
        }

        // 非 2xx: RetryInfo を抽出してから分類する。
        let body = resp.text().await.unwrap_or_default();
        let env: ErrorEnvelope = serde_json::from_str(&body).unwrap_or_default();
        match status {
            429 => Err(GeminiError::RateLimited {
                retry_after: env.retry_delay_secs().map(Duration::from_secs),
            }),
            500 | 502 | 503 | 504 => Err(GeminiError::ServerError { status }),
            _ => Err(GeminiError::Status { status }),
        }
    }

    /// 補助生成（ツール無し・§8.2.6）。タグ自動付与・要約・文字起こし用。
    ///
    /// 失敗時は現行同様 `None`（呼び出し側で縮退）。`response_schema` を渡すと構造化出力。
    pub async fn generate_aux(
        &self,
        system_instruction: Option<&str>,
        contents: Vec<Content>,
        generation_config: Option<GenerationConfig>,
    ) -> Option<GenerateContentResponse> {
        let req = GenerateContentRequest {
            contents,
            system_instruction: system_instruction.map(SystemInstruction::text),
            tools: Vec::new(),
            tool_config: None,
            generation_config,
        };
        match self.send_with_retry(&req, AUX_MAX_RETRIES, AUX_TIMEOUT).await {
            Ok(resp) => Some(resp),
            Err(err) => {
                tracing::warn!(error = %err, "補助生成に失敗（None へ縮退）");
                None
            }
        }
    }
}

#[async_trait]
impl GenerateBackend for GeminiClient {
    async fn generate(
        &self,
        system_instruction: Option<&str>,
        declarations: &[FunctionDeclaration],
        contents: &[Content],
        tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        let tools = if declarations.is_empty() {
            Vec::new()
        } else {
            vec![Tool {
                function_declarations: declarations.to_vec(),
            }]
        };
        let req = GenerateContentRequest {
            contents: contents.to_vec(),
            system_instruction: system_instruction.map(SystemInstruction::text),
            tools,
            tool_config,
            generation_config: None,
        };
        self.send_with_retry(&req, LOOP_MAX_RETRIES, LOOP_TIMEOUT)
            .await
    }
}

/// reqwest エラーを `GeminiError` へ写像（timeout は専用 variant、他は Transport）。
fn map_transport_error(e: reqwest::Error) -> GeminiError {
    if e.is_timeout() {
        GeminiError::Timeout
    } else {
        GeminiError::Transport(e.to_string())
    }
}
