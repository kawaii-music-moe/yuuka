#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{
    header::{COOKIE, LOCATION, SET_COOKIE},
    Request, StatusCode,
};
use axum::routing::post;
use axum::Json;
use serde_json::{json, Value};
use tower::ServiceExt;
use yuuka_auth::{AuthRuntime, CompositeAuth, SessionStore};
use yuuka_web::{framework_routes, AppState, Db, WebConfig};

struct TestApp {
    router: axum::Router,
    db_path: PathBuf,
    _file: tempfile::NamedTempFile,
}

fn app_with(sk_internal_url: &str, admin_ids: Vec<String>) -> TestApp {
    app_with_config(sk_internal_url, admin_ids, WebConfig::default())
}

fn app_with_config(sk_internal_url: &str, admin_ids: Vec<String>, config: WebConfig) -> TestApp {
    let file = tempfile::NamedTempFile::new().expect("temp db");
    let db = Db::open(file.path()).expect("open db");
    let sessions = SessionStore::in_memory();
    let auth = Arc::new(CompositeAuth::new(db.clone(), sessions.clone(), 7));
    let state = AppState::new(auth, config, db);
    let runtime = Arc::new(AuthRuntime::new(
        sessions,
        7,
        "https://account.sk-9.net".to_owned(),
        sk_internal_url.to_owned(),
        admin_ids,
    ));
    TestApp {
        router: framework_routes()
            .merge(yuuka_auth::routes(runtime))
            .with_state(state),
        db_path: file.path().to_owned(),
        _file: file,
    }
}

fn app() -> axum::Router {
    app_with("http://127.0.0.1:3100", Vec::new()).router
}

