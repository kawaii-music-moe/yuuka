//! OIDC ログインの統合テスト（テスト用の OIDC プロバイダーを実際に立てて、Yuuka のルートを通す）。
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Form, State};
use axum::http::{
    header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE},
    HeaderMap, Request, StatusCode,
};
use axum::routing::{get, post};
use axum::Json;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use yuuka_auth::{AuthRuntime, CompositeAuth, OidcClient, SessionStore};
use yuuka_core::config::OidcSettings;
use yuuka_types::{Role, SessionUser};
use yuuka_web::{framework_routes, AppState, Db, WebConfig};

const CLIENT_ID: &str = "yuuka";
const CLIENT_SECRET: &str = "s3cret+/:";
const PUBLIC_BASE: &str = "https://yuuka.example.com";
const CALLBACK: &str = "https://yuuka.example.com/api/auth/oidc/callback";

// ---------------------------------------------------------------------------------------------
// テスト用の OIDC プロバイダー
// ---------------------------------------------------------------------------------------------

/// 認可エンドポイントで発行した code に結び付ける内容。
struct Grant {
    account: Value,
    nonce: String,
    challenge: String,
    redirect_uri: String,
}

#[derive(Default)]
struct ProviderState {
    grants: HashMap<String, Grant>,
    /// 有効なリフレッシュトークン → (アカウント, sid)。
    refresh_tokens: HashMap<String, (Value, String)>,
    revoked: HashSet<String>,
}

struct Provider {
    issuer: String,
    key_der: Vec<u8>,
    jwk: Value,
    state: Mutex<ProviderState>,
    token_calls: AtomicUsize,
    refresh_calls: AtomicUsize,
    serial: AtomicU64,
    /// アクセストークンの寿命（秒）。0 にすると毎回リフレッシュさせられる。
    expires_in: AtomicU64,
    /// true のあいだ token エンドポイントが 503 を返す（プロバイダー障害）。
    down: AtomicBool,
    /// true なら ID トークンに違う nonce を入れる。
    wrong_nonce: AtomicBool,
}

impl Provider {
    fn sign(&self, typ: Option<&str>, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("test-key".to_owned());
        header.typ = typ.map(str::to_owned);
        jsonwebtoken::encode(&header, claims, &EncodingKey::from_ec_der(&self.key_der))
            .expect("sign")
    }

    fn id_token(&self, account: &Value, sid: &str, nonce: Option<&str>) -> String {
        let now = unix_now();
        let mut claims = json!({
            "iss": self.issuer,
            "sub": account["sub"],
            "aud": CLIENT_ID,
            "exp": now + 300,
            "iat": now,
            "sid": sid,
            "name": account["name"],
        });
        if let Some(discord_id) = account.get("discord_id") {
            claims["discord_id"] = discord_id.clone();
        }
        if let Some(nonce) = nonce {
            claims["nonce"] = json!(nonce);
        }
        self.sign(Some("JWT"), &claims)
    }

    /// プロバイダーでのログイン後に戻ってくる code を発行する（認可エンドポイントの代わり）。
    fn authorize(&self, authorize_url: &str, account: Value) -> String {
        let url = reqwest::Url::parse(authorize_url).expect("authorize url");
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], CLIENT_ID);
        assert_eq!(q["code_challenge_method"], "S256");
        let code = format!("code-{}", self.serial.fetch_add(1, Ordering::SeqCst));
        self.state.lock().expect("lock").grants.insert(
            code.clone(),
            Grant {
                account,
                nonce: q["nonce"].clone(),
                challenge: q["code_challenge"].clone(),
                redirect_uri: q["redirect_uri"].clone(),
            },
        );
        code
    }

    fn logout_token(&self, typ: Option<&str>, sid: Option<&str>, sub: Option<&str>) -> String {
        let now = unix_now();
        let mut claims = json!({
            "iss": self.issuer,
            "aud": CLIENT_ID,
            "iat": now,
            "exp": now + 120,
            "jti": format!("jti-{}", self.serial.fetch_add(1, Ordering::SeqCst)),
            "events": { "http://schemas.openid.net/event/backchannel-logout": {} },
        });
        if let Some(sid) = sid {
            claims["sid"] = json!(sid);
        }
        if let Some(sub) = sub {
            claims["sub"] = json!(sub);
        }
        self.sign(typ, &claims)
    }

    /// プロバイダー側でログアウトした（リフレッシュトークンを失効させた）ことにする。
    fn end_all_grants(&self) {
        let mut state = self.state.lock().expect("lock");
        let tokens: Vec<String> = state.refresh_tokens.keys().cloned().collect();
        state.refresh_tokens.clear();
        state.revoked.extend(tokens);
    }
}

