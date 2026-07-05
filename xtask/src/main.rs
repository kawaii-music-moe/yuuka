//! xtask — 開発補助タスクランナー（絶対制約5 の型生成器）。
//!
//! - `cargo run -p xtask -- gen-types`         : ts-rs 型を `frontend/src/lib/api/generated/` へ生成。
//! - `cargo run -p xtask -- gen-types --check` : 生成物が最新か検査（ドリフトあれば非ゼロ終了・R-12）。
//!
//! build.rs ではなく xtask パターン（副作用でワークツリーを汚さず再現性を確保）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gen-types") => {
            let check = args.iter().any(|a| a == "--check");
            match run_gen_types(check) {
                Ok(()) => ExitCode::SUCCESS,
                Err(msg) => {
                    eprintln!("xtask gen-types failed: {msg}");
                    ExitCode::FAILURE
                }
            }
        }
        other => {
            eprintln!("unknown xtask command: {other:?}");
            eprintln!("usage: cargo run -p xtask -- gen-types [--check]");
            ExitCode::FAILURE
        }
    }
}

/// frontend の API ディレクトリ（`apps/yuuka/frontend/src/lib/api`）を解決する。
///
/// xtask は `apps/yuuka/xtask` にあるため、親（`apps/yuuka`）配下を辿る。
fn frontend_api_dir() -> Option<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest.parent()?;
    Some(
        workspace
            .join("frontend")
            .join("src")
            .join("lib")
            .join("api"),
    )
}

fn run_gen_types(check: bool) -> Result<(), String> {
    let api_dir = frontend_api_dir().ok_or_else(|| "cannot locate frontend api dir".to_owned())?;

    if check {
        // 一時ディレクトリへ再生成し、コミット済みと内容比較（git 非依存）。
        let tmp = std::env::temp_dir().join("yuuka_gen_types_check");
        let _ = std::fs::remove_dir_all(&tmp);
        yuuka_types::export_all(&tmp).map_err(|e| e.to_string())?;
        let fresh = read_dir_map(&tmp.join("generated"));
        let committed = read_dir_map(&api_dir.join("generated"));
        let _ = std::fs::remove_dir_all(&tmp);
        if fresh != committed {
            return Err(
                "generated types are stale; run `cargo run -p xtask -- gen-types`".to_owned(),
            );
        }
        Ok(())
    } else {
        yuuka_types::export_all(&api_dir).map_err(|e| e.to_string())?;
        println!(
            "generated ts bindings under {}",
            api_dir.join("generated").display()
        );
        Ok(())
    }
}

/// ディレクトリ直下のファイル名→内容のマップ（内容比較用）。
fn read_dir_map(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let (Some(name), Ok(content)) = (
            path.file_name().and_then(|n| n.to_str()),
            std::fs::read_to_string(&path),
        ) {
            out.insert(name.to_owned(), content);
        }
    }
    out
}
