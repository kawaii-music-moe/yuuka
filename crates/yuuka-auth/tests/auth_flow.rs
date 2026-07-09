//! P1-1 認証発行の end-to-end 統合テスト。
//!
//! 実配線を検証する: `AuthRuntime` が発行したセッションを、**同一の in-memory `SessionStore`** を
//! 共有する `CompositeAuth` が `/api/me` で検証できること。setup（自動ログイン）→ /api/me、login →
//! /api/me、誤パスワード 401、未認証 401 を通す。
//!
//! テストバイナリ全体を test コンテキストとして扱う（`clippy.toml` の `allow-*-in-tests` は
//! `tests/` のヘルパ関数までは緩めないため、ファイル冒頭で明示的に許可する）。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;
use yuuka_auth::{AuthRuntime, CompositeAuth, NullRegistrationDm, SessionStore};
use yuuka_core::secrets::SecretString;
use yuuka_crypto::SystemCrypto;
use yuuka_web::{apply_common_layers, framework_routes, AppState, Db, WebConfig};

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// migrations 適用済みの一時 DB を開く（本番 open は CREATE しないので先にファイルを作る）。
fn fresh_db() -> Db {
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("yuuka_auth_it_{}_{n}.sqlite", std::process::id()));
    {
        rusqlite::Connection::open(&path).expect("seed file");
    }
    Db::open(&path).expect("open db")
}

/// framework（/api/me）+ 認証発行ルータを組み、in-memory セッションを発行↔検証で共有する app を作る。
fn build() -> Router {
    let db = fresh_db();
    let sessions = SessionStore::in_memory();
    let crypto = Arc::new(
        SystemCrypto::new(SecretString::from("integration-test-secret".to_owned())).expect("crypto"),
    );
    // /api/me の検証側（CompositeAuth）と発行側（AuthRuntime）が同じ sessions を共有するのが要。
    let auth = Arc::new(CompositeAuth::new(db.clone(), sessions.clone(), 7));
    let state = AppState::new(auth, WebConfig::default(), db);
    let runtime = Arc::new(AuthRuntime::new(
        sessions,
        7,
        Some(crypto),
        Arc::new(NullRegistrationDm),
        Vec::new(),
    ));
    apply_common_layers(framework_routes().merge(yuuka_auth::routes(runtime)), false).with_state(state)
}

/// ConnectInfo（peer アドレス）を載せた JSON リクエストを 1 発投げる。
async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Option<String>, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    let req = if let Some(b) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    // 本番同様に ConnectInfo<SocketAddr> を注入（レート制限のクライアント IP 解決経路を通す）。
    let mut req = req;
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 40000)));

    let resp = app.clone().oneshot(req).await.expect("response");
    let status = resp.status();
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, set_cookie, value)
}

/// `Set-Cookie` から `name=token` の先頭ペアだけを取り出して Cookie ヘッダ用に整形する。
fn cookie_pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap_or("").trim().to_owned()
}

const DISCORD_ID: &str = "123456789012345678";
const PASSWORD: &str = "Password1!";

fn setup_body() -> Value {
    json!({
        "discordId": DISCORD_ID,
        "username": "admin",
        "password": PASSWORD,
        "geminiApiKey": "test-gemini-key",
    })
}

#[tokio::test]
async fn setup_status_reports_need_setup_then_flips() {
    let app = build();
    let (status, _, body) = call(&app, "GET", "/api/setup/status", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["needSetup"], json!(true));

    let (status, _, _) = call(&app, "POST", "/api/setup", None, Some(setup_body())).await;
    assert_eq!(status, StatusCode::OK);

    let (_, _, body) = call(&app, "GET", "/api/setup/status", None, None).await;
    assert_eq!(body["needSetup"], json!(false), "セットアップ後は needSetup=false");
}

#[tokio::test]
async fn setup_issues_session_and_me_verifies_it() {
    let app = build();
    // setup → 200 + Set-Cookie（自動ログイン）。
    let (status, set_cookie, body) = call(&app, "POST", "/api/setup", None, Some(setup_body())).await;
    assert_eq!(status, StatusCode::OK, "setup 応答: {body}");
    let cookie = cookie_pair(&set_cookie.expect("setup が Set-Cookie を返す"));
    assert!(cookie.starts_with("yuuka-session="), "cookie = {cookie}");

    // 発行されたセッションで /api/me が 200（発行↔検証が同一ストアを共有している証拠）。
    let (status, _, body) = call(&app, "GET", "/api/me", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "me 応答: {body}");
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["user"]["discordId"], json!(DISCORD_ID));
    assert_eq!(body["user"]["username"], json!("admin"));
    // 最初のユーザーは admin。
    assert_eq!(body["user"]["role"], json!("admin"));
}

#[tokio::test]
async fn login_after_setup_then_me_ok_and_wrong_password_401() {
    let app = build();
    let (status, _, _) = call(&app, "POST", "/api/setup", None, Some(setup_body())).await;
    assert_eq!(status, StatusCode::OK);

    // 正しい資格情報 → 200 + Cookie。
    let login = json!({ "discordId": DISCORD_ID, "password": PASSWORD });
    let (status, set_cookie, body) = call(&app, "POST", "/api/login", None, Some(login)).await;
    assert_eq!(status, StatusCode::OK, "login 応答: {body}");
    let cookie = cookie_pair(&set_cookie.expect("login が Set-Cookie を返す"));
    let (status, _, _) = call(&app, "GET", "/api/me", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);

    // 誤ったパスワード → 401（Cookie を発行しない）。
    let bad = json!({ "discordId": DISCORD_ID, "password": "WrongPass9!" });
    let (status, set_cookie, body) = call(&app, "POST", "/api/login", None, Some(bad)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "誤パスワードは 401");
    assert_eq!(body["success"], json!(false));
    assert!(set_cookie.is_none(), "失敗時は Set-Cookie を返さない");
}

#[tokio::test]
async fn me_without_cookie_is_401() {
    let app = build();
    let (status, _, _) = call(&app, "GET", "/api/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_destroys_session() {
    let app = build();
    let (_, set_cookie, _) = call(&app, "POST", "/api/setup", None, Some(setup_body())).await;
    let cookie = cookie_pair(&set_cookie.expect("cookie"));

    // ログアウト前は /api/me 200。
    let (status, _, _) = call(&app, "GET", "/api/me", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);

    // ログアウト（同一 Cookie 認証・same-origin なので CSRF は通る）。
    let (status, _, _) = call(&app, "POST", "/api/logout", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);

    // 失効後は /api/me 401。
    let (status, _, _) = call(&app, "GET", "/api/me", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "ログアウト後は 401");
}

#[tokio::test]
async fn register_without_discord_returns_502_via_null_dm() {
    // 招待コードが無効なので、DM 到達前に 400（招待コード検証で弾かれる）。
    let app = build();
    let body = json!({
        "discordId": DISCORD_ID,
        "username": "u",
        "password": PASSWORD,
        "inviteCode": "NOPE",
        "geminiApiKey": "k",
    });
    let (status, _, body) = call(&app, "POST", "/api/register", None, Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "無効招待コードは 400: {body}");
}
