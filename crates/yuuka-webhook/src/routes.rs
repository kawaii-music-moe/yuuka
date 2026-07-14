//! Webhook 受信・管理 Web-API（Node `webhookRoutes`）。
//!
//! 受信 `POST /hook/{token}`（auth:none・トークン単位レート制限・即時 200 応答後に非同期処理）と
//! 管理 `/api/webhooks/*`（auth:user）。シークレットは作成/更新時に [`SystemCrypto`] で暗号化して保存し、
//! 応答へは `has_secret` フラグのみ出す。受信の実処理（HMAC 検証・通知・todo/reminder 生成）は
//! [`WebhookProcessor`] シーム越しに委譲する（未配線時は既定 [`NullWebhookProcessor`]＝no-op）。

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use yuuka_crypto::SystemCrypto;
use yuuka_web::{AppState, AuthenticatedUser};

use crate::repo::{self, EncryptedSecret, EndpointCreate, EndpointPatch, WebhookEndpoint};

/// 受信処理シーム（Node `processIncomingWebhook`＝HMAC 検証・通知・todo/reminder 生成）。
#[async_trait]
pub trait WebhookProcessor: Send + Sync {
    /// 受信を非同期処理する（fire-and-forget・エラーは内部で握る）。
    async fn process(
        &self,
        endpoint: WebhookEndpoint,
        raw_body: Vec<u8>,
        signature: Option<String>,
    );
}

/// 受信処理未配線時の既定プロセッサ（no-op）。
pub struct NullWebhookProcessor;

#[async_trait]
impl WebhookProcessor for NullWebhookProcessor {
    async fn process(
        &self,
        _endpoint: WebhookEndpoint,
        _raw_body: Vec<u8>,
        _signature: Option<String>,
    ) {
    }
}

/// シークレット暗号化用 crypto（省略可・Extension で運ぶ newtype）。
#[derive(Clone)]
struct WebhookCrypto(Option<Arc<SystemCrypto>>);

const RATE_LIMIT_PER_MINUTE: u32 = 30;

/// トークン単位の受信レート制限バケット（毎分上限・Node `checkRateLimit`）。
static RATE_BUCKETS: LazyLock<Mutex<HashMap<String, (u32, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// レート制限を消費する（超過なら `false`）。
fn check_rate_limit(token: &str) -> bool {
    let now = Instant::now();
    let mut map = RATE_BUCKETS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // 肥大防止に期限切れを掃除。
    map.retain(|_, (_, reset)| *reset > now);
    match map.get_mut(token) {
        Some(bucket) if bucket.1 > now => {
            bucket.0 += 1;
            bucket.0 <= RATE_LIMIT_PER_MINUTE
        }
        _ => {
            map.insert(token.to_owned(), (1, now + Duration::from_secs(60)));
            true
        }
    }
}

/// Webhook ルータ（既定 [`NullWebhookProcessor`]・crypto なし）。
pub fn routes() -> Router<AppState> {
    routes_with(None, Arc::new(NullWebhookProcessor))
}

/// crypto/プロセッサを注入して Webhook ルータを組む。
pub fn routes_with(
    crypto: Option<Arc<SystemCrypto>>,
    processor: Arc<dyn WebhookProcessor>,
) -> Router<AppState> {
    Router::new()
        .route("/hook/{token}", post(receive))
        .route("/api/webhooks", get(list))
        .route("/api/webhooks/create", post(create))
        .route("/api/webhooks/update", post(update))
        .route("/api/webhooks/delete", post(delete))
        .route("/api/webhooks/deliveries", get(deliveries))
        .layer(Extension(WebhookCrypto(crypto)))
        .layer(Extension(processor))
}

// ─── POST /hook/{token}（受信・auth:none） ───────────────────────────────────

async fn receive(
    State(state): State<AppState>,
    Extension(processor): Extension<Arc<dyn WebhookProcessor>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !check_rate_limit(&token) {
        return status_json(StatusCode::TOO_MANY_REQUESTS, "rate limited");
    }
    let endpoint = match repo::get_endpoint_by_token(&state.db, &token).await {
        Ok(Some(e)) => e,
        Ok(None) => return status_json(StatusCode::NOT_FOUND, "not found"),
        Err(_) => return server_error(),
    };
    if !endpoint.enabled {
        return status_json(StatusCode::GONE, "endpoint disabled");
    }
    let signature = headers
        .get("x-hub-signature-256")
        .or_else(|| headers.get("x-signature-256"))
        .or_else(|| headers.get("x-hub-signature"))
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    // 即時 200（外部サービスを待たせない）→ 非同期で実処理（fire-and-forget）。
    tokio::spawn(async move {
        processor.process(endpoint, body.to_vec(), signature).await;
    });
    (
        StatusCode::OK,
        Json(json!({ "success": true, "message": "accepted" })),
    )
        .into_response()
}

// ─── GET /api/webhooks（一覧・auth:user） ────────────────────────────────────

async fn list(user: AuthenticatedUser, State(state): State<AppState>) -> Response {
    let endpoints = match repo::list_endpoints(&state.db, &user.0.discord_id).await {
        Ok(e) => e,
        Err(_) => return server_error(),
    };
    let base = state.config.base_url.as_deref();
    let views: Vec<Value> = endpoints.iter().map(|e| endpoint_view(e, base)).collect();
    (
        StatusCode::OK,
        Json(json!({ "success": true, "endpoints": views })),
    )
        .into_response()
}

// ─── POST /api/webhooks/create（作成・auth:user） ────────────────────────────

async fn create(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Extension(WebhookCrypto(crypto)): Extension<WebhookCrypto>,
    Json(body): Json<Value>,
) -> Response {
    let name = str_field(&body, "name");
    if name.is_empty() {
        return bad_request("name は必須です。");
    }
    // セキュリティ: 署名検証必須のためシークレット（16 文字以上）を必須化。
    let secret = str_field(&body, "secret");
    if secret.chars().count() < 16 {
        return bad_request(
            "Webhookシークレット（16文字以上）は必須です。受信は HMAC-SHA256 署名で検証されます。",
        );
    }
    let enc = match encrypt_secret(crypto.as_deref(), &secret) {
        Ok(e) => e,
        Err(_) => return server_error(),
    };

    let input = EndpointCreate {
        name,
        secret: Some(enc),
        notify_target_type: target_type(&body),
        notify_target_id: opt_trimmed(&body, "notifyTargetId"),
        template: string_or_null(&body, "template"),
        filter_keyword: string_or_null(&body, "filterKeyword"),
        create_todo: bool_field(&body, "createTodo"),
        create_reminder: bool_field(&body, "createReminder"),
    };
    let endpoint = match repo::create_endpoint(&state.db, &user.0.discord_id, input).await {
        Ok(e) => e,
        Err(_) => return server_error(),
    };
    let base = state.config.base_url.as_deref();
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "endpoint": endpoint_view(&endpoint, base),
            "message": format!("Webhookエンドポイント「{}」を作成しました。発行されたURLを外部サービスに登録してください。", endpoint.name),
        })),
    )
        .into_response()
}

