//! yuuka-supervisor（lib）— アプリ組立とライフサイクル。
//!
//! Phase 1: フレームワークルート（yuuka-web）と各ドメイン（T1）のルータを merge し、
//! 共通レイヤ（CSRF/body 上限/セキュリティヘッダ）を被せて完成ルータを返す。
//! config 読込・AuthBackend 実装・`axum::serve`・JoinSet 監督は後続増分で追加する。
//! DAG: `supervisor → web, todo, …`（ドメインを束ねる最下流）。

use axum::Router;
use yuuka_web::{apply_common_layers, framework_routes, AppState};

/// 完成アプリのルータを組み立てる（フレームワーク + 全ドメイン + 共通レイヤ）。
///
/// 新ドメイン（finance/schedule/…）は `.merge(yuuka_xxx::routes())` を足す。
pub fn build_app(state: AppState) -> Router {
    let routes = framework_routes().merge(yuuka_todo::routes());
    apply_common_layers(routes).with_state(state)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    struct FakeAuth;

    #[async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok((token == "good").then(|| SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            }))
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    fn test_db() -> Db {
        let path = std::env::temp_dir()
            .join(format!("yuuka_sup_test_{}.sqlite", std::process::id()));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed");
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS todos (id INTEGER PRIMARY KEY AUTOINCREMENT, \
                 user_id TEXT NOT NULL, bot_id TEXT NOT NULL DEFAULT 'system_default', \
                 title TEXT NOT NULL, description TEXT, due_date TEXT, start_date TEXT, \
                 priority TEXT, tags TEXT NOT NULL DEFAULT '[]', status TEXT NOT NULL DEFAULT 'open', \
                 progress INTEGER NOT NULL DEFAULT 0, parent_id INTEGER, linked_payment_id INTEGER, \
                 due_reminded INTEGER NOT NULL DEFAULT 0, repeat_rule TEXT, repeat_until TEXT, \
                 repeat_count INTEGER, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);",
            )
            .expect("ddl");
        }
        Db::open(&path).expect("open")
    }

    fn app() -> Router {
        super::build_app(AppState::new(Arc::new(FakeAuth), WebConfig::default(), test_db()))
    }

    #[tokio::test]
    async fn merged_app_serves_framework_and_domain_routes() {
        // framework route /api/me（web）。
        let me = app()
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(me.status(), StatusCode::OK);

        // domain route /api/tasks（todo）— 認証必須が効く。
        let tasks_unauth = app()
            .oneshot(Request::builder().uri("/api/tasks").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(tasks_unauth.status(), StatusCode::UNAUTHORIZED);

        let tasks = app()
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tasks.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn csrf_layer_applies_to_domain_post() {
        // 共通レイヤ（CSRF）がドメイン POST にも被さる: Cookie×cross-site → 403。
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("sec-fetch-site", "cross-site")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