fn token_error(status: StatusCode, error: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": error })))
}

async fn token_endpoint(
    State(p): State<Arc<Provider>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    p.token_calls.fetch_add(1, Ordering::SeqCst);
    if p.down.load(Ordering::SeqCst) {
        return token_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable");
    }
    // client_secret_basic（ID とシークレットは form エンコードしてから Basic にする）。
    let expected = format!(
        "Basic {}",
        STANDARD.encode(format!("{CLIENT_ID}:{}", "s3cret%2B%2F%3A"))
    );
    if headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some(expected.as_str()) {
        return token_error(StatusCode::UNAUTHORIZED, "invalid_client");
    }
    let serial = p.serial.fetch_add(1, Ordering::SeqCst);
    let expires_in = p.expires_in.load(Ordering::SeqCst);
    let mut state = p.state.lock().expect("lock");
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let Some(grant) = form.get("code").and_then(|code| state.grants.remove(code)) else {
                return token_error(StatusCode::BAD_REQUEST, "invalid_grant");
            };
            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
            let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            if challenge != grant.challenge || form.get("redirect_uri") != Some(&grant.redirect_uri)
            {
                return token_error(StatusCode::BAD_REQUEST, "invalid_grant");
            }
            let sid = format!("sid-{serial}");
            let refresh = format!("rt-{serial}");
            state
                .refresh_tokens
                .insert(refresh.clone(), (grant.account.clone(), sid.clone()));
            let nonce = if p.wrong_nonce.load(Ordering::SeqCst) {
                "other-nonce".to_owned()
            } else {
                grant.nonce
            };
            let id_token = p.id_token(&grant.account, &sid, Some(&nonce));
            (
                StatusCode::OK,
                Json(json!({
                    "access_token": format!("at-{serial}"),
                    "token_type": "Bearer",
                    "expires_in": expires_in,
                    "refresh_token": refresh,
                    "id_token": id_token,
                })),
            )
        }
        Some("refresh_token") => {
            p.refresh_calls.fetch_add(1, Ordering::SeqCst);
            let Some((account, sid)) = form
                .get("refresh_token")
                .and_then(|token| state.refresh_tokens.remove(token))
            else {
                return token_error(StatusCode::BAD_REQUEST, "invalid_grant");
            };
            let refresh = format!("rt-{serial}");
            state
                .refresh_tokens
                .insert(refresh.clone(), (account.clone(), sid.clone()));
            let id_token = p.id_token(&account, &sid, None);
            (
                StatusCode::OK,
                Json(json!({
                    "access_token": format!("at-{serial}"),
                    "token_type": "Bearer",
                    "expires_in": expires_in,
                    "refresh_token": refresh,
                    "id_token": id_token,
                })),
            )
        }
        _ => token_error(StatusCode::BAD_REQUEST, "unsupported_grant_type"),
    }
}

async fn revoke_endpoint(
    State(p): State<Arc<Provider>>,
    Form(form): Form<HashMap<String, String>>,
) -> StatusCode {
    let mut state = p.state.lock().expect("lock");
    if let Some(token) = form.get("token") {
        state.refresh_tokens.remove(token);
        state.revoked.insert(token.clone());
    }
    StatusCode::OK
}

