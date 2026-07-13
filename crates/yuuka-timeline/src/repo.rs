//! `TimelineRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制・§12.2 契約5）。
//!
//! 全クエリは `WHERE user_id = ? AND bot_id = ?` を必須にし、`&UserScope` を取ることで
//! 「user_id/bot_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。対象表は `timeline_records`。

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{DayPlanBlock, NewDayPlanBlock, NewTimelineRecord, TimelineRecord, UpdatePlanBlock};

/// 返却列（クリーンビュー・内部列 user_id/bot_id は含めない）。
const RECORD_COLUMNS: &str = "id, date, recorded_at, type, title, content, todo_id, expense_id, \
     amount, expense_category, media_path, media_type, location, created_at";

/// 計画ブロックの返却列（クリーンビュー・内部列 user_id/bot_id は含めない）。
const PLAN_COLUMNS: &str = "id, date, start_time, end_time, type, title, description, todo_id, \
     transit_from, transit_to, transit_line, position, created_at, updated_at";

/// timeline リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct TimelineRepo<'a> {
    read: &'a ReadPool,
    writer: &'a WriterHandle,
}

impl ScopedRepo for TimelineRepo<'_> {}

impl<'a> TimelineRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// スコープ内・指定日の全記録を `recorded_at` 昇順で返す（Node `listTimelineRecords`）。
    ///
    /// `date` が `None`（未指定）なら **本日（UTC）** に畳む（Node の
    /// `date ?? new Date().toISOString().slice(0,10)` と一致。SQLite `date('now')` は UTC）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(
        &self,
        scope: &UserScope,
        date: Option<String>,
    ) -> Result<Vec<TimelineRecord>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {RECORD_COLUMNS} FROM timeline_records \
                     WHERE user_id = ?1 AND bot_id = ?2 AND date = COALESCE(?3, date('now')) \
                     ORDER BY recorded_at ASC, id ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid, date], row_to_record)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一記録を取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, id: i64) -> Result<Option<TimelineRecord>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {RECORD_COLUMNS} FROM timeline_records \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, uid, bid], row_to_record)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// 記録を作成し、作成後の行を返す（Node `addTimelineRecord`・プレーン記録経路）。
    ///
    /// `recorded_at` は未指定なら `datetime('now')`（**UTC**。Node は `new Date().toISOString()`
    /// ＝ UTC で入れるため localtime にすると +9h ずれる。`created_at` は Node 同様 localtime）。
    ///
    /// **cross-domain（task_done）**: `type=task_done` かつ `todo_id` 指定時は、同一トランザクション
    /// 内で先に紐付き todos を `done` へ更新する（Node route/tool の `completeTodo`・状態無条件更新＝
    /// `WHERE user_id AND bot_id AND id`。既に done でも冪等）。type=expense は [`Self::add_expense_record`]。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add(
        &self,
        scope: &UserScope,
        input: NewTimelineRecord,
    ) -> Result<TimelineRecord, DbError> {
        let (uid, bid) = scope_keys(scope);
        // Node: `type === "task_done" && typeof todoId === "number"` のときのみ completeTodo。
        let complete_todo_id = if input.r#type == "task_done" {
            input.todo_id
        } else {
            None
        };
        let id = self
            .writer
            .transaction(move |tx| {
                // task_done の紐付き todo を先に完了（Node completeTodo・無条件 UPDATE）。
                if let Some(todo_id) = complete_todo_id {
                    tx.execute(
                        "UPDATE todos SET status = 'done', updated_at = datetime('now', 'localtime') \
                         WHERE user_id = ?1 AND bot_id = ?2 AND id = ?3",
                        params![uid, bid, todo_id],
                    )
                    .map_err(map_sqlite)?;
                }
                // 内部列 expense_id / expense_category / media_path / media_type は INSERT 対象外
                // （列省略でデフォルト NULL）。クライアントからは設定不可（dto の M-8 注記）。
                tx.execute(
                    "INSERT INTO timeline_records \
                       (user_id, bot_id, date, recorded_at, type, title, content, \
                        todo_id, amount, location, created_at) \
                     VALUES (?1, ?2, ?3, \
                             COALESCE(?4, datetime('now')), \
                             ?5, ?6, ?7, ?8, ?9, ?10, \
                             datetime('now', 'localtime'))",
                    params![
                        uid,
                        bid,
                        input.date,
                        input.recorded_at,
                        input.r#type,
                        input.title,
                        input.content,
                        input.todo_id,
                        input.amount,
                        input.location,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted timeline record not found".to_owned()))
    }

    /// `type=expense` の記録を作成し、`expenses` にも二次登録する（Node `addExpenseRecord`）。
    ///
    /// 単一 writer トランザクションで原子的に: (1) `expenses` へ INSERT（`source='timeline'`・
    /// `type='expense'`・`memo=title`・`date=input.date`・`time` は `recorded_at.slice(11,19)`
    /// があればそれ、無ければ localtime）、(2) `timeline_records` へ INSERT（`type='expense'`・
    /// `expense_id`=(1) の rowid・`expense_category`・`amount` を連結）。
    /// finance/todo クレートを import せず（依存 DAG 維持）、[`crate::repo`] 内の raw SQL で束ねる
    /// （finance `settle_plan` と同じ cross-table・単一 tx パターン）。
    ///
    /// `amount` は Node の `Number(b.amount)` 相当（route/tool が検証済みの非 0 値）。`expenses.amount`
    /// は INTEGER 列だが SQLite の型親和性で損失なく整数化される（Node の格納と一致）。
    ///
    /// # Errors
    /// 各 INSERT 失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add_expense_record(
        &self,
        scope: &UserScope,
        input: &NewTimelineRecord,
        amount: f64,
        category: String,
    ) -> Result<TimelineRecord, DbError> {
        let (uid, bid) = scope_keys(scope);
        let date = input.date.clone();
        let recorded_at = input.recorded_at.clone();
        let title = input.title.clone();
        let location = input.location.clone();
        // Node: `expTime = recordedAt ? recordedAt.slice(11, 19) : undefined`（"HH:MM:SS"）。
        // JS slice のセマンティクス（Some のとき必ず値・範囲外は空文字）に合わせる: 空文字は SQL の
        // COALESCE では NULL 扱いにならず expenses.time="" として格納され Node と一致する。None（未指定）
        // のみ localtime に落ちる。get(11..19)（<19 文字で None→localtime）だと Node と食い違うため回避。
        let exp_time = recorded_at
            .as_deref()
            .map(|s| s.chars().skip(11).take(8).collect::<String>());
        // expenses.amount は INTEGER 列で read 側が i64 固定のため、非整数（円は整数運用）は round して
        // 整数を格納する（REAL 行が 1 件でも混ざると finance の list/get が i64 デコードで 500 に落ちる）。
        // timeline_records.amount は REAL 列のため raw f64 のまま（Node の値と一致）。
        let expense_amount = amount.round() as i64;
        let id = self
            .writer
            .transaction(move |tx| {
                // (1) expenses への二次登録（source='timeline'・memo=title・time は slice or localtime）。
                tx.execute(
                    "INSERT INTO expenses \
                       (user_id, bot_id, type, amount, category, memo, date, time, source, created_at) \
                     VALUES (?1, ?2, 'expense', ?3, ?4, ?5, ?6, \
                             COALESCE(?7, time('now', 'localtime')), \
                             'timeline', datetime('now', 'localtime'))",
                    params![uid, bid, expense_amount, category, title, date, exp_time],
                )
                .map_err(map_sqlite)?;
                let expense_id = tx.last_insert_rowid();
                // (2) timeline_records（expense_id 連結・expense_category・content/todo_id/media は NULL）。
                tx.execute(
                    "INSERT INTO timeline_records \
                       (user_id, bot_id, date, recorded_at, type, title, content, \
                        todo_id, expense_id, amount, expense_category, media_path, media_type, \
                        location, created_at) \
                     VALUES (?1, ?2, ?3, \
                             COALESCE(?4, datetime('now')), \
                             'expense', ?5, NULL, NULL, ?6, ?7, ?8, NULL, NULL, ?9, \
                             datetime('now', 'localtime'))",
                    params![uid, bid, date, recorded_at, title, expense_id, amount, category, location],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted expense timeline record not found".to_owned()))
    }

    /// `type=media` の記録を作成する（Node `addTimelineRecord` の mediaPath/mediaType 付き経路）。
    ///
    /// メディアファイルは呼び出し側（route/tool）が [`crate::media::save_media_base64`] 等で保存済みで、
    /// ここでは `media_path`（ファイル名）/`media_type`（photo|video）を含めて 1 行 INSERT する。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add_media_record(
        &self,
        scope: &UserScope,
        input: &NewTimelineRecord,
        media_path: String,
        media_type: String,
    ) -> Result<TimelineRecord, DbError> {
        let (uid, bid) = scope_keys(scope);
        let date = input.date.clone();
        let recorded_at = input.recorded_at.clone();
        let title = input.title.clone();
        let content = input.content.clone();
        let location = input.location.clone();
        let id = self
            .writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO timeline_records \
                       (user_id, bot_id, date, recorded_at, type, title, content, \
                        media_path, media_type, location, created_at) \
                     VALUES (?1, ?2, ?3, \
                             COALESCE(?4, datetime('now')), \
                             'media', ?5, ?6, ?7, ?8, ?9, \
                             datetime('now', 'localtime'))",
                    params![uid, bid, date, recorded_at, title, content, media_path, media_type, location],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted media timeline record not found".to_owned()))
    }

    /// 記録を削除する（削除できたら `true`。Node `deleteTimelineRecord`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM timeline_records WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
                        params![id, uid, bid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }

    // ── 計画ブロック（day_plan_blocks）───────────────────────────────────────────

    /// スコープ内・指定日の全計画ブロックを返す（Node `listDayPlanBlocks`）。
    ///
    /// `date` が `None`（未指定）なら **本日（UTC）** に畳む（[`Self::list`] と同一・`day`
    /// ルートの today フォールバックを SQL の `COALESCE(?3, date('now'))` で束ねる）。
    /// 並びは Node と同一: `start_time IS NULL` を後ろへ、次に `start_time ASC`、最後に
    /// `position ASC`（同点安定化のため `id ASC` を最後に付す）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_plans(
        &self,
        scope: &UserScope,
        date: Option<String>,
    ) -> Result<Vec<DayPlanBlock>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {PLAN_COLUMNS} FROM day_plan_blocks \
                     WHERE user_id = ?1 AND bot_id = ?2 AND date = COALESCE(?3, date('now')) \
                     ORDER BY \
                       CASE WHEN start_time IS NULL THEN 1 ELSE 0 END, \
                       start_time ASC, position ASC, id ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid, date], row_to_plan)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一計画ブロックを取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get_plan(
        &self,
        scope: &UserScope,
        id: i64,
    ) -> Result<Option<DayPlanBlock>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {PLAN_COLUMNS} FROM day_plan_blocks \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, uid, bid], row_to_plan)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// 計画ブロックを作成し、作成後の行を返す（Node `addDayPlanBlock`）。
    ///
    /// 未指定の任意列は NULL（`position` 未指定は Node 同様 `0`）。`created_at`/`updated_at` は
    /// 表 DEFAULT（`datetime('now','localtime')`）に委ねる。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add_plan(
        &self,
        scope: &UserScope,
        input: NewDayPlanBlock,
    ) -> Result<DayPlanBlock, DbError> {
        let (uid, bid) = scope_keys(scope);
        let position = input.position.unwrap_or(0);
        let id = self
            .writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO day_plan_blocks \
                       (user_id, bot_id, date, start_time, end_time, type, title, description, \
                        todo_id, transit_from, transit_to, transit_line, position) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        uid,
                        bid,
                        input.date,
                        input.start_time,
                        input.end_time,
                        input.r#type,
                        input.title,
                        input.description,
                        input.todo_id,
                        input.transit_from,
                        input.transit_to,
                        input.transit_line,
                        position,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get_plan(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted day plan block not found".to_owned()))
    }

    /// 計画ブロックを部分更新し、更新後の行を返す（無ければ `None`。Node `updateDayPlanBlock`）。
    ///
    /// Node の per-field 存在意味論を写す（[`UpdatePlanBlock`] のドキュメント参照）。`updated_at` は
    /// 常に `datetime('now','localtime')` に更新する。更新対象フィールドが 1 つも無くても Node は
    /// `updated_at` だけを更新するため `changes > 0`（該当行が在れば）→ 行を返す。
    ///
    /// # Errors
    /// 更新失敗・更新後の取得失敗時 [`DbError`]。
    pub async fn update_plan(
        &self,
        scope: &UserScope,
        input: UpdatePlanBlock,
    ) -> Result<Option<DayPlanBlock>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let id = input.id;
        let changed = self
            .writer
            .transaction(move |tx| {
                // Node と同順に SET 句を積む（updated_at を先頭に固定）。
                let mut sets: Vec<String> = vec!["updated_at = datetime('now','localtime')".to_owned()];
                let mut vals: Vec<SqlValue> = Vec::new();
                let mut next = 1;
                // string ガード系（title/type）: 値が在るときのみ更新。
                if let Some(title) = input.title {
                    sets.push(format!("title = ?{next}"));
                    vals.push(SqlValue::Text(title));
                    next += 1;
                }
                // description: 値が在るときのみ更新・空文字は NULL（Node `description || null`）。
                if let Some(description) = input.description {
                    sets.push(format!("description = ?{next}"));
                    vals.push(if description.is_empty() {
                        SqlValue::Null
                    } else {
                        SqlValue::Text(description)
                    });
                    next += 1;
                }
                if let Some(ty) = input.r#type {
                    sets.push(format!("type = ?{next}"));
                    vals.push(SqlValue::Text(ty));
                    next += 1;
                }
                // キー存在系（nullable）: 外側 Some でキー存在 → SET（`null` は SqlValue::Null）。
                push_present_text(&mut sets, &mut vals, &mut next, "start_time", input.start_time);
                push_present_text(&mut sets, &mut vals, &mut next, "end_time", input.end_time);
                push_present_int(&mut sets, &mut vals, &mut next, "todo_id", input.todo_id);
                push_present_text(&mut sets, &mut vals, &mut next, "transit_from", input.transit_from);
                push_present_text(&mut sets, &mut vals, &mut next, "transit_to", input.transit_to);
                push_present_text(&mut sets, &mut vals, &mut next, "transit_line", input.transit_line);

                let where_id = next;
                let where_uid = next + 1;
                let where_bid = next + 2;
                let sql = format!(
                    "UPDATE day_plan_blocks SET {} \
                     WHERE id = ?{where_id} AND user_id = ?{where_uid} AND bot_id = ?{where_bid}",
                    sets.join(", ")
                );
                vals.push(SqlValue::Integer(id));
                vals.push(SqlValue::Text(uid));
                vals.push(SqlValue::Text(bid));
                let n = tx
                    .execute(&sql, params_from_iter(vals.iter()))
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await?;
        if changed {
            self.get_plan(scope, id).await
        } else {
            Ok(None)
        }
    }

    /// 計画ブロックを削除する（削除できたら `true`。Node `deleteDayPlanBlock`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete_plan(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM day_plan_blocks WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
                        params![id, uid, bid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }
}

