//! MCP 仕様の Authorization（OAuth 2.1）をホスト側で回すための部品。
//!
//! 認可の手順そのもの（保護リソースメタデータ RFC 9728 → 認可サーバーメタデータ RFC 8414 / OIDC の発見、
//! クライアント登録の優先順位〔事前登録 → Client ID Metadata Document → 動的クライアント登録〕、PKCE S256、
//! `resource` パラメータ、トークンの更新）は rmcp の `AuthorizationManager` / `AuthorizationSession` に
//! 任せる。ここに置くのは yuuka 固有の差し込み口だけ:
//!
//! - [`DbCredentialStore`]: 資格情報を `mcp_oauth_credentials` に暗号化して保存する（再起動を跨ぐ）。
//! - [`GuardedOAuthHttp`]: 発見・登録・トークン取得の HTTP を、SSRF ガード付きクライアントで送る。
//! - [`PendingAuthorizations`]: 認可 URL を発行してからコールバックが戻るまでのセッションを保持する。
//! - Client ID Metadata Document（[`client_metadata_document`]）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rmcp::transport::auth::{
    AuthError, AuthorizationSession, CredentialStore, OAuthHttpClient, OAuthHttpClientError,
    OAuthHttpClientFuture, OAuthHttpRedirectPolicy, OAuthHttpRequest, StoredCredentials,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use yuuka_crypto::SystemCrypto;
use yuuka_outbound::assert_safe_outbound_url_without_dns;
use yuuka_web::Db;

use crate::repo::{self, EncryptedOAuthCredentials};

/// 認可サーバーからのコールバックを受けるパス。
pub(crate) const CALLBACK_PATH: &str = "/api/mcp-servers/oauth/callback";
/// Client ID Metadata Document を公開するパス（URL そのものが client_id になる）。
pub(crate) const CLIENT_METADATA_PATH: &str = "/api/mcp-servers/oauth/client-metadata.json";
/// 認可サーバーに名乗るクライアント名。
pub(crate) const CLIENT_NAME: &str = "yuuka";

/// 認可 URL を発行してからコールバックを待つ時間。
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);
/// 同時に保持する認可待ちの上限（放置された開始要求でメモリを食い潰さない）。
const MAX_PENDING: usize = 64;
/// OAuth 応答ボディの上限。
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// 発見時に追うリダイレクトの上限。
const MAX_REDIRECTS: usize = 5;

/// Client ID Metadata Document（SEP-991）。`client_id` は公開 URL そのもので、文書の URL と一致させる。
#[must_use]
pub(crate) fn client_metadata_document(base_origin: &str) -> Value {
    json!({
        "client_id": format!("{base_origin}{CLIENT_METADATA_PATH}"),
        "client_name": CLIENT_NAME,
        "client_uri": base_origin,
        "redirect_uris": [format!("{base_origin}{CALLBACK_PATH}")],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "application_type": "web",
    })
}

// ─── 資格情報の保存先（DB・暗号化） ─────────────────────────────────────────────

/// rmcp の [`CredentialStore`] を `mcp_oauth_credentials` で実装する。中身は `StoredCredentials` の JSON を
/// [`SystemCrypto`] で暗号化したもの（`auth_credential` と同じ鍵・鍵ローテーションの対象）。
pub(crate) struct DbCredentialStore {
    pub(crate) db: Db,
    pub(crate) crypto: Arc<SystemCrypto>,
    pub(crate) server_id: i64,
}

fn store_err(e: impl std::fmt::Display) -> AuthError {
    AuthError::CredentialStoreError(e.to_string())
}

#[async_trait]
impl CredentialStore for DbCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let Some(row) = repo::load_oauth_credentials(&self.db, self.server_id)
            .await
            .map_err(store_err)?
        else {
            return Ok(None);
        };
        let plain = self
            .crypto
            .decrypt_text(&row.encrypted, &row.iv, &row.tag)
            .map_err(store_err)?;
        serde_json::from_str(&plain).map(Some).map_err(store_err)
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let plain = serde_json::to_string(&credentials).map_err(store_err)?;
        let enc = self.crypto.encrypt_text(&plain).map_err(store_err)?;
        repo::save_oauth_credentials(
            &self.db,
            self.server_id,
            EncryptedOAuthCredentials {
                encrypted: enc.encrypted,
                iv: enc.iv,
                tag: enc.auth_tag,
            },
        )
        .await
        .map_err(store_err)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        repo::delete_oauth_credentials(&self.db, self.server_id)
            .await
            .map_err(store_err)
    }
}

// ─── OAuth の HTTP（SSRF ガード付き） ────────────────────────────────────────────

/// rmcp の [`OAuthHttpClient`] を、検査済みアドレスにだけ接続する reqwest クライアントで実装する。
///
/// クライアント自体は自動リダイレクト無効。`Follow` を求められたときは、ここで 1 段ずつ宛先を検査して
/// 追う（IP リテラルはリゾルバを通らないため、URL ごとに [`assert_safe_outbound_url_without_dns`] で弾く）。
pub(crate) struct GuardedOAuthHttp {
    pub(crate) client: reqwest::Client,
}

