//! yuuka-mcp — 外部 MCP サーバーの管理 API（`/api/mcp-servers*`）と MCP クライアント。
//!
//! MCP の通信はすべて標準仕様どおりに行う（docs/design/mcp_standard_protocol.md）。
//!
//! - **プロトコル**: 公式 Rust SDK `rmcp` の Streamable HTTP クライアント（[`RmcpClient`]）。
//! - **認可**: MCP 仕様の Authorization（OAuth 2.1・保護リソースメタデータ・PKCE・Client ID Metadata
//!   Document / 動的クライアント登録）。rmcp の OAuth 実装を使い、資格情報は DB に暗号化して保存する。
//!   固定の Bearer トークンも引き続き登録できる（OAuth に対応していないサーバー向け）。
//! - **画面**: MCP Apps 拡張（`io.modelcontextprotocol/ui`）。ツールの `_meta.ui.resourceUri` が指す
//!   `ui://` リソースを、管理画面のサンドボックス iframe に表示する。画面からの `tools/call`・
//!   `resources/read` はホスト（このクレートの API）が代わりに実行する。
//!
//! 認可は yuuka-web の [`AuthenticatedUser`](yuuka_web::AuthenticatedUser) extractor で型強制する。
//! ルート固有の実行時依存（保存時暗号・MCP クライアント・サンドボックス設定）は [`McpRuntime`] に
//! まとめ `Extension` レイヤで各ハンドラへ注入する（`SettingsRuntime`/`AdminRuntime` と同方式）。
//!
//! **外部 MCP サーバーへの通信シーム**: [`McpClient`] ポート越しに委譲し、未配線時は
//! [`NullMcpClient`]（正直な縮退＝取得・実行・認可は失敗）へ落とす。DB 効果（登録・削除・有効/無効・
//! 監査）は常に完全に働く。

use std::sync::Arc;

use async_trait::async_trait;
use axum::routing::{get, post};
use axum::{Extension, Router};
use serde_json::{Map, Value};
use yuuka_crypto::SystemCrypto;
use yuuka_web::AppState;

mod oauth;
pub mod provider;
pub mod repo;
mod rmcp_client;
mod routes;
mod sandbox;

pub use provider::{mcp_function_name, McpProvider, TOOLS_CACHE_TTL};
pub use repo::McpServerRecord;
pub use rmcp_client::RmcpClient;
pub use sandbox::{sandbox_host, sandbox_host_guard, SANDBOX_PATH};

/// MCP Apps の拡張 ID（クライアント能力 `extensions` のキー）。
pub const UI_EXTENSION_ID: &str = "io.modelcontextprotocol/ui";
/// MCP Apps の UI リソースの MIME タイプ。
pub const UI_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

// ─── ツール定義 ───────────────────────────────────────────────────────────────

/// 外部 MCP サーバーが公開する 1 ツール（`tools/list` 由来）。
#[derive(Debug, Clone, Default)]
pub struct McpTool {
    /// ツール名。
    pub name: String,
    /// 説明（省略可）。
    pub description: Option<String>,
    /// 入力パラメータの JSON Schema（`inputSchema`・省略可）。FC ループの `parametersJsonSchema` に使う。
    pub input_schema: Option<Value>,
    /// MCP Apps の `_meta.ui`（`resourceUri`・`visibility` など・省略可）。
    pub ui: Option<Value>,
}

impl McpTool {
    /// 紐づく UI リソースの URI（`ui://...`）。無ければ `None`。
    #[must_use]
    pub fn ui_resource_uri(&self) -> Option<&str> {
        self.ui
            .as_ref()?
            .get("resourceUri")?
            .as_str()
            .filter(|u| u.starts_with("ui://"))
    }

    /// `_meta.ui.visibility` に `who`（`"model"` / `"app"`）が含まれるか。省略時は両方に公開。
    fn visible_to(&self, who: &str) -> bool {
        match self.ui.as_ref().and_then(|ui| ui.get("visibility")) {
            Some(Value::Array(v)) => v.iter().any(|x| x.as_str() == Some(who)),
            _ => true,
        }
    }

    /// AI（Gemini の Function Calling）に見せてよいか。`visibility` に `"model"` が無いツールは
    /// 画面専用なので出さない（MCP Apps 仕様の MUST）。
    #[must_use]
    pub fn visible_to_model(&self) -> bool {
        self.visible_to("model")
    }

    /// 画面（MCP Apps のビュー）から呼んでよいか。`visibility` に `"app"` が無いツールは拒否する。
    #[must_use]
    pub fn callable_from_app(&self) -> bool {
        self.visible_to("app")
    }
}

