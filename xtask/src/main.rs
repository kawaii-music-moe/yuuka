//! xtask — 開発補助タスクランナー（絶対制約5 の型生成器）。
//!
//! - `cargo run -p xtask -- gen-types`         : ts-rs 型を `frontend/src/lib/api/generated/` へ生成。
//! - `cargo run -p xtask -- gen-types --check` : 生成物が最新か検査（ドリフトあれば非ゼロ終了・R-12）。
//! - `cargo run -p xtask -- salvage-snapshot ...` : 稼働中の DB のスナップショットを取る（[`salvage`] 参照）。
//! - `cargo run -p xtask -- salvage-import ...` : そのスナップショットを、初期セットアップし直した
//!   新環境へ投入する（[`salvage`] 参照）。
//!
//! build.rs ではなく xtask パターン（副作用でワークツリーを汚さず再現性を確保）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod salvage;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("gen-types") => {
            let check = rest.iter().any(|a| a == "--check");
            finish("gen-types", run_gen_types(check))
        }
        Some("salvage-snapshot") => finish("salvage-snapshot", salvage::snapshot(rest)),
        Some("salvage-import") => finish("salvage-import", salvage::run(rest)),
        other => {
            eprintln!("unknown xtask command: {other:?}");
            eprintln!("usage: cargo run -p xtask -- gen-types [--check]");
            eprintln!("       {}", salvage::SNAPSHOT_USAGE);
            eprintln!("       {}", salvage::USAGE);
            ExitCode::FAILURE
        }
    }
}

/// サブコマンドの結果を終了コードにする（失敗ならコマンド名つきで標準エラーへ出す）。
fn finish(command: &str, result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("xtask {command} failed: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// 生成型の置き場（`frontend/src/lib/api/generated`）を解決する。
///
/// xtask はワークスペース直下の `xtask/` にあるため、その親から辿る。
fn generated_dir() -> Result<PathBuf, String> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .ok_or_else(|| "cannot locate workspace root".to_owned())?;
    Ok(workspace.join("frontend/src/lib/api/generated"))
}

/// 生成は一時ディレクトリで行い、コミット済みの生成型と内容で比べる（git 非依存）。
/// `--check` でなければ差分だけ書き込み、DTO が消えて生成されなくなった型のファイルは削除する。
fn run_gen_types(check: bool) -> Result<(), String> {
    let generated = generated_dir()?;
    let fresh = render_bindings()?;
    let committed = read_dir_map(&generated);

    if check {
        if fresh != committed {
            return Err(
                "generated types are stale; run `cargo run -p xtask -- gen-types`".to_owned(),
            );
        }
        return Ok(());
    }

    std::fs::create_dir_all(&generated).map_err(|e| e.to_string())?;
    let mut removed = 0usize;
    for name in committed.keys().filter(|name| !fresh.contains_key(*name)) {
        std::fs::remove_file(generated.join(name)).map_err(|e| e.to_string())?;
        removed += 1;
    }
    let mut written = 0usize;
    for (name, content) in &fresh {
        if committed.get(name) != Some(content) {
            std::fs::write(generated.join(name), content).map_err(|e| e.to_string())?;
            written += 1;
        }
    }
    println!(
        "generated ts bindings under {} ({} files, {written} written, {removed} removed)",
        generated.display(),
        fresh.len()
    );
    Ok(())
}

/// 全ドメインの wire DTO を一時ディレクトリへ書き出し、ファイル名→内容のマップで返す。
fn render_bindings() -> Result<BTreeMap<String, String>, String> {
    // 並行して走っても衝突しないよう、プロセスごとに別のディレクトリを使う。
    let tmp = std::env::temp_dir().join(format!("yuuka_gen_types_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let exported = export_all_domains(&tmp).map(|()| read_dir_map(&tmp.join("generated")));
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(exported?
        .into_iter()
        .map(|(name, content)| {
            // ts-rs はフィールドと doc コメントの区切りに行末空白を出力する。
            // 再生成後も `git diff --check` を通すため、ここで行末空白を除去する。
            let content = if name.ends_with(".ts") {
                strip_trailing_whitespace(&content)
            } else {
                content
            };
            (name, content)
        })
        .collect())
}

/// foundation（yuuka-types）と各ドメインクレートの wire DTO をまとめて生成する。
/// 新ドメインを追加したらここに1行足す。
fn export_all_domains(base: &Path) -> Result<(), String> {
    let exporters = [
        ("yuuka-types", yuuka_types::export_all as fn(&Path) -> _),
        ("yuuka-todo", yuuka_todo::export_bindings),
        ("yuuka-finance", yuuka_finance::export_bindings),
        ("yuuka-schedule", yuuka_schedule::export_bindings),
        ("yuuka-timeline", yuuka_timeline::export_bindings),
        ("yuuka-reminder", yuuka_reminder::export_bindings),
        ("yuuka-personal", yuuka_personal::export_bindings),
        ("yuuka-credential", yuuka_credential::export_bindings),
        ("yuuka-playbook", yuuka_playbook::export_bindings),
        ("yuuka-persona", yuuka_persona::export_bindings),
    ];
    for (krate, export) in exporters {
        export(base).map_err(|e| format!("{krate}: {e}"))?;
    }
    Ok(())
}

fn strip_trailing_whitespace(source: &str) -> String {
    source
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// ディレクトリ直下のファイル名→内容のマップ（内容比較用）。ディレクトリが無ければ空。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_whitespace_is_stripped_and_a_final_newline_kept() {
        assert_eq!(strip_trailing_whitespace("a  \n b\t\n"), "a\n b\n");
        assert_eq!(strip_trailing_whitespace("a"), "a\n");
    }
}