// ─── POST /api/webhooks/update（更新・auth:user） ────────────────────────────

async fn update(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Extension(WebhookCrypto(crypto)): Extension<WebhookCrypto>,
    Json(body): Json<Value>,
) -> Response {
    let Some(id) = int_field(&body, "id") else {
        return bad_request("id は必須です。");
    };
    // シークレットを更新する場合は 16 文字以上（空へのクリアは許可しない）。
    let secret_present = body.get("secret").is_some();
    let secret_value = str_field(&body, "secret");
    if secret_present && secret_value.chars().count() < 16 {
        return bad_request(
            "Webhookシークレットは16文字以上が必要です（署名検証のため空にはできません）。",
        );
    }
    let secret_patch = if secret_present {
        match encrypt_secret(crypto.as_deref(), &secret_value) {
            Ok(e) => Some(Some(e)),
            Err(_) => return server_error(),
        }
    } else {
        None
    };

    let patch = EndpointPatch {
        name: present_string(&body, "name").filter(|s| !s.trim().is_empty()),
        secret: secret_patch,
        notify_target_type: body.get("notifyTargetType").map(|_| target_type(&body)),
        notify_target_id: body
            .get("notifyTargetId")
            .map(|_| opt_trimmed(&body, "notifyTargetId")),
        template: body
            .get("template")
            .map(|_| string_or_null(&body, "template")),
        filter_keyword: body
            .get("filterKeyword")
            .map(|_| string_or_null(&body, "filterKeyword")),
        create_todo: body
            .get("createTodo")
            .map(|_| bool_field(&body, "createTodo")),
        create_reminder: body
            .get("createReminder")
            .map(|_| bool_field(&body, "createReminder")),
        enabled: body.get("enabled").map(|_| bool_field(&body, "enabled")),
    };

    let ok = match repo::update_endpoint(&state.db, &user.0.discord_id, id, patch).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    let base = state.config.base_url.as_deref();
    let mut payload = serde_json::Map::new();
    payload.insert("success".to_owned(), json!(ok));
    if ok {
        if let Ok(Some(fresh)) = repo::get_endpoint(&state.db, &user.0.discord_id, id).await {
            payload.insert("endpoint".to_owned(), endpoint_view(&fresh, base));
        }
    }
    payload.insert(
        "message".to_owned(),
        json!(if ok {
            "Webhookエンドポイントを更新しました。"
        } else {
            "エンドポイントが見つかりません。"
        }),
    );
    (StatusCode::OK, Json(Value::Object(payload))).into_response()
}

