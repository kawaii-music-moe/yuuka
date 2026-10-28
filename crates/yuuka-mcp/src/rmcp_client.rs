//! 公式 Rust SDK `rmcp` による MCP クライアント（`tools/list` / `tools/call`）。
//!
//! [`RmcpClient`] は [`McpClient`] ポートの実装。MCP プロトコル部分（初期化ハンドシェイク・
//! Streamable HTTP・SSE・`Mcp-Session-Id`・セッション失効時の再初期化・`tools/list_changed` 通知）は
//! rmcp に任せ、サーバー ID ごとに接続（`RunningService`）をキャッシュする。ダッシュボード取得と
//! プロキシ中継は MCP ではない通常の HTTP なので [`DashboardHttpClient`] に委譲する。
//!
//! **SSRF ガード**（従来より強い）:
//! 1. 新しく接続する前に [`assert_safe_outbound_url`] で scheme/userinfo/localhost/IP リテラル/解決先を
//!    検査する。キャッシュ済み接続を使う操作では DNS を引かない検査
//!    （[`assert_safe_outbound_url_without_dns`]）だけ行う。
//! 2. rmcp transport へ差し込む `reqwest::Client` は検証付きリゾルバ（[`GuardedResolver`]）を持つ。
//!    接続に使う IP はリゾルバが検査した結果そのものなので、検査と接続の間の DNS リバインディングを
//!    許さない（従来の事前検査のみ方式の TOCTOU を解消）。
//! 3. 自動リダイレクトは無効（リダイレクト先で内部へ逃がさない）。システムプロキシも使わない
//!    （プロキシ経由だと解決が行われずリゾルバ検査を迂回できるため）。
//!
//! **再試行**: `tools/list`（冪等）だけ、トランスポート断のとき再接続して 1 回再試行する。`tools/call` は
//! 再試行しない（サーバーがツールを実行した後に応答ストリームが切れた場合に、副作用が二重になるため）。
//! セッション失効は rmcp が内部で再初期化する（`reinit_on_expired_session`）。
//!
//! **live 検証は実環境へ延期**: 実 MCP サーバーとの往復はユニットテスト対象外。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use reqwest13::dns::{Addrs, Name, Resolve, Resolving};
use rmcp::handler::client::ClientHandler;
use rmcp::model::{CallToolRequestParams, PaginatedRequestParams, Tool};
use rmcp::service::{NotificationContext, RunningService, ServiceError};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{Peer, RoleClient, ServiceExt};
use serde_json::Value;
use tokio::sync::Mutex;
use yuuka_crypto::SystemCrypto;
use yuuka_outbound::{
    assert_safe_outbound_url, assert_safe_outbound_url_without_dns, resolve_public_addrs,
};
use yuuka_web::Db;

use crate::http_client::{DashboardHttpClient, REQUEST_TIMEOUT};
use crate::provider::tools_to_cache_json;
use crate::repo::{self, McpServerRecord};
use crate::{McpClient, McpTool, ProxyResponse};

/// 接続時に DNS 解決結果を検査するリゾルバ。内部/予約レンジが 1 つでも含まれれば接続を拒否する
/// （判定はブラウザの取得・フィルタプロキシと同じ [`resolve_public_addrs`]）。
struct GuardedResolver;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs = resolve_public_addrs(&host, 0).await?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// SSRF ガード付きの rmcp 用 HTTP クライアントを作る。
fn build_guarded_http() -> Result<reqwest13::Client, String> {
    // reqwest13 は rustls-no-provider なので、プロセス既定の CryptoProvider が無いと Client の構築で
    // panic する。ring を既定にする（既に入っていれば何もしない。twilight・reqwest 0.12 も ring を使う）。
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest13::Client::builder()
        .dns_resolver(Arc::new(GuardedResolver))
        .redirect(reqwest13::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("MCP 用 HTTP クライアントを構築できませんでした: {e}"))
}

/// `tools/list` で取りに行くページ数と件数の上限（外部サーバーがカーソルを返し続けても止める）。
const MAX_TOOL_PAGES: usize = 20;
const MAX_TOOLS: usize = 1000;

/// `tools/list_changed` を受けてから取りに行くまでの待ち（連続した通知を 1 回にまとめる）。
const LIST_CHANGED_DEBOUNCE: Duration = Duration::from_secs(2);

