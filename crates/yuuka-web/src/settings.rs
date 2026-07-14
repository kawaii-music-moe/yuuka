//! `system_settings` の読み取りと公開 legal URL 解決（Node `systemSettingsRepo` / `publicLegalUrls` パリティ）。
//!
//! `/api/setup/status`・`/api/me` は privacy/terms URL を返すが、Node は **admin が UI で保存した
//! `system_settings` の値を優先**し、無ければ config（env）へフォールバックする
//! （`getSystemSetting(key) || config.X`）。この precedence を Rust 両ルートで共有するためのヘルパ。

use rusqlite::{params, OptionalExtension};
use yuuka_db::map_sqlite;

use crate::state::{AppState, Db};

/// `system_settings` から `key` の値を読む（Node `getSystemSetting`・**fail-safe**）。
///
/// 行なし・クエリ失敗はいずれも `None`（既定へフォールバック・エラーは握って warn に残す。Node の
/// try/catch → defaultValue と同じく本処理を落とさない）。
pub async fn get_system_setting(db: &Db, key: &str) -> Option<String> {
    let key = key.to_owned();
    match db
        .read
        .read(move |conn| {
            conn.query_row(
                "SELECT value FROM system_settings WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "system_settings 読み取りに失敗（既定値へフォールバック）");
            None
        }
    }
}

/// privacy/terms URL を Node `publicLegalUrls` パリティで解決する。
///
/// `system_settings` の**非空**値を優先し、無ければ config（env）へフォールバックする。Node の
/// `getSystemSetting(key) || config.X` は空文字も falsy 扱いのため、`.filter(|v| !v.is_empty())` で
/// 「DB に空文字が保存されていたら config へ落とす」挙動まで一致させる。
pub async fn public_legal_urls(state: &AppState) -> (String, String) {
    let privacy = get_system_setting(&state.db, "privacy_policy_url")
        .await
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| state.config.privacy_policy_url.clone());
    let terms = get_system_setting(&state.db, "terms_url")
        .await
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| state.config.terms_url.clone());
    (privacy, terms)
}

#[cfg(test)]
mod tests {
    use super::{get_system_setting, public_legal_urls};
    use crate::state::{AppState, Db};
    use crate::{AuthBackend, WebConfig};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use yuuka_core::AuthError;
    use yuuka_types::SessionUser;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct NoAuth;
    #[async_trait]
    impl AuthBackend for NoAuth {
        async fn session_user(&self, _t: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
        async fn desktop_user(&self, _t: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    fn db_with_setting(rows: &[(&str, &str)]) -> Db {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_settings_test_{}_{n}.sqlite",
            std::process::id()
        ));
        {
            rusqlite::Connection::open(&path).expect("seed file");
        }
        let db = Db::open(&path).expect("open"); // migrations で system_settings を作成。
        let conn = rusqlite::Connection::open(&path).expect("open2");
        for (k, v) in rows {
            conn.execute(
                "INSERT OR REPLACE INTO system_settings (key, value) VALUES (?1, ?2)",
                rusqlite::params![k, v],
            )
            .expect("seed setting");
        }
        db
    }

    fn state_with(db: Db, config: WebConfig) -> AppState {
        AppState::new(Arc::new(NoAuth), config, db)
    }

    #[tokio::test]
    async fn db_override_beats_config() {
        let db = db_with_setting(&[("privacy_policy_url", "https://db.example/privacy")]);
        let config = WebConfig {
            privacy_policy_url: "https://env.example/privacy".to_owned(),
            terms_url: "https://env.example/terms".to_owned(),
            ..WebConfig::default()
        };
        let state = state_with(db, config);
        let (privacy, terms) = public_legal_urls(&state).await;
        // privacy は DB 値が勝つ。terms は DB に無いので config へフォールバック。
        assert_eq!(privacy, "https://db.example/privacy");
        assert_eq!(terms, "https://env.example/terms");
    }

    #[tokio::test]
    async fn empty_db_value_falls_back_to_config() {
        // Node の `||` は空文字を falsy 扱い → config へフォールバックする。
        let db = db_with_setting(&[("terms_url", "")]);
        let config = WebConfig {
            terms_url: "https://env.example/terms".to_owned(),
            ..WebConfig::default()
        };
        let state = state_with(db, config);
        let (_privacy, terms) = public_legal_urls(&state).await;
        assert_eq!(terms, "https://env.example/terms");
    }

    #[tokio::test]
    async fn missing_key_is_none() {
        let db = db_with_setting(&[]);
        assert_eq!(get_system_setting(&db, "privacy_policy_url").await, None);
    }
}
