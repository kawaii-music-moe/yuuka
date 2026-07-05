//! アプリ共有状態（axum `State`）。

use std::sync::Arc;

use crate::auth::AuthBackend;
use crate::config::WebConfig;

/// ルータ全体で共有する状態。認証バックエンドと設定を trait/Arc で保持し、
/// Phase 1 の各ドメインはここへ repo/registry を差し込んでいく。
#[derive(Clone)]
pub struct AppState {
    /// 認証バックエンド（Cookie=Redis / Bearer=SQLite）。実装は増分で差し替え。
    pub auth: Arc<dyn AuthBackend>,
    /// web 実行時設定。
    pub config: Arc<WebConfig>,
}

impl AppState {
    /// 認証バックエンドと設定から状態を作る。
    #[must_use]
    pub fn new(auth: Arc<dyn AuthBackend>, config: WebConfig) -> Self {
        Self {
            auth,
            config: Arc::new(config),
        }
    }
}
