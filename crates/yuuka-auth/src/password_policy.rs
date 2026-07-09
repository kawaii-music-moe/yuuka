//! パスワードポリシー検証（Node `src/services/passwordPolicy.ts` §5.4.3 パリティ）。
//!
//! 規則（先勝ち・最初の失敗で確定）:
//! 1. 長さ **8 文字以上**（Node `password.length` ＝ UTF-16 コード単位。`encode_utf16().count()` で一致）。
//! 2. 大文字 / 小文字 / 数字 / 記号（`[^A-Za-z0-9]`）の **2 種類以上**。
//! 3. よく使われるパスワード（`common-passwords-10k.txt` top-10k）で**ない**こと（**fail-open**：
//!    ファイルが見つからなければチェックをスキップし登録を妨げない・Node と同一）。
//!
//! 失敗メッセージは Node の日本語文言とバイト単位で一致させる（フロントが文言依存のため）。

use std::collections::HashSet;
use std::sync::OnceLock;

/// 検証失敗時のユーザー向けメッセージ（Node と一字一句一致）。
const MSG_TOO_SHORT: &str = "パスワードは8文字以上にしてください。";
const MSG_TOO_FEW_KINDS: &str = "大文字・小文字・数字・記号のうち2種類以上を含めてください。";
const MSG_COMMON: &str =
    "よく使われるパスワードのため使用できません。別のパスワードを設定してください。";

/// パスワードがポリシーを満たすか検証する。`Ok(())` なら合格、`Err(msg)` は失敗理由（Node 文言）。
///
/// # Errors
/// ポリシー違反時にユーザー向け日本語メッセージ（Node パリティ）を返す。
pub fn validate_password(password: &str) -> Result<(), &'static str> {
    // 1) 長さ: Node は UTF-16 コード単位（`String.length`）。サロゲートペアを 2 と数える点まで一致させる。
    if password.encode_utf16().count() < 8 {
        return Err(MSG_TOO_SHORT);
    }

    // 2) 文字種: 大文字・小文字・数字・記号（非英数）のうち 2 種類以上。
    let mut kinds = 0;
    if password.chars().any(|c| c.is_ascii_uppercase()) {
        kinds += 1;
    }
    if password.chars().any(|c| c.is_ascii_lowercase()) {
        kinds += 1;
    }
    if password.chars().any(|c| c.is_ascii_digit()) {
        kinds += 1;
    }
    // Node `/[^A-Za-z0-9]/`: ASCII 英数**以外**（空白・記号・非 ASCII すべて）を「記号」種とみなす。
    if password.chars().any(|c| !c.is_ascii_alphanumeric()) {
        kinds += 1;
    }
    if kinds < 2 {
        return Err(MSG_TOO_FEW_KINDS);
    }

    // 3) 一般的パスワード denylist（fail-open）。Node は `password.toLowerCase()` で照合。
    if common_passwords().contains(&password.to_lowercase()) {
        return Err(MSG_COMMON);
    }

    Ok(())
}

/// top-10k デニーリスト（遅延ロード・**fail-open**）。ファイルが無ければ空集合＝チェックをスキップ。
///
/// Node と同じ候補パス（cwd 相対）を順に試し、最初に存在したものを採用する。読み込みは 1 回だけ。
fn common_passwords() -> &'static HashSet<String> {
    static CACHE: OnceLock<HashSet<String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        const CANDIDATES: [&str; 2] = [
            "src/assets/common-passwords-10k.txt",
            "dist/assets/common-passwords-10k.txt",
        ];
        for candidate in CANDIDATES {
            match std::fs::read_to_string(candidate) {
                Ok(raw) => {
                    let set: HashSet<String> = raw
                        .lines()
                        .map(|l| l.trim().to_lowercase())
                        .filter(|l| !l.is_empty())
                        .collect();
                    tracing::info!(count = set.len(), file = candidate, "一般的パスワードリストを読み込み");
                    return set;
                }
                Err(_) => continue,
            }
        }
        // fail-open: 見つからなければチェックをスキップ（Node と同一・登録を妨げない）。
        tracing::warn!("common-passwords-10k.txt が見つかりません。一般的パスワードチェックをスキップします");
        HashSet::new()
    })
}

#[cfg(test)]
mod tests {
    use super::{validate_password, MSG_TOO_FEW_KINDS, MSG_TOO_SHORT};

    #[test]
    fn rejects_short_password() {
        assert_eq!(validate_password("Aa1x"), Err(MSG_TOO_SHORT));
        // 7 文字は不合格、8 文字（2 種以上）は合格。
        assert_eq!(validate_password("Abc123!"), Err(MSG_TOO_SHORT));
    }

    #[test]
    fn rejects_single_char_class() {
        // 8 文字だが小文字のみ（1 種）→ 不合格。
        assert_eq!(validate_password("abcdefgh"), Err(MSG_TOO_FEW_KINDS));
        // 数字のみ（1 種）。
        assert_eq!(validate_password("12345678"), Err(MSG_TOO_FEW_KINDS));
    }

    #[test]
    fn accepts_two_classes_min_length() {
        // 小文字 + 数字（2 種）・8 文字（denylist に無い前提の合成値）。
        assert!(validate_password("abcd1234efgh").is_ok());
        // 大文字 + 記号。
        assert!(validate_password("ABCD!@#$xyz").is_ok());
        // 非 ASCII を「記号」種として数える（日本語 + 英字 = 2 種）。
        assert!(validate_password("パスワードabc").is_ok());
    }

    #[test]
    fn utf16_length_counts_surrogate_pairs_as_two() {
        // 絵文字（サロゲートペア）4 個 = UTF-16 で 8 コード単位・chars では 4。
        // Node の `.length` は 8 とみなし合格（記号種 1 のみなので kinds<2 で TOO_FEW になる）。
        // ここでは長さ判定のみを検証: 8 コード単位なので TOO_SHORT にはならない。
        let emojis = "😀😀😀😀"; // 4 chars, 8 utf16 units
        assert_ne!(validate_password(emojis), Err(MSG_TOO_SHORT));
    }
}
