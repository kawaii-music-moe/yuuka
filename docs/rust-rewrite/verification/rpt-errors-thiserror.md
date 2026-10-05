I now have all the verified data I need. Let me compile the research report in Japanese Markdown.

---

# thiserror エラー設計 調査レポート（2026-07-01 時点・一次ソース照合済み）

> 前提: 本プロジェクトでは `anyhow`/`eyre`/`Box<dyn Error>` による型消去("エラー握りつぶし")は**禁止**。`thiserror` スタイルの具象・列挙エラー型のみ許容。
> バージョン数値は全て crates.io JSON API（`https://crates.io/api/v1/crates/...`）で実測。

---

## 1. thiserror の現行メジャーバージョンと de-facto 標準性

- **推奨**: `thiserror` を **2.x 系** で採用。ライブラリのエラー型定義におけるデファクト標準は 2026 年も揺るがない（dtolnay クレート）。
- **バージョン**: **`2.0.18`**（`max_stable_version` = `newest_version` = `2.0.18`、公開日 **2026-01-18**）。**2.0 は既に出荷済み**であり、1.x ではない。MSRV = **Rust 1.68**、edition 2021。累計DL **約11.4億**、直近90日DL **約2.79億**。
- **根拠**:
  - crates.io API 実測（`max_version: 2.0.18`, `updated_at: 2026-01-18`）— https://crates.io/crates/thiserror
  - リポジトリは dtolnay 本人管理 — https://github.com/dtolnay/thiserror
  - 直近DL比較で `anyhow`(1.0.103, 直近1.65億) と並ぶ両輪だが、**具象型定義**用途では thiserror が標準。2026年の解説記事群も「library=thiserror / application=anyhow」を定説として繰り返している — https://oneuptime.com/blog/post/2026-01-25-error-types-thiserror-anyhow-rust/view
- **落とし穴**:
  - **1.x → 2.0 移行時の非互換**に注意。2.0 では `#[error("{x}")]` の**フィールド補間の解決順序**が変わり、`self.x`/`self.0` を暗黙参照する挙動が厳格化された。1.x 前提のコードをそのまま貼ると補間対象の解釈がずれる場合がある。新規なら 2.0.18 固定で問題なし。
  - `thiserror` は**手続きマクロのみ**を提供する軽量クレート。ランタイム機能（バックトレース収集の自動化等）は最小限、という設計思想を理解しておく。
- **確信度**: **高**（バージョンは API 実測、標準性は複数一次/二次ソースで一致）。

---

## 2. 階層化エラー enum のベストプラクティス（`#[from]` / `#[error]` / `#[source]`）

- **推奨**: 各層のエラーを具象 enum で定義し、下位層エラーを **`#[from]`** で自動合成（`From` impl 自動生成）＋ **`#[error("...")]`** で Display を付与。下位エラーを保持しつつメッセージを足すだけの場合は `#[source]`、メッセージを足さず素通しするなら `#[error(transparent)]`。
- **バージョン**: 構文は **thiserror 2.0.18 の docs.rs / README で確認**（docs は 2.0.18 対応）。
- **確認済みの正確な属性構文**（docs.rs/thiserror・GitHub README より）:
  - `#[error("{var}")]` → `write!("{}", self.var)` / `#[error("{0}")]` → `write!("{}", self.0)`。`:?` 指定で Debug 補間。任意の追加フォーマット引数・式も可。
  - **`#[from]`**: 付与したバリアントごとに **`From` impl を自動生成**。**そのバリアントはソースエラー(＋任意で backtrace)以外のフィールドを持ってはならない**という制約がある。
  - **`#[from]` は `#[source]` を暗黙に含む** → 両方書く必要はない（同一フィールドが自動的に `source()` になる）。
  - **`#[source]`**: 下位エラーを `Error::source()` として公開（`From` は生成しない）。追加コンテキストのフィールドを同居させたい場合はこちら。
  - **`#[error(transparent)]`**: Display と source を下位エラーへ素通し（メッセージを追加しない）。
  - **`#[backtrace]`**: source かつ backtrace 指定のフィールドで `provide()` を下位へ転送し、両層でバックトレースを共有（nightly / 対応 std 機能に依存）。
  - 例:
    ```rust
    #[derive(Debug, thiserror::Error)]
    pub enum RepoError {
        #[error("db query failed")]
        Db(#[from] sqlx::Error),          // From<sqlx::Error> 自動生成、source も自動
        #[error("record {id} not found")]
        NotFound { id: u64 },             // 追加コンテキスト
        #[error(transparent)]
        Other(#[from] std::io::Error),    // 素通し
    }
    ```
