//! Bot 共有 Web-API（Node `botRoutes` の共有 3 本・全て auth:user）。
//!
//! 共有設定の閲覧 / 招待作成 / 取消。作成者（Admin は取消のみ）スコープを型で強制せず handler で判定する
//! （Node と同分岐）。招待 DM は [`ShareInviteDm`] ポート越しに送る（Discord 未配線時は既定
//! [`NullShareInviteDm`]＝未送信・DB 上の招待は承認待ち一覧から拾える）。
//! bot CRUD / sync-discord / profile（同 `botRoutes`）は Discord ランタイム health に依存するため別増分。

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use yuuka_web::{AppState, AuthenticatedUser, Db};

use crate::bot_repo::{self, BotShareRow};

/// 共有招待 DM 送信ポート（Node `sendShareInviteDM`）。Discord 未配線時は [`NullShareInviteDm`]。
#[async_trait]
pub trait ShareInviteDm: Send + Sync {
    /// 招待先へ承認/辞退ボタン付き DM を送る（成功なら `true`）。
    async fn send_invite(
        &self,
        share_id: i64,
        target_user_id: &str,
        bot_name: &str,
        owner_name: &str,
        persona_name: Option<&str>,
    ) -> bool;
}

/// Discord 未配線時の既定 DM sender（no-op・常に未送信）。
pub struct NullShareInviteDm;

#[async_trait]
impl ShareInviteDm for NullShareInviteDm {
    async fn send_invite(
        &self,
        _share_id: i64,
        _target_user_id: &str,
        _bot_name: &str,
        _owner_name: &str,
        _persona_name: Option<&str>,
    ) -> bool {
        false
    }
}

/// 共有ルータ（既定 [`NullShareInviteDm`]・`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    routes_with(Arc::new(NullShareInviteDm))
}

/// 実 DM sender を注入して共有ルータを組む（Discord 配線時に使う）。
pub fn routes_with(dm: Arc<dyn ShareInviteDm>) -> Router<AppState> {
    Router::new()
        .route("/api/bots/shares", get(list_shares))
        .route("/api/bots/shares/invite", post(invite))
        .route("/api/bots/shares/revoke", post(revoke))
        .layer(Extension(dm))
}

#[derive(Debug, Deserialize)]
struct BotIdQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

// ─── GET /api/bots/shares（共有設定の閲覧・作成者のみ） ──────────────────────

async fn list_shares(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotIdQuery>,
) -> Response {
    let uid = user.0.discord_id;
    let Some(bot_id) = q.bot_id.filter(|s| !s.is_empty()) else {
        return bad_request("botId が必要です。");
    };
    let bot = match bot_repo::get_bot(&db, &bot_id).await {
        Ok(b) => b,
        Err(_) => return server_error(),
    };
    // 作成者のみ閲覧可（Node は owner 不一致/不在を 403）。
    let Some(bot) = bot.filter(|b| b.owner_id == uid) else {
        return status_json(
            StatusCode::FORBIDDEN,
            "Botの作成者のみが共有設定を閲覧できます。",
        );
    };
    let shares = match bot_repo::list_shares_for_bot(&db, &bot_id).await {
        Ok(s) => s,
        Err(_) => return server_error(),
    };
    let mut out = Vec::with_capacity(shares.len());
    for s in shares {
        let shared_username = match bot_repo::get_username(&db, &s.shared_user_id).await {
            Ok(u) => u,
            Err(_) => return server_error(),
        };
        out.push(share_json(&s, shared_username.as_deref()));
    }
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "shares": out,
            "recommended_persona_id": bot.recommended_persona_id,
        })),
    )
        .into_response()
}

// ─── POST /api/bots/shares/invite（招待作成・作成者のみ） ────────────────────

