//! 自前 JoinSet スーパーバイザ（§5.2・絶対制約2）。
//!
//! 全長寿命サービス（web / discord テナント / cron / daemon）を [`JoinSet`] 配下に置き、
//! `join_next_with_id` で終了・panic を検知して**サービス個別に指数バックオフ再 spawn**する。
//! panic はタスク境界で隔離されプロセスを殺さない（`panic = "unwind"` 前提・§4.7）。
//!
//! 設計上の要点:
//! - **非ブロッキング再起動**: バックオフ待機を supervise ループ内で `sleep` せず、遅延を
//!   サービスタスク自身の先頭に載せて JoinSet へ再 spawn する（1 サービスの backoff 中に
//!   他サービスの障害検知が止まらない）。
//! - **`Fatality` 分類**: `Transient` のみ再起動、`Permanent` は停止＋アラート（恒久障害の
//!   無限スピンを避ける・§5.7）。panic は一過性とみなし再起動する。
//! - **安定稼働で backoff リセット**: `stable_after` 以上稼働してから失敗したサービスは
//!   backoff を初期化する（断続障害で遅延が際限なく伸びるのを防ぐ・§5.2 実装メモ）。
//! - **停止協調**: 外部 shutdown シグナルで `watch` フラグを立て、以後は再起動せず、猶予内に
//!   graceful 終了を待って残りを abort する（§5.4。tokio のみで実装し tgs 依存を持ち込まない）。

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use backon::{BackoffBuilder, ExponentialBackoff, ExponentialBuilder};
use tokio::sync::watch;
use tokio::task::{Id, JoinSet};
use yuuka_core::Fatality;

/// サービスループの回復可能エラー（**致命エラーは混ぜない**・§5.7）。
///
/// `ConfigError` 等の起動時 fail-fast は起動シーケンス（`main`）だけが扱い、supervise 下の
/// サービスは `Result<(), ServiceError>`（回復可能のみ）を返す不変条件を型で保証する。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ServiceError {
    /// 一過性障害。指数バックオフで再起動する。
    #[error("service transient failure: {0}")]
    Transient(String),
    /// 恒久障害（再起動で直らない）。停止＋アラート、再起動しない。
    #[error("service permanent failure: {0}")]
    Permanent(String),
}

impl ServiceError {
    /// 一過性障害を作る（再起動対象）。
    #[must_use]
    pub fn transient(msg: impl Into<String>) -> Self {
        Self::Transient(msg.into())
    }

    /// 恒久障害を作る（再起動しない）。
    #[must_use]
    pub fn permanent(msg: impl Into<String>) -> Self {
        Self::Permanent(msg.into())
    }

    /// supervisor 分類。`Transient` のみ再起動、`Permanent` は停止。
    #[must_use]
    pub fn fatality(&self) -> Fatality {
        match self {
            ServiceError::Transient(_) => Fatality::Transient,
            ServiceError::Permanent(_) => Fatality::Permanent,
        }
    }
}

/// shutdown 伝播トークン。各サービスへ配り、停止協調を観測させる（§5.4）。
#[derive(Clone)]
pub struct ShutdownToken(watch::Receiver<bool>);

impl ShutdownToken {
    /// 既に停止協調中か。
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        *self.0.borrow()
    }

    /// 停止協調が始まるまで待つ（サービスの `select!` 分岐に配線する）。
    pub async fn cancelled(&mut self) {
        // 既に true なら即返る。false の間は変化を待つ。
        while !*self.0.borrow() {
            if self.0.changed().await.is_err() {
                // 送信側 drop = supervisor 消失。安全側（停止扱い）で抜ける。
                return;
            }
        }
    }
}

/// 監督対象の長寿命サービス。
///
/// `run` は Ok(()) で「意図的停止（再起動しない）」、`Err(ServiceError)` で回復可能失敗を表す。
/// panic はタスク境界で隔離され、一過性障害として再起動される。
#[async_trait]
pub trait SupervisedService: Send + Sync {
    /// ログ・backoff 索引用の識別名。
    fn name(&self) -> String;

    /// 長寿命ループ本体。`shutdown` を `select!` に配線して協調停止する。
    async fn run(&self, shutdown: ShutdownToken) -> Result<(), ServiceError>;
}

/// 再起動ポリシー。
#[derive(Debug, Clone, Copy)]
pub struct RestartPolicy {
    pub min_delay: Duration,
    pub max_delay: Duration,
    /// これ以上連続稼働してから失敗したら backoff を初期化する。
    pub stable_after: Duration,
    /// 停止協調後、残タスクを強制 abort するまでの猶予。
    pub shutdown_grace: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            min_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(30),
            stable_after: Duration::from_secs(60),
            shutdown_grace: Duration::from_secs(20),
        }
    }
}

