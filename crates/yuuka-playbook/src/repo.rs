//! `PlaybookRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制・§12.2 契約5）。
//!
//! 全クエリは `WHERE user_id = ? AND bot_id = ?` を必須にし、`&UserScope` を取ることで
//! 「user_id/bot_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。playbook はスコープ内で `name` が一意。

use rusqlite::{params, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{NewPlaybook, NewSchedule, Playbook, PlaybookRun, PlaybookSchedule};

/// 返却列（クリーンビュー・内部列 id/user_id/bot_id/created_at/updated_at は含めない）。
const PLAYBOOK_COLUMNS: &str = "name, title, keywords, description, steps";

/// スケジュール返却列（クリーンビュー・内部所有者列 `user_id` は含めない）。
const SCHEDULE_COLUMNS: &str = "id, bot_id, playbook_name, cron_expression, description, \
     enabled, last_run_at, next_run_at, created_at, updated_at";

/// 実行履歴返却列（クリーンビュー・内部所有者列 `user_id` は含めない）。
const RUN_COLUMNS: &str =
    "id, schedule_id, bot_id, playbook_name, status, output, started_at, finished_at";

/// Node `listRuns` の既定 LIMIT（`limit = 50`）。
const RUNS_DEFAULT_LIMIT: i64 = 50;

/// playbook リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct PlaybookRepo<'a> {
    pub(crate) read: &'a ReadPool,
    pub(crate) writer: &'a WriterHandle,
}

impl ScopedRepo for PlaybookRepo<'_> {}

