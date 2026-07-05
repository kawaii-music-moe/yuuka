//! 層別エラーアーキテクチャ（絶対制約1）。
//!
//! - 各層 = 1 具体列挙型（`Other(String)` 的逃げ道を作らない）。
//! - `#[from]` は **真の層境界のみ**（直下位 enum → 直上位 enum の 1 段畳み込み）。
//!   層跨ぎ・HTTP 写像は明示 `match` / `map_err` で行う（§4.3）。
//! - `#[error(transparent)]` は **最下段の素通し限定**（§4.2）。
//! - クレート/モジュール境界を越えて公開する enum に `#[non_exhaustive]`（§4.4）。
//! - `anyhow`/`eyre`/`color-eyre`/`Box<dyn Error>` は使用しない（cargo-deny で機械強制）。
//!
//! 本モジュールは feature crate（db/web/gemini/discord/tools/ipc）がまだ空 stub の
//! Phase 0 段階なので、下位層エラーは **core 内に凍結定義**する。各 feature crate は
//! Phase 1 でこれらを自クレートへ移管/実装していくが、`AppError` への畳み込み口と
//! HTTP status の網羅 match はここで凍結し、バリアント追加漏れをコンパイルエラー化する。

use std::time::Duration;

/// 起動・設定層。config.yaml 欠落／型不一致／必須 secret 不備。
///
/// **唯一の fail-fast 層**（§5.6）。長寿命サービスのループ内では発生させない
/// （設定は起動時に検証済みの型付き構造体として渡す）。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("config file not found: {path}")]
    NotFound { path: String },

    #[error("config parse failed")]
    Parse {
        #[source]
        source: serde_yaml::Error,
    },

    #[error("required config field missing: {field}")]
    MissingField { field: &'static str },

    #[error("invalid config value for {field}: {reason}")]
    InvalidValue {
        field: &'static str,
        reason: String,
    },

    #[error("required secret missing: {name}")]
    MissingSecret { name: &'static str },
}

/// DB ドライバ層（rusqlite）。SQLITE_BUSY、制約違反、I/O、シリアライズ。
///
/// 回復可能（リトライ／縮退）。rusqlite / JoinError からの `#[from]` は
/// yuuka-db 実装時に付ける（Phase 0 では driver 依存を core に持ち込まない）。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DbError {
    #[error("write transaction is busy after retries")]
    Busy,

    #[error("writer actor is gone")]
    WriterGone,

    #[error("schema incompatible: expected version {expected}, found {found}")]
    Migration { expected: String, found: String },

    #[error("database operation failed: {0}")]
    Operation(String),
}

impl DbError {
    /// SQLITE_BUSY 相当か（backon リトライ判定に使う。実装は yuuka-db で拡張）。
    #[must_use]
    pub fn is_busy(&self) -> bool {
        matches!(self, DbError::Busy)
    }

    /// supervisor 分類（§5.7）。**DB エラーは一律 Transient にしてはならない**:
    /// `Migration`（schema_version 不一致）は起動時 fail-fast＝`Fatal`、`WriterGone`
    /// （writer actor 消失）は再起動で直らない恒久障害＝`Permanent`、`Busy`/`Operation`
    /// のみ一過性＝`Transient`。DbError は `#[non_exhaustive]` だが core 内なので網羅
    /// match（`_ =>` 不要）でバリアント追加漏れをコンパイルエラー化できる。
    #[must_use]
    pub fn fatality(&self) -> Fatality {
        match self {
            DbError::Migration { .. } => Fatality::Fatal,
            DbError::WriterGone => Fatality::Permanent,
            DbError::Busy | DbError::Operation(_) => Fatality::Transient,
        }
    }
}

impl RepoError {
    /// supervisor 分類。`Db` は内側 [`DbError::fatality`] へ委譲し、`NotFound`/
    /// `UserScopeMissing`（要求・ロジック不備）は `Permanent`（再起動で直らない）。
    #[must_use]
    pub fn fatality(&self) -> Fatality {
        match self {
            RepoError::Db(e) => e.fatality(),
            RepoError::NotFound { .. } | RepoError::UserScopeMissing => Fatality::Permanent,
        }
    }
}

