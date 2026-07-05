//! In-memory 機密の取り扱い（§10.2・確定選定#19）。
//!
//! 復号後の Discord トークン・Gemini キー・MCP 資格情報等をメモリ保持する間は
//! [`secrecy::SecretString`] で包む。`Debug` を redact し drop で zeroize、
//! `expose_secret()` でのみ露出する。ログ/`Debug` 経由の漏洩を塞ぐ。
//!
//! 機密は wire DTO の**フィールドに存在させない**（構造的フェイルクローズ）。
//! ここは「保持」の共通型を再エクスポートするだけで、DTO 側では使わない。

pub use secrecy::{ExposeSecret, SecretBox, SecretString};

/// プレーン文字列を機密として包む薄いヘルパ。
#[must_use]
pub fn wrap_secret(plain: String) -> SecretString {
    SecretString::from(plain)
}