impl<'a> PlaybookRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// スコープ内の playbook を更新日時降順で返す（`query` 指定時は部分一致で絞る）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(
        &self,
        scope: &UserScope,
        query: Option<String>,
    ) -> Result<Vec<Playbook>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let mut out = Vec::new();
                match query {
                    Some(q) if !q.is_empty() => {
                        let like = format!("%{q}%");
                        let sql = format!(
                            "SELECT {PLAYBOOK_COLUMNS} FROM playbooks \
                             WHERE user_id = ?1 AND bot_id = ?2 AND ( \
                               name LIKE ?3 OR title LIKE ?3 OR description LIKE ?3 \
                               OR steps LIKE ?3 OR keywords LIKE ?3 ) \
                             ORDER BY updated_at DESC, name ASC"
                        );
                        let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                        let rows = stmt
                            .query_map(params![uid, bid, like], row_to_playbook)
                            .map_err(map_sqlite)?;
                        for row in rows {
                            out.push(row.map_err(map_sqlite)?);
                        }
                    }
                    _ => {
                        let sql = format!(
                            "SELECT {PLAYBOOK_COLUMNS} FROM playbooks \
                             WHERE user_id = ?1 AND bot_id = ?2 \
                             ORDER BY updated_at DESC, name ASC"
                        );
                        let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                        let rows = stmt
                            .query_map(params![uid, bid], row_to_playbook)
                            .map_err(map_sqlite)?;
                        for row in rows {
                            out.push(row.map_err(map_sqlite)?);
                        }
                    }
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一 playbook を名前で取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, name: String) -> Result<Option<Playbook>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {PLAYBOOK_COLUMNS} FROM playbooks \
                     WHERE user_id = ?1 AND bot_id = ?2 AND name = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![uid, bid, name], row_to_playbook)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// playbook を upsert し、保存後の行を返す（Node `savePlaybook`）。
    ///
    /// `name` は Node 同様 `[^a-zA-Z0-9\-_]` を `_` に置換し小文字化する。正規化後が空なら
    /// [`DbError::Operation`]（route 層で 400 に変換）。
    ///
    /// # Errors
    /// 正規化後 name が空、または挿入／取得失敗時 [`DbError`]。
    pub async fn save(&self, scope: &UserScope, input: NewPlaybook) -> Result<Playbook, DbError> {
        let (uid, bid) = scope_keys(scope);
        let safe_name = normalize_name(&input.name);
        if safe_name.is_empty() {
            return Err(DbError::Operation("playbook name is invalid".to_owned()));
        }
        let name_for_get = safe_name.clone();
        self.writer
            .transaction(move |tx| {
                let keywords_json =
                    serde_json::to_string(&input.keywords).unwrap_or_else(|_| "[]".to_owned());
                tx.execute(
                    "INSERT INTO playbooks \
                       (user_id, bot_id, name, title, keywords, description, steps, \
                        created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, \
                             datetime('now', 'localtime'), datetime('now', 'localtime')) \
                     ON CONFLICT(user_id, bot_id, name) DO UPDATE SET \
                       title = excluded.title, \
                       keywords = excluded.keywords, \
                       description = excluded.description, \
                       steps = excluded.steps, \
                       updated_at = datetime('now', 'localtime')",
                    params![
                        uid,
                        bid,
                        safe_name,
                        input.title,
                        keywords_json,
                        input.description,
                        input.steps,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await?;
        self.get(scope, name_for_get)
            .await?
            .ok_or_else(|| DbError::Operation("saved playbook not found".to_owned()))
    }

    /// playbook を名前で削除する（削除できたら `true`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, name: String) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM playbooks WHERE user_id = ?1 AND bot_id = ?2 AND name = ?3",
                        params![uid, bid, name],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }

    // ── 定期実行スケジュール（Node `playbookScheduleService`） ──
    //
    // スケジュールの CRUD/toggle は Node と同じく **`user_id` スコープのみ**で束ねる
    // （`UNIQUE(user_id, playbook_name)`・`listSchedules(userId)` は bot 横断）。`bot_id` は
    // 実行結果の通知先として列に保持するだけで、行の同定キーには使わない。`&UserScope` を取る
    // のは route が常に `resolve_scope` で解決しているためだが、クエリでは `user_id` のみ使う。

    /// スコープ user のスケジュールを作成日時降順で返す（Node `listSchedules`・bot 横断）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_schedules(
        &self,
        scope: &UserScope,
    ) -> Result<Vec<PlaybookSchedule>, DbError> {
        let uid = scope.user_id().as_str().to_owned();
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {SCHEDULE_COLUMNS} FROM playbook_schedules \
                     WHERE user_id = ?1 ORDER BY created_at DESC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid], row_to_schedule)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スケジュールを upsert する（Node `upsertSchedule`）。
    ///
    /// Node パリティ: 対象 playbook が **bot スコープに存在しない**場合は `Ok(None)`（route が
    /// 「マクロが見つかりません」を 400 で返す）。`UNIQUE(user_id, playbook_name)` の
    /// `ON CONFLICT DO UPDATE` で `bot_id`/`cron_expression`/`description`/`enabled`/`updated_at`
    /// を上書きし、保存後の行を返す。cron 式の妥当性検証（`cron.validate`）は croner が本
    /// クレートの依存に無いため route/repo とも deferred。
    ///
    /// # Errors
    /// 挿入／取得失敗時 [`DbError`]。
    pub async fn upsert_schedule(
        &self,
        scope: &UserScope,
        input: NewSchedule,
    ) -> Result<Option<PlaybookSchedule>, DbError> {
        let (uid, bid) = scope_keys(scope);
        // Node `findPlaybooks(userId, botId).some(p => p.name === playbookName)` 相当の存在検証。
        if self
            .get(scope, input.playbook_name.clone())
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let name_for_get = input.playbook_name.clone();
        let get_uid = uid.clone();
        self.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO playbook_schedules \
                       (user_id, bot_id, playbook_name, cron_expression, description, enabled) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                     ON CONFLICT(user_id, playbook_name) DO UPDATE SET \
                       bot_id = excluded.bot_id, \
                       cron_expression = excluded.cron_expression, \
                       description = excluded.description, \
                       enabled = excluded.enabled, \
                       updated_at = datetime('now', 'localtime')",
                    params![
                        uid,
                        bid,
                        input.playbook_name,
                        input.cron_expression,
                        input.description,
                        i64::from(input.enabled),
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await?;
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {SCHEDULE_COLUMNS} FROM playbook_schedules \
                     WHERE user_id = ?1 AND playbook_name = ?2"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![get_uid, name_for_get], row_to_schedule)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// スケジュールの有効／無効を切り替える（Node `toggleSchedule`）。
    ///
    /// 所有者検証（`id` かつ `user_id` 一致）を SQL で束ね、更新できたら `true`。存在しない or
    /// 他人の id は `false`（route が「スケジュールが見つかりません」を 400 で返す）。cron の
    /// 再登録／停止（node-cron 相当）は実行エンジン未移植のため deferred。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn toggle_schedule(
        &self,
        scope: &UserScope,
        id: i64,
        enabled: bool,
    ) -> Result<bool, DbError> {
        let uid = scope.user_id().as_str().to_owned();
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "UPDATE playbook_schedules \
                         SET enabled = ?1, updated_at = datetime('now', 'localtime') \
                         WHERE id = ?2 AND user_id = ?3",
                        params![i64::from(enabled), id, uid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }

    /// スケジュールを削除する（Node `deleteSchedule`）。
    ///
    /// 所有者検証（`id` かつ `user_id` 一致）を SQL で束ね、削除できたら `true`。存在しない or
    /// 他人の id は `false`。cron ジョブ停止は実行エンジン未移植のため deferred。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete_schedule(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let uid = scope.user_id().as_str().to_owned();
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM playbook_schedules WHERE id = ?1 AND user_id = ?2",
                        params![id, uid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }

    /// 実行履歴を返す（Node `listRuns`・`user_id AND bot_id` スコープ・`started_at` 降順）。
    ///
    /// `schedule_id` 指定時はその schedule に絞る。件数は Node 既定 `LIMIT 50`。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_runs(
        &self,
        scope: &UserScope,
        schedule_id: Option<i64>,
    ) -> Result<Vec<PlaybookRun>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let mut out = Vec::new();
                match schedule_id {
                    Some(sid) => {
                        let sql = format!(
                            "SELECT {RUN_COLUMNS} FROM playbook_runs \
                             WHERE user_id = ?1 AND bot_id = ?2 AND schedule_id = ?3 \
                             ORDER BY started_at DESC LIMIT ?4"
                        );
                        let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                        let rows = stmt
                            .query_map(params![uid, bid, sid, RUNS_DEFAULT_LIMIT], row_to_run)
                            .map_err(map_sqlite)?;
                        for row in rows {
                            out.push(row.map_err(map_sqlite)?);
                        }
                    }
                    None => {
                        let sql = format!(
                            "SELECT {RUN_COLUMNS} FROM playbook_runs \
                             WHERE user_id = ?1 AND bot_id = ?2 \
                             ORDER BY started_at DESC LIMIT ?3"
                        );
                        let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                        let rows = stmt
                            .query_map(params![uid, bid, RUNS_DEFAULT_LIMIT], row_to_run)
                            .map_err(map_sqlite)?;
                        for row in rows {
                            out.push(row.map_err(map_sqlite)?);
                        }
                    }
                }
                Ok(out)
            })
            .await
    }
}