/// キー存在系（テキスト nullable）の SET 句を積む: 外側 `Some` でキー存在 → 更新（`null`→NULL）。
fn push_present_text(
    sets: &mut Vec<String>,
    vals: &mut Vec<SqlValue>,
    next: &mut usize,
    col: &str,
    field: Option<Option<String>>,
) {
    if let Some(inner) = field {
        sets.push(format!("{col} = ?{next}"));
        vals.push(inner.map_or(SqlValue::Null, SqlValue::Text));
        *next += 1;
    }
}

/// キー存在系（整数 nullable）の SET 句を積む: 外側 `Some` でキー存在 → 更新（`null`→NULL）。
fn push_present_int(
    sets: &mut Vec<String>,
    vals: &mut Vec<SqlValue>,
    next: &mut usize,
    col: &str,
    field: Option<Option<i64>>,
) {
    if let Some(inner) = field {
        sets.push(format!("{col} = ?{next}"));
        vals.push(inner.map_or(SqlValue::Null, SqlValue::Integer));
        *next += 1;
    }
}

/// スコープから所有 String キーを取り出す（`spawn_blocking` の `'static` クロージャ用）。
fn scope_keys(scope: &UserScope) -> (String, String) {
    (
        scope.user_id().as_str().to_owned(),
        scope.bot_id().as_str().to_owned(),
    )
}

