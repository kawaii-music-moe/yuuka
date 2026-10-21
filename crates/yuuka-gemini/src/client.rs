//! `GeminiClient` — reqwest 薄クライアント（§8.2.1〜8.2.3）。
//!
//! classic `generateContent` v1beta を叩き、429/`RetryInfo`/5xx/timeout を
//! [`GeminiError`] の variant に落とす。バックオフは `backon` の指数（ジッタ明示）だが、
//! **`RetryInfo.retryDelay` があればそれを最優先**する（現行 `generateWithRetry` の 1:1 移植）。
//! サーキットブレーカを併用して連続失敗時は即 fail（劣化縮退）。
//!
//! 既定設定のクライアント（[`GeminiClient::new`]）は HTTP 接続とブレーカをプロセス全体で共有する。
//! 本文を逐次受け取る [`GenerateBackend::generate_streaming`] は `streamGenerateContent`（SSE）を使う。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use backon::{BackoffBuilder, ExponentialBuilder};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use yuuka_core::GeminiError;

use crate::breaker::CircuitBreaker;
use crate::stream::{SseDecoder, StreamAccumulator};
use crate::wire::{
    Content, ErrorEnvelope, FunctionDeclaration, GenerateContentRequest, GenerateContentResponse,
    GenerationConfig, SystemInstruction, Tool, ToolConfig,
};

/// 既定モデル（GA・`-preview` は使わない・§8.2.5）。
pub const DEFAULT_MODEL: &str = "gemini-3.1-flash-lite";

