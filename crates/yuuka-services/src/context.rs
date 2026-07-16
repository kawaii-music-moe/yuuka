//! サービス共通コンテキスト（DB・通知・メトリクス・横断アクセス証憑）。
//!
//! cron/バッチのブートストラップ起点として [`CrossUserAccess`] を保持する（全ユーザー横断走査は
//! ここでしか作れない・§7.3）。各サービスの tick はこのコンテキストからスコープ無し repo と
//! notifier を得る。`Clone` 可（Db/Arc/証憑いずれも安価に複製できる）。

use std::sync::Arc;

use yuuka_core::CrossUserAccess;
use yuuka_web::Db;

use crate::backup::{BackupRunner, NullBackupRunner};
use crate::metrics::MetricsRegistry;
use crate::notifier::Notifier;
use crate::turn::PlaybookRunner;

/// サービス実行時の依存一式。
#[derive(Clone)]
pub struct ServiceContext {
    /// 共有 DB ハンドル（read pool + 単一 writer actor）。
    pub db: Db,
    /// ユーザー通知配信（discord 未配線時は [`crate::notifier::NullNotifier`]）。
    pub notifier: Arc<dyn Notifier>,
    /// 軽量メトリクスレジストリ（定期ログサービスが snapshot を出す）。
    pub metrics: Arc<MetricsRegistry>,
    /// マクロ定期実行の秘書ターン起動ポート（未配線時は [`crate::turn::NullPlaybookRunner`]）。
    pub playbook_runner: Arc<dyn PlaybookRunner>,
    /// 定期バックアップの実行ポート（未配線時は [`NullBackupRunner`]・[`crate::backup::BackupService`] が使う）。
    pub backup: Arc<dyn BackupRunner>,
    /// 横断（全ユーザー跨ぎ）アクセス証憑。cron の起点はここに限定される。
    pub cross: CrossUserAccess,
}

impl ServiceContext {
    /// 依存を束ねてコンテキストを作る（横断証憑はここで発行＝cron 起点）。バックアップは
    /// [`ServiceContext::with_backup`] で live 実行ポートを注入する（既定は [`NullBackupRunner`]）。
    #[must_use]
    pub fn new(
        db: Db,
        notifier: Arc<dyn Notifier>,
        metrics: Arc<MetricsRegistry>,
        playbook_runner: Arc<dyn PlaybookRunner>,
    ) -> Self {
        Self {
            db,
            notifier,
            metrics,
            playbook_runner,
            backup: Arc::new(NullBackupRunner),
            cross: CrossUserAccess::for_scheduled_task(),
        }
    }

    /// バックアップ実行ポートを差し替える（main が `GoogleBackupClient` アダプタを注入する）。
    #[must_use]
    pub fn with_backup(mut self, backup: Arc<dyn BackupRunner>) -> Self {
        self.backup = backup;
        self
    }
}