/// リポジトリ層（データアクセス）。`DbError` の意味付け、`NotFound`、スコープ欠落。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RepoError {
    // 真の層境界: DbError -> RepoError。
    #[error("database error")]
    Db(#[from] DbError),

    #[error("record not found: {kind} id={id}")]
    NotFound { kind: &'static str, id: i64 },

    /// データ分離キー欠落を型で顕在化（UserScope と連動）。
    #[error("user scope missing for repository operation")]
    UserScopeMissing,
}

/// 認証・認可層。セッション無効、権限不足、トークン不正。回復可能（401/403 化）。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthError {
    #[error("session invalid or expired")]
    SessionInvalid,

    #[error("bearer token malformed")]
    TokenMalformed,

    #[error("forbidden: insufficient role")]
    Forbidden,

    /// 認証ストア（Redis/SQLite）への到達不能。**未認証(401)ではなく回復可能な上流障害**
    /// として区別し、502 化・監視/リトライ判断に使う（Redis 断を 401 に潰さない）。
    #[error("auth backend unavailable")]
    Backend,
}

/// 入力検証層。DTO 制約違反、範囲外、必須欠落。回復可能（400 化）。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ValidationError {
    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("required field missing: {field}")]
    MissingField { field: &'static str },
}

/// Gemini 薄ラッパ層。429/`RetryInfo`、5xx、パース失敗、safety block。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GeminiError {
    /// 現行 rate-limit バックオフを 1:1 移植するための構造化バリアント。
    /// `retry_after` は RetryInfo(retryDelay) 由来。
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },

    #[error("gemini returned status {status}")]
    Status { status: u16 },

    #[error("response decode failed")]
    Decode {
        #[source]
        source: serde_json::Error,
    },

    #[error("content blocked by safety filter")]
    SafetyBlocked,
}

/// Discord 層（twilight）。ゲートウェイ断、HTTP 4xx/5xx、レート制限。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiscordError {
    #[error("gateway connection lost")]
    GatewayClosed,

    #[error("discord http error: status {status}")]
    Http { status: u16 },

    #[error("discord rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },
}

/// ツール／プラグイン層（Native/MCP/WASM）。呼び出し失敗、能力スコープ違反、タイムアウト。
///
/// 設計上 `PluginError` と同義（本プロジェクトは `ToolError` を第一名とする。§05）。
/// `type PluginError = ToolError` のエイリアスを併設する。
///
/// **凍結の例外（意図的）**: 05-plugins-types.md §9.1 の `Mcp(#[from] McpError)` /
/// `Wasm(#[from] WasmError)` 層境界バリアントは、下位クレート（yuuka-tools の
/// McpProvider/WasmProvider）が未実装の Phase 0 では**あえて追加しない**。本 enum は
/// `#[non_exhaustive]` なので、Phase 1 でこれらを追加しても下流ビルドは壊れない
/// （＝「凍結後は触らない」原則の明示的な許可された拡張点）。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ToolError {
    #[error("invalid tool name: {0}")]
    InvalidToolName(String),

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("tool unavailable (disabled/removed/out-of-scope)")]
    Unavailable,

    #[error("capability denied: {capability}")]
    CapabilityDenied { capability: String },

    #[error("arguments do not match schema: {0}")]
    InvalidArguments(String),

    #[error("tool execution timed out")]
    Timeout,

    #[error("tool execution failed: {0}")]
    Execution(String),
}

/// `ToolError` の設計別名（§4.1 表の `PluginError` に相当）。
pub type PluginError = ToolError;

/// プロセス間・synapse 層。接続断、プロトコル不整合。回復可能（縮退＝直近履歴のみ）。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IpcError {
    #[error("ipc connection closed")]
    Closed,

    #[error("ipc protocol mismatch: {0}")]
    Protocol(String),

    #[error("ipc request timed out")]
    Timeout,
}

/// Web 面（axum ハンドラ）。各層を HTTP へ写像する統合型。
///
/// **`#[non_exhaustive]` は付けない**（§4.5）: `IntoResponse`（yuuka-web 側）と
/// 同一クレートで網羅 match を保ち、バリアント追加漏れをコンパイルエラー化するため。
/// core 側では HTTP status を返す網羅 match（`_ =>` 禁止）を [`WebError::status`] に凍結する。
#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("invalid request: {0}")]
    Validation(String),
    #[error("upstream unavailable")]
    Upstream,
    #[error("internal error")]
    Internal,
}

