//! `TodoRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制・§12.2 契約5）。
//!
//! 全クエリは `WHERE user_id = ? AND bot_id = ?` を必須にし、`&UserScope` を取ることで
//! 「user_id/bot_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。

use std::collections::{HashMap, HashSet};

use rusqlite::{params, params_from_iter, OptionalExtension, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{NewTodo, PriorityUpdate, TaskProgressLog, Todo, TodoUpdate, TodoWithSubtasks};

/// 返却列（クリーンビュー・内部列 user_id/bot_id 等は含めない）。
const TODO_COLUMNS: &str = "id, title, description, due_date, start_date, priority, tags, status, \
     progress, parent_id, repeat_rule, repeat_until, repeat_count, created_at, updated_at";

/// [`TODO_COLUMNS`] を `todos.` 修飾したもの（`tree` CTE との JOIN で `id` が曖昧にならないよう）。
/// 出力列名は修飾を外した `id`/`title`… になるため [`row_to_todo`] はそのまま使える。
const TODO_COLUMNS_QUALIFIED: &str =
    "todos.id, todos.title, todos.description, todos.due_date, todos.start_date, todos.priority, \
     todos.tags, todos.status, todos.progress, todos.parent_id, todos.repeat_rule, \
     todos.repeat_until, todos.repeat_count, todos.created_at, todos.updated_at";

/// 一覧共通の並び順（Node `ORDER_CLAUSE`）: 優先度（high→medium→low→未設定）→ 期限近い順
/// （期限なしは後ろ）→ 作成日時降順。
const ORDER_CLAUSE: &str = " ORDER BY \
     CASE priority WHEN 'high' THEN 0 WHEN 'medium' THEN 1 WHEN 'low' THEN 2 ELSE 3 END, \
     CASE WHEN due_date IS NULL THEN 1 ELSE 0 END, \
     datetime(due_date) ASC, \
     created_at DESC";

/// todo リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct TodoRepo<'a> {
    pub(crate) read: &'a ReadPool,
    pub(crate) writer: &'a WriterHandle,
}

impl ScopedRepo for TodoRepo<'_> {}

