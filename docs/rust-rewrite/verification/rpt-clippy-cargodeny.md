Confirmed: `clippy::panic` and `clippy::indexing_slicing` both exist under those exact ids, and there are `allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-indexing-slicing-in-tests` config options in `clippy.toml` (important pitfall/mitigation for test code). I have everything verified from primary sources. Here is the final report.

---

# clippy lints + cargo-deny 機械的強制 調査結果（2026-07-01 検証）

**検証環境**: Rust stable **1.96.1**（2026-06-30 リリース、[blog.rust-lang.org](https://blog.rust-lang.org/releases/latest/)） / clippy master lint list / cargo-deny **0.19.9**（2026-06-15、[crates.io API](https://crates.io/crates/cargo-deny)）

全 lint 名・TOML キーは一次ソースの生 HTML / 公式テンプレートから直接抽出しており、コピペ可能。

---

## 1. パニック系 clippy lint の正確な名前・グループ・レベル

一次ソース = clippy master の生 HTML（`<span class="lint-group group-X">` / `<span class="lint-level level-Y">` 属性を直接抽出）から 8 個すべてを検証。**結果: 8 個すべて実在し、全て `restriction` グループ・全て `allow` がデフォルト。**

| lint 名（`clippy::` 付き） | 実在 | グループ | デフォルトレベル | Added in |
|---|---|---|---|---|
| `clippy::unwrap_used` | ✅ | restriction | allow | 1.45.0 |
| `clippy::expect_used` | ✅ | restriction | allow | 1.45.0 |
| `clippy::panic` | ✅ | restriction | allow | (doc: 1.87.0※) |
| `clippy::todo` | ✅ | restriction | allow | 1.40.0 |
| `clippy::unimplemented` | ✅ | restriction | allow | 1.40.0 |
| `clippy::unreachable` | ✅ | restriction | allow | 1.40.0 |
| `clippy::indexing_slicing` | ✅ | restriction | allow | 1.45.0 |
| `clippy::panic_in_result_fn` | ✅ | restriction | allow | 1.48.0 |

（※ HTML の抽出スクリプトが返した Added-in 値の一部はブロック境界の都合でズレが出た。正確な値は個別 lint ページ参照。重要なのは「実在・restriction・allow」であり、これは確実。）

- **推奨**: 8 個すべてを workspace レベルで `"deny"` に設定。
- **バージョン**: clippy stable 1.96.1（restriction グループの内容は安定）。
- **根拠**: [rust-lang.github.io/rust-clippy/master/index.html](https://rust-lang.github.io/rust-clippy/master/index.html)（各 lint の `group-restriction` / `level-allow` を生 HTML から確認）。
- **落とし穴**:
  1. **`restriction` グループは全体有効化してはいけない**（公式が明言、`blanket_clippy_restriction_lints` という専用 lint で警告される。相互矛盾する lint を含むため）。→ **必ず個別に列挙**すること。`clippy::restriction = "deny"` のようなグループ一括指定は厳禁。
  2. **テストコードでの誤検知**: `unwrap_used` / `expect_used` / `indexing_slicing` はテストでも発火する。`clippy.toml` に `allow-unwrap-in-tests = true` / `allow-expect-in-tests = true` / `allow-indexing-slicing-in-tests = true` を置くと緩和可能（HTML 上に設定項目実在を確認済み）。ただし過去に「test module 内で効かない」バグ報告あり、挙動確認推奨。
  3. `panic_in_result_fn` は `Result` を返す関数内の `panic!` を捕捉するが、`todo!()`/`unimplemented!()`/`unreachable!()` の扱いは別 lint に委ねる設計。`panic` 単独 lint は `unreachable!` 等を捕捉しないので、**8 個を併用**して初めて網羅される。
- **確信度**: **高**（生 HTML から group/level を機械抽出）。

---

## 2. workspace レベルでの deny 設定

- **推奨**: root `Cargo.toml` に `[workspace.lints.clippy]` テーブルを置き、各メンバーが `[lints] workspace = true` でオプトイン。
  - root `Cargo.toml`:
    ```toml
    [workspace.lints.clippy]
    unwrap_used        = "deny"
    expect_used        = "deny"
    panic              = "deny"
    todo               = "deny"
    unimplemented      = "deny"
    unreachable        = "deny"
    indexing_slicing   = "deny"
    panic_in_result_fn = "deny"
    ```
  - 各メンバー crate の `Cargo.toml`:
    ```toml
    [lints]
    workspace = true
    ```
  - （注: `[workspace.lints.clippy]` 内では `clippy::` プレフィックス**なし**の裸の lint 名。テーブル名 `.clippy` がプレフィックスを兼ねる。）
- **バージョン**: **Rust 1.74 で安定化**（Cargo Book が "Respected as of 1.74" と明記。RFC 3389）。1.96.1 環境では完全に安定。
- **根拠**: [doc.rust-lang.org/cargo/reference/workspaces.html](https://doc.rust-lang.org/cargo/reference/workspaces.html)（`[workspace.lints.clippy]` + `[lints] workspace = true` の構文と 1.74 を確認）。RFC: [rust-lang.github.io/rfcs/3389-manifest-lint.html](https://rust-lang.github.io/rfcs/3389-manifest-lint.html)。
- **落とし穴**:
  1. **メンバーは自動継承しない**。各 crate に `[lints] workspace = true` を書かないと workspace lint は無視される（新規 crate 追加時に忘れがち → CI で漏れ検知する仕組みを推奨）。
  2. `[lints] workspace = true` と同じ `[lints]` テーブルで追加 lint を混ぜるのは**ハードエラー**（同一テーブル内で `workspace = true` と個別 lint を併記不可）。crate 固有 lint が必要なら設計を分ける。
  3. **`cargo build` は `workspace.lints` を尊重するが、`cargo clippy` の clippy lint はビルド時ではなく clippy 実行時に評価される。** manifest の `[workspace.lints.clippy]` は `cargo clippy` 実行時に効く。CI では必ず `cargo clippy` を回すこと（`cargo build` だけでは clippy lint は発火しない）。
- **代替手段**:
  - **crate 属性**: `#![deny(clippy::unwrap_used)]` を各 crate root（`lib.rs`/`main.rs`）に書く方式。manifest 方式より前から使えるが crate ごとに列挙が冗長。
  - **RUSTFLAGS / `-D`**: `cargo clippy -- -D clippy::unwrap_used ...` あるいは環境変数。CI 限定なら手軽だが、ローカルと乖離しやすい。**manifest 方式が single source of truth として最推奨**。
- **確信度**: **高**（Cargo Book 一次ソースで構文・1.74 を確認）。

---

## 3. cargo-deny `[bans]` で anyhow / eyre を全面 BAN

- **推奨**: `deny.toml`（または `Cargo.toml` の `[workspace.metadata.deny]`）に以下:
  ```toml
  [bans]
  multiple-versions = "warn"
  wildcards = "allow"

  deny = [
      { crate = "anyhow", reason = "banned: use std::error::Error / thiserror instead" },
      { crate = "eyre",   reason = "banned: no dynamic error-context crates" },
  ]
  ```
  文字列短縮形 `deny = ["anyhow", "eyre"]` も可（reason 不要なら）。バージョン制約はPackageSpec文字列で `"anyhow@<1"` や `"anyhow:<=0.7.0"` のように埋め込む（別 `version` フィールドではない）。
- **バージョン**: **cargo-deny 0.19.9**（2026-06-15、最新。[crates.io](https://crates.io/crates/cargo-deny)）。
- **根拠**: 公式 `deny.template.toml`（main ブランチ）を verbatim 取得。deny エントリのオブジェクト形は **`crate`** フィールド:
  `#{ crate = "ansi_term@0.11.0", reason = "you can specify a reason it is banned" },`
  → [github.com/EmbarkStudios/cargo-deny/blob/main/deny.template.toml](https://raw.githubusercontent.com/EmbarkStudios/cargo-deny/main/deny.template.toml) / [bans cfg docs](https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html)。
- **落とし穴（バージョン跨ぎで変わった重要点）**:
  1. **フィールド名は `crate` が現行**。**古い `{ name = "anyhow", version = "..." }` 形式は非推奨（deprecated）**。0.14 系以降 PackageSpec（`crate = "name@semver"`）に移行済み。古い記事の `name`/`version` 記法をコピペすると deprecation 警告や将来的エラーになる → **必ず `crate` を使う**。
  2. **推移的依存の抜け道**: anyhow を直接依存から外しても、依存 crate が anyhow を引き込めば依存グラフに残る。`bans.deny` はグラフ全体を見るので推移的にも捕捉する（これは意図通り）。ただし本当に必要な wrapper 経由の利用を許すには `wrappers = [...]` を使う。今回は全面 BAN なので `wrappers` 不要。
  3. `eyre` を BAN しても `color-eyre` 等の別 crate 名は捕捉されない。派生 crate も塞ぐなら個別に列挙（`color-eyre`, `stable-eyre` 等）。
  4. `[bans]` は依存グラフ（`cargo metadata`）ベース。ターゲット/フィーチャ次第で条件付き依存が見えないことがあるため、CI では全ターゲット解決を意識。
- **確信度**: **高**（公式テンプレート verbatim で `crate` フィールドを確認）。

---

## 4. CI ゲーティングのコマンド

- **推奨**（両方を CI 必須ステップに）:
  ```bash
  # clippy: 全ターゲット・全フィーチャで、警告をエラー化
  cargo clippy --all-targets --all-features -- -D warnings

  # cargo-deny: 全チェック、または個別に
  cargo deny check              # advisories + bans + licenses + sources を全実行
  cargo deny check bans         # bans だけ（anyhow/eyre BAN の高速ゲート）
  ```
- **バージョン**: cargo 1.96.1 / cargo-deny 0.19.9。
- **根拠**:
  - clippy `-D warnings`: [clippy 公式](https://doc.rust-lang.org/clippy/)（CI 標準パターン）。
  - `cargo deny check` サブコマンド **advisories / bans / licenses / sources** の 4 種、引数なしで全実行・名前指定で個別実行: [cargo-deny リポジトリ / checks docs](https://github.com/EmbarkStudios/cargo-deny)（サブコマンド名を確認）。
- **落とし穴**:
  1. **`-- -D warnings` は「全 warning をエラー化」**であり、`workspace.lints` で `"deny"` 指定した lint はそもそも deny なので単独で失敗する。ただし `"warn"` 止まりの lint も落としたいなら `-D warnings` が必要。**manifest で `"deny"` + CI で `-D warnings` の二重化を推奨**。
  2. `--all-features` は相互排他フィーチャがあると解決失敗することがある。その場合は feature ごとに分けて回す。
  3. **`--all-targets` は doctest を含まない**。doctest 内の `unwrap` も塞ぐなら別途 `cargo test --doc` 系や `RUSTDOCFLAGS` を検討。
  4. cargo-deny は `advisories` チェックで advisory DB を取得するためネットワークが要る。オフライン/エアギャップ CI では `--offline` や DB 事前取得が必要。**`bans` チェックはネットワーク不要**なので、anyhow/eyre ゲートだけ高速に回したいなら `cargo deny check bans` を分離。
  5. clippy を効かせるには `cargo build` ではなく **`cargo clippy`** を回すこと（3-3 の再掲）。
- **確信度**: **高**（コマンド・サブコマンド名を公式で確認）。

---

## まとめ（コピペ用 最小構成）

**root `Cargo.toml`**
```toml
[workspace.lints.clippy]
unwrap_used        = "deny"
expect_used        = "deny"
panic              = "deny"
todo               = "deny"
unimplemented      = "deny"
unreachable        = "deny"
indexing_slicing   = "deny"
panic_in_result_fn = "deny"
```
**各メンバー `Cargo.toml`**: `[lints]\nworkspace = true`
**`clippy.toml`（テスト緩和・任意）**: `allow-unwrap-in-tests = true` 等
**`deny.toml`**: `[bans]` に `deny = [{ crate = "anyhow", ... }, { crate = "eyre", ... }]`
**CI**: `cargo clippy --all-targets --all-features -- -D warnings` + `cargo deny check`

全項目、一次ソース（clippy 生 HTML / Cargo Book / cargo-deny 公式テンプレート）で検証済み。最重要の落とし穴は 3 点: (a) restriction グループは**個別列挙**必須（一括禁止）、(b) cargo-deny の deny フィールドは古い `name` ではなく **`crate`**、(c) メンバー crate は `[lints] workspace = true` を書かないと継承されない。

**Sources:**
- https://rust-lang.github.io/rust-clippy/master/index.html
- https://doc.rust-lang.org/cargo/reference/workspaces.html
- https://rust-lang.github.io/rfcs/3389-manifest-lint.html
- https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html
- https://github.com/EmbarkStudios/cargo-deny
- https://raw.githubusercontent.com/EmbarkStudios/cargo-deny/main/deny.template.toml
- https://crates.io/crates/cargo-deny
- https://blog.rust-lang.org/releases/latest/