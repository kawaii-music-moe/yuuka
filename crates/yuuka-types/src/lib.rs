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
    // ★ Phase 1 で DTO を追加したら**必ずここに1行足す**こと。忘れると gen-types --check が
    //   ドリフトを見逃す。テスト `every_ts_derive_is_exported` が登録漏れを検出する。
    <Role as TS>::export_all(&cfg)?;
    <SessionUser as TS>::export_all(&cfg)?;
    <BotSummary as TS>::export_all(&cfg)?;
    <Envelope<SessionUser> as TS>::export_all(&cfg)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dto.rs / envelope.rs のソースを走査し、`TS` を derive する型名を拾う。
    fn ts_derived_types(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut armed = false;
        for line in src.lines() {
            let t = line.trim_start();
            if t.starts_with("#[derive(") && t.contains("TS") {
                armed = true;
                continue;
            }
            if armed {
                if t.starts_with("#[") {
                    continue; // #[serde(...)] / #[ts(...)] 等の属性行はスキップ。
                }
                let decl = t
                    .strip_prefix("pub struct ")
                    .or_else(|| t.strip_prefix("pub enum "));
                if let Some(rest) = decl {
                    let ident: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    if !ident.is_empty() {
                        out.push(ident);
                    }
                }
                armed = false;
            }
        }
        out
    }

    #[test]
    fn every_ts_derive_is_exported() {
        // TS を derive する型が全て export_all で生成されるか検査。DTO を追加し export_all への
        // 登録を忘れると、再生成側もコミット側も欠落して gen-types --check が素通りする
        // （単一真実源の空洞化・B-1）。この test がその漏れをテスト時に捕捉する。
        let mut expected = ts_derived_types(include_str!("dto.rs"));
        expected.extend(ts_derived_types(include_str!("envelope.rs")));
        assert!(!expected.is_empty(), "source scan found no TS-deriving types");

        let dir =
            std::env::temp_dir().join(format!("yuuka_types_complete_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        export_all(&dir).expect("export bindings");
        let gen = dir.join("generated");
        for name in &expected {
            assert!(
                gen.join(format!("{name}.ts")).exists(),
                "type `{name}` derives TS but export_all() did not emit it — add it to export_all \
                 (otherwise gen-types --check would silently miss the drift)"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_dto_exposes_secret_fields() {
        // 全生成 TS の**コード行**（docstring 除外）を走査し、機密を示唆するフィールド名が
        // 現れないことを検査（構造的フェイルクローズの回帰防止・R-13/B-2）。
        let dir = std::env::temp_dir().join(format!("yuuka_types_secret_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        export_all(&dir).expect("export bindings");
        let gen = dir.join("generated");

        // 機密を示唆する断片（部分一致）。allow は「機密の有無を示す存在フラグ」で値ではない。
        let forbidden = [
            "token", "secret", "password", "passwd", "hash", "cipher", "ciphertext", "encrypted",
            "salt", "apikey", "api_key", "privatekey", "private_key", "credential", "_iv", "_tag",
            "refresh",
        ];
        let allow = ["has_token", "has_gemini_key", "hastoken", "hasgeminikey"];

        let mut checked = 0_usize;
        for entry in std::fs::read_dir(&gen).expect("read generated dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("ts") {
                continue;
            }
            let content = std::fs::read_to_string(&path).expect("read ts file");
            // docstring/コメント行を除外し、型定義（フィールド）行のみ検査する。
            let code = content
                .lines()
                .map(str::trim_start)
                .filter(|l| {
                    !(l.starts_with("//")
                        || l.starts_with("/*")
                        || l.starts_with('*')
                        || l.is_empty())
                })
                .collect::<Vec<_>>()
                .join("\n")
                .to_ascii_lowercase();
            // 許可語を無害化してから禁止語を探す（"has_token" 内の "token" を誤検知しない）。
            let mut scan = code;
            for a in allow {
                scan = scan.replace(a, "__allowed__");
            }
            for bad in forbidden {
                assert!(
                    !scan.contains(bad),
                    "generated {:?} appears to expose a secret-shaped field containing {bad:?}",
                    path.file_name()
                );
            }
            checked += 1;
        }
        assert!(
            checked >= 4,
            "expected to scan >= 4 generated files, scanned {checked}"
        );
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