impl GuardedOAuthHttp {
    async fn send(
        &self,
        request: http::Request<Vec<u8>>,
        redirect_policy: OAuthHttpRedirectPolicy,
        timeout: Option<Duration>,
    ) -> Result<http::Response<Vec<u8>>, OAuthHttpClientError> {
        let (parts, body) = request.into_parts();
        let mut method = parts.method;
        let mut url = parts.uri.to_string();
        let mut body = Some(body);
        let headers = parts.headers;
        let mut hops = 0;
        loop {
            assert_safe_outbound_url_without_dns(&url).map_err(OAuthHttpClientError::from)?;
            let mut req = self
                .client
                .request(method.clone(), url.as_str())
                .headers(headers.clone());
            if let Some(b) = body.clone() {
                req = req.body(b);
            }
            if let Some(t) = timeout {
                req = req.timeout(t);
            }
            let mut resp = req.send().await.map_err(OAuthHttpClientError::from)?;
            let status = resp.status();
            if redirect_policy == OAuthHttpRedirectPolicy::Follow && status.is_redirection() {
                let location = resp
                    .headers()
                    .get(http::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                if let Some(location) = location {
                    hops += 1;
                    if hops > MAX_REDIRECTS {
                        return Err("OAuth のリダイレクトが多すぎます".into());
                    }
                    let next = url::Url::parse(&url)
                        .and_then(|base| base.join(&location))
                        .map_err(OAuthHttpClientError::from)?;
                    url = next.to_string();
                    // 303 と、POST への 301/302 は GET に変えて本文を落とす（ブラウザ準拠）。
                    if status == http::StatusCode::SEE_OTHER
                        || (method == http::Method::POST
                            && matches!(
                                status,
                                http::StatusCode::MOVED_PERMANENTLY | http::StatusCode::FOUND
                            ))
                    {
                        method = http::Method::GET;
                        body = None;
                    }
                    continue;
                }
            }

            let mut out = http::Response::builder()
                .status(status)
                .version(resp.version());
            for (name, value) in resp.headers() {
                out = out.header(name, value);
            }
            let mut buf = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(OAuthHttpClientError::from)? {
                if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
                    return Err("OAuth の応答が大きすぎます".into());
                }
                buf.extend_from_slice(&chunk);
            }
            return out.body(buf).map_err(OAuthHttpClientError::from);
        }
    }
}

impl OAuthHttpClient for GuardedOAuthHttp {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        let OAuthHttpRequest {
            request,
            redirect_policy,
            timeout,
            ..
        } = request;
        Box::pin(self.send(request, redirect_policy, timeout))
    }
}

// ─── 認可待ちのセッション ─────────────────────────────────────────────────────

/// 認可 URL を発行済みで、コールバックを待っている 1 件。
pub(crate) struct Pending {
    pub(crate) server_id: i64,
    pub(crate) user_id: String,
    pub(crate) created: Instant,
    pub(crate) session: AuthorizationSession,
}

/// `state`（CSRF トークン）→ 認可待ちセッション。
#[derive(Default)]
pub(crate) struct PendingAuthorizations {
    inner: Mutex<HashMap<String, Pending>>,
}

impl PendingAuthorizations {
    /// 認可待ちを登録する。期限切れを掃除し、上限を超えるなら最も古いものを捨てる。
    pub(crate) async fn insert(&self, state: String, pending: Pending) {
        let mut map = self.inner.lock().await;
        map.retain(|_, p| p.created.elapsed() < PENDING_TTL);
        while map.len() >= MAX_PENDING {
            let Some(oldest) = map
                .iter()
                .min_by_key(|(_, p)| p.created)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            map.remove(&oldest);
        }
        map.insert(state, pending);
    }

    /// `state` の認可待ちを取り出す（一度きり）。期限切れは `None`。
    pub(crate) async fn take(&self, state: &str) -> Option<Pending> {
        let mut map = self.inner.lock().await;
        let pending = map.remove(state)?;
        (pending.created.elapsed() < PENDING_TTL).then_some(pending)
    }

    /// 指定サーバーの認可待ちを捨てる（サーバー削除時）。
    pub(crate) async fn forget_server(&self, server_id: i64) {
        self.inner
            .lock()
            .await
            .retain(|_, p| p.server_id != server_id);
    }
}

/// 認可 URL から `state` パラメータを取り出す。
#[must_use]
pub(crate) fn state_of(auth_url: &str) -> Option<String> {
    let url = url::Url::parse(auth_url).ok()?;
    url.query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_metadata_matches_its_own_url() {
        let doc = client_metadata_document("https://yuuka.example");
        assert_eq!(
            doc["client_id"],
            "https://yuuka.example/api/mcp-servers/oauth/client-metadata.json"
        );
        assert_eq!(
            doc["redirect_uris"][0],
            "https://yuuka.example/api/mcp-servers/oauth/callback"
        );
        assert_eq!(doc["token_endpoint_auth_method"], "none");
    }

    #[test]
    fn state_is_read_from_authorization_url() {
        assert_eq!(
            state_of("https://as.example/authorize?response_type=code&state=abc123&x=1").as_deref(),
            Some("abc123")
        );
        assert_eq!(state_of("https://as.example/authorize?state="), None);
        assert_eq!(state_of("not a url"), None);
    }
}
