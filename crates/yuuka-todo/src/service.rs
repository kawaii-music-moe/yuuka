//! ToDo の登録・更新・完了・削除の共通経路（D02 #126）。
//!
//! 管理画面の `/api/tasks/*`・PWA の `/api/client/todos`・エージェントのツール（`addTodo` ほか）は
//! 書き込みをすべてここに通す。入口ごとに repo を直接呼ぶと、入力検証や既定値が経路ごとに食い違う
//! ため（例: 管理 API だけが繰り返し規則を検証していた）。予定の `yuuka-schedule/src/service.rs`
//! （D03 #127）と同じ形。
//!
//! 入口に残すのは、入力形式の解釈（JSON の欠落・型・パス引数）と応答形式への変換だけ。業務ルールに
//! 合わない入力は [`DomainError::Invalid`] で返し、各入口がそれぞれの形式（HTTP 400・ツールの
//! `{success:false}`）に変換する。自動タグの予約は INSERT/UPDATE トリガーが担うため、どの入口からの
//! 書き込みにも同じように付く（R05 #102）。

use yuuka_core::{cron_util, UserScope};
use yuuka_web::{Db, DomainError};

use crate::dto::{NewTodo, Todo, TodoUpdate, TodoWithSubtasks};
use crate::repo::TodoRepo;

/// [`TodoService::update_progress`] の結果。入口ごとに応答（HTTP の 404/409、ツールの案内文）が
/// 違うため、エラーではなく結果として返す。
#[derive(Debug)]
pub enum ProgressUpdate {
    /// 更新した。
    Updated(Todo),
    /// スコープ内にタスクが無い。
    NotFound,
    /// サブタスクを持つ親。進捗は子から自動計算されるので手動では更新しない。
    HasSubtasks {
        task: Todo,
        subtasks: Vec<TodoWithSubtasks>,
    },
}

/// ToDo の書き込み（全入口共通）。
pub struct TodoService<'a> {
    db: &'a Db,
}

impl<'a> TodoService<'a> {
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// ToDo を登録する。タイトル必須。繰り返し規則は前後の空白を除き、空なら単発として扱い、
    /// 実行時と同じ cron parser で検証する（R19 #116）。
    ///
    /// # Errors
    /// 入力が不正なら [`DomainError::Invalid`]、DB 失敗は [`DomainError::Db`]。
    pub async fn add(&self, scope: &UserScope, mut input: NewTodo) -> Result<Todo, DomainError> {
        normalize_new(&mut input)?;
        Ok(TodoRepo::new(self.db).add(scope, input).await?)
    }

    /// 指定項目だけを更新する。スコープ内に無ければ `None`。
    ///
    /// # Errors
    /// DB 失敗時 [`DomainError::Db`]。
    pub async fn update(
        &self,
        scope: &UserScope,
        id: i64,
        input: TodoUpdate,
    ) -> Result<Option<Todo>, DomainError> {
        Ok(TodoRepo::new(self.db).update(scope, id, input).await?)
    }

    /// 完了にする。スコープ内に無ければ `None`。
    ///
    /// # Errors
    /// DB 失敗時 [`DomainError::Db`]。
    pub async fn complete(&self, scope: &UserScope, id: i64) -> Result<Option<Todo>, DomainError> {
        Ok(TodoRepo::new(self.db).complete(scope, id).await?)
    }

    /// 完了・未完了を切り替える（PWA のチェックボックス）。完了は [`Self::complete`] と同じ処理、
    /// 未完了は状態を `open` に戻す。スコープ内に無ければ `None`。
    ///
    /// # Errors
    /// DB 失敗時 [`DomainError::Db`]。
    pub async fn set_completed(
        &self,
        scope: &UserScope,
        id: i64,
        completed: bool,
    ) -> Result<Option<Todo>, DomainError> {
        if completed {
            return self.complete(scope, id).await;
        }
        let reopen = TodoUpdate {
            id,
            status: Some("open".to_owned()),
            ..Default::default()
        };
        self.update(scope, id, reopen).await
    }

    /// 進捗（0-100、repo でクランプ）を更新し、進捗ログを 1 件残す。サブタスクを持つ親は
    /// 進捗が子から自動計算されるため更新しない。
    ///
    /// # Errors
    /// DB 失敗時 [`DomainError::Db`]。
    pub async fn update_progress(
        &self,
        scope: &UserScope,
        id: i64,
        progress: i64,
        note: Option<String>,
    ) -> Result<ProgressUpdate, DomainError> {
        let repo = TodoRepo::new(self.db);
        let Some(task) = repo.get(scope, id).await? else {
            return Ok(ProgressUpdate::NotFound);
        };
        let subtasks = repo.list_subtasks_tree(scope, id).await?;
        if !subtasks.is_empty() {
            return Ok(ProgressUpdate::HasSubtasks { task, subtasks });
        }
        Ok(
            match repo.update_progress(scope, id, progress, note).await? {
                Some(updated) => ProgressUpdate::Updated(updated),
                // 確認後に別経路で削除された。
                None => ProgressUpdate::NotFound,
            },
        )
    }

    /// 子孫ごと削除する。削除できたら `true`。
    ///
    /// # Errors
    /// DB 失敗時 [`DomainError::Db`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DomainError> {
        Ok(TodoRepo::new(self.db).delete(scope, id).await?)
    }
}

/// 登録内容を検証し、繰り返し規則を正規化する（空は単発扱い）。
fn normalize_new(input: &mut NewTodo) -> Result<(), DomainError> {
    if input.title.trim().is_empty() {
        return Err(DomainError::Invalid("title is required".to_owned()));
    }
    input.repeat_rule = input
        .repeat_rule
        .take()
        .map(|rule| rule.trim().to_owned())
        .filter(|rule| !rule.is_empty());
    if input
        .repeat_rule
        .as_deref()
        .is_some_and(|rule| !cron_util::is_valid(rule))
    {
        return Err(DomainError::Invalid(
            "repeatRule must be a valid cron expression".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_todo(title: &str, repeat_rule: Option<&str>) -> NewTodo {
        NewTodo {
            title: title.to_owned(),
            repeat_rule: repeat_rule.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn blank_title_is_rejected() {
        let mut input = new_todo("  ", None);
        assert!(matches!(
            normalize_new(&mut input),
            Err(DomainError::Invalid(m)) if m == "title is required"
        ));
    }

    #[test]
    fn blank_repeat_rule_means_one_off() {
        let mut input = new_todo("掃除", Some("  "));
        normalize_new(&mut input).unwrap();
        assert_eq!(input.repeat_rule, None);
    }

    #[test]
    fn repeat_rule_is_trimmed_and_checked_with_the_runtime_parser() {
        let mut input = new_todo("掃除", Some(" 0 9 * * 1 "));
        normalize_new(&mut input).unwrap();
        assert_eq!(input.repeat_rule.as_deref(), Some("0 9 * * 1"));

        // 5 フィールドでも発火日が実在しない式は保存しない。
        let mut input = new_todo("掃除", Some("0 0 30 2 *"));
        assert!(matches!(
            normalize_new(&mut input),
            Err(DomainError::Invalid(_))
        ));
    }
}
