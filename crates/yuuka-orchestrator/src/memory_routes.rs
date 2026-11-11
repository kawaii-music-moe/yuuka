//! 会話記憶（シナプス）を利用者自身が見て消す Web-API（docs/design/memory_retention.md §4-E・全て auth:user）。
//!
//! - `GET /api/memories?botId=`: 本人 × Bot の kept を強度の高い順に返し、評価待ち（candidate）は件数だけ返す。
//! - `DELETE /api/memories/{id}?botId=`: 本人 × Bot の kept を 1 件消す。DB から消えたときだけ RAM 索引
//!   （[`SynapseEngine::forget`]）からも除く。他人の行・候補の行は消さず、Node パリティの
//!   **200 `{success:false}`** を返す（M-12・404 にしない）。
//!
//! スコープは共通の [`resolve_scope`]（`?botId=` を bot アクセス認可つきで解決・未指定は 403・
//! 未アクセスは 404）。記憶はこれまでどおり利用者 × Bot でしか読めない（§4-E プライバシー）。

use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use yuuka_synapse::SynapseEngine;
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db};

use crate::synapse_repo;

#[derive(Debug, Deserialize)]
struct BotIdQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

/// 記憶ルータ。RAM 索引は supervisor が持つ [`SynapseEngine`]（会話エンジンと同じ `Arc<Mutex>`）を注入する。
pub fn routes_with(engine: Arc<Mutex<SynapseEngine>>) -> Router<AppState> {
    Router::new()
        .route("/api/memories", get(list))
        .route("/api/memories/{id}", axum::routing::delete(remove))
        .layer(Extension(engine))
}

/// `GET /api/memories?botId=` — `{ success, memories: [...], pendingCount }`。
async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotIdQuery>,
) -> Result<Json<Value>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let (uid, bid) = (scope.user_id().as_str(), scope.bot_id().as_str());
    let memories = synapse_repo::list_kept(&db, uid, bid).await?;
    let pending = synapse_repo::count_candidates(&db, uid, bid).await?;
    Ok(Json(json!({
        "success": true,
        "memories": memories,
        "pendingCount": pending,
    })))
}

