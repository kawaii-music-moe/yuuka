//! yuuka-schedule — schedule ドメイン（T1 fan-out）。**参照実装は yuuka-todo**（repo/dto/routes の縦スライス）。
//!
//! 参照スコープ = コア CRUD（list_upcoming/add/delete）。Google カレンダー同期
//! （google_event_id/google_calendar_id・link/update/backfill）、期間集約
//! （listSchedulesInRange・日報/週報）は deferred（後続パスで追加）。cron リマインド
//! （getUnreminded/markReminded）は Phase 4 で [`cron`] に追加済み。DAG: `schedule → web, db, types, core`。

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
    <dto::Schedule as TS>::export_all(&cfg)?;
    <dto::NewSchedule as TS>::export_all(&cfg)?;
    <dto::ScheduleListData as TS>::export_all(&cfg)?;
    <dto::ScheduleData as TS>::export_all(&cfg)?;
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

    use crate::dto::NewSchedule;
    use crate::repo::ScheduleRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // schedules テーブル: CREATE TABLE 定義 + 移行で後付けされる bot_id 列を含む。
    const SCHEDULES_DDL: &str = "CREATE TABLE schedules (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        title TEXT NOT NULL,
        description TEXT,
        start_at TEXT NOT NULL,
        end_at TEXT,
        remind_before_minutes INTEGER NOT NULL DEFAULT 10,
        reminded INTEGER NOT NULL DEFAULT 0,
        google_event_id TEXT,
        google_calendar_id TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_schedule_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(SCHEDULES_DDL).expect("create schedules");
        }
        Db::open(&path).expect("open db")
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    /// 開始時刻を「now から +offset 日」の localtime で作る（list_upcoming の窓に載せる）。
    fn start_in_days(offset: i64) -> String {
        format!("datetime('now','localtime','+{offset} days')")
    }

    fn new_schedule(title: &str, start_expr: &str) -> NewSchedule {
        NewSchedule {
            title: title.to_owned(),
            // start_at は SQL 式ではなく実値で渡す必要があるため、テスト側で評価済み文字列を使う。
            start_at: start_expr.to_owned(),
            end_at: None,
            remind_before_minutes: None,
            description: None,
        }
    }

    /// now 基準の相対式（+N days）を評価して具体的な datetime 文字列にする。
    /// start_at には SQL 式ではなく実値を渡す必要があるため。任意の接続で評価できる。
    fn eval_datetime(expr: &str) -> String {
        let conn = rusqlite::Connection::open_in_memory().expect("mem conn");
        conn.query_row(&format!("SELECT {expr}"), [], |r| r.get::<_, String>(0))
            .expect("eval datetime")
    }

    #[tokio::test]
    async fn add_list_and_scope_isolation() {
        let db = seed_db();
        let repo = ScheduleRepo::new(&db);

        let start = eval_datetime(&start_in_days(1));
        let created = repo
            .add(&scope("userA"), new_schedule("a-meeting", &start))
            .await
            .unwrap();
        assert_eq!(created.title, "a-meeting");
        // remind_before_minutes 未指定は既定 10（Node parity）。
        assert_eq!(created.remind_before_minutes, 10);

        let listed = repo.list_upcoming(&scope("userA"), 7).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "a-meeting");

        // 別ユーザーには見えない（分離キーを型で強制）。
        assert!(repo
            .list_upcoming(&scope("userB"), 7)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn list_upcoming_windows_by_days() {
        let db = seed_db();
        let repo = ScheduleRepo::new(&db);

        let soon = eval_datetime(&start_in_days(2));
        let far = eval_datetime(&start_in_days(30));
        repo.add(&scope("u"), new_schedule("soon", &soon))
            .await
            .unwrap();
        repo.add(&scope("u"), new_schedule("far", &far))
            .await
            .unwrap();

        // 7 日窓には soon のみ。
        let listed = repo.list_upcoming(&scope("u"), 7).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "soon");

        // 60 日窓には両方。
        let listed = repo.list_upcoming(&scope("u"), 60).await.unwrap();
        assert_eq!(listed.len(), 2);
    }

    #[tokio::test]
    async fn delete_scoped() {
        let db = seed_db();
        let repo = ScheduleRepo::new(&db);
        let start = eval_datetime(&start_in_days(1));
        let created = repo
            .add(&scope("u"), new_schedule("t", &start))
            .await
            .unwrap();

        // 別スコープからは削除できない。
        assert!(!repo.delete(&scope("other"), created.id).await.unwrap());
        // 自スコープなら削除できる。
        assert!(repo.delete(&scope("u"), created.id).await.unwrap());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), created.id).await.unwrap());
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
        // now 基準の相対式で確実に窓に載る開始時刻を実値化する。
        let start = {
            let conn = rusqlite::Connection::open_in_memory().expect("mem conn");
            conn.query_row("SELECT datetime('now','localtime','+3 days')", [], |r| {
                r.get::<_, String>(0)
            })
            .expect("eval start")
        };
        let body = serde_json::json!({ "title": "standup", "startAt": start }).to_string();
        let add = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/schedules/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/schedules?days=7")
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
        assert_eq!(j["schedules"][0]["title"], serde_json::json!("standup"));
        // 内部・Google 同期列は露出しない（構造的フェイルクローズ）。
        assert!(j["schedules"][0]["user_id"].is_null());
        assert!(j["schedules"][0]["bot_id"].is_null());
        assert!(j["schedules"][0]["reminded"].is_null());
        assert!(j["schedules"][0]["google_event_id"].is_null());
        assert!(j["schedules"][0]["google_calendar_id"].is_null());
    }

    #[tokio::test]
    async fn route_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/schedules")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// M-12 golden: `POST /api/schedules/delete` は `200 {success:<bool>}`（`deletedId` 無し）。
    /// 実在削除は `success:true`、該当無（二重削除）は **404 ではなく** `200 {success:false}`。
    #[tokio::test]
    async fn route_delete_returns_bare_success_without_deleted_id() {
        let app = app();
        let add = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/schedules/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"title":"del-me","startAt":"2999-01-01 09:00:00"}"#,
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
        let id = j["schedule"]["id"].as_i64().expect("id");

        let del = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/schedules/delete")
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
        assert!(j["deletedId"].is_null());
        assert!(j["deleted_id"].is_null());

        // 二重削除（該当無）→ 200 {success:false}（404 にしない）。
        let del2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/schedules/delete")
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
}