/// `tools/list` をページ数・件数の上限つきで全件取る（rmcp の `list_all_tools` は上限なし）。
async fn list_tools_capped(
    peer: &Peer<RoleClient>,
    server_id: i64,
) -> Result<Vec<Tool>, ServiceError> {
    let mut tools = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_TOOL_PAGES {
        let page = peer
            .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
            .await?;
        tools.extend(page.tools);
        cursor = page.next_cursor;
        if cursor.is_none() || tools.len() >= MAX_TOOLS {
            break;
        }
    }
    if cursor.is_some() || tools.len() > MAX_TOOLS {
        tracing::warn!(
            server_id,
            "tools/list が上限（{MAX_TOOL_PAGES} ページ / {MAX_TOOLS} 件）に達したため、残りは取得しません"
        );
        tools.truncate(MAX_TOOLS);
    }
    Ok(tools)
}

/// `tools/list_changed` による再取得を、サーバーごとに同時 1 本へまとめる。
#[derive(Default)]
struct RefreshGate {
    /// 再取得タスクが動いているか。
    running: AtomicBool,
    /// まだ反映していない通知があるか。
    pending: AtomicBool,
}

impl RefreshGate {
    /// 通知を記録する。再取得タスクを新しく起動すべきなら `true`（動いていれば、そのタスクが拾う）。
    fn notify(&self) -> bool {
        self.pending.store(true, Ordering::SeqCst);
        !self.running.swap(true, Ordering::SeqCst)
    }

    /// 1 回分の取得を始める直前に呼ぶ（ここまでの通知を消化済みにする）。
    fn begin(&self) {
        self.pending.store(false, Ordering::SeqCst);
    }

    /// 取得を終えたときに呼ぶ。続けてもう 1 回取りに行くべきなら `true`。止めた直後に届いた通知を
    /// 取りこぼさず、別のタスクが既に拾っていればそちらに任せる。
    fn finish(&self) -> bool {
        self.running.store(false, Ordering::SeqCst);
        self.pending.load(Ordering::SeqCst) && !self.running.swap(true, Ordering::SeqCst)
    }
}

/// rmcp の `ClientHandler`。サーバーからの `tools/list_changed` を受けて `tools_cache` を更新する。
#[derive(Clone)]
struct ToolsCacheHandler {
    server_id: i64,
    db: Db,
    gate: Arc<RefreshGate>,
}

impl ClientHandler for ToolsCacheHandler {
    async fn on_tool_list_changed(&self, context: NotificationContext<RoleClient>) {
        // MCP サーバーは利用者が登録する外部サーバーなので、通知を連打されてもタスクを積み上げない。
        // 動いている再取得があれば、そのタスクが通知をまとめて拾う。
        if !self.gate.notify() {
            return;
        }
        // 通知ハンドラ内でリクエストを待つとメッセージループを塞ぐため、別タスクで再取得する。
        let (server_id, db, peer, gate) = (
            self.server_id,
            self.db.clone(),
            context.peer,
            self.gate.clone(),
        );
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(LIST_CHANGED_DEBOUNCE).await;
                gate.begin();
                refresh_tools_cache(&peer, server_id, &db).await;
                if !gate.finish() {
                    break;
                }
            }
        });
    }
}

