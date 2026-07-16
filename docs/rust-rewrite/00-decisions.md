# yuuka バックエンド Rust 移行 — 確定技術選定 (ADR)

> 2026-07-01 時点。全項目は `docs/rust-rewrite/verification/` の一次ソース照合レポート（15領域）に基づく。
> バージョン数値は crates.io API / docs.rs / 公式ドキュメントで実測済み。以降の全計画・実装はこの決定に整合させること。
>
> **絶対制約（ユーザー要件・全決定の上位）**
> 1. 厳格エラー：`anyhow`/`eyre`/`color-eyre`/`Box<dyn Error>` 等の握り潰し禁止。`thiserror` の具体列挙型のみ。
> 2. 常時稼働・自己復帰：些細な障害で落ちない。supervisor + バックオフ再起動 + 劣化縮退。致命的設定不備のみ fail-fast。
> 3. 現行 Node/TS より高速・堅牢、マルチスレッド。
> 4. 将来のユーザー製カスタムモジュール拡張性を初期設計に。
> 5. フロント(Svelte/TS)⇄Rust 型を単一真実源(Rust)から自動生成。

---

## 技術選定サマリ

| # | 領域 | 採用 | バージョン(2026-07実測) | 却下/代替 | 確信度 |
|---|---|---|---|---|---|
| 1 | 言語/ビルド | Rust stable + Cargo workspace | rustc **1.96.1** | — | 高 |
| 2 | 非同期ランタイム | Tokio (multi-thread, `panic=unwind`) | **1.52.x** | — | 高 |
| 3 | Web フレームワーク | axum + tower / tower-http | axum **0.8.9** / tower-http **0.6.x** | actix-web | 高 |
| 4 | エラー型 | thiserror（層別具体列挙型） | **2.0.18** | anyhow(禁止), snafu, eyre(禁止) | 高 |
| 5 | エラー機械強制 | clippy workspace lints + cargo-deny bans | clippy 1.96 / cargo-deny **0.19.9** | — | 高 |
| 6 | 自己復帰・監督 | 自前 JoinSet supervisor + tokio-graceful-shutdown | tgs **0.19.3** | — | 高 |
| 7 | リトライ | backon（ジッタ必須） | **1.6.0** | **backoff(禁止=RUSTSEC-2025-0012)**, tryhard | 高 |
| 8 | サーキットブレーカ | recloser または自前 | recloser **1.4.0** | failsafe(2年停滞) | 中〜高 |
| 9 | DB ドライバ | rusqlite（bundled SQLite）＋単一writer actor＋read pool | rusqlite **0.40.1** / SQLite **3.53.2** | sqlx 0.9(代替) | 高 |
| 10 | read pool | deadpool-sqlite または r2d2_sqlite | deadpool **0.13.0** / r2d2_sqlite **0.34.0** | — | 高 |
| 11 | マイグレーション | refinery（前方専用・checksum・冪等baseline） | **0.9.2** | sqlx migrate(代替), 現行 DROP方式(廃止) | 高 |
| 12 | Discord | twilight（マルチテナント・自前poll loop） | gateway **0.17.1** | serenity 0.12.5 | 高 |
| 13 | Gemini | reqwest+serde+thiserror 自前薄ラッパ | reqwest **0.13.4** | gemini-rust 1.7.1(予備), google-generative-ai-rs(不適) | 中〜高 |
| 14 | Gemini API 面 | classic `generateContent` v1beta（1:1移植） | model `gemini-3.1-flash-lite`(GA) | Interactions API(将来) | 高 |
| 15 | ツール/プラグイン | ToolProvider trait レジストリ（Native/MCP/WASM 三系統） | — | 動的.so(危険=不採用) | 高 |
| 16 | MCP 統合 | rmcp（公式SDK, client） | **2.0.0**（spec 2025-11-25） | rust-mcp-sdk | 高 |
| 17 | 非信頼プラグイン実行 | Extism（wasmtime 上・deny-by-default manifest） | extism **1.30.0** / wasmtime **46** | 生wasmtime(代替), .so(不採用) | 高 |
| 18 | フロント型生成 | ts-rs（型のみ・単一真実源） | **12.0.1** | utoipa→openapi-typescript(上位互換), specta(不適) | 高 |
| 19 | 機密フェイルクローズ | 専用DTO struct（機密はフィールドに存在させない）+ secrecy | secrecy 最新 | 現行 zod allowlist を型で置換 | 高 |
| 20 | デプロイ/移行 | Docker多段(既存rust-builder流用) + nginx strangler | — | — | 高 |

