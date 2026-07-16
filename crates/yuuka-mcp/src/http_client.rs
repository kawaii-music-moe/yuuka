//! 実 MCP HTTP クライアント（Node `src/services/mcpClient.ts` パリティ）。
//!
//! [`HttpMcpClient`] は [`McpClient`](crate::McpClient) ポートを `reqwest` 背後で実装する。
//! MCP Streamable HTTP トランスポート（JSON-RPC 2.0 over HTTP POST）の最小実装で、
//! `tools/list` 取得（`initialize` → `notifications/initialized` → `tools/list`）・ダッシュボード
//! 判定/取得・エンドポイント中継を担う。応答が SSE（`text/event-stream`）のときは `data:` 行から
//! JSON-RPC を抽出し、`Mcp-Session-Id` 応答ヘッダをサーバー単位でプロセス内保持（次リクエストへ透過）
//! する。セッション由来エラー（4xx/session）は一度だけ再初期化して再試行する。
//!
//! **SSRF ガード**: すべての外向き取得（refresh/probe/fetch/proxy）の直前に宛先 URL を再検証する
//! （DNS リバインディング含む内部/予約レンジ到達を遮断）。`yuuka-browser` の同名ガードは module が
//! `pub` で公開されておらず（`mod ssrf;`・`assert_safe_outbound_url` 未 re-export）本 crate から参照
//! できないため、Node `assertSafeOutboundUrl` パリティの最小実装を本モジュール内に持つ。
//!
//! **認証情報の復号**: `auth_credential_encrypted/iv/tag` を [`SystemCrypto::decrypt_text`] で復号し
//! `Authorization: Bearer {cred}` を付ける。復号失敗はログして認証なしで続行する（Node パリティ）。
//!
//! **live 検証は実環境へ延期**: 実 MCP サーバーへの HTTP（tools/list 往復・SSE ストリーム・
//! ダッシュボード HTTP・プロキシ中継・DNS 解決を伴う SSRF）はネットワーク到達を要するため、ここでは
//! ユニットテスト対象外（実環境疎通で検証）。ユニットテストは純ロジック
//! （SSE `data:` パース→JSON-RPC 抽出・`tools/list` 結果パース・origin 抽出・session-id セッション
//! エラー判定・SSRF の IP レンジ判定）に限定する。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use serde_json::{json, Value};
use yuuka_crypto::SystemCrypto;

use crate::repo::McpServerRecord;
use crate::{McpClient, McpTool, ProxyResponse};

/// MCP プロトコルバージョン（Node `PROTOCOL_VERSION`）。
const PROTOCOL_VERSION: &str = "2025-03-26";
/// 1 リクエストのタイムアウト（Node `REQUEST_TIMEOUT_MS`）。
const REQUEST_TIMEOUT: Duration = Duration::from_millis(15_000);
/// ダッシュボード有効判定パス（Node `DASHBOARD_ENABLE_PATH`）。
const DASHBOARD_ENABLE_PATH: &str = "/dashboard/enable";
/// ダッシュボード本体パス（Node `DASHBOARD_PATH`）。
const DASHBOARD_PATH: &str = "/dashboard";
/// プロキシ既定 Accept（Node の proxy 既定）。
const DEFAULT_ACCEPT: &str = "application/json, text/event-stream";

/// 実 MCP HTTP クライアント（`reqwest` 背後で Node `mcpClient.ts` を移植）。
///
/// `crypto` が `None` のときは認証情報の復号を行わない（Bearer 非注入）。`sessions` は
/// サーバー ID → `Mcp-Session-Id` のプロセス内キャッシュ、`initialized` は初期化済みサーバー ID 集合。
pub struct HttpMcpClient {
    /// 認証情報（`auth_credential`）の復号鍵（`None` なら認証なしで続行）。
    crypto: Option<Arc<SystemCrypto>>,
    /// 外部 MCP サーバーへの HTTP クライアント（main.rs で構築して注入）。
    http: reqwest::Client,
    /// サーバー ID → `Mcp-Session-Id`（initialize 応答で発行された場合のみ保持）。
    sessions: Mutex<HashMap<i64, String>>,
    /// 初期化済みサーバー ID 集合（Node `initializedServers`）。
    initialized: Mutex<std::collections::HashSet<i64>>,
}