/// SQLite 行を [`DayPlanBlock`] へ変換する。
fn row_to_plan(row: &Row) -> rusqlite::Result<DayPlanBlock> {
    Ok(DayPlanBlock {
        id: row.get("id")?,
        date: row.get("date")?,
        start_time: row.get("start_time")?,
        end_time: row.get("end_time")?,
        r#type: row.get("type")?,
        title: row.get("title")?,
        description: row.get("description")?,
        todo_id: row.get("todo_id")?,
        transit_from: row.get("transit_from")?,
        transit_to: row.get("transit_to")?,
        transit_line: row.get("transit_line")?,
        position: row.get("position")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// SQLite 行を [`TimelineRecord`] へ変換する。
fn row_to_record(row: &Row) -> rusqlite::Result<TimelineRecord> {
    Ok(TimelineRecord {
        id: row.get("id")?,
        date: row.get("date")?,
        recorded_at: row.get("recorded_at")?,
        r#type: row.get("type")?,
        title: row.get("title")?,
        content: row.get("content")?,
        todo_id: row.get("todo_id")?,
        expense_id: row.get("expense_id")?,
        amount: row.get("amount")?,
        expense_category: row.get("expense_category")?,
        media_path: row.get("media_path")?,
        media_type: row.get("media_type")?,
        location: row.get("location")?,
        created_at: row.get("created_at")?,
    })
}