/// ツールの `_meta` から MCP Apps の `ui` オブジェクトを取り出す。
///
/// 正式な `_meta.ui`（オブジェクト）を優先し、無ければ廃止予定の平坦なキー `_meta["ui/resourceUri"]`
/// を `{"resourceUri": ...}` として読む。どちらも無ければ `None`。
#[must_use]
pub fn ui_meta_from_meta(meta: &Map<String, Value>) -> Option<Value> {
    if let Some(ui) = meta.get("ui").filter(|v| v.is_object()) {
        return Some(ui.clone());
    }
    let uri = meta.get("ui/resourceUri")?.as_str()?;
    let mut ui = Map::new();
    ui.insert("resourceUri".to_owned(), Value::String(uri.to_owned()));
    Some(Value::Object(ui))
}

// ─── OAuth 認可の入出力 ───────────────────────────────────────────────────────

/// OAuth 認可を始めるための設定（ホスト側の URL と、任意の事前登録クライアント）。
#[derive(Debug, Clone)]
pub struct AuthorizationStart {
    /// 認可を始めたユーザー（コールバックで同一人物か確かめる）。
    pub user_id: String,
    /// コールバック URL（`{BASE_URL}/api/mcp-servers/oauth/callback`）。
    pub redirect_uri: String,
    /// Client ID Metadata Document の URL（HTTPS で公開しているときだけ）。
    pub client_metadata_url: Option<String>,
    /// 認可サーバーに事前登録したクライアント ID（任意。あれば最優先で使う）。
    pub client_id: Option<String>,
    /// 事前登録クライアントのシークレット（任意）。
    pub client_secret: Option<String>,
}

/// 認可サーバーからのコールバック（`?code=...&state=...&iss=...`）。
#[derive(Debug, Clone)]
pub struct AuthorizationCallback {
    /// 認可コード。
    pub code: String,
    /// CSRF 対策の `state`。
    pub state: String,
    /// RFC 9207 の発行者（任意）。
    pub issuer: Option<String>,
    /// コールバックを受けたユーザー（開始したユーザーと一致する必要がある）。
    pub user_id: String,
}

// ─── 外部 MCP サーバーへの通信シーム ───────────────────────────────────────────

/// 外部 MCP サーバーへのアクセスポート。未配線時は [`NullMcpClient`]。
///
/// 実装は利用直前の SSRF 再検証・タイムアウト・認証情報（Bearer または OAuth）の付与を担う。
/// ここに切り出すことで管理系ルートの DB 効果を外部依存なしにテストできる。
#[async_trait]
pub trait McpClient: Send + Sync {
    /// `tools/list` を取得する。
    ///
    /// # Errors
    /// 到達不可・認可が必要・プロトコルエラー等で `Err(message)`。
    async fn refresh_tools(&self, server: &repo::McpServerRecord) -> Result<Vec<McpTool>, String>;

    /// `tools/call` を実行して結果テキスト（`content` の text 部分を連結）を返す（FC ループ用）。
    ///
    /// ツール側エラー（`isError:true`）・トランスポート断とも再試行しない（副作用の二重実行を避ける）。
    ///
    /// # Errors
    /// 到達不可・タイムアウト・RPC エラー・ツールエラー（`isError`）等で `Err(message)`。
    async fn call_tool(
        &self,
        server: &repo::McpServerRecord,
        tool_name: &str,
        arguments: Value,
    ) -> Result<String, String>;

    /// `tools/call` を実行して `CallToolResult` をそのまま（JSON で）返す（MCP Apps の画面へ渡す用）。
    /// `isError:true` も成功として返す（画面側で表示する）。
    ///
    /// # Errors
    /// 到達不可・タイムアウト・RPC エラー等で `Err(message)`。
    async fn call_tool_result(
        &self,
        server: &repo::McpServerRecord,
        tool_name: &str,
        arguments: Value,
    ) -> Result<Value, String>;

    /// `resources/read` を実行して `ReadResourceResult` を JSON で返す（MCP Apps の UI リソース取得用）。
    ///
    /// # Errors
    /// 到達不可・タイムアウト・RPC エラー等で `Err(message)`。
    async fn read_resource(
        &self,
        server: &repo::McpServerRecord,
        uri: &str,
    ) -> Result<Value, String>;

    /// OAuth 認可を始め、利用者を送る認可 URL を返す（保護リソースメタデータの発見 → クライアント登録 →
    /// PKCE 付き認可 URL の生成）。
    ///
    /// # Errors
    /// 発見・登録の失敗、暗号鍵未設定等で `Err(message)`。
    async fn start_authorization(
        &self,
        server: &repo::McpServerRecord,
        start: AuthorizationStart,
    ) -> Result<String, String>;

    /// 認可コードをトークンへ交換して保存する。成功したらそのサーバー ID を返す。
    ///
    /// # Errors
    /// `state` 不明・期限切れ・別ユーザー・交換失敗等で `Err(message)`。
    async fn complete_authorization(&self, callback: AuthorizationCallback) -> Result<i64, String>;

    /// 直近の接続でサーバーが認可を求めたか（401/403 の `WWW-Authenticate` を受けたか）。
    async fn authorization_required(&self, _server_id: i64) -> bool {
        false
    }

    /// サーバーの無効化・削除・認証情報の変更時に、保持している接続/セッションを破棄する。
    async fn forget(&self, _server_id: i64) {}
}

