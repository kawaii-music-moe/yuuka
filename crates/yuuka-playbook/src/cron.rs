//! cron（マクロ定期実行）用の**全ユーザー横断走査**（§3.6・現行 `playbookScheduleService` の
//! `startPlaybookScheduleService` の全件読み込み・`createRun`/`finishRun`/`updateLastRun`）。
//!
//! 通常の `UserScope` 経路から**隔離**するため、各メソッドは [`CrossUserAccess`] 証憑を要求する
//! （横断アクセスは cron/バッチ起点でしか作れない・§7.3）。cron 式の due 判定は croner を持つ
//! 上位（`yuuka-services`）が [`DueSchedule`] を受けて評価する（本 crate は croner 非依存を保つ）。

use rusqlite::{params, Row};
use yuuka_core::{CrossUserAccess, DbError};
use yuuka_db::map_sqlite;

use crate::repo::PlaybookRepo;

/// 有効な（`enabled=1`）定期実行スケジュール（cron 専用・内部所有者列を含む）。
///
/// due 判定に必要な `cron_expression`/`last_run_at`/`created_at`、実行に必要な所有者 `user_id`・
/// 通知先 `bot_id`・対象 `playbook_name` を持つ（クリーンビュー `PlaybookSchedule` とは別型）。
#[derive(Debug, Clone)]
pub struct DueSchedule {
    pub id: i64,
    pub user_id: String,
    pub bot_id: String,
    pub playbook_name: String,
    pub cron_expression: String,
    pub last_run_at: Option<String>,
    pub created_at: String,
}

const ENABLED_COLUMNS: &str =
    "id, user_id, bot_id, playbook_name, cron_expression, last_run_at, created_at";

impl PlaybookRepo<'_> {
    /// 有効なスケジュールを**全ユーザー横断**で返す（Node `startPlaybookScheduleService` の
    /// `SELECT * FROM playbook_schedules WHERE enabled = 1`）。due 判定は呼び出し側。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_enabled_schedules(
        &self,
        _cron: CrossUserAccess,
    ) -> Result<Vec<DueSchedule>, DbError> {
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {ENABLED_COLUMNS} FROM playbook_schedules \
                     WHERE enabled = 1 ORDER BY id ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt.query_map([], row_to_due).map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// 実行を開始し `playbook_runs` に `running` 行を作る（Node `createRun`）。run id を返す。
    ///
    /// # Errors
    /// 挿入失敗時 [`DbError`]。
    pub async fn record_run_start(
        &self,
        _cron: CrossUserAccess,
        schedule_id: i64,
        user_id: String,
        bot_id: String,
        playbook_name: String,
    ) -> Result<i64, DbError> {
        self.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO playbook_runs \
                       (schedule_id, user_id, bot_id, playbook_name, status) \
                     VALUES (?1, ?2, ?3, ?4, 'running')",
                    params![schedule_id, user_id, bot_id, playbook_name],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await
    }

    /// 実行を確定する（Node `finishRun`・`status`/`output`/`finished_at` を更新）。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn record_run_finish(
        &self,
        _cron: CrossUserAccess,
        run_id: i64,
        status: &str,
        output: String,
    ) -> Result<(), DbError> {
        let status = status.to_owned();
        self.writer
            .transaction(move |tx| {
                tx.execute(
                    "UPDATE playbook_runs \
                     SET status = ?1, output = ?2, finished_at = datetime('now', 'localtime') \
                     WHERE id = ?3",
                    params![status, output, run_id],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }

    /// スケジュールの `last_run_at` を現在時刻へ更新する（Node `updateLastRun`）。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn touch_last_run(
        &self,
        _cron: CrossUserAccess,
        schedule_id: i64,
    ) -> Result<(), DbError> {
        self.writer
            .transaction(move |tx| {
                tx.execute(
                    "UPDATE playbook_schedules \
                     SET last_run_at = datetime('now', 'localtime'), \
                         updated_at = datetime('now', 'localtime') \
                     WHERE id = ?1",
                    params![schedule_id],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }
}

fn row_to_due(row: &Row) -> rusqlite::Result<DueSchedule> {
    Ok(DueSchedule {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        bot_id: row.get("bot_id")?,
        playbook_name: row.get("playbook_name")?,
        cron_expression: row.get("cron_expression")?,
        last_run_at: row.get("last_run_at")?,
        created_at: row.get("created_at")?,
    })
}
