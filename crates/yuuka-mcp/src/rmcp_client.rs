//! 公式 Rust SDK `rmcp` による MCP クライアント（`tools/list` / `tools/call`）。
//!
//! [`RmcpClient`] は [`McpClient`] ポートの実装。MCP プロトコル部分（初期化ハンドシェイク・
//! Streamable HTTP・SSE・`Mcp-Session-Id`・セッション失効時の再初期化・`tools/list_changed` 通知）は
//! rmcp に任せ、サーバー ID ごとに接続（`RunningService`）をキャッシュする。ダッシュボード取得と
//! プロキシ中継は MCP ではない通常の HTTP なので [`DashboardHttpClient`] に委譲する。
//!
//! **SSRF ガード**（従来より強い）:
//! 1. 接続前に [`assert_safe_outbound_url`] で scheme/userinfo/localhost/IP リテラル/解決先を検査する。
//! 2. rmcp transport へ差し込む `reqwest::Client` は検証付きリゾルバ（[`GuardedResolver`]）を持つ。
//!    接続に使う IP はリゾルバが検査した結果そのものなので、検査と接続の間の DNS リバインディングを
//!    許さない（従来の事前検査のみ方式の TOCTOU を解消）。
//! 3. 自動リダイレクトは無効（リダイレクト先で内部へ逃がさない）。システムプロキシも使わない
//!    （プロキシ経由だと解決が行われずリゾルバ検査を迂回できるため）。
//!
//! **live 検証は実環境へ延期**: 実 MCP サーバーとの往復はユニットテスト対象外。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use reqwest13::dns::{Addrs, Name, Resolve, Resolving};
use rmcp::handler::client::ClientHandler;
use rmcp::model::{CallToolRequestParams, Tool};
use rmcp::service::{NotificationContext, RunningService, ServiceError};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{Peer, RoleClient, ServiceExt};
use serde_json::Value;
use tokio::sync::Mutex;
use yuuka_crypto::SystemCrypto;
use yuuka_outbound::{assert_safe_outbound_url, is_blocked_ip};
use yuuka_web::Db;

use crate::http_client::{DashboardHttpClient, REQUEST_TIMEOUT};
use crate::provider::tools_to_cache_json;
use crate::repo::{self, McpServerRecord};
use crate::{McpClient, McpTool, ProxyResponse};