impl WebError {
    /// HTTP status への **網羅 match（`_ =>` 禁止）**。
    ///
    /// バリアント追加時にこの match がコンパイルエラーになることで写像漏れを検知する。
    /// `IntoResponse` 本体は yuuka-web（Phase 1）で書くが、status 割当と
    /// 「内部 Display をクライアントに漏らさない」原則は core で凍結する。
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            WebError::Unauthorized => 401,
            WebError::Forbidden => 403,
            WebError::NotFound => 404,
            WebError::Validation(_) => 400,
            WebError::Upstream => 502,
            WebError::Internal => 500,
        }
    }

    /// クライアントへ返してよいメッセージ。内部 Display（DbError 等の機微）は漏らさない。
    /// Validation のみユーザー入力由来なので Display を露出してよい。
    #[must_use]
    pub fn client_message(&self) -> String {
        match self {
            WebError::Unauthorized => "unauthorized".to_owned(),
            WebError::Forbidden => "forbidden".to_owned(),
            WebError::NotFound => "not found".to_owned(),
            WebError::Validation(m) => m.clone(),
            WebError::Upstream => "upstream unavailable".to_owned(),
            WebError::Internal => "internal".to_owned(),
        }
    }
}

/// 最上位アプリエラー（supervisor / 起動シーケンスが扱う統合型）。
///
/// 各層エラーを **真の層境界の `#[from]`** で畳み込む。`ConfigError` のみ fail-fast、
/// 他は回復可能。`status()` は網羅 match（`_ =>` 禁止）でバリアント追加漏れをコンパイルエラー化。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AppError {
    #[error("configuration error")]
    Config(#[from] ConfigError),
    #[error("database error")]
    Db(#[from] DbError),
    #[error("repository error")]
    Repo(#[from] RepoError),
    #[error("authentication error")]
    Auth(#[from] AuthError),
    #[error("validation error")]
    Validation(#[from] ValidationError),
    #[error("gemini error")]
    Gemini(#[from] GeminiError),
    #[error("discord error")]
    Discord(#[from] DiscordError),
    #[error("plugin/tool error")]
    Tool(#[from] ToolError),
    #[error("ipc error")]
    Ipc(#[from] IpcError),
    #[error("web error")]
    Web(#[from] WebError),
}

impl AppError {
    /// このエラーが起動時 fail-fast に相当するか（`ConfigError` / `DbError::Migration`
    /// 等の `Fatal`・§5.6）。
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(self.fatality(), Fatality::Fatal)
    }

    /// 一過性障害か（supervisor が指数バックオフ再 spawn するのは `Transient` のみ・§5.2）。
    /// `Permanent`（再起動で直らない）/`Fatal`（起動時致命）は再起動対象にしない。
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(self.fatality(), Fatality::Transient)
    }

    /// supervisor 分類（§5.7）。**網羅 match（`_ =>` 禁止）**でバリアント追加漏れを検知。
    #[must_use]
    pub fn fatality(&self) -> Fatality {
        match self {
            // 起動時 config/secret 不備のみ致命的。
            AppError::Config(_) => Fatality::Fatal,
            // DB/Repo は variant 別に委譲（Migration=Fatal, WriterGone=Permanent, Busy=Transient）。
            // 一律 Transient にすると起動時 schema 不一致が無限バックオフ再起動になる。
            AppError::Db(e) => e.fatality(),
            AppError::Repo(e) => e.fatality(),
            // 外部依存の一過性障害はバックオフ再起動対象。
            AppError::Gemini(_) => Fatality::Transient,
            AppError::Discord(_) => Fatality::Transient,
            AppError::Ipc(_) => Fatality::Transient,
            // 恒久（クライアント要求の不備）はリクエスト単位で 4xx 化し、再起動対象にしない。
            AppError::Auth(_) => Fatality::Permanent,
            AppError::Validation(_) => Fatality::Permanent,
            AppError::Tool(_) => Fatality::Permanent,
            AppError::Web(_) => Fatality::Permanent,
        }
    }
}

/// レジリエンス分類（§5.7・supervisor 用）。
///
/// - `Transient`  = バックオフ再起動（一過性障害）。
/// - `Permanent`  = 停止・管理 UI 通知（恒久障害・再起動しても直らない）。
/// - `Fatal`      = 起動時 config/secret 不備のみ（プロセス即終了）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fatality {
    Transient,
    Permanent,
    Fatal,
}

/// 再起動可否分類（`Fatality` と同一 3 値。呼称の異なる別名として凍結）。
///
/// supervisor はサービスの戻り値エラーを `Retryability` で分類し、
/// `Transient` のみ指数バックオフ再 spawn する。
pub type Retryability = Fatality;
