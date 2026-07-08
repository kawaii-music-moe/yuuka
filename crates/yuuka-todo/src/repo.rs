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

use crate::dto::{NewTodo, Todo, TodoWithSubtasks};

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
                // ルーチン列は repeat_rule がある時のみ有効（Node addTodo と同じ・無ければ NULL）。
                let (repeat_rule, repeat_until, repeat_count) = match &input.repeat_rule {
                    Some(rule) => (
                        Some(rule.clone()),
                        input.repeat_until.clone(),
                        input.repeat_count,
                    ),
                    None => (None, None, None),
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

    /// todo を削除する（削除できたら `true`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM todos WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
                        params![id, uid, bid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
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
/// 完了率 `round(done/total*100)`（葉が 0 件なら 0）。
fn effective_progress(todo: &Todo, subtasks: &[TodoWithSubtasks]) -> i64 {
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
