//! デスクトップ端末（desktop トークン）管理 Web-API（Node `deviceMgmtRoutes`・auth:user）。
//!
//! `GET /api/devices`（接続端末一覧・トークン本体は返さず `current` のみ判定）と
//! `POST /api/devices/revoke`（端末単位の soft 失効）を提供する。認可は [`AuthenticatedUser`]
//! （Cookie / Bearer どちらでも本人）で型強制、DB は `State<Db>`。`current` 判定は Bearer 経路の
//! ときだけ可能（`Authorization: Bearer` を sha256 して `token_hash` と突き合わせる）。
//! レスポンス形は Node `sendJson` にバイト単位で合わせる。

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use yuuka_web::{AppState, AuthenticatedUser, Db};

use crate::{desktop, sha256_hex};

/// 端末管理ルータ（`AppState` 上でマージされる・supervisor の domain merge に載る）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/devices", get(list_devices))
        .route("/api/devices/revoke", post(revoke_device))
}

async fn list_devices(
    user: AuthenticatedUser,
    State(db): State<Db>,
    headers: HeaderMap,
) -> Response {
    let user_id = user.0.discord_id;
    // Bearer 認証時のみ「現在の端末」を判定できる（Cookie 経路では None）。
    let current_hash = bearer_token(&headers).map(|t| sha256_hex(&t));
    let tokens = match desktop::list_for_user(&db, &user_id).await {
        Ok(list) => list,
        Err(_) => return server_error(),
    };
    let devices: Vec<Value> = tokens
        .into_iter()
        .map(|t| {
            json!({
                "id": t.id,
                "device_name": t.device_name,
                "created_at": t.created_at,
                "last_used_at": t.last_used_at,
                "current": current_hash.as_deref() == Some(t.token_hash.as_str()),
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "success": true, "devices": devices })),
    )
        .into_response()
}

async fn revoke_device(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(body): Json<Value>,
) -> Response {
    let user_id = user.0.discord_id;
    let Some(id) = parse_device_id(body.get("id")) else {
        return status_json(StatusCode::BAD_REQUEST, "端末IDが不正です。");
    };
    match desktop::revoke(&db, id, &user_id).await {
        Ok(true) => {}
        Ok(false) => {
            return status_json(StatusCode::NOT_FOUND, "対象の端末が見つかりません。");
        }
        Err(_) => return server_error(),
    }
    // 監査は best-effort（内部でエラーを握る・本処理は落とさない・既存方針）。
    crate::audit::add_audit_log(
        &db,
        &user_id,
        "desktop.token_revoke",
        Some(&id.to_string()),
        None,
    )
    .await;
    (StatusCode::OK, Json(json!({ "success": true }))).into_response()
}

/// `Authorization: Bearer <token>` から生トークンを取り出す（Node `getBearerToken`）。
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let rest = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))?;
    let tok = rest.trim();
    (!tok.is_empty()).then(|| tok.to_owned())
}

/// 端末ID を解決する（Node `typeof id === "number" ? id : parseInt(String(id ?? ""), 10)` +
/// `Number.isInteger` チェックの簡約）。数値は整数のみ・文字列は先頭整数部を読む。
fn parse_device_id(v: Option<&Value>) -> Option<i64> {
    match v {
        // 数値は整数のときだけ有効（3.5 等は as_i64 が None＝Node isInteger false と一致）。
        Some(Value::Number(n)) => n.as_i64(),
        // 文字列は parseInt 相当（先頭の符号 + 数字列）。
        Some(Value::String(s)) => parse_leading_int(s),
        _ => None,
    }
}

