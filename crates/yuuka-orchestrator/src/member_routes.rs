//! ギルド利用メンバーの利用申請 Web-API（Node `memberRequestRoutes`・全て auth:user）。
//!
//! 申請作成（申請者）/ 自分の申請状況 / オーナー向け承認一覧 / 承認・却下 の 4 本。業務ロジックは
//! [`crate::bot_repo`] の `submit_member_request`/`decide_member_request`（Discord ボタンフローと共通の
//! DB 確定層）を再利用し、HTTP status は結果の `code`（[`SubmitDeny`]/[`DecideDeny`]）で分岐する。
//!
//! **DM のシーム**: 申請受付/決定の通知は [`MemberDmSender`] ポート越しに fire-and-forget で送る。
//! Discord メッセンジャ未配線のうちは既定 [`NullMemberDmSender`]（no-op）へ縮退する（DB 上の申請は
//! Web 管理から拾えるため DM 失敗は許容・Node も同方針）。実配線時は [`routes_with`] で実 sender を注入。

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use yuuka_discord::{MemberDecision, MemberDmSender};
use yuuka_web::{AppState, AuthenticatedUser, Db};

use crate::bot_repo::{self, DecideDeny, MemberRequestRow, SubmitDeny};

/// Discord 未配線時の既定 DM sender（no-op・常に未送信）。
pub struct NullMemberDmSender;

#[async_trait]
impl MemberDmSender for NullMemberDmSender {
    async fn send_request_dm(
        &self,
        _owner_id: &str,
        _bot_name: &str,
        _applicant_label: &str,
        _guild_label: &str,
        _note: Option<&str>,
        _request_id: i64,
    ) -> bool {
        false
    }
    async fn send_decision_dm(
        &self,
        _applicant_id: &str,
        _bot_name: &str,
        _approved: bool,
    ) -> bool {
        false
    }
}

/// 利用申請ルータ（既定 [`NullMemberDmSender`]・`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    routes_with(Arc::new(NullMemberDmSender))
}

/// 実 DM sender を注入して利用申請ルータを組む（Discord 配線時に使う）。
pub fn routes_with(dm: Arc<dyn MemberDmSender>) -> Router<AppState> {
    Router::new()
        .route(
            "/api/bots/member-requests",
            post(submit).get(list_for_owner),
        )
        .route("/api/bots/member-requests/mine", get(list_mine))
        .route("/api/bots/member-requests/{id}/decide", post(decide))
        .layer(Extension(dm))
}

#[derive(Debug, Deserialize)]
struct OwnerListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

// ─── POST /api/bots/member-requests（申請作成） ──────────────────────────────