/// sk-account の交換エンドポイントを模したサーバー。`good-<n>` だけを受け付け、
/// `accounts[n]` のアカウントを返す。
async fn mock_sk_account(accounts: Vec<Value>) -> String {
    let accounts = Arc::new(accounts);
    let router = axum::Router::new().route(
        "/api/services/yuuka/exchange",
        post(move |Json(body): Json<Value>| {
            let accounts = accounts.clone();
            async move {
                let code = body.get("code").and_then(Value::as_str).unwrap_or_default();
                match code
                    .strip_prefix("good-")
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|n| accounts.get(n))
                {
                    Some(account) => (StatusCode::OK, Json(json!({ "account": account }))),
                    None => (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({ "error": "invalid or expired code" })),
                    ),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    format!("http://{addr}")
}

fn account(id: &str, discord: &str, name: &str) -> Value {
    json!({
        "id": id,
        "discordUserId": discord,
        "username": name,
        "displayName": name,
        "avatarUrl": null,
        "role": "user",
    })
}

async fn get(router: &axum::Router, uri: &str, cookie: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        request = request.header(COOKIE, cookie);
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

fn set_cookies(response: &axum::response::Response) -> Vec<String> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().expect("ascii").to_owned())
        .collect()
}

/// `start` を通して `(state, ブラウザが返す Cookie ヘッダー)` を得る。
async fn begin_login(router: &axum::Router) -> (String, String) {
    let response = get(router, "/api/auth/sk/start", None).await;
    let location = response
        .headers()
        .get(LOCATION)
        .expect("location")
        .to_str()
        .expect("ascii")
        .to_owned();
    let state = location
        .split_once("?state=")
        .expect("state in authorize url")
        .1
        .to_owned();
    let cookie = set_cookies(&response)
        .into_iter()
        .next()
        .expect("state cookie");
    let pair = cookie.split(';').next().expect("name=value").to_owned();
    (state, pair)
}

/// ログインを始めたブラウザが sk-account から戻ってくる callback。
async fn callback(router: &axum::Router, code: &str) -> axum::response::Response {
    let (state, cookie) = begin_login(router).await;
    get(
        router,
        &format!("/api/auth/sk/callback?code={code}&state={state}"),
        Some(&cookie),
    )
    .await
}

/// `(sk_account_id, username, role)` を返す。
fn user_row(path: &PathBuf, discord_id: &str) -> Option<(Option<String>, String, String)> {
    let conn = rusqlite::Connection::open(path).expect("open");
    conn.query_row(
        "SELECT sk_account_id, username, role FROM users WHERE discord_id = ?1",
        [discord_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .ok()
}

fn seed_user(path: &PathBuf, discord_id: &str, role: &str, sk_account_id: Option<&str>) {
    let conn = rusqlite::Connection::open(path).expect("open");
    conn.execute(
        "INSERT INTO users (discord_id, username, salt, role, sk_account_id) \
         VALUES (?1, 'old-name', '00ff', ?2, ?3)",
        rusqlite::params![discord_id, role, sk_account_id],
    )
    .expect("seed user");
}

#[tokio::test]
async fn sk_login_start_redirects_to_the_only_identity_provider() {
    let response = get(&app(), "/api/auth/sk/start", None).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response
        .headers()
        .get(LOCATION)
        .expect("location")
        .to_str()
        .expect("ascii");
    let state = location
        .strip_prefix("https://account.sk-9.net/api/services/yuuka/authorize?state=")
        .expect("authorize url with state");
    assert_eq!(state.len(), 43, "{state}");

    let cookies = set_cookies(&response);
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    let cookie = &cookies[0];
    assert!(
        cookie.starts_with(&format!("yuuka-sk-state={state};")),
        "{cookie}"
    );
    for attr in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=600"] {
        assert!(cookie.contains(attr), "{attr}: {cookie}");
    }
}

#[tokio::test]
async fn sk_login_start_uses_host_prefixed_secure_state_cookie_over_https() {
    let config = WebConfig {
        https: true,
        ..WebConfig::default()
    };
    let t = app_with_config("http://127.0.0.1:3100", Vec::new(), config);
    let response = get(&t.router, "/api/auth/sk/start", None).await;
    let cookie = set_cookies(&response).pop().expect("state cookie");
    assert!(cookie.starts_with("__Host-yuuka-sk-state="), "{cookie}");
    assert!(cookie.contains("; Secure"), "{cookie}");
    // __Host- は Path=/ かつ Domain 無しでないとブラウザに拒否される。
    assert!(cookie.contains("Path=/;"), "{cookie}");
    assert!(!cookie.contains("Domain="), "{cookie}");
}

fn has_session_cookie(response: &axum::response::Response) -> bool {
    set_cookies(response)
        .iter()
        .any(|c| c.starts_with("yuuka-session="))
}

fn clears_state_cookie(response: &axum::response::Response) -> bool {
    set_cookies(response)
        .iter()
        .any(|c| c.starts_with("yuuka-sk-state=;") && c.contains("Max-Age=0"))
}

#[tokio::test]
async fn callback_rejects_code_without_matching_state() {
    let sk = mock_sk_account(vec![account("acc-6", "600", "Mallory")]).await;
    let t = app_with(&sk, Vec::new());
    let (state, cookie) = begin_login(&t.router).await;
    let (other_state, _) = begin_login(&t.router).await;

    // 攻撃者が自分の code を載せた callback へ被害者を送り込む場合（被害者は state Cookie を
    // 持たない／別のログインの state を持つ）を含め、どれも code を交換しない。
    let cases = [
        (
            format!("/api/auth/sk/callback?code=good-0&state={state}"),
            None,
        ),
        (
            "/api/auth/sk/callback?code=good-0".to_owned(),
            Some(cookie.clone()),
        ),
        (
            format!("/api/auth/sk/callback?code=good-0&state={other_state}"),
            Some(cookie.clone()),
        ),
        (
            format!("/api/auth/sk/callback?code=good-0&state={}", &state[..42]),
            Some(cookie.clone()),
        ),
    ];
    for (uri, cookie) in cases {
        let response = get(&t.router, &uri, cookie.as_deref()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        assert!(!has_session_cookie(&response), "{uri}");
        assert!(clears_state_cookie(&response), "{uri}");
    }
    assert!(
        user_row(&t.db_path, "600").is_none(),
        "state が合わなければ code は交換されない"
    );
}

#[tokio::test]
async fn callback_with_matching_state_logs_in_and_clears_state_cookie() {
    let sk = mock_sk_account(vec![account("acc-7", "700", "Trent")]).await;
    let t = app_with(&sk, Vec::new());
    let response = callback(&t.router, "good-0").await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert!(has_session_cookie(&response));
    assert!(clears_state_cookie(&response));
}

#[tokio::test]
async fn password_login_and_registration_routes_do_not_exist() {
    for uri in [
        "/api/login",
        "/api/register",
        "/api/register/verify",
        "/api/setup",
        "/api/settings/password",
    ] {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn callback_creates_first_user_as_admin_and_issues_session() {
    let sk = mock_sk_account(vec![account("acc-1", "100", "Alice")]).await;
    let t = app_with(&sk, Vec::new());

    let response = callback(&t.router, "good-0").await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers().get(LOCATION).expect("location"), "/");
    let cookie = set_cookies(&response)
        .into_iter()
        .find(|c| c.starts_with("yuuka-session="))
        .expect("session cookie");
    assert!(cookie.contains("HttpOnly"), "{cookie}");

    let (sk_id, name, role) = user_row(&t.db_path, "100").expect("user created");
    assert_eq!(sk_id.as_deref(), Some("acc-1"));
    assert_eq!(name, "Alice");
    assert_eq!(role, "admin", "最初の利用者は admin");
}

#[tokio::test]
async fn callback_creates_later_users_as_user_unless_listed_admin() {
    let sk = mock_sk_account(vec![
        account("acc-2", "200", "Bob"),
        account("acc-3", "300", "Carol"),
    ])
    .await;
    let t = app_with(&sk, vec!["300".to_owned()]);
    seed_user(&t.db_path, "999", "admin", None);

    assert_eq!(
        callback(&t.router, "good-0").await.status(),
        StatusCode::FOUND
    );
    assert_eq!(
        callback(&t.router, "good-1").await.status(),
        StatusCode::FOUND
    );
    assert_eq!(user_row(&t.db_path, "200").expect("bob").2, "user");
    assert_eq!(user_row(&t.db_path, "300").expect("carol").2, "admin");
}

#[tokio::test]
async fn callback_claims_unlinked_user_row_with_same_discord_id() {
    let sk = mock_sk_account(vec![account("acc-4", "400", "Dave")]).await;
    let t = app_with(&sk, Vec::new());
    seed_user(&t.db_path, "400", "admin", None);

    assert_eq!(
        callback(&t.router, "good-0").await.status(),
        StatusCode::FOUND
    );
    let (sk_id, name, role) = user_row(&t.db_path, "400").expect("user");
    assert_eq!(sk_id.as_deref(), Some("acc-4"));
    assert_eq!(name, "Dave");
    assert_eq!(role, "admin", "既存のロールを保つ");

    // 二回目以降も同じ組み合わせならログインできる。
    assert_eq!(
        callback(&t.router, "good-0").await.status(),
        StatusCode::FOUND
    );
}

#[tokio::test]
async fn callback_rejects_discord_id_linked_to_another_sk_account() {
    let sk = mock_sk_account(vec![account("acc-intruder", "500", "Eve")]).await;
    let t = app_with(&sk, Vec::new());
    seed_user(&t.db_path, "500", "user", Some("acc-owner"));

    let response = callback(&t.router, "good-0").await;
    assert_ne!(response.status(), StatusCode::FOUND);
    assert!(!has_session_cookie(&response));
    assert_eq!(
        user_row(&t.db_path, "500").expect("user").0.as_deref(),
        Some("acc-owner")
    );
}

#[tokio::test]
async fn callback_rejects_invalid_or_missing_code() {
    let sk = mock_sk_account(Vec::new()).await;
    let t = app_with(&sk, Vec::new());

    let response = callback(&t.router, "bogus").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!has_session_cookie(&response));

    let (state, cookie) = begin_login(&t.router).await;
    let response = get(
        &t.router,
        &format!("/api/auth/sk/callback?state={state}"),
        Some(&cookie),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
