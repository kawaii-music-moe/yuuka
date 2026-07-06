//! yuuka-timeline — timeline ドメイン（T1 fan-out・**参照実装は yuuka-todo**）: Repo + wire DTO
//! と route を縦に持つ。凍結契約（core/types）は変更しない。
//! 依存 DAG は `timeline → web, db, types, core`。ルータは supervisor が共通レイヤ配下にマージする。
//!
//! Phase 1 参照スコープ = `timeline_records` のコア CRUD（day list / add / delete）。
//! day_plan_blocks CRUD・media 保存/配信・`type=expense` の expenses 二重登録・
//! `type=task_done` の todos 完了連携は完成パスで追加（deferred）。

pub mod dto;
pub mod repo;
pub mod routes;

pub use routes::routes;

use std::path::Path;

use ts_rs::TS;

/// 本ドメインの wire DTO を `base_dir/generated/` へ生成する（xtask gen-types が呼ぶ）。
///
/// # Errors
/// ts-rs のシリアライズ／書き込み失敗時 [`ts_rs::ExportError`]。
pub fn export_bindings(base_dir: &Path) -> Result<(), ts_rs::ExportError> {
    let cfg = ts_rs::Config::new().with_out_dir(base_dir.to_path_buf());
    <dto::TimelineRecord as TS>::export_all(&cfg)?;
    <dto::NewTimelineRecord as TS>::export_all(&cfg)?;
    <dto::TimelineDayData as TS>::export_all(&cfg)?;
    <dto::TimelineRecordData as TS>::export_all(&cfg)?;
    <dto::TimelineDeletedData as TS>::export_all(&cfg)?;
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

    use crate::dto::NewTimelineRecord;
    use crate::repo::TimelineRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    const RECORDS_DDL: &str = "CREATE TABLE timeline_records (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        date TEXT NOT NULL,
        recorded_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
        type TEXT NOT NULL DEFAULT 'memo',
        title TEXT,
        content TEXT,
        todo_id INTEGER,
        expense_id INTEGER,
        amount REAL,
        expense_category TEXT,
        media_path TEXT,
        media_type TEXT,
        location TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_timeline_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(RECORDS_DDL).expect("create timeline_records");
        }
        Db::open(&path).expect("open db")
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    fn new_memo(date: &str, title: &str) -> NewTimelineRecord {
        NewTimelineRecord {
            date: date.to_owned(),
            r#type: "memo".to_owned(),
            recorded_at: None,
            title: Some(title.to_owned()),
            content: None,
            todo_id: None,
            expense_id: None,
            amount: None,
            expense_category: None,
            media_path: None,
            media_type: None,
            location: None,
        }
    }

    #[tokio::test]
    async fn add_list_and_scope_isolation() {
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        repo.add(&scope("userA"), new_memo("2026-07-06", "hello"))
            .await
            .unwrap();

        let listed = repo
            .list(&scope("userA"), Some("2026-07-06".to_owned()))
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title.as_deref(), Some("hello"));
        assert_eq!(listed[0].r#type, "memo");
        assert!(!listed[0].recorded_at.is_empty());

        // 別日には出ない（date フィルタ）。
        assert!(repo
            .list(&scope("userA"), Some("2026-07-07".to_owned()))
            .await
            .unwrap()
            .is_empty());

        // 別ユーザーには見えない（分離キーを型で強制）。
        assert!(repo
            .list(&scope("userB"), Some("2026-07-06".to_owned()))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn get_and_delete() {
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        let created = repo
            .add(&scope("u"), new_memo("2026-07-06", "t"))
            .await
            .unwrap();

        let fetched = repo
            .get(&scope("u"), created.id)
            .await
            .unwrap()
            .expect("row");
        assert_eq!(fetched.id, created.id);

        // 別ユーザーからは get できない。
        assert!(repo.get(&scope("other"), created.id).await.unwrap().is_none());

        assert!(repo.delete(&scope("u"), created.id).await.unwrap());
        assert!(repo
            .list(&scope("u"), Some("2026-07-06".to_owned()))
            .await
            .unwrap()
            .is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), created.id).await.unwrap());
    }

    #[tokio::test]
    async fn recorded_at_passthrough() {
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        let mut input = new_memo("2026-07-06", "with-ts");
        input.recorded_at = Some("2026-07-06 09:30:00".to_owned());
        let created = repo.add(&scope("u"), input).await.unwrap();
        assert_eq!(created.recorded_at, "2026-07-06 09:30:00");
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
    async fn route_add_then_day() {
        let app = app();
        let add = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/record")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"date":"2026-07-06","type":"memo","title":"hi"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/timeline/day?date=2026-07-06")
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
        assert_eq!(j["records"][0]["title"], serde_json::json!("hi"));
        assert_eq!(j["records"][0]["type"], serde_json::json!("memo"));
        // 内部列は露出しない（構造的フェイルクローズ）。
        assert!(j["records"][0]["user_id"].is_null());
        assert!(j["records"][0]["bot_id"].is_null());
    }

    #[tokio::test]
    async fn day_without_date_falls_back_to_today() {
        // date 未指定は 400 ではなく本日にフォールバックして 200（Node parity）。
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/timeline/day")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn route_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/timeline/day?date=2026-07-06")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
