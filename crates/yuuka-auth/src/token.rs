//! CSPRNG セッショントークン生成（Node `generateToken` パリティ）。
//!
//! Node `src/utils/crypto.ts`: `crypto.randomBytes(32).toString("base64url")` ＝
//! 32 バイトの CSPRNG を **URL セーフ base64（パディング無し）** で符号化した 43 文字。
//! 検証側（[`crate::sha256_hex`] → Redis `session:{hash}`）と対になる発行側の唯一の実装。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;

/// Node `generateToken()` と同一形式の不透明トークンを 1 本発行する。
///
/// 32 バイト CSPRNG → base64url（no-pad, 43 文字）。予約文字を含まないため cookie/Redis キーで
/// そのまま安全に使える（`__Host-` cookie・`session:{sha256hex(token)}`）。
///
/// # Errors
/// CSPRNG（`getrandom`）が失敗した場合 [`getrandom::Error`]。弱い/予測可能なトークンを返すより
/// 呼び出し側へ失敗を伝播させる（OS のエントロピー枯渇は実質起こらない）。
pub fn generate_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::generate_token;

    #[test]
    fn token_is_43_char_url_safe_no_pad() {
        let t = generate_token().expect("token");
        // 32 バイト → base64url no-pad = 43 文字（Node randomBytes(32).toString("base64url") と同じ長さ）。
        assert_eq!(t.len(), 43, "token = {t}");
        // URL セーフ集合（A-Za-z0-9-_）のみ・`+`/`/`/`=` を含まない。
        assert!(
            t.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "非 URL セーフ文字を含む: {t}"
        );
    }

    #[test]
    fn tokens_are_unique() {
        // 連続発行が衝突しない（CSPRNG である最低限の担保）。
        let a = generate_token().expect("token");
        let b = generate_token().expect("token");
        assert_ne!(a, b);
    }
}