---

## 各決定の要点と根拠

### エラー処理（絶対制約1の実現）— [errors-thiserror](verification/rpt-errors-thiserror.md), [clippy-cargodeny](verification/rpt-clippy-cargodeny.md)
- **thiserror 2.0.18**（2.0系は出荷済・de-facto標準）。層別に具体エラー列挙型を定義：`ConfigError` / `DbError` / `RepoError` / `AuthError` / `ValidationError` / `GeminiError` / `DiscordError` / `PluginError` / `IpcError` / `WebError` 等。
- `#[from]` は**真の層境界のみ**（同型複数バリアントで衝突するので乱用禁止）。層をまたぐ変換は明示 `match`。`#[error(transparent)]` は最下段限定。
- 公開/クレート跨ぎのエラー enum に `#[non_exhaustive]`（std言語属性）。ただし axum `IntoResponse` は**同一クレート内**で網羅 match を書けるようにし、バリアント追加漏れをコンパイルエラーで検知。
- HTTP 写像は**手書き `IntoResponse`**（thiserror は HTTP 非提供）。`DbError` 等の内部 Display をクライアントに漏らさない（`"internal"` に丸める）。
- **機械的強制**：root `Cargo.toml` の `[workspace.lints.clippy]` に `unwrap_used / expect_used / panic / todo / unimplemented / unreachable / indexing_slicing / panic_in_result_fn` を**個別に** `"deny"`（restriction グループの一括禁止は厳禁）。各メンバー `[lints] workspace=true`。`clippy.toml` で `allow-unwrap-in-tests=true` 等。`deny.toml` の `[bans]` で `deny=[{crate="anyhow"},{crate="eyre"},{crate="color-eyre"}]`（**`crate=` フィールド**、旧 `name=` は非推奨）。CI: `cargo clippy --all-targets --all-features -- -D warnings` + `cargo deny check`。

### 自己復帰・常時稼働（絶対制約2の実現）— [resilience](verification/rpt-resilience-tokio-backon-recloser.md)
- **`panic = "unwind"` を厳守**（`abort` にするとパニック隔離が無効化＝プロセス即死）。
- **supervisor = 自前 `tokio::task::JoinSet` + `join_next()` ループ**：`Err(JoinError)`（`is_panic()`）を検知→当該サービスを**指数バックオフで再 spawn**。`spawn` したタスクの panic はプロセスを殺さず JoinHandle 経由でエラー化される（tokio 公式）。`join_all()` は使わない（1つのpanicで全abort）。
- **停止協調**：`tokio-graceful-shutdown 0.19.3`（サブシステムツリー + SIGTERM 伝播）。ただし「再起動」ロジックは自前補完（tgs は主に停止協調）。
- **リトライ**：`backon 1.6.0`（`ExponentialBuilder::default().with_jitter()` を**明示**）。`backoff` は **RUSTSEC-2025-0012 で非メンテ宣言＝採用禁止**（`cargo audit` が警告）。
- **サーキットブレーカ**：外部依存(Gemini/Google/Discord)ごとに `recloser 1.4.0` か、`AtomicU*` の自前状態機械（生態系が薄いため自前も対等な選択肢）。
- **劣化縮退**：synapse ダウン→直近履歴のみ（現行踏襲）、Redis ダウン→インメモリセッション（**ただし移行期はプロセスローカルで他系に不可視＝断続ログアウトに注意**）。回復可能／致命的を型・ポリシーで峻別（致命的＝起動時 config/secret 不備のみ）。

