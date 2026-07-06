//! yuuka-web 実行時設定（Phase 1 増分1）。
//!
//! 現状は policy URL と HTTPS 判定のみ。後続で `yuuka_core::Config` から構築する。

/// web 層の実行時設定。
#[derive(Debug, Clone, Default)]
pub struct WebConfig {
    /// プライバシーポリシー URL（`/api/me` 等で常に返す。未設定は空文字＝Node と一致）。
    pub privacy_policy_url: String,
    /// 利用規約 URL（同上）。
    pub terms_url: String,
    /// HTTPS デプロイか（Cookie 名 `__Host-` の選択に使う・§11.3）。
    pub https: bool,
}

impl WebConfig {
    /// 検証済みの [`yuuka_core::Config`] から web 実行時設定を導出する。
    #[must_use]
    pub fn from_core(cfg: &yuuka_core::Config) -> Self {
        Self {
            privacy_policy_url: cfg.privacy_policy_url.clone(),
            terms_url: cfg.terms_url.clone(),
            https: cfg.is_https_deployment(),
        }
    }
}