/// `tools/list` を取り直して `tools_cache` に書く（`tools/list_changed` 用・失敗はログのみ）。
async fn refresh_tools_cache(peer: &Peer<RoleClient>, server_id: i64, db: &Db) {
    let tools =
        match tokio::time::timeout(REQUEST_TIMEOUT, list_tools_capped(peer, server_id)).await {
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
    if let Err(e) = repo::update_tools_cache(db, server_id, &json_str).await {
        tracing::warn!(server_id, error = %e, "MCP tools_cache の更新に失敗");
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
    /// サーバーごとの接続処理の直列化（同時呼び出しで二重に initialize しない）。
    connect_locks: std::sync::Mutex<HashMap<i64, Arc<Mutex<()>>>>,
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
            connect_locks: std::sync::Mutex::new(HashMap::new()),
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
            gate: Arc::new(RefreshGate::default()),
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

    /// キャッシュ済みで、設定が同じで、閉じていない接続。
    async fn cached(&self, server_id: i64, fp: &str) -> Option<Arc<Connection>> {
        let map = self.connections.lock().await;
        map.get(&server_id)
            .filter(|c| c.fingerprint == fp && !c.service.is_closed())
            .cloned()
    }

    /// サーバーごとの接続ロック（無ければ作る）。
    fn connect_lock(&self, server_id: i64) -> Arc<Mutex<()>> {
        self.connect_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(server_id)
            .or_default()
            .clone()
    }

    /// 生きている接続を返す。無い/閉じている/設定が変わっている場合は再接続する。
    async fn connection(&self, server: &McpServerRecord) -> Result<Arc<Connection>, String> {
        // 認証情報の復号と DNS を引かない SSRF 検査は毎回行う（設定変更を拾う）。解決先の検査は、
        // 新しく接続するときの connect() と、接続時の GuardedResolver が行う。
        assert_safe_outbound_url_without_dns(&server.endpoint_url)?;
        let fp = fingerprint(server, self.http.bearer_token(server).as_deref());
        if let Some(conn) = self.cached(server.id, &fp).await {
            return Ok(conn);
        }
        let lock = self.connect_lock(server.id);
        let _guard = lock.lock().await;
        // 待っている間に別の呼び出しが接続していれば、それを使う。
        if let Some(conn) = self.cached(server.id, &fp).await {
            return Ok(conn);
        }
        let conn = Arc::new(self.connect(server).await?);
        // 置き換えられた旧接続は Drop でキャンセルされる。
        self.connections
            .lock()
            .await
            .insert(server.id, conn.clone());
        Ok(conn)
    }

    /// `conn` がまだキャッシュに入っていれば捨てる（次回利用時に再接続）。別の呼び出しが入れた
    /// 新しい接続は消さない。
    async fn drop_connection_if_same(&self, server_id: i64, conn: &Arc<Connection>) {
        let mut map = self.connections.lock().await;
        if map.get(&server_id).is_some_and(|c| Arc::ptr_eq(c, conn)) {
            map.remove(&server_id);
        }
    }

    /// 操作を実行する。トランスポート断のときは壊れた接続を捨て、`retry` なら再接続して 1 回だけ
    /// 再試行する。`retry` は冪等な操作（`tools/list`）にだけ使う。タイムアウトとサーバー報告エラーは
    /// 再試行しない。
    async fn run<T, F, Fut>(
        &self,
        server: &McpServerRecord,
        retry: bool,
        op: F,
    ) -> Result<T, String>
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
                Err(e @ (ServiceError::TransportSend(_) | ServiceError::TransportClosed)) => {
                    self.drop_connection_if_same(server.id, &conn).await;
                    if retry && !retried {
                        retried = true;
                        continue;
                    }
                    return Err(format!("{e}"));
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
        let server_id = server.id;
        let tools = self
            .run(server, true, |peer| async move {
                list_tools_capped(&peer, server_id).await
            })
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
        // tools/call は再試行しない（実行後に応答ストリームが切れた場合に副作用が二重になるため）。
        let result = self
            .run(server, false, |peer| {
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
        self.connections.lock().await.remove(&server_id);
        self.connect_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&server_id);
        self.http.forget_session(server_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolver_rejects_loopback_and_allows_nothing_blocked() {
        // localhost は解決する前に、内部アドレスの IP リテラルは解決結果の検査で拒否する。
        let name: Name = "localhost".parse().expect("name");
        let err = GuardedResolver.resolve(name).await.err().expect("blocked");
        assert!(err.to_string().contains("ローカルホスト"));
        let name: Name = "127.0.0.1".parse().expect("name");
        let err = GuardedResolver.resolve(name).await.err().expect("blocked");
        assert!(err.to_string().contains("内部/予約済み"));
    }

    /// reqwest13 は rustls-no-provider。provider を入れてから構築するので panic しない。
    #[test]
    fn guarded_http_builds_with_ring_provider() {
        assert!(build_guarded_http().is_ok());
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }

    /// 連続した通知は 1 本のタスクにまとまり、取得中に来た通知はもう 1 回の取得で拾う。
    #[test]
    fn refresh_gate_coalesces_notifications() {
        let gate = RefreshGate::default();
        assert!(gate.notify(), "最初の通知でタスクを起動する");
        assert!(!gate.notify(), "動いている間の通知は起動しない");
        gate.begin();
        assert!(!gate.finish(), "取得前の通知は消化済み");

        assert!(gate.notify());
        gate.begin();
        assert!(!gate.notify(), "取得中の通知");
        assert!(gate.finish(), "取得中の通知があればもう 1 回取りに行く");
        gate.begin();
        assert!(!gate.finish());
        assert!(gate.notify(), "止まった後の通知は新しいタスクを起動する");
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