impl HttpMcpClient {
    /// 実クライアントを構築する。`crypto` は認証情報復号鍵（`None` = 認証なし・main.rs で配線）。
    #[must_use]
    pub fn new(crypto: Option<Arc<SystemCrypto>>, http: reqwest::Client) -> Self {
        Self {
            crypto,
            http,
            sessions: Mutex::new(HashMap::new()),
            initialized: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// キャッシュ済み session-id を返す（無ければ `None`）。
    fn session_id(&self, server_id: i64) -> Option<String> {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&server_id)
            .cloned()
    }

    /// session-id をキャッシュへ保存する（応答ヘッダ由来）。
    fn store_session_id(&self, server_id: i64, sid: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(server_id, sid.to_owned());
    }

    /// session-id と初期化済みフラグを破棄する（セッションエラー時の再初期化前）。
    fn forget_session(&self, server_id: i64) {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&server_id);
        self.initialized
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&server_id);
    }

    /// 初期化済みか。
    fn is_initialized(&self, server_id: i64) -> bool {
        self.initialized
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&server_id)
    }

    /// 初期化済みフラグを立てる。
    fn mark_initialized(&self, server_id: i64) {
        self.initialized
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(server_id);
    }

    /// サーバーの `Authorization: Bearer` 値を組み立てる（Node `buildAuthHeader`）。
    /// 復号失敗はログして `None`（認証なしで続行）。暗号鍵未設定/認証情報無しも `None`。
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

    /// 1 回の JSON-RPC リクエストを送る（Node `rpcRequest`）。`is_notification` のときは応答ボディ非期待。
    ///
    /// 成功時は解析済み JSON-RPC 応答（通知は `None`）。上流エラー・SSRF・接続失敗・RPC error は `Err`。
    async fn rpc_request(
        &self,
        server: &McpServerRecord,
        method: &str,
        params: Option<Value>,
        is_notification: bool,
        request_id: Option<i64>,
    ) -> Result<Option<Value>, String> {
        // 送信ペイロード（Node: params は指定時のみ・id は非通知時のみ）。
        let mut payload = serde_json::Map::new();
        payload.insert("jsonrpc".to_owned(), json!("2.0"));
        payload.insert("method".to_owned(), json!(method));
        if let Some(p) = params {
            payload.insert("params".to_owned(), p);
        }
        if let Some(id) = request_id {
            payload.insert("id".to_owned(), json!(id));
        }

        // SSRF 対策: 利用直前に宛先を再検証（DNS リバインディング含む内部到達を遮断）。
        assert_safe_outbound_url(&server.endpoint_url).await?;

        let mut req = self
            .http
            .post(&server.endpoint_url)
            .timeout(REQUEST_TIMEOUT)
            .header("Content-Type", "application/json")
            .header("Accept", DEFAULT_ACCEPT)
            .header("MCP-Protocol-Version", PROTOCOL_VERSION);
        if let Some(token) = self.bearer_token(server) {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(sid) = self.session_id(server.id) {
            req = req.header("Mcp-Session-Id", sid);
        }
        let resp = req
            .json(&Value::Object(payload))
            .send()
            .await
            .map_err(|e| format!("{e}"))?;

        // セッション ID の保持（initialize 応答で発行される場合がある）。
        if let Some(sid) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
        {
            self.store_session_id(server.id, sid);
        }

        if is_notification {
            return Ok(None); // 通知は応答ボディを期待しない（202 等）。
        }

        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(200).collect();
            return Err(format!("HTTP {}: {snippet}", status.as_u16()));
        }

        let body_text = resp.text().await.map_err(|e| format!("{e}"))?;

        let rpc = if content_type.contains("text/event-stream") {
            parse_sse_body(&body_text, request_id)
        } else if body_text.is_empty() {
            None
        } else {
            serde_json::from_str::<Value>(&body_text)
                .map_err(|e| format!("JSON パースに失敗しました: {e}"))
                .map(Some)?
        };

        // RPC error は Err に写像（Node `MCP error {code}: {message}`）。
        if let Some(v) = rpc.as_ref() {
            if let Some(err) = v.get("error") {
                let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                let message = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return Err(format!("MCP error {code}: {message}"));
            }
        }
        Ok(rpc)
    }

    /// initialize ハンドシェイクを実行する（未初期化のときのみ・Node `ensureInitialized`）。
    ///
    /// `notifications/initialized` の失敗は致命的でないため握る（Node と同じ）。
    async fn ensure_initialized(&self, server: &McpServerRecord) -> Result<(), String> {
        if self.is_initialized(server.id) {
            return Ok(());
        }
        let id = self.next_request_id();
        self.rpc_request(
            server,
            "initialize",
            Some(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "yuuka", "version": "2.0" },
            })),
            false,
            Some(id),
        )
        .await?;
        // initialized 通知（失敗は握る）。
        let _ = self
            .rpc_request(server, "notifications/initialized", Some(json!({})), true, None)
            .await;
        self.mark_initialized(server.id);
        Ok(())
    }

    /// リクエスト ID を採番する（プロセス単調増加・Node `nextRequestId++`）。
    fn next_request_id(&self) -> i64 {
        use std::sync::atomic::{AtomicI64, Ordering};
        static NEXT: AtomicI64 = AtomicI64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    /// `tools/list` を 1 回だけ実行して結果を整形する（`refresh_tools` の 1 試行分）。
    async fn tools_list_once(&self, server: &McpServerRecord) -> Result<Vec<McpTool>, String> {
        let id = self.next_request_id();
        let rpc = self
            .rpc_request(server, "tools/list", Some(json!({})), false, Some(id))
            .await?;
        Ok(parse_tools_result(rpc.as_ref()))
    }

    /// Bearer 付き GET を送る（Node `authedGet`）。SSRF 再検証・タイムアウト付き。
    async fn authed_get(&self, server: &McpServerRecord, url: &str) -> Result<reqwest::Response, String> {
        assert_safe_outbound_url(url).await?;
        let mut req = self.http.get(url).timeout(REQUEST_TIMEOUT);
        if let Some(token) = self.bearer_token(server) {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        req.send().await.map_err(|e| format!("{e}"))
    }
}

/// SSE 応答ボディから当該リクエストの JSON-RPC 応答を抽出する（Node `parseSseBody`）。
///
/// `expected_id` 指定時は id 一致応答を優先（バッチ/サーバー発リクエストとの取り違え防止）、
/// 無ければ最後の result/error 応答へフォールバックする。通知・サーバー発リクエストはスキップ。
fn parse_sse_body(body: &str, expected_id: Option<i64>) -> Option<Value> {
    let mut last: Option<Value> = None;
    let mut matched: Option<Value> = None;
    for raw_line in body.split('\n') {
        let line = raw_line.trim();
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        let data = rest.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<Value>(data) else {
            continue; // 部分的な data 行は無視。
        };
        // 応答（result/error 持ち）のみ対象。
        if parsed.get("result").is_some() || parsed.get("error").is_some() {
            if let Some(want) = expected_id {
                if parsed.get("id").and_then(Value::as_i64) == Some(want) {
                    matched = Some(parsed.clone());
                }
            }
            last = Some(parsed);
        }
    }
    matched.or(last)
}

/// JSON-RPC `tools/list` の `result.tools[]` を [`McpTool`] へ整形する（Node `listTools` の map）。
///
/// 非 object 要素・name 空は落とす。description は文字列のときのみ採用（Node `t.description ? String(...) : undefined`）。
fn parse_tools_result(rpc: Option<&Value>) -> Vec<McpTool> {
    let Some(Value::Array(items)) = rpc.and_then(|v| v.get("result")).and_then(|r| r.get("tools"))
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|t| {
            let obj = t.as_object()?;
            let name = obj.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            let description = obj
                .get("description")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            Some(McpTool {
                name: name.to_owned(),
                description,
            })
        })
        .collect()
}