// ─── POST /api/webhooks/delete（削除・auth:user） ────────────────────────────

async fn delete(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let Some(id) = int_field(&body, "id") else {
        return bad_request("id は必須です。");
    };
    let ok = match repo::delete_endpoint(&state.db, &user.0.discord_id, id).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    (
        StatusCode::OK,
        Json(json!({
            "success": ok,
            "message": if ok { "Webhookエンドポイントを削除しました。" } else { "エンドポイントが見つかりません。" },
        })),
    )
        .into_response()
}

// ─── GET /api/webhooks/deliveries（受信履歴・auth:user） ─────────────────────

#[derive(Debug, Deserialize)]
struct DeliveriesQuery {
    #[serde(default, rename = "endpointId")]
    endpoint_id: Option<String>,
}

async fn deliveries(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Query(q): Query<DeliveriesQuery>,
) -> Response {
    // Node: `endpointId ? Number(endpointId) : undefined`・整数のみ有効。
    let endpoint_id = q.endpoint_id.as_deref().and_then(|s| s.parse::<i64>().ok());
    let list = match repo::list_deliveries(&state.db, &user.0.discord_id, endpoint_id, 50).await {
        Ok(v) => v,
        Err(_) => return server_error(),
    };
    let views: Vec<Value> = list
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "endpoint_id": d.endpoint_id,
                "user_id": d.user_id,
                "payload": d.payload,
                "status": d.status,
                "detail": d.detail,
                "created_at": d.created_at,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "success": true, "deliveries": views })),
    )
        .into_response()
}

// ─── ヘルパ ──────────────────────────────────────────────────────────────────

/// 安全ビュー（Node `toEndpointView` + `url`）。暗号文シークレットは出さない。
fn endpoint_view(e: &WebhookEndpoint, base_url: Option<&str>) -> Value {
    json!({
        "id": e.id,
        "name": e.name,
        "token": e.token,
        "has_secret": e.has_secret(),
        "notify_target_type": e.notify_target_type,
        "notify_target_id": e.notify_target_id,
        "template": e.template,
        "filter_keyword": e.filter_keyword,
        "create_todo": e.create_todo,
        "create_reminder": e.create_reminder,
        "enabled": e.enabled,
        "created_at": e.created_at,
        "url": build_hook_url(base_url, &e.token),
    })
}

/// 受信 URL（Node `buildHookUrl`）。base_url 設定時は絶対・未設定は相対。
fn build_hook_url(base_url: Option<&str>, token: &str) -> String {
    match base_url.map(str::trim).filter(|s| !s.is_empty()) {
        Some(base) => format!("{}/hook/{token}", base.trim_end_matches('/')),
        None => format!("/hook/{token}"),
    }
}

/// 平文シークレットを暗号化（crypto 未注入は `Err`）。
fn encrypt_secret(
    crypto: Option<&SystemCrypto>,
    secret: &str,
) -> Result<EncryptedSecret, yuuka_crypto::CryptoError> {
    let crypto = crypto.ok_or(yuuka_crypto::CryptoError::SecretMissing)?;
    let enc = crypto.encrypt_text(secret.trim())?;
    Ok(EncryptedSecret {
        encrypted: enc.encrypted,
        iv: enc.iv,
        tag: enc.auth_tag,
    })
}

/// `notifyTargetType === "channel" ? channel : dm`（Node）。
fn target_type(body: &Value) -> String {
    if body.get("notifyTargetType").and_then(Value::as_str) == Some("channel") {
        "channel".to_owned()
    } else {
        "dm".to_owned()
    }
}

fn str_field(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned()
}