async fn start_provider() -> Arc<Provider> {
    let rng = SystemRandom::new();
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).expect("generate key");
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
        .expect("key pair");
    // 非圧縮の公開鍵: 0x04 || x(32) || y(32)
    let public = pair.public_key().as_ref();
    let jwk = json!({
        "kty": "EC",
        "crv": "P-256",
        "kid": "test-key",
        "alg": "ES256",
        "use": "sig",
        "x": URL_SAFE_NO_PAD.encode(&public[1..33]),
        "y": URL_SAFE_NO_PAD.encode(&public[33..65]),
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let issuer = format!("http://{}", listener.local_addr().expect("addr"));
    let provider = Arc::new(Provider {
        issuer: issuer.clone(),
        key_der: pkcs8.as_ref().to_vec(),
        jwk,
        state: Mutex::new(ProviderState::default()),
        token_calls: AtomicUsize::new(0),
        refresh_calls: AtomicUsize::new(0),
        serial: AtomicU64::new(0),
        expires_in: AtomicU64::new(900),
        down: AtomicBool::new(false),
        wrong_nonce: AtomicBool::new(false),
    });
    let discovery = json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "revocation_endpoint": format!("{issuer}/revoke"),
    });
    let router = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || async move { Json(discovery) }),
        )
        .route(
            "/jwks",
            get(|State(p): State<Arc<Provider>>| async move { Json(json!({ "keys": [p.jwk] })) }),
        )
        .route("/token", post(token_endpoint))
        .route("/revoke", post(revoke_endpoint))
        .with_state(provider.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    provider
}

fn account(sub: &str, discord: &str, name: &str) -> Value {
    json!({ "sub": sub, "discord_id": discord, "name": name })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

// ---------------------------------------------------------------------------------------------
// Yuuka 側
// ---------------------------------------------------------------------------------------------

struct TestApp {
    router: axum::Router,
    sessions: SessionStore,
    db_path: PathBuf,
    _file: tempfile::NamedTempFile,
}

fn oidc_client(issuer: &str) -> Arc<OidcClient> {
    let settings = OidcSettings {
        issuer: issuer.to_owned(),
        internal_url: None,
        client_id: CLIENT_ID.to_owned(),
        client_secret: Some(CLIENT_SECRET.to_owned().into()),
        scope: "openid profile discord".to_owned(),
        discord_id_claim: "discord_id".to_owned(),
        label: "Test Account".to_owned(),
    };
    Arc::new(OidcClient::from_settings(&settings, PUBLIC_BASE).expect("client"))
}

fn build_app(oidc: Option<Arc<OidcClient>>, admin_ids: Vec<String>, config: WebConfig) -> TestApp {
    let file = tempfile::NamedTempFile::new().expect("temp db");
    let db = Db::open(file.path()).expect("open db");
    let sessions = SessionStore::in_memory();
    let auth =
        Arc::new(CompositeAuth::new(db.clone(), sessions.clone(), 7).with_oidc(oidc.clone()));
    let state = AppState::new(auth, config, db);
    let runtime = Arc::new(AuthRuntime::new(sessions.clone(), 7, admin_ids, oidc));
    TestApp {
        router: framework_routes()
            .merge(yuuka_auth::routes(runtime))
            .with_state(state),
        sessions,
        db_path: file.path().to_owned(),
        _file: file,
    }
}

fn app_with(provider: &Provider, admin_ids: Vec<String>) -> TestApp {
    build_app(
        Some(oidc_client(&provider.issuer)),
        admin_ids,
        WebConfig::default(),
    )
}

async fn send(router: &axum::Router, request: Request<Body>) -> axum::response::Response {
    router.clone().oneshot(request).await.expect("response")
}

async fn get_with(
    router: &axum::Router,
    uri: &str,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        request = request.header(COOKIE, cookie);
    }
    send(router, request.body(Body::empty()).expect("request")).await
}