/// 外部 MCP サーバー未配線時の縮退実装（正直な縮退・no live 効果）。
pub struct NullMcpClient;

const NOT_WIRED: &str = "MCP クライアントが配線されていません";

#[async_trait]
impl McpClient for NullMcpClient {
    async fn refresh_tools(&self, _server: &repo::McpServerRecord) -> Result<Vec<McpTool>, String> {
        Err(NOT_WIRED.to_owned())
    }

    async fn call_tool(
        &self,
        _server: &repo::McpServerRecord,
        _tool_name: &str,
        _arguments: Value,
    ) -> Result<String, String> {
        Err(NOT_WIRED.to_owned())
    }

    async fn call_tool_result(
        &self,
        _server: &repo::McpServerRecord,
        _tool_name: &str,
        _arguments: Value,
    ) -> Result<Value, String> {
        Err(NOT_WIRED.to_owned())
    }

    async fn read_resource(
        &self,
        _server: &repo::McpServerRecord,
        _uri: &str,
    ) -> Result<Value, String> {
        Err(NOT_WIRED.to_owned())
    }

    async fn start_authorization(
        &self,
        _server: &repo::McpServerRecord,
        _start: AuthorizationStart,
    ) -> Result<String, String> {
        Err(NOT_WIRED.to_owned())
    }

    async fn complete_authorization(
        &self,
        _callback: AuthorizationCallback,
    ) -> Result<i64, String> {
        Err(NOT_WIRED.to_owned())
    }
}

// ─── ランタイム + ルータ ──────────────────────────────────────────────────────

/// MCP ルートが使う実行時依存（`Extension` で各ハンドラへ注入）。
pub struct McpRuntime {
    /// 認証情報（`auth_credential`）の暗号化に使う（未設定＝暗号鍵無しなら認証情報付き登録は 500 に縮退）。
    crypto: Option<Arc<SystemCrypto>>,
    /// 外部 MCP サーバーへのクライアント（既定は [`NullMcpClient`]）。
    client: Arc<dyn McpClient>,
}

impl McpRuntime {
    /// 実行時依存を束ねる。
    #[must_use]
    pub fn new(crypto: Option<Arc<SystemCrypto>>, client: Arc<dyn McpClient>) -> Self {
        Self { crypto, client }
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
        // OAuth 2.1 認可（MCP 仕様 Authorization）。
        .route(
            "/api/mcp-servers/{id}/oauth/start",
            post(routes::oauth_start),
        )
        .route(
            "/api/mcp-servers/{id}/oauth/disconnect",
            post(routes::oauth_disconnect),
        )
        .route(
            "/api/mcp-servers/oauth/callback",
            get(routes::oauth_callback),
        )
        .route(
            oauth::CLIENT_METADATA_PATH,
            get(routes::oauth_client_metadata),
        )
        // MCP Apps（UI リソースの取得と、画面からのツール実行・リソース読み取りの中継）。
        .route(
            "/api/mcp-servers/{id}/apps/resource",
            post(routes::apps_resource),
        )
        .route("/api/mcp-servers/{id}/apps/call", post(routes::apps_call))
        .route(SANDBOX_PATH, get(sandbox::sandbox_page))
        .layer(Extension(runtime))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool(ui: Option<Value>) -> McpTool {
        McpTool {
            name: "t".to_owned(),
            ui,
            ..McpTool::default()
        }
    }

    #[test]
    fn ui_resource_uri_requires_ui_scheme() {
        assert_eq!(tool(None).ui_resource_uri(), None);
        assert_eq!(
            tool(Some(json!({ "resourceUri": "ui://srv/dash" }))).ui_resource_uri(),
            Some("ui://srv/dash")
        );
        assert_eq!(
            tool(Some(json!({ "resourceUri": "https://evil/x" }))).ui_resource_uri(),
            None
        );
    }

    #[test]
    fn visibility_defaults_to_both() {
        let t = tool(None);
        assert!(t.visible_to_model() && t.callable_from_app());
        let app_only = tool(Some(json!({ "visibility": ["app"] })));
        assert!(!app_only.visible_to_model() && app_only.callable_from_app());
        let model_only = tool(Some(json!({ "visibility": ["model"] })));
        assert!(model_only.visible_to_model() && !model_only.callable_from_app());
    }

    #[test]
    fn ui_meta_prefers_object_and_reads_legacy_key() {
        let meta = json!({ "ui": { "resourceUri": "ui://a" }, "ui/resourceUri": "ui://b" });
        assert_eq!(
            ui_meta_from_meta(meta.as_object().unwrap()),
            Some(json!({ "resourceUri": "ui://a" }))
        );
        let legacy = json!({ "ui/resourceUri": "ui://b" });
        assert_eq!(
            ui_meta_from_meta(legacy.as_object().unwrap()),
            Some(json!({ "resourceUri": "ui://b" }))
        );
        assert_eq!(ui_meta_from_meta(&Map::new()), None);
    }
}