/// ユーザーが選択できる Gemini モデルの許可リスト（GA モデルのみ・issue #39）。
///
/// PWA 設定画面（`PUT /api/client/settings`）はモデル選択肢を Gemini モデルに限定した上で、
/// **サーバー側でもこの許可リストに対して検証**する。Node 版は `body.model` を検証なしに
/// `users.gemini_model` へ保存しており、GPT-4o 等の他社モデル名を選ぶと以後 Discord を含む
/// そのユーザーの全会話が Gemini 呼び出し失敗で壊れていた（`docs/rust-rewrite/verification/
/// rpt-gemini-design-verify.md` で確認済みの GA モデル一覧と一致させ、単一の定数として
/// `crates/` 側の唯一の正とする）。
pub const ALLOWED_MODELS: &[&str] = &[
    "gemini-3.5-flash-lite",
    "gemini-3.5-flash",
    "gemini-3.1-flash-lite",
    "gemini-2.5-pro",
    "gemini-2.5-flash",
    "gemini-2.5-flash-lite",
];
const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
/// FC ループ用タイムアウト（現行 120s・[`src/gemini.ts:420`]）。
const LOOP_TIMEOUT: Duration = Duration::from_secs(120);
/// 補助生成用タイムアウト（現行 60s・[`src/services/llmClient.ts:137`]）。
const AUX_TIMEOUT: Duration = Duration::from_secs(60);
/// FC 生成のリトライ上限（現行 `maxRetries=3`）。
const LOOP_MAX_RETRIES: u32 = 3;
/// 補助生成のリトライ上限（現行 `maxRetries=2`・§8.2.6）。
const AUX_MAX_RETRIES: u32 = 2;
/// 使っていない接続を残しておく時間。会話の間が数分空いても TLS の確立からやり直さない。
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// アイドル中の HTTP/2 接続へ送る PING の間隔（途中の機器に黙って切られないようにする）。
const HTTP2_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// 生成中の本文を受け取るコールバック（[`GenerateBackend::generate_streaming`]）。
///
/// 引数の寿命を明示した別名にしているのは、`async_trait` が引数中の省略された寿命へ名前を付け、
/// `Fn(&str)` の高階の寿命が特定の寿命へ固定されてしまうのを避けるため。`'a` は借用したクロージャを
/// 渡せるようにする（型の別名の `dyn` は既定で `'static` になる）。
pub type OnText<'a> = dyn for<'s> Fn(&'s str) + Send + Sync + 'a;

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

    /// [`generate`](Self::generate) の本文を逐次受け取る版。本文が届くたびに、そのリクエストでここまでに
    /// 届いた本文全体（思考 part は除く）で `on_text` を呼ぶ。戻り値は届いた断片を 1 つにまとめた応答で、
    /// [`generate`](Self::generate) の戻り値と同じ形。リトライでやり直すと本文も最初から届き直す。
    ///
    /// 既定実装は逐次受信せず、生成し終えた本文で 1 回だけ `on_text` を呼ぶ（テストの fake backend 用）。
    ///
    /// # Errors
    /// [`generate`](Self::generate) と同じ。
    async fn generate_streaming(
        &self,
        system_instruction: Option<&str>,
        declarations: &[FunctionDeclaration],
        contents: &[Content],
        tool_config: Option<ToolConfig>,
        on_text: &OnText<'_>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        let resp = self
            .generate(system_instruction, declarations, contents, tool_config)
            .await?;
        let text = resp.text();
        if !text.is_empty() {
            on_text(&text);
        }
        Ok(resp)
    }
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

/// 既定設定のクライアントが共有する HTTP 接続とブレーカ（プロセスに 1 つ）。
///
/// クライアントはターンごとに作るが、確立済みの HTTP/2 接続（TLS ハンドシェイク済み）を使い回し、連続失敗も
/// 同じモデル・キーの呼び出し全体で数える。以前は毎回作り直していたため、ターンの最初の呼び出しが必ず接続の
/// 確立から始まり、ブレーカも毎ターン閉じた状態に戻って、上流が落ちていても無駄打ちを続けていた。
struct SharedTransport {
    http: reqwest::Client,
    /// (モデル, API キーの SHA-256) ごとのブレーカ。混雑の 503 はモデル単位、429 はキー単位で起きるため
    /// 両方で分け、別の利用者のキーの失敗で他の利用者まで止めない。キーそのものは持たない。
    breakers: Mutex<HashMap<BreakerKey, Arc<CircuitBreaker>>>,
}

/// ブレーカの引き当てキー（モデル名, API キーの SHA-256）。
type BreakerKey = (String, [u8; 32]);

static SHARED: OnceLock<SharedTransport> = OnceLock::new();

impl SharedTransport {
    fn get() -> Result<&'static Self, GeminiError> {
        if let Some(shared) = SHARED.get() {
            return Ok(shared);
        }
        // 構築に失敗したら次の呼び出しで作り直す（失敗をプロセスの寿命まで固定しない）。
        let http = build_http()?;
        Ok(SHARED.get_or_init(|| Self {
            http,
            breakers: Mutex::default(),
        }))
    }

    fn breaker(&self, model: &str, api_key: &SecretString) -> Arc<CircuitBreaker> {
        let key_digest: [u8; 32] = Sha256::digest(api_key.expose_secret().as_bytes()).into();
        let mut breakers = self.breakers.lock().unwrap_or_else(PoisonError::into_inner);
        let breaker = breakers
            .entry((model.to_owned(), key_digest))
            .or_insert_with(|| {
                let opts = ClientOptions::default();
                Arc::new(CircuitBreaker::new(
                    opts.breaker_failure_threshold,
                    opts.breaker_cooldown,
                ))
            });
        Arc::clone(breaker)
    }
}

fn build_http() -> Result<reqwest::Client, GeminiError> {
    reqwest::Client::builder()
        .timeout(LOOP_TIMEOUT)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .http2_keep_alive_interval(HTTP2_KEEP_ALIVE_INTERVAL)
        .http2_keep_alive_while_idle(true)
        .build()
        .map_err(|e| GeminiError::Transport(e.to_string()))
}

/// "models/foo" が渡ってもエンドポイント側で二重付与しないよう正規化。
fn normalize_model(model: String) -> String {
    model
        .strip_prefix("models/")
        .map(str::to_owned)
        .unwrap_or(model)
}

/// Gemini REST クライアント。API キーは `secrecy` で保持しログに漏らさない。
pub struct GeminiClient {
    http: reqwest::Client,
    api_key: SecretString,
    model: String,
    base_url: String,
    breaker: Arc<CircuitBreaker>,
}

