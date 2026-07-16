//! ユーザー単位 Google Drive バックアップの定期実行サービス（§8・現行 `backupService.ts`）。
//!
//! Node は `node-cron("15 * * * *")` で毎時 15 分に発火し、`listAllUserIds()` で全ユーザーを走査する。
//! 各ユーザーの `getUserBackupConfig` を読み、無効はスキップ、`intervalHours`（既定 24・最短 1）を経過
//! （`Date.now() - lastRun >= intervalMs`）していれば `runBackup(userId)`（一時 SQLite エクスポート →
//! ZIP → Drive アップロード → 世代管理 → `touchBackupLastRun`）を実行する。Rust も同モデルを踏襲する。
//!
//! ## スケジュールモデル（briefing/report との違い）
//! briefing/report は per-user cron（`schedule_cron` を毎分 `cron_matches_now` 判定）だが、**backup は
//! per-user cron ではなく固定間隔（`interval_hours`）+ `last_run_at` マーカー**で due 判定する（Node の
//! `backupTick` はこの間隔差分を見る）。よってスケジュールは固定 [`Schedule::Cron`]`("15 * * * *")`
//! （毎時 15 分に間隔判定）で、`last_run_at` の更新で冪等性（同一間隔内の二重実行防止）を担保する。
//!
//! ## 実バックアップ（export/zip/upload/prune/touch）は [`BackupRunner`] ポート越し
//! 実 Drive アップロードは `services → google` の逆依存を避けるため（[`crate::turn::PlaybookRunner`] と
//! 同思想）、services 所有のポート [`BackupRunner`] を supervisor（main.rs）で `GoogleBackupClient` へ
//! 橋渡しする。`last_run_at` の更新は `run_backup`（＝Node `runBackup` の `touchBackupLastRun`）が担う
//! ため、このサービスは due 判定と走査だけを持つ。未配線（`YUUKA_RUST_CRON` 無効/暗号鍵無し）時は
//! [`NullBackupRunner`] へ縮退し、走査はするが実行は常に失敗（Node の Google 未連携時と同じ非致命）。

use async_trait::async_trait;
use chrono::{DateTime, Local};
use yuuka_core::{CrossUserAccess, DbError};
use yuuka_db::map_sqlite;
use yuuka_web::Db;

use crate::context::ServiceContext;
use crate::schedule::{CronService, Schedule};

/// バックアップ間隔の下限（時間・Node `Math.max(1, intervalHours || 24)`）。
const MIN_INTERVAL_HOURS: i64 = 1;
/// 既定バックアップ間隔（時間・Node `intervalHours || 24`）。
const DEFAULT_INTERVAL_HOURS: i64 = 24;

/// 実バックアップ（export → zip → Drive アップロード → 世代管理 → last_run 更新）を起動するポート。
///
/// `services → google` の逆依存を避けるため services 所有（[`crate::turn::PlaybookRunner`] と同方式）。
/// 返り値はアップロード先ファイル URL（Node `runBackup` の返り値）。失敗は人間可読な文言を `Err`。
#[async_trait]
pub trait BackupRunner: Send + Sync {
    /// 指定ユーザーのデータを Drive へバックアップし、ファイル URL を返す（Node `runBackup`）。
    ///
    /// # Errors
    /// バックアップ処理（設定不正・Google 未連携・上流失敗）に失敗した場合、文言を `Err` で返す。
    async fn run_backup(&self, user_id: &str) -> Result<String, String>;
}

/// 未配線時の縮退ランナー（常に失敗＝走査はするが実行はされない・Node の Google 未連携と同じ非致命）。
pub struct NullBackupRunner;

#[async_trait]
impl BackupRunner for NullBackupRunner {
    async fn run_backup(&self, _user_id: &str) -> Result<String, String> {
        Err("Google Drive バックアップが未配線のため実行できません。".to_owned())
    }
}

/// 定期バックアップサービス。
pub struct BackupService;