/// セッション未確立/失効由来のエラーか（Node `isSessionError`）。
///
/// `HTTP 400`/`HTTP 404` または `session`/`not initialized`/`initializ` を含むメッセージのみ真。
/// タイムアウト・ネットワーク断・ツールエラーは含めない（副作用のある呼び出しを二重実行しないため）。
fn is_session_error(msg: &str) -> bool {
    let has_http_40x = msg.contains("HTTP 400") || msg.contains("HTTP 404");
    let lower = msg.to_ascii_lowercase();
    has_http_40x
        || lower.contains("session")
        || lower.contains("not initialized")
        || lower.contains("initializ")
}

/// `endpoint_url` の origin（`scheme://host[:port]`）を返す（Node `mcpOrigin` = `new URL(...).origin`）。
///
/// # Errors
/// URL パース不能時に日本語メッセージ。
fn mcp_origin(endpoint_url: &str) -> Result<String, String> {
    let url = url::Url::parse(endpoint_url).map_err(|_| "URLの形式が不正です。".to_owned())?;
    let scheme = url.scheme();
    let Some(host) = url.host_str() else {
        return Err("URLの形式が不正です。".to_owned());
    };
    match url.port() {
        Some(port) => Ok(format!("{scheme}://{host}:{port}")),
        None => Ok(format!("{scheme}://{host}")),
    }
}

