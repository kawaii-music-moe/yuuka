//! `/ws/chat` の live end-to-end 統合テスト（実 WebSocket クライアント → 実サーバ）。
//!
//! 実配線を検証する: 実バインドしたサーバへ Bearer 認証つきで WS 接続 → `ready` 受信 → `msg` 送信 →
//! `ChatEngine`（fake Gemini backend）が処理して `done` を返す。ネットワーク（Gemini）不要。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use secrecy::SecretString;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use yuuka_core::{AuthError, GeminiError};
use yuuka_crypto::SystemCrypto;
use yuuka_gemini::{
    Content, FunctionDeclaration, GenerateBackend, GenerateContentResponse, ToolConfig,
};
use yuuka_orchestrator::{ChatEngine, GeminiFactory};
use yuuka_supervisor::{build_app, ws_routes};
use yuuka_tools::ToolRegistry;
use yuuka_types::{Role, SessionUser};
use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Bearer トークン "good" で固定ユーザーを返す fake 認証。
struct FakeAuth;
#[async_trait]
impl AuthBackend for FakeAuth {
    async fn session_user(&self, _t: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok(None)
    }
    async fn desktop_user(&self, t: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok((t == "good").then(|| SessionUser {
            discord_id: "u1".to_owned(),
            username: "yuu".to_owned(),
            role: Role::User,
        }))
    }
}

/// Cookie セッション "sess-ok" **と** Bearer "good" の両方を解決する fake（CSWSH 回帰用）。
/// これで「Cookie は正当なのに WS では拒否される」＝Bearer 専用化を検証できる。
struct CookieAndBearerAuth;
#[async_trait]
impl AuthBackend for CookieAndBearerAuth {
    async fn session_user(&self, t: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok((t == "sess-ok").then(|| SessionUser {
            discord_id: "u1".to_owned(),
            username: "yuu".to_owned(),
            role: Role::User,
        }))
    }
    async fn desktop_user(&self, t: &str) -> Result<Option<SessionUser>, AuthError> {
        Ok((t == "good").then(|| SessionUser {
            discord_id: "u1".to_owned(),
            username: "yuu".to_owned(),
            role: Role::User,
        }))
    }
}

/// 固定テキストを返す fake Gemini backend。
struct FakeBackend {
    text: String,
}
#[async_trait]
impl GenerateBackend for FakeBackend {
    async fn generate(
        &self,
        _s: Option<&str>,
        _d: &[FunctionDeclaration],
        _c: &[Content],
        _tc: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        Ok(serde_json::from_value(json!({
            "candidates": [ { "content": { "role": "model", "parts": [ { "text": self.text } ] } } ]
        }))
        .unwrap())
    }
}

struct FakeFactory {
    text: String,
}
impl GeminiFactory for FakeFactory {
    fn build(&self, _m: &str, _k: SecretString) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        Ok(Arc::new(FakeBackend {
            text: self.text.clone(),
        }))
    }
}

/// users 行に暗号化 Gemini キーを seed して DB を返す。
fn seeded_db() -> (Db, Arc<SystemCrypto>) {
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("yuuka_ws_it_{}_{n}.sqlite", std::process::id()));
    {
        rusqlite::Connection::open(&path).unwrap();
    }
    let db = Db::open(&path).unwrap();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("ws-test-secret".to_owned())).unwrap());
    let enc = crypto.encrypt_text("fake-key").unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO users (discord_id, username, password_hash, salt, role, \
         gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag) \
         VALUES ('u1', 'yuu', 'x', '00', 'user', ?1, ?2, ?3)",
        rusqlite::params![enc.encrypted, enc.iv, enc.auth_tag],
    )
    .unwrap();
    (db, crypto)
}

