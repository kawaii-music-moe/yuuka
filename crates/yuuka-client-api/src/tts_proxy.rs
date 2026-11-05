//! `/api/client/tts/*` — GPU の無い端末のための、サーバーでの音声生成（`clients/pwa/server/tts`）への中継。
//!
//! 音声生成サーバーは認証を持たず 127.0.0.1 で待ち受けるので、ここでログイン済みかを確かめてから
//! `TTS_SERVER_URL` へ転送する。生成した音声は区切りごとに届くので、応答はストリームのまま返す
//! （最初の区切りが届いた時点で PWA が鳴らし始められる）。未設定なら 404 で、PWA は標準の読み上げを使う。
//!
//! `TTS_SIGNING_KEY` があれば、要求に Ed25519 の署名を付ける。音声生成サーバーは公開鍵だけを持ち、署名の
//! 無い要求・時刻のずれた要求・送り直された要求を断る（`clients/pwa/server/tts/auth.ts`）。別のマシンに置くときに使う。

use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use ring::rand::{SecureRandom as _, SystemRandom};
use ring::signature::Ed25519KeyPair;
use yuuka_core::secrets::ExposeSecret as _;
use yuuka_web::{AppState, AuthenticatedUser};

use crate::error::ClientApiError;

/// 中継する音声生成サーバーの操作（それ以外のパスは転送しない）。
const ALLOWED: [(&str, &str); 4] = [
    ("GET", "status"),
    ("POST", "speak"),
    ("POST", "design"),
    ("POST", "clone"),
];

/// 署名する文字列の先頭（形式を変えたら上げる。`auth.ts` の `SIGNATURE_VERSION` と合わせる）。
const SIGNATURE_VERSION: &str = "yuuka-tts-v1";
const HEADER_TIME: &str = "x-yuuka-tts-time";
const HEADER_NONCE: &str = "x-yuuka-tts-nonce";
const HEADER_BODY_SHA256: &str = "x-yuuka-tts-body-sha256";
const HEADER_SIGNATURE: &str = "x-yuuka-tts-signature";

/// 要求に付ける署名のヘッダー。
#[derive(Debug, PartialEq, Eq)]
struct SignedHeaders {
    body_sha256: String,
    signature: String,
}

/// `yuuka-tts-v1 \n METHOD \n path \n 時刻 ms \n nonce \n 本文の SHA-256（hex）` に Ed25519 で署名する。
fn sign_request(
    seed: &[u8],
    method: &str,
    path: &str,
    time_ms: &str,
    nonce: &str,
    body: &[u8],
) -> Result<SignedHeaders, ring::error::KeyRejected> {
    let key = Ed25519KeyPair::from_seed_unchecked(seed)?;
    let body_sha256 = hex::encode(ring::digest::digest(&ring::digest::SHA256, body));
    let message = [
        SIGNATURE_VERSION,
        method,
        path,
        time_ms,
        nonce,
        &body_sha256,
    ]
    .join("\n");
    Ok(SignedHeaders {
        signature: STANDARD.encode(key.sign(message.as_bytes())),
        body_sha256,
    })
}

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
    let mut request = client().request(method.clone(), url);
    if let Some(content_type) = headers.get(header::CONTENT_TYPE) {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(key) = &state.config.tts_signing_key {
        // 鍵は起動時に 32 バイトの seed か確かめてある（`Config::validate`）。
        let seed = STANDARD
            .decode(key.expose_secret().trim())
            .map_err(ClientApiError::internal)?;
        let mut nonce = [0u8; 16];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(ClientApiError::internal)?;
        let nonce = URL_SAFE_NO_PAD.encode(nonce);
        let time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(ClientApiError::internal)?
            .as_millis()
            .to_string();
        let signed = sign_request(
            &seed,
            method.as_str(),
            &format!("/{op}"),
            &time_ms,
            &nonce,
            &body,
        )
        .map_err(ClientApiError::internal)?;
        request = request
            .header(HEADER_TIME, time_ms)
            .header(HEADER_NONCE, nonce)
            .header(HEADER_BODY_SHA256, signed.body_sha256)
            .header(HEADER_SIGNATURE, signed.signature);
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// `clients/pwa/server/tts/auth.test.ts` と同じ固定の値。Ed25519 の署名は決定的なので、ここと Node の
    /// 署名が一致すれば、音声生成サーバーが本体の署名を確かめられる。
    #[test]
    fn signature_matches_the_tts_server_vector() {
        let seed: Vec<u8> = (0u8..32).collect();
        let signed = sign_request(
            &seed,
            "POST",
            "/speak",
            "1762300000000",
            "AAECAwQFBgcICQoLDA0ODw",
            "{\"text\":\"はい。\"}".as_bytes(),
        )
        .unwrap();
        assert_eq!(
            signed,
            SignedHeaders {
                body_sha256: "1e92f7713c5b643a5b11d074e0a309cc2e9d28b5e6d84608560d7f1b4321ce1b"
                    .to_owned(),
                signature: "qu2BWar/M/pR0mxaJBXJP3ovcHH+8rnyFeXhkhDCPUpDuEXEsSkItPfpQcjAjw2drbyze1kGUW1Poo3btYmwAg=="
                    .to_owned(),
            }
        );
    }
}