#[async_trait]
impl McpClient for HttpMcpClient {
    async fn probe_dashboard(&self, server: &McpServerRecord) -> bool {
        let Ok(origin) = mcp_origin(&server.endpoint_url) else {
            return false;
        };
        let url = format!("{origin}{DASHBOARD_ENABLE_PATH}");
        // 到達不可・非 200 は false（Node は catch で false）。
        match self.authed_get(server, &url).await {
            Ok(res) => res.status().as_u16() == 200,
            Err(_) => false,
        }
    }

    async fn fetch_dashboard_html(&self, server: &McpServerRecord) -> Result<(u16, String), String> {
        let origin = mcp_origin(&server.endpoint_url)?;
        let url = format!("{origin}{DASHBOARD_PATH}");
        let res = self.authed_get(server, &url).await?;
        let status = res.status().as_u16();
        let html = res.text().await.map_err(|e| format!("{e}"))?;
        Ok((status, html))
    }

    async fn refresh_tools(&self, server: &McpServerRecord) -> Result<Vec<McpTool>, String> {
        // Node `callRpc`: まず直接実行し、セッションエラーに限り一度だけ再初期化して再試行する。
        match self.tools_list_once(server).await {
            Ok(tools) => Ok(tools),
            Err(e) if is_session_error(&e) => {
                // ステートフルサーバーのセッション未確立/失効。再初期化して一度だけ再試行する。
                self.forget_session(server.id);
                self.ensure_initialized(server).await?;
                self.tools_list_once(server).await
            }
            Err(e) => Err(e),
        }
    }

    async fn proxy(
        &self,
        server: &McpServerRecord,
        body: Bytes,
        accept: Option<&str>,
    ) -> Result<ProxyResponse, String> {
        // SSRF 対策: 中継直前に宛先を再検証（DNS リバインディング含む内部到達を遮断）。
        assert_safe_outbound_url(&server.endpoint_url).await?;

        let accept = accept.filter(|s| !s.is_empty()).unwrap_or(DEFAULT_ACCEPT);
        let mut req = self
            .http
            .post(&server.endpoint_url)
            .timeout(REQUEST_TIMEOUT)
            .header("Content-Type", "application/json")
            .header("Accept", accept)
            .header("MCP-Protocol-Version", PROTOCOL_VERSION);
        if let Some(token) = self.bearer_token(server) {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(sid) = self.session_id(server.id) {
            req = req.header("Mcp-Session-Id", sid);
        }

        let resp = req
            .body(body)
            .send()
            .await
            .map_err(|e| format!("{e}"))?;

        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_owned();
        let mcp_session_id = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if let Some(sid) = mcp_session_id.as_deref() {
            self.store_session_id(server.id, sid);
        }

        // ProxyResponse.body は `Bytes`（ストリーム欄なし）のためボディ全体を読む。
        let out_body = resp.bytes().await.map_err(|e| format!("{e}"))?;

        Ok(ProxyResponse {
            status,
            content_type,
            mcp_session_id,
            body: out_body,
        })
    }
}

// ─── SSRF ガード（Node `assertSafeOutboundUrl` / yuuka-browser ssrf.rs パリティ） ──────────────
//
// yuuka-browser の `assert_safe_outbound_url` は `mod ssrf;`（非 pub・未 re-export）で外部参照不可、
// かつ本 crate は yuuka-browser 外を編集できないため、Node パリティの最小実装を持つ。

/// 外向き URL の安全性を検証する（Node `assertSafeOutboundUrl`）。
///
/// # Errors
/// 形式不正・非 http(s)・認証情報付き・localhost・内部/予約レンジ解決時に日本語メッセージ。
async fn assert_safe_outbound_url(raw: &str) -> Result<(), String> {
    let url = url::Url::parse(raw).map_err(|_| "URLの形式が不正です。".to_owned())?;

    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "許可されていないスキームです: {scheme}:（http/https のみ）"
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLに認証情報（user:pass@）を含めることはできません。".to_owned());
    }

