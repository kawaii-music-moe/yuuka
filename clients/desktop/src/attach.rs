//! 送信添付（画像）のステージング。
//!
//! 仕様: client_design.md §7 / backend_api.md §3.2。
//!
//! 取り込み経路は ① ファイル選択（`rfd`）② ドラッグ＆ドロップ（egui `dropped_files`）
//! ③ クリップボード貼り付け（`arboard`）の 3 つ。サムネ確認の上、WS の
//! `msg.image`（`{mime, data(base64)}`）として送る。
//!
//! バックエンドの会話コアは画像を Gemini ネイティブで処理する（外部 OCR/STT 不要）。
//! 非画像ファイルはサーバ側に処理経路が無いため、ここでは**画像のみ**受け付ける。

use std::path::Path;

/// 送信待ちの画像添付（生バイト＋MIME＋表示名）。base64 化は送信直前に行う。
#[derive(Debug, Clone)]
pub struct StagedImage {
    pub mime: String,
    pub bytes: Vec<u8>,
    pub name: String,
    /// 内容ハッシュ（egui テクスチャの安定 URI に使う）。名前が同じ別画像
    /// （例: 連続貼り付けの clipboard.png）でもキャッシュが古くならないよう内容で識別する。
    pub tag: u64,
}

/// 画像バイトの内容ハッシュ（URI 識別用。衝突回避が目的で暗号強度は不要）。
fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

impl StagedImage {
    /// 生バイトから構築する（`tag` は内容ハッシュで一度だけ計算）。
    pub fn new(mime: impl Into<String>, bytes: Vec<u8>, name: impl Into<String>) -> Self {
        let tag = hash_bytes(&bytes);
        StagedImage {
            mime: mime.into(),
            bytes,
            name: name.into(),
            tag,
        }
    }
}

/// 受け付ける画像拡張子（ファイル選択フィルタ／D&D 判定に使う）。
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];

/// 拡張子（大小無視）→ MIME。未対応は `None`。
pub fn mime_for_ext(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// ファイルパスから画像を取り込む。拡張子が画像でない/読めない場合は `None`。
pub fn from_path(path: &Path) -> Option<StagedImage> {
    let ext = path.extension()?.to_str()?;
    let mime = mime_for_ext(ext)?;
    let bytes = std::fs::read(path).ok()?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();
    Some(StagedImage::new(mime, bytes, name))
}

/// ファイル名（拡張子で MIME 判定）＋生バイトから取り込む（D&D が path を持たない場合用）。
pub fn from_named_bytes(name: &str, bytes: &[u8]) -> Option<StagedImage> {
    let ext = Path::new(name).extension()?.to_str()?;
    let mime = mime_for_ext(ext)?;
    Some(StagedImage::new(mime, bytes.to_vec(), name))
}

/// 生 RGBA8 画像を PNG へエンコードして取り込む（クリップボード画像用）。
pub fn from_rgba(width: usize, height: usize, rgba: &[u8]) -> Option<StagedImage> {
    let img = image::RgbaImage::from_raw(width as u32, height as u32, rgba.to_vec())?;
    let mut png: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(StagedImage::new("image/png", png, "clipboard.png"))
}

/// クリップボードの画像を取り込む（無ければ `None`）。`arboard` は RGBA8 を返す。
pub fn from_clipboard() -> Option<StagedImage> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    from_rgba(img.width, img.height, &img.bytes)
}

impl StagedImage {
    /// WS の base64 添付（`{mime, data}`）へ変換する。
    pub fn to_attachment(&self) -> crate::model::Attachment {
        use base64::Engine;
        crate::model::Attachment {
            mime: self.mime.clone(),
            data: base64::engine::general_purpose::STANDARD.encode(&self.bytes),
        }
    }

    /// base64 化後にサーバの添付上限を超えそうか（早期に弾くための概算）。
    ///
    /// サーバの上限はフレーム全体のバイト長に効くため、base64 膨張ぶん（約 4/3 倍）を
    /// 見込んで生バイトで判定する。
    pub fn exceeds_limit(&self, max_upload_mb: u32) -> bool {
        let limit = (max_upload_mb as u64) * 1024 * 1024;
        (self.bytes.len() as u64).saturating_mul(4) / 3 > limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_detection_is_case_insensitive() {
        assert_eq!(mime_for_ext("PNG"), Some("image/png"));
        assert_eq!(mime_for_ext("Jpg"), Some("image/jpeg"));
        assert_eq!(mime_for_ext("jpeg"), Some("image/jpeg"));
        assert_eq!(mime_for_ext("webp"), Some("image/webp"));
        assert_eq!(mime_for_ext("txt"), None);
    }

    #[test]
    fn from_named_bytes_rejects_non_image() {
        assert!(from_named_bytes("notes.txt", b"hello").is_none());
        let img = from_named_bytes("a.png", b"\x89PNG...").unwrap();
        assert_eq!(img.mime, "image/png");
        assert_eq!(img.name, "a.png");
    }

    #[test]
    fn exceeds_limit_accounts_for_base64_growth() {
        // 上限 1MB。生 1MB ちょうどは base64 で ~1.33MB → 超過。
        let one_mb = StagedImage::new("image/png", vec![0u8; 1024 * 1024], "x.png");
        assert!(one_mb.exceeds_limit(1));
        // 700KB は base64 ~0.93MB → 1MB 以内。
        let small = StagedImage::new("image/png", vec![0u8; 700 * 1024], "x.png");
        assert!(!small.exceeds_limit(1));
    }

    #[test]
    fn to_attachment_base64_roundtrips() {
        use base64::Engine;
        let s = StagedImage::new("image/png", vec![1, 2, 3, 4], "x.png");
        let att = s.to_attachment();
        assert_eq!(att.mime, "image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(att.data.as_bytes())
            .unwrap();
        assert_eq!(decoded, vec![1, 2, 3, 4]);
    }
}