#[async_trait]
impl CronService for BackupService {
    fn name(&self) -> &'static str {
        "backup"
    }

    fn schedule(&self) -> Schedule {
        // Node `cron.schedule("15 * * * *")`（毎時 15 分に間隔判定）。
        Schedule::Cron("15 * * * *")
    }

    fn run_on_start(&self) -> bool {
        // Node は node-cron 登録のみで起動時 catch-up しない。かつ due 判定は last_run_at ベースで
        // あり、起動時即実行しても間隔内なら各ユーザーはスキップされる（冪等）が、Node パリティで false。
        false
    }

    async fn tick(&self, ctx: &ServiceContext) {
        if let Err(e) = process_due_backups(ctx, Local::now()).await {
            tracing::error!(error = %e, "❌ バックアップ走査でエラー");
        }
    }
}

/// 1 ユーザーのバックアップ設定（Node `getUserBackupConfig` の due 判定に使う範囲）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct UserBackupState {
    user_id: String,
    enabled: bool,
    interval_hours: i64,
    last_run_at: Option<String>,
}

/// 全ユーザーを走査し、間隔を経過した有効ユーザーのバックアップを実行する（Node `backupTick`）。
async fn process_due_backups(ctx: &ServiceContext, now: DateTime<Local>) -> Result<(), DbError> {
    let states = list_backup_states(&ctx.db, ctx.cross).await?;
    for state in states {
        if !is_due(&state, now) {
            continue;
        }
        tracing::info!(user = %state.user_id, "⏰ [Backup] 定期バックアップを開始します");
        match ctx.backup.run_backup(&state.user_id).await {
            Ok(url) => {
                tracing::info!(user = %state.user_id, %url, "✅ [Backup] 定期バックアップ完了")
            }
            // 1 ユーザーの失敗で走査全体を止めない（Node の per-user try/catch）。last_run_at は
            // run_backup 成功時のみ更新されるため、失敗ユーザーは次 tick で再試行される。
            Err(e) => {
                tracing::error!(user = %state.user_id, error = %e, "❌ [Backup] 定期バックアップに失敗");
            }
        }
    }
    Ok(())
}

/// バックアップが due か（有効 + 間隔経過・Node `backupTick` の判定）。
///
/// `intervalMs = max(1, intervalHours || 24) * 3600_000`。`lastRunAt` を DB 文字列（ローカル）として
/// 解釈し、`now - lastRun >= interval` なら due。`lastRunAt` 不在（未実行）は常に due（Node `lastRun=0`）。
fn is_due(state: &UserBackupState, now: DateTime<Local>) -> bool {
    if !state.enabled {
        return false;
    }
    // Node: `Math.max(1, conf.intervalHours || 24)`。0/NULL は 24 へ、その後 1 で下限クランプ。
    let hours = if state.interval_hours <= 0 {
        DEFAULT_INTERVAL_HOURS
    } else {
        state.interval_hours
    }
    .max(MIN_INTERVAL_HOURS);
    let interval = chrono::Duration::hours(hours);

    let Some(last_run_raw) = state.last_run_at.as_deref().filter(|s| !s.is_empty()) else {
        return true; // 未実行（lastRun=0）は常に due。
    };
    let Some(last_run) = parse_local_datetime(last_run_raw) else {
        // 解釈不能な last_run は「未実行」とみなし due（Node は NaN の減算＝due 側に倒れる）。
        return true;
    };
    now - last_run >= interval
}

/// DB 文字列（`'YYYY-MM-DD HH:MM:SS'` または `'...T...'`）をローカル日時に解釈する。
fn parse_local_datetime(raw: &str) -> Option<DateTime<Local>> {
    use chrono::TimeZone;
    // Node は `new Date(lastRunAt.replace(" ", "T"))`。空白/T の両方を許容する。
    let normalized = raw.replace(' ', "T");
    let naive = chrono::NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S").ok()?;
    Local.from_local_datetime(&naive).single()
}