    match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            if is_blocked_ipv4(ip) {
                return Err(format!(
                    "内部/予約済みアドレスへの接続は許可されていません: {ip}"
                ));
            }
            Ok(())
        }
        Some(url::Host::Ipv6(ip)) => {
            if is_blocked_ipv6(ip) {
                return Err(format!(
                    "内部/予約済みアドレスへの接続は許可されていません: {ip}"
                ));
            }
            Ok(())
        }
        Some(url::Host::Domain(domain)) => {
            let host = domain.to_ascii_lowercase();
            if host.is_empty() || host == "localhost" || host.ends_with(".localhost") {
                return Err("ローカルホストへの接続は許可されていません。".to_owned());
            }
            let port = url.port_or_known_default().unwrap_or(80);
            let addrs = tokio::net::lookup_host((host.as_str(), port))
                .await
                .map_err(|_| format!("ホスト名を解決できませんでした: {host}"))?;
            let mut resolved = false;
            for addr in addrs {
                resolved = true;
                let ip = addr.ip();
                if is_blocked_ip(ip) {
                    return Err(format!(
                        "内部/予約済みアドレスに解決されるホストへの接続は許可されていません: {host} -> {ip}"
                    ));
                }
            }
            if resolved {
                Ok(())
            } else {
                Err(format!("ホスト名を解決できませんでした: {host}"))
            }
        }
        None => Err("ローカルホストへの接続は許可されていません。".to_owned()),
    }
}

/// IP が内部/予約レンジかを判定する（`true` = ブロック）。
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

/// IPv4 の内部/予約レンジ判定（Node `isBlockedIpv4` の全レンジ）。
fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _d] = ip.octets();
    a == 0                                   // 0.0.0.0/8 "this host"
        || a == 10                           // 10.0.0.0/8 private
        || a == 127                          // 127.0.0.0/8 loopback
        || (a == 169 && b == 254)            // 169.254.0.0/16 link-local（メタデータ）
        || (a == 172 && (16..=31).contains(&b)) // 172.16.0.0/12 private
        || (a == 192 && b == 168)            // 192.168.0.0/16 private
        || (a == 192 && b == 0 && c == 0)    // 192.0.0.0/24 IETF
        || (a == 192 && b == 0 && c == 2)    // 192.0.2.0/24 TEST-NET-1
        || (a == 198 && (b == 18 || b == 19)) // 198.18.0.0/15 benchmark
        || (a == 198 && b == 51 && c == 100) // 198.51.100.0/24 TEST-NET-2
        || (a == 203 && b == 0 && c == 113)  // 203.0.113.0/24 TEST-NET-3
        || (a == 100 && (64..=127).contains(&b)) // 100.64.0.0/10 CGNAT
        || a >= 224 // 224.0.0.0/4 multicast + 240.0.0.0/4 reserved + broadcast
}

