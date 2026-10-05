//! タイムライン メディアの保存/配信ユーティリティ（Node `timelineRepo` の `saveMediaFile`/
//! `resolveMediaPath`/`mimeTypeToExt` と `timelineRoutes` の `MIME_MAP` を移植）。
//!
//! 保存ディレクトリは [`WebConfig::media_dir`](yuuka_web::WebConfig)（既定 `data/media`）。ファイル名は
//! `'{YYYYMM}-{millis}-{suffix}{ext}'`（Node は `Date.now()` + `Math.random()`・ここは millis + 単調
//! カウンタで衝突回避）。配信は path traversal 対策（`/`・`\`・`..` 拒否 + prefix 検証）付き。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;

/// 許可するメディア拡張子（Node `ALLOWED_MEDIA_EXTS`）。
const ALLOWED_EXTS: &[&str] = &[
    ".jpg", ".jpeg", ".png", ".webp", ".gif", ".heic", ".heif", ".mp4", ".mov", ".webm", ".m4v",
];

/// MIME → 拡張子（Node `mimeTypeToExt`・未知は `.bin`）。
fn mime_to_ext(mime: &str) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => ".jpg",
        "image/png" => ".png",
        "image/webp" => ".webp",
        "image/gif" => ".gif",
        "image/heic" => ".heic",
        "image/heif" => ".heif",
        "video/mp4" => ".mp4",
        "video/quicktime" => ".mov",
        "video/webm" => ".webm",
        "video/x-m4v" => ".m4v",
        _ => ".bin",
    }
}

/// 配信時の Content-Type（Node `MIME_MAP`・未知は `application/octet-stream`）。
#[must_use]
pub fn content_type_for(filename: &str) -> &'static str {
    let lower = filename.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "m4v" => "video/x-m4v",
        _ => "application/octet-stream",
    }
}

/// `mime_type` から `"photo" | "video"` を返す（Node `mimeType.startsWith("video/")`）。
#[must_use]
pub fn media_type_of(mime: &str) -> &'static str {
    if mime.to_ascii_lowercase().starts_with("video/") {
        "video"
    } else {
        "photo"
    }
}

/// base64 のメディアをローカル保存し、相対ファイル名を返す（Node `saveMediaFile` の base64 経路）。
///
/// # Errors
/// MIME が許可拡張子に写らない・base64 デコード失敗・ディレクトリ作成/書き込み失敗時に
/// 人間可読なメッセージ（Node の `throw new Error(...)` に対応）。
pub async fn save_media_base64(
    media_dir: &Path,
    base64_data: &str,
    mime_type: &str,
    date: &str,
) -> Result<String, String> {
    let ext = mime_to_ext(mime_type);
    if !ALLOWED_EXTS.contains(&ext) {
        return Err(format!("不正なメディア形式: {mime_type}"));
    }
    tokio::fs::create_dir_all(media_dir)
        .await
        .map_err(|e| format!("メディア保存先の作成に失敗しました: {e}"))?;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data.as_bytes())
        .map_err(|e| format!("base64 のデコードに失敗しました: {e}"))?;

    let filename = generate_filename(date, ext);
    let full = media_dir.join(&filename);
    tokio::fs::write(&full, &bytes)
        .await
        .map_err(|e| format!("メディアの書き込みに失敗しました: {e}"))?;
    Ok(filename)
}

/// メディアファイルのフルパスを返す（path traversal 対策済み・Node `resolveMediaPath`）。
///
/// `/`・`\`・`..` を含むファイル名は拒否し、結合後に `media_dir` 配下であることを検証する。
#[must_use]
pub fn resolve_media_path(media_dir: &Path, filename: &str) -> Option<PathBuf> {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return None;
    }
    let full = media_dir.join(filename);
    // 防御的二重確認（filename に区切りが無いので通常は自明だが Node の startsWith 検証に合わせる）。
    if !full.starts_with(media_dir) {
        return None;
    }
    Some(full)
}

/// `'{YYYYMM}-{millis}-{suffix}{ext}'`（Node `${yyyymm}-${Date.now()}-${rand6}${ext}`）。
///
/// suffix は **CSPRNG**（`getrandom`・4 バイト=32bit hex）。配信は所有者非照合（filename ベース・Node
/// パリティ）なため、ファイル名の推測不能性が唯一の障壁になる。単調カウンタだと総当たり可能なため
/// 暗号乱数にする（Node の `Math.random` 31bit 以上を確保）。乱数取得失敗時は millis の hex を fallback。
fn generate_filename(date: &str, ext: &str) -> String {
    let yyyymm: String = date.get(0..7).unwrap_or("000000").replace('-', "");
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut buf = [0u8; 4];
    let suffix = if getrandom::getrandom(&mut buf).is_ok() {
        buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
    } else {
        format!("{millis:08x}")
    };
    format!("{yyyymm}-{millis}-{suffix}{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_and_content_type_mapping() {
        assert_eq!(mime_to_ext("image/jpeg"), ".jpg");
        assert_eq!(mime_to_ext("video/mp4"), ".mp4");
        assert_eq!(mime_to_ext("application/pdf"), ".bin");
        assert_eq!(content_type_for("a.jpg"), "image/jpeg");
        assert_eq!(content_type_for("a.MOV"), "video/quicktime");
        assert_eq!(content_type_for("a.txt"), "application/octet-stream");
        assert_eq!(media_type_of("video/webm"), "video");
        assert_eq!(media_type_of("image/png"), "photo");
    }

    #[test]
    fn resolve_rejects_traversal() {
        let dir = Path::new("/tmp/media");
        assert!(resolve_media_path(dir, "../etc/passwd").is_none());
        assert!(resolve_media_path(dir, "a/b.jpg").is_none());
        assert!(resolve_media_path(dir, "a\\b.jpg").is_none());
        assert_eq!(
            resolve_media_path(dir, "202607-1-abc.jpg"),
            Some(dir.join("202607-1-abc.jpg"))
        );
    }

    #[tokio::test]
    async fn save_base64_writes_file_and_rejects_bad_mime() {
        let dir = tempfile::tempdir().unwrap();
        // "hello" を base64。
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let name = save_media_base64(dir.path(), &b64, "image/png", "2026-07-06")
            .await
            .unwrap();
        assert!(name.starts_with("202607-"));
        assert!(name.ends_with(".png"));
        let content = std::fs::read(dir.path().join(&name)).unwrap();
        assert_eq!(content, b"hello");

        // 不正 MIME は Err。
        let err = save_media_base64(dir.path(), &b64, "application/pdf", "2026-07-06").await;
        assert!(err.is_err());
    }
}