/// 全ユーザーのバックアップ設定を横断で読む（Node `listAllUserIds` + `getUserBackupConfig` を 1 クエリに）。
///
/// 横断アクセスは cron/バッチ起点でしか作れない（[`CrossUserAccess`] 証憑必須・§7.3）。briefing/report の
/// `list_enabled_*` と同規律で `UserScope` 経路から隔離する。無効ユーザーも含めて読み、due 判定は
/// [`is_due`] が担う（enabled=0 は false）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
async fn list_backup_states(
    db: &Db,
    _cross: CrossUserAccess,
) -> Result<Vec<UserBackupState>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT discord_id, backup_enabled, backup_interval_hours, backup_last_run_at \
                     FROM users WHERE backup_enabled = 1 ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(UserBackupState {
                        user_id: r.get(0)?,
                        enabled: r.get::<_, i64>(1)? != 0,
                        interval_hours: r.get(2)?,
                        last_run_at: r.get(3)?,
                    })
                })
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use chrono::TimeZone;
    use yuuka_web::Db;

    use super::*;
    use crate::test_support::ctx_with_backup;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("valid local time")
    }

    fn state(enabled: bool, interval: i64, last_run: Option<&str>) -> UserBackupState {
        UserBackupState {
            user_id: "u".to_owned(),
            enabled,
            interval_hours: interval,
            last_run_at: last_run.map(str::to_owned),
        }
    }

    #[test]
    fn disabled_is_never_due() {
        assert!(!is_due(&state(false, 24, None), at(2026, 7, 8, 12, 0)));
    }

    #[test]
    fn never_run_is_due() {
        // last_run 不在（未実行）は常に due。
        assert!(is_due(&state(true, 24, None), at(2026, 7, 8, 12, 0)));
        assert!(is_due(&state(true, 24, Some("")), at(2026, 7, 8, 12, 0)));
    }

    #[test]
    fn due_when_interval_elapsed() {
        // 24h 間隔・前回 24h+ 前 → due。
        let now = at(2026, 7, 8, 12, 0);
        assert!(is_due(&state(true, 24, Some("2026-07-07 11:00:00")), now));
    }

    #[test]
    fn not_due_within_interval() {
        // 24h 間隔・前回 1h 前 → まだ due でない。
        let now = at(2026, 7, 8, 12, 0);
        assert!(!is_due(&state(true, 24, Some("2026-07-08 11:00:00")), now));
    }

    #[test]
    fn interval_zero_defaults_to_24h() {
        // interval=0 → 24h 既定（Node `intervalHours || 24`）。前回 12h 前はまだ due でない。
        let now = at(2026, 7, 8, 12, 0);
        assert!(!is_due(&state(true, 0, Some("2026-07-08 00:00:00")), now));
        // 25h 前なら due。
        assert!(is_due(&state(true, 0, Some("2026-07-07 11:00:00")), now));
    }

    #[test]
    fn min_interval_one_hour() {
        // interval=1（下限）・前回 1h ちょうど前 → due（>=）。
        let now = at(2026, 7, 8, 12, 0);
        assert!(is_due(&state(true, 1, Some("2026-07-08 11:00:00")), now));
        // 30 分前 → まだ due でない。
        assert!(!is_due(&state(true, 1, Some("2026-07-08 11:30:00")), now));
    }

    #[test]
    fn iso_t_separator_is_parsed() {
        // `T` 区切りの last_run（Node は replace(" ","T") で正規化）も解釈できる。
        let now = at(2026, 7, 8, 12, 0);
        assert!(is_due(&state(true, 1, Some("2026-07-08T10:00:00")), now));
        assert!(!is_due(&state(true, 24, Some("2026-07-08T10:00:00")), now));
    }

    #[test]
    fn garbage_last_run_is_treated_as_due() {
        assert!(is_due(
            &state(true, 24, Some("not-a-date")),
            at(2026, 7, 8, 12, 0)
        ));
    }

    /// 実行された user_id を記録する [`BackupRunner`]。
    struct RecordingRunner {
        ran: Mutex<Vec<String>>,
        ok: bool,
    }

    #[async_trait]
    impl BackupRunner for RecordingRunner {
        async fn run_backup(&self, user_id: &str) -> Result<String, String> {
            self.ran.lock().unwrap().push(user_id.to_owned());
            if self.ok {
                Ok(format!("https://drive/{user_id}"))
            } else {
                Err("boom".to_owned())
            }
        }
    }

    /// 実 `users` スキーマ（migrations 適用）で DB を開き、指定行を writer 経由で仕込む。
    ///
    /// `users` は core テーブルで migrations（V17 baseline）が backup 列込みで作る。テスト用の
    /// 縮小 DDL を先置きすると `idx_users_username` 作成でスキーマ衝突するため、空ファイルから
    /// migrations に作らせてから INSERT する（NOT NULL の username/password_hash/salt はダミー値）。
    async fn seed(rows: &[(&str, i64, i64, Option<&str>)]) -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.path().join(format!("backup_svc_{seq}.sqlite"));
        // 空ファイルを作り、Db::open の writer で migrations を適用（実 users スキーマを得る）。
        rusqlite::Connection::open(&path).expect("empty file");
        let db = Db::open(&path).expect("open db");
        for (uid, enabled, interval, last_run) in rows {
            let (uid, last_run) = ((*uid).to_owned(), last_run.map(str::to_owned));
            let (enabled, interval) = (*enabled, *interval);
            db.writer
                .execute(move |conn| {
                    conn.execute(
                        "INSERT INTO users \
                         (discord_id, username, password_hash, salt, backup_enabled, \
                          backup_interval_hours, backup_last_run_at) \
                         VALUES (?1, ?1, 'x', 'x', ?2, ?3, ?4)",
                        rusqlite::params![uid, enabled, interval, last_run],
                    )
                    .map_err(yuuka_db::map_sqlite)?;
                    Ok(())
                })
                .await
                .expect("seed row");
        }
        (db, dir)
    }

    #[tokio::test]
    async fn scans_and_runs_only_due_enabled_users() {
        // alice: enabled, never run → due. bob: enabled but ran 1h ago w/ 24h interval → not due.
        // carol: disabled → excluded by SQL. dave: enabled, ran 25h ago → due.
        let now = at(2026, 7, 8, 12, 0);
        let (db, _dir) = seed(&[
            ("alice", 1, 24, None),
            ("bob", 1, 24, Some("2026-07-08 11:00:00")),
            ("carol", 0, 24, None),
            ("dave", 1, 24, Some("2026-07-07 10:00:00")),
        ])
        .await;
        let runner = Arc::new(RecordingRunner {
            ran: Mutex::new(Vec::new()),
            ok: true,
        });
        let ctx = ctx_with_backup(db, runner.clone());

        process_due_backups(&ctx, now).await.expect("scan ok");

        let mut ran = runner.ran.lock().unwrap().clone();
        ran.sort();
        assert_eq!(ran, vec!["alice".to_owned(), "dave".to_owned()]);
    }

    #[tokio::test]
    async fn one_failure_does_not_stop_scan() {
        let now = at(2026, 7, 8, 12, 0);
        let (db, _dir) = seed(&[("alice", 1, 24, None), ("bob", 1, 24, None)]).await;
        // 失敗ランナーでも両ユーザーが試行される（per-user try/catch）。
        let runner = Arc::new(RecordingRunner {
            ran: Mutex::new(Vec::new()),
            ok: false,
        });
        let ctx = ctx_with_backup(db, runner.clone());

        process_due_backups(&ctx, now)
            .await
            .expect("scan ok despite failures");
        assert_eq!(runner.ran.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn does_not_run_on_start() {
        assert!(!BackupService.run_on_start());
    }

    #[test]
    fn schedule_is_hourly_at_15() {
        assert!(matches!(
            BackupService.schedule(),
            Schedule::Cron("15 * * * *")
        ));
    }
}