/// IPv6 の内部/予約レンジ判定（Node `isBlockedIpv6`）。
fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    // IPv4-mapped (::ffff:a.b.c.d) は内側の v4 で判定。
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(v4);
    }
    let [seg0, seg1, ..] = ip.segments();
    (seg0 & 0xffc0) == 0xfe80        // fe80::/10 link-local
        || (seg0 & 0xfe00) == 0xfc00 // fc00::/7 unique-local
        || (seg0 & 0xffc0) == 0xfec0 // fec0::/10 site-local（deprecated）
        || (seg0 & 0xff00) == 0xff00 // ff00::/8 multicast
        || (seg0 == 0x2001 && seg1 == 0x0db8) // 2001:db8::/32 documentation
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(endpoint: &str) -> McpServerRecord {
        McpServerRecord {
            id: 1,
            user_id: Some("alice".to_owned()),
            name: "n".to_owned(),
            endpoint_url: endpoint.to_owned(),
            auth_credential_encrypted: None,
            auth_credential_iv: None,
            auth_credential_tag: None,
            tools_cache: "[]".to_owned(),
            tools_cache_updated: None,
            requires_confirmation: 1,
            enabled: 1,
            created_at: "now".to_owned(),
        }
    }

    // ── SSE data: 行パース → JSON-RPC 抽出 ─────────────────────────────────────

    #[test]
    fn sse_extracts_matching_id() {
        let body = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/x\"}\n",
            "\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"tools\":[]}}\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":8,\"result\":{\"ok\":true}}\n",
        );
        // id 一致を優先して返す。
        let v = parse_sse_body(body, Some(7)).expect("some");
        assert_eq!(v.get("id").and_then(Value::as_i64), Some(7));
    }

    #[test]
    fn sse_falls_back_to_last_when_no_id_match() {
        let body = concat!(
            "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"a\":1}}\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"b\":2}}\n",
        );
        // 期待 id が無ければ最後の result 応答。
        let v = parse_sse_body(body, Some(99)).expect("some");
        assert_eq!(v.get("id").and_then(Value::as_i64), Some(2));
    }

    #[test]
    fn sse_skips_notifications_done_and_bad_lines() {
        let body = concat!(
            "data: [DONE]\n",
            "data: not-json\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notify\"}\n", // result/error 無しはスキップ
            ": comment line\n",
            "data:{\"jsonrpc\":\"2.0\",\"id\":3,\"error\":{\"code\":-1,\"message\":\"boom\"}}\n", // 空白無し data:
        );
        let v = parse_sse_body(body, None).expect("some");
        assert!(v.get("error").is_some());
        assert_eq!(v.get("id").and_then(Value::as_i64), Some(3));
    }

    #[test]
    fn sse_empty_returns_none() {
        assert!(parse_sse_body("", None).is_none());
        assert!(parse_sse_body("data: [DONE]\n", None).is_none());
    }

    // ── tools/list 結果パース ─────────────────────────────────────────────────

    #[test]
    fn tools_result_parses_and_filters() {
        let rpc = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "tools": [
                    { "name": "echo", "description": "repeat" },
                    { "name": "noop" },                       // description 欠落 → None
                    { "name": "", "description": "skip" },    // name 空 → 落とす
                    { "description": "no-name" },             // name 無し → 落とす
                    "not-an-object",                          // 非 object → 落とす
                    { "name": "empty-desc", "description": "" } // 空 description → None
                ]
            }
        });
        let tools = parse_tools_result(Some(&rpc));
        assert_eq!(tools.len(), 3);
        assert_eq!(tools.first().unwrap().name, "echo");
        assert_eq!(tools.first().unwrap().description.as_deref(), Some("repeat"));
        assert_eq!(tools.get(1).unwrap().name, "noop");
        assert_eq!(tools.get(1).unwrap().description, None);
        assert_eq!(tools.get(2).unwrap().name, "empty-desc");
        assert_eq!(tools.get(2).unwrap().description, None);
    }

    #[test]
    fn tools_result_non_array_is_empty() {
        assert!(parse_tools_result(None).is_empty());
        assert!(parse_tools_result(Some(&json!({ "result": {} }))).is_empty());
        assert!(parse_tools_result(Some(&json!({ "result": { "tools": "x" } }))).is_empty());
        assert!(parse_tools_result(Some(&json!({}))).is_empty());
    }

    // ── origin 抽出 ───────────────────────────────────────────────────────────

    #[test]
    fn origin_extraction() {
        assert_eq!(
            mcp_origin("https://mcp.example/mcp").as_deref(),
            Ok("https://mcp.example")
        );
        assert_eq!(
            mcp_origin("http://mcp.example:8080/mcp?q=1#h").as_deref(),
            Ok("http://mcp.example:8080")
        );
        // 既定ポートは URL.origin では省かれる（ホストのみ）。
        assert_eq!(
            mcp_origin("https://mcp.example:443/x").as_deref(),
            Ok("https://mcp.example")
        );
        assert!(mcp_origin("not a url").is_err());
    }

    // ── session-id / セッションエラー判定 ────────────────────────────────────

    #[test]
    fn session_error_classification() {
        assert!(is_session_error("HTTP 400: bad request"));
        assert!(is_session_error("HTTP 404: not found"));
        assert!(is_session_error("Session expired"));
        assert!(is_session_error("server not initialized"));
        assert!(is_session_error("failed to initialize"));
        // タイムアウト・断・500 は再試行対象外。
        assert!(!is_session_error("HTTP 500: internal"));
        assert!(!is_session_error("connection reset"));
        assert!(!is_session_error("operation timed out"));
    }

    #[test]
    fn session_id_cache_roundtrip() {
        let client = HttpMcpClient::new(None, reqwest::Client::new());
        assert_eq!(client.session_id(1), None);
        client.store_session_id(1, "sess-abc");
        assert_eq!(client.session_id(1).as_deref(), Some("sess-abc"));
        // 上書き。
        client.store_session_id(1, "sess-def");
        assert_eq!(client.session_id(1).as_deref(), Some("sess-def"));
        // forget で session と initialized を破棄。
        client.mark_initialized(1);
        assert!(client.is_initialized(1));
        client.forget_session(1);
        assert_eq!(client.session_id(1), None);
        assert!(!client.is_initialized(1));
    }

    #[test]
    fn bearer_token_none_without_crypto_or_creds() {
        let client = HttpMcpClient::new(None, reqwest::Client::new());
        // 認証情報無し → None。
        assert_eq!(client.bearer_token(&server("https://x/mcp")), None);
        // 暗号鍵無しだが認証情報あり → None（復号不能で認証なし続行）。
        let mut s = server("https://x/mcp");
        s.auth_credential_encrypted = Some("enc".to_owned());
        s.auth_credential_iv = Some("iv".to_owned());
        s.auth_credential_tag = Some("tag".to_owned());
        assert_eq!(client.bearer_token(&s), None);
    }

    // ── SSRF ガード（IP レンジ判定・純ロジック） ──────────────────────────────

    #[test]
    fn ssrf_ipv4_ranges() {
        for ip in [
            "0.0.0.1", "10.0.0.1", "127.0.0.1", "169.254.169.254", "172.16.0.1",
            "192.168.1.1", "100.64.0.1", "224.0.0.1", "255.255.255.255",
        ] {
            assert!(is_blocked_ipv4(ip.parse().unwrap()), "expected blocked: {ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "172.15.0.1", "100.63.0.1"] {
            assert!(!is_blocked_ipv4(ip.parse().unwrap()), "expected allowed: {ip}");
        }
    }

    #[test]
    fn ssrf_ipv6_ranges_and_dispatch() {
        for ip in ["::1", "::", "fe80::1", "fc00::1", "fec0::1", "ff02::1", "2001:db8::1"] {
            assert!(is_blocked_ipv6(ip.parse().unwrap()), "expected blocked: {ip}");
        }
        assert!(!is_blocked_ipv6("2606:4700:4700::1111".parse().unwrap()));
        // is_blocked_ip のディスパッチ。
        assert!(is_blocked_ip("127.0.0.1".parse().unwrap()));
        assert!(!is_blocked_ip("8.8.8.8".parse().unwrap()));
    }

    #[tokio::test]
    async fn ssrf_rejects_scheme_userinfo_localhost_and_ip_literals() {
        assert!(assert_safe_outbound_url("ftp://example.com")
            .await
            .unwrap_err()
            .contains("許可されていないスキーム"));
        assert!(assert_safe_outbound_url("http://user:pass@example.com")
            .await
            .unwrap_err()
            .contains("認証情報"));
        assert!(assert_safe_outbound_url("http://localhost/")
            .await
            .unwrap_err()
            .contains("ローカルホスト"));
        assert!(assert_safe_outbound_url("http://127.0.0.1/")
            .await
            .unwrap_err()
            .contains("内部/予約済み"));
        assert!(assert_safe_outbound_url("http://[::1]/")
            .await
            .unwrap_err()
            .contains("内部/予約済み"));
        assert!(assert_safe_outbound_url("not-a-url")
            .await
            .unwrap_err()
            .contains("URLの形式"));
    }
}
