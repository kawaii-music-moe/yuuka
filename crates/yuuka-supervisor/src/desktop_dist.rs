//! デスクトップクライアント配布 Web-API（Node `desktopClientRoutes`・auth:user）。
//!
//! `GET /api/desktop/info`（配布バイナリのメタ情報）と `GET /api/desktop/download`
//! （Windows 版 `yuuka-desktop.exe` の添付配信）。バイナリは Docker の desktop-builder ステージで
//! 生成され配布ディレクトリ（既定 `dist/downloads`・env `DESKTOP_DOWNLOAD_DIR` で上書き）へ置かれる。
//! ファイルが無い環境（ローカル/本 Rust ランタイム＝exe 非同梱）では info が `available:false` を返し、
//! ダッシュボードがその旨を表示する（Node と同挙動）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use yuuka_web::{AppState, AuthenticatedUser};

const EXE_NAME: &str = "yuuka-desktop.exe";

/// 配布バイナリのメタ情報（Node `readDesktopMeta`）。
struct DesktopMeta {
    available: bool,
    size: u64,
    version: String,
    built_at: Option<String>,
}

/// 配布ディレクトリを解決する（env `DESKTOP_DOWNLOAD_DIR` → 既定 `cwd/dist/downloads`）。
fn resolve_download_dir() -> PathBuf {
    std::env::var_os("DESKTOP_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("dist")
                .join("downloads")
        })
}

/// デスクトップ配布ルータ（配布ディレクトリを env/cwd から解決して Extension に載せる）。
pub fn routes() -> Router<AppState> {
    routes_in(resolve_download_dir())
}

/// 配布ディレクトリを与えてルータを組む（テスト用に dir 注入可能）。
pub fn routes_in(dir: PathBuf) -> Router<AppState> {
    Router::new()
        .route("/api/desktop/info", get(info))
        .route("/api/desktop/download", get(download))
        .layer(Extension(Arc::new(dir)))
}

/// バイナリのメタ情報を読む（存在しなければ `available:false`・Node `readDesktopMeta`）。
async fn read_meta(dir: &Path) -> DesktopMeta {
    let exe = dir.join(EXE_NAME);
    match tokio::fs::metadata(&exe).await {
        Ok(stat) if stat.is_file() => {
            // version.txt が無くてもバイナリ配布は可能（version 不明扱い）。
            let version = tokio::fs::read_to_string(dir.join("version.txt"))
                .await
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".to_owned());
            let built_at = stat.modified().ok().map(|t| {
                chrono::DateTime::<chrono::Utc>::from(t)
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            });
            DesktopMeta {
                available: true,
                size: stat.len(),
                version,
                built_at,
            }
        }
        _ => DesktopMeta {
            available: false,
            size: 0,
            version: "unknown".to_owned(),
            built_at: None,
        },
    }
}

async fn info(
    _user: AuthenticatedUser,
    _state: State<AppState>,
    Extension(dir): Extension<Arc<PathBuf>>,
) -> Response {
    let meta = read_meta(&dir).await;
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "available": meta.available,
            "filename": EXE_NAME,
            "size": meta.size,
            "version": meta.version,
            "built_at": meta.built_at,
        })),
    )
        .into_response()
}

async fn download(
    _user: AuthenticatedUser,
    _state: State<AppState>,
    Extension(dir): Extension<Arc<PathBuf>>,
) -> Response {
    let meta = read_meta(&dir).await;
    if !meta.available {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "message": "デスクトップ版のバイナリがまだ配置されていません。デプロイ後にお試しください。",
            })),
        )
            .into_response();
    }
    // exe は非同梱環境が通常のため、存在時のみ読み出す（メモリ読み込み・添付配信）。
    match tokio::fs::read(dir.join(EXE_NAME)).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(
                header::CONTENT_TYPE,
                "application/vnd.microsoft.portable-executable",
            )
            .header(header::CONTENT_LENGTH, bytes.len())
            .header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{EXE_NAME}\""),
            )
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(axum::body::Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        // 読み出し失敗（配信途中の消失等）は 404 相当へ縮退。
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "success": false, "message": "配信に失敗しました。" })),
        )
            .into_response(),
    }
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
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn test_db() -> Db {
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_desktopdist_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).expect("create empty");
            drop(conn);
        }
        Db::open(&path).expect("open")
    }

    fn app(dir: std::path::PathBuf) -> axum::Router {
        let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), test_db());
        super::routes_in(dir).with_state(state)
    }

    async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Vec<u8>) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, bytes.to_vec())
    }

    #[tokio::test]
    async fn info_reports_unavailable_when_missing() {
        let dir = std::env::temp_dir().join(format!("yuuka_dl_missing_{}", std::process::id()));
        let app = app(dir);
        let (st, body) = get(&app, "/api/desktop/info").await;
        assert_eq!(st, StatusCode::OK);
        let j: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(j["available"], false);
        assert_eq!(j["filename"], "yuuka-desktop.exe");
        assert_eq!(j["version"], "unknown");
        assert!(j["built_at"].is_null());

        let (st, _) = get(&app, "/api/desktop/download").await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn info_and_download_when_present() {
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("yuuka_dl_present_{}_{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("yuuka-desktop.exe"), b"MZ\x00\x01binary").unwrap();
        std::fs::write(dir.join("version.txt"), "1.2.3\n").unwrap();

        let app = app(dir.clone());
        let (st, body) = get(&app, "/api/desktop/info").await;
        assert_eq!(st, StatusCode::OK);
        let j: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(j["available"], true);
        assert_eq!(j["version"], "1.2.3");
        assert_eq!(j["size"], 10);
        assert!(j["built_at"].as_str().unwrap().ends_with('Z'));

        let (st, bytes) = get(&app, "/api/desktop/download").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(bytes, b"MZ\x00\x01binary");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