/// `parseInt(s, 10)` の簡約（trim 後・先頭の符号 + 連続する 10 進数字・以降は無視）。
fn parse_leading_int(s: &str) -> Option<i64> {
    let mut buf = String::new();
    for (i, c) in s.trim().char_indices() {
        if (i == 0 && matches!(c, '+' | '-')) || c.is_ascii_digit() {
            buf.push(c);
        } else {
            break;
        }
    }
    // 符号のみ・空は無効。
    buf.parse::<i64>().ok()
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
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    use crate::sha256_hex;

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
        async fn desktop_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            // Bearer 経路: "tok-a" のみ本人。
            Ok((token == "tok-a").then(|| SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            }))
        }
    }

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_device_test_{}_{seq}.sqlite",
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
            for uid in ["u", "other"] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                     VALUES (?1, ?1, 'x', 'x')",
                    rusqlite::params![uid],
                )
                .expect("seed user");
            }
            // 端末2件（tok-a=本人の現在端末・tok-b=別端末）+ 他人の端末（不可視）。
            conn.execute(
                "INSERT INTO desktop_tokens (user_id, token_hash, device_name, created_at, last_used_at) \
                 VALUES ('u', ?1, 'MacBook', '2026-07-01 00:00:00', '2026-07-10 00:00:00')",
                rusqlite::params![sha256_hex("tok-a")],
            )
            .expect("seed tok-a");
            conn.execute(
                "INSERT INTO desktop_tokens (user_id, token_hash, device_name, created_at, last_used_at) \
                 VALUES ('u', ?1, 'iPhone', '2026-07-05 00:00:00', NULL)",
                rusqlite::params![sha256_hex("tok-b")],
            )
            .expect("seed tok-b");
            conn.execute(
                "INSERT INTO desktop_tokens (user_id, token_hash, device_name, created_at) \
                 VALUES ('other', ?1, 'Foreign', '2026-07-05 00:00:00')",
                rusqlite::params![sha256_hex("tok-x")],
            )
            .expect("seed tok-x");
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
        auth_header: (&str, &str),
        body: &str,
    ) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(auth_header.0, auth_header.1)
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
    async fn list_marks_current_only_on_bearer_and_hides_others() {
        let app = app();

        // Bearer tok-a: 自分の 2 端末が最近使用順（tok-a が current）。他人は不可視。
        let (st, j) = send(
            &app,
            "GET",
            "/api/devices",
            ("authorization", "Bearer tok-a"),
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let devices = j["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 2);
        // last_used_at DESC: tok-a(2026-07-10) が先頭・current=true。
        assert_eq!(devices[0]["device_name"], "MacBook");
        assert_eq!(devices[0]["current"], true);
        assert_eq!(devices[1]["device_name"], "iPhone");
        assert_eq!(devices[1]["current"], false);
        // トークン本体は返さない。
        assert!(devices[0].get("token_hash").is_none());

        // Cookie 経路（good）: current は常に false。
        let (st, j) = send(
            &app,
            "GET",
            "/api/devices",
            ("cookie", "__Host-yuuka-session=good"),
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(j["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["current"] == false));
    }

    #[tokio::test]
    async fn revoke_soft_deletes_then_gone_and_bad_id_400() {
        let app = app();

        // 不正 ID → 400。
        let (st, j) = send(
            &app,
            "POST",
            "/api/devices/revoke",
            ("cookie", "__Host-yuuka-session=good"),
            r#"{"id":"abc"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(j["message"], "端末IDが不正です。");

        // 端末 iPhone の id を取得して失効。
        let (_st, j) = send(
            &app,
            "GET",
            "/api/devices",
            ("cookie", "__Host-yuuka-session=good"),
            "",
        )
        .await;
        let devices = j["devices"].as_array().unwrap();
        let iphone_id = devices
            .iter()
            .find(|d| d["device_name"] == "iPhone")
            .unwrap()["id"]
            .as_i64()
            .unwrap();

        let (st, j) = send(
            &app,
            "POST",
            "/api/devices/revoke",
            ("cookie", "__Host-yuuka-session=good"),
            &format!(r#"{{"id":{iphone_id}}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);

        // 再失効は 404（既に revoked）。
        let (st, _j) = send(
            &app,
            "POST",
            "/api/devices/revoke",
            ("cookie", "__Host-yuuka-session=good"),
            &format!(r#"{{"id":{iphone_id}}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // 一覧から消える（残り 1 件）。
        let (_st, j) = send(
            &app,
            "GET",
            "/api/devices",
            ("cookie", "__Host-yuuka-session=good"),
            "",
        )
        .await;
        assert_eq!(j["devices"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn parse_device_id_matches_node_semantics() {
        use super::parse_device_id;
        use serde_json::json;
        assert_eq!(parse_device_id(Some(&json!(5))), Some(5));
        assert_eq!(parse_device_id(Some(&json!("7"))), Some(7));
        assert_eq!(parse_device_id(Some(&json!("12abc"))), Some(12)); // parseInt 相当。
        assert_eq!(parse_device_id(Some(&json!(3.5))), None); // 非整数。
        assert_eq!(parse_device_id(Some(&json!("abc"))), None);
        assert_eq!(parse_device_id(None), None);
    }
}
