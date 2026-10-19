# yuuka 開発ルール

## ビルドと検証

- 開発中の確認は `cargo check`（必要に応じて `cargo clippy` / `cargo fmt`）だけで行う。
- `cargo build` / `cargo test` / `cargo run` / `cargo nextest` などの実ビルドは、`develop` ブランチでの検証時以外は実行しない。
  - 理由: 複数の worktree で並列にビルドすると、開発サーバ（RAM 15GB）のメモリが枯渇してクラッシュするため。
  - `.claude/hooks/guard-cargo-build.sh` が、`develop` 以外のブランチでのこれらのコマンドをブロックする。
