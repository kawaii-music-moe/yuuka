//! レシート専用 API / Discord OCR の入力検証（汎用チャット添付とは別契約）。
use base64::Engine as _;

/// MIME を正規化し、許可画像形式と非空の base64 を検証する。
/// サイズは HTTP の既存ボディ上限と同じ 10 MiB（エンコード後）。画像実体のデコードは Gemini に任せる。
///
/// # Errors
/// 不正・空の入力、非対応 MIME、サイズ超過はユーザー向けメッセージを返す。
pub fn validate(image: &str, mime: &str) -> Result<String, &'static str> {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if image.is_empty() || mime.is_empty() {
        return Err("画像データ(base64)とMIMEタイプが必要です。");
    }
    if ![
        "image/png",
        "image/jpeg",
        "image/jpg",
        "image/webp",
        "image/heic",
        "image/heif",
        "image/gif",
    ]
    .contains(&mime.as_str())
    {
        return Err("対応していない画像形式です（png/jpeg/webp/heic/gif）。");
    }
    if image.len() > 10 * 1024 * 1024 {
        return Err("画像が大きすぎます。画像を縮小してください。");
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image)
        .map_err(|_| "画像データを読み込めませんでした。")?;
    if bytes.is_empty() {
        return Err("画像データを読み込めませんでした。");
    }
    Ok(mime)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_before_llm_and_normalizes_mime() {
        assert_eq!(
            validate("QUJD", " IMAGE/PNG ; charset=x"),
            Ok("image/png".to_owned())
        );
        for (data, mime) in [
            ("", "image/png"),
            ("!", "image/png"),
            ("QUJD", "text/plain"),
            ("QUJD", ""),
        ] {
            assert!(validate(data, mime).is_err());
        }
        assert!(validate(&"A".repeat(10 * 1024 * 1024 + 1), "image/png").is_err());
    }
}
