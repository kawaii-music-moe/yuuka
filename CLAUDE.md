# yuuka 開発ルール

## ビルドと検証

- 開発中の確認は `cargo check`（必要に応じて `cargo clippy` / `cargo fmt`）だけで行う。
- `cargo build` / `cargo test` / `cargo run` / `cargo nextest` などの実ビルドは、`develop` ブランチでの検証時以外は実行しない。
  - 理由: 複数の worktree で並列にビルドすると、開発サーバ（RAM 15GB）のメモリが枯渇してクラッシュするため。
  - `.claude/hooks/guard-cargo-build.sh` が、`develop` 以外のブランチでのこれらのコマンドをブロックする。

## PR レビュー

ユーザーが「PR レビュー」と言ったら、次の流れで進める。

1. 対象 PR の差分・説明・CI 結果を確認してレビューする。
2. 問題があれば PR にレビューコメントとして指摘を追記する（マージはしない）。
3. 問題がなければ PR をマージする。
