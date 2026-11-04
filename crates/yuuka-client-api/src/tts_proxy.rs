//! `/api/client/tts/*` — GPU の無い端末のための、サーバーでの音声生成（`client/pwa/server/tts`）への中継。
//!
//! 音声生成サーバーは認証を持たず 127.0.0.1 で待ち受けるので、ここでログイン済みかを確かめてから
//! `TTS_SERVER_URL` へ転送する。生成した音声は区切りごとに届くので、応答はストリームのまま返す
//! （最初の区切りが届いた時点で PWA が鳴らし始められる）。未設定なら 404 で、PWA は標準の読み上げを使う。

use std::sync::OnceLock;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use yuuka_web::{AppState, AuthenticatedUser};

use crate::error::ClientApiError;

/// 中継する音声生成サーバーの操作（それ以外のパスは転送しない）。
const ALLOWED: [(&str, &str); 4] = [
    ("GET", "status"),
    ("POST", "speak"),
    ("POST", "design"),
    ("POST", "clone"),
];

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/client/tts/{op}", get(proxy).post(proxy))
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        // 生成は 1 文で数秒〜十数秒（初回はモデルの読み込みを含む）。応答全体の時間は区切らない。
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_default()
    })
}

fn unavailable(status: StatusCode, message: &str) -> ClientApiError {
    ClientApiError {
        status,
        message: message.to_owned(),
    }
}

async fn proxy(
    _user: AuthenticatedUser,
    State(state): State<AppState>,
    Path(op): Path<String>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ClientApiError> {
    if !ALLOWED.contains(&(method.as_str(), op.as_str())) {
        return Err(ClientApiError::not_found("Not found"));
    }
    let Some(base) = state.config.tts_server_url.as_deref() else {
        return Err(unavailable(
            StatusCode::NOT_FOUND,
            "サーバーでの音声生成は設定されていません",
        ));
    };
    let url = format!("{}/{op}", base.trim_end_matches('/'));
    let mut request = client().request(method, url);
    if let Some(content_type) = headers.get(header::CONTENT_TYPE) {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    let upstream = request.body(body).send().await.map_err(|e| {
        tracing::warn!(error = %e, "tts proxy: upstream unreachable");
        unavailable(
            StatusCode::SERVICE_UNAVAILABLE,
            "サーバーでの音声生成を使えません（音声生成サーバーが動いていません）",
        )
    })?;
    let mut response = Response::builder().status(upstream.status().as_u16());
    for name in [header::CONTENT_TYPE.as_str(), "x-sample-rate"] {
        if let Some(value) = upstream.headers().get(name) {
            response = response.header(name, value);
        }
    }
    response
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(upstream.bytes_stream()))
        .map(IntoResponse::into_response)
        .map_err(ClientApiError::internal)
}