impl GeminiClient {
    /// 既定オプションでクライアントを作る。HTTP 接続と、同じモデル・キーのブレーカはプロセス全体で共有する。
    ///
    /// # Errors
    /// reqwest クライアントの構築に失敗した場合 [`GeminiError::Transport`]。
    pub fn new(model: impl Into<String>, api_key: SecretString) -> Result<Self, GeminiError> {
        let shared = SharedTransport::get()?;
        let model = normalize_model(model.into());
        let breaker = shared.breaker(&model, &api_key);
        Ok(Self {
            http: shared.http.clone(),
            api_key,
            model,
            base_url: DEFAULT_BASE_URL.to_owned(),
            breaker,
        })
    }

    /// オプション指定でクライアントを作る。接続とブレーカはこのクライアント専用（他と共有しない）。
    ///
    /// # Errors
    /// reqwest クライアントの構築に失敗した場合 [`GeminiError::Transport`]。
    pub fn with_options(
        model: impl Into<String>,
        api_key: SecretString,
        opts: ClientOptions,
    ) -> Result<Self, GeminiError> {
        Ok(Self {
            http: build_http()?,
            api_key,
            model: normalize_model(model.into()),
            base_url: opts.base_url,
            breaker: Arc::new(CircuitBreaker::new(
                opts.breaker_failure_threshold,
                opts.breaker_cooldown,
            )),
        })
    }

    /// 使用中のモデル名。
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// 逐次受信なら `streamGenerateContent`（SSE）、そうでなければ `generateContent`。
    fn endpoint(&self, streaming: bool) -> String {
        let method = if streaming {
            "streamGenerateContent?alt=sse"
        } else {
            "generateContent"
        };
        format!(
            "{}/v1beta/models/{}:{method}",
            self.base_url.trim_end_matches('/'),
            self.model
        )
    }