- **根拠**: https://docs.rs/thiserror/latest/thiserror/ および https://github.com/dtolnay/thiserror（README, "A `From` impl is generated for each variant that contains a `#[from]` attribute" / "the `#[from]` attribute always implies `#[source]`")。
- **落とし穴**:
  - **`#[from]` は 1 バリアント = 1 ソース型が実質前提**。同じ下位型（例: `std::io::Error`）を複数バリアントで `#[from]` すると `From` impl が衝突しコンパイル不能。素通しが複数必要なら片方は `#[source]` に落とすか、下位型を newtype で分ける。
  - `#[from]` を多用すると「どこでどの層のエラーが混入したか」の意味論が薄れる。**層をまたぐ変換は明示的にマッピングする**方が禁止方針（握りつぶし回避）と整合する。安易な `#[from]` チェーンは anyhow 的な"何でも吸い込む"型に近づくため、層境界では意図的に変換する設計を推奨。
  - `#[error(transparent)]` はメッセージを一切足さない＝**呼び出し側でコンテキストが失われる**。層の最下段以外での多用は避ける。
- **確信度**: **高**（docs.rs と GitHub README の二重確認、構文一致）。

---

## 3. `#[non_exhaustive]` — 公開エラー enum への付与

- **推奨**: **公開 API のエラー enum には原則 `#[non_exhaustive]` を付ける**。将来バリアントを追加しても下位クレートの `match` を壊さない（＝破壊的変更にならない）ため。内部専用エラーには不要。
- **バージョン**: これは **thiserror の機能ではなく標準 Rust の言語属性**（安定版・Rust Reference 記載）。thiserror 2.0.18 とは独立に、derive の上に併記して使う。
- **根拠**: https://doc.rust-lang.org/reference/attributes/type_system.html
  - enum に付けると、**定義クレート外の `match` はワイルドカード `_ =>` アームが必須**になる。既知の全バリアントを列挙してもコンパイラは網羅的と見なさない。
  - 従って**新バリアント追加が既存の下位コードを壊さない**（`_` で自動的に吸収される）。struct/variant に付けた場合はフィールド追加も非破壊（外部からの構築禁止・`..` 必須）。
  - **定義クレート内では効果なし**（自クレート内は網羅 match・構築とも従来どおり可能）。
  - 使い方:
    ```rust
    #[non_exhaustive]
    #[derive(Debug, thiserror::Error)]
    pub enum ApiError { /* ... */ }
    ```
- **落とし穴**:
  - **諸刃の剣**: 下位クレートに `_ =>` を強制するため、**HTTP マッピング（§4）を同一クレート内で書く**なら網羅 match が使えて好都合だが、別クレートで完全網羅マッピングをしたい消費側には制約になる。多クレート構成なら「どの crate 境界で `#[non_exhaustive]` を切るか」を設計時に決める。
  - モノレポ内・単一クレート内で完結するエラー（例: axum の `IntoResponse` を同じ crate で実装）なら**付けても自分の網羅 match は書ける**。公開ライブラリ境界のエラーにのみ付けるのが実務的。
  - thiserror の derive とは干渉しない（純粋に std 属性として作用）。
- **確信度**: **高**（Rust Reference 一次ソースで挙動確認）。

---

## 4. エラー → HTTP レスポンスのマッピング（axum `IntoResponse`）

- **推奨**: **型定義は thiserror、HTTP マッピングは手書きの `IntoResponse` 実装**、という役割分担が定石。エラー enum の各バリアントを `match` して `StatusCode`（＋ボディ）へ落とす。**thiserror 自体は HTTP マッピングを一切提供しない**（提供するのは Display/Error/From のみ）。
- **バージョン**: **axum `0.8.9`**（`max_version`、公開 2026-04-14）で確認。パターンは 0.7/0.8 系で共通。
- **根拠**: https://docs.rs/axum/latest/axum/response/index.html（"Anything that implements `IntoResponse` can be returned from a handler"）。イディオム例:
  ```rust
  impl axum::response::IntoResponse for ApiError {
      fn into_response(self) -> axum::response::Response {
          let (status, msg) = match self {
              ApiError::NotFound { .. }   => (StatusCode::NOT_FOUND, self.to_string()),
              ApiError::Db(_)             => (StatusCode::INTERNAL_SERVER_ERROR, "internal".into()),
              // ...
          };
          (status, msg).into_response()
      }
  }
  ```
  ハンドラは `Result<T, ApiError>` を返せば `?` で伝播でき、`ApiError` は `IntoResponse` 経由で HTTP 化される。