impl<'a> TodoRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// 親タスク一覧をサブタスク（ネスト）＋算出進捗付きで返す（Node `listTodoTree`・H-3）。
    ///
    /// `status`/`tag` フィルタは**親に適用**し、サブタスクは状態に関わらず全同梱する（進捗算出のため）。
    /// `status` は `"open"`/`"done"` を指定（`"all"` または `None` は絞り込みなし）。ソートは
    /// [`ORDER_CLAUSE`]。フィルタ済みルート＋全子孫を再帰 CTE で 1 クエリ収集し、Rust でネスト化する。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_tree(
        &self,
        scope: &UserScope,
        status: Option<String>,
        tag: Option<String>,
    ) -> Result<Vec<TodoWithSubtasks>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                // 動的フィルタ。?1=uid ?2=bid（全段で再利用）、?3~ にフィルタ値を割り当てる。
                let mut filters = String::new();
                let mut extra: Vec<String> = Vec::new();
                let mut next = 3;
                if let Some(s) = status.filter(|s| s != "all") {
                    filters.push_str(&format!(" AND status = ?{next}"));
                    extra.push(s);
                    next += 1;
                }
                if let Some(t) = tag {
                    filters.push_str(&format!(
                        " AND EXISTS (SELECT 1 FROM json_each(todos.tags) \
                          WHERE json_each.value = ?{next})"
                    ));
                    extra.push(t);
                }
                // フィルタ済みルート親＋その全子孫を 1 クエリで収集（scope 検査を全段に付与）。
                let sql = format!(
                    "WITH RECURSIVE tree(id) AS ( \
                       SELECT id FROM todos \
                         WHERE user_id = ?1 AND bot_id = ?2 AND parent_id IS NULL{filters} \
                       UNION ALL \
                       SELECT t.id FROM todos t JOIN tree ON t.parent_id = tree.id \
                         WHERE t.user_id = ?1 AND t.bot_id = ?2 \
                     ) \
                     SELECT {TODO_COLUMNS_QUALIFIED} FROM todos JOIN tree ON todos.id = tree.id \
                     WHERE todos.user_id = ?1 AND todos.bot_id = ?2{ORDER_CLAUSE}"
                );
                let mut all_params: Vec<String> = vec![uid, bid];
                all_params.extend(extra);
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params_from_iter(all_params.iter()), row_to_todo)
                    .map_err(map_sqlite)?;
                let mut flat = Vec::new();
                for row in rows {
                    flat.push(row.map_err(map_sqlite)?);
                }
                Ok(build_todo_tree(flat))
            })
            .await
    }

    /// スコープ内の単一 todo を取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, id: i64) -> Result<Option<Todo>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {TODO_COLUMNS} FROM todos \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, uid, bid], row_to_todo)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// 未完了 ToDo に付いた全タグを（重複ありで）平坦に返す（Node `listAllTags` の集計元）。
    ///
    /// `tags` は JSON 配列列（例 `["買い物","緊急"]`）。行ごとに parse して連結する。集計（件数・
    /// 並び）は呼び出し側で行う。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn open_tags(&self, scope: &UserScope) -> Result<Vec<String>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT tags FROM todos \
                         WHERE user_id = ?1 AND bot_id = ?2 AND status = 'open'",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid], |row| row.get::<_, String>(0))
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    let raw = row.map_err(map_sqlite)?;
                    // 破損した JSON は空扱い（握り潰さず空配列にフォールバック・集計を落とさない）。
                    let tags: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
                    out.extend(tags);
                }
                Ok(out)
            })
            .await
    }

    /// todo のタグを丸ごと置き換え、更新後の行を返す（Node `updateTodoTags`）。該当無は `None`。
    ///
    /// `tags` は JSON 配列文字列として保存する（正規化は呼び出し側で済ませる）。
    ///
    /// # Errors
    /// 書き込み・取得失敗時 [`DbError`]。
    pub async fn update_tags(
        &self,
        scope: &UserScope,
        id: i64,
        tags: Vec<String>,
    ) -> Result<Option<Todo>, DbError> {
        let (uid, bid) = scope_keys(scope);
        // タグ配列を JSON 文字列へ（固定形状なので失敗し得ないが lint 準拠で握らず伝播）。
        let tags_json =
            serde_json::to_string(&tags).map_err(|e| DbError::Operation(format!("tags: {e}")))?;
        let changed = self
            .writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "UPDATE todos SET tags = ?1, updated_at = datetime('now', 'localtime') \
                         WHERE id = ?2 AND user_id = ?3 AND bot_id = ?4",
                        params![tags_json, id, uid, bid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await?;
        if changed {
            self.get(scope, id).await
        } else {
            Ok(None)
        }
    }

    /// ルーチン（繰り返し）を終了する（Node `stopRoutine`）。`repeat_rule`/`_until`/`_count` を NULL 化。
    ///
    /// 対象がルーチン（`repeat_rule IS NOT NULL`）でなければ・存在しなければ `None`（単発タスクは
    /// そのまま残る）。更新後の行を返す。
    ///
    /// # Errors
    /// 書き込み・取得失敗時 [`DbError`]。
    pub async fn stop_routine(&self, scope: &UserScope, id: i64) -> Result<Option<Todo>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let changed = self
            .writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "UPDATE todos SET repeat_rule = NULL, repeat_until = NULL, \
                         repeat_count = NULL, updated_at = datetime('now', 'localtime') \
                         WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3 AND repeat_rule IS NOT NULL",
                        params![id, uid, bid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await?;
        if changed {
            self.get(scope, id).await
        } else {
            Ok(None)
        }
    }

    /// todo を作成し、作成後の行を返す。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add(&self, scope: &UserScope, input: NewTodo) -> Result<Todo, DbError> {
        let (uid, bid) = scope_keys(scope);
        let id = self
            .writer
            .transaction(move |tx| {
                let tags_json =
                    serde_json::to_string(&input.tags).unwrap_or_else(|_| "[]".to_owned());
                // parent_id はスコープ内に実在する場合のみ採用（Node normalizeParentId）。
                // 後付け列 parent_id は FK が効かないため、他スコープ/不在 id はここで NULL に降格し
                // クロススコープ参照を防ぐ。
                let parent_id = match input.parent_id {
                    Some(pid) => tx
                        .query_row(
                            "SELECT id FROM todos WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
                            params![pid, uid, bid],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(map_sqlite)?,
                    None => None,
                };
                // ルーチン列は repeat_rule がある時のみ有効。かつ **サブタスク（parent_id あり）は
                // ルーチンにしない**（親のみ繰り返し対象・Node addTodo `todoRepo.ts:176-178`
                // `parentId == null ? repeatRule : null`）。この parent_id ゲートが無いと、
                // サブタスクに repeat_* が残り recurrence サービスが子タスクを複製してしまう。
                let (repeat_rule, repeat_until, repeat_count) = match (parent_id, &input.repeat_rule) {
                    (None, Some(rule)) => (
                        Some(rule.clone()),
                        input.repeat_until.clone(),
                        input.repeat_count,
                    ),
                    _ => (None, None, None),
                };
                tx.execute(
                    "INSERT INTO todos \
                       (user_id, bot_id, title, description, due_date, start_date, priority, tags, \
                        parent_id, repeat_rule, repeat_until, repeat_count, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, \
                             datetime('now', 'localtime'), datetime('now', 'localtime'))",
                    params![
                        uid,
                        bid,
                        input.title,
                        input.description,
                        input.due_date,
                        input.start_date,
                        input.priority,
                        tags_json,
                        parent_id,
                        repeat_rule,
                        repeat_until,
                        repeat_count,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted todo not found".to_owned()))
    }

    /// todo を完了（status=done, progress=100）にし、更新後の行を返す（無ければ `None`）。
    ///
    /// # Errors
    /// 更新・取得失敗時 [`DbError`]。
    pub async fn complete(&self, scope: &UserScope, id: i64) -> Result<Option<Todo>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let changed = self
            .writer
            .transaction(move |tx| {
                // Node completeTodo は status/updated_at のみ更新し progress は不変。
                let n = tx
                    .execute(
                        "UPDATE todos SET status = 'done', \
                         updated_at = datetime('now', 'localtime') \
                         WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
                        params![id, uid, bid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await?;
        if changed {
            self.get(scope, id).await
        } else {
            Ok(None)
        }
    }

    /// todo を削除する（全子孫も再帰的に削除・削除できたら `true`）。
    ///
    /// **M-6（連鎖削除 parity）**: Node `deleteTodo`（[`src/db/todoRepo.ts`] `:364-381`）は
    /// `WITH RECURSIVE descendants` で全子孫 id を収集してから一括 DELETE する。`parent_id` は
    /// 後付け列で FK `ON DELETE CASCADE` が効かないため、単一行 DELETE だと子が孤児化し、
    /// `build_todo_tree` が孤児をルートへ昇格させて**削除したはずのサブタスクがトップレベルに
    /// 再出現**する。再帰 CTE で子孫を巻き取り、Node と挙動を一致させる。再帰段にも scope 検査を
    /// 付け、万一のクロススコープ parent_id 連鎖でも他人の行を消さない（`list_tree` と同方針・
    /// Node より厳格側）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "WITH RECURSIVE descendants(id) AS ( \
                           SELECT id FROM todos \
                             WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3 \
                           UNION ALL \
                           SELECT t.id FROM todos t JOIN descendants d ON t.parent_id = d.id \
                             WHERE t.user_id = ?2 AND t.bot_id = ?3 \
                         ) \
                         DELETE FROM todos WHERE id IN (SELECT id FROM descendants)",
                        params![id, uid, bid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }

    /// ガント表示対象（**開始日 or 期限のどちらかを持つ親タスク**）をサブタスク付きで返す
    /// （Node `listGanttTasks`）。両方未設定のタスクは [`Self::list_someday`] へ回す仕様のため除外。
    ///
    /// 並びは Node のガント専用順（`COALESCE(start_date, due_date)` 昇順 → `COALESCE(due_date,
    /// start_date)` 昇順 → 作成日時降順・NULL は後ろ）。親抽出後、子孫を [`Self::attach_subtasks`]
    /// で束ねる。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_gantt(&self, scope: &UserScope) -> Result<Vec<TodoWithSubtasks>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let parent_ids = self
            .read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id FROM todos \
                         WHERE user_id = ?1 AND bot_id = ?2 AND parent_id IS NULL \
                           AND (start_date IS NOT NULL OR due_date IS NOT NULL) \
                         ORDER BY \
                           CASE WHEN COALESCE(start_date, due_date) IS NULL THEN 1 ELSE 0 END, \
                           datetime(COALESCE(start_date, due_date)) ASC, \
                           datetime(COALESCE(due_date, start_date)) ASC, \
                           created_at DESC",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid], |row| row.get::<_, i64>(0))
                    .map_err(map_sqlite)?;
                let mut ids = Vec::new();
                for row in rows {
                    ids.push(row.map_err(map_sqlite)?);
                }
                Ok(ids)
            })
            .await?;
        self.attach_subtasks(scope, parent_ids).await
    }

    /// 「いつかやる」（**開始日・期限とも未設定の親タスク**）をサブタスク付きで返す
    /// （Node `listSomedayTasks`）。ガントに載せられないタスクの受け皿。並びは [`ORDER_CLAUSE`]。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_someday(&self, scope: &UserScope) -> Result<Vec<TodoWithSubtasks>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let parent_ids = self
            .read
            .read(move |conn| {
                let sql = format!(
                    "SELECT id FROM todos \
                     WHERE user_id = ?1 AND bot_id = ?2 AND parent_id IS NULL \
                       AND start_date IS NULL AND due_date IS NULL{ORDER_CLAUSE}"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid], |row| row.get::<_, i64>(0))
                    .map_err(map_sqlite)?;
                let mut ids = Vec::new();
                for row in rows {
                    ids.push(row.map_err(map_sqlite)?);
                }
                Ok(ids)
            })
            .await?;
        self.attach_subtasks(scope, parent_ids).await
    }

    /// 指定タスクの**直接の子タスク群**をツリー形式で返す（Node `listSubtasksTree`）。
    ///
    /// Node は「指定親を1件ルートとして子孫ツリーを構築し、そのルートの `subtasks` を返す」ため、
    /// **親自身は含めず子（各自の孫を内包）だけ**を返す。存在しない親（またはスコープ外）は空。
    /// detail route と progress の「子ありは手動進捗不可」判定に使う。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_subtasks_tree(
        &self,
        scope: &UserScope,
        parent_id: i64,
    ) -> Result<Vec<TodoWithSubtasks>, DbError> {
        // 親を 1 件だけルートに据えて全子孫を束ね、その subtasks（＝直接の子ツリー）を取り出す。
        let mut roots = self.attach_subtasks(scope, vec![parent_id]).await?;
        Ok(match roots.pop() {
            Some(root) => root.subtasks,
            None => Vec::new(),
        })
    }

    /// 進捗ログを新しい順（`created_at` 降順 → `id` 降順）で返す（Node `listProgressLogs`）。
    ///
    /// クリーンビュー（`user_id`/`bot_id` は返さない・[`TaskProgressLog`]）。scope 一致必須。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_progress_logs(
        &self,
        scope: &UserScope,
        todo_id: i64,
    ) -> Result<Vec<TaskProgressLog>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, todo_id, progress, note, created_at FROM task_progress_logs \
                         WHERE user_id = ?1 AND bot_id = ?2 AND todo_id = ?3 \
                         ORDER BY datetime(created_at) DESC, id DESC",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid, todo_id], |row| {
                        Ok(TaskProgressLog {
                            id: row.get("id")?,
                            todo_id: row.get("todo_id")?,
                            progress: row.get("progress")?,
                            note: row.get("note")?,
                            created_at: row.get("created_at")?,
                        })
                    })
                    .map_err(map_sqlite)?;
                let mut logs = Vec::new();
                for row in rows {
                    logs.push(row.map_err(map_sqlite)?);
                }
                Ok(logs)
            })
            .await
    }

    /// todo を部分更新し、更新後の行を返す（該当無・変更無は据え置きの最新行 / 不在は `None`）。
    ///
    /// **Node `updateTodo` パリティ**: 指定フィールドのみ動的 SET する。`due_date`/`start_date` は
    /// **空文字でクリア**（NULL 化）し、`due_date` 変更時は `due_reminded = 0` にリセットして新期限で
    /// 再度リマインドされるようにする。`priority` は 3 値（[`PriorityUpdate`]・据え置き／クリア／設定）。
    /// SET 対象が無い場合は Node 同様 UPDATE を発行せず現在行をそのまま返す。
    ///
    /// # Errors
    /// 更新・取得失敗時 [`DbError`]。
    pub async fn update(
        &self,
        scope: &UserScope,
        id: i64,
        input: TodoUpdate,
    ) -> Result<Option<Todo>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let outcome = self
            .writer
            .transaction(move |tx| {
                // 動的 SET 句を組み立てる（Node updateTodo と同順・同条件）。値は Vec に積む。
                let mut sets: Vec<String> = Vec::new();
                let mut vals: Vec<rusqlite::types::Value> = Vec::new();
                if let Some(title) = input.title {
                    sets.push("title = ?".to_owned());
                    vals.push(title.into());
                }
                if let Some(description) = input.description {
                    sets.push("description = ?".to_owned());
                    vals.push(description.into());
                }
                if let Some(due_date) = input.due_date {
                    // 空文字は期限クリア（NULL）。変更時は due_reminded をリセット。
                    sets.push("due_date = ?".to_owned());
                    sets.push("due_reminded = 0".to_owned());
                    vals.push(empty_to_null(due_date));
                }
                if let Some(start_date) = input.start_date {
                    // 空文字は開始日クリア（NULL）。
                    sets.push("start_date = ?".to_owned());
                    vals.push(empty_to_null(start_date));
                }
                match input.priority {
                    PriorityUpdate::Unchanged => {}
                    PriorityUpdate::Clear => {
                        sets.push("priority = ?".to_owned());
                        vals.push(rusqlite::types::Value::Null);
                    }
                    PriorityUpdate::Set(p) => {
                        sets.push("priority = ?".to_owned());
                        vals.push(p.into());
                    }
                }
                if let Some(status) = input.status {
                    sets.push("status = ?".to_owned());
                    vals.push(status.into());
                }
                // Node: SET 対象が無ければ UPDATE せず現在行を返す（変更なし＝存在すれば現状維持）。
                if sets.is_empty() {
                    return Ok(UpdateOutcome::NoChange);
                }
                sets.push("updated_at = datetime('now', 'localtime')".to_owned());
                let sql = format!(
                    "UPDATE todos SET {} WHERE user_id = ? AND bot_id = ? AND id = ?",
                    sets.join(", ")
                );
                vals.push(uid.into());
                vals.push(bid.into());
                vals.push(id.into());
                let n = tx
                    .execute(&sql, params_from_iter(vals.iter()))
                    .map_err(map_sqlite)?;
                Ok(if n > 0 {
                    UpdateOutcome::Changed
                } else {
                    UpdateOutcome::Missing
                })
            })
            .await?;
        match outcome {
            // 変更あり／SET 対象無し（現状維持）→ 最新行を返す（不在なら None）。
            UpdateOutcome::Changed | UpdateOutcome::NoChange => self.get(scope, id).await,
            // UPDATE が 0 行 = 対象不在（スコープ外含む）→ None。
            UpdateOutcome::Missing => Ok(None),
        }
    }

    /// 進捗を更新し進捗ログを 1 件追記する（トランザクション・Node `updateProgress`）。
    ///
    /// `progress` は 0-100 に**クランプ**（`round` 後）。`100` で `status=done`、`100` 未満で
    /// `status=open` へ同期する。対象行が無ければ何もせず `None`。子を持つ親は呼び出し側
    /// （route/`update_progress` 前の `list_subtasks_tree` 判定）で弾く前提。
    ///
    /// # Errors
    /// 更新・取得失敗時 [`DbError`]。
    pub async fn update_progress(
        &self,
        scope: &UserScope,
        id: i64,
        progress: i64,
        note: Option<String>,
    ) -> Result<Option<Todo>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let clamped = progress.clamp(0, 100);
        let changed = self
            .writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "UPDATE todos SET progress = ?1, \
                         status = CASE WHEN ?1 >= 100 THEN 'done' ELSE 'open' END, \
                         updated_at = datetime('now', 'localtime') \
                         WHERE user_id = ?2 AND bot_id = ?3 AND id = ?4",
                        params![clamped, uid, bid, id],
                    )
                    .map_err(map_sqlite)?;
                if n == 0 {
                    return Ok(false);
                }
                tx.execute(
                    "INSERT INTO task_progress_logs (user_id, bot_id, todo_id, progress, note, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, datetime('now', 'localtime'))",
                    params![uid, bid, id, clamped, note],
                )
                .map_err(map_sqlite)?;
                Ok(true)
            })
            .await?;
        if changed {
            self.get(scope, id).await
        } else {
            Ok(None)
        }
    }

    /// 指定した親 id 群とその全子孫を再帰 CTE で 1 クエリ収集し、ツリー化して**指定ルートのみ**
    /// を親 id の入力順で返す（Node `attachSubtasks`）。
    ///
    /// gantt/someday/detail/subtasks が親抽出後に共通で使う。ソートは [`ORDER_CLAUSE`]。再帰段にも
    /// scope 検査を付け、クロススコープ parent_id 連鎖でも他人の行を取り込まない（`list_tree` と同方針）。
    async fn attach_subtasks(
        &self,
        scope: &UserScope,
        parent_ids: Vec<i64>,
    ) -> Result<Vec<TodoWithSubtasks>, DbError> {
        if parent_ids.is_empty() {
            return Ok(Vec::new());
        }
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                // ?1=uid ?2=bid、?3.. に親 id を割り当てる（IN 句のプレースホルダを動的生成）。
                let placeholders = (0..parent_ids.len())
                    .map(|i| format!("?{}", i + 3))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "WITH RECURSIVE tree(id) AS ( \
                       SELECT id FROM todos \
                         WHERE user_id = ?1 AND bot_id = ?2 AND id IN ({placeholders}) \
                       UNION ALL \
                       SELECT t.id FROM todos t JOIN tree ON t.parent_id = tree.id \
                         WHERE t.user_id = ?1 AND t.bot_id = ?2 \
                     ) \
                     SELECT {TODO_COLUMNS_QUALIFIED} FROM todos JOIN tree ON todos.id = tree.id \
                     WHERE todos.user_id = ?1 AND todos.bot_id = ?2{ORDER_CLAUSE}"
                );
                let mut all_params: Vec<rusqlite::types::Value> = vec![uid.into(), bid.into()];
                all_params.extend(parent_ids.iter().map(|&pid| pid.into()));
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params_from_iter(all_params.iter()), row_to_todo)
                    .map_err(map_sqlite)?;
                let mut flat = Vec::new();
                for row in rows {
                    flat.push(row.map_err(map_sqlite)?);
                }
                // 全件ツリーを構築したうえで、指定ルートのみを入力順で返す（Node attachSubtasks）。
                let full_tree = build_todo_tree(flat);
                let mut by_id: HashMap<i64, TodoWithSubtasks> =
                    full_tree.into_iter().map(|n| (n.id, n)).collect();
                let mut roots = Vec::new();
                for pid in parent_ids {
                    if let Some(node) = by_id.remove(&pid) {
                        roots.push(node);
                    }
                }
                Ok(roots)
            })
            .await
    }
}