/// Node `savePlaybook` の name 正規化（`[^a-zA-Z0-9\-_]` を `_` に、小文字化）。
fn normalize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// スコープから所有 String キーを取り出す（`spawn_blocking` の `'static` クロージャ用）。
fn scope_keys(scope: &UserScope) -> (String, String) {
    (
        scope.user_id().as_str().to_owned(),
        scope.bot_id().as_str().to_owned(),
    )
}

/// SQLite 行を [`Playbook`] へ変換する（`keywords` は JSON 文字列 → `Vec<String>`）。
fn row_to_playbook(row: &Row) -> rusqlite::Result<Playbook> {
    let keywords_json: Option<String> = row.get("keywords")?;
    let keywords: Vec<String> = keywords_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    Ok(Playbook {
        name: row.get("name")?,
        title: row.get("title")?,
        keywords,
        description: row
            .get::<_, Option<String>>("description")?
            .unwrap_or_default(),
        steps: row.get::<_, Option<String>>("steps")?.unwrap_or_default(),
    })
}

/// SQLite 行を [`PlaybookSchedule`] へ変換する（`enabled` は 0/1 → bool・Node `rowToSchedule`）。
fn row_to_schedule(row: &Row) -> rusqlite::Result<PlaybookSchedule> {
    Ok(PlaybookSchedule {
        id: row.get("id")?,
        bot_id: row.get("bot_id")?,
        playbook_name: row.get("playbook_name")?,
        cron_expression: row.get("cron_expression")?,
        description: row
            .get::<_, Option<String>>("description")?
            .unwrap_or_default(),
        enabled: row.get::<_, i64>("enabled")? == 1,
        last_run_at: row.get("last_run_at")?,
        next_run_at: row.get("next_run_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// SQLite 行を [`PlaybookRun`] へ変換する。
fn row_to_run(row: &Row) -> rusqlite::Result<PlaybookRun> {
    Ok(PlaybookRun {
        id: row.get("id")?,
        schedule_id: row.get("schedule_id")?,
        bot_id: row.get("bot_id")?,
        playbook_name: row.get("playbook_name")?,
        status: row.get("status")?,
        output: row.get::<_, Option<String>>("output")?.unwrap_or_default(),
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
    })
}
