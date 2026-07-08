//! サービス共通コンテキスト（DB・通知・メトリクス・横断アクセス証憑）。
//!
//! cron/バッチのブートストラップ起点として [`CrossUserAccess`] を保持する（全ユーザー横断走査は
//! ここでしか作れない・§7.3）。各サービスの tick はこのコンテキストからスコープ無し repo と
//! notifier を得る。`Clone` 可（Db/Arc/証憑いずれも安価に複製できる）。

use std::sync::Arc;

use yuuka_core::CrossUserAccess;
use yuuka_web::Db;

use crate::metrics::MetricsRegistry;
use crate::notifier::Notifier;

/// サービス実行時の依存一式。
#[derive(Clone)]
pub struct ServiceContext {
    /// 共有 DB ハンドル（read pool + 単一 writer actor）。
    pub db: Db,
    /// ユーザー通知配信（discord 未配線時は [`crate::notifier::NullNotifier`]）。
    pub notifier: Arc<dyn Notifier>,
    /// 軽量メトリクスレジストリ（定期ログサービスが snapshot を出す）。
    pub metrics: Arc<MetricsRegistry>,
    /// 横断（全ユーザー跨ぎ）アクセス証憑。cron の起点はここに限定される。
    pub cross: CrossUserAccess,
}

impl ServiceContext {
    /// 依存を束ねてコンテキストを作る（横断証憑はここで発行＝cron 起点）。
    #[must_use]
    pub fn new(db: Db, notifier: Arc<dyn Notifier>, metrics: Arc<MetricsRegistry>) -> Self {
        Self {
            db,
            notifier,
            metrics,
            cross: CrossUserAccess::for_scheduled_task(),
        }
    }
}