### Web ランタイム — [axum-web-runtime](verification/rpt-axum-web-runtime.md)
- **axum 0.8.9**（hyper 1.x / tower 0.5 / tower-http 0.6.x）。**tower-http は 0.6.x 固定**（0.7.0 は 2026-06-15 と新しく axum 0.8.9 は `^0.6.8` を pin。型互換確認まで 0.6 系）。axum は 0.x なのでマイナー更新=破壊的変更前提でバージョン管理。
- WebSocket は **`axum::extract::ws`（`ws` feature）** 内蔵で `/ws/chat` を置換。
- 静的配信は **`ServeDir::new(..).precompressed_br().precompressed_gzip()`**（`.br`/`.gz` サイドカーをビルド時生成）+ SPA フォールバック。CSP等は **`SetResponseHeaderLayer::overriding`**、動的圧縮は **`CompressionLayer`**（役割別）。
- 認可レベル(none/user/admin)は **`FromRequestParts` 実装のカスタム型**（`AuthenticatedUser`/`AdminUser`）で**型強制**。任意認証は `OptionalFromRequestParts`（`Option<AuthUser>`）。axum 0.8 は `#[async_trait]` 不要（RPITIT）。
- Cookie(`__Host-yuuka-session`)＋Bearer(desktop) の二経路を extractor で解決。config.yaml → 型付き構造体を起動時に厳密検証。

### DB・マイグレーション・データ分離 — [db-sqlx-vs-rusqlite](verification/rpt-db-sqlx-vs-rusqlite.md), [migrations](verification/rpt-migrations-sqlx-refinery.md), [dual-sqlite-hazard](verification/rpt-dual-sqlite-hazard.md)
- **rusqlite 0.40.1（bundled SQLite 3.53.2）を採用**。理由：既存 `rust_synapse` が rusqlite 使用＝統一、書き込み経路の予測可能性、SQLite 機能の深い制御。（sqlx 0.9 はコンパイル時クエリ検査が魅力だが、SQLite書き込み並行の footgun と synapse との二重化で見送り。代替として記録。）
- **単一writer actor（全書き込みを1タスク/1コネクションに直列化）+ read pool（deadpool-sqlite 0.13 / r2d2_sqlite 0.34）**。同期呼び出しは `spawn_blocking`。
- **PRAGMA を明示設定**：`journal_mode=WAL`, `busy_timeout=5000`（Node既定と一致）, `foreign_keys=ON`, `synchronous=NORMAL`。**全書き込みTxは `BEGIN IMMEDIATE`**（DEFERRED→writeアップグレードの即-BUSYを回避）。アプリ層で `SQLITE_BUSY` を backon リトライ。
- **データ分離**：`UserId` newtype を全リポジトリ署名に通し、型で分離キー欠落を防ぐ。
- **マイグレーション**：`refinery 0.9.2`（rusqlite ネイティブ、前方専用、`refinery_schema_history` で checksum 管理）。現行 v17 スキーマを **`CREATE TABLE IF NOT EXISTS` の冪等 baseline (V1)** に凍結（既存DB=無害、新規DB=生成）。**現行の SCHEMA_VERSION 不一致 DROP 再作成（データ喪失）は完全撤廃**。migration 所有権は単一プロセスに一元化。

### Discord — [discord-serenity-twilight](verification/rpt-discord-serenity-twilight.md)
- **twilight 0.17.1 を採用**（serenity 0.12.5 却下）。1プロセスで多数のボットトークン（現行 `customClients` 相当）を扱うマルチテナントに最適。`Shard` は再接続/resume を内蔵するが**caller駆動のpoll loop**＝supervisor と統合してテナント別バックオフ/再起動を掛けられる（serenity はループを隠蔽し個別復帰が困難）。
- crate 構成：`twilight-gateway`(Shard) / `twilight-http`(InteractionClient) / `twilight-model` / `twilight-cache-inmemory`(任意・テナント別) / `twilight-util`(builders)。ボタンは `InteractionResponse`（`UpdateMessage` 等）で対応。HTTP クライアントは共有、キャッシュはテナント別 or 無しを明示選択。

