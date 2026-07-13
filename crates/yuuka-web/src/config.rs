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
    /// CSRF の許可オリジン ホスト名（`config.base_url` のホスト名・Node `isAllowedHost`）。
    /// `None`（`BASE_URL` 未設定）は localhost 群を許可する開発既定。**クライアント供給の `Host`
    /// には依存しない**（Host 注入で allowlist を迂回されないための設定ベース信頼アンカー）。
    pub allowed_host: Option<String>,
    /// XFF 信頼判定に使う信頼プロキシ（レート制限のクライアント IP 解決・Node `getClientIp`）。
    /// 直前 peer がこのリストに含まれるときのみ `X-Forwarded-For` を信頼する。
    pub trusted_proxies: Vec<std::net::IpAddr>,
    /// タイムライン メディアの保存ディレクトリ（Node `MEDIA_DIR = cwd/data/media`）。
    /// `from_core` は `data/media`（cwd 相対）を設定する。`Default` は空（媒体経路を使わないテスト用）。
    pub media_dir: std::path::PathBuf,
}

impl WebConfig {
    /// 検証済みの [`yuuka_core::Config`] から web 実行時設定を導出する。
    #[must_use]
    pub fn from_core(cfg: &yuuka_core::Config) -> Self {
        Self {
            privacy_policy_url: cfg.privacy_policy_url.clone(),
            terms_url: cfg.terms_url.clone(),
            https: cfg.is_https_deployment(),
            allowed_host: crate::csrf::allowed_host_from_base_url(cfg.base_url.as_deref()),
            trusted_proxies: cfg.trusted_proxies.clone(),
            media_dir: std::path::PathBuf::from("data/media"),
        }
    }
}