async fn post_form(router: &axum::Router, uri: &str, body: String) -> axum::response::Response {
    send(
        router,
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .expect("request"),
    )
    .await
}

fn set_cookies(response: &axum::response::Response) -> Vec<String> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().expect("ascii").to_owned())
        .collect()
}

fn location(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(LOCATION)
        .expect("location")
        .to_str()
        .expect("ascii")
        .to_owned()
}

fn name_value(cookie: &str) -> String {
    cookie.split(';').next().expect("name=value").to_owned()
}

/// `start` を通して `(認可 URL, state, ブラウザが返す state Cookie)` を得る。
async fn begin_login(router: &axum::Router) -> (String, String, String) {
    let response = get_with(router, "/api/auth/oidc/start", None).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let authorize_url = location(&response);
    let state = reqwest::Url::parse(&authorize_url)
        .expect("authorize URL")
        .query_pairs()
        .find(|(key, _)| key == "state")
        .expect("state in authorize URL")
        .1
        .into_owned();
    let cookie = name_value(&set_cookies(&response).pop().expect("state cookie"));
    (authorize_url, state, cookie)
}

/// プロバイダーで `account` としてログインし、Yuuka の callback まで戻ってくる。
async fn login(t: &TestApp, provider: &Provider, account: Value) -> axum::response::Response {
    let (authorize_url, state, cookie) = begin_login(&t.router).await;
    let code = provider.authorize(&authorize_url, account);
    get_with(
        &t.router,
        &format!("/api/auth/oidc/callback?code={code}&state={state}"),
        Some(&cookie),
    )
    .await
}

/// ログインして Yuuka のセッション Cookie（`name=value`）を得る。
async fn login_session(t: &TestApp, provider: &Provider, account: Value) -> String {
    let response = login(t, provider, account).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    name_value(
        &set_cookies(&response)
            .into_iter()
            .find(|c| c.starts_with("yuuka-session="))
            .expect("session cookie"),
    )
}

async fn logged_in(t: &TestApp, session: &str) -> StatusCode {
    get_with(&t.router, "/api/users", Some(session))
        .await
        .status()
}

fn has_session_cookie(response: &axum::response::Response) -> bool {
    set_cookies(response)
        .iter()
        .any(|c| c.starts_with("yuuka-session="))
}

fn clears_state_cookie(response: &axum::response::Response) -> bool {
    set_cookies(response)
        .iter()
        .any(|c| c.starts_with("yuuka-oidc-state=;") && c.contains("Max-Age=0"))
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

// ---------------------------------------------------------------------------------------------
// ログインの開始
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn start_redirects_to_the_provider_with_pkce_state_and_nonce() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    let response = get_with(&t.router, "/api/auth/oidc/start", None).await;
    assert_eq!(response.status(), StatusCode::FOUND);

    let url = reqwest::Url::parse(&location(&response)).expect("url");
    assert_eq!(
        format!("{}{}", url.origin().ascii_serialization(), url.path()),
        format!("{}/authorize", provider.issuer)
    );
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(
        q["redirect_uri"], CALLBACK,
        "戻り先は Host ヘッダーではなく設定から作る"
    );
    assert_eq!(q["scope"], "openid profile discord");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["code_challenge"].len(), 43);
    assert_eq!(q["state"].len(), 43);
    assert_eq!(q["nonce"].len(), 43);
    assert_ne!(q["state"], q["nonce"]);

    let cookies = set_cookies(&response);
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    let cookie = &cookies[0];
    assert!(
        cookie.starts_with(&format!("yuuka-oidc-state={};", q["state"])),
        "{cookie}"
    );
    for attr in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=600"] {
        assert!(cookie.contains(attr), "{attr}: {cookie}");
    }
}

