//! 公式 Rust SDK `rmcp` による MCP クライアント（`tools/list` / `tools/call` / `resources/read`・OAuth）。
//!
//! [`RmcpClient`] は [`McpClient`] ポートの実装。MCP プロトコル部分（初期化ハンドシェイク・
//! Streamable HTTP・SSE・`Mcp-Session-Id`・セッション失効時の再初期化・`tools/list_changed` 通知）と
//! OAuth 2.1 認可（[`crate::oauth`]）は rmcp に任せ、サーバー ID ごとに接続（`RunningService`）を
//! キャッシュする。初期化時に MCP Apps 拡張（`io.modelcontextprotocol/ui`）への対応を申告する。
//!
//! **認証の選び方**（サーバーごと）:
//! 1. 固定の Bearer トークンが登録されていれば、それを付ける（OAuth 非対応サーバー向け）。
//! 2. OAuth の資格情報が保存されていれば、rmcp の `AuthClient` で付ける（期限切れは自動で更新）。
//! 3. どちらも無ければ認証なしで接続する。サーバーが 401/403 で認可を求めたら、その
//!    `WWW-Authenticate` を覚えておき（[`McpClient::authorization_required`]）、管理画面から認可を始める。
//!
//! **SSRF ガード**:
//! 1. 新しく接続する前に [`assert_safe_outbound_url`] で scheme/userinfo/localhost/IP リテラル/解決先を
//!    検査する。キャッシュ済み接続を使う操作では DNS を引かない検査
//!    （[`assert_safe_outbound_url_without_dns`]）だけ行う。
//! 2. MCP・OAuth の HTTP に使う `reqwest::Client` は検証付きリゾルバ（[`GuardedResolver`]）を持つ。
//!    接続に使う IP はリゾルバが検査した結果そのものなので、検査と接続の間の DNS リバインディングを
//!    許さない。判定は他の外向き通信と同じ [`resolve_public_addrs`]（yuuka-outbound）。
//! 3. 自動リダイレクトは無効。システムプロキシも使わない（プロキシ経由だと解決が行われずリゾルバ検査を
//!    迂回できるため）。
//!
//! **再試行**: `tools/list`・`resources/read`（冪等）だけ、トランスポート断のとき再接続して 1 回再試行
//! する。`tools/call` は再試行しない（サーバーがツールを実行した後に応答ストリームが切れた場合に、
//! 副作用が二重になるため）。セッション失効は rmcp が内部で再初期化する（`reinit_on_expired_session`）。
//!
//! **live 検証は実環境へ延期**: 実 MCP サーバー・認可サーバーとの往復はユニットテスト対象外。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use rmcp::handler::client::ClientHandler;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ExtensionCapabilities, Implementation,
    PaginatedRequestParams, ReadResourceRequestParams, Tool,
};
use rmcp::service::{NotificationContext, RunningService, ServiceError};
use rmcp::transport::auth::{
    AuthClient, AuthorizationManager, AuthorizationRequest, AuthorizationSession,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError, StreamableHttpClientTransportConfig,
};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{Peer, RoleClient, ServiceExt};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use yuuka_crypto::SystemCrypto;
use yuuka_outbound::{
    assert_safe_outbound_url, assert_safe_outbound_url_without_dns, resolve_public_addrs,
};
use yuuka_web::Db;

use crate::oauth::{self, DbCredentialStore, GuardedOAuthHttp, Pending, PendingAuthorizations};
use crate::provider::tools_to_cache_json;
use crate::repo::{self, McpServerRecord};
use crate::{
    ui_meta_from_meta, AuthorizationCallback, AuthorizationStart, McpClient, McpTool,
    UI_EXTENSION_ID, UI_RESOURCE_MIME_TYPE,
};

/// 1 リクエストのタイムアウト。
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_millis(15_000);

/// 認可が必要なときの利用者向けメッセージ。
const AUTH_REQUIRED_MESSAGE: &str =
    "このMCPサーバーは認可が必要です。管理画面の「OAuthで認可」から認可してください。";

/// 接続時に DNS 解決結果を検査するリゾルバ。内部/予約レンジが 1 つでも含まれれば接続を拒否する
/// （判定はブラウザの取得・フィルタプロキシと同じ [`resolve_public_addrs`]）。
///
/// yuuka-outbound の `PublicOnlyResolver` と同じ役割だが、あちらは workspace の reqwest 0.12 用。
/// rmcp が reqwest 0.13 を要求するため、トレイト実装だけここに置く（判定ロジックは共有）。
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