fn present_string(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// `typeof x === "string" && x.trim() ? x.trim() : null`。
fn opt_trimmed(body: &Value, key: &str) -> Option<String> {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// `typeof x === "string" ? x.trim() || null : null`（template/filterKeyword）。
fn string_or_null(body: &Value, key: &str) -> Option<String> {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn bool_field(body: &Value, key: &str) -> bool {
    body.get(key).and_then(Value::as_bool) == Some(true)
}

/// `Number(x)` が整数か（数値の整数・整数文字列のみ・Node `Number.isInteger`）。
fn int_field(body: &Value, key: &str) -> Option<i64> {
    match body.get(key) {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
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

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use secrecy::SecretString;
    use serde_json::Value;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_crypto::SystemCrypto;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct FakeAuth;

    #[async_trait::async_trait]
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

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_webhook_test_{}_{seq}.sqlite",
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
            conn.execute(
                "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                 VALUES ('u', 'u', 'x', 'x')",
                [],
            )
            .expect("seed user");
        }
        db
    }

    fn app() -> axum::Router {
        let crypto = Arc::new(SystemCrypto::new(SecretString::from("test-secret-xyz")).unwrap());
        let config = WebConfig {
            base_url: Some("https://x.test/".to_owned()),
            ..WebConfig::default()
        };
        let state = AppState::new(Arc::new(FakeAuth), config, seed_db());
        super::routes_with(Some(crypto), Arc::new(super::NullWebhookProcessor)).with_state(state)
    }

    async fn send(
        app: &axum::Router,
        method: &str,
        uri: &str,
        cookie: bool,
        body: &str,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if cookie {
            req = req.header("cookie", "__Host-yuuka-session=good");
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::from(body.to_owned())).unwrap())
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
    async fn management_crud_flow() {
        let app = app();

        // name 必須。
        let (st, _) = send(&app, "POST", "/api/webhooks/create", true, "{}").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // secret <16 は拒否。
        let (st, _) = send(
            &app,
            "POST",
            "/api/webhooks/create",
            true,
            r#"{"name":"CI","secret":"short"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // 作成。
        let (st, j) = send(
            &app,
            "POST",
            "/api/webhooks/create",
            true,
            r#"{"name":"CI","secret":"abcdef0123456789xyz","createTodo":true}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let ep = &j["endpoint"];
        assert_eq!(ep["name"], "CI");
        assert_eq!(ep["has_secret"], true);
        assert_eq!(ep["create_todo"], true);
        assert_eq!(ep["enabled"], true);
        assert!(ep.get("secret_encrypted").is_none()); // 暗号文は出さない。
        let token = ep["token"].as_str().unwrap().to_owned();
        assert_eq!(ep["url"], format!("https://x.test/hook/{token}"));
        let id = ep["id"].as_i64().unwrap();

        // 一覧に出る。
        let (_st, j) = send(&app, "GET", "/api/webhooks", true, "").await;
        assert_eq!(j["endpoints"].as_array().unwrap().len(), 1);

        // 更新: id 必須。
        let (st, _) = send(&app, "POST", "/api/webhooks/update", true, "{}").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // secret 更新は 16 文字以上必須。
        let (st, _) = send(
            &app,
            "POST",
            "/api/webhooks/update",
            true,
            &format!(r#"{{"id":{id},"secret":"x"}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // enabled=false + 改名。
        let (st, j) = send(
            &app,
            "POST",
            "/api/webhooks/update",
            true,
            &format!(r#"{{"id":{id},"name":"CI2","enabled":false}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);
        assert_eq!(j["endpoint"]["name"], "CI2");
        assert_eq!(j["endpoint"]["enabled"], false);

        // deliveries は空。
        let (st, j) = send(&app, "GET", "/api/webhooks/deliveries", true, "").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["deliveries"].as_array().unwrap().len(), 0);

        // 削除。
        let (st, j) = send(
            &app,
            "POST",
            "/api/webhooks/delete",
            true,
            &format!(r#"{{"id":{id}}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);
        // 二重削除は success:false。
        let (_st, j) = send(
            &app,
            "POST",
            "/api/webhooks/delete",
            true,
            &format!(r#"{{"id":{id}}}"#),
        )
        .await;
        assert_eq!(j["success"], false);
    }

    #[tokio::test]
    async fn receive_status_codes() {
        let app = app();
        // 未知トークン → 404。
        let (st, _) = send(&app, "POST", "/hook/nonexistent-token", false, "{}").await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // エンドポイント作成 → token 取得。
        let (_st, j) = send(
            &app,
            "POST",
            "/api/webhooks/create",
            true,
            r#"{"name":"H","secret":"abcdefghij0123456789"}"#,
        )
        .await;
        let token = j["endpoint"]["token"].as_str().unwrap().to_owned();
        let id = j["endpoint"]["id"].as_i64().unwrap();

        // enabled → 200 accepted。
        let (st, j) = send(
            &app,
            "POST",
            &format!("/hook/{token}"),
            false,
            r#"{"event":"x"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["message"], "accepted");

        // 無効化 → 410。
        send(
            &app,
            "POST",
            "/api/webhooks/update",
            true,
            &format!(r#"{{"id":{id},"enabled":false}}"#),
        )
        .await;
        let (st, _) = send(&app, "POST", &format!("/hook/{token}"), false, "{}").await;
        assert_eq!(st, StatusCode::GONE);
    }
}
