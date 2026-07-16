//! `migrations/` の SQL ファイル変更で再コンパイルを強制する。
//!
//! `refinery::embed_migrations!("migrations")` はマクロ展開時に `migrations/` を読むが、
//! Cargo は `.sql` を rustc の入力として追跡しないため、**新しいマイグレーションファイルを
//! 追加しても yuuka-db が再コンパイルされず**、古い埋め込み（そのマイグレーション欠落）の
//! バイナリが使われ続けることがある（インクリメンタルビルドのステイル）。
//! ここで `rerun-if-changed` をディレクトリと各ファイルへ出すことで、追加・編集を確実に検知する。

use std::fs;

fn main() {
    // ディレクトリ自体（ファイル追加/削除を検知）。
    println!("cargo:rerun-if-changed=migrations");
    // 各ファイル（内容編集を検知）。
    if let Ok(entries) = fs::read_dir("migrations") {
        for entry in entries.flatten() {
            if let Some(path) = entry.path().to_str() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
}
