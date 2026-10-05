# scaffold — 実装開始時にコピーして使う設定テンプレート

これらは**まだ有効化されていない参照テンプレート**（`docs/rust-rewrite/scaffold/` 配下に置いてあるだけ）。
Rust 実装を開始するとき、新規 Cargo workspace のルートへコピーして使う。全て 2026-07-01 時点の
一次ソース（clippy 1.96 / cargo-deny 0.19.9 / crates.io）で検証済み（[../verification/](../verification/)）。

| テンプレート | 配置先 | 役割 |
|---|---|---|
| `Cargo.workspace.toml` | workspace ルート `Cargo.toml` | `[workspace.lints.clippy]` で握り潰し系 lint を deny（絶対制約1の機械強制） |
| `member-Cargo.toml.snippet` | 各メンバー crate の `Cargo.toml` | `[lints] workspace = true`（継承オプトイン。忘れると lint が効かない） |
| `deny.toml` | workspace ルート | cargo-deny `[bans]` で anyhow/eyre/color-eyre/backoff を依存禁止 |
| `clippy.toml` | workspace ルート | テストコードでの unwrap/expect/indexing 誤発火を緩和 |
| `rust-toolchain.toml` | workspace ルート | ツールチェーン固定 |
| `ci-checks.sh` | CI | clippy + cargo-deny ゲート |

## 重要な運用ルール（[../verification/rpt-clippy-cargodeny.md](../verification/rpt-clippy-cargodeny.md) 参照）
- `restriction` グループは**個別列挙必須**（一括 `clippy::restriction` 禁止＝相互矛盾 lint を含む）。
- 各メンバー crate に `[lints] workspace = true` を**必ず**書く（継承は自動でない）。
- cargo-deny の deny エントリは現行 `crate =` フィールド（旧 `name`/`version` は非推奨）。
- `panic = "unwind"` を維持（`abort` にすると自己復帰のパニック隔離が無効化。[../verification/rpt-resilience-tokio-backon-recloser.md](../verification/rpt-resilience-tokio-backon-recloser.md)）。
- CI では `cargo build` ではなく **`cargo clippy`** を回す（clippy lint はビルド時に評価されない）。