- **落とし穴**:
  - axum docs は明示的に警告している: **戻り値を `Result<impl IntoResponse, E>` にすると `?` の型推論が壊れやすい**。戻り値は具象化（`Result<Json<T>, ApiError>` 等）するのが安全。
  - **内部エラーの Display をそのままクライアントに返さない**。`ApiError::Db(_)` のような機微を含むバリアントは `"internal"` 等に丸め、`self.to_string()` を露出させるのは安全なバリアントのみ。
  - `match self` で網羅する都合上、**エラー enum が別クレートで `#[non_exhaustive]` だと `_ =>` が強制される**（§3）。マッピングを行うクレートとエラー定義クレートの関係を意識。同一クレートに置けば完全網羅 match が書け、バリアント追加時にコンパイルエラーで漏れを検知できる（推奨）。
- **確信度**: **高**（axum バージョンは API 実測、パターンは公式 docs と広範なイディオムで一致）。

---

## 5. 代替評価: snafu vs thiserror（2026）＋ 新興クレート

### snafu

- **推奨**: **既定は thiserror。snafu は「コンテキストセレクタによる意味論的バックトレース」や `Location` 追跡を全面採用したい大規模アプリでのみ検討**。本プロジェクトの方針（具象・列挙型）とは両立するが、標準化・エコシステム親和性から thiserror を第一候補とする。
- **バージョン**: **`0.9.1`**（`max_version`、公開 **2026-05-29**。直前は 0.9.0=2026-03、0.8.9=2025-09）。**活発に維持されている**（2026年に2リリース）。maintainer = shepmaster、リポジトリ https://github.com/shepmaster/snafu。累計DL 約9,400万・直近約1,210万（thiserror の 1/20 規模）。
- **snafu が追加するもの**（thiserror に対して）:
  - **Context selectors**: エラー生成箇所ごとに小さく具体的な型を強制し、"file not found while finding Y while reconciling X" のような**意味論的バックトレース**を得る。thiserror の粗い単一 `Error` 型より追跡が容易。
  - **`snafu::Location`**: 各エラー構築地点のコード位置を自動収集し、伝播経路を追える。
  - **backtrace / futures 統合**をフィーチャで提供。
- **根拠**:
  - バージョン・維持状況: crates.io API 実測 — https://crates.io/crates/snafu / https://docs.rs/snafu/latest/snafu/guide/index.html（0.9.1 対応、"ergonomic error handling library"）
  - 機能比較: https://github.com/kube-rs/kube/discussions/453（context selectors の"意味論的バックトレース"論）、https://dev.to/leapcell/rust-error-handling-compared-anyhow-vs-thiserror-vs-snafu-2003
- **snafu を選ぶ理由になり得るか**: **深いエラー連鎖のデバッグ・大規模モジュラー設計で位置追跡が要件**なら合理的。ただし学習コスト・ボイラープレート（selector 型）増、エコシステム事例の少なさがトレードオフ。**HTTP マッピング（§4）は snafu でも自前実装が必要**で thiserror に対する優位はない。→ 本プロジェクトでは**採用しない前提で妥当**。
- **落とし穴**: context selector は独自の生成型（`XxxSnafu`）を大量に生むため、`?` 周辺の記法・型が thiserror と大きく異なる。**チーム全体の慣れ**が前提。0.8→0.9 で API 変更もあるため固定バージョン運用を。
- **確信度**: **高**（バージョン・維持は API 実測、機能は複数ソース一致）。

### 新興クレート（2025/2026）

