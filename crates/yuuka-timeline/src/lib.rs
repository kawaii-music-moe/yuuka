//! yuuka-timeline — timeline ドメイン（T1 fan-out・**参照実装は yuuka-todo**）: Repo + wire DTO
//! と route を縦に持つ。凍結契約（core/types）は変更しない。
//! 依存 DAG は `timeline → web, db, types, core`。ルータは supervisor が共通レイヤ配下にマージする。
//!
//! 参照スコープ = `timeline_records` のコア CRUD（day list / add / delete）＋
//! `day_plan_blocks` CRUD（plan add / update / delete・day に blocks 同梱）＋ cross-domain 副作用
//! （`type=expense` の expenses 二重登録・`type=task_done` の todos 完了連携）＋ メディア保存/配信
//! （[`media`]・`/api/timeline/media*`）。tool 経由の Discord 添付 URL 取得のみ deferred（reqwest 依存）。

pub mod dto;
pub mod media;
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
    <dto::TimelineRecord as TS>::export_all(&cfg)?;
    <dto::NewTimelineRecord as TS>::export_all(&cfg)?;
    <dto::DayPlanBlock as TS>::export_all(&cfg)?;
    <dto::NewDayPlanBlock as TS>::export_all(&cfg)?;
    <dto::UpdatePlanBlock as TS>::export_all(&cfg)?;
    <dto::TimelineDayData as TS>::export_all(&cfg)?;
    <dto::TimelineRecordData as TS>::export_all(&cfg)?;
    <dto::DayPlanBlockData as TS>::export_all(&cfg)?;
    <dto::NewTimelineMedia as TS>::export_all(&cfg)?;
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

    use crate::dto::{NewDayPlanBlock, NewTimelineRecord, UpdatePlanBlock};
    use crate::repo::TimelineRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // 実効スキーマは `Db::open`（= 内部で run_migrations）が V17__baseline.sql を丸ごと適用して
    // 作る（timeline_records/day_plan_blocks/expenses/todos すべて含む）。テスト側で部分 DDL を
    // 手書きすると後続 INDEX（例 idx_todos_parent ON todos(parent_id)）と衝突するため、空ファイル
    // からマイグレーションに一任する（finance テストと同方針）。cross-domain（expenses/todos）も
    // これで正しい列構成が揃う。
    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_timeline_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            // ReadPool は READ_ONLY で開くため、writer がマイグレーションを流す前に空ファイルを作る。
            let conn = rusqlite::Connection::open(&path).expect("create empty db file");
            drop(conn);
        }
        let db = Db::open(&path).expect("open db");
        // expenses/todos は users への FK を張るため、対象ユーザーを先に作る（Node は作成済み前提）。
        {
            let conn = rusqlite::Connection::open(&path).expect("seed users conn");
            for uid in ["u", "userA", "userB", "other"] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                     VALUES (?1, ?1, 'x', 'x')",
                    rusqlite::params![uid],
                )
                .expect("seed user");
            }
        }
        db
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
            amount: None,
            category: None,
            location: None,
        }
    }

    fn new_plan(date: &str, ty: &str, title: &str) -> NewDayPlanBlock {
        NewDayPlanBlock {
            date: date.to_owned(),
            r#type: ty.to_owned(),
            title: title.to_owned(),
            description: None,
            start_time: None,
            end_time: None,
            todo_id: None,
            transit_from: None,
            transit_to: None,
            transit_line: None,
            position: None,
        }
    }

    /// キーを一切セットしない更新（全 nullable フィールドは外側 `None`＝キー非存在）。
    fn empty_update(id: i64) -> UpdatePlanBlock {
        UpdatePlanBlock {
            id,
            title: None,
            description: None,
            r#type: None,
            start_time: None,
            end_time: None,
            todo_id: None,
            transit_from: None,
            transit_to: None,
            transit_line: None,
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

    #[tokio::test]
    async fn add_expense_record_links_expenses_row() {
        // cross-domain: type=expense は expenses へ二次登録し expense_id/expense_category を連結。
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        let mut input = new_memo("2026-07-06", "ランチ");
        input.r#type = "expense".to_owned();
        input.recorded_at = Some("2026-07-06 12:34:56".to_owned());
        let record = repo
            .add_expense_record(&scope("u"), &input, 1500.0, "食費".to_owned())
            .await
            .unwrap();
        assert_eq!(record.r#type, "expense");
        assert_eq!(record.expense_category.as_deref(), Some("食費"));
        assert_eq!(record.amount, Some(1500.0));
        let expense_id = record.expense_id.expect("expense_id connected");

        // expenses に対応行（source='timeline'・memo=title・time=recorded_at slice・amount）。
        let (amount, source, memo, time): (i64, String, String, String) = db
            .read
            .read(move |conn| {
                conn.query_row(
                    "SELECT amount, source, memo, time FROM expenses WHERE id = ?1",
                    rusqlite::params![expense_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(amount, 1500);
        assert_eq!(source, "timeline");
        assert_eq!(memo, "ランチ");
        assert_eq!(time, "12:34:56"); // recorded_at.slice(11,19)

        // 別ユーザーには見えない（スコープ分離）。
        assert!(repo.list(&scope("other"), Some("2026-07-06".to_owned())).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn add_expense_record_rounds_amount_for_integer_expenses_column() {
        // 非整数 amount でも expenses.amount(INTEGER 列)は整数格納し、i64 read が 500 に落ちないこと。
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        let mut input = new_memo("2026-07-06", "端数");
        input.r#type = "expense".to_owned();
        let record = repo
            .add_expense_record(&scope("u"), &input, 1500.5, "食費".to_owned())
            .await
            .unwrap();
        // timeline_records.amount は REAL 列なので raw f64（Node の値と一致）。
        assert_eq!(record.amount, Some(1500.5));
        let expense_id = record.expense_id.unwrap();
        // expenses.amount は i64 として読める（round 済み・型エラーで落ちない）。
        let amount: i64 = db
            .read
            .read(move |conn| {
                conn.query_row(
                    "SELECT amount FROM expenses WHERE id = ?1",
                    rusqlite::params![expense_id],
                    |r| r.get(0),
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(amount, 1501); // 1500.5 は round-half-away で 1501。

        // finance の list 相当（expenses を i64 で全件読む）が型エラーで落ちないこと。
        let count: i64 = db
            .read
            .read(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM expenses WHERE typeof(amount) = 'integer'",
                    [],
                    |r| r.get(0),
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn task_done_completes_linked_todo_atomically() {
        // cross-domain: type=task_done + todo_id は同一 tx で紐付き todos を done にする。
        let db = seed_db();
        db.writer
            .transaction(|tx| {
                tx.execute(
                    "INSERT INTO todos (id, user_id, bot_id, title, status) \
                     VALUES (7, 'u', 'system_default', 'やること', 'open')",
                    [],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        let repo = TimelineRepo::new(&db);
        let mut input = new_memo("2026-07-06", "完了メモ");
        input.r#type = "task_done".to_owned();
        input.todo_id = Some(7);
        let record = repo.add(&scope("u"), input).await.unwrap();
        assert_eq!(record.r#type, "task_done");
        assert_eq!(record.todo_id, Some(7));

        let status: String = db
            .read
            .read(|conn| {
                conn.query_row("SELECT status FROM todos WHERE id = 7", [], |r| r.get(0))
                    .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(status, "done");
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
    async fn route_expense_record_double_registers_and_validates() {
        let app = app();
        // type=expense: 200・record.expense_id 連結（家計簿へ二重登録された証跡）。
        let ok = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/record")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"date":"2026-07-06","type":"expense","amount":1500,"category":"食費","title":"ランチ"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(ok.into_body(), usize::MAX).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["record"]["type"], serde_json::json!("expense"));
        assert_eq!(j["record"]["expense_category"], serde_json::json!("食費"));
        assert!(j["record"]["expense_id"].is_i64());

        // amount 欠落は 400（Node `amount が必要です。`）。
        let bad = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/record")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"date":"2026-07-06","type":"expense"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn media_upload_and_serve_roundtrip() {
        use base64::Engine as _;

        // media_dir を実ディレクトリ（tempdir）に差し替えた AppState を作る。
        let media_dir = tempfile::tempdir().expect("media tempdir");
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        let config = WebConfig {
            media_dir: media_dir.path().to_path_buf(),
            ..WebConfig::default()
        };
        let app = super::routes().with_state(AppState::new(auth, config, seed_db()));

        // base64 "hello" を image/png としてアップロード。
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let body = serde_json::json!({
            "date": "2026-07-06",
            "base64": b64,
            "mimeType": "image/png",
            "title": "写真"
        });
        let up = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/media")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(up.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(up.into_body(), usize::MAX).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["record"]["type"], serde_json::json!("media"));
        assert_eq!(j["record"]["media_type"], serde_json::json!("photo"));
        let filename = j["record"]["media_path"].as_str().unwrap().to_owned();
        assert!(filename.ends_with(".png"));

        // 配信: 認証付きで 200・Content-Type + 本文一致。
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/timeline/media/{filename}"))
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(
            get.headers().get("content-type").unwrap(),
            "image/png"
        );
        let served = axum::body::to_bytes(get.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&served[..], b"hello");

        // 未存在ファイルは 404。
        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/timeline/media/nope.jpg")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        // 未認証は 401（配信も auth:user）。
        let unauth = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/timeline/media/{filename}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
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

    /// wire 契約の直接凍結: 入力 DTO は camelCase（`recordedAt`/`todoId`）を受理し、snake_case は
    /// 拾わない。内部列（`expenseId`/`mediaPath`/`mediaType`/`expenseCategory`）は DTO に存在せず、
    /// 送っても無視する（M-8: 素通し INSERT を防ぐ・Node と同じく 400 にはしない）。
    #[test]
    fn new_timeline_record_wire_contract() {
        let camel: NewTimelineRecord = serde_json::from_str(
            r#"{"date":"2026-07-06","type":"memo","recordedAt":"2026-07-06 09:00:00","todoId":123,"content":"x"}"#,
        )
        .unwrap();
        assert_eq!(camel.recorded_at.as_deref(), Some("2026-07-06 09:00:00"));
        assert_eq!(camel.todo_id, Some(123));

        // snake_case は camelCase 契約では拾われない。
        let snake: NewTimelineRecord = serde_json::from_str(
            r#"{"date":"2026-07-06","type":"memo","recorded_at":"2026-07-06 09:00:00"}"#,
        )
        .unwrap();
        assert_eq!(snake.recorded_at, None);

        // 内部列を送っても DTO に存在しないため無視される（デシリアライズは成功）。
        let ignored: NewTimelineRecord = serde_json::from_str(
            r#"{"date":"2026-07-06","type":"memo","expenseId":9,"mediaPath":"/etc/passwd","mediaType":"video","expenseCategory":"food"}"#,
        )
        .unwrap();
        assert_eq!(ignored.date, "2026-07-06");

        // `amount` は Node が body から読むため受理する。
        let expense: NewTimelineRecord =
            serde_json::from_str(r#"{"date":"2026-07-06","type":"expense","amount":1500}"#).unwrap();
        assert_eq!(expense.amount, Some(1500.0));
    }

    /// M-8 回帰: クライアントが内部列を送り込んでも INSERT されない（NULL のまま）。
    #[tokio::test]
    async fn route_record_ignores_internal_columns() {
        let app = app();
        let add = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/record")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"date":"2026-07-06","type":"memo","title":"t","mediaPath":"/etc/passwd","mediaType":"video","expenseId":99,"expenseCategory":"x"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(add.into_body(), usize::MAX).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // 内部列はクライアント設定不可 → NULL のまま。
        assert!(j["record"]["media_path"].is_null());
        assert!(j["record"]["media_type"].is_null());
        assert!(j["record"]["expense_id"].is_null());
        assert!(j["record"]["expense_category"].is_null());
    }

    // ── day_plan_blocks（plan CRUD）──────────────────────────────────────────────

    #[tokio::test]
    async fn plan_add_list_and_scope_isolation() {
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        let created = repo
            .add_plan(&scope("userA"), new_plan("2026-07-06", "event", "会議"))
            .await
            .unwrap();
        assert_eq!(created.title, "会議");
        assert_eq!(created.r#type, "event");
        // position 未指定は 0（Node parity）。
        assert_eq!(created.position, 0);
        assert!(created.start_time.is_none());

        let listed = repo
            .list_plans(&scope("userA"), Some("2026-07-06".to_owned()))
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "会議");

        // 別日には出ない。
        assert!(repo
            .list_plans(&scope("userA"), Some("2026-07-07".to_owned()))
            .await
            .unwrap()
            .is_empty());
        // 別ユーザーには見えない（分離キーを型で強制）。
        assert!(repo
            .list_plans(&scope("userB"), Some("2026-07-06".to_owned()))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn plan_list_ordering() {
        // start_time IS NULL は最後・非 NULL は start_time ASC・同点は position ASC（Node parity）。
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        let s = scope("u");

        let mut a = new_plan("2026-07-06", "event", "早朝");
        a.start_time = Some("09:00".to_owned());
        a.position = Some(5);
        repo.add_plan(&s, a).await.unwrap();

        let mut b = new_plan("2026-07-06", "event", "夕方");
        b.start_time = Some("18:00".to_owned());
        repo.add_plan(&s, b).await.unwrap();

        // 時間未定（NULL）を 2 件・position で順序決定。
        let mut c = new_plan("2026-07-06", "free", "自由2");
        c.position = Some(2);
        repo.add_plan(&s, c).await.unwrap();
        let mut d = new_plan("2026-07-06", "free", "自由1");
        d.position = Some(1);
        repo.add_plan(&s, d).await.unwrap();

        let listed = repo
            .list_plans(&s, Some("2026-07-06".to_owned()))
            .await
            .unwrap();
        let titles: Vec<&str> = listed.iter().map(|b| b.title.as_str()).collect();
        assert_eq!(titles, vec!["早朝", "夕方", "自由1", "自由2"]);
    }

    #[tokio::test]
    async fn plan_update_partial_and_null_semantics() {
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        let s = scope("u");
        let mut base = new_plan("2026-07-06", "event", "元タイトル");
        base.start_time = Some("10:00".to_owned());
        base.description = Some("説明".to_owned());
        base.todo_id = Some(42);
        let created = repo.add_plan(&s, base).await.unwrap();

        // title を string で更新・start_time はキー存在で null にできる・
        // end_time はキー非存在なので不変。description は空文字→NULL。
        let upd = UpdatePlanBlock {
            id: created.id,
            title: Some("新タイトル".to_owned()),
            description: Some(String::new()), // 空文字 → NULL
            r#type: None,
            start_time: Some(None), // キー存在・null → NULL 化
            end_time: None,         // キー非存在 → 不変
            todo_id: None,          // キー非存在 → 不変（42 のまま）
            transit_from: None,
            transit_to: None,
            transit_line: None,
        };
        let updated = repo.update_plan(&s, upd).await.unwrap().expect("row");
        assert_eq!(updated.title, "新タイトル");
        assert!(updated.description.is_none()); // 空文字 → NULL
        assert!(updated.start_time.is_none()); // null 明示 → NULL
        assert_eq!(updated.r#type, "event"); // 不変
        assert_eq!(updated.todo_id, Some(42)); // キー非存在 → 不変
        assert!(updated.updated_at.len() >= "2026-01-01".len());
    }

    #[tokio::test]
    async fn plan_update_empty_touches_updated_at_only() {
        // Node: SET 対象が無くても updated_at は更新され changes>0 → 行を返す。
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        let s = scope("u");
        let created = repo
            .add_plan(&s, new_plan("2026-07-06", "event", "t"))
            .await
            .unwrap();
        let updated = repo
            .update_plan(&s, empty_update(created.id))
            .await
            .unwrap()
            .expect("row");
        assert_eq!(updated.title, "t"); // 不変
    }

    #[tokio::test]
    async fn plan_update_not_found_and_scope() {
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        let created = repo
            .add_plan(&scope("owner"), new_plan("2026-07-06", "event", "t"))
            .await
            .unwrap();
        // 存在しない id → None。
        assert!(repo
            .update_plan(&scope("owner"), empty_update(999_999))
            .await
            .unwrap()
            .is_none());
        // 別ユーザーからは更新できない → None（scope 分離）。
        assert!(repo
            .update_plan(&scope("intruder"), empty_update(created.id))
            .await
            .unwrap()
            .is_none());
        // owner から見ればタイトルは無傷。
        assert_eq!(
            repo.get_plan(&scope("owner"), created.id)
                .await
                .unwrap()
                .expect("row")
                .title,
            "t"
        );
    }

    #[tokio::test]
    async fn plan_delete_and_double_delete() {
        let db = seed_db();
        let repo = TimelineRepo::new(&db);
        let s = scope("u");
        let created = repo
            .add_plan(&s, new_plan("2026-07-06", "event", "t"))
            .await
            .unwrap();
        // 別ユーザーからは削除不可。
        assert!(!repo.delete_plan(&scope("other"), created.id).await.unwrap());
        assert!(repo.delete_plan(&s, created.id).await.unwrap());
        // 二重削除は false（Node parity・200 {success:false}）。
        assert!(!repo.delete_plan(&s, created.id).await.unwrap());
    }

    /// wire 契約: plan 作成は camelCase（`startTime`/`todoId`/`transitFrom`）を受理する。
    #[test]
    fn new_plan_wire_contract() {
        let camel: NewDayPlanBlock = serde_json::from_str(
            r#"{"date":"2026-07-06","type":"transit","title":"移動","startTime":"08:00","endTime":"08:30","todoId":7,"transitFrom":"渋谷","transitTo":"新宿","transitLine":"山手線","position":3}"#,
        )
        .unwrap();
        assert_eq!(camel.start_time.as_deref(), Some("08:00"));
        assert_eq!(camel.end_time.as_deref(), Some("08:30"));
        assert_eq!(camel.todo_id, Some(7));
        assert_eq!(camel.transit_from.as_deref(), Some("渋谷"));
        assert_eq!(camel.position, Some(3));
    }

    /// wire 契約: 更新の二重 Option がキー存在（`Some(None)`）とキー非存在（`None`）を区別する。
    #[test]
    fn update_plan_present_vs_absent() {
        // startTime を明示 null で送る → キー存在・内側 None。
        let present_null: UpdatePlanBlock =
            serde_json::from_str(r#"{"id":1,"startTime":null}"#).unwrap();
        assert_eq!(present_null.start_time, Some(None));
        // endTime キー非存在 → 外側 None（不変扱い）。
        assert_eq!(present_null.end_time, None);

        // startTime を値付きで送る → Some(Some(..))。
        let present_val: UpdatePlanBlock =
            serde_json::from_str(r#"{"id":1,"startTime":"07:00"}"#).unwrap();
        assert_eq!(present_val.start_time, Some(Some("07:00".to_owned())));

        // title は string ガード: 数値/ null は拾わない（外側 None）。
        let bad_title: UpdatePlanBlock =
            serde_json::from_str(r#"{"id":1,"title":null}"#).unwrap();
        assert_eq!(bad_title.title, None);
    }

    #[tokio::test]
    async fn route_plan_add_update_delete_and_day() {
        let app = app();
        // 作成。
        let add = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/plan")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"date":"2026-07-06","type":"event","title":"会議","startTime":"10:00"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(add.into_body(), usize::MAX).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["block"]["title"], serde_json::json!("会議"));
        // 内部列は露出しない（構造的フェイルクローズ）。
        assert!(j["block"]["user_id"].is_null());
        assert!(j["block"]["bot_id"].is_null());
        let id = j["block"]["id"].as_i64().unwrap();

        // 更新。
        let upd = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/plan/update")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"id":{id},"title":"打合せ"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(upd.status(), StatusCode::OK);
        let ub = axum::body::to_bytes(upd.into_body(), usize::MAX).await.unwrap();
        let uj: serde_json::Value = serde_json::from_slice(&ub).unwrap();
        assert_eq!(uj["block"]["title"], serde_json::json!("打合せ"));

        // day は blocks と records の両方を返す（Node parity）。
        let day = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/timeline/day?date=2026-07-06")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(day.status(), StatusCode::OK);
        let db_ = axum::body::to_bytes(day.into_body(), usize::MAX).await.unwrap();
        let dj: serde_json::Value = serde_json::from_slice(&db_).unwrap();
        assert_eq!(dj["blocks"][0]["title"], serde_json::json!("打合せ"));
        assert!(dj["records"].is_array());

        // 削除。
        let del = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/plan/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(del.status(), StatusCode::OK);
        let delb = axum::body::to_bytes(del.into_body(), usize::MAX).await.unwrap();
        let dlj: serde_json::Value = serde_json::from_slice(&delb).unwrap();
        assert_eq!(dlj["success"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn route_plan_add_missing_fields_is_400() {
        // title 欠落 → 400（Node parity）。
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/plan")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"date":"2026-07-06","type":"event"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn route_plan_update_not_found_is_404() {
        // 存在しない id → 404（Node parity・delete の 200 とは異なる）。
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/plan/update")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":424242,"title":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn route_plan_update_id_zero_is_400() {
        // id=0 は Node の `!id` で 400（0 は falsy）。
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/plan/update")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":0,"title":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn route_plan_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/timeline/plan")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"date":"2026-07-06","type":"event","title":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