impl RestartPolicy {
    fn new_backoff(&self) -> ExponentialBackoff {
        ExponentialBuilder::default()
            .with_jitter() // 明示 ON（thundering herd 回避・§5.3）
            .with_min_delay(self.min_delay)
            .with_max_delay(self.max_delay)
            .without_max_times() // 常時稼働: 再起動回数の上限は設けない
            .build()
    }
}

/// スーパーバイザ。サービス群を保持し、`run` で監督ループを駆動する。
pub struct Supervisor {
    services: Vec<Arc<dyn SupervisedService>>,
    policy: RestartPolicy,
}

/// サービスタスクの戻り（idx と結果）。panic 時は JoinError の task id から idx を引く。
type TaskOutput = (usize, Result<(), ServiceError>);

/// サービス毎の backoff 状態＋直近 spawn 時刻。
struct BackoffState {
    backoff: ExponentialBackoff,
    spawned_at: Instant,
}

impl Supervisor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            services: Vec::new(),
            policy: RestartPolicy::default(),
        }
    }

    #[must_use]
    pub fn with_policy(mut self, policy: RestartPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// サービスを登録する。
    #[must_use]
    pub fn service(mut self, svc: Arc<dyn SupervisedService>) -> Self {
        self.services.push(svc);
        self
    }

    /// 監督ループを駆動する。`shutdown_signal` 完了で停止協調へ入る。
    ///
    /// パニックした・一過性エラーで終了したサービスを指数バックオフで再 spawn し続ける。
    /// Ok(()) 終了と `Permanent` 失敗は再起動しない。全サービスが停止したらループを抜ける。
    pub async fn run(self, shutdown_signal: impl Future<Output = ()> + Send) {
        let (tx, rx) = watch::channel(false);
        let mut set: JoinSet<TaskOutput> = JoinSet::new();
        let mut id_to_idx: HashMap<Id, usize> = HashMap::new();
        let mut states: HashMap<usize, BackoffState> = HashMap::new();

        // 初回 spawn（遅延なし）。
        for idx in 0..self.services.len() {
            self.spawn_service(
                &mut set,
                &mut id_to_idx,
                &mut states,
                &rx,
                idx,
                Duration::ZERO,
            );
        }

        tokio::pin!(shutdown_signal);

        loop {
            tokio::select! {
                // 外部 shutdown。以後は再起動しない（下の drain で残りを待つ）。
                () = &mut shutdown_signal => {
                    tracing::info!("supervisor: shutdown シグナル受信。停止協調を開始");
                    let _ = tx.send(true);
                    break;
                }
                joined = set.join_next_with_id() => {
                    let Some(joined) = joined else {
                        // 全サービス停止（意図的停止/恒久障害が出尽くした）。
                        tracing::info!("supervisor: 監督対象が全て停止");
                        return;
                    };
                    self.handle_joined(joined, &mut set, &mut id_to_idx, &mut states, &rx);
                }
            }
        }

        // ── 停止フェーズ: 猶予内に graceful 終了を待ち、残りを abort ──
        self.drain(set).await;
    }

    /// 1 サービスを（任意の初期遅延付きで）spawn し、id→idx 索引と backoff 状態を更新する。
    fn spawn_service(
        &self,
        set: &mut JoinSet<TaskOutput>,
        id_to_idx: &mut HashMap<Id, usize>,
        states: &mut HashMap<usize, BackoffState>,
        rx: &watch::Receiver<bool>,
        idx: usize,
        initial_delay: Duration,
    ) {
        let Some(svc) = self.services.get(idx).cloned() else {
            return;
        };
        let mut token = ShutdownToken(rx.clone());
        let handle = set.spawn(async move {
            // backoff 待機はサービスタスク側で行う（supervise ループを塞がない）。
            if !initial_delay.is_zero() {
                tokio::select! {
                    () = tokio::time::sleep(initial_delay) => {}
                    // 待機中に停止協調が来たら起動せず意図的停止扱いで抜ける。
                    () = token.cancelled() => return (idx, Ok(())),
                }
            }
            let result = svc.run(token).await;
            (idx, result)
        });
        id_to_idx.insert(handle.id(), idx);
        states.entry(idx).or_insert_with(|| BackoffState {
            backoff: self.policy.new_backoff(),
            spawned_at: Instant::now(),
        });
        // 再 spawn 時は spawned_at を更新（安定稼働判定の起点）。
        if let Some(state) = states.get_mut(&idx) {
            state.spawned_at = Instant::now();
        }
    }

    /// join 結果を分類し、必要なら再起動をスケジュールする。
    fn handle_joined(
        &self,
        joined: Result<(Id, TaskOutput), tokio::task::JoinError>,
        set: &mut JoinSet<TaskOutput>,
        id_to_idx: &mut HashMap<Id, usize>,
        states: &mut HashMap<usize, BackoffState>,
        rx: &watch::Receiver<bool>,
    ) {
        // 停止協調中は再起動しない（§5.4）。
        let shutting_down = *rx.borrow();

        let (idx, restart) = match joined {
            Ok((id, (idx, result))) => {
                id_to_idx.remove(&id);
                let name = self.name_of(idx);
                match result {
                    Ok(()) => {
                        tracing::info!(service = %name, "意図的停止（再起動しない）");
                        (idx, false)
                    }
                    Err(e) => match e.fatality() {
                        Fatality::Transient => {
                            tracing::warn!(service = %name, error = %e, "一過性障害・再起動します");
                            (idx, true)
                        }
                        // Permanent / Fatal は再起動しない（恒久障害の無限スピン回避）。
                        _ => {
                            tracing::error!(service = %name, error = %e, "恒久障害・停止（要アラート）");
                            (idx, false)
                        }
                    },
                }
            }
            Err(join_err) => {
                let idx = id_to_idx.remove(&join_err.id());
                match idx {
                    Some(idx) => {
                        let name = self.name_of(idx);
                        if join_err.is_panic() {
                            tracing::error!(service = %name, "PANIC をタスク境界で隔離・再起動します");
                            (idx, true)
                        } else {
                            // abort されたタスク（停止協調中など）。再起動しない。
                            tracing::debug!(service = %name, "タスク abort（再起動しない）");
                            (idx, false)
                        }
                    }
                    None => return, // 対応 idx 不明（既に処理済み）。
                }
            }
        };

        if !restart || shutting_down {
            return;
        }

        // 安定稼働していたら backoff をリセットしてから次遅延を決める。
        let delay = {
            let policy = self.policy;
            let state = states.entry(idx).or_insert_with(|| BackoffState {
                backoff: policy.new_backoff(),
                spawned_at: Instant::now(),
            });
            if state.spawned_at.elapsed() >= policy.stable_after {
                state.backoff = policy.new_backoff();
            }
            state.backoff.next().unwrap_or(policy.max_delay)
        };

        tracing::info!(service = %self.name_of(idx), ?delay, "backoff 後に再起動");
        self.spawn_service(set, id_to_idx, states, rx, idx, delay);
    }

    /// 停止フェーズ: 猶予内に graceful 終了を待ち、残りを abort する。
    async fn drain(&self, mut set: JoinSet<TaskOutput>) {
        let deadline = Instant::now() + self.policy.shutdown_grace;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, set.join_next()).await {
                Ok(Some(_)) => {}       // 1 タスク graceful 終了。次を待つ。
                Ok(None) => return,     // 全 graceful 終了。
                Err(_timeout) => break, // 猶予切れ。
            }
        }
        tracing::warn!("supervisor: graceful 猶予切れ。残タスクを abort します");
        set.shutdown().await; // 残り全 abort。
    }

    fn name_of(&self, idx: usize) -> String {
        self.services
            .get(idx)
            .map(|s| s.name())
            .unwrap_or_else(|| format!("service#{idx}"))
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
// panic-isolation を検証するテストは意図的に panic! を使う（clippy::panic はテスト例外設定が
// 無いためモジュール単位で許可する）。
#[allow(clippy::panic)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// 呼び出し回数を数え、指定挙動（panic/transient/permanent/stop-after-N）を演じるサービス。
    struct FlakyService {
        name: &'static str,
        starts: Arc<AtomicUsize>,
        behavior: Behavior,
    }

    #[derive(Clone, Copy)]
    enum Behavior {
        /// N 回目までは transient 失敗、その後は停止協調まで生きる。
        TransientUntil(usize),
        /// 毎回 panic（再起動され続ける）。
        AlwaysPanic,
        /// 1 回だけ permanent 失敗（再起動されない）。
        Permanent,
        /// 即 Ok(())（意図的停止・再起動されない）。
        StopImmediately,
        /// 停止協調まで生きる（正常サービス）。
        LiveUntilShutdown,
    }

    #[async_trait]
    impl SupervisedService for FlakyService {
        fn name(&self) -> String {
            self.name.to_owned()
        }
        async fn run(&self, mut shutdown: ShutdownToken) -> Result<(), ServiceError> {
            let n = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
            match self.behavior {
                Behavior::TransientUntil(k) => {
                    if n <= k {
                        return Err(ServiceError::transient(format!("fail#{n}")));
                    }
                    shutdown.cancelled().await;
                    Ok(())
                }
                Behavior::AlwaysPanic => {
                    panic!("boom#{n}");
                }
                Behavior::Permanent => Err(ServiceError::permanent("dead")),
                Behavior::StopImmediately => Ok(()),
                Behavior::LiveUntilShutdown => {
                    shutdown.cancelled().await;
                    Ok(())
                }
            }
        }
    }

    fn fast_policy() -> RestartPolicy {
        RestartPolicy {
            min_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            stable_after: Duration::from_secs(60),
            shutdown_grace: Duration::from_millis(200),
        }
    }

    #[tokio::test]
    async fn transient_failures_are_restarted_then_service_survives() {
        let starts = Arc::new(AtomicUsize::new(0));
        let svc = Arc::new(FlakyService {
            name: "flaky",
            starts: Arc::clone(&starts),
            behavior: Behavior::TransientUntil(3),
        });
        let sup = Supervisor::new().with_policy(fast_policy()).service(svc);

        // 3 回失敗→4 回目で生存。少し待ってから shutdown を送る。
        let (tx, rx) = watch::channel(false);
        let shutdown = async move {
            let mut rx = rx;
            let _ = rx.changed().await;
        };
        let handle = tokio::spawn(sup.run(shutdown));

        // 4 回目が起動して生きるまで待つ。
        for _ in 0..200 {
            if starts.load(Ordering::SeqCst) >= 4 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            starts.load(Ordering::SeqCst) >= 4,
            "3 失敗後に再起動して生存"
        );

        let _ = tx.send(true);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn panics_are_isolated_and_restarted() {
        let starts = Arc::new(AtomicUsize::new(0));
        let svc = Arc::new(FlakyService {
            name: "panicky",
            starts: Arc::clone(&starts),
            behavior: Behavior::AlwaysPanic,
        });
        let sup = Supervisor::new().with_policy(fast_policy()).service(svc);
        let (tx, rx) = watch::channel(false);
        let shutdown = async move {
            let mut rx = rx;
            let _ = rx.changed().await;
        };
        let handle = tokio::spawn(sup.run(shutdown));

        // panic が隔離され再起動され続ける（プロセスは死なない）＝複数回起動を観測。
        for _ in 0..200 {
            if starts.load(Ordering::SeqCst) >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            starts.load(Ordering::SeqCst) >= 3,
            "panic 後も再起動が続く（隔離されプロセスは生存）"
        );

        let _ = tx.send(true);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn permanent_failure_is_not_restarted() {
        let starts = Arc::new(AtomicUsize::new(0));
        let svc = Arc::new(FlakyService {
            name: "perma",
            starts: Arc::clone(&starts),
            behavior: Behavior::Permanent,
        });
        let sup = Supervisor::new().with_policy(fast_policy()).service(svc);
        // shutdown を送らなくても、恒久障害で全サービス停止→run は自然に return する。
        tokio::time::timeout(Duration::from_secs(2), sup.run(std::future::pending()))
            .await
            .expect("恒久障害で監督ループが自然終了する");
        assert_eq!(
            starts.load(Ordering::SeqCst),
            1,
            "恒久障害は 1 回のみ・再起動しない"
        );
    }

    #[tokio::test]
    async fn intentional_stop_is_not_restarted() {
        let starts = Arc::new(AtomicUsize::new(0));
        let svc = Arc::new(FlakyService {
            name: "stopper",
            starts: Arc::clone(&starts),
            behavior: Behavior::StopImmediately,
        });
        let sup = Supervisor::new().with_policy(fast_policy()).service(svc);
        tokio::time::timeout(Duration::from_secs(2), sup.run(std::future::pending()))
            .await
            .expect("意図的停止で監督ループが自然終了する");
        assert_eq!(starts.load(Ordering::SeqCst), 1, "Ok(()) は再起動しない");
    }

    #[tokio::test]
    async fn healthy_service_stops_gracefully_on_shutdown() {
        let starts = Arc::new(AtomicUsize::new(0));
        let svc = Arc::new(FlakyService {
            name: "healthy",
            starts: Arc::clone(&starts),
            behavior: Behavior::LiveUntilShutdown,
        });
        let sup = Supervisor::new().with_policy(fast_policy()).service(svc);
        let (tx, rx) = watch::channel(false);
        let shutdown = async move {
            let mut rx = rx;
            let _ = rx.changed().await;
        };
        let handle = tokio::spawn(sup.run(shutdown));

        // 起動を確認してから shutdown。
        for _ in 0..200 {
            if starts.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        let _ = tx.send(true);
        // graceful 猶予内に終了する。
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("shutdown で監督ループが終了")
            .unwrap();
    }
}