    /// リクエストを組み立ててリトライ付きで送る共通処理。`on_text` があれば本文を逐次受け取る。
    async fn send_with_retry(
        &self,
        req: &GenerateContentRequest,
        max_retries: u32,
        per_request_timeout: Duration,
        on_text: Option<&OnText<'_>>,
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
            match self.send_once(req, per_request_timeout, on_text).await {
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
        on_text: Option<&OnText<'_>>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        let resp = self
            .http
            .post(self.endpoint(on_text.is_some()))
            // ?key= は URL がログに残るためヘッダで渡す（§8.2.2）。
            .header("x-goog-api-key", self.api_key.expose_secret())
            .timeout(per_request_timeout)
            .json(req)
            .send()
            .await
            .map_err(map_transport_error)?;

        let status = resp.status().as_u16();
        if status == 200 {
            if let Some(on_text) = on_text {
                return read_stream(resp, on_text).await;
            }
            let bytes = resp.bytes().await.map_err(map_transport_error)?;
            return serde_json::from_slice::<GenerateContentResponse>(&bytes)
                .map_err(|source| GeminiError::Decode { source });
        }

        // 非 2xx: RetryInfo を抽出してから分類する。
        let body = resp.text().await.unwrap_or_default();
        let env: ErrorEnvelope = serde_json::from_str(&body).unwrap_or_default();
        Err(status_error(status, &env))
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
        match self
            .send_with_retry(&req, AUX_MAX_RETRIES, AUX_TIMEOUT, None)
            .await
        {
            Ok(resp) => Some(resp),
            Err(err) => {
                tracing::warn!(error = %err, "補助生成に失敗（None へ縮退）");
                None
            }
        }
    }
}

/// FC ループの 1 回分のリクエスト（ツール宣言が空なら `tools` を送らない）。
fn loop_request(
    system_instruction: Option<&str>,
    declarations: &[FunctionDeclaration],
    contents: &[Content],
    tool_config: Option<ToolConfig>,
) -> GenerateContentRequest {
    let tools = if declarations.is_empty() {
        Vec::new()
    } else {
        vec![Tool {
            function_declarations: declarations.to_vec(),
        }]
    };
    GenerateContentRequest {
        contents: contents.to_vec(),
        system_instruction: system_instruction.map(SystemInstruction::text),
        tools,
        tool_config,
        generation_config: None,
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
        let req = loop_request(system_instruction, declarations, contents, tool_config);
        self.send_with_retry(&req, LOOP_MAX_RETRIES, LOOP_TIMEOUT, None)
            .await
    }

    async fn generate_streaming(
        &self,
        system_instruction: Option<&str>,
        declarations: &[FunctionDeclaration],
        contents: &[Content],
        tool_config: Option<ToolConfig>,
        on_text: &OnText<'_>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        let req = loop_request(system_instruction, declarations, contents, tool_config);
        self.send_with_retry(&req, LOOP_MAX_RETRIES, LOOP_TIMEOUT, Some(on_text))
            .await
    }
}

/// `streamGenerateContent?alt=sse` の本文を最後まで読み、断片を 1 つの応答へまとめる。本文が増えるたびに
/// `on_text` を呼ぶ。読み取り中の切断・時間切れは HTTP の失敗と同じ扱い（リトライ対象）。
async fn read_stream(
    mut resp: reqwest::Response,
    on_text: &OnText<'_>,
) -> Result<GenerateContentResponse, GeminiError> {
    let mut decoder = SseDecoder::default();
    let mut acc = StreamAccumulator::default();
    while let Some(bytes) = resp.chunk().await.map_err(map_transport_error)? {
        for event in decoder.feed(&bytes) {
            if acc.push_event(&event)? {
                on_text(acc.text());
            }
        }
    }
    if let Some(event) = decoder.finish() {
        if acc.push_event(&event)? {
            on_text(acc.text());
        }
    }
    Ok(acc.into_response())
}

/// 失敗したステータスとエラー本文を `GeminiError` へ分類する（本文の途中で届いたエラーにも使う）。
pub(crate) fn status_error(status: u16, env: &ErrorEnvelope) -> GeminiError {
    // 原因究明にはエラー本文が不可欠（例: thoughtSignature 欠落の 400）。message を必ずログへ残す。
    tracing::warn!(
        status,
        api_status = env.error.as_ref().and_then(|e| e.status.as_deref()),
        message = env.error.as_ref().and_then(|e| e.message.as_deref()),
        "Gemini generateContent が非 2xx を返却"
    );
    match status {
        429 => GeminiError::RateLimited {
            retry_after: env.retry_delay_secs().map(Duration::from_secs),
        },
        500 | 502 | 503 | 504 => GeminiError::ServerError { status },
        _ => GeminiError::Status { status },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> SecretString {
        SecretString::from(s.to_owned())
    }

    #[test]
    fn default_clients_share_breaker_per_model_and_key() {
        let a = GeminiClient::new("gemini-test-shared", key("k1")).unwrap();
        let b = GeminiClient::new("models/gemini-test-shared", key("k1")).unwrap();
        let other_key = GeminiClient::new("gemini-test-shared", key("k2")).unwrap();
        let other_model = GeminiClient::new("gemini-test-shared-2", key("k1")).unwrap();
        assert!(Arc::ptr_eq(&a.breaker, &b.breaker));
        assert!(!Arc::ptr_eq(&a.breaker, &other_key.breaker));
        assert!(!Arc::ptr_eq(&a.breaker, &other_model.breaker));
    }

    #[test]
    fn clients_with_options_keep_their_own_breaker() {
        let opts = ClientOptions::default;
        let a = GeminiClient::with_options("gemini-test-own", key("k"), opts()).unwrap();
        let b = GeminiClient::with_options("gemini-test-own", key("k"), opts()).unwrap();
        assert!(!Arc::ptr_eq(&a.breaker, &b.breaker));
    }

    #[test]
    fn endpoint_switches_to_sse_when_streaming() {
        let c = GeminiClient::new("models/gemini-x", key("k")).unwrap();
        assert!(c
            .endpoint(false)
            .ends_with("/v1beta/models/gemini-x:generateContent"));
        assert!(c
            .endpoint(true)
            .ends_with("/v1beta/models/gemini-x:streamGenerateContent?alt=sse"));
    }
}
