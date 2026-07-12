//! yuuka-playbook — playbook ドメイン（T1 fan-out・**参照実装は yuuka-todo**）:
//! Repo + wire DTO + route を縦に持つ。
//!
//! 凍結契約（core/types）は変更しない。DAG: `playbook → web, db, types, core`。
//! ルータは supervisor が共通レイヤ配下にマージする。
//!
//! スコープ = コア CRUD（list/save/delete、name キーの upsert）＋定期実行スケジュール
//! （schedules の CRUD/toggle）と実行履歴（runs）。cron 実行エンジン本体（node-cron 相当の
//! スケジューラ・executePlaybook）は別サービスとして deferred。cron 式の妥当性検証は
//! croner がこのクレートの依存に無いため deferred（後述の routes.rs 参照）。

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
    <dto::Playbook as TS>::export_all(&cfg)?;
    <dto::NewPlaybook as TS>::export_all(&cfg)?;
    <dto::PlaybookListData as TS>::export_all(&cfg)?;
    <dto::PlaybookData as TS>::export_all(&cfg)?;
    <dto::PlaybookSchedule as TS>::export_all(&cfg)?;
    <dto::PlaybookRun as TS>::export_all(&cfg)?;
    <dto::NewSchedule as TS>::export_all(&cfg)?;
    <dto::ToggleScheduleInput as TS>::export_all(&cfg)?;
    <dto::ScheduleIdInput as TS>::export_all(&cfg)?;
    <dto::ScheduleListData as TS>::export_all(&cfg)?;
    <dto::ScheduleData as TS>::export_all(&cfg)?;
    <dto::RunListData as TS>::export_all(&cfg)?;
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

    use crate::dto::{NewPlaybook, NewSchedule};
    use crate::repo::PlaybookRepo;

    fn new_schedule(name: &str, cron: &str, enabled: bool) -> NewSchedule {
        NewSchedule {
            playbook_name: name.to_owned(),
            cron_expression: cron.to_owned(),
            description: String::new(),
            enabled,
        }
    }

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // V17__baseline.sql と同一スキーマ（schedules/runs は enabled 0/1・UNIQUE(user_id,
    // playbook_name)・bot_id 列・FK CASCADE を含む）。
    const PLAYBOOKS_DDL: &str = "CREATE TABLE playbooks (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        name TEXT NOT NULL,
        title TEXT NOT NULL,
        keywords TEXT DEFAULT '[]',
        description TEXT DEFAULT '',
        steps TEXT NOT NULL DEFAULT '',
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        UNIQUE(user_id, bot_id, name)
    );
    CREATE TABLE playbook_schedules (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        playbook_name TEXT NOT NULL,
        cron_expression TEXT NOT NULL,
        description TEXT DEFAULT '',
        enabled INTEGER NOT NULL DEFAULT 1,
        last_run_at TEXT,
        next_run_at TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        UNIQUE(user_id, playbook_name)
    );
    CREATE TABLE playbook_runs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        schedule_id INTEGER NOT NULL,
        user_id TEXT NOT NULL,
        playbook_name TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'running',
        output TEXT DEFAULT '',
        started_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        finished_at TEXT,
        bot_id TEXT NOT NULL DEFAULT 'system_default'
    );";

    fn seed_db_with_path() -> (Db, std::path::PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("yuuka_playbook_test_{}_{seq}.sqlite", std::process::id()));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(PLAYBOOKS_DDL).expect("create playbooks");
        }
        let db = Db::open(&path).expect("open db");
        (db, path)
    }

    fn seed_db() -> Db {
        seed_db_with_path().0
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    fn new_playbook(name: &str, title: &str, keywords: Vec<String>) -> NewPlaybook {
        NewPlaybook {
            name: name.to_owned(),
            title: title.to_owned(),
            keywords,
            description: String::new(),
            steps: "step one".to_owned(),
        }
    }

    #[tokio::test]
    async fn save_list_and_scope_isolation() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(
            &scope("userA"),
            new_playbook("Morning Routine", "朝の準備", vec!["朝".to_owned()]),
        )
        .await
        .unwrap();

        let listed = repo.list(&scope("userA"), None).await.unwrap();
        assert_eq!(listed.len(), 1);
        // Node parity: name は正規化される（空白→_・小文字化）。
        assert_eq!(listed[0].name, "morning_routine");
        assert_eq!(listed[0].title, "朝の準備");
        assert_eq!(listed[0].keywords, vec!["朝".to_owned()]);
        assert_eq!(listed[0].steps, "step one");

        // 別ユーザーには見えない（分離キーを型で強制）。
        assert!(repo.list(&scope("userB"), None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn save_is_upsert() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("u"), new_playbook("pb", "v1", vec![]))
            .await
            .unwrap();
        let updated = repo
            .save(
                &scope("u"),
                NewPlaybook {
                    name: "pb".to_owned(),
                    title: "v2".to_owned(),
                    keywords: vec!["k".to_owned()],
                    description: "d".to_owned(),
                    steps: "s2".to_owned(),
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.title, "v2");
        // upsert なので重複せず1件。
        let listed = repo.list(&scope("u"), None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].steps, "s2");
    }

    #[tokio::test]
    async fn get_and_delete() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("u"), new_playbook("pb", "t", vec![]))
            .await
            .unwrap();

        let got = repo
            .get(&scope("u"), "pb".to_owned())
            .await
            .unwrap()
            .expect("found");
        assert_eq!(got.title, "t");

        assert!(repo.delete(&scope("u"), "pb".to_owned()).await.unwrap());
        assert!(repo.list(&scope("u"), None).await.unwrap().is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), "pb".to_owned()).await.unwrap());
    }

    #[tokio::test]
    async fn list_query_filters() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("u"), new_playbook("alpha", "First", vec![]))
            .await
            .unwrap();
        repo.save(&scope("u"), new_playbook("beta", "Second", vec![]))
            .await
            .unwrap();

        let hits = repo
            .list(&scope("u"), Some("alpha".to_owned()))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "alpha");
        // 空 query は全件（Node の falsy 扱いと一致）。
        assert_eq!(
            repo.list(&scope("u"), Some(String::new())).await.unwrap().len(),
            2
        );
    }

    #[tokio::test]
    async fn delete_is_scope_isolated() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("userA"), new_playbook("shared", "t", vec![]))
            .await
            .unwrap();
        // 別ユーザーは同名を削除できない（クロススコープ削除を防ぐ）。
        assert!(!repo
            .delete(&scope("userB"), "shared".to_owned())
            .await
            .unwrap());
        assert_eq!(repo.list(&scope("userA"), None).await.unwrap().len(), 1);
    }

    // ── 定期実行スケジュール / 実行履歴 ──

    #[tokio::test]
    async fn upsert_schedule_requires_existing_playbook() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        // playbook が無ければ None（Node の「マクロが見つかりません」経路）。
        assert!(repo
            .upsert_schedule(&scope("u"), new_schedule("ghost", "0 8 * * *", true))
            .await
            .unwrap()
            .is_none());

        repo.save(&scope("u"), new_playbook("daily", "Daily", vec![]))
            .await
            .unwrap();
        let saved = repo
            .upsert_schedule(&scope("u"), new_schedule("daily", "0 8 * * *", true))
            .await
            .unwrap()
            .expect("schedule saved");
        assert_eq!(saved.playbook_name, "daily");
        assert_eq!(saved.cron_expression, "0 8 * * *");
        assert!(saved.enabled);
        assert_eq!(saved.bot_id, "system_default");
    }

    #[tokio::test]
    async fn upsert_schedule_is_upsert_on_user_and_name() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("u"), new_playbook("daily", "Daily", vec![]))
            .await
            .unwrap();
        repo.upsert_schedule(&scope("u"), new_schedule("daily", "0 8 * * *", true))
            .await
            .unwrap()
            .unwrap();
        let updated = repo
            .upsert_schedule(&scope("u"), new_schedule("daily", "30 9 * * *", false))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.cron_expression, "30 9 * * *");
        assert!(!updated.enabled);
        // UNIQUE(user_id, playbook_name) なので 1 件のまま。
        assert_eq!(repo.list_schedules(&scope("u")).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_schedules_is_user_scoped() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("userA"), new_playbook("a", "A", vec![]))
            .await
            .unwrap();
        repo.upsert_schedule(&scope("userA"), new_schedule("a", "0 8 * * *", true))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(repo.list_schedules(&scope("userA")).await.unwrap().len(), 1);
        // 別ユーザーには見えない。
        assert!(repo.list_schedules(&scope("userB")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn toggle_and_delete_schedule_enforce_ownership() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("owner"), new_playbook("pb", "PB", vec![]))
            .await
            .unwrap();
        let saved = repo
            .upsert_schedule(&scope("owner"), new_schedule("pb", "0 8 * * *", true))
            .await
            .unwrap()
            .unwrap();
        let id = saved.id;

        // 他人は toggle できない。
        assert!(!repo.toggle_schedule(&scope("intruder"), id, false).await.unwrap());
        // オーナーは toggle 可・enabled が反映される。
        assert!(repo.toggle_schedule(&scope("owner"), id, false).await.unwrap());
        let after = repo.list_schedules(&scope("owner")).await.unwrap();
        assert!(!after[0].enabled);

        // 他人は delete できない。
        assert!(!repo.delete_schedule(&scope("intruder"), id).await.unwrap());
        assert_eq!(repo.list_schedules(&scope("owner")).await.unwrap().len(), 1);
        // オーナーは delete 可。二重削除は false。
        assert!(repo.delete_schedule(&scope("owner"), id).await.unwrap());
        assert!(!repo.delete_schedule(&scope("owner"), id).await.unwrap());
        assert!(repo.list_schedules(&scope("owner")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_runs_scoped_and_filtered() {
        let (db, path) = seed_db_with_path();
        // playbook_runs を直接シードする（実行エンジンは未移植のため）。
        {
            let conn = rusqlite::Connection::open(&path).expect("open");
            conn.execute_batch(
                "INSERT INTO playbook_runs (schedule_id, user_id, bot_id, playbook_name, status, output, started_at) \
                   VALUES (1, 'u', 'system_default', 'pb', 'success', 'ok1', '2026-07-01 10:00:00');
                 INSERT INTO playbook_runs (schedule_id, user_id, bot_id, playbook_name, status, output, started_at) \
                   VALUES (1, 'u', 'system_default', 'pb', 'failed', 'err', '2026-07-02 10:00:00');
                 INSERT INTO playbook_runs (schedule_id, user_id, bot_id, playbook_name, status, output, started_at) \
                   VALUES (2, 'u', 'system_default', 'other', 'running', '', '2026-07-03 10:00:00');
                 INSERT INTO playbook_runs (schedule_id, user_id, bot_id, playbook_name, status, output, started_at) \
                   VALUES (1, 'other', 'system_default', 'pb', 'success', 'x', '2026-07-04 10:00:00');",
            )
            .expect("seed runs");
        }
        let repo = PlaybookRepo::new(&db);
        // user スコープの 3 件のみ・started_at 降順。
        let all = repo.list_runs(&scope("u"), None).await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].started_at, "2026-07-03 10:00:00");
        // schedule_id=1 に絞ると 2 件。
        let s1 = repo.list_runs(&scope("u"), Some(1)).await.unwrap();
        assert_eq!(s1.len(), 2);
        assert!(s1.iter().all(|r| r.schedule_id == 1));
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
    async fn route_save_then_list() {
        let app = app();
        let save = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playbooks/save")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"hello","title":"Hi","keywords":["home"],"steps":"do it"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(save.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/playbooks")
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
        assert_eq!(j["playbooks"][0]["name"], serde_json::json!("hello"));
        assert_eq!(j["playbooks"][0]["keywords"][0], serde_json::json!("home"));
        // 内部列は露出しない（構造的フェイルクローズ）。
        assert!(j["playbooks"][0]["user_id"].is_null());
        assert!(j["playbooks"][0]["bot_id"].is_null());
        assert!(j["playbooks"][0]["id"].is_null());
        assert!(j["playbooks"][0]["created_at"].is_null());
    }

    #[tokio::test]
    async fn route_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/playbooks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── 定期実行スケジュール / 実行履歴（route レベル） ──

    async fn post_json(app: &axum::Router, uri: &str, body: &'static str) -> axum::response::Response {
        app.clone()
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
            .unwrap()
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn route_schedule_save_list_toggle_delete() {
        let app = app();
        // playbook を先に作らないと schedule save は 400（マクロが見つかりません）。
        let pb = post_json(
            &app,
            "/api/playbooks/save",
            r#"{"name":"daily","title":"Daily","steps":"go"}"#,
        )
        .await;
        assert_eq!(pb.status(), StatusCode::OK);

        // schedules/save 成功 → 200 + {success, message, schedule}。
        let save = post_json(
            &app,
            "/api/playbooks/schedules/save",
            r#"{"playbook_name":"daily","cron_expression":"0 8 * * *"}"#,
        )
        .await;
        assert_eq!(save.status(), StatusCode::OK);
        let sj = body_json(save).await;
        assert_eq!(sj["success"], serde_json::json!(true));
        assert_eq!(sj["message"], serde_json::json!("スケジュール「daily」を保存しました。"));
        assert_eq!(sj["schedule"]["playbook_name"], serde_json::json!("daily"));
        assert_eq!(sj["schedule"]["enabled"], serde_json::json!(true));
        // 内部所有者列 user_id は露出しない。
        assert!(sj["schedule"]["user_id"].is_null());
        let id = sj["schedule"]["id"].as_i64().expect("id");

        // schedules 一覧に 1 件。
        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/playbooks/schedules")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let lj = body_json(list).await;
        assert_eq!(lj["schedules"].as_array().unwrap().len(), 1);

        // toggle（無効化）→ 200 + 無効化メッセージ。
        let toggle = post_json(
            &app,
            "/api/playbooks/schedules/toggle",
            // format! を避け固定 id=1（AUTOINCREMENT の最初の行）。
            r#"{"id":1,"enabled":false}"#,
        )
        .await;
        assert_eq!(id, 1);
        assert_eq!(toggle.status(), StatusCode::OK);
        let tj = body_json(toggle).await;
        assert_eq!(tj["message"], serde_json::json!("スケジュールを無効化しました。"));

        // delete → 200 + 削除メッセージ。
        let del = post_json(&app, "/api/playbooks/schedules/delete", r#"{"id":1}"#).await;
        assert_eq!(del.status(), StatusCode::OK);
        let dj = body_json(del).await;
        assert_eq!(dj["message"], serde_json::json!("スケジュールを削除しました。"));
    }

    #[tokio::test]
    async fn route_schedule_save_missing_fields_400() {
        let app = app();
        let resp = post_json(
            &app,
            "/api/playbooks/schedules/save",
            r#"{"playbook_name":"","cron_expression":""}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let j = body_json(resp).await;
        assert_eq!(j["success"], serde_json::json!(false));
        assert_eq!(
            j["message"],
            serde_json::json!("playbookNameとcronExpressionは必須です。")
        );
    }

    #[tokio::test]
    async fn route_schedule_save_unknown_playbook_400() {
        let app = app();
        let resp = post_json(
            &app,
            "/api/playbooks/schedules/save",
            r#"{"playbook_name":"ghost","cron_expression":"0 8 * * *"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let j = body_json(resp).await;
        assert_eq!(j["message"], serde_json::json!("マクロ「ghost」が見つかりません。"));
    }

    #[tokio::test]
    async fn route_toggle_missing_id_400() {
        let app = app();
        let resp = post_json(&app, "/api/playbooks/schedules/toggle", r#"{"enabled":true}"#).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let j = body_json(resp).await;
        assert_eq!(j["message"], serde_json::json!("idは必須です。"));
    }

    #[tokio::test]
    async fn route_delete_missing_id_400() {
        let app = app();
        let resp = post_json(&app, "/api/playbooks/schedules/delete", r#"{}"#).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let j = body_json(resp).await;
        assert_eq!(j["message"], serde_json::json!("idは必須です。"));
    }

    #[tokio::test]
    async fn route_toggle_unknown_id_400() {
        let app = app();
        let resp = post_json(&app, "/api/playbooks/schedules/toggle", r#"{"id":999,"enabled":true}"#).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let j = body_json(resp).await;
        assert_eq!(j["message"], serde_json::json!("スケジュールが見つかりません。"));
    }

    #[tokio::test]
    async fn route_runs_empty_ok() {
        let app = app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/playbooks/runs")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["runs"].as_array().unwrap().len(), 0);
    }
}