- **error-stack**: thiserror の代替兼拡張。**任意の attachment 付きエラースタック**（生成・伝播情報）を追加。**バージョン `0.7.1`（公開 2026-05-27、0.7.0=2026-03）で活発に維持**されているが、**直近DL 約71万**と thiserror の桁違いに小さくニッチ。thiserror を"置き換える"標準勢力にはなっていない。— https://crates.io/crates/error-stack / https://nrc.github.io/error-docs/ecosystem.html
- **std への吸収傾向**: 2025 年に anyhow 相当の機能の多くが `std` に取り込まれつつある（Report/backtrace 系）。ただしこれは**握りつぶし系（anyhow/eyre）の代替**であり、**具象・列挙型を作る thiserror の代替ではない**。本プロジェクト方針では無関係。
- **結論**: **2026 年時点で thiserror を脅かす新標準は出ていない**。error-stack は"補完・拡張"の位置付けで、採用理由は「attachment 付きの豊かなエラースタックが欲しい」場合に限られる。
- **根拠**: https://markaicode.com/rust-error-handling-2025-guide/ , https://lib.rs/crates/anyhow
- **確信度**: **中〜高**（error-stack の数値は API 実測=高、"新標準は不在"という判断は網羅性の性質上=中）。

---

## 総括（プロジェクト方針への適合）

| 項目 | 結論 | バージョン | 確信度 |
|---|---|---|---|
| 1. 標準性 | thiserror が 2026 も de-facto、2.0 出荷済み | **2.0.18** (2026-01-18) | 高 |
| 2. 属性構文 | `#[from]`(From自動+source暗黙) / `#[source]` / `#[error]` / `transparent` | 2.0.18 | 高 |
| 3. `#[non_exhaustive]` | 公開エラー enum に付与推奨、std 言語属性 | std(安定) | 高 |
| 4. HTTP マッピング | 型=thiserror／`IntoResponse`=手書き。thiserror はHTTP非提供 | axum **0.8.9** | 高 |
| 5. snafu / 新興 | 既定 thiserror、snafu(**0.9.1**)は特定要件のみ、新標準は不在 | snafu 0.9.1 / error-stack 0.7.1 | 高 |

方針（anyhow/eyre/Box<dyn Error> 禁止・具象列挙型のみ）とは **thiserror 2.0.18 + std `#[non_exhaustive]` + 手書き axum `IntoResponse`** の組み合わせが完全に整合。層境界では `#[from]` の乱用を避け明示変換することで"握りつぶし"を構造的に排除できる。

### 参照した一次ソース
- crates.io API 実測: [thiserror](https://crates.io/crates/thiserror) / [snafu](https://crates.io/crates/snafu) / [error-stack](https://crates.io/crates/error-stack) / [axum](https://crates.io/crates/axum)
- [docs.rs/thiserror](https://docs.rs/thiserror/latest/thiserror/) / [github.com/dtolnay/thiserror](https://github.com/dtolnay/thiserror)
- [docs.rs/snafu guide](https://docs.rs/snafu/latest/snafu/guide/index.html) / [github.com/shepmaster/snafu](https://github.com/shepmaster/snafu)
- [docs.rs/axum response](https://docs.rs/axum/latest/axum/response/index.html)
- [Rust Reference: non_exhaustive](https://doc.rust-lang.org/reference/attributes/type_system.html)

### 参考（二次ソース）
- [oneuptime: thiserror/anyhow 2026](https://oneuptime.com/blog/post/2026-01-25-error-types-thiserror-anyhow-rust/view)
- [kube-rs discussion #453 (thiserror vs snafu)](https://github.com/kube-rs/kube/discussions/453)
- [dev.to: anyhow vs thiserror vs snafu](https://dev.to/leapcell/rust-error-handling-compared-anyhow-vs-thiserror-vs-snafu-2003)
- [nrc error-docs ecosystem](https://nrc.github.io/error-docs/ecosystem.html) / [markaicode 2025 guide](https://markaicode.com/rust-error-handling-2025-guide/)

---

**主要な検証結果（要点）**: thiserror は **2.0.18**（2.0 系は出荷済み、1.x ではない）が最新でデファクト標準。snafu も **0.9.1**（2026-05）で活発維持だが規模は 1/20、特定要件のみ推奨。axum は **0.8.9**。`#[non_exhaustive]` は thiserror ではなく std 言語属性。HTTP マッピングは thiserror が提供せず `IntoResponse` の手書きが定石。2026 年に thiserror を脅かす新標準クレートは確認されず（error-stack 0.7.1 は補完的ニッチ）。全バージョンは crates.io JSON API で実測済み。

**注記**: このレポートはファイルに書き出していません（指示どおり）。プロジェクトの実装計画に組み込む場合は、上記の一次ソース URL をそのまま参照してください。