### Gemini — [gemini-design-verify](verification/rpt-gemini-design-verify.md), [gemini-funccalling-mcp](verification/rpt-gemini-funccalling-mcp.md), [gemini-rest-crates](verification/rpt-gemini-rest-crates.md)
- **自前 reqwest 0.13 + serde + thiserror ラッパを第一推奨**（公式Rust SDK 不在。`google-generative-ai-rs` はアーカイブ済&FC未実装で不適。`gemini-rust 1.7.1` は機能豊富だが厳格エラー方針・API面固定を実装で要確認＝予備）。表面積が小さく、429/`RetryInfo(retryDelay)` を thiserror variant で完全掌握できる（現行の rate-limit バックオフを 1:1 移植）。
- **classic `generateContent` v1beta を採用**（現行コードを 1:1 移植可能）。⚠️2026年に **Interactions API が GA 化し「正面玄関」に昇格・generateContent は "legacy" だが完全サポート継続**＝将来リスクとして記録。**camelCase struct に厳密固定**（Interactions の snake_case を混入させない）。
- Function Calling：`tools[].functionDeclarations`、`toolConfig.functionCallingConfig`（`AUTO`既定 / `ANY`+`allowedFunctionNames` で完了ハルシネーション是正）、`functionCall`↔`functionResponse` 往復、**並行呼び出し公式サポート**、`maxIterations` でループ制限。プラグインのツールスキーマは **`parametersJsonSchema`（フルJSON Schema）** を使い sanitizer で `$schema` 除去等。
- マルチモーダル：`inlineData`(base64) でレシート解析（20MB超のみ Files API）。SSE は `streamGenerateContent?alt=sse` + `eventsource-stream 0.2.3` + **行バッファ必須**（ストリーミングはパリティ上は任意）。モデル `gemini-3.1-flash-lite`(GA) は**変更不要**、`-preview` 名は使わない。

### ツール/プラグイン基盤（絶対制約4の実現）— [plugins-wasm-extism](verification/rpt-plugins-wasm-extism.md), [mcp-rmcp](verification/rpt-mcp-rmcp.md)
- **`ToolProvider` トレイト + 中央レジストリ**（`HashMap<String, Arc<dyn ToolProvider>>`、ツール名は**ソース別 namespace 接頭辞**で衝突回避、Gemini制約の 128字/`[a-zA-Z0-9_:.-]` 準拠）。
  ```
  trait ToolProvider { fn list(&self)->Vec<ToolSpec>; async fn invoke(&self, name:&str, args:Value, ctx:&ToolContext)->Result<ToolOutput,PluginError>; }
  ```
- 3系統のバックエンド：
  - **NativeProvider**：内蔵Rustトレイト（現行 `src/functions/*` 相当）。
  - **McpProvider**：**rmcp 2.0.0（公式SDK）** の client として外部MCPサーバへ接続（stdio=`TokioChildProcess` / Streamable HTTP、spec 2025-11-25）。現行 `mcpDynamic`/`mcpClient` を吸収。aggregator/gateway パターン。
  - **WasmProvider**：**Extism 1.30.0（wasmtime 上）** で**非信頼なユーザー製プラグイン**を実行。`Manifest` の `allowed_hosts`/`allowed_paths`/memory/timeout で **deny-by-default** 能力付与、polyglot PDK（Rust/Go/JS/…）。（標準志向なら 生 wasmtime 46 + Component Model/WASI 0.2 が代替。）
- **動的 `.so`（libloading/abi_stable）は非信頼コードに不採用**（サンドボックス無し＝ホスト完全侵害、panic-across-FFI が UB）。
- リクエスト毎に全 provider から `functionDeclarations` を生成、`functionCall.id` を並行相関に保持、`ToolContext` に `UserId` を載せ能力スコープ/データ分離を強制。

