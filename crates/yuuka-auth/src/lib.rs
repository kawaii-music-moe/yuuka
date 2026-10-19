//! yuuka-auth — 実 [`AuthBackend`] 実装。
//!
//! - **Cookie セッション**: 共有 Redis の不透明トークン（Node が発行、Rust は**検証のみ**）。
//!   `session:{sha256hex(token)}` の JSON `{discordId,username,role}` を引き、TTL をスライディング更新。
//! - **Bearer デスクトップトークン**: SQLite `desktop_tokens`（sha256hex・失効/TTL 判定・`users` JOIN・
//!   `last_used_at` touch）。
//!
//! トークンのハッシュ・キー書式・TTL・シリアライズは既存 Node（`sessionService.ts`/
//! `desktopAuthService.ts`）と一致させる。Redis 到達不能でも起動は継続し、Cookie 経路のみ
//! 縮退（Bearer は SQLite なので動作）＝制約2（常時稼働・自己復帰）。

mod desktop;
mod device;
mod device_auth;
mod session;

pub mod audit;
pub mod routes;
pub mod token;
pub mod users;

pub use device::routes as device_routes;
pub use device_auth::{device_auth_routes, DeviceAuthStore};
pub use routes::{build_session_cookie, routes, AuthRuntime, SessionCookieToken};
pub use session::SessionStore;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use yuuka_core::config::DEFAULT_DESKTOP_TOKEN_TTL_DAYS;
use yuuka_core::AuthError;
use yuuka_types::SessionUser;
use yuuka_web::{AuthBackend, Db};

/// 生トークン文字列(UTF-8) の SHA-256 を**小文字 hex**で返す（Node `sha256Hex` と一致）。
#[must_use]
pub fn sha256_hex(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Cookie（Redis）＋ Bearer（SQLite）を束ねる実 [`AuthBackend`]。
pub struct CompositeAuth {
    db: Db,
    sessions: SessionStore,
    session_ttl_secs: u64,
    /// デスクトップトークンの TTL（日・config `DESKTOP_TOKEN_TTL_DAYS`・既定 90）。
    desktop_token_ttl_days: u32,
}

impl CompositeAuth {
    /// DB ハンドル・セッションストア・セッション TTL（日）から構築する。
    #[must_use]
    pub fn new(db: Db, sessions: SessionStore, session_ttl_days: u32) -> Self {
        Self {
            db,
            sessions,
            session_ttl_secs: u64::from(session_ttl_days) * 24 * 60 * 60,
            desktop_token_ttl_days: DEFAULT_DESKTOP_TOKEN_TTL_DAYS,
        }
    }

    /// デスクトップトークンの TTL（日・config `DESKTOP_TOKEN_TTL_DAYS`）を差し替える。未指定なら既定 90 日。
    #[must_use]
    pub fn with_desktop_token_ttl_days(mut self, days: u32) -> Self {
        self.desktop_token_ttl_days = days;
        self
    }
}

#[async_trait]
impl AuthBackend for CompositeAuth {
    async fn session_user(&self, cookie_token: &str) -> Result<Option<SessionUser>, AuthError> {
        let hash = sha256_hex(cookie_token);
        self.sessions.get(&hash, self.session_ttl_secs).await
    }

    async fn desktop_user(&self, bearer_token: &str) -> Result<Option<SessionUser>, AuthError> {
        let hash = sha256_hex(bearer_token);
        desktop::verify(&self.db, hash, i64::from(self.desktop_token_ttl_days)).await
    }
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn sha256_hex_matches_node() {
        // Node: crypto.createHash("sha256").update("abc").digest("hex")
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 空文字の既知ベクタ。
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