#[tokio::test]
async fn start_uses_host_prefixed_secure_state_cookie_over_https() {
    let provider = start_provider().await;
    let config = WebConfig {
        https: true,
        ..WebConfig::default()
    };
    let t = build_app(Some(oidc_client(&provider.issuer)), Vec::new(), config);
    let response = get_with(&t.router, "/api/auth/oidc/start", None).await;
    let cookie = set_cookies(&response).pop().expect("state cookie");
    assert!(cookie.starts_with("__Host-yuuka-oidc-state="), "{cookie}");
    assert!(cookie.contains("; Secure"), "{cookie}");
    // __Host- は Path=/ かつ Domain 無しでないとブラウザに拒否される。
    assert!(cookie.contains("Path=/;"), "{cookie}");
    assert!(!cookie.contains("Domain="), "{cookie}");
}

#[tokio::test]
async fn login_is_unavailable_without_a_client_secret() {
    let t = build_app(None, Vec::new(), WebConfig::default());
    let response = get_with(&t.router, "/api/auth/oidc/start", None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let provider = get_with(&t.router, "/api/auth/oidc/provider", None).await;
    assert_eq!(provider.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(provider.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    assert_eq!(
        body,
        json!({ "success": true, "enabled": false, "label": null })
    );
}

#[tokio::test]
async fn provider_endpoint_reports_the_label() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    let response = get_with(&t.router, "/api/auth/oidc/provider", None).await;
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    assert_eq!(
        body,
        json!({ "success": true, "enabled": true, "label": "Test Account" })
    );
}

#[tokio::test]
async fn password_login_and_registration_routes_do_not_exist() {
    let t = build_app(None, Vec::new(), WebConfig::default());
    for uri in [
        "/api/login",
        "/api/register",
        "/api/register/verify",
        "/api/setup",
        "/api/settings/password",
        "/api/auth/sk/start",
        "/api/auth/sk/callback",
    ] {
        let response = send(
            &t.router,
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

// ---------------------------------------------------------------------------------------------
// callback
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn callback_rejects_code_without_matching_state() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    let (authorize_url, state, cookie) = begin_login(&t.router).await;
    let (_, other_state, _) = begin_login(&t.router).await;
    let code = provider.authorize(&authorize_url, account("acc-6", "600", "Mallory"));

    // 攻撃者が自分の code を載せた callback へ被害者を送り込む場合（被害者は state Cookie を
    // 持たない／別のログインの state を持つ）を含め、どれも code を交換しない。
    let cases = [
        (
            format!("/api/auth/oidc/callback?code={code}&state={state}"),
            None,
        ),
        (
            format!("/api/auth/oidc/callback?code={code}"),
            Some(cookie.clone()),
        ),
        (
            format!("/api/auth/oidc/callback?code={code}&state={other_state}"),
            Some(cookie.clone()),
        ),
        (
            format!("/api/auth/oidc/callback?code={code}&state={}", &state[..42]),
            Some(cookie.clone()),
        ),
    ];
    for (uri, cookie) in cases {
        let response = get_with(&t.router, &uri, cookie.as_deref()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        assert!(!has_session_cookie(&response), "{uri}");
        assert!(clears_state_cookie(&response), "{uri}");
    }
    assert_eq!(
        provider.token_calls.load(Ordering::SeqCst),
        0,
        "state が合わなければ code は交換しない"
    );
    assert!(user_row(&t.db_path, "600").is_none());
}

#[tokio::test]
async fn callback_creates_first_user_as_user_and_issues_session() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());

    let response = login(&t, &provider, account("acc-1", "100", "Alice")).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(location(&response), "/");
    assert!(clears_state_cookie(&response));
    let cookie = set_cookies(&response)
        .into_iter()
        .find(|c| c.starts_with("yuuka-session="))
        .expect("session cookie");
    assert!(cookie.contains("HttpOnly"), "{cookie}");

    let (sk_id, name, role) = user_row(&t.db_path, "100").expect("user created");
    assert_eq!(
        sk_id.as_deref(),
        Some("acc-1"),
        "sub を sk_account_id に入れる"
    );
    assert_eq!(name, "Alice");
    assert_eq!(role, "user", "未指定の最初の利用者も user");
    assert_eq!(logged_in(&t, &name_value(&cookie)).await, StatusCode::OK);
}

#[tokio::test]
async fn callback_requires_explicit_admin_even_for_first_login() {
    let provider = start_provider().await;
    // 管理者が後からログインする場合、先にログインした第三者を昇格させない。
    let t = app_with(&provider, vec!["300".to_owned()]);
    login_session(&t, &provider, account("acc-2", "200", "Bob")).await;
    login_session(&t, &provider, account("acc-3", "300", "Carol")).await;
    assert_eq!(user_row(&t.db_path, "200").expect("bob").2, "user");
    assert_eq!(user_row(&t.db_path, "300").expect("carol").2, "admin");

    // 明示された本人が最初にログインした場合は管理者を作れる。
    let fresh = app_with(&provider, vec!["100".to_owned()]);
    login_session(&fresh, &provider, account("acc-1", "100", "Alice")).await;
    assert_eq!(user_row(&fresh.db_path, "100").expect("alice").2, "admin");
}

#[tokio::test]
async fn state_is_single_use() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    let (authorize_url, state, cookie) = begin_login(&t.router).await;
    let code = provider.authorize(&authorize_url, account("acc-1", "100", "Alice"));
    let uri = format!("/api/auth/oidc/callback?code={code}&state={state}");
    assert_eq!(
        get_with(&t.router, &uri, Some(&cookie)).await.status(),
        StatusCode::FOUND
    );
    let replay = get_with(&t.router, &uri, Some(&cookie)).await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    assert!(!has_session_cookie(&replay));
}

#[tokio::test]
async fn callback_creates_later_users_as_user_unless_listed_admin() {
    let provider = start_provider().await;
    let t = app_with(&provider, vec!["300".to_owned()]);
    seed_user(&t.db_path, "999", "admin", None);

    login_session(&t, &provider, account("acc-2", "200", "Bob")).await;
    login_session(&t, &provider, account("acc-3", "300", "Carol")).await;
    assert_eq!(user_row(&t.db_path, "200").expect("bob").2, "user");
    assert_eq!(user_row(&t.db_path, "300").expect("carol").2, "admin");
}

#[tokio::test]
async fn callback_claims_unlinked_user_row_with_same_discord_id() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    seed_user(&t.db_path, "400", "admin", None);

    login_session(&t, &provider, account("acc-4", "400", "Dave")).await;
    let (sk_id, name, role) = user_row(&t.db_path, "400").expect("user");
    assert_eq!(sk_id.as_deref(), Some("acc-4"));
    assert_eq!(name, "Dave");
    assert_eq!(role, "admin", "既存のロールを保つ");

    // 二回目以降も同じ組み合わせならログインできる。
    login_session(&t, &provider, account("acc-4", "400", "Dave")).await;
}

#[tokio::test]
async fn callback_rejects_discord_id_linked_to_another_account() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    seed_user(&t.db_path, "500", "user", Some("acc-owner"));

    let response = login(&t, &provider, account("acc-intruder", "500", "Eve")).await;
    assert_ne!(response.status(), StatusCode::FOUND);
    assert!(!has_session_cookie(&response));
    assert_eq!(
        user_row(&t.db_path, "500").expect("user").0.as_deref(),
        Some("acc-owner")
    );
}