/// `update` の結果 3 値（変更あり／SET 対象無し＝現状維持／対象不在）。
enum UpdateOutcome {
    Changed,
    NoChange,
    Missing,
}

/// 空文字を SQL NULL に、非空文字を Text に写す（Node `updateTodo` の `=== "" ? null : value`）。
fn empty_to_null(value: String) -> rusqlite::types::Value {
    if value.is_empty() {
        rusqlite::types::Value::Null
    } else {
        value.into()
    }
}

/// スコープから所有 String キーを取り出す（`spawn_blocking` の `'static` クロージャ用）。
fn scope_keys(scope: &UserScope) -> (String, String) {
    (
        scope.user_id().as_str().to_owned(),
        scope.bot_id().as_str().to_owned(),
    )
}

/// フラットな行（ORDER_CLAUSE 順）からツリーを構築する（Node `buildTodoTree`）。
///
/// ルート（`parent_id IS NULL`、または親が取得集合に居ない孤立ノード）を親の取得順で返し、
/// `subtasks` は行順（ORDER_CLAUSE）で入れ子にする。各ノードの `effective_progress` を算出する。
fn build_todo_tree(rows: Vec<Todo>) -> Vec<TodoWithSubtasks> {
    let id_set: HashSet<i64> = rows.iter().map(|t| t.id).collect();
    let mut child_ids: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut root_ids: Vec<i64> = Vec::new();
    for t in &rows {
        match t.parent_id {
            // 親が取得集合内にある → その子。行順で push（ORDER_CLAUSE を維持）。
            Some(pid) if id_set.contains(&pid) => child_ids.entry(pid).or_default().push(t.id),
            // parent_id NULL または親不在（孤立）→ ルートへ昇格。
            _ => root_ids.push(t.id),
        }
    }
    let mut node_map: HashMap<i64, Todo> = rows.into_iter().map(|t| (t.id, t)).collect();
    root_ids
        .into_iter()
        .filter_map(|id| build_node(id, &mut node_map, &child_ids))
        .collect()
}