async fn submit(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(dm): Extension<Arc<dyn MemberDmSender>>,
    Json(body): Json<Value>,
) -> Response {
    let applicant = user.0.discord_id;
    let bot_id = body.get("botId").and_then(Value::as_str).unwrap_or("");
    let guild_id = body
        .get("guildId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let note = body.get("note").and_then(Value::as_str).map(str::to_owned);
    if bot_id.is_empty() || !is_snowflake(guild_id) {
        return status_json(
            StatusCode::BAD_REQUEST,
            "botId とギルドID（数字）が必要です。",
        );
    }

    let result = match bot_repo::submit_member_request(
        &db,
        bot_id,
        guild_id,
        &applicant,
        note.clone(),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => return server_error(),
    };
    if !result.ok {
        let status = match result.code {
            Some(SubmitDeny::BotNotFound) => StatusCode::NOT_FOUND,
            _ => StatusCode::CONFLICT,
        };
        return status_json(status, &result.message);
    }

    yuuka_auth::audit::add_audit_log(
        &db,
        &applicant,
        "bot.member_request_submit",
        Some(&format!("{bot_id}:{guild_id}")),
        None,
    )
    .await;

    // オーナーへ受付 DM（fire-and-forget・失敗しても申請は Web 管理から拾える）。
    if let (Some(owner_id), Some(bot_name), Some(request_id)) =
        (result.owner_id, result.bot_name, result.request_id)
    {
        let _ = dm
            .send_request_dm(
                &owner_id,
                &bot_name,
                &format!("ユーザー {applicant}"),
                &format!("ギルド {guild_id}"),
                note.as_deref(),
                request_id,
            )
            .await;
    }
    ok_message("利用申請を送信しました。Bot作成者の承認をお待ちください。")
}

// ─── GET /api/bots/member-requests/mine（自分の申請） ────────────────────────

async fn list_mine(user: AuthenticatedUser, State(db): State<Db>) -> Response {
    let uid = user.0.discord_id;
    let requests = match bot_repo::list_member_requests_by_user(&db, &uid).await {
        Ok(r) => r,
        Err(_) => return server_error(),
    };
    // bot_name = 名前 or bot_id フォールバック（Node `getBotById(...)?.name ?? r.bot_id`）。
    let mut out = Vec::with_capacity(requests.len());
    for r in requests {
        let bot_name = match bot_repo::get_bot(&db, &r.bot_id).await {
            Ok(Some(b)) => b.name,
            Ok(None) => r.bot_id.clone(),
            Err(_) => return server_error(),
        };
        out.push(member_request_json(&r, &bot_name));
    }
    (
        StatusCode::OK,
        Json(json!({ "success": true, "requests": out })),
    )
        .into_response()
}

// ─── GET /api/bots/member-requests（オーナー向け一覧） ───────────────────────

async fn list_for_owner(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<OwnerListQuery>,
) -> Response {
    let uid = user.0.discord_id;
    let status = match q.status.as_deref() {
        Some(s @ ("pending" | "approved" | "rejected")) => Some(s.to_owned()),
        _ => None,
    };

    // 閲覧できるのは所有 Bot のみ（Admin は botId 指定で任意 Bot を対象にできる）。
    let mut bots = match bot_repo::list_bots_owned_by(&db, &uid).await {
        Ok(b) => b,
        Err(_) => return server_error(),
    };
    if let Some(filter) = q.bot_id.as_deref().filter(|s| !s.is_empty()) {
        let is_admin = match bot_repo::is_admin(&db, &uid).await {
            Ok(v) => v,
            Err(_) => return server_error(),
        };
        if is_admin {
            bots = match bot_repo::get_bot(&db, filter).await {
                Ok(Some(b)) => vec![(b.id, b.name)],
                Ok(None) => Vec::new(),
                Err(_) => return server_error(),
            };
        } else {
            bots.retain(|(id, _)| id == filter);
        }
    }

    let mut out: Vec<Value> = Vec::new();
    for (bot_id, bot_name) in bots {
        let reqs =
            match bot_repo::list_member_requests_for_bot(&db, &bot_id, status.as_deref()).await {
                Ok(r) => r,
                Err(_) => return server_error(),
            };
        for r in reqs {
            out.push(member_request_json(&r, &bot_name));
        }
    }
    // 複数 Bot をまたぐため created_at 降順に整列（Node の localeCompare 降順）。
    out.sort_by(|a, b| {
        b["created_at"]
            .as_str()
            .unwrap_or("")
            .cmp(a["created_at"].as_str().unwrap_or(""))
    });
    (
        StatusCode::OK,
        Json(json!({ "success": true, "requests": out })),
    )
        .into_response()
}

// ─── POST /api/bots/member-requests/{id}/decide（承認/却下） ─────────────────

async fn decide(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(dm): Extension<Arc<dyn MemberDmSender>>,
    Path(id_str): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let actor = user.0.discord_id;
    let Some(id) = parse_int(&id_str) else {
        return status_json(StatusCode::BAD_REQUEST, "申請IDが不正です。");
    };
    let decision = match (
        body.get("decision").and_then(Value::as_str),
        body.get("action").and_then(Value::as_str),
    ) {
        (Some("approved"), _) | (_, Some("approve")) => MemberDecision::Approved,
        (Some("rejected"), _) | (_, Some("reject")) => MemberDecision::Rejected,
        _ => {
            return status_json(
                StatusCode::BAD_REQUEST,
                "decision（approved/rejected）が必要です。",
            )
        }
    };

    // 監査 target 用に決定前の申請内容を控える（Node は existing を先に引く）。
    let existing = match bot_repo::get_member_request(&db, id).await {
        Ok(e) => e,
        Err(_) => return server_error(),
    };
    let result = match bot_repo::decide_member_request(&db, id, decision, &actor).await {
        Ok(r) => r,
        Err(_) => return server_error(),
    };
    if !result.ok {
        let status = match result.code {
            Some(DecideDeny::NotFound) => StatusCode::NOT_FOUND,
            Some(DecideDeny::Forbidden) => StatusCode::FORBIDDEN,
            _ => StatusCode::CONFLICT,
        };
        return status_json(status, &result.message);
    }

    let approved = decision == MemberDecision::Approved;
    let action = if approved {
        "bot.member_request_approve"
    } else {
        "bot.member_request_reject"
    };
    let target = existing.as_ref().map_or_else(
        || id.to_string(),
        |e| format!("{}:{}:{}", e.bot_id, e.guild_id, e.user_id),
    );
    yuuka_auth::audit::add_audit_log(&db, &actor, action, Some(&target), None).await;

    // 申請者へ結果 DM（fire-and-forget）。
    if let (Some(applicant_id), Some(bot_name)) = (result.applicant_id, result.bot_name) {
        let _ = dm
            .send_decision_dm(&applicant_id, &bot_name, approved)
            .await;
    }
    ok_message(if approved {
        "利用申請を承認しました。"
    } else {
        "利用申請を却下しました。"
    })
}

// ─── ヘルパ ──────────────────────────────────────────────────────────────────

/// `^\d{5,25}$`（Node `isSnowflake`）。
fn is_snowflake(value: &str) -> bool {
    let len = value.len();
    (5..=25).contains(&len) && value.bytes().all(|b| b.is_ascii_digit())
}

/// `parseInt(s, 10)` の簡約（trim 後・先頭符号 + 連続 10 進数字）。
fn parse_int(s: &str) -> Option<i64> {
    let mut buf = String::new();
    for (i, c) in s.trim().char_indices() {
        if (i == 0 && matches!(c, '+' | '-')) || c.is_ascii_digit() {
            buf.push(c);
        } else {
            break;
        }
    }
    buf.parse::<i64>().ok()
}

/// 申請行 + bot_name を JSON へ（Node `{ ...r, bot_name }`・全列 verbatim）。
fn member_request_json(r: &MemberRequestRow, bot_name: &str) -> Value {
    json!({
        "id": r.id,
        "bot_id": r.bot_id,
        "guild_id": r.guild_id,
        "user_id": r.user_id,
        "status": r.status,
        "note": r.note,
        "decided_by": r.decided_by,
        "created_at": r.created_at,
        "updated_at": r.updated_at,
        "bot_name": bot_name,
    })
}

fn ok_message(message: &str) -> Response {
    (
        StatusCode::OK,
        Json(json!({ "success": true, "message": message })),
    )
        .into_response()
}

fn status_json(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
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

    // トークン == discord_id。admin だけ role=admin。
    struct FakeAuth;

    #[async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            let known = ["u", "owner", "admin", "other"];
            Ok(known.contains(&token).then(|| SessionUser {
                discord_id: token.to_owned(),
                username: token.to_owned(),
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
            "yuuka_member_test_{}_{seq}.sqlite",
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
                ("u", "user"),
                ("owner", "user"),
                ("admin", "admin"),
                ("other", "user"),
            ] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt, role) \
                     VALUES (?1, ?1, 'x', 'x', ?2)",
                    rusqlite::params![uid, role],
                )
                .expect("seed user");
            }
            conn.execute(
                "INSERT INTO bots (id, user_id, name, created_at) \
                 VALUES ('b1', 'owner', 'TestBot', '2026-07-01 00:00:00')",
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
    async fn submit_validation_and_dedup() {
        let app = app();
        // missing botId。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/member-requests",
            "u",
            r#"{"guildId":"123456"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // bad guildId（<5桁）。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/member-requests",
            "u",
            r#"{"botId":"b1","guildId":"12"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // bot 不在 → 404。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/member-requests",
            "u",
            r#"{"botId":"nope","guildId":"123456"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        // オーナー自身 → 409（is_owner）。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/member-requests",
            "owner",
            r#"{"botId":"b1","guildId":"123456"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);

        // 正常申請。
        let (st, j) = send(
            &app,
            "POST",
            "/api/bots/member-requests",
            "u",
            r#"{"botId":"b1","guildId":"123456","note":" hi "}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);
        // 二重 pending → 409。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/member-requests",
            "u",
            r#"{"botId":"b1","guildId":"123456"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn mine_owner_list_and_decide_flow() {
        let app = app();
        // u が申請。
        send(
            &app,
            "POST",
            "/api/bots/member-requests",
            "u",
            r#"{"botId":"b1","guildId":"123456","note":"よろしく"}"#,
        )
        .await;

        // mine: bot_name 付き・pending。
        let (st, j) = send(&app, "GET", "/api/bots/member-requests/mine", "u", "").await;
        assert_eq!(st, StatusCode::OK);
        let mine = j["requests"].as_array().unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0]["bot_name"], "TestBot");
        assert_eq!(mine[0]["status"], "pending");
        assert_eq!(mine[0]["note"], "よろしく");
        let req_id = mine[0]["id"].as_i64().unwrap();

        // owner 一覧に出る。approved フィルタでは 0 件。
        let (_st, j) = send(&app, "GET", "/api/bots/member-requests", "owner", "").await;
        assert_eq!(j["requests"].as_array().unwrap().len(), 1);
        let (_st, j) = send(
            &app,
            "GET",
            "/api/bots/member-requests?status=approved",
            "owner",
            "",
        )
        .await;
        assert_eq!(j["requests"].as_array().unwrap().len(), 0);

        // decide: 不正 id → 400。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/member-requests/abc/decide",
            "owner",
            "{}",
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // decision 欠落 → 400。
        let (st, _) = send(
            &app,
            "POST",
            &format!("/api/bots/member-requests/{req_id}/decide"),
            "owner",
            "{}",
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // 非オーナー → 403。
        let (st, _) = send(
            &app,
            "POST",
            &format!("/api/bots/member-requests/{req_id}/decide"),
            "other",
            r#"{"decision":"approved"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // 存在しない id → 404。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/member-requests/9999/decide",
            "owner",
            r#"{"decision":"approved"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // owner 承認 → 200。
        let (st, j) = send(
            &app,
            "POST",
            &format!("/api/bots/member-requests/{req_id}/decide"),
            "owner",
            r#"{"decision":"approved"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["message"], "利用申請を承認しました。");
        // 再承認 → 409（already_decided）。
        let (st, _) = send(
            &app,
            "POST",
            &format!("/api/bots/member-requests/{req_id}/decide"),
            "owner",
            r#"{"decision":"approved"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);

        // mine は approved に。
        let (_st, j) = send(&app, "GET", "/api/bots/member-requests/mine", "u", "").await;
        assert_eq!(j["requests"][0]["status"], "approved");

        // bot_members に u が追加された（承認済みユーザーは再申請すると already_member=409）。
        let (st, _) = send(
            &app,
            "POST",
            "/api/bots/member-requests",
            "u",
            r#"{"botId":"b1","guildId":"123456"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn admin_can_target_any_bot_in_owner_list() {
        let app = app();
        send(
            &app,
            "POST",
            "/api/bots/member-requests",
            "u",
            r#"{"botId":"b1","guildId":"123456"}"#,
        )
        .await;
        // admin は自分の所有 bot が無くても botId 指定で任意 bot の申請を見られる。
        let (st, j) = send(
            &app,
            "GET",
            "/api/bots/member-requests?botId=b1",
            "admin",
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["requests"].as_array().unwrap().len(), 1);
        // 非 admin の other が botId 指定しても所有外なので 0 件。
        let (_st, j) = send(
            &app,
            "GET",
            "/api/bots/member-requests?botId=b1",
            "other",
            "",
        )
        .await;
        assert_eq!(j["requests"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn snowflake_and_parse_int() {
        use super::{is_snowflake, parse_int};
        assert!(is_snowflake("123456"));
        assert!(!is_snowflake("12"));
        assert!(!is_snowflake("12a456"));
        assert_eq!(parse_int("42"), Some(42));
        assert_eq!(parse_int("abc"), None);
    }
}
