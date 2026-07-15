//! プロキシ用トークン（ダッシュボード SPA の API 中継を認証する短命トークン）。
//!
//! Node `mcpRoutes.ts` の `proxyTokens` インメモリ Map パリティ。ダッシュボード HTML 発行時
//! （Cookie 認証済みユーザー）に生成し、SPA からの `Authorization: Bearer` で検証する。SPA は
//! `tools/list`・`tools/call` を複数回呼ぶため使い捨て（single-use）ではなく、TTL 内は再利用可能な
//! セッショントークンである。`{serverId, userId}` に束縛し、256bit 乱数で不可推測・サーバー無効化/
//! 削除で即時失効する。状態はインメモリ（`Mutex<HashMap>`・[`yuuka_auth`] の `DeviceAuthStore` と同方式）。

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// プロキシトークンの TTL（1 時間・Node `PROXY_TOKEN_TTL_MS`）。
const PROXY_TOKEN_TTL: Duration = Duration::from_secs(60 * 60);

/// 発行済みトークン 1 件（Node `ProxyTokenEntry`）。
struct ProxyTokenEntry {
    server_id: i64,
    user_id: String,
    expires_at: Instant,
}

/// プロキシトークンのインメモリストア（`token(hex) → entry`）。web 再起動を跨ぐよう main で 1 度生成し
/// `Extension` として貫通させる（Node のモジュールスコープ `Map` 相当）。
pub struct ProxyTokenManager {
    tokens: Mutex<HashMap<String, ProxyTokenEntry>>,
}

impl Default for ProxyTokenManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyTokenManager {
    /// 空のストアを作る。
    #[must_use]
    pub fn new() -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, ProxyTokenEntry>> {
        self.tokens.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 期限切れトークンをまとめて掃除する（発行・検証の双方で呼び、放置による肥大を防ぐ・Node
    /// `sweepExpiredProxyTokens`）。
    fn sweep(tokens: &mut HashMap<String, ProxyTokenEntry>, now: Instant) {
        tokens.retain(|_, v| v.expires_at > now);
    }

    /// プロキシトークンを発行する（Node `issueProxyToken`）。256bit 乱数を hex で返す。
    ///
    /// 乱数取得に失敗した場合は空文字を返す（実質発生しない・呼び出し側は検証で弾かれるだけ）。
    #[must_use]
    pub fn issue(&self, server_id: i64, user_id: &str) -> String {
        let now = Instant::now();
        let mut bytes = [0u8; 32];
        if getrandom::getrandom(&mut bytes).is_err() {
            return String::new();
        }
        let token = hex::encode(bytes);
        let mut tokens = self.lock();
        Self::sweep(&mut tokens, now);
        tokens.insert(
            token.clone(),
            ProxyTokenEntry {
                server_id,
                user_id: user_id.to_owned(),
                expires_at: now + PROXY_TOKEN_TTL,
            },
        );
        token
    }

    /// プロキシトークンを検証する（`server_id` 一致・未失効・Node `validateProxyToken`）。一致すれば
    /// 発行ユーザー ID を返す。失効分はここでも掃除する。
    #[must_use]
    pub fn validate(&self, token: &str, server_id: i64) -> Option<String> {
        let now = Instant::now();
        let mut tokens = self.lock();
        Self::sweep(&mut tokens, now);
        let entry = tokens.get(token)?;
        if entry.server_id != server_id {
            return None;
        }
        if entry.expires_at <= now {
            tokens.remove(token);
            return None;
        }
        Some(entry.user_id.clone())
    }

    /// 指定サーバーのプロキシトークンを即時失効させる（無効化・削除時に呼ぶ・Node
    /// `revokeProxyTokensForServer`）。
    pub fn revoke_for_server(&self, server_id: i64) {
        let mut tokens = self.lock();
        tokens.retain(|_, v| v.server_id != server_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_validate_roundtrip() {
        let mgr = ProxyTokenManager::new();
        let token = mgr.issue(7, "alice");
        // 256bit=32byte → 64 hex 文字。
        assert_eq!(token.len(), 64);
        // 正しい server_id なら発行ユーザーを返す。
        assert_eq!(mgr.validate(&token, 7).as_deref(), Some("alice"));
        // server_id 不一致は None。
        assert_eq!(mgr.validate(&token, 8), None);
        // 未知トークンは None。
        assert_eq!(mgr.validate("deadbeef", 7), None);
    }

    #[test]
    fn revoke_for_server_clears_only_that_server() {
        let mgr = ProxyTokenManager::new();
        let t7 = mgr.issue(7, "alice");
        let t8 = mgr.issue(8, "bob");
        mgr.revoke_for_server(7);
        assert_eq!(mgr.validate(&t7, 7), None);
        // 別サーバーのトークンは残る。
        assert_eq!(mgr.validate(&t8, 8).as_deref(), Some("bob"));
    }

    #[test]
    fn expired_token_is_rejected() {
        let mgr = ProxyTokenManager::new();
        let token = mgr.issue(1, "u");
        // 失効時刻を過去へ差し替えて期限切れを再現する。
        {
            let mut tokens = mgr.lock();
            if let Some(e) = tokens.get_mut(&token) {
                e.expires_at = Instant::now() - Duration::from_secs(1);
            }
        }
        assert_eq!(mgr.validate(&token, 1), None);
    }
}
