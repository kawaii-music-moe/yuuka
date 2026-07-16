//! Discord テナントの live ライフサイクルレジストリ（web ⇄ gateway の配線・N-lifecycle 解消）。
//!
//! カットオーバー前は [`yuuka_integrated::NullBotLifecycle`] / [`yuuka_admin::NullBotRuntime`] が
//! 「常に停止・start 常に false（=502）」を返す縮退だった。本モジュールはテナント（Shard poll
//! ループ）を **レジストリ所有のタスク**として管理し、web ルート（integrated / admin / settings）
//! から実テナントの稼働状態参照・起動・停止・再起動をできるようにする。
//!
//! # 監督（絶対制約2 の等価性）
//! 旧配線はテナントを [`Supervisor`](crate::Supervisor) のサービスとして置いたが、Supervisor は
//! 起動時固定集合のみで動的 start/stop ができない。本レジストリはテナント毎に
//! [`tenant_loop`] を spawn し、旧 `DiscordTenantService` と同じ分類で自前監督する:
//! - 一過性障害（gateway 断・transport error・panic）→ 指数バックオフ再起動
//! - 恒久クローズ（無効トークン等 `DiscordError::is_fatal`）→ 再起動しない（タスク終了）
//! - 停止協調（watch チャンネル）→ graceful 終了（close フレーム送出）
//!
//! # 排他
//! `start`/`stop` は `ops`（async Mutex）で直列化する（二重 start での孤児タスク防止）。
//! `slots` は同期 Mutex（await を跨いで保持しない）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use yuuka_core::BotId;
use yuuka_discord::{DiscordManager, DiscordMessenger, TenantRunner, TenantStatus};

/// 起動完了（READY）待ちの上限。Node の login 成功待ち相当。無効トークンは通常数秒で
/// 恒久クローズになるため、この窓内に「失敗確定」も「接続成功」も大半が収まる。
const START_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 停止協調（close フレーム送出）待ちの上限。超過時はタスクを abort する。
const STOP_GRACE: Duration = Duration::from_secs(8);
/// 一過性障害の再起動バックオフ（初期値 → 上限。旧 Supervisor RestartPolicy と同水準）。
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// 稼働中テナント 1 枠。
struct Slot {
    /// 停止協調シグナル（true 送信で tenant_loop が graceful 終了する）。
    cancel: watch::Sender<bool>,
    /// tenant_loop タスク。`is_finished` が「もう走っていない」判定（恒久クローズ後など）。
    join: JoinHandle<()>,
    /// gateway 接続状態（READY で true）。runner と共有。
    status: Arc<TenantStatus>,
}

/// Discord テナントの動的ライフサイクル管理（web 層への live 配線点）。
pub struct TenantRegistry {
    manager: Arc<DiscordManager>,
    messenger: Arc<DiscordMessenger>,
    slots: Mutex<HashMap<String, Slot>>,
    /// start/stop の直列化（二重 start による孤児タスク・レース防止）。
    ops: tokio::sync::Mutex<()>,
}

impl TenantRegistry {
    #[must_use]
    pub fn new(manager: Arc<DiscordManager>, messenger: Arc<DiscordMessenger>) -> Self {
        Self {
            manager,
            messenger,
            slots: Mutex::new(HashMap::new()),
            ops: tokio::sync::Mutex::new(()),
        }
    }

    /// slots ロック取得（poison は into_inner で回復。スロット表は panic 跨ぎでも整合する
    /// 単純 map で、部分更新の中間状態が無い）。
    fn slots(&self) -> std::sync::MutexGuard<'_, HashMap<String, Slot>> {
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 起動時テナント（`DiscordManager::prepare` の runner 群）をレジストリ管理下に置く。
    /// 接続完了は待たない（boot は非同期に READY を迎える・従来の supervisor 配置と同じ）。
    pub fn adopt(&self, runner: TenantRunner) {
        self.spawn_slot(runner);
    }

    /// 稼働状態 `(running, connected)`。running=タスク存命・connected=READY 受信済み。
    #[must_use]
    pub fn run_status(&self, bot_id: &str) -> (bool, bool) {
        let slots = self.slots();
        match slots.get(bot_id) {
            Some(s) if !s.join.is_finished() => (true, s.status.connected()),
            _ => (false, false),
        }
    }