/// `DELETE /api/memories/{id}?botId=` — 本人 × Bot の kept なら消して `{success:true}`、
/// 該当無（他人の行・候補・存在しない id）は `{success:false}`。
async fn remove(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(engine): Extension<Arc<Mutex<SynapseEngine>>>,
    Path(id): Path<i64>,
    Query(q): Query<BotIdQuery>,
) -> Result<Json<Value>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let deleted =
        synapse_repo::delete_kept_owned(&db, scope.user_id().as_str(), scope.bot_id().as_str(), id)
            .await?;
    if deleted {
        engine.lock().await.forget(id);
    }
    Ok(Json(json!({ "success": deleted })))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tokio::sync::Mutex;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_synapse::SynapseEngine;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::test_support::{seed_bot, TEST_BOT_ID};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    use crate::synapse_repo::{self, InsertArgs, STATUS_CANDIDATE, STATUS_KEPT};

    struct FakeAuth(SessionUser);

    #[async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok((token == "good").then(|| self.0.clone()))
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    fn auth(uid: &str) -> Arc<FakeAuth> {
        Arc::new(FakeAuth(SessionUser {
            discord_id: uid.to_owned(),
            username: uid.to_owned(),
            role: Role::User,
        }))
    }

    async fn setup() -> (Db, Arc<Mutex<SynapseEngine>>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        // Rust は存在しない DB を作らない。空ファイルを作って `Db::open` の migrations に任せる。
        drop(rusqlite::Connection::open(&path).unwrap());
        let db = Db::open(Path::new(&path)).unwrap();
        // 利用者 A と B がそれぞれ同じ Bot を使える（B は共有先）。
        seed_bot(&db, TEST_BOT_ID, "userA").await;
        db.writer
            .transaction(|tx| {
                tx.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, salt) VALUES ('userB', 'userB', '')",
                    [],
                )
                .map_err(yuuka_db::map_sqlite)?;
                tx.execute(
                    "INSERT INTO bot_shares (bot_id, shared_user_id, status) VALUES (?1, 'userB', 'active')",
                    [TEST_BOT_ID],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        let (engine, _) = SynapseEngine::boot(&path, yuuka_synapse::DEFAULT_DIM);
        (db, Arc::new(Mutex::new(engine)), dir)
    }

    async fn insert(db: &Db, user: &str, content: &str, status: &'static str) -> i64 {
        synapse_repo::insert_synapse(
            db,
            InsertArgs {
                user_id: user.into(),
                bot_id: TEST_BOT_ID.into(),
                guild_id: None,
                content: content.into(),
                topic_id: None,
                source_msg_id: None,
                ctx_tod: None,
                ctx_dow: None,
                status,
                content_hash: None,
            },
        )
        .await
        .unwrap()
    }

    fn app(db: &Db, engine: &Arc<Mutex<SynapseEngine>>, uid: &str) -> axum::Router {
        super::routes_with(engine.clone()).with_state(AppState::new(
            auth(uid),
            WebConfig::default(),
            db.clone(),
        ))
    }

    fn req(method: &str, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", "__Host-yuuka-session=good")
            .body(Body::empty())
            .unwrap()
    }

    async fn json_of(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn count_rows(db: &Db) -> i64 {
        db.read
            .read(|conn| {
                conn.query_row("SELECT count(*) FROM synapses", [], |r| r.get(0))
                    .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap()
    }

    /// 一覧は本人の kept だけ（他人の行・候補は出ない）。候補は件数だけ返る。
    #[tokio::test]
    async fn list_returns_only_own_kept_and_pending_count() {
        let (db, engine, _dir) = setup().await;
        let a1 = insert(&db, "userA", "利用者は紅茶が好き", STATUS_KEPT).await;
        let _a2 = insert(&db, "userA", "今日は疲れた", STATUS_CANDIDATE).await;
        let _b1 = insert(&db, "userB", "B の記憶", STATUS_KEPT).await;

        let resp = app(&db, &engine, "userA")
            .oneshot(req("GET", &format!("/api/memories?botId={TEST_BOT_ID}")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_of(resp).await;
        assert_eq!(body["success"], true);
        assert_eq!(body["pendingCount"], 1);
        let ids: Vec<i64> = body["memories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![a1]);
        assert_eq!(body["memories"][0]["content"], "利用者は紅茶が好き");
        assert_eq!(body["memories"][0]["durability"], "medium");
        assert!(
            body["memories"][0].get("user_id").is_none(),
            "内部列は出さない"
        );
    }

    /// botId 無しは 403、アクセスできない Bot は 404。
    #[tokio::test]
    async fn list_requires_accessible_bot() {
        let (db, engine, _dir) = setup().await;
        let resp = app(&db, &engine, "userA")
            .oneshot(req("GET", "/api/memories"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let resp = app(&db, &engine, "stranger")
            .oneshot(req("GET", &format!("/api/memories?botId={TEST_BOT_ID}")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// 他人の記憶は消せない（同じ Bot を共有していても）。本人の kept は消え、索引からも除かれる。
    #[tokio::test]
    async fn delete_only_removes_own_kept() {
        let (db, engine, _dir) = setup().await;
        let a1 = insert(&db, "userA", "利用者は紅茶が好き", STATUS_KEPT).await;
        let a_pending = insert(&db, "userA", "今日は疲れた", STATUS_CANDIDATE).await;
        let b1 = insert(&db, "userB", "B の記憶", STATUS_KEPT).await;
        // a1 を索引にも載せておく（forget が効くことの確認用）。
        {
            let mut eng = engine.lock().await;
            let scope = yuuka_synapse::Scope {
                user_id: "userA".into(),
                bot_id: TEST_BOT_ID.into(),
                guild_id: None,
            };
            eng.index(
                scope,
                a1,
                None,
                "利用者は紅茶が好き".to_owned(),
                yuuka_synapse::FormationContext::default(),
            );
            assert_eq!(eng.total(), 1);
        }

        // B の行を A が消そうとしても 200 {success:false} で、行は残る。
        let resp = app(&db, &engine, "userA")
            .oneshot(req(
                "DELETE",
                &format!("/api/memories/{b1}?botId={TEST_BOT_ID}"),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(json_of(resp).await["success"], false);
        assert_eq!(count_rows(&db).await, 3);

        // 候補（評価待ち）は利用者からは消せない。
        let resp = app(&db, &engine, "userA")
            .oneshot(req(
                "DELETE",
                &format!("/api/memories/{a_pending}?botId={TEST_BOT_ID}"),
            ))
            .await
            .unwrap();
        assert_eq!(json_of(resp).await["success"], false);
        assert_eq!(count_rows(&db).await, 3);

        // 本人の kept は消え、索引からも消える。
        let resp = app(&db, &engine, "userA")
            .oneshot(req(
                "DELETE",
                &format!("/api/memories/{a1}?botId={TEST_BOT_ID}"),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(json_of(resp).await["success"], true);
        assert_eq!(count_rows(&db).await, 2);
        assert_eq!(engine.lock().await.total(), 0);

        // botId 無しは 403（消さない）。
        let resp = app(&db, &engine, "userB")
            .oneshot(req("DELETE", &format!("/api/memories/{b1}")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(count_rows(&db).await, 2);
    }
}