/// 実サーバを ephemeral ポートで起動し、Bearer 認証つき WS クライアントで接続する。
#[tokio::test]
async fn ws_chat_ready_then_msg_returns_done() {
    let (db, crypto) = seeded_db();
    let engine = Arc::new(ChatEngine::new(
        db.clone(),
        Some(crypto),
        ToolRegistry::new(),
        Arc::new(FakeFactory {
            text: "こんにちは！".to_owned(),
        }),
    ));
    let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), db);
    let app = build_app(
        state,
        axum::Router::new(),
        axum::Router::new(),
        axum::Router::new(),
        axum::Router::new(),
        axum::Router::new(),
        // credential ルータ（本テストでは不要・空ルータ）。
        axum::Router::new(),
        // デバイスフロー ルータ（本テストでは不要・空ルータ）。
        axum::Router::new(),
        ws_routes(engine, 20),
        None,
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // Bearer 認証つきで /ws/chat?botId=system_default へ接続。
    let mut req = format!("ws://{addr}/ws/chat?botId=system_default")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", "Bearer good".parse().unwrap());
    let (mut ws, _resp) = tokio_tungstenite::connect_async(req)
        .await
        .expect("connect");

    // 1) 最初のフレームは ready（束縛 Bot・ユーザー・上限）。
    let ready = next_json(&mut ws).await;
    assert_eq!(ready["type"], "ready", "ready: {ready}");
    assert_eq!(ready["user"]["discordId"], "u1");
    assert_eq!(ready["bot"]["id"], "system_default");
    assert_eq!(ready["maxUploadMb"], 20);

    // 2) msg 送信 → status/done を受け取る（done まで読む）。
    ws.send(WsMessage::Text(r#"{"type":"msg","text":"やあ"}"#.into()))
        .await
        .unwrap();

    let done = read_until(&mut ws, "done").await;
    assert_eq!(done["text"], "こんにちは！");
    assert!(done["messageId"].is_string());
    assert_eq!(done["deferred"], false);

    ws.close(None).await.ok();
    server.abort();
}

/// **B5（CSWSH 回帰）**: 正当な Cookie セッションだけを持ち Bearer を持たない WS upgrade は
/// **拒否**されねばならない（`/ws/chat` は Bearer 専用）。これが 101 で通ると、悪意サイトが
/// ambient Cookie を載せて Cross-Site WebSocket Hijacking を成立させられる。
#[tokio::test]
async fn ws_chat_rejects_cookie_only_auth() {
    let (db, crypto) = seeded_db();
    let engine = Arc::new(ChatEngine::new(
        db.clone(),
        Some(crypto),
        ToolRegistry::new(),
        Arc::new(FakeFactory {
            text: "x".to_owned(),
        }),
    ));
    let state = AppState::new(Arc::new(CookieAndBearerAuth), WebConfig::default(), db);
    let app = build_app(
        state,
        axum::Router::new(),
        axum::Router::new(),
        axum::Router::new(),
        axum::Router::new(),
        axum::Router::new(),
        // credential ルータ（本テストでは不要・空ルータ）。
        axum::Router::new(),
        // デバイスフロー ルータ（本テストでは不要・空ルータ）。
        axum::Router::new(),
        ws_routes(engine, 20),
        None,
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // 正当な Cookie セッションのみ（Authorization ヘッダ無し）で接続を試みる。
    let mut req = format!("ws://{addr}/ws/chat?botId=system_default")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("cookie", "__Host-yuuka-session=sess-ok".parse().unwrap());
    let result = tokio_tungstenite::connect_async(req).await;
    assert!(
        result.is_err(),
        "Cookie 認証での WS upgrade は 401 で拒否されねばならない（CSWSH 対策 B5）"
    );

    // 対照: 同一サーバに Bearer で接続すると 101 で通る（Bearer 専用が機能している証左）。
    let mut ok_req = format!("ws://{addr}/ws/chat?botId=system_default")
        .into_client_request()
        .unwrap();
    ok_req
        .headers_mut()
        .insert("authorization", "Bearer good".parse().unwrap());
    let (mut ws, _resp) = tokio_tungstenite::connect_async(ok_req)
        .await
        .expect("Bearer 認証の WS upgrade は成功する");
    let ready = next_json(&mut ws).await;
    assert_eq!(ready["type"], "ready");
    ws.close(None).await.ok();

    server.abort();
}

/// 次のテキストフレームを JSON として読む。
async fn next_json(ws: &mut WsStream) -> Value {
    loop {
        match ws.next().await.expect("stream").expect("frame") {
            WsMessage::Text(t) => return serde_json::from_str(&t).expect("json"),
            WsMessage::Ping(_) | WsMessage::Pong(_) => {}
            other => panic!("unexpected frame: {other:?}"),
        }
    }
}

/// 指定 `type` のフレームが来るまで読み飛ばす（status を挟んでも done を拾う）。
async fn read_until(ws: &mut WsStream, want_type: &str) -> Value {
    loop {
        let v = next_json(ws).await;
        if v["type"] == want_type {
            return v;
        }
    }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
