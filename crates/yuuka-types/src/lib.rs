//! yuuka-types — wire DTO + `Envelope<T>` + ts-rs による型生成（絶対制約5）。
//!
//! フロント(Svelte/TS)⇄Rust の型を **Rust を単一真実源**として生成する。生成は
//! `cargo xtask gen-types`（xtask が [`export_all`] を呼ぶ）に一元化し、
//! `frontend/src/lib/api/generated/` へ出力する。既存の手書き `types.ts` は
//! Phase 1 の各ドメイン移行に合わせて段階的に置換する（Phase 0 では触らない）。
//!
//! DAG: `types → core`（core にのみ依存）。

pub mod dto;
pub mod envelope;

pub use dto::{BotSummary, Role, SessionUser};
pub use envelope::Envelope;

use std::path::Path;

use ts_rs::TS;

/// 全 wire DTO を `base_dir/generated/` へ TypeScript として書き出す。
///
/// ts-rs 12 は `Config { export_dir, .. }` を基点に `#[ts(export_to = "generated/")]`
/// を解決する。`export_all` は依存型も辿るため、代表インスタンス化から一括生成する。
///
/// # Errors
/// ts-rs のシリアライズ／ファイル書き込みに失敗した場合 [`ts_rs::ExportError`]。
pub fn export_all(base_dir: &Path) -> Result<(), ts_rs::ExportError> {
    let cfg = ts_rs::Config::new().with_out_dir(base_dir.to_path_buf());
    // 各型を明示 export（Envelope の `#[serde(flatten)]` は data 型をインライン化し
    // 依存として単独 export しないため、SessionUser 等も個別に書き出す）。export_all は
    // 依存も辿り冪等に上書きする。
    <Role as TS>::export_all(&cfg)?;
    <SessionUser as TS>::export_all(&cfg)?;
    <BotSummary as TS>::export_all(&cfg)?;
    <Envelope<SessionUser> as TS>::export_all(&cfg)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtos_carry_no_secret_fields() {
        // 生成 TS に機密列名が現れないこと（構造的フェイルクローズ・R-13）を文字列で検証。
        let decl = <BotSummary as TS>::decl(&ts_rs::Config::new());
        for forbidden in ["encrypted", "token_", "_iv", "_tag", "password", "salt", "secret"] {
            assert!(
                !decl.contains(forbidden),
                "DTO leaks a secret-shaped field ({forbidden}): {decl}"
            );
        }
    }

    #[test]
    fn export_all_writes_bindings() {
        let dir = std::env::temp_dir().join(format!("yuuka_types_export_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        export_all(&dir).expect("export bindings");

        let gen = dir.join("generated");
        for name in ["Role.ts", "SessionUser.ts", "BotSummary.ts", "Envelope.ts"] {
            assert!(gen.join(name).exists(), "missing generated file: {name}");
        }
        // SessionUser は camelCase 生成（discordId）であること。
        let su = std::fs::read_to_string(gen.join("SessionUser.ts")).expect("read SessionUser.ts");
        assert!(su.contains("discordId"), "SessionUser not camelCased: {su}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
