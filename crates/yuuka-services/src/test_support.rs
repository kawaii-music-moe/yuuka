//! テスト用ヘルパ: 指定 DDL でシード済みの一時 Db とサービスコンテキストを作る。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use yuuka_web::Db;

use crate::context::ServiceContext;
use crate::metrics::MetricsRegistry;
use crate::notifier::{Notification, Notifier, NullNotifier};
use crate::turn::{NullPlaybookRunner, PlaybookRunner};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// 指定 DDL で表を作った一時ファイル DB を開く。返す `TempDir` は生存させておくこと
/// （drop でファイル削除）。
pub fn seeded_db(ddl: &str) -> (Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.path().join(format!("svc_test_{seq}.sqlite"));
    {
        let conn = rusqlite::Connection::open(&path).expect("open seed conn");
        conn.execute_batch(ddl).expect("seed ddl");
    }
    let db = Db::open(&path).expect("open db");
    (db, dir)
}

/// `NullNotifier`（配信不可＝false）付きコンテキスト。
pub fn ctx_null(db: Db) -> ServiceContext {
    ServiceContext::new(
        db,
        Arc::new(NullNotifier),
        Arc::new(MetricsRegistry::new()),
        Arc::new(NullPlaybookRunner),
    )
}

/// 任意の notifier 付きコンテキスト。
pub fn ctx_with(db: Db, notifier: Arc<dyn Notifier>) -> ServiceContext {
    ServiceContext::new(
        db,
        notifier,
        Arc::new(MetricsRegistry::new()),
        Arc::new(NullPlaybookRunner),
    )
}

/// 任意の notifier + playbook runner 付きコンテキスト（playbook スケジュールのテスト用）。
pub fn ctx_full(
    db: Db,
    notifier: Arc<dyn Notifier>,
    playbook_runner: Arc<dyn PlaybookRunner>,
) -> ServiceContext {
    ServiceContext::new(db, notifier, Arc::new(MetricsRegistry::new()), playbook_runner)
}

/// 送信を記録するテスト用 notifier（`succeed` で成功/失敗を切り替える）。
pub struct RecordingNotifier {
    pub sent: Mutex<Vec<Notification>>,
    pub succeed: bool,
}

impl RecordingNotifier {
    /// 成功/失敗を指定して作る。
    pub fn new(succeed: bool) -> Self {
        Self {
            sent: Mutex::new(Vec::new()),
            succeed,
        }
    }

    /// 記録済み送信件数。
    pub fn count(&self) -> usize {
        self.sent.lock().expect("lock").len()
    }
}

#[async_trait]
impl Notifier for RecordingNotifier {
    async fn send(&self, notification: Notification) -> bool {
        self.sent.lock().expect("lock").push(notification);
        self.succeed
    }
}