/// SSRF ガード付きの HTTP クライアントを作る（MCP の通信と OAuth の発見・登録・トークン取得で共用）。
fn guarded_http() -> Result<reqwest::Client, String> {
    // reqwest は rustls-no-provider なので、プロセス既定の CryptoProvider が無いと Client の構築で
    // panic する。ring を既定にする（既に入っていれば何もしない。twilight・reqwest 0.12 も ring を使う）。
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .dns_resolver(Arc::new(GuardedResolver))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("MCP 用 HTTP クライアントを構築できませんでした: {e}"))
}

/// 初期化時に名乗るクライアント情報。MCP Apps 拡張への対応を `extensions` で申告する。
fn client_config() -> ClientConfig {
    let mut ui = serde_json::Map::new();
    ui.insert("mimeTypes".to_owned(), json!([UI_RESOURCE_MIME_TYPE]));
    let mut extensions = ExtensionCapabilities::new();
    extensions.insert(UI_EXTENSION_ID.to_owned(), ui);
    let mut capabilities = ClientCapabilities::default();
    capabilities.extensions = Some(extensions);
    ClientConfig::new(
        capabilities,
        Implementation::new("yuuka", env!("CARGO_PKG_VERSION")),
    )
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

/// rmcp の `ClientHandler`。初期化時のクライアント情報を返し、サーバーからの `tools/list_changed` を
/// 受けて `tools_cache` を更新する。
#[derive(Clone)]
struct ToolsCacheHandler {
    server_id: i64,
    db: Db,
    gate: Arc<RefreshGate>,
}

impl ClientHandler for ToolsCacheHandler {
    fn get_info(&self) -> ClientConfig {
        client_config()
    }

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

/// キャッシュ済み接続。`fingerprint` が変わった（エンドポイント/固定トークンの変更）ら作り直す。
/// OAuth の資格情報が変わったとき（認可・解除）は [`McpClient::forget`] で捨てる。
struct Connection {
    fingerprint: String,
    service: RunningService<RoleClient, ToolsCacheHandler>,
}

/// エラーの原因をたどって、サーバーが返した `WWW-Authenticate`（401/403）を探す。
fn auth_challenge_in(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut current = Some(error);
    while let Some(e) = current {
        if let Some(a) = e.downcast_ref::<AuthRequiredError>() {
            return Some(a.www_authenticate_header.clone());
        }
        if let Some(s) = e.downcast_ref::<InsufficientScopeError>() {
            return Some(s.www_authenticate_header.clone());
        }
        current = e.source();
    }
    None
}

/// rmcp ベースの [`McpClient`] 実装。
pub struct RmcpClient {
    /// SSRF ガード付き HTTP クライアント（MCP の通信と OAuth で共用）。
    http: reqwest::Client,
    /// 固定トークンの復号と OAuth 資格情報の暗号化（`None` = 暗号鍵未設定）。
    crypto: Option<Arc<SystemCrypto>>,
    db: Db,
    connections: Mutex<HashMap<i64, Arc<Connection>>>,
    /// サーバーごとの接続処理の直列化（同時呼び出しで二重に initialize しない）。
    connect_locks: std::sync::Mutex<HashMap<i64, Arc<Mutex<()>>>>,
    /// 直近にサーバーが返した認可要求（`WWW-Authenticate`）。認可の開始時に発見の起点に使う。
    challenges: std::sync::Mutex<HashMap<i64, String>>,
    /// 認可 URL を発行してコールバックを待っているセッション。
    pending: PendingAuthorizations,
}

impl RmcpClient {
    /// `crypto` は固定トークンの復号と OAuth 資格情報の暗号化に使う（`None` = 認証情報なし扱い）。
    /// `db` は `tools/list_changed` による `tools_cache` 更新と OAuth 資格情報の保存に使う。
    ///
    /// # Errors
    /// SSRF ガード付き HTTP クライアントを構築できない場合。
    pub fn new(crypto: Option<Arc<SystemCrypto>>, db: Db) -> Result<Self, String> {
        Ok(Self {
            http: guarded_http()?,
            crypto,
            db,
            connections: Mutex::new(HashMap::new()),
            connect_locks: std::sync::Mutex::new(HashMap::new()),
            challenges: std::sync::Mutex::new(HashMap::new()),
            pending: PendingAuthorizations::default(),
        })
    }

    /// 登録済みの固定トークン（`Authorization: Bearer`）を復号する。復号失敗はログして `None`。
    fn bearer_token(&self, server: &McpServerRecord) -> Option<String> {
        let (Some(enc), Some(iv), Some(tag)) = (
            server.auth_credential_encrypted.as_deref(),
            server.auth_credential_iv.as_deref(),
            server.auth_credential_tag.as_deref(),
        ) else {
            return None;
        };
        let crypto = self.crypto.as_ref()?;
        match crypto.decrypt_text(enc, iv, tag) {
            Ok(cred) => Some(cred),
            Err(e) => {
                tracing::error!(
                    server = %server.name,
                    error = %e,
                    "MCP サーバーの認証情報の復号に失敗しました（認証なしで続行）"
                );
                None
            }
        }
    }

    fn challenges(&self) -> std::sync::MutexGuard<'_, HashMap<i64, String>> {
        self.challenges
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// サーバーの認可要求を覚える（401/403 を受けたとき）。
    fn remember_challenge(&self, server_id: i64, challenge: String) {
        self.challenges().insert(server_id, challenge);
    }

    /// OAuth 用の `AuthorizationManager`（SSRF ガード付き HTTP・DB の資格情報）を作る。
    async fn auth_manager(
        &self,
        server: &McpServerRecord,
        crypto: Arc<SystemCrypto>,
    ) -> Result<AuthorizationManager, String> {
        let mut manager = AuthorizationManager::new_with_oauth_http_client(
            server.endpoint_url.as_str(),
            Arc::new(GuardedOAuthHttp {
                client: self.http.clone(),
            }),
        )
        .await
        .map_err(|e| format!("OAuth の準備に失敗しました: {e}"))?;
        manager.set_credential_store(DbCredentialStore {
            db: self.db.clone(),
            crypto,
            server_id: server.id,
        });
        Ok(manager)
    }

    /// 接続を確立する（SSRF 事前検査 → 認証方式の選択 → rmcp の initialize ハンドシェイク）。
    async fn connect(&self, server: &McpServerRecord) -> Result<Connection, String> {
        assert_safe_outbound_url(&server.endpoint_url).await?;
        let token = self.bearer_token(server);
        let fingerprint = fingerprint(server, token.as_deref());

        let config = StreamableHttpClientTransportConfig::with_uri(server.endpoint_url.as_str())
            .reinit_on_expired_session(true);
        let handler = ToolsCacheHandler {
            server_id: server.id,
            db: self.db.clone(),
            gate: Arc::new(RefreshGate::default()),
        };

        let oauth_manager = match (&token, &self.crypto) {
            (None, Some(crypto)) => {
                let stored = repo::load_oauth_credentials(&self.db, server.id)
                    .await
                    .map_err(|e| format!("OAuth 資格情報を読めませんでした: {e}"))?;
                match stored {
                    Some(_) => {
                        let mut manager = self.auth_manager(server, crypto.clone()).await?;
                        // 保存済みトークンを読み込む（発見済みメタデータが無ければここで発見する）。
                        if let Err(e) = manager.initialize_from_store().await {
                            tracing::warn!(server = %server.name, error = %e, "OAuth 資格情報の読み込みに失敗（認証なしで続行）");
                        }
                        Some(manager)
                    }
                    None => None,
                }
            }
            _ => None,
        };

        let served = if let Some(token) = token {
            let transport = StreamableHttpClientTransport::with_client(
                self.http.clone(),
                config.auth_header(token),
            );
            tokio::time::timeout(REQUEST_TIMEOUT, handler.serve(transport)).await
        } else if let Some(manager) = oauth_manager {
            let transport = StreamableHttpClientTransport::with_client(
                AuthClient::new(self.http.clone(), manager),
                config,
            );
            tokio::time::timeout(REQUEST_TIMEOUT, handler.serve(transport)).await
        } else {
            let transport = StreamableHttpClientTransport::with_client(self.http.clone(), config);
            tokio::time::timeout(REQUEST_TIMEOUT, handler.serve(transport)).await
        };
        let service = match served {
            Err(_) => return Err("MCP サーバーへの接続がタイムアウトしました".to_owned()),
            Ok(Ok(service)) => service,
            Ok(Err(e)) => {
                if let Some(challenge) = e.auth_challenge() {
                    self.remember_challenge(server.id, challenge.to_owned());
                    return Err(AUTH_REQUIRED_MESSAGE.to_owned());
                }
                if e.is_authorization_required() {
                    return Err(AUTH_REQUIRED_MESSAGE.to_owned());
                }
                return Err(format!("{e}"));
            }
        };
        self.challenges().remove(&server.id);
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
            .unwrap_or_else(PoisonError::into_inner)
            .entry(server_id)
            .or_default()
            .clone()
    }

    /// 生きている接続を返す。無い/閉じている/設定が変わっている場合は再接続する。
    async fn connection(&self, server: &McpServerRecord) -> Result<Arc<Connection>, String> {
        // 認証情報の復号と DNS を引かない SSRF 検査は毎回行う（設定変更を拾う）。解決先の検査は、
        // 新しく接続するときの connect() と、接続時の GuardedResolver が行う。
        assert_safe_outbound_url_without_dns(&server.endpoint_url)?;
        let fp = fingerprint(server, self.bearer_token(server).as_deref());
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
    /// 再試行する。`retry` は冪等な操作（`tools/list`・`resources/read`）にだけ使う。タイムアウトと
    /// サーバー報告エラーは再試行しない。認可要求（401/403）は覚えて、利用者向けの案内を返す。
    async fn run<T, F, Fut>(
        &self,
        server: &McpServerRecord,
        retry: bool,
        op: F,
    ) -> Result<T, String>
    where
        F: Fn(Arc<Connection>) -> Fut,
        Fut: std::future::Future<Output = Result<T, ServiceError>>,
    {
        let mut retried = false;
        loop {
            let conn = self.connection(server).await?;
            let result = tokio::time::timeout(REQUEST_TIMEOUT, op(conn.clone()))
                .await
                .map_err(|_| "MCP サーバーの応答がタイムアウトしました".to_owned())?;
            match result {
                Ok(v) => return Ok(v),
                Err(ServiceError::TransportSend(e)) => {
                    self.drop_connection_if_same(server.id, &conn).await;
                    if let Some(challenge) = auth_challenge_in(e.error.as_ref()) {
                        self.remember_challenge(server.id, challenge);
                        return Err(AUTH_REQUIRED_MESSAGE.to_owned());
                    }
                    if retry && !retried {
                        retried = true;
                        continue;
                    }
                    return Err(format!("{}", ServiceError::TransportSend(e)));
                }
                Err(e @ ServiceError::TransportClosed) => {
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

    /// 接続とロックを捨てる（次回利用時に作り直す）。
    async fn drop_connection(&self, server_id: i64) {
        self.connections.lock().await.remove(&server_id);
        self.connect_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&server_id);
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
            ui: t.meta.as_ref().and_then(|m| ui_meta_from_meta(&m.0)),
        })
        .collect()
}

/// ツール引数を JSON オブジェクトへ（`null` は引数なし）。
fn to_arguments(arguments: Value) -> Result<Option<serde_json::Map<String, Value>>, String> {
    match arguments {
        Value::Object(map) => Ok(Some(map)),
        Value::Null => Ok(None),
        _ => Err("ツール引数は JSON オブジェクトである必要があります".to_owned()),
    }
}

#[async_trait]
impl McpClient for RmcpClient {
    async fn refresh_tools(&self, server: &McpServerRecord) -> Result<Vec<McpTool>, String> {
        let server_id = server.id;
        let tools = self
            .run(server, true, |conn| async move {
                list_tools_capped(conn.service.peer(), server_id).await
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
        let arguments = to_arguments(arguments)?;
        // tools/call は再試行しない（実行後に応答ストリームが切れた場合に副作用が二重になるため）。
        let result = self
            .run(server, false, |conn| {
                let mut params = CallToolRequestParams::new(tool_name.to_owned());
                params.arguments = arguments.clone();
                async move { conn.service.call_tool(params).await }
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

    async fn call_tool_result(
        &self,
        server: &McpServerRecord,
        tool_name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let arguments = to_arguments(arguments)?;
        let result = self
            .run(server, false, |conn| {
                let mut params = CallToolRequestParams::new(tool_name.to_owned());
                params.arguments = arguments.clone();
                async move { conn.service.call_tool(params).await }
            })
            .await?;
        serde_json::to_value(result).map_err(|e| format!("{e}"))
    }

    async fn read_resource(&self, server: &McpServerRecord, uri: &str) -> Result<Value, String> {
        let result = self
            .run(server, true, |conn| {
                let params = ReadResourceRequestParams::new(uri.to_owned());
                async move { conn.service.read_resource(params).await }
            })
            .await?;
        serde_json::to_value(result).map_err(|e| format!("{e}"))
    }

    async fn start_authorization(
        &self,
        server: &McpServerRecord,
        start: AuthorizationStart,
    ) -> Result<String, String> {
        let Some(crypto) = self.crypto.clone() else {
            return Err(
                "暗号鍵が設定されていないため、OAuth の資格情報を保存できません。".to_owned(),
            );
        };
        if self.bearer_token(server).is_some() {
            return Err(
                "固定の認証トークンが登録されているサーバーです。OAuth を使う場合はトークンなしで登録し直してください。"
                    .to_owned(),
            );
        }
        assert_safe_outbound_url(&server.endpoint_url).await?;

        let mut manager = self.auth_manager(server, crypto).await?;
        let challenge = self.challenges().get(&server.id).cloned();
        // 保護リソースメタデータ（RFC 9728）→ 認可サーバーメタデータの順に発見する。401 の
        // WWW-Authenticate を受けていれば、その resource_metadata と scope を起点にする。
        let resolution = manager
            .resolve_metadata_from_challenge(challenge.as_deref())
            .await
            .map_err(|e| format!("認可サーバーを見つけられませんでした: {e}"))?;
        manager.set_metadata(resolution.metadata);

        let mut request = AuthorizationRequest::new(start.redirect_uri)
            .with_client_name(oauth::CLIENT_NAME)
            .with_application_type("web");
        if let Some(url) = start.client_metadata_url {
            request = request.with_client_metadata_url(url);
        }
        if let Some(client_id) = start.client_id {
            request = request.with_preregistered_client(client_id);
            if let Some(secret) = start.client_secret {
                request = request.with_client_secret(secret);
            }
        }
        if let Some(challenge) = challenge {
            request = request.with_challenge(challenge);
        }
        let session = AuthorizationSession::new(manager, request)
            .await
            .map_err(|(_, e)| format!("認可を開始できませんでした: {e}"))?;
        let url = session.get_authorization_url().to_owned();
        let state =
            oauth::state_of(&url).ok_or_else(|| "認可 URL に state がありません".to_owned())?;
        self.pending
            .insert(
                state,
                Pending {
                    server_id: server.id,
                    user_id: start.user_id,
                    created: Instant::now(),
                    session,
                },
            )
            .await;
        Ok(url)
    }

    async fn complete_authorization(&self, callback: AuthorizationCallback) -> Result<i64, String> {
        let pending = self.pending.take(&callback.state).await.ok_or_else(|| {
            "認可の有効期限が切れたか、不明な要求です。もう一度やり直してください。".to_owned()
        })?;
        if pending.user_id != callback.user_id {
            return Err("認可を始めたユーザーと異なるため、受け付けられません。".to_owned());
        }
        pending
            .session
            .handle_callback_with_issuer(
                &callback.code,
                &callback.state,
                callback.issuer.as_deref(),
            )
            .await
            .map_err(|e| format!("トークンを取得できませんでした: {e}"))?;
        // 新しい資格情報で接続し直す。
        self.challenges().remove(&pending.server_id);
        self.drop_connection(pending.server_id).await;
        Ok(pending.server_id)
    }

    async fn authorization_required(&self, server_id: i64) -> bool {
        self.challenges().contains_key(&server_id)
    }

    async fn forget(&self, server_id: i64) {
        self.drop_connection(server_id).await;
        self.challenges().remove(&server_id);
        self.pending.forget_server(server_id).await;
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

    /// reqwest は rustls-no-provider。provider を入れてから構築するので panic しない。
    #[test]
    fn guarded_http_has_ring_provider() {
        assert!(guarded_http().is_ok());
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }

    /// 初期化で MCP Apps 拡張への対応を申告する。
    #[test]
    fn client_config_advertises_mcp_apps() {
        let v = serde_json::to_value(client_config()).expect("json");
        assert_eq!(
            v["capabilities"]["extensions"][UI_EXTENSION_ID]["mimeTypes"][0],
            UI_RESOURCE_MIME_TYPE
        );
        assert_eq!(v["clientInfo"]["name"], "yuuka");
    }

    #[test]
    fn auth_challenge_is_found_in_error_chain() {
        #[derive(Debug)]
        struct Wrap(AuthRequiredError);
        impl std::fmt::Display for Wrap {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("wrap")
            }
        }
        impl std::error::Error for Wrap {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let e = Wrap(AuthRequiredError::new(
            "Bearer resource_metadata=\"https://x/.well-known/oauth-protected-resource\""
                .to_owned(),
        ));
        assert_eq!(
            auth_challenge_in(&e).as_deref(),
            Some("Bearer resource_metadata=\"https://x/.well-known/oauth-protected-resource\"")
        );
        let plain = std::io::Error::other("x");
        assert_eq!(auth_challenge_in(&plain), None);
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