### フロント⇄Rust 型連携（絶対制約5の実現）— [typegen-tsrs-utoipa](verification/rpt-typegen-tsrs-utoipa.md)
- **ts-rs 12.0.1 を第一推奨**（型のみ・安定・活発）。現行の手書き `frontend/src/lib/api/types.ts`(655行) の二重管理を直接解消。**上位互換の選択肢**＝`utoipa 5.5.0 → OpenAPI 3.1 → openapi-typescript 7.13.0(+openapi-fetch)`（エンドポイント契約＋型付きクライアントまで欲しい場合）。**specta は不適**（v2 が RC 継続・Tauri 志向・rspc 終了）。
- **機密フェイルクローズは「専用DTO struct」で構造的に保証**：機密列は DTO の**フィールドに存在させない**→生成 TS にも現れず漏洩は型的に不可能（現行 zod allowlist の思想をコンパイル時保証へ）。in-memory 機密は `secrecy`。
- CI ドリフト検出：生成→`git diff --exit-code`。`ApiResponse<T>`（data ラッパ無し）エンベロープを型化。

### デプロイ・段階移行 — [nginx-session-strangler](verification/rpt-nginx-session-strangler.md), [dual-sqlite-hazard](verification/rpt-dual-sqlite-hazard.md)
- Docker 多段（既存 `rust-builder` ステージ流用）。**nginx strangler**（リポジトリに既存）：`upstream node/rust` + **per-location `proxy_pass`（末尾URI無しでパス保存）**。移行済みルートのみ Rust upstream へ。
- **共有 Redis の不透明トークンセッション＝署名鍵共有不要**（両バックエンドがトークンを同一アルゴリズムでハッシュ→Redis参照。一致必須：ハッシュ方式・キー書式・シリアライズ・TTL）。`__Host-` Cookie は Domain 不可＝**同一オリジン必須**（nginx 単一オリジン背後で新旧を出し分け）。WebSocket は `map $http_upgrade $connection_upgrade` + `Upgrade`/`Connection` 転送 + `proxy_read_timeout 3600s` + バックエンド ping。
- **SQLite 移行ハザードの核心（最重要）**：同一ホスト別プロセスの Node+Rust 同時アクセスは**公式に安全**（POSIX lock + 共有 `-wal`/`-shm`、NFS不可）。ただし **writer は同時に1つが絶対条件**。両側書き込みは DEFERRED→write アップグレードで **busy_timeout を無視した即 `SQLITE_BUSY`** を招く（busy_timeout では解決不能）。→ **移行期は「Node が全書き込み・Rust は read-only、カットオーバー時に一度だけ writer を Rust へ移譲」= 単一writer集約を最優先**（synapse の read-only 実績と整合）。両writer やむなき場合のみ 両側 busy_timeout明示 + 全Tx `BEGIN IMMEDIATE` + アプリ層リトライ を全て課す。長寿命 reader が checkpoint を阻害するため Rust 側は statement を確実に finalize/reset。
- カナリア：`split_clients`（REST 先・WS 最後、WS は%分割せずエンドポイント一括）。shadow/mirror は**読み取り専用/冪等のみ**（書き込みバックエンドへmirror＝二重書き込み）。レガシー削除は**検証後の最終ステップ**、旧経路を warm 維持で即ロールバック、ロールバック窓ではスキーマ凍結。

---

## 未解決の決定事項（ユーザー判断が要る点）
1. **フロント型生成**：ts-rs（型のみ・低リスク）か utoipa+openapi-typescript（契約+型付きクライアント・工数増）か。→ 既定は ts-rs、契約まで欲しければ utoipa。
2. **DB**：rusqlite（推奨・synapse統一・書込予測可）か sqlx（コンパイル時クエリ検査が欲しい場合）か。→ 既定 rusqlite。
3. **Gemini API 面**：classic generateContent（1:1移植・既定）で開始し、Interactions API 移行を将来課題とするか、最初から Interactions を狙うか（Rust 生態系ほぼ未対応のため薄ラッパ必須）。
4. **edition**：2021 か 2024 か（MSRV 影響）。
5. **プラグイン初期スコープ**：Native+MCP を先行し WASM(Extism) を後続フェーズにするか、初期から3系統揃えるか。
