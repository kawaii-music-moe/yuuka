//! PIN 付きアカウント/Bot 移行パッケージの生成。

use axum::extract::State;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use yuuka_crypto::{derive_user_key, encrypt_bytes_with_key, generate_user_salt};
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

const PACKAGE_FORMAT: &str = "yuuka-bot-package";
const PACKAGE_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
pub(crate) struct PackageInput {
    pin: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageEnvelope {
    format: &'static str,
    version: u32,
    created_at: String,
    kdf: &'static str,
    cipher: &'static str,
    salt: String,
    iv: String,
    auth_tag: String,
    ciphertext: String,
}

pub(crate) async fn download(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<PackageInput>,
) -> Result<Response, ApiError> {
    if !valid_pin(&body.pin) {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "message": "PINコードは6〜12桁の半角数字で入力してください。"})),
        )
            .into_response());
    }

    let database = yuuka_google::export_user_database(&state.db, &user.0.discord_id).await?;
    let media = package_media(&state, &user.0.discord_id).await?;
    let payload =
        zip_payload(&database, &media).map_err(|_| ApiError(yuuka_core::WebError::Internal))?;
    let salt = generate_user_salt().map_err(|_| ApiError(yuuka_core::WebError::Internal))?;
    let key =
        derive_user_key(&body.pin, &salt).map_err(|_| ApiError(yuuka_core::WebError::Internal))?;
    let encrypted = encrypt_bytes_with_key(&key, &payload)
        .map_err(|_| ApiError(yuuka_core::WebError::Internal))?;
    let now = Utc::now();
    let envelope = PackageEnvelope {
        format: PACKAGE_FORMAT,
        version: PACKAGE_VERSION,
        created_at: now.to_rfc3339(),
        kdf: "argon2id-m19456-t2-p1",
        cipher: "aes-256-gcm",
        salt,
        iv: encrypted.iv,
        auth_tag: encrypted.auth_tag,
        ciphertext: encrypted.encrypted,
    };
    let bytes =
        serde_json::to_vec(&envelope).map_err(|_| ApiError(yuuka_core::WebError::Internal))?;
    let filename = format!("yuuka-bots-{}.yuuka", now.format("%Y%m%d-%H%M%S"));
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.yuuka.package+json"),
    );
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    Ok((headers, bytes).into_response())
}

async fn package_media(
    state: &AppState,
    user_id: &str,
) -> Result<Vec<(String, Vec<u8>)>, ApiError> {
    let uid = user_id.to_owned();
    let names = state
        .db
        .read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT t.media_path FROM timeline_records t \
                     LEFT JOIN bots b ON b.id = t.bot_id \
                     WHERE t.media_path IS NOT NULL \
                       AND (t.user_id = ?1 OR b.user_id = ?1)",
                )
                .map_err(yuuka_db::map_sqlite)?;
            let rows = stmt
                .query_map([uid], |row| row.get::<_, String>(0))
                .map_err(yuuka_db::map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(yuuka_db::map_sqlite)
        })
        .await?;

    let mut media = Vec::with_capacity(names.len());
    for name in names {
        let path = safe_media_path(&state.config.media_dir, &name)
            .ok_or(ApiError(yuuka_core::WebError::Internal))?;
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|_| ApiError(yuuka_core::WebError::Internal))?;
        media.push((name, bytes));
    }
    Ok(media)
}

fn safe_media_path(root: &Path, name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    (path.file_name()?.to_str()? == name).then(|| root.join(name))
}

fn zip_payload(
    database: &[u8],
    media: &[(String, Vec<u8>)],
) -> Result<Vec<u8>, zip::result::ZipError> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("data/yuuka.db", options)?;
        zip.write_all(database)?;
        for (name, bytes) in media {
            zip.start_file(format!("data/media/{name}"), options)?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

fn valid_pin(pin: &str) -> bool {
    (6..=12).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{safe_media_path, valid_pin, zip_payload};
    use std::io::Read as _;

    #[test]
    fn pin_policy() {
        assert!(valid_pin("123456"));
        assert!(valid_pin("012345678901"));
        assert!(!valid_pin("12345"));
        assert!(!valid_pin("12345a"));
        assert!(!valid_pin("１２３４５６"));
    }

    #[test]
    fn payload_contains_database_and_media() {
        let bytes = zip_payload(&[1, 2, 3], &[("photo.jpg".into(), vec![4, 5])]).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut db = Vec::new();
        archive
            .by_name("data/yuuka.db")
            .unwrap()
            .read_to_end(&mut db)
            .unwrap();
        assert_eq!(db, [1, 2, 3]);
        assert!(archive.by_name("data/media/photo.jpg").is_ok());
    }

    #[test]
    fn media_path_rejects_traversal() {
        let root = std::path::Path::new("data/media");
        assert!(safe_media_path(root, "photo.jpg").is_some());
        assert!(safe_media_path(root, "../secret").is_none());
        assert!(safe_media_path(root, "nested/photo.jpg").is_none());
    }
}
