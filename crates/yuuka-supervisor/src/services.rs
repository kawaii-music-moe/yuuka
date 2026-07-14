//! cron サービスを [`SupervisedService`] 化するアダプタ（§5.2・絶対制約2）。
//!
//! `yuuka-services` は supervisor に依存しない（DAG: `supervisor → services`）。本モジュールが
//! [`CronService`] を [`SupervisedService`] でラップし、[`run_cron`] の長寿命ループを協調停止付きで
//! 回す。tick 失敗はループ内で吸収されるため（自己復帰）、`run` は通常 shutdown での正常終了
//! （`Ok(())`）のみを返す。tick が panic した場合はタスク境界で supervisor が隔離・再起動する
//! （discord テナントと同じ扱い）。

use std::sync::Arc;

use async_trait::async_trait;
use yuuka_services::{run_cron, CronService, ServiceContext};

use crate::supervisor::{ServiceError, ShutdownToken, SupervisedService};

/// 1 つの cron サービスを監督下タスク化するアダプタ。
pub struct CronSupervised {
    svc: Arc<dyn CronService>,
    ctx: ServiceContext,
}

impl CronSupervised {
    /// サービスと実行コンテキストからアダプタを作る。
    #[must_use]
    pub fn new(svc: Arc<dyn CronService>, ctx: ServiceContext) -> Self {
        Self { svc, ctx }
    }
}

#[async_trait]
impl SupervisedService for CronSupervised {
    fn name(&self) -> String {
        format!("cron:{}", self.svc.name())
    }

    async fn run(&self, mut shutdown: ShutdownToken) -> Result<(), ServiceError> {
        // 停止協調は cancel future として cron ループの `select!` へ配線する。
        run_cron(
            &*self.svc,
            &self.ctx,
            async move { shutdown.cancelled().await },
        )
        .await;
        // cron ループは cancel でのみ抜ける＝意図的停止（再起動しない）。
        Ok(())
    }
}

/// 全 cron サービス（[`yuuka_services::build_services`]）を監督下タスクへ変換する。
///
/// 監督下 spawn は strangler カットオーバー時（Node cron 停止 → Rust cron 起動）にのみ行う。
/// main は環境変数ゲートでこの登録を制御する（既定は無効＝Node が cron を所有・二重 writer 回避）。
#[must_use]
pub fn build_supervised_services(ctx: &ServiceContext) -> Vec<Arc<dyn SupervisedService>> {
    yuuka_services::build_services()
        .into_iter()
        .map(|svc| Arc::new(CronSupervised::new(svc, ctx.clone())) as Arc<dyn SupervisedService>)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use yuuka_services::{MetricsRegistry, NullNotifier, NullPlaybookRunner, ServiceContext};
    use yuuka_web::Db;

    use super::{build_supervised_services, CronSupervised};
    use crate::supervisor::SupervisedService;

    fn bare_db() -> Db {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("yuuka_svc_sup_{}_{seq}.sqlite", std::process::id()));
        // Db::open は既存ファイル前提（read/writer は CREATE しない）。先にファイルを作る。
        rusqlite::Connection::open(&path).expect("create db file");
        // 空 DB で十分（アダプタ生成・名前確認は DB を触らない）。
        Db::open(&path).expect("open db")
    }

    fn ctx() -> ServiceContext {
        ServiceContext::new(
            bare_db(),
            Arc::new(NullNotifier),
            Arc::new(MetricsRegistry::new()),
            Arc::new(NullPlaybookRunner),
        )
    }

    #[test]
    fn builds_all_services_with_cron_prefixed_names() {
        let svcs = build_supervised_services(&ctx());
        // 実装 7（playbook 含む）+ 予約シーム 3（report/briefing/backup）= 10。
        assert_eq!(svcs.len(), 10);
        let names: Vec<String> = svcs.iter().map(|s| s.name()).collect();
        assert!(names.iter().all(|n| n.starts_with("cron:")));
        assert!(names.contains(&"cron:reminder".to_owned()));
        assert!(names.contains(&"cron:metrics".to_owned()));
        assert!(names.contains(&"cron:backup".to_owned()));
    }

    #[test]
    fn adapter_name_is_prefixed() {
        let svcs = yuuka_services::build_services();
        let first = svcs.into_iter().next().expect("at least one service");
        let name = first.name().to_owned();
        let adapter = CronSupervised::new(first, ctx());
        assert_eq!(adapter.name(), format!("cron:{name}"));
    }
}