async fn invite(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(dm): Extension<Arc<dyn ShareInviteDm>>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot_id = str_field(&body, "botId");
    let target = str_field(&body, "targetUserId");
    if bot_id.is_empty() || target.is_empty() {
        return bad_request("botId と targetUserId が必要です。");
    }
    let bot = match bot_repo::get_bot(&db, &bot_id).await {
        Ok(b) => b,
        Err(_) => return server_error(),
    };
    let Some(bot) = bot.filter(|b| b.owner_id == uid) else {
        return status_json(
            StatusCode::FORBIDDEN,
            "Botの作成者のみが共有招待を作成できます。",
        );
    };
    if target == uid {
        return bad_request("自分自身を招待することはできません。");
    }
    // 対象ユーザーが登録済みか（username で存在確認）。
    let target_username = match bot_repo::get_username(&db, &target).await {
        Ok(u) => u,
        Err(_) => return server_error(),
    };
    let Some(target_username) = target_username else {
        return status_json(
            StatusCode::NOT_FOUND,
            "対象ユーザーが登録されていません。先にユーザー登録が必要です。",
        );
    };

    let share = match bot_repo::create_share_invite(&db, &bot_id, &uid, &target).await {
        Ok(s) => s,
        Err(_) => return server_error(),
    };

    // 推奨ペルソナ（公開）名を DM に添える。
    let persona_name = match bot.recommended_persona_id {
        Some(pid) => match bot_repo::get_public_persona(&db, pid).await {
            Ok(p) => p.map(|p| p.name),
            Err(_) => None,
        },
        None => None,
    };
    let owner_name = match bot_repo::get_username(&db, &uid).await {
        Ok(Some(n)) => n,
        _ => uid.clone(),
    };
    let dm_sent = dm
        .send_invite(
            share.id,
            &target,
            &bot.name,
            &owner_name,
            persona_name.as_deref(),
        )
        .await;

    let message = if dm_sent {
        format!(
            "{target_username} さんへ招待DMを送信しました。承認されるとアクセスが有効になります。"
        )
    } else {
        "招待を作成しましたが、DM送信に失敗しました（Bot未起動またはDM拒否設定の可能性があります）。"
            .to_owned()
    };
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "share": share_json(&share, None),
            "message": message,
        })),
    )
        .into_response()
}

// ─── POST /api/bots/shares/revoke（取消・作成者 or Admin） ───────────────────

async fn revoke(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<Value>,
) -> Response {
    let uid = user.0.discord_id;
    let bot_id = str_field(&body, "botId");
    let target = str_field(&body, "targetUserId");
    if bot_id.is_empty() || target.is_empty() {
        return bad_request("botId と targetUserId が必要です。");
    }
    let bot = match bot_repo::get_bot(&db, &bot_id).await {
        Ok(b) => b,
        Err(_) => return server_error(),
    };
    // 作成者 or Admin のみ取消可（Node §5.3.2）。
    let is_owner = bot.as_ref().is_some_and(|b| b.owner_id == uid);
    let is_admin = match bot_repo::is_admin(&db, &uid).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    if bot.is_none() || (!is_owner && !is_admin) {
        return status_json(
            StatusCode::FORBIDDEN,
            "Botの作成者のみが共有を取り消せます。",
        );
    }

    let ok = match bot_repo::revoke_share(&db, &bot_id, &target).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    // Admin が他人の Bot を取り消したら監査（Node `admin.share_revoke`）。
    if ok && !is_owner {
        yuuka_auth::audit::add_audit_log(
            &db,
            &uid,
            "admin.share_revoke",
            Some(&format!("{bot_id}:{target}")),
            None,
        )
        .await;
    }
    (
        StatusCode::OK,
        Json(json!({
            "success": ok,
            "message": if ok { "共有アクセスを取り消しました。" } else { "共有設定が見つかりません。" },
        })),
    )
        .into_response()
}

// ─── ヘルパ ──────────────────────────────────────────────────────────────────

fn str_field(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned()
}

/// 共有行 + shared_username を JSON へ（Node `{ ...s, shared_username }`・invite は username 無し）。
fn share_json(s: &BotShareRow, shared_username: Option<&str>) -> Value {
    json!({
        "id": s.id,
        "bot_id": s.bot_id,
        "owner_id": s.owner_id,
        "shared_user_id": s.shared_user_id,
        "status": s.status,
        "created_at": s.created_at,
        "updated_at": s.updated_at,
        "shared_username": shared_username,
    })
}

