//! 型付き Config と厳密 load/validate（§6.8）。
//!
//! config.yaml → 型付き構造体へ厳密デシリアライズし、必須欠落・型不一致・不正値は
//! **起動時に fail-fast**（`ConfigError`。§5.6 の唯一の致命ポイント）。
//! 機密は本 struct に平文で持たず [`crate::secrets`] の `SecretString` で扱う。

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::ConfigError;

/// 起動時に検証済みの型付き設定。以後サービスループへ不変で渡す。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// 公開ベース URL（`https://` なら Cookie ハードニング + HSTS を強制）。
    #[serde(default)]
    pub base_url: Option<String>,
    /// バインドホスト。
    pub host: IpAddr,
    /// バインドポート。
    pub port: u16,
    /// SQLite DB ファイルパス。
    pub db_path: PathBuf,
    /// セッション TTL（日）。
    pub session_ttl_days: u32,
    /// XFF 信頼判定に使う信頼プロキシ。
    #[serde(default)]
    pub trusted_proxies: Vec<IpAddr>,
    /// index.html へ差し込む google-site-verification 値。
    #[serde(default)]
    pub google_site_verification: Option<String>,
}

impl Config {
    /// config.yaml を読み込み、厳密検証して返す。**起動時のみ**呼ぶ（fail-fast）。
    ///
    /// # Errors
    /// ファイル欠落・パース失敗・値検証失敗で [`ConfigError`]。
    pub fn load_and_validate(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|_| ConfigError::NotFound {
            path: path.display().to_string(),
        })?;
        let cfg: Config =
            serde_yaml::from_str(&raw).map_err(|source| ConfigError::Parse { source })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// パース済み設定の値レベル検証。
    ///
    /// # Errors
    /// ポート 0 や TTL 0 等の不正値で [`ConfigError::InvalidValue`]。
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.port == 0 {
            return Err(ConfigError::InvalidValue {
                field: "port",
                reason: "port must be non-zero".to_owned(),
            });
        }
        if self.session_ttl_days == 0 {
            return Err(ConfigError::InvalidValue {
                field: "session_ttl_days",
                reason: "session TTL must be at least 1 day".to_owned(),
            });
        }
        Ok(())
    }

    /// HTTPS 本番デプロイか（`base_url` が `https://` で始まるか・§6.8 の型表現）。
    #[must_use]
    pub fn is_https_deployment(&self) -> bool {
        self.base_url
            .as_deref()
            .is_some_and(|u| u.starts_with("https://"))
    }
}