#[tokio::test]
async fn callback_rejects_invalid_or_missing_code() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());

    let (_, state, cookie) = begin_login(&t.router).await;
    let response = get_with(
        &t.router,
        &format!("/api/auth/oidc/callback?code=bogus&state={state}"),
        Some(&cookie),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!has_session_cookie(&response));

    let (_, state, cookie) = begin_login(&t.router).await;
    let response = get_with(
        &t.router,
        &format!("/api/auth/oidc/callback?state={state}&error=access_denied"),
        Some(&cookie),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!has_session_cookie(&response));
}

#[tokio::test]
async fn callback_rejects_id_token_with_another_nonce() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    provider.wrong_nonce.store(true, Ordering::SeqCst);
    let response = login(&t, &provider, account("acc-1", "100", "Alice")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!has_session_cookie(&response));
    assert!(user_row(&t.db_path, "100").is_none());
}

#[tokio::test]
async fn callback_rejects_account_without_discord_id() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    let response = login(
        &t,
        &provider,
        json!({ "sub": "acc-x", "name": "No Discord" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!has_session_cookie(&response));
}

#[tokio::test]
async fn callback_rejects_response_from_another_issuer() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    let (authorize_url, state, cookie) = begin_login(&t.router).await;
    let code = provider.authorize(&authorize_url, account("acc-1", "100", "Alice"));
    let response = get_with(
        &t.router,
        &format!(
            "/api/auth/oidc/callback?code={code}&state={state}&iss=https%3A%2F%2Fevil.example"
        ),
        Some(&cookie),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------------------------
// ログイン後の確認・ログアウト
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn expired_access_token_is_refreshed_once_and_session_stays() {
    let provider = start_provider().await;
    provider.expires_in.store(0, Ordering::SeqCst);
    let t = app_with(&provider, Vec::new());
    let session = login_session(&t, &provider, account("acc-1", "100", "Alice")).await;

    // 同時に来ても、リフレッシュは重ねずに 1 つずつ行う（重ねると古いトークンの再使用と見なされて失効する）。
    let (a, b) = tokio::join!(logged_in(&t, &session), logged_in(&t, &session));
    assert_eq!((a, b), (StatusCode::OK, StatusCode::OK));
    assert!(provider.refresh_calls.load(Ordering::SeqCst) >= 1);
    provider.expires_in.store(900, Ordering::SeqCst);
    assert_eq!(logged_in(&t, &session).await, StatusCode::OK);
    let calls = provider.refresh_calls.load(Ordering::SeqCst);
    assert_eq!(logged_in(&t, &session).await, StatusCode::OK);
    assert_eq!(
        provider.refresh_calls.load(Ordering::SeqCst),
        calls,
        "期限内ならプロバイダーに問い合わせない"
    );
}

#[tokio::test]
async fn session_ends_when_the_provider_has_logged_out() {
    let provider = start_provider().await;
    provider.expires_in.store(0, Ordering::SeqCst);
    let t = app_with(&provider, Vec::new());
    let session = login_session(&t, &provider, account("acc-1", "100", "Alice")).await;

    provider.end_all_grants();
    assert_eq!(logged_in(&t, &session).await, StatusCode::UNAUTHORIZED);
    // 一度終わったセッションは、プロバイダーが戻っても使えない。
    assert_eq!(logged_in(&t, &session).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn session_survives_while_the_provider_is_unreachable() {
    let provider = start_provider().await;
    provider.expires_in.store(0, Ordering::SeqCst);
    let t = app_with(&provider, Vec::new());
    let session = login_session(&t, &provider, account("acc-1", "100", "Alice")).await;

    provider.down.store(true, Ordering::SeqCst);
    assert_eq!(logged_in(&t, &session).await, StatusCode::OK);
    let calls = provider.token_calls.load(Ordering::SeqCst);
    assert_eq!(logged_in(&t, &session).await, StatusCode::OK);
    assert_eq!(
        provider.token_calls.load(Ordering::SeqCst),
        calls,
        "届かなかった直後は叩き続けない"
    );
}

#[tokio::test]
async fn backchannel_logout_ends_sessions_with_the_same_sid() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    let alice = login_session(&t, &provider, account("acc-1", "100", "Alice")).await;
    let bob = login_session(&t, &provider, account("acc-2", "200", "Bob")).await;
    // Alice のログインで発行した sid（テスト用プロバイダーは token 呼び出しの連番から作る）。
    let alice_sid = {
        let state = provider.state.lock().expect("lock");
        state
            .refresh_tokens
            .values()
            .find(|(account, _)| account["sub"] == "acc-1")
            .map(|(_, sid)| sid.clone())
            .expect("alice sid")
    };

    let token = provider.logout_token(Some("logout+jwt"), Some(&alice_sid), Some("acc-1"));
    let response = post_form(
        &t.router,
        "/api/auth/oidc/backchannel-logout",
        format!("logout_token={token}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(logged_in(&t, &alice).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        logged_in(&t, &bob).await,
        StatusCode::OK,
        "別の人のセッションは残る"
    );
}

#[tokio::test]
async fn backchannel_logout_without_sid_ends_all_sessions_of_the_subject() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    let first = login_session(&t, &provider, account("acc-1", "100", "Alice")).await;
    let second = login_session(&t, &provider, account("acc-1", "100", "Alice")).await;

    let token = provider.logout_token(Some("logout+jwt"), None, Some("acc-1"));
    let response = post_form(
        &t.router,
        "/api/auth/oidc/backchannel-logout",
        format!("logout_token={token}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(logged_in(&t, &first).await, StatusCode::UNAUTHORIZED);
    assert_eq!(logged_in(&t, &second).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn backchannel_logout_rejects_tokens_that_are_not_logout_tokens() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    let session = login_session(&t, &provider, account("acc-1", "100", "Alice")).await;

    let now = unix_now();
    let without_event = provider.sign(
        Some("logout+jwt"),
        &json!({ "iss": provider.issuer, "aud": CLIENT_ID, "iat": now, "exp": now + 120, "sub": "acc-1" }),
    );
    let with_nonce = provider.sign(
        Some("logout+jwt"),
        &json!({
            "iss": provider.issuer, "aud": CLIENT_ID, "iat": now, "exp": now + 120, "sub": "acc-1",
            "nonce": "n", "events": { "http://schemas.openid.net/event/backchannel-logout": {} },
        }),
    );
    let id_token_typed = provider.logout_token(Some("JWT"), None, Some("acc-1"));
    let id_token = provider.id_token(&account("acc-1", "100", "Alice"), "sid-x", Some("n"));
    for token in [
        without_event,
        with_nonce,
        id_token_typed,
        id_token,
        "not-a-jwt".to_owned(),
    ] {
        let response = post_form(
            &t.router,
            "/api/auth/oidc/backchannel-logout",
            format!("logout_token={token}"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{token}");
    }
    let missing = post_form(
        &t.router,
        "/api/auth/oidc/backchannel-logout",
        String::new(),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    assert_eq!(logged_in(&t, &session).await, StatusCode::OK);
}

#[tokio::test]
async fn logout_ends_the_session_and_revokes_the_refresh_token() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    let session = login_session(&t, &provider, account("acc-1", "100", "Alice")).await;

    let response = send(
        &t.router,
        Request::builder()
            .method("POST")
            .uri("/api/logout")
            .header(COOKIE, &session)
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(logged_in(&t, &session).await, StatusCode::UNAUTHORIZED);

    // 失効は応答を待たずに送るので、届くまで少し待つ。
    for _ in 0..50 {
        if !provider.state.lock().expect("lock").revoked.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("リフレッシュトークンが失効されていない");
}

#[tokio::test]
async fn sessions_issued_before_oidc_are_no_longer_accepted() {
    let provider = start_provider().await;
    let t = app_with(&provider, Vec::new());
    seed_user(&t.db_path, "100", "user", Some("acc-1"));
    // OIDC の結び付きを持たない、従来方式で発行されたセッション。
    let token = t
        .sessions
        .create(
            &SessionUser {
                discord_id: "100".to_owned(),
                username: "Alice".to_owned(),
                role: Role::User,
            },
            3600,
        )
        .await
        .expect("session");
    assert_eq!(
        logged_in(&t, &format!("yuuka-session={token}")).await,
        StatusCode::UNAUTHORIZED
    );
}