fn status_json(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

fn bad_request(message: &str) -> Response {
    status_json(StatusCode::BAD_REQUEST, message)
}

fn server_error() -> Response {
    status_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        "内部エラーが発生しました。",
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct FakeAuth;

    #[async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            let known = ["owner", "target", "admin", "other"];
            Ok(known.contains(&token).then(|| SessionUser {
                discord_id: token.to_owned(),
                username: format!("{token}_name"),
                role: if token == "admin" {
                    Role::Admin
                } else {
                    Role::User
                },
            }))
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_share_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).expect("create empty");
            drop(conn);
        }
        let db = Db::open(&path).expect("open");
        {
            let conn = rusqlite::Connection::open(&path).expect("seed conn");
            for (uid, role) in [
                ("owner", "user"),
                ("target", "user"),
                ("admin", "admin"),
                ("other", "user"),
            ] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt, role) \
                     VALUES (?1, ?1 || '_name', 'x', 'x', ?2)",
                    rusqlite::params![uid, role],
                )
                .expect("seed user");
            }
            conn.execute(
                "INSERT INTO bots (id, user_id, name) VALUES ('b1', 'owner', 'TestBot')",
                [],
            )
            .expect("seed bot");
        }
        db
    }

    fn app() -> axum::Router {
        let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), seed_db());
        super::routes().with_state(state)
    }

    async fn send(
        app: &axum::Router,
        method: &str,
        uri: &str,
        token: &str,
        body: &str,
    ) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("cookie", format!("__Host-yuuka-session={token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, json)
    }

    #[tokio::test]
    async fn invite_list_revoke_flow() {
        let app = app();

        // 非作成者は共有閲覧不可（403）。
        let (st, _) = send(&app, "GET", "/api/bots/shares?botId=b1", "other", "").await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // 未登録ユーザー招待 → 404。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/shares/invite",
            "owner",
            r#"{"botId":"b1","targetUserId":"ghost"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        // 自分自身 → 400。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/shares/invite",
            "owner",
            r#"{"botId":"b1","targetUserId":"owner"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // 非作成者が招待 → 403。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/shares/invite",
            "other",
            r#"{"botId":"b1","targetUserId":"target"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // 正常招待（Null DM のため message は失敗系だが success:true）。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/shares/invite",
            "owner",
            r#"{"botId":"b1","targetUserId":"target"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);
        assert_eq!(j["share"]["status"], "pending");
        assert!(j["message"].as_str().unwrap().contains("DM送信に失敗"));

        // 一覧に出る（shared_username 付き）。
        let (st, j) = send(&app, "GET", "/api/bots/shares?botId=b1", "owner", "").await;
        assert_eq!(st, StatusCode::OK);
        let shares = j["shares"].as_array().unwrap();
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0]["shared_user_id"], "target");
        assert_eq!(shares[0]["shared_username"], "target_name");

        // 取消: 非作成者・非 admin → 403。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/shares/revoke",
            "other",
            r#"{"botId":"b1","targetUserId":"target"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // Admin が取消 → 200 success:true。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/shares/revoke",
            "admin",
            r#"{"botId":"b1","targetUserId":"target"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);
        assert_eq!(j["message"], "共有アクセスを取り消しました。");
        // 取消後も一覧には revoked 行として残る（status=revoked）。
        let (_st, j) = send(&app, "GET", "/api/bots/shares?botId=b1", "owner", "").await;
        assert_eq!(j["shares"][0]["status"], "revoked");
        // 共有行が存在しない相手の取消は success:false +「見つかりません」（WHERE 不一致・Node parity）。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/shares/revoke",
            "owner",
            r#"{"botId":"b1","targetUserId":"other"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], false);
        assert_eq!(j["message"], "共有設定が見つかりません。");
    }

    #[tokio::test]
    async fn validation_missing_fields() {
        let app = app();
        let (st, _) = send(&app, "GET", "/api/bots/shares", "owner", "").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/shares/invite",
            "owner",
            r#"{"botId":"b1"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = send(&app, "POST", "/api/bots/shares/revoke", "owner", "{}").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}