    /// テナントを（再）起動する。既存タスクは先に graceful 停止（＝トークン更新も反映される）。
    /// READY まで最大 [`START_CONNECT_TIMEOUT`] 待ち、恒久クローズで死んだら false。
    /// トークン未登録も false（呼び出し側が「トークンを確認」応答にする）。
    pub async fn start(&self, bot_id: &str) -> bool {
        let _guard = self.ops.lock().await;
        self.stop_locked(bot_id).await;

        let Some(runner) = self
            .manager
            .prepare_runner(&BotId::new(bot_id), &self.messenger)
            .await
        else {
            tracing::warn!(bot_id, "起動失敗: トークン未登録（prepare_runner が None）");
            return false;
        };
        let status = runner.status();
        self.spawn_slot(runner);

        // READY か恒久終了を短時間ポーリング（状態セルは atomic・通知チャンネル無しで十分）。
        let deadline = tokio::time::Instant::now() + START_CONNECT_TIMEOUT;
        loop {
            if status.connected() {
                return true;
            }
            let finished = {
                let slots = self.slots();
                slots.get(bot_id).is_none_or(|s| s.join.is_finished())
            };
            if finished {
                tracing::warn!(bot_id, "起動失敗: テナントが接続前に終了（無効トークン等）");
                return false;
            }
            if tokio::time::Instant::now() >= deadline {
                // まだ接続試行中（ネットワーク遅延等）。タスクは残す＝起動要求は受理扱い。
                tracing::info!(
                    bot_id,
                    "起動受理: READY 待ちがタイムアウト（接続試行は継続）"
                );
                return true;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// テナントを graceful に停止する（close フレーム送出まで待つ・超過は abort）。
    pub async fn stop(&self, bot_id: &str) {
        let _guard = self.ops.lock().await;
        self.stop_locked(bot_id).await;
    }

    /// 全テナント停止（プロセス graceful shutdown 用）。
    pub async fn shutdown(&self) {
        let _guard = self.ops.lock().await;
        let ids: Vec<String> = {
            let slots = self.slots();
            slots.keys().cloned().collect()
        };
        for id in ids {
            self.stop_locked(&id).await;
        }
    }

    /// `ops` 保持前提の停止実体。slot を取り外し、cancel 送信 → join 待ち（超過 abort）。
    async fn stop_locked(&self, bot_id: &str) {
        let slot = {
            let mut slots = self.slots();
            slots.remove(bot_id)
        };
        let Some(slot) = slot else { return };
        let _ = slot.cancel.send(true);
        let mut join = slot.join;
        if tokio::time::timeout(STOP_GRACE, &mut join).await.is_err() {
            tracing::warn!(bot_id, "停止協調がタイムアウト。タスクを abort します");
            join.abort();
        }
    }

    /// runner を監督ループ付きタスクとして spawn し slot 登録する。
    fn spawn_slot(&self, runner: TenantRunner) {
        let bot_id = runner.bot_id().to_string();
        let status = runner.status();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let join = tokio::spawn(tenant_loop(Arc::new(runner), cancel_rx));
        let mut slots = self.slots();
        if let Some(old) = slots.insert(
            bot_id.clone(),
            Slot {
                cancel: cancel_tx,
                join,
                status,
            },
        ) {
            // start は stop_locked 済みなので通常到達しない（防御: 旧タスクを孤児にしない）。
            tracing::warn!(bot_id, "既存 slot を差し替え（旧タスクを停止）");
            let _ = old.cancel.send(true);
            old.join.abort();
        }
    }
}

/// 1 テナントの監督ループ（旧 `DiscordTenantService` + Supervisor 再起動政策の等価品）。
///
/// panic は attempt を内側タスクに隔離して `JoinError` として観測し、一過性障害と同様に
/// バックオフ再起動する（プロセスは落とさない・絶対制約2）。
async fn tenant_loop(runner: Arc<TenantRunner>, cancel: watch::Receiver<bool>) {
    let bot_id = runner.bot_id().to_string();
    let mut delay = BACKOFF_INITIAL;
    loop {
        let mut cancel_rx = cancel.clone();
        let r = runner.clone();
        let attempt = tokio::spawn(async move {
            r.run(async move {
                // 送信側 drop（レジストリ破棄）も停止扱い。
                let _ = cancel_rx.wait_for(|v| *v).await;
            })
            .await
        });
        match attempt.await {
            // graceful 停止 or ストリーム正常終了 → 再起動しない（旧 map_discord_result の Ok）。
            Ok(Ok(())) => break,
            // 恒久クローズ（無効トークン・DisallowedIntents 等）→ 再起動しない。
            Ok(Err(e)) if e.is_fatal() => {
                tracing::error!(bot_id, error = %e, "テナント恒久停止（再起動しない）");
                break;
            }
            // 一過性障害 → バックオフ再起動。
            Ok(Err(e)) => {
                tracing::warn!(bot_id, error = %e, delay_s = delay.as_secs(), "テナント再起動（一過性障害）");
            }
            // panic（JoinError）→ 一過性と同様にバックオフ再起動。
            Err(e) => {
                tracing::error!(bot_id, error = %e, delay_s = delay.as_secs(), "テナント panic。バックオフ再起動");
            }
        }
        if *cancel.borrow() {
            break;
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(BACKOFF_MAX);
        if *cancel.borrow() {
            break;
        }
    }
}

// ─── web 層トレイトへのアダプタ ─────────────────────────────────────────────────

/// [`yuuka_integrated::BotLifecycle`] の live 実装（統合管理 overview / start / stop / restart）。
pub struct RegistryLifecycle(pub Arc<TenantRegistry>);

#[async_trait::async_trait]
impl yuuka_integrated::BotLifecycle for RegistryLifecycle {
    fn run_status(&self, bot_id: &str) -> yuuka_integrated::BotRunStatus {
        let (running, connected) = self.0.run_status(bot_id);
        yuuka_integrated::BotRunStatus { running, connected }
    }

    async fn start(&self, bot_id: &str) -> bool {
        self.0.start(bot_id).await
    }

    async fn stop(&self, bot_id: &str) {
        self.0.stop(bot_id).await;
    }
}

/// [`yuuka_admin::BotRuntime`] の live 実装（admin デフォルト Bot 再起動・settings 所有 Bot 操作）。
///
/// `restart_default` の `token` 引数は使わない: 全呼び出し元が DB 保存後に呼ぶ契約のため、
/// レジストリが DB から復号し直す（プレーンテキストトークンの取り回しを増やさない）。
pub struct RegistryBotRuntime(pub Arc<TenantRegistry>);

#[async_trait::async_trait]
impl yuuka_admin::BotRuntime for RegistryBotRuntime {
    fn is_running(&self, bot_id: &str) -> bool {
        self.0.run_status(bot_id).0
    }

    async fn stop(&self, bot_id: &str) {
        self.0.stop(bot_id).await;
    }

    async fn restart_default(&self, _token: &str) {
        let _ = self.0.start(yuuka_core::BotId::SYSTEM_DEFAULT).await;
    }

    async fn start_custom(&self, bot_id: &str) {
        let _ = self.0.start(bot_id).await;
    }
}
