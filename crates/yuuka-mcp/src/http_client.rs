//! ダッシュボード取得・プロキシ中継用の素の HTTP クライアント（Node `src/services/mcpClient.ts` の
//! MCP 以外の部分）。
//!
//! MCP プロトコル（`initialize` / `tools/list` / `tools/call`・SSE・セッション管理）は公式 SDK `rmcp`
//! で実装した [`RmcpClient`](crate::RmcpClient) が担う。ここに残るのは MCP ではない通常の HTTP だけ:
//! `GET <origin>/dashboard[/enable]` と、ダッシュボード SPA の JSON-RPC をそのまま上流へ中継する
//! `proxy`（`Mcp-Session-Id` はクライアントとの透過のためにだけプロセス内保持する）。
//!
//! **SSRF ガード**: すべての外向き取得の直前に宛先 URL を再検証する（DNS 解決先の全アドレスを検査）。
//! 共通 `yuuka-outbound` を利用する。検査後の接続 IP 固定・自動リダイレクト先の再検査は行っていない
//! （MCP 経路は [`RmcpClient`](crate::RmcpClient) 側で解決時検査 + リダイレクト無効化を行う）。
//!
//! **認証情報の復号**: `auth_credential_encrypted/iv/tag` を [`SystemCrypto::decrypt_text`] で復号し
//! `Authorization: Bearer {cred}` を付ける。復号失敗はログして認証なしで続行する（Node パリティ）。
//!
//! **live 検証は実環境へ延期**: ダッシュボード HTTP・プロキシ中継・DNS 解決を伴う SSRF は
//! ネットワーク到達を要するためユニットテスト対象外。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::Bytes;
use yuuka_crypto::SystemCrypto;
use yuuka_outbound::assert_safe_outbound_url;

use crate::repo::McpServerRecord;
use crate::ProxyResponse;

/// プロキシ中継時に付ける MCP プロトコルバージョン。
const PROTOCOL_VERSION: &str = "2025-11-25";
/// 1 リクエストのタイムアウト（Node `REQUEST_TIMEOUT_MS`）。
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_millis(15_000);
/// ダッシュボード有効判定パス（Node `DASHBOARD_ENABLE_PATH`）。
const DASHBOARD_ENABLE_PATH: &str = "/dashboard/enable";
/// ダッシュボード本体パス（Node `DASHBOARD_PATH`）。
const DASHBOARD_PATH: &str = "/dashboard";
/// プロキシ既定 Accept（Node の proxy 既定）。
const DEFAULT_ACCEPT: &str = "application/json, text/event-stream";

/// ダッシュボード/プロキシ用 HTTP クライアント。
///
/// `crypto` が `None` のときは認証情報の復号を行わない（Bearer 非注入）。`sessions` は
/// プロキシ中継の `Mcp-Session-Id` 透過用（サーバー ID → セッション ID）。
pub(crate) struct DashboardHttpClient {
    /// 認証情報（`auth_credential`）の復号鍵（`None` なら認証なしで続行）。
    crypto: Option<Arc<SystemCrypto>>,
    /// 外部 MCP サーバーへの HTTP クライアント（main.rs で構築して注入）。
    http: reqwest::Client,
    /// サーバー ID → `Mcp-Session-Id`（プロキシ中継で上流が発行した場合のみ保持）。
    sessions: Mutex<HashMap<i64, String>>,
}

impl DashboardHttpClient {
    pub(crate) fn new(crypto: Option<Arc<SystemCrypto>>, http: reqwest::Client) -> Self {
        Self {
            crypto,
            http,
            sessions: Mutex::new(HashMap::new()),
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

    /// プロキシ用 session-id を破棄する（サーバー無効化・削除・認証情報変更時）。
    pub(crate) fn forget_session(&self, server_id: i64) {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&server_id);
    }

    /// サーバーの `Authorization: Bearer` 値を組み立てる（Node `buildAuthHeader`）。
    /// 復号失敗はログして `None`（認証なしで続行）。暗号鍵未設定/認証情報無しも `None`。
    pub(crate) fn bearer_token(&self, server: &McpServerRecord) -> Option<String> {
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

    /// Bearer 付き GET を送る（Node `authedGet`）。SSRF 再検証・タイムアウト付き。
    async fn authed_get(
        &self,
        server: &McpServerRecord,
        url: &str,
    ) -> Result<reqwest::Response, String> {
        assert_safe_outbound_url(url).await?;
        let mut req = self.http.get(url).timeout(REQUEST_TIMEOUT);
        if let Some(token) = self.bearer_token(server) {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        req.send().await.map_err(|e| format!("{e}"))
    }

    /// `GET <origin>/dashboard/enable` が 200 か。
    pub(crate) async fn probe_dashboard(&self, server: &McpServerRecord) -> bool {
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

    pub(crate) async fn fetch_dashboard_html(
        &self,
        server: &McpServerRecord,
    ) -> Result<(u16, String), String> {
        let origin = mcp_origin(&server.endpoint_url)?;
        let url = format!("{origin}{DASHBOARD_PATH}");
        let res = self.authed_get(server, &url).await?;
        let status = res.status().as_u16();
        let html = res.text().await.map_err(|e| format!("{e}"))?;
        Ok((status, html))
    }

    pub(crate) async fn proxy(
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

        let resp = req.body(body).send().await.map_err(|e| format!("{e}"))?;

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
    fn proxy_session_id_cache_roundtrip() {
        let client = DashboardHttpClient::new(None, reqwest::Client::new());
        assert_eq!(client.session_id(1), None);
        client.store_session_id(1, "sess-abc");
        assert_eq!(client.session_id(1).as_deref(), Some("sess-abc"));
        client.store_session_id(1, "sess-def");
        assert_eq!(client.session_id(1).as_deref(), Some("sess-def"));
        client.forget_session(1);
        assert_eq!(client.session_id(1), None);
    }

    #[test]
    fn bearer_token_none_without_crypto_or_creds() {
        let client = DashboardHttpClient::new(None, reqwest::Client::new());
        // 認証情報無し → None。
        assert_eq!(client.bearer_token(&server("https://x/mcp")), None);
        // 暗号鍵無しだが認証情報あり → None（復号不能で認証なし続行）。
        let mut s = server("https://x/mcp");
        s.auth_credential_encrypted = Some("enc".to_owned());
        s.auth_credential_iv = Some("iv".to_owned());
        s.auth_credential_tag = Some("tag".to_owned());
        assert_eq!(client.bearer_token(&s), None);
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
