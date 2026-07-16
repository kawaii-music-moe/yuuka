//! yuuka-mcp — MCP サーバー管理 + ダッシュボードプロキシ API（`/api/mcp-servers*`・`/proxy/mcp/:id/mcp`）。
//! Node `src/server/routes/mcpRoutes.ts` パリティ。
//!
//! 認可は yuuka-web の [`AuthenticatedUser`](yuuka_web::AuthenticatedUser) extractor で型強制する
//! （管理系 7 ルートは `auth:"user"`・proxy 2 ルートは `auth:"none"` で proxyToken 主体）。ルート固有の
//! 実行時依存（保存時暗号・外部 MCP サーバー HTTP クライアント・proxyToken ストア）は [`McpRuntime`] に
//! まとめ `Extension` レイヤで各ハンドラへ注入する（`SettingsRuntime`/`AdminRuntime` と同方式）。
//!
//! **外部 MCP サーバー HTTP シーム**: `tools/list` 取得・ダッシュボード判定/取得・エンドポイント中継は
//! 外部サードパーティ MCP サーバーへの HTTP に触れる。これは [`McpClient`] ポート越しに委譲し、未配線時は
//! [`NullMcpClient`]（正直な縮退＝probe false・取得/中継は失敗）へ落とす。**DB 効果（登録・削除・
//! 有効/無効・監査）と proxyToken の発行/失効は常に完全に働く**。実クライアント（`reqwest` 背後で
//! Node `mcpClient.ts` の JSON-RPC/SSE + SSRF ガードを移植）を注入すれば live 化する。

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::routing::{get, options, post};
use axum::{Extension, Router};
use serde_json::Value;
use yuuka_crypto::SystemCrypto;
use yuuka_web::AppState;

mod http_client;
pub mod provider;
pub mod repo;
mod routes;
mod tokens;

pub use http_client::HttpMcpClient;
pub use provider::{mcp_function_name, McpProvider, TOOLS_CACHE_TTL};
pub use repo::McpServerRecord;
pub use tokens::ProxyTokenManager;

// ─── 外部 MCP サーバー HTTP シーム ───────────────────────────────────────────

/// 外部 MCP サーバーが公開する 1 ツール（`tools/list` 由来・Node `McpToolDef`）。
#[derive(Debug, Clone)]
pub struct McpTool {
    /// ツール名。
    pub name: String,
    /// 説明（省略可）。
    pub description: Option<String>,
    /// 入力パラメータの JSON Schema（`tools/list` の `inputSchema`・省略可・Node `McpToolDef.inputSchema`）。
    /// FC ループの `parametersJsonSchema` 生成（動的ツール登録）に使う。
    pub input_schema: Option<Value>,
}

/// 上流 MCP サーバーからのプロキシ応答（Node の `upstreamRes` 転送に対応）。
#[derive(Debug, Clone)]
pub struct ProxyResponse {
    /// HTTP ステータス。
    pub status: u16,
    /// `Content-Type`（既定 `application/json`）。
    pub content_type: String,
    /// `Mcp-Session-Id`（あれば透過する）。
    pub mcp_session_id: Option<String>,
    /// 応答ボディ（バイト列を素通し）。
    pub body: Bytes,
}

/// 外部 MCP サーバーへの HTTP アクセスポート（Node `mcpClient.ts` 相当）。未配線時は [`NullMcpClient`]。
///
/// 実装は Node と同じく利用直前の SSRF 再検証（DNS リバインディング遮断）・タイムアウト・Bearer 認証
/// 注入を担う。ここに切り出すことで管理系ルートの DB 効果と proxyToken 管理を外部依存なしにテストできる。
#[async_trait]
pub trait McpClient: Send + Sync {
    /// サーバーが管理ページを提供しているか（`GET <origin>/dashboard/enable` が 200 か）。到達不可は false。
    async fn probe_dashboard(&self, server: &repo::McpServerRecord) -> bool;

    /// 管理ページ HTML を取得する（`GET <origin>/dashboard`）。返り値は `(status, html)`。
    ///
    /// # Errors
    /// 到達不可・タイムアウト等で `Err(message)`（`message` はユーザー向け日本語に埋め込まれる）。
    async fn fetch_dashboard_html(
        &self,
        server: &repo::McpServerRecord,
    ) -> Result<(u16, String), String>;

    /// `tools/list` を取得する（Node `listTools`）。
    ///
    /// # Errors
    /// 到達不可・プロトコルエラー等で `Err(message)`。
    async fn refresh_tools(&self, server: &repo::McpServerRecord) -> Result<Vec<McpTool>, String>;