/// 接続時に DNS 解決結果を検査するリゾルバ。内部/予約レンジが 1 つでも含まれれば接続を拒否する。
struct GuardedResolver;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addrs.is_empty() {
                return Err(format!("ホスト名を解決できませんでした: {host}").into());
            }
            if let Some(blocked) = addrs.iter().find(|a| is_blocked_ip(a.ip())) {
                return Err(format!(
                    "内部/予約済みアドレスに解決されるホストへの接続は許可されていません: {host} -> {}",
                    blocked.ip()
                )
                .into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// SSRF ガード付きの rmcp 用 HTTP クライアントを作る。
fn build_guarded_http() -> Result<reqwest13::Client, String> {
    reqwest13::Client::builder()
        .dns_resolver(Arc::new(GuardedResolver))
        .redirect(reqwest13::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("MCP 用 HTTP クライアントを構築できませんでした: {e}"))
}

/// rmcp の `ClientHandler`。サーバーからの `tools/list_changed` を受けて `tools_cache` を更新する。
#[derive(Clone)]
struct ToolsCacheHandler {
    server_id: i64,
    db: Db,
}

impl ClientHandler for ToolsCacheHandler {
    async fn on_tool_list_changed(&self, context: NotificationContext<RoleClient>) {
        // 通知ハンドラ内でリクエストを待つとメッセージループを塞ぐため、別タスクで再取得する。
        let (server_id, db, peer) = (self.server_id, self.db.clone(), context.peer);
        tokio::spawn(async move {
            let tools = match tokio::time::timeout(REQUEST_TIMEOUT, peer.list_all_tools()).await {
                Ok(Ok(tools)) => tools,
                Ok(Err(e)) => {
                    tracing::warn!(server_id, error = %e, "tools/list_changed 後の tools/list に失敗");
                    return;
                }
                Err(_) => {
                    tracing::warn!(
                        server_id,
                        "tools/list_changed 後の tools/list がタイムアウト"
                    );
                    return;
                }
            };
            let json_str = tools_to_cache_json(&to_mcp_tools(&tools));
            if let Err(e) = repo::update_tools_cache(&db, server_id, &json_str).await {
                tracing::warn!(server_id, error = %e, "MCP tools_cache の更新に失敗");
            }
        });
    }
}

/// キャッシュ済み接続。`fingerprint` が変わった（エンドポイント/認証情報の変更）ら作り直す。
struct Connection {
    fingerprint: String,
    service: RunningService<RoleClient, ToolsCacheHandler>,
}

/// rmcp ベースの [`McpClient`] 実装。
pub struct RmcpClient {
    /// ダッシュボード取得・プロキシ中継（MCP 以外の HTTP）と Bearer 復号。
    http: DashboardHttpClient,
    /// rmcp transport 用の SSRF ガード付き HTTP クライアント。
    guarded: reqwest13::Client,
    db: Db,
    connections: Mutex<HashMap<i64, Arc<Connection>>>,
}

impl RmcpClient {
    /// `crypto` は認証情報復号鍵（`None` = 認証なし）。`http` はダッシュボード/プロキシ用、
    /// `db` は `tools/list_changed` による `tools_cache` 更新用。
    ///
    /// # Errors
    /// SSRF ガード付き HTTP クライアントを構築できない場合。
    pub fn new(
        crypto: Option<Arc<SystemCrypto>>,
        http: reqwest::Client,
        db: Db,
    ) -> Result<Self, String> {
        Ok(Self {
            http: DashboardHttpClient::new(crypto, http),
            guarded: build_guarded_http()?,
            db,
            connections: Mutex::new(HashMap::new()),
        })
    }

    /// 接続を確立する（SSRF 事前検査 → rmcp の initialize ハンドシェイク）。
    async fn connect(&self, server: &McpServerRecord) -> Result<Connection, String> {
        assert_safe_outbound_url(&server.endpoint_url).await?;
        let token = self.http.bearer_token(server);
        let fingerprint = fingerprint(server, token.as_deref());

        let mut config =
            StreamableHttpClientTransportConfig::with_uri(server.endpoint_url.as_str())
                .reinit_on_expired_session(true);
        if let Some(token) = token {
            config = config.auth_header(token);
        }
        let transport = StreamableHttpClientTransport::with_client(self.guarded.clone(), config);
        let handler = ToolsCacheHandler {
            server_id: server.id,
            db: self.db.clone(),
        };
        let service = tokio::time::timeout(REQUEST_TIMEOUT, handler.serve(transport))
            .await
            .map_err(|_| "MCP サーバーへの接続がタイムアウトしました".to_owned())?
            .map_err(|e| format!("{e}"))?;
        Ok(Connection {
            fingerprint,
            service,
        })
    }

    /// 生きている接続を返す。無い/閉じている/設定が変わっている場合は再接続する。
    async fn connection(&self, server: &McpServerRecord) -> Result<Arc<Connection>, String> {
        // 認証情報の復号と SSRF 検査は毎回行う（DNS 先の変化・認証情報変更を拾う）。
        assert_safe_outbound_url(&server.endpoint_url).await?;
        let fp = fingerprint(server, self.http.bearer_token(server).as_deref());
        {
            let map = self.connections.lock().await;
            if let Some(conn) = map.get(&server.id) {
                if conn.fingerprint == fp && !conn.service.is_closed() {
                    return Ok(conn.clone());
                }
            }
        }
        let conn = Arc::new(self.connect(server).await?);
        // 置き換えられた旧接続は Drop でキャンセルされる。
        self.connections
            .lock()
            .await
            .insert(server.id, conn.clone());
        Ok(conn)
    }

    /// 接続を捨てる（次回利用時に再接続）。
    async fn drop_connection(&self, server_id: i64) {
        self.connections.lock().await.remove(&server_id);
    }

    /// 操作を実行し、トランスポート断（セッション失効・切断）のときだけ再接続して 1 回リトライする。
    /// タイムアウトとサーバー報告エラーは再試行しない（副作用の二重実行を避ける）。
    async fn with_retry<T, F, Fut>(&self, server: &McpServerRecord, op: F) -> Result<T, String>
    where
        F: Fn(Peer<RoleClient>) -> Fut,
        Fut: std::future::Future<Output = Result<T, ServiceError>>,
    {
        let mut retried = false;
        loop {
            let conn = self.connection(server).await?;
            let result = tokio::time::timeout(REQUEST_TIMEOUT, op(conn.service.peer().clone()))
                .await
                .map_err(|_| "MCP サーバーの応答がタイムアウトしました".to_owned())?;
            match result {
                Ok(v) => return Ok(v),
                Err(ServiceError::TransportSend(_) | ServiceError::TransportClosed) if !retried => {
                    retried = true;
                    self.drop_connection(server.id).await;
                }
                Err(e) => return Err(format!("{e}")),
            }
        }
    }
}

fn fingerprint(server: &McpServerRecord, token: Option<&str>) -> String {
    format!("{}\0{}", server.endpoint_url, token.unwrap_or_default())
}

fn to_mcp_tools(tools: &[Tool]) -> Vec<McpTool> {
    tools
        .iter()
        .map(|t| McpTool {
            name: t.name.to_string(),
            description: t.description.as_ref().map(ToString::to_string),
            input_schema: Some(Value::Object((*t.input_schema).clone())),
        })
        .collect()
}

#[async_trait]
impl McpClient for RmcpClient {
    async fn probe_dashboard(&self, server: &McpServerRecord) -> bool {
        self.http.probe_dashboard(server).await
    }

    async fn fetch_dashboard_html(
        &self,
        server: &McpServerRecord,
    ) -> Result<(u16, String), String> {
        self.http.fetch_dashboard_html(server).await
    }

    async fn refresh_tools(&self, server: &McpServerRecord) -> Result<Vec<McpTool>, String> {
        let tools = self
            .with_retry(server, |peer| async move { peer.list_all_tools().await })
            .await?;
        Ok(to_mcp_tools(&tools))
    }

    async fn call_tool(
        &self,
        server: &McpServerRecord,
        tool_name: &str,
        arguments: Value,
    ) -> Result<String, String> {
        let arguments = match arguments {
            Value::Object(map) => Some(map),
            Value::Null => None,
            _ => return Err("ツール引数は JSON オブジェクトである必要があります".to_owned()),
        };
        let result = self
            .with_retry(server, |peer| {
                let mut params = CallToolRequestParams::new(tool_name.to_owned());
                params.arguments = arguments.clone();
                async move { peer.call_tool(params).await }
            })
            .await?;
        let text = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        if result.is_error == Some(true) {
            Err(if text.is_empty() {
                "ツールがエラーを返しました".to_owned()
            } else {
                text
            })
        } else {
            Ok(text)
        }
    }

    async fn proxy(
        &self,
        server: &McpServerRecord,
        body: Bytes,
        accept: Option<&str>,
    ) -> Result<ProxyResponse, String> {
        self.http.proxy(server, body, accept).await
    }

    async fn forget(&self, server_id: i64) {
        self.drop_connection(server_id).await;
        self.http.forget_session(server_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolver_rejects_loopback_and_allows_nothing_blocked() {
        // localhost は 127.0.0.1/::1 に解決されるためリゾルバが拒否する。
        let name: Name = "localhost".parse().expect("name");
        let err = GuardedResolver.resolve(name).await.err().expect("blocked");
        assert!(err.to_string().contains("内部/予約済み"));
    }

    #[test]
    fn fingerprint_changes_with_endpoint_or_token() {
        let s = |url: &str| McpServerRecord {
            id: 1,
            user_id: None,
            name: "n".to_owned(),
            endpoint_url: url.to_owned(),
            auth_credential_encrypted: None,
            auth_credential_iv: None,
            auth_credential_tag: None,
            tools_cache: "[]".to_owned(),
            tools_cache_updated: None,
            requires_confirmation: 1,
            enabled: 1,
            created_at: "now".to_owned(),
        };
        assert_ne!(
            fingerprint(&s("https://a/mcp"), None),
            fingerprint(&s("https://b/mcp"), None)
        );
        assert_ne!(
            fingerprint(&s("https://a/mcp"), None),
            fingerprint(&s("https://a/mcp"), Some("t"))
        );
    }
}