/// `id` のノードを子孫ごと組み立てる（`node_map` から所有権を取り出しつつ再帰）。
fn build_node(
    id: i64,
    node_map: &mut HashMap<i64, Todo>,
    child_ids: &HashMap<i64, Vec<i64>>,
) -> Option<TodoWithSubtasks> {
    let todo = node_map.remove(&id)?;
    let subtasks: Vec<TodoWithSubtasks> = child_ids
        .get(&id)
        .map(|kids| {
            kids.iter()
                .filter_map(|&cid| build_node(cid, node_map, child_ids))
                .collect()
        })
        .unwrap_or_default();
    let effective_progress = effective_progress(&todo, &subtasks);
    Some(TodoWithSubtasks {
        id: todo.id,
        title: todo.title,
        description: todo.description,
        due_date: todo.due_date,
        start_date: todo.start_date,
        priority: todo.priority,
        tags: todo.tags,
        status: todo.status,
        progress: todo.progress,
        parent_id: todo.parent_id,
        repeat_rule: todo.repeat_rule,
        repeat_until: todo.repeat_until,
        repeat_count: todo.repeat_count,
        created_at: todo.created_at,
        updated_at: todo.updated_at,
        subtasks,
        effective_progress,
    })
}

/// 算出進捗（Node `computeEffectiveProgress`）: 子なしは `done?100:progress`、子ありは葉の
/// 完了率 `round(done/total*100)`（葉が 0 件なら 0）。detail route も同じ式で兄弟キーを埋める。
pub(crate) fn effective_progress(todo: &Todo, subtasks: &[TodoWithSubtasks]) -> i64 {
    if subtasks.is_empty() {
        return if todo.status == "done" {
            100
        } else {
            todo.progress
        };
    }
    let (done, total) = count_leaves(subtasks);
    if total == 0 {
        0
    } else {
        ((f64::from(done) / f64::from(total)) * 100.0).round() as i64
    }
}

/// サブツリー内の**葉**（子を持たないノード）の (完了数, 総数) を再帰集計する。
fn count_leaves(nodes: &[TodoWithSubtasks]) -> (i32, i32) {
    let mut done = 0;
    let mut total = 0;
    for n in nodes {
        if n.subtasks.is_empty() {
            total += 1;
            if n.status == "done" {
                done += 1;
            }
        } else {
            let (d, t) = count_leaves(&n.subtasks);
            done += d;
            total += t;
        }
    }
    (done, total)
}

/// SQLite 行を [`Todo`] へ変換する（`tags` は JSON 文字列 → `Vec<String>`）。
fn row_to_todo(row: &Row) -> rusqlite::Result<Todo> {
    let tags_json: String = row.get("tags")?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    Ok(Todo {
        id: row.get("id")?,
        title: row.get("title")?,
        description: row.get("description")?,
        due_date: row.get("due_date")?,
        start_date: row.get("start_date")?,
        priority: row.get("priority")?,
        tags,
        status: row.get("status")?,
        progress: row.get("progress")?,
        parent_id: row.get("parent_id")?,
        repeat_rule: row.get("repeat_rule")?,
        repeat_until: row.get("repeat_until")?,
        repeat_count: row.get("repeat_count")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}