    /// `tools/call` を実行して結果テキスト（`content` の text 部分を連結）を返す（Node `callTool`）。
    ///
    /// トランスポート/セッション由来エラーは Node `callRpc` と同じく 1 度だけ再初期化して再試行する。
    /// ツール側エラー（`isError:true`）は再試行しない（副作用の二重実行を避ける）。
    ///
    /// # Errors
    /// 到達不可・タイムアウト・RPC エラー・ツールエラー（`isError`）等で `Err(message)`。
    async fn call_tool(
        &self,
        server: &repo::McpServerRecord,
        tool_name: &str,
        arguments: Value,
    ) -> Result<String, String>;

    /// エンドポイントへ 1 リクエストを中継する（Node の `fetch(server.endpoint_url, ...)` 転送）。
    ///
    /// # Errors
    /// 接続失敗・タイムアウト等で `Err(message)`。
    async fn proxy(
        &self,
        server: &repo::McpServerRecord,
        body: Bytes,
        accept: Option<&str>,
    ) -> Result<ProxyResponse, String>;

    /// akizakura.css（ダッシュボードの design system）を取得する。取得不能は `None`（`<link>` を残す縮退）。
    async fn fetch_akizakura_css(&self) -> Option<String> {
        None
    }
}

/// 外部 MCP サーバー HTTP 未配線時の縮退実装（正直な縮退・no live 効果）。
///
/// probe は常に false（管理ページ非提供扱い）、HTML/tools/proxy は常に `Err`（外部未接続を明示）。
/// これにより登録・削除・有効/無効・proxyToken 管理は完全に働きつつ、外部到達分だけが縮退する。
pub struct NullMcpClient;

#[async_trait]
impl McpClient for NullMcpClient {
    async fn probe_dashboard(&self, _server: &repo::McpServerRecord) -> bool {
        false
    }

    async fn fetch_dashboard_html(
        &self,
        _server: &repo::McpServerRecord,
    ) -> Result<(u16, String), String> {
        Err("MCP クライアントが配線されていません".to_owned())
    }

    async fn refresh_tools(&self, _server: &repo::McpServerRecord) -> Result<Vec<McpTool>, String> {
        Err("MCP クライアントが配線されていません".to_owned())
    }

    async fn call_tool(
        &self,
        _server: &repo::McpServerRecord,
        _tool_name: &str,
        _arguments: Value,
    ) -> Result<String, String> {
        Err("MCP クライアントが配線されていません".to_owned())
    }

    async fn proxy(
        &self,
        _server: &repo::McpServerRecord,
        _body: Bytes,
        _accept: Option<&str>,
    ) -> Result<ProxyResponse, String> {
        Err("MCP クライアントが配線されていません".to_owned())
    }
}

// ─── ランタイム + ルータ ──────────────────────────────────────────────────────

/// MCP ルートが使う実行時依存（`Extension` で各ハンドラへ注入）。
pub struct McpRuntime {
    /// 認証情報（`auth_credential`）の暗号化に使う（未設定＝暗号鍵無しなら認証情報付き登録は 500 に縮退）。
    crypto: Option<Arc<SystemCrypto>>,
    /// 外部 MCP サーバーへの HTTP クライアント（既定は [`NullMcpClient`]）。
    client: Arc<dyn McpClient>,
    /// ダッシュボード SPA の API 中継を認証する proxyToken ストア（web 再起動を跨いで保持）。
    tokens: Arc<ProxyTokenManager>,
}

impl McpRuntime {
    /// 実行時依存を束ねる。
    #[must_use]
    pub fn new(
        crypto: Option<Arc<SystemCrypto>>,
        client: Arc<dyn McpClient>,
        tokens: Arc<ProxyTokenManager>,
    ) -> Self {
        Self {
            crypto,
            client,
            tokens,
        }
    }
}

/// MCP ルータ（`AppState` 上でマージされる）。ルート固有依存を `Extension` で載せる。
pub fn routes(runtime: Arc<McpRuntime>) -> Router<AppState> {
    Router::new()
        .route("/api/mcp-servers", get(routes::list))
        .route("/api/mcp-servers/add", post(routes::add))
        .route("/api/mcp-servers/refresh", post(routes::refresh))
        .route("/api/mcp-servers/toggle", post(routes::toggle))
        .route("/api/mcp-servers/delete", post(routes::delete))
        .route(
            "/api/mcp-servers/{id}/dashboard/status",
            get(routes::dashboard_status),
        )
        .route("/api/mcp-servers/{id}/dashboard", get(routes::dashboard))
        .route(
            "/proxy/mcp/{id}/mcp",
            options(routes::proxy_preflight).post(routes::proxy_post),
        )
        .layer(Extension(runtime))
}
