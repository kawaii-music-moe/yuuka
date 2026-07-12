//! yuuka-todo — todo ドメイン（T1 **参照実装**）: Repo + wire DTO + route を縦に持つ。
//!
//! 残り8ドメイン（finance/schedule/timeline/reminder/personal/credential/playbook/persona）は
//! 本クレートを雛形に per-domain クレートで並行実装する。凍結契約（core/types）は変更しない。
//! DAG: `todo → web, db, types, core`。ルータは supervisor が共通レイヤ配下にマージする。
//!
//! Phase 1 参照スコープ = コア CRUD（list/add/complete/delete）。cron 側のルーチン繰り越し・期限
//! 通知の全件走査（listOverdueRoutines/advanceRoutine/endRoutine/listOpenDueWithin/markDueReminded）は
//! Phase 4 で [`cron`] に追加済み。recurrence の登録 UI・finance 連携・gantt/someday・progress log・
//! 優先度2段・15 tool function・subtree 削除は todo 完成パスで追加。

pub mod cron;
pub mod dto;
pub mod repo;
pub mod routes;
pub mod tools;

pub use routes::routes;
pub use tools::tools;

use std::path::Path;

use ts_rs::TS;

/// 本ドメインの wire DTO を `base_dir/generated/` へ生成する（xtask gen-types が呼ぶ）。
///
/// # Errors
/// ts-rs のシリアライズ／書き込み失敗時 [`ts_rs::ExportError`]。
pub fn export_bindings(base_dir: &Path) -> Result<(), ts_rs::ExportError> {
    let cfg = ts_rs::Config::new().with_out_dir(base_dir.to_path_buf());
    <dto::Todo as TS>::export_all(&cfg)?;
    <dto::TodoWithSubtasks as TS>::export_all(&cfg)?;
    <dto::NewTodo as TS>::export_all(&cfg)?;
    <dto::TaskListData as TS>::export_all(&cfg)?;
    <dto::TaskData as TS>::export_all(&cfg)?;
    <dto::TaskProgressLog as TS>::export_all(&cfg)?;
    <dto::TaskDetailData as TS>::export_all(&cfg)?;
    <dto::TodoUpdate as TS>::export_all(&cfg)?;
    <dto::TodoProgress as TS>::export_all(&cfg)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use yuuka_core::{AuthError, BotId, UserId, UserScope};
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    use crate::dto::{NewTodo, PriorityUpdate, TodoUpdate};
    use crate::repo::TodoRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    const TODOS_DDL: &str = "CREATE TABLE todos (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        title TEXT NOT NULL,
        description TEXT,
        due_date TEXT,
        start_date TEXT,
        priority TEXT,
        tags TEXT NOT NULL DEFAULT '[]',
        status TEXT NOT NULL DEFAULT 'open',
        progress INTEGER NOT NULL DEFAULT 0,
        parent_id INTEGER,
        linked_payment_id INTEGER,
        due_reminded INTEGER NOT NULL DEFAULT 0,
        repeat_rule TEXT,
        repeat_until TEXT,
        repeat_count INTEGER,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );";

    /// task_progress_logs（進捗ログ）。V17__baseline.sql と同列（progress/note の追記対象）。
    const PROGRESS_LOGS_DDL: &str = "CREATE TABLE task_progress_logs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        todo_id INTEGER NOT NULL,
        progress INTEGER NOT NULL,
        note TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_todo_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(TODOS_DDL).expect("create todos");
            conn.execute_batch(PROGRESS_LOGS_DDL)
                .expect("create task_progress_logs");
        }
        Db::open(&path).expect("open db")
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    fn new_todo(title: &str, tags: Vec<String>) -> NewTodo {
        NewTodo {
            title: title.to_owned(),
            tags,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn add_list_and_scope_isolation() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        repo.add(&scope("userA"), new_todo("a-task", vec!["x".to_owned()]))
            .await
            .unwrap();

        let listed = repo.list_tree(&scope("userA"), None, None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "a-task");
        assert_eq!(listed[0].tags, vec!["x".to_owned()]);
        assert_eq!(listed[0].status, "open");

        // 別ユーザーには見えない（分離キーを型で強制）。
        assert!(repo
            .list_tree(&scope("userB"), None, None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn complete_and_delete() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let created = repo.add(&scope("u"), new_todo("t", vec![])).await.unwrap();

        let done = repo
            .complete(&scope("u"), created.id)
            .await
            .unwrap()
            .expect("completed row");
        assert_eq!(done.status, "done");
        // Node parity: complete は progress を変更しない（作成時の 0 のまま）。
        assert_eq!(done.progress, 0);

        assert!(repo.delete(&scope("u"), created.id).await.unwrap());
        assert!(repo
            .list_tree(&scope("u"), None, None)
            .await
            .unwrap()
            .is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), created.id).await.unwrap());
    }

    #[tokio::test]
    async fn delete_cascades_to_descendants_m6() {
        // M-6: 親の削除で全子孫が再帰的に消える（Node `WITH RECURSIVE descendants` parity）。
        // 単一行 DELETE だと子が孤児化し build_todo_tree がルートへ昇格させて再出現する。
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let s = scope("u");
        let parent = repo.add(&s, new_todo("parent", vec![])).await.unwrap();
        let child = repo
            .add(
                &s,
                NewTodo {
                    title: "child".to_owned(),
                    parent_id: Some(parent.id),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let grandchild = repo
            .add(
                &s,
                NewTodo {
                    title: "grandchild".to_owned(),
                    parent_id: Some(child.id),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // 別系統の独立タスク（削除に巻き込まれてはいけない）。
        let other = repo.add(&s, new_todo("other", vec![])).await.unwrap();

        assert!(repo.delete(&s, parent.id).await.unwrap());

        // 子孫がルートに再出現せず、残るのは other のみ。
        let listed = repo.list_tree(&s, None, None).await.unwrap();
        assert_eq!(listed.len(), 1, "親削除で子孫も消え、孤児のルート昇格が起きない");
        assert_eq!(listed[0].title, "other");
        // 子孫は個別 get でも消滅。
        assert!(repo.get(&s, child.id).await.unwrap().is_none());
        assert!(repo.get(&s, grandchild.id).await.unwrap().is_none());
        // 無関係タスクは無傷。
        assert!(repo.get(&s, other.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn parent_id_outside_scope_is_demoted() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let parent = repo
            .add(&scope("userA"), new_todo("parent", vec![]))
            .await
            .unwrap();

        // 別ユーザーが userA の todo を親に指定 → NULL に降格（クロススコープ参照を防ぐ）。
        let cross = repo
            .add(
                &scope("userB"),
                NewTodo {
                    title: "child".to_owned(),
                    parent_id: Some(parent.id),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(cross.parent_id, None);

        // 同一スコープの親は保持。
        let sibling = repo
            .add(
                &scope("userA"),
                NewTodo {
                    title: "sibling".to_owned(),
                    parent_id: Some(parent.id),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(sibling.parent_id, Some(parent.id));
    }

    #[tokio::test]
    async fn subtask_does_not_inherit_repeat_fields() {
        // Node parity: サブタスク（parent_id あり）はルーチンにしない（親のみ繰り返し対象）。
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let s = scope("u");
        let parent = repo.add(&s, new_todo("parent", vec![])).await.unwrap();

        // 親はルーチン可（repeat_rule 保持）。
        let routine_parent = repo
            .add(
                &s,
                NewTodo {
                    title: "routine".to_owned(),
                    repeat_rule: Some("daily".to_owned()),
                    repeat_count: Some(5),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(routine_parent.repeat_rule.as_deref(), Some("daily"));
        assert_eq!(routine_parent.repeat_count, Some(5));

        // サブタスクに repeat_* を付けても NULL 化される（recurrence が子を複製しない）。
        let sub = repo
            .add(
                &s,
                NewTodo {
                    title: "sub".to_owned(),
                    parent_id: Some(parent.id),
                    repeat_rule: Some("daily".to_owned()),
                    repeat_count: Some(5),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(sub.parent_id, Some(parent.id));
        assert_eq!(sub.repeat_rule, None, "サブタスクはルーチンにしない");
        assert_eq!(sub.repeat_count, None);
    }

    struct FakeAuth {
        user: SessionUser,
    }

    #[async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok((token == "good").then(|| self.user.clone()))
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    fn app() -> axum::Router {
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        let state = AppState::new(auth, WebConfig::default(), seed_db());
        super::routes().with_state(state)
    }

    #[tokio::test]
    async fn route_add_then_list() {
        let app = app();
        let add = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"hello","tags":["home"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["tasks"][0]["title"], serde_json::json!("hello"));
        assert_eq!(j["tasks"][0]["tags"][0], serde_json::json!("home"));
        // 内部列は露出しない（構造的フェイルクローズ）。
        assert!(j["tasks"][0]["user_id"].is_null());
        assert!(j["tasks"][0]["bot_id"].is_null());
    }

    #[tokio::test]
    async fn route_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// H-1 回帰: フロント／Node は **camelCase**（`dueDate`/`startDate`）を送り、旧 UI は
    /// **数値優先度**（`2`）を送る。どちらも無音で落とさず永続化することを凍結する。
    #[tokio::test]
    async fn route_add_accepts_camelcase_and_numeric_priority() {
        let app = app();
        let add = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"title":"camel","dueDate":"2026-07-08","startDate":"2026-07-07","priority":2}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(add.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // camelCase が `#[serde(default)]` で None に落ちず永続化される。
        assert_eq!(j["task"]["due_date"], serde_json::json!("2026-07-08"));
        assert_eq!(j["task"]["start_date"], serde_json::json!("2026-07-07"));
        // 数値 2 → "high"（Node normalizePriority と一致）。
        assert_eq!(j["task"]["priority"], serde_json::json!("high"));
    }

    /// wire 契約の直接凍結: 入力 DTO は camelCase を受理し、snake_case は拾わない。
    /// `priority` は数値 0/1/2・文字列・不正値を Node `normalizePriority` と同一規則で正規化する。
    #[test]
    fn newtodo_wire_contract_is_camelcase_in() {
        // camelCase を受理する。
        let camel: NewTodo = serde_json::from_str(
            r#"{"title":"t","dueDate":"2026-07-08","startDate":"2026-07-07","parentId":42}"#,
        )
        .unwrap();
        assert_eq!(camel.due_date.as_deref(), Some("2026-07-08"));
        assert_eq!(camel.start_date.as_deref(), Some("2026-07-07"));
        assert_eq!(camel.parent_id, Some(42));

        // snake_case は camelCase 契約では拾われない（フロントは camelCase のみ送る）。
        let snake: NewTodo =
            serde_json::from_str(r#"{"title":"t","due_date":"2026-07-08"}"#).unwrap();
        assert_eq!(snake.due_date, None);

        // priority の正規化（Node normalizePriority 厳密一致・不正値は None で 400 にしない）。
        let priority = |body: &str| serde_json::from_str::<NewTodo>(body).unwrap().priority;
        assert_eq!(
            priority(r#"{"title":"t","priority":2}"#).as_deref(),
            Some("high")
        );
        assert_eq!(
            priority(r#"{"title":"t","priority":1}"#).as_deref(),
            Some("medium")
        );
        assert_eq!(
            priority(r#"{"title":"t","priority":0}"#).as_deref(),
            Some("low")
        );
        assert_eq!(
            priority(r#"{"title":"t","priority":"high"}"#).as_deref(),
            Some("high")
        );
        assert_eq!(priority(r#"{"title":"t","priority":""}"#), None);
        assert_eq!(priority(r#"{"title":"t","priority":"bogus"}"#), None);
        assert_eq!(priority(r#"{"title":"t","priority":null}"#), None);
        assert_eq!(priority(r#"{"title":"t","priority":9}"#), None);
        assert_eq!(priority(r#"{"title":"t"}"#), None);
    }

    /// フィールド指定用の最小 `NewTodo`（テスト側で必要な列だけ後から差し替える）。
    fn nt(title: &str) -> NewTodo {
        NewTodo {
            title: title.to_owned(),
            ..Default::default()
        }
    }

    /// H-3: 親のみをルートに、サブタスクを `subtasks` へネストし、`effective_progress` を
    /// 葉の完了率で算出する（子なしは done→100 / 未完→progress）。
    #[tokio::test]
    async fn list_tree_nests_subtasks_and_computes_progress() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let sc = scope("u");
        let parent = repo.add(&sc, nt("parent")).await.unwrap();
        let mut ca = nt("child-a");
        ca.parent_id = Some(parent.id);
        let child_a = repo.add(&sc, ca).await.unwrap();
        let mut cb = nt("child-b");
        cb.parent_id = Some(parent.id);
        repo.add(&sc, cb).await.unwrap();
        // 葉 2 件のうち 1 件を完了 → 50%。
        repo.complete(&sc, child_a.id).await.unwrap();

        let tree = repo.list_tree(&sc, None, None).await.unwrap();
        assert_eq!(tree.len(), 1, "親のみルート");
        let root = &tree[0];
        assert_eq!(root.id, parent.id);
        assert_eq!(root.subtasks.len(), 2);
        assert_eq!(root.effective_progress, 50);

        let done_leaf = root
            .subtasks
            .iter()
            .find(|t| t.id == child_a.id)
            .expect("done leaf");
        assert_eq!(done_leaf.status, "done");
        assert_eq!(done_leaf.effective_progress, 100);
        let open_leaf = root
            .subtasks
            .iter()
            .find(|t| t.id != child_a.id)
            .expect("open leaf");
        assert_eq!(open_leaf.effective_progress, 0);
    }

    /// H-3: `status` フィルタは親に適用（pending→open / done / all）。
    #[tokio::test]
    async fn list_tree_status_filter() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let sc = scope("u");
        let open_root = repo.add(&sc, nt("open-root")).await.unwrap();
        let done_root = repo.add(&sc, nt("done-root")).await.unwrap();
        repo.complete(&sc, done_root.id).await.unwrap();

        let open_only = repo
            .list_tree(&sc, Some("open".to_owned()), None)
            .await
            .unwrap();
        assert_eq!(open_only.len(), 1);
        assert_eq!(open_only[0].id, open_root.id);

        let done_only = repo
            .list_tree(&sc, Some("done".to_owned()), None)
            .await
            .unwrap();
        assert_eq!(done_only.len(), 1);
        assert_eq!(done_only[0].id, done_root.id);

        // None（=all）は両方。
        assert_eq!(repo.list_tree(&sc, None, None).await.unwrap().len(), 2);
    }

    /// H-3: `tag` フィルタは json_each で照合し、該当タグを持つ親のみ返す。
    #[tokio::test]
    async fn list_tree_tag_filter() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let sc = scope("u");
        let mut work = nt("work-task");
        work.tags = vec!["work".to_owned()];
        let mut home = nt("home-task");
        home.tags = vec!["home".to_owned()];
        let w = repo.add(&sc, work).await.unwrap();
        repo.add(&sc, home).await.unwrap();

        let filtered = repo
            .list_tree(&sc, None, Some("work".to_owned()))
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, w.id);
    }

    /// H-3: ORDER_CLAUSE = 優先度（high→medium→low→未設定）→ 期限 → 作成日時降順。
    #[tokio::test]
    async fn list_tree_sorts_by_priority() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let sc = scope("u");
        for (title, prio) in [
            ("low-t", Some("low")),
            ("high-t", Some("high")),
            ("med-t", Some("medium")),
            ("none-t", None),
        ] {
            let mut t = nt(title);
            t.priority = prio.map(str::to_owned);
            repo.add(&sc, t).await.unwrap();
        }
        let tree = repo.list_tree(&sc, None, None).await.unwrap();
        let titles: Vec<&str> = tree.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["high-t", "med-t", "low-t", "none-t"]);
    }

    /// H-3 E2E: `GET /api/tasks` がネストしたツリーを返し、内部列を露出しない。
    #[tokio::test]
    async fn route_tasks_returns_nested_tree_without_internal_columns() {
        let app = app();
        // 親を追加し id を取得。
        let add_parent = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"P"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(add_parent.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let pid = j["task"]["id"].as_i64().expect("parent id");

        // サブタスクを親の下に追加。
        let add_child = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"title":"C","parentId":{pid}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add_child.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // 親のみルート、サブタスクはネスト。
        assert_eq!(j["tasks"][0]["title"], serde_json::json!("P"));
        assert_eq!(
            j["tasks"][0]["subtasks"][0]["title"],
            serde_json::json!("C")
        );
        assert!(j["tasks"][0]["effective_progress"].is_number());
        // 内部列は露出しない（clean view の凍結）。
        assert!(j["tasks"][0]["user_id"].is_null());
        assert!(j["tasks"][0]["bot_id"].is_null());
        assert!(j["tasks"][0]["linked_payment_id"].is_null());
        assert!(j["tasks"][0]["subtasks"][0]["user_id"].is_null());
    }

    /// M-12 golden: `POST /api/tasks/delete` は `200 {success:<bool>}` を返す。
    /// 実在削除は `success:true`、該当無（二重削除）は **404 ではなく** `200 {success:false}`。
    /// いずれも `deletedId`/`deleted_id` を返さない（`*DeletedData` 幽霊型の除去）。
    #[tokio::test]
    async fn route_delete_returns_bare_success_without_deleted_id() {
        let app = app();
        let add = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"to-delete"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(add.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = j["task"]["id"].as_i64().expect("id");

        let del = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(del.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(del.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        // deletedId は返さない（Node parity）。
        assert!(j["deletedId"].is_null());
        assert!(j["deleted_id"].is_null());

        // 二重削除（該当無）は 404 ではなく 200 {success:false}。
        let del2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(del2.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(del2.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(false));
    }

    /// M-12 golden: `POST /api/tasks/complete` の該当無は **404 ではなく** `200 {success:false}`
    /// （`task` キーは付かない・Node `{success:!!todo, task}` パリティ）。
    #[tokio::test]
    async fn route_complete_missing_returns_success_false() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/complete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":999999}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(false));
        assert!(j["task"].is_null());
    }

    // ─── 新規ルート: gantt / someday / detail / update / progress ───────────────

    /// `nt` に開始日/期限を差した親を作るヘルパ。
    async fn add_dated(
        repo: &TodoRepo<'_>,
        s: &UserScope,
        title: &str,
        start: Option<&str>,
        due: Option<&str>,
    ) -> crate::dto::Todo {
        let mut t = nt(title);
        t.start_date = start.map(str::to_owned);
        t.due_date = due.map(str::to_owned);
        repo.add(s, t).await.unwrap()
    }

    /// list_gantt: 開始日 or 期限を持つ親のみ返し、両方 NULL は除外する（Node `listGanttTasks`）。
    #[tokio::test]
    async fn repo_list_gantt_includes_only_dated_parents() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let s = scope("u");
        let with_due = add_dated(&repo, &s, "with-due", None, Some("2026-07-10")).await;
        let with_start = add_dated(&repo, &s, "with-start", Some("2026-07-05"), None).await;
        // 両方 NULL の someday タスクは gantt に載らない。
        repo.add(&s, nt("someday")).await.unwrap();

        let gantt = repo.list_gantt(&s).await.unwrap();
        let ids: Vec<i64> = gantt.iter().map(|t| t.id).collect();
        assert_eq!(gantt.len(), 2, "日付付き親のみ");
        assert!(ids.contains(&with_due.id));
        assert!(ids.contains(&with_start.id));
        // 並び: COALESCE(start,due) 昇順（with-start=07-05 が with-due=07-10 より先）。
        assert_eq!(gantt[0].id, with_start.id);
        assert_eq!(gantt[1].id, with_due.id);
    }

    /// list_gantt: サブタスクはネストして返る（進捗算出のため状態問わず同梱）。
    #[tokio::test]
    async fn repo_list_gantt_nests_subtasks() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let s = scope("u");
        let parent = add_dated(&repo, &s, "p", None, Some("2026-07-10")).await;
        let mut child = nt("c");
        child.parent_id = Some(parent.id);
        let child = repo.add(&s, child).await.unwrap();
        repo.complete(&s, child.id).await.unwrap();

        let gantt = repo.list_gantt(&s).await.unwrap();
        assert_eq!(gantt.len(), 1);
        assert_eq!(gantt[0].subtasks.len(), 1);
        assert_eq!(gantt[0].effective_progress, 100, "唯一の葉が完了→100%");
    }

    /// list_someday: 開始日・期限とも NULL の親のみ返す（Node `listSomedayTasks`）。
    #[tokio::test]
    async fn repo_list_someday_includes_only_undated_parents() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let s = scope("u");
        let undated = repo.add(&s, nt("undated")).await.unwrap();
        add_dated(&repo, &s, "dated", None, Some("2026-07-10")).await;

        let someday = repo.list_someday(&s).await.unwrap();
        assert_eq!(someday.len(), 1);
        assert_eq!(someday[0].id, undated.id);
    }

    /// list_subtasks_tree: 親自身は含めず、直接の子（各自の孫を内包）だけ返す（Node `listSubtasksTree`）。
    #[tokio::test]
    async fn repo_list_subtasks_tree_returns_children_not_parent() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let s = scope("u");
        let parent = repo.add(&s, nt("parent")).await.unwrap();
        let mut c = nt("child");
        c.parent_id = Some(parent.id);
        let child = repo.add(&s, c).await.unwrap();
        let mut g = nt("grandchild");
        g.parent_id = Some(child.id);
        let grandchild = repo.add(&s, g).await.unwrap();

        let subs = repo.list_subtasks_tree(&s, parent.id).await.unwrap();
        assert_eq!(subs.len(), 1, "直接の子のみをトップに");
        assert_eq!(subs[0].id, child.id);
        assert_eq!(subs[0].subtasks.len(), 1, "孫は子の下にネスト");
        assert_eq!(subs[0].subtasks[0].id, grandchild.id);

        // 子を持たないタスクは空。存在しない id も空。
        assert!(repo.list_subtasks_tree(&s, grandchild.id).await.unwrap().is_empty());
        assert!(repo.list_subtasks_tree(&s, 999_999).await.unwrap().is_empty());
    }

    /// update_progress: 0-100 にクランプし進捗ログを追記、100 で done へ同期する（Node `updateProgress`）。
    #[tokio::test]
    async fn repo_update_progress_clamps_logs_and_syncs_status() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let s = scope("u");
        let t = repo.add(&s, nt("t")).await.unwrap();

        // 上限超過は 100 にクランプ＝done。
        let done = repo
            .update_progress(&s, t.id, 150, Some("finished".to_owned()))
            .await
            .unwrap()
            .expect("row");
        assert_eq!(done.progress, 100);
        assert_eq!(done.status, "done");

        // 下限未満は 0 にクランプ＝open。
        let reopened = repo
            .update_progress(&s, t.id, -20, None)
            .await
            .unwrap()
            .expect("row");
        assert_eq!(reopened.progress, 0);
        assert_eq!(reopened.status, "open");

        // 進捗ログは新しい順（2 件目=-20→0 が先頭）。note も保持。
        let logs = repo.list_progress_logs(&s, t.id).await.unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].progress, 0);
        assert_eq!(logs[0].note, None);
        assert_eq!(logs[1].progress, 100);
        assert_eq!(logs[1].note.as_deref(), Some("finished"));
        assert_eq!(logs[1].todo_id, t.id);

        // 不在 id は None（ログも増えない）。
        assert!(repo.update_progress(&s, 999_999, 50, None).await.unwrap().is_none());
    }

    /// update: 部分更新。due_date 変更で due_reminded がリセットされ、空文字で NULL クリアされる。
    #[tokio::test]
    async fn repo_update_partial_and_clear_fields() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let s = scope("u");
        let t = add_dated(&repo, &s, "orig", Some("2026-07-01"), Some("2026-07-02")).await;

        // title/description のみ更新、他は据え置き。
        let upd = repo
            .update(
                &s,
                t.id,
                TodoUpdate {
                    id: t.id,
                    title: Some("renamed".to_owned()),
                    description: Some("desc".to_owned()),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .expect("row");
        assert_eq!(upd.title, "renamed");
        assert_eq!(upd.description.as_deref(), Some("desc"));
        assert_eq!(upd.due_date.as_deref(), Some("2026-07-02"), "未指定は据え置き");

        // 空文字で due_date/start_date をクリア（NULL 化）。
        let cleared = repo
            .update(
                &s,
                t.id,
                TodoUpdate {
                    id: t.id,
                    due_date: Some(String::new()),
                    start_date: Some(String::new()),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .expect("row");
        assert_eq!(cleared.due_date, None);
        assert_eq!(cleared.start_date, None);

        // 不在 id は None（スコープ外含む）。
        assert!(repo
            .update(&s, 999_999, TodoUpdate { id: 999_999, title: Some("x".to_owned()), ..Default::default() })
            .await
            .unwrap()
            .is_none());
    }

    /// update: PriorityUpdate 3 値（据え置き / クリア / 設定）を正しく反映する。
    #[tokio::test]
    async fn repo_update_priority_tristate() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let s = scope("u");
        let mut base = nt("t");
        base.priority = Some("high".to_owned());
        let t = repo.add(&s, base).await.unwrap();
        assert_eq!(t.priority.as_deref(), Some("high"));

        // Unchanged: priority を触らない（他フィールドだけ更新）→ high のまま。
        let kept = repo
            .update(&s, t.id, TodoUpdate { id: t.id, title: Some("k".to_owned()), priority: PriorityUpdate::Unchanged, ..Default::default() })
            .await
            .unwrap()
            .expect("row");
        assert_eq!(kept.priority.as_deref(), Some("high"));

        // Set: medium へ。
        let set = repo
            .update(&s, t.id, TodoUpdate { id: t.id, priority: PriorityUpdate::Set("medium".to_owned()), ..Default::default() })
            .await
            .unwrap()
            .expect("row");
        assert_eq!(set.priority.as_deref(), Some("medium"));

        // Clear: NULL へ。
        let cleared = repo
            .update(&s, t.id, TodoUpdate { id: t.id, priority: PriorityUpdate::Clear, ..Default::default() })
            .await
            .unwrap()
            .expect("row");
        assert_eq!(cleared.priority, None);
    }

    /// TodoUpdate wire 契約: camelCase を受理し、priority の null/空/正規/不正を PriorityUpdate へ、
    /// status は open/done のみ採用する。
    #[test]
    fn todoupdate_wire_contract() {
        let parse = |b: &str| serde_json::from_str::<TodoUpdate>(b).unwrap();
        // camelCase の dueDate/startDate を拾う。
        let u = parse(r#"{"id":5,"dueDate":"2026-07-08","startDate":"2026-07-07"}"#);
        assert_eq!(u.id, 5);
        assert_eq!(u.due_date.as_deref(), Some("2026-07-08"));
        assert_eq!(u.start_date.as_deref(), Some("2026-07-07"));
        assert_eq!(u.priority, PriorityUpdate::Unchanged, "未指定は据え置き");

        // priority: null/空はクリア、正規は設定、不正・数値外は据え置き。
        assert_eq!(parse(r#"{"id":1,"priority":null}"#).priority, PriorityUpdate::Clear);
        assert_eq!(parse(r#"{"id":1,"priority":""}"#).priority, PriorityUpdate::Clear);
        assert_eq!(parse(r#"{"id":1,"priority":"high"}"#).priority, PriorityUpdate::Set("high".to_owned()));
        assert_eq!(parse(r#"{"id":1,"priority":2}"#).priority, PriorityUpdate::Set("high".to_owned()));
        assert_eq!(parse(r#"{"id":1,"priority":"bogus"}"#).priority, PriorityUpdate::Unchanged);
        assert_eq!(parse(r#"{"id":1,"priority":9}"#).priority, PriorityUpdate::Unchanged);

        // status: open/done のみ採用、他は None（据え置き）。
        assert_eq!(parse(r#"{"id":1,"status":"open"}"#).status.as_deref(), Some("open"));
        assert_eq!(parse(r#"{"id":1,"status":"done"}"#).status.as_deref(), Some("done"));
        assert_eq!(parse(r#"{"id":1,"status":"bogus"}"#).status, None);
        assert_eq!(parse(r#"{"id":1,"status":null}"#).status, None);
    }

    /// helper: 認証つきで JSON を POST/GET する（E2E）。
    async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let j = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        (status, j)
    }

    async fn post_json(app: &axum::Router, uri: &str, body: String) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let j = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        (status, j)
    }

    /// 親を 1 件追加して id を返す（E2E ヘルパ）。
    async fn add_parent(app: &axum::Router, body: &str) -> i64 {
        let (status, j) = post_json(app, "/api/tasks/add", body.to_owned()).await;
        assert_eq!(status, StatusCode::OK);
        j["task"]["id"].as_i64().expect("id")
    }

    /// `GET /api/tasks/detail`: id 未指定/非数値/0 は 400、不在は 404、成功は camelCase 兄弟キー付き。
    #[tokio::test]
    async fn route_detail_validates_and_returns_siblings() {
        let app = app();
        // id 未指定 → 400。
        assert_eq!(get_json(&app, "/api/tasks/detail").await.0, StatusCode::BAD_REQUEST);
        // 非数値 → 400（Node Number("abc")=NaN）。
        assert_eq!(get_json(&app, "/api/tasks/detail?id=abc").await.0, StatusCode::BAD_REQUEST);
        // 0 → 400（Node !0）。
        assert_eq!(get_json(&app, "/api/tasks/detail?id=0").await.0, StatusCode::BAD_REQUEST);
        // 不在 → 404。
        assert_eq!(get_json(&app, "/api/tasks/detail?id=999999").await.0, StatusCode::NOT_FOUND);

        // (a) 葉タスクの detail: 自身の進捗ログが progressLogs に載る（Node は要求 id のログを返す）。
        let leaf = add_parent(&app, r#"{"title":"L"}"#).await;
        let (ps, _) = post_json(&app, "/api/tasks/progress", format!(r#"{{"id":{leaf},"progress":40,"note":"half"}}"#)).await;
        assert_eq!(ps, StatusCode::OK);
        let (ls, lj) = get_json(&app, &format!("/api/tasks/detail?id={leaf}")).await;
        assert_eq!(ls, StatusCode::OK);
        assert_eq!(lj["success"], serde_json::json!(true));
        // task はフラット単一 todo（subtasks を内包しない）。
        assert_eq!(lj["task"]["title"], serde_json::json!("L"));
        assert!(lj["task"]["subtasks"].is_null(), "task はフラット（サブツリー非内包）");
        // 葉なので subtasks は空、effectiveProgress は手動 progress を反映。
        assert_eq!(lj["subtasks"].as_array().unwrap().len(), 0);
        assert_eq!(lj["effectiveProgress"], serde_json::json!(40));
        // progressLogs(camelCase) に自身のログが載る。
        assert_eq!(lj["progressLogs"][0]["progress"], serde_json::json!(40));
        assert_eq!(lj["progressLogs"][0]["note"], serde_json::json!("half"));
        // clean view: 進捗ログにも内部列は出ない。
        assert!(lj["progressLogs"][0]["user_id"].is_null());
        assert!(lj["progressLogs"][0]["bot_id"].is_null());

        // (b) 親の detail: 兄弟キー subtasks に子がネストし、effectiveProgress は葉から算出（子未完→0）。
        let pid = add_parent(&app, r#"{"title":"P"}"#).await;
        post_json(&app, "/api/tasks/add", format!(r#"{{"title":"C","parentId":{pid}}}"#)).await;
        let (pstatus, pj) = get_json(&app, &format!("/api/tasks/detail?id={pid}")).await;
        assert_eq!(pstatus, StatusCode::OK);
        assert_eq!(pj["subtasks"][0]["title"], serde_json::json!("C"));
        assert!(pj["effectiveProgress"].is_number());
        // 親自身の進捗ログは無い（子に付けた場合も親の progressLogs は空・Node は要求 id のログのみ）。
        assert_eq!(pj["progressLogs"].as_array().unwrap().len(), 0);
    }

    /// `GET /api/tasks/gantt` / `someday`: 日付有無で振り分け、{success, tasks} を返す。
    #[tokio::test]
    async fn route_gantt_and_someday_partition_by_date() {
        let app = app();
        add_parent(&app, r#"{"title":"dated","dueDate":"2026-07-10"}"#).await;
        add_parent(&app, r#"{"title":"undated"}"#).await;

        let (gs, gj) = get_json(&app, "/api/tasks/gantt").await;
        assert_eq!(gs, StatusCode::OK);
        assert_eq!(gj["success"], serde_json::json!(true));
        assert_eq!(gj["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(gj["tasks"][0]["title"], serde_json::json!("dated"));
        assert!(gj["tasks"][0]["user_id"].is_null(), "clean view");

        let (ss, sj) = get_json(&app, "/api/tasks/someday").await;
        assert_eq!(ss, StatusCode::OK);
        assert_eq!(sj["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(sj["tasks"][0]["title"], serde_json::json!("undated"));
    }

    /// `POST /api/tasks/update`: id 未指定/0 は 400、不在は 404、成功は {success, task}。
    #[tokio::test]
    async fn route_update_validates_and_updates() {
        let app = app();
        // id なし → 400（missing field id で DTO パース失敗）。
        assert_eq!(post_json(&app, "/api/tasks/update", r#"{"title":"x"}"#.to_owned()).await.0, StatusCode::BAD_REQUEST);
        // id=0 → 400。
        assert_eq!(post_json(&app, "/api/tasks/update", r#"{"id":0,"title":"x"}"#.to_owned()).await.0, StatusCode::BAD_REQUEST);
        // 不在 → 404。
        assert_eq!(post_json(&app, "/api/tasks/update", r#"{"id":999999,"title":"x"}"#.to_owned()).await.0, StatusCode::NOT_FOUND);

        let pid = add_parent(&app, r#"{"title":"orig"}"#).await;
        let (status, j) = post_json(&app, "/api/tasks/update", format!(r#"{{"id":{pid},"title":"updated","priority":"high","status":"done"}}"#)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(j["task"]["title"], serde_json::json!("updated"));
        assert_eq!(j["task"]["priority"], serde_json::json!("high"));
        assert_eq!(j["task"]["status"], serde_json::json!("done"));
    }

    /// `POST /api/tasks/progress`: 葉タスクは 200、サブタスクを持つ親は 409、不在は 404、id なしは 400。
    #[tokio::test]
    async fn route_progress_leaf_ok_parent_conflict() {
        let app = app();
        // id なし → 400。
        assert_eq!(post_json(&app, "/api/tasks/progress", r#"{"progress":50}"#.to_owned()).await.0, StatusCode::BAD_REQUEST);
        // 不在 → 404。
        assert_eq!(post_json(&app, "/api/tasks/progress", r#"{"id":999999,"progress":50}"#.to_owned()).await.0, StatusCode::NOT_FOUND);

        // 葉タスクは進捗更新可（200）。
        let leaf = add_parent(&app, r#"{"title":"leaf"}"#).await;
        let (ls, lj) = post_json(&app, "/api/tasks/progress", format!(r#"{{"id":{leaf},"progress":60}}"#)).await;
        assert_eq!(ls, StatusCode::OK);
        assert_eq!(lj["task"]["progress"], serde_json::json!(60));

        // サブタスクを持つ親は 409（進捗は子から算出のため手動不可）。
        let parent = add_parent(&app, r#"{"title":"parent"}"#).await;
        post_json(&app, "/api/tasks/add", format!(r#"{{"title":"child","parentId":{parent}}}"#)).await;
        assert_eq!(post_json(&app, "/api/tasks/progress", format!(r#"{{"id":{parent},"progress":50}}"#)).await.0, StatusCode::CONFLICT);
    }
}
