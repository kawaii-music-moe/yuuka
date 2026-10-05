I have enough verified primary-source data to compile the report. Key finding on tower-http compatibility: since tower-http exposes public types through axum (ServeDir is used with axum's Router via ServiceExt), and axum 0.8.9 pins `tower-http = ^0.6.8`, the safe/idiomatic pairing is tower-http 0.6.x. tower-http 0.7.0 (released 2026-06-15) is very recent; whether it's compatible depends on whether it broke public `http`/`tower` types. I'll flag this clearly as a pitfall. Now compiling the final report.

---

# axum web runtime 技術選定リサーチ (2026-07-01 時点)

検証は crates.io API・docs.rs・GitHub の一次情報に基づく。バージョンは推測せず crates.io/docs.rs で確認済み。

## 総括表

| 項目 | 推奨 | 検証バージョン | 確信度 |
|---|---|---|---|
| 1. axum 本体 / tower・hyper 関係 | axum 0.8 系採用 | axum **0.8.9** / hyper **^1.1.0** / tower **^0.5.2** | 高 |
| 2. WebSocket (in-tree) | `axum::extract::ws` 採用 (`ws` feature) | axum **0.8.9** | 高 |
| 3. ServeDir precompressed | `.precompressed_br()` / `.precompressed_gzip()` | tower-http **0.6.8**(axum 0.8.9 が pin) | 高 |
| 4. CSP 等ヘッダ付与 | `SetResponseHeaderLayer` + `CompressionLayer` | tower-http **0.6.8** | 高 |
| 5. 型強制 auth extractor | `FromRequestParts` 実装パターン | axum **0.8.9** | 高 |

---

## 1. axum 現行メジャーバージョンと tower / hyper との関係

**推奨**: axum **0.8 系**を採用。まだ 1.0 未到達で、最新安定版は 0.8.9。

**バージョン (検証済み)**:
- axum = **0.8.9**(2026-04-14 リリース。crates.io / docs.rs 双方で確認)
- 依存: **hyper `^1.1.0`**(hyper 1.x 系)、**tower `^0.5.2`**、tower-layer `^0.3.2`、tower-service `^0.3`、tower-http `^0.6.8`(optional)

**根拠**:
- axum 0.8.9 の Cargo 依存関係(docs.rs crate メタデータ): https://docs.rs/crate/axum/latest — hyper `^1.1.0`, tower `^0.5.2`, tower-http `^0.6.8` を明記。
- crates.io パッケージページ: https://crates.io/crates/axum
- axum 0.8.0 アナウンス: https://tokio.rs/blog/2025-01-01-announcing-axum-0-8-0
- axum は hyper の薄いラッパーであり、独自ミドルウェア機構を持たず `tower::Service` を採用する設計(公式 README): https://github.com/tokio-rs/axum

**落とし穴**:
- **0.x であるため semver 上、マイナー更新 (0.7→0.8) で破壊的変更が入る**。実際 0.8 ではパスパラメータ構文が `/:id` → `/{id}` へ変更されるなど破壊的変更があった。バージョン固定 (`=0.8` ではなく `^0.8` でメジャー相当のマイナーに追随) の運用ポリシーを決めること。1.0 到達までは「マイナー = メジャー」と見なす。
- hyper は既に 1.x(安定 API)だが、axum 自体が 0.x なので、axum を跨ぐ破壊的変更のほうが実務リスクが高い。
- GitHub main ブランチは次期 0.9 に向けた破壊的変更を蓄積中。crates.io リリース済みは v0.8.x ブランチ相当。

**確信度**: 高

---

## 2. ビルトイン WebSocket サポート (`axum::extract::ws`)

**推奨**: `axum::extract::ws` を採用。**別クレート不要**、axum 本体の in-tree モジュール。

**バージョン (検証済み)**: axum **0.8.9**。**`ws` feature フラグが必須**("Available on crate feature `ws` only")。

**根拠**:
- docs.rs 一次情報: https://docs.rs/axum/latest/axum/extract/ws/index.html — `axum::extract::ws` が axum クレート内モジュールであることを確認。
- 主要型: **`WebSocketUpgrade`**(アップグレード用エクストラクタ)、**`WebSocket`**(メッセージストリーム)、**`Message`**(メッセージ enum)、`CloseFrame`、`Utf8Bytes`、`OnFailedUpgrade` トレイト。
- echo ハンドラ、状態受け渡し、futures による並行 read/write の実例が同ドキュメントに掲載。

**落とし穴**:
- `Cargo.toml` の axum features に `ws` を明示的に追加しないとコンパイルエラー(デフォルト無効)。
- WebSocket ハンドラはボディを消費するため、他のボディ消費エクストラクタと併用不可。`WebSocketUpgrade` はハンドラ引数の最後に置く必要がある。
- axum 0.8 で `Message` enum のバリアント/API に細かい変更履歴があるため、旧サンプル (0.6/0.7) のコピペは避ける。

**確信度**: 高

---

## 3. tower-http `ServeDir` による静的配信 + プリコンプレス

**推奨**: `ServeDir::new(...).precompressed_br().precompressed_gzip()` を採用。`.br` / `.gz` サイドカーファイルを Accept-Encoding に応じて配信。

**バージョン (検証済み)**: axum 0.8.9 が pin する **tower-http `^0.6.8`** を使用。tower-http 最新は **0.7.0**(2026-06-15、crates.io API・docs.rs 双方で確認)だが後述の互換注意あり。

**根拠 (メソッド名確認済み)** — docs.rs 一次情報 https://docs.rs/tower-http/latest/tower_http/services/struct.ServeDir.html :
- **`precompressed_br()`** → `dir/foo.txt.br`(Brotli)
- **`precompressed_gzip()`** → `dir/foo.txt.gz`(gzip)
- `precompressed_zstd()` → `.zst`
- `precompressed_deflate()` → `.zz`
- 動作: クライアントの `Accept-Encoding` が該当エンコーディングを許可していればサイドカー(例 `foo.txt.gz`)を返し、無ければ非圧縮版へフォールバック。複数バリアントの併用可(br + gzip 同時指定可能)。

**落とし穴**:
- **tower-http 0.7.0 と axum 0.8.9 の semver 不整合**: axum 0.8.9 は `tower-http ^0.6.8` に依存。`ServeDir` を axum の `Router` に組み込む際、両者が公開する `http` / `tower` 型が同一メジャーである必要がある。実務では **tower-http 0.6.x(最新 0.6.11)を選ぶのが安全**。0.7.0 は 2026-06-15 リリースと非常に新しく、axum 0.8 側の追随(0.7 対応版 axum リリース)を確認するまで採用を待つこと。0.7 系は圧縮のワイルドカード/identity 処理変更・`tokio`/`async-compression` の暗黙 feature 削除など破壊的変更を含む。
  - 参照: https://github.com/tower-rs/tower-http/blob/main/tower-http/CHANGELOG.md
  - 過去に同型問題あり: https://github.com/tokio-rs/axum/issues/2416(ServeDir が axum とバージョン不整合で非互換になった事例)
- サイドカーファイルはビルド時に事前生成が前提(`ServeDir` は動的圧縮しない)。動的圧縮は #4 の `CompressionLayer` の役割で、両者は別物。
- feature フラグ: tower-http の `fs`(ServeDir)+ `compression-br` / `compression-gzip` 等を有効化する必要がある。

**確信度**: 高(メソッド名・動作)/ tower-http 0.7 の axum 0.8 互換性のみ中(未検証のため 0.6 系推奨)

---

## 4. レスポンスヘッダ (CSP 等) の付与

**推奨**:
- 固定ヘッダ(CSP, X-Content-Type-Options 等)の付与 → **`tower_http::set_header::SetResponseHeaderLayer`**
- レスポンスボディの動的圧縮 → **`tower_http::compression::CompressionLayer`**
- (両者は目的が異なる。CSP は `SetResponseHeaderLayer`、圧縮は `CompressionLayer`。)

**バージョン (検証済み)**: tower-http **0.6.8**(axum 0.8.9 pin)。型名は 0.6/0.7 双方に存在。

**根拠 (型名確認済み)**:
- `SetResponseHeaderLayer<M>` は `tower_http::set_header` に存在: https://docs.rs/tower-http/latest/tower_http/set_header/struct.SetResponseHeaderLayer.html
  - コンストラクタ 3 種:
    - **`overriding()`** — 既存の同名ヘッダを削除して置換(CSP に推奨)
    - **`appending()`** — 既存値を保持しつつ追加(複数値化)
    - **`if_not_present()`** — 既存値がある場合は挿入しない
  - `HeaderName` と値メーカ `M` を取る。`Content-Security-Policy` の付与は `overriding()` が定石。
- `CompressionLayer` は `tower_http::compression` に存在: https://docs.rs/tower-http/latest/tower_http/compression/struct.CompressionLayer.html
  - Accept-Encoding を見て gzip / deflate / brotli / zstd を選択し `Content-Encoding` を付与。既に `content-encoding` があるレスポンスは再圧縮しない。

**落とし穴**:
- **`SetResponseHeaderLayer` と `CompressionLayer` は責務が別**。CSP のような「静的ヘッダ」を `CompressionLayer` で付けることはできない。CSP = `SetResponseHeaderLayer::overriding`、圧縮 = `CompressionLayer` と使い分ける。
- レイヤ適用順序に注意。`ServiceBuilder` / `Router::layer` の重ね順で内側/外側が決まる(tower のレイヤは後入れが外側)。圧縮後にヘッダを触る必要がある場合は順序を検証すること。
- `#3` の `ServeDir` プリコンプレスと `CompressionLayer` を併用すると二重圧縮の懸念。プリコンプレス済みレスポンスは `content-encoding` を持つため `CompressionLayer` は再圧縮を回避するが、経路構成は要確認。
- 該当 feature(`set-header`, `compression-full` 等)の有効化が必要。

**確信度**: 高

---

## 5. 型で強制する認可レベル (none/user/admin) — Extractor パターン

**推奨**: カスタム型に **`FromRequestParts`** を実装し、ハンドラ引数に置くことで型レベルで認可を強制する。これが axum 0.8 の**イディオマティックなパターン**。

**バージョン (検証済み)**: axum **0.8.9**。トレイト名は 0.8 でも **`FromRequestParts`**(健在)。

**根拠**:
- docs.rs 一次情報: https://docs.rs/axum/latest/axum/extract/trait.FromRequestParts.html
  - シグネチャ(0.8、`#[async_trait]` 不要 = RPITIT 化):
    ```rust
    fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send
    ```
  - **`FromRequestParts` vs `FromRequest`**: `FromRequestParts` はボディを消費せず順序自由(ヘッダ/拡張/メタデータのみ参照)。`FromRequest` はボディを消費する場合に使い、ハンドラ引数の最後に 1 個だけ。認可はヘッダ(`Authorization`)や拡張参照が主なので **`FromRequestParts` が自然な選択**。
- axum 0.8 の変更点: **`#[async_trait]` マクロ不要**に(RPITIT により)。旧 0.6/0.7 の `async_trait` 付きサンプルは 0.8 では書き換えが必要。
  - https://tokio.rs/blog/2025-01-01-announcing-axum-0-8-0
- 認可レベルを型で表現する具体例: `AuthenticatedUser` / `AdminUser` などの型を用意し、それぞれ `FromRequestParts` を実装。ハンドラが `admin: AdminUser` を引数に取れば、その型抽出に失敗する(=権限不足)リクエストは自動で拒否 → コンパイル時に「このハンドラは admin 必須」が可視化される。
  - 実例参照: https://mattrighetti.com/2025/05/03/authentication-with-axum
- **任意認可 (Option)**: axum 0.8 では `OptionalFromRequestParts` により `Option<AuthUser>` パターンを実装可能(認証あり/なし両対応のエンドポイント向け)。none/user/admin の三段階設計で「user 任意」を表現するのに有用。

**落とし穴**:
- 0.6/0.7 の `#[async_trait]` 付き実装例を流用すると 0.8 でビルド不可。0.8 は RPITIT 前提で書く。
- `S`(state)ジェネリクスの扱いに注意。DB プールや設定を state 経由で取得する場合、`S: Send + Sync` 境界と `FromRef` の設定が必要。state アクセスの定番は Discussion #1732 参照: https://github.com/tokio-rs/axum/discussions/1732
- `Rejection` 型を適切に設計しないと、認可失敗が汎用 500 になり得る。401/403 を返す `IntoResponse` 実装を明示すること。
- 型強制は「そのハンドラに到達する条件」を保証するが、複数権限の OR/AND 合成はエクストラクタの組み合わせでは表現しにくい。複雑なポリシーはミドルウェア併用を検討。

**確信度**: 高

---

## 実務上の最重要注意点(再掲)

1. **axum は 0.x**(0.8.9)。1.0 未到達のため **マイナー更新に破壊的変更が入る**前提でバージョン管理・移行計画を立てること。
2. **tower-http は 0.6.x で固定推奨**。0.7.0 は 2026-06-15 リリースと新しく、axum 0.8.9 は `tower-http ^0.6.8` を pin しているため、`ServeDir` を axum の `Router` に組む際の型互換を確認するまで **0.6 系(最新 0.6.11)を採用**。
3. hyper は **1.x(安定)**で問題なし。実務リスクの主因は axum/tower-http の 0.x semver。

## 主要ソース

- https://crates.io/crates/axum / https://docs.rs/crate/axum/latest
- https://docs.rs/axum/latest/axum/extract/ws/index.html
- https://docs.rs/axum/latest/axum/extract/trait.FromRequestParts.html
- https://docs.rs/tower-http/latest/tower_http/services/struct.ServeDir.html
- https://docs.rs/tower-http/latest/tower_http/set_header/struct.SetResponseHeaderLayer.html
- https://docs.rs/tower-http/latest/tower_http/compression/struct.CompressionLayer.html
- https://github.com/tower-rs/tower-http/blob/main/tower-http/CHANGELOG.md
- https://tokio.rs/blog/2025-01-01-announcing-axum-0-8-0

---

### 検証メモ(呼び出し元向け)

すべて 2026-07-01 の一次情報で確認済み。**バージョンの load-bearing な数値**:
- **axum 0.8.9**(hyper `^1.1.0`, tower `^0.5.2`, tower-http `^0.6.8`)— crates.io API + docs.rs で二重確認。1.0 未到達。
- **tower-http 0.7.0**(2026-06-15)が最新だが、axum 0.8.9 は 0.6.8 系を pin。**実務では tower-http 0.6.x 推奨**。
- WebSocket は axum 本体 in-tree(`ws` feature)、FromRequestParts は axum 0.8 でも現行トレイト名、ServeDir の `.precompressed_br()`/`.precompressed_gzip()` およびヘッダ用 `SetResponseHeaderLayer`(`overriding()`/`appending()`/`if_not_present()`)はメソッド名まで確認済み。

一点だけ注意: 検索スニペットの一部が「tower-http 最新は 0.6.11」と古い情報を返したが、crates.io API と docs.rs crate ページの両方で **0.7.0 が最新**と確認できたため、そちらを採用した。この 0.7.0 の axum 0.8 互換性は未検証のため確信度「中」とし、0.6 系採用を推奨した。