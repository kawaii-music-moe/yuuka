# yuuka バックエンド Rust 移行マスタープラン — 第1部：概要・アーキテクチャ・技術選定

> 本書は最終マスタープランの **第1部（セクション 1〜3）**。
> 全内容は確定 ADR [`../00-decisions.md`](../00-decisions.md) と一次ソース照合レポート群
> [`../verification/`](../verification/) に厳密整合させており、ここで技術を再決定することはしない。
> 詳細な設計材料は [`../design-checkpoint.md`](../design-checkpoint.md)（22 セクション）にある。
>
> **本書全体を貫く前提（絶対制約・[00-decisions §絶対制約](../00-decisions.md)）**：
> 1. **厳格エラー**：`anyhow`/`eyre`/`color-eyre`/`Box<dyn Error>` の握り潰しは全面禁止。`thiserror` の層別具体列挙型のみ。
> 2. **常時稼働・自己復帰**：些細な障害でプロセスを落とさない。supervisor + 指数バックオフ再起動 + 劣化縮退。致命的設定不備のみ fail-fast。
>
> この 2 点は個別セクションの機能要件ではなく、全クレート・全タスクに適用される**構造的前提**である。

---

## 1. 目的とゴール / 成功条件

### 1.1 なぜ Rust 全面書き換えか

現行 yuuka は Node/TypeScript 製のモノリシックなプロセスであり、単一プロセス内に以下が同居している。

- **生 `http` 自作サーバ**（`src/server.ts` / `src/server/`）：フレームワークを使わず自前でルーティング・認可・静的配信・`ws` パッケージによる `/ws/chat` WebSocket を実装。
- **`better-sqlite3`**（`src/db/database.ts`）：同期 API の SQLite。マイグレーションは `src/db/migrations.ts` の `SCHEMA_VERSION` 不一致時に **DROP して再作成**（＝データ喪失リスク）という危険な方式。
- **`discord.js`**（`src/bot.ts`）：複数ボットトークンを `customClients` 相当で束ねるマルチテナント運用。ループ・再接続をライブラリが隠蔽するため、テナント個別の復帰制御が効かない。
- **Gemini クライアント**（`src/gemini.ts` / `@google/generative-ai`）：`generateContent` を叩き、`RetryInfo.retryDelay` を REST エラー body からパースして自前リトライ（`generateWithRetry`）。
- **常駐 Rust デーモン 2 種を子プロセス spawn**：
  - `yuuka-synapse`（記憶エンジン、`src/rust_synapse/`）を `spawn(bin, ["daemon", ...])` で常駐させ、**改行区切り JSON-RPC**（stdin/stdout）で通信（`src/services/synapseEngine.ts`）。
  - `yuuka-crawler`（ブラウザ、`src/rust_crawler/`）を同型で子プロセス化。
  - いずれも `pending` Map による id 相関・タイムアウト・`exit`/`error`/EPIPE 監視・自動再起動を TS 側に手書きしている。バイナリ不在時は degraded 動作（直近履歴のみ等）にフォールバック。
- さらに `node-cron` ベースの常駐サービス群（`src/services/` 配下：reminder / report / briefing / backup / birthday / payment / playbook / todo recurrence / metrics / clipboard cleanup / synapse など）を `src/index.ts` から個別 `start*` / `stop*` で起動・停止している。

この構成の問題は、(a) 単一プロセス＝実質シングルスレッド寄りで**マルチコアを活かせない**、(b) Rust デーモンとの通信が **JSON-RPC over パイプ**でありシリアライズ／プロセス境界のオーバーヘッドと失敗モードを抱える、(c) SQLite マイグレーションが**破壊的**、(d) エラーが `Box<dyn Error>` / `Result<_, String>`（既存 Rust クレート）や TS の握り潰しで**型で追跡できない**、という 4 点に集約される。

Rust 全面書き換えの狙いは、これらを一挙に解消することにある。すなわち **Tokio マルチスレッド（work-stealing）による複数サービス同時実行の高速化・マルチコア活用**、**単一バイナリ内 supervised task 化による IPC 削減と堅牢な自己復帰**、そして **`thiserror` 層別エラー＋`clippy`/`cargo-deny` 機械強制による堅牢性**である。既存 Rust デーモン（synapse/crawler）は既に Rust であり、サーバ本体を Rust 化することで**単一 cargo workspace に統合**でき、子プロセス spawn ではなく同一バイナリ内タスクへ寄せられる。

### 1.2 5 つの絶対制約＝成功条件と実現セクションの対応

以下の 5 制約（[00-decisions §絶対制約](../00-decisions.md)）を**成功条件**とし、本マスタープランのどのセクションで実現されるかを対応付ける。制約 1・2 は本書全体の前提でもある。

| # | 絶対制約（成功条件） | 実現される主なセクション | 根拠レポート |
|---|---|---|---|
| 1 | **厳格エラー**：`anyhow`/`eyre`/`Box<dyn Error>` 禁止、`thiserror` 層別列挙型のみ、`clippy`/`cargo-deny` で機械強制 | §2.1（クレート＝層＝エラー型の一致）／第2部エラー処理章 | [rpt-errors-thiserror](../verification/rpt-errors-thiserror.md), [rpt-clippy-cargodeny](../verification/rpt-clippy-cargodeny.md) |
| 2 | **常時稼働・自己復帰**：supervisor＋バックオフ再起動＋劣化縮退、致命的設定不備のみ fail-fast | §2.2（`yuuka-supervisor` / JoinSet 監督モデル）／第2部レジリエンス章 | [rpt-resilience-tokio-backon-recloser](../verification/rpt-resilience-tokio-backon-recloser.md) |
| 3 | **現行より高速・堅牢・マルチスレッド** | §2.2（Tokio multi-thread 実行モデル・子プロセス統合）／§3（Web/DB/Discord 選定） | [rpt-axum-web-runtime](../verification/rpt-axum-web-runtime.md), [rpt-db-sqlx-vs-rusqlite](../verification/rpt-db-sqlx-vs-rusqlite.md) |
| 4 | **ユーザー製カスタムモジュール拡張性を初期設計に** | §2.1（`yuuka-tools`：`ToolProvider` レジストリ Native/MCP/WASM）／第2部プラグイン章 | [rpt-plugins-wasm-extism](../verification/rpt-plugins-wasm-extism.md), [rpt-mcp-rmcp](../verification/rpt-mcp-rmcp.md) |
| 5 | **フロント(Svelte/TS)⇄Rust 型を単一真実源(Rust)から自動生成** | §2.1（`yuuka-core`/DTO＋`xtask` 型生成）／第2部型連携章 | [rpt-typegen-tsrs-utoipa](../verification/rpt-typegen-tsrs-utoipa.md) |

> 5 制約はいずれも「実装したら達成」ではなく「**構造で保証されているか**」で判定する。例：制約 1 は grep や目視ではなく `cargo deny check`＋`cargo clippy -- -D warnings` が CI で緑であること、制約 5 は `cargo xtask gen-types --check` がドリフトゼロで通ることをもって成功とする。

---

## 2. 全体アーキテクチャ

### 2.1 Cargo workspace クレート構成

単一の `cargo workspace` に統合する。既存の独立クレート `src/rust_synapse/`・`src/rust_crawler/`（[design-checkpoint](../design-checkpoint.md) で「ルート `Cargo.toml` は存在せず 2 クレートが独立」と実確認済み）を workspace に取り込み、サーバ本体・bot・AI・サービス群を新規クレートとして追加する。

> **命名について**：本マスタープランでは以下のクレート名を確定名とする。設計ドラフト（[design-checkpoint §C.1](../design-checkpoint.md)）では `yuuka-foundation` / `yuuka-domain` / `yuuka-engine` / `bins/gen-types` という暫定名を用いた箇所があるが、本書の命名（`yuuka-core` / `yuuka-services` / `yuuka-supervisor` / `xtask`）に読み替える。責務の切り方は同一である。

```
apps/yuuka/
  Cargo.toml                      # [workspace] / [workspace.dependencies] / [workspace.lints]
  clippy.toml                     # allow-unwrap-in-tests=true 等
  deny.toml                       # [bans] anyhow/eyre/color-eyre/backoff を拒否
  crates/
    yuuka-core/                   # config・型・エラー基盤・UserId 等 newtype・secrecy・telemetry
    yuuka-db/                     # rusqlite 単一writer actor + read pool + refinery migration + repo
    yuuka-web/                    # axum サーバ・ルート・認可 extractor・静的配信・WS
    yuuka-discord/                # twilight マルチテナント bot（gateway/http/model）
    yuuka-gemini/                 # reqwest 薄ラッパ・generateContent・function calling
    yuuka-tools/                  # ToolProvider レジストリ（Native / MCP=rmcp / WASM=Extism）
    yuuka-services/               # cron/briefing/report/reminder/backup/synapse 連携等の常駐タスク
    yuuka-synapse/                # 既存 rust_synapse を workspace 化（記憶エンジン）
    yuuka-crawler/                # 既存 rust_crawler を workspace 化（ブラウザ）
  bins/
    yuuka/                        # ★エントリ = yuuka-supervisor：JoinSet 監督・起動/停止ライフサイクル
  xtask/                          # 型生成(ts-rs)・cargo-deny/clippy 補助・CI ドリフト検査
```

各クレートの責務：

- **`yuuka-core`**（全員が依存する土台）
  config.yaml → 型付き構造体への厳密検証、**層別エラー enum の基底**（`ConfigError` 等の最下段）、`UserId` をはじめとする newtype（データ分離キーを型で強制）、機密の `secrecy::SecretString` ラッパ、telemetry/ログ初期化。最初に凍結しないと全クレートが型で衝突するため、移行 Phase F で単独先行する（[design-checkpoint §Phase F](../design-checkpoint.md)）。

- **`yuuka-db`**（DB 担当・[rpt-db-sqlx-vs-rusqlite](../verification/rpt-db-sqlx-vs-rusqlite.md), [rpt-migrations-sqlx-refinery](../verification/rpt-migrations-sqlx-refinery.md)）
  **rusqlite 0.40.1（bundled SQLite 3.53.2）**。全書き込みを 1 タスク・1 コネクションに直列化する**単一 writer actor**＋読み取り用の **read pool（deadpool-sqlite 0.13 / r2d2_sqlite 0.34）**。同期呼び出しは `spawn_blocking`。PRAGMA（`journal_mode=WAL` / `busy_timeout=5000` / `foreign_keys=ON` / `synchronous=NORMAL`）を明示、全書き込み Tx は `BEGIN IMMEDIATE`。マイグレーションは **refinery 0.9.2**（前方専用・checksum 管理・現行 v17 スキーマを `CREATE TABLE IF NOT EXISTS` の冪等 baseline V1 に凍結）。現行 `src/db/migrations.ts` の DROP 再作成方式は**完全撤廃**。`DbError` を持つ。全リポジトリ署名に `UserId` を通す。

- **`yuuka-web`**（Web 担当・[rpt-axum-web-runtime](../verification/rpt-axum-web-runtime.md)）
  **axum 0.8.9 / tower-http 0.6.x**。ルータ・middleware・静的配信（`ServeDir` + `precompressed_br/gzip` + SPA フォールバック）・`/ws/chat`（`axum::extract::ws`）。認可レベル（none/user/admin）は `FromRequestParts` 実装のカスタム型（`AuthenticatedUser` / `AdminUser`）で**型強制**、任意認証は `OptionalFromRequestParts`。Cookie(`__Host-yuuka-session`)＋Bearer(desktop) の二経路。`WebError` を持ち、`IntoResponse` を手実装して内部 Display をクライアントに漏らさない。現行 `src/server.ts` の生 http 実装を置換。

- **`yuuka-discord`**（bot 担当・[rpt-discord-serenity-twilight](../verification/rpt-discord-serenity-twilight.md)）
  **twilight 0.17.1**（`twilight-gateway`/`twilight-http`/`twilight-model`/`twilight-cache-inmemory`/`twilight-util`）。1 プロセスで多数のボットトークンを扱うマルチテナント。`Shard` を **caller 駆動の poll loop** で回し、supervisor と統合してテナント別バックオフ／再起動を掛ける。`DiscordError` を持つ。現行 `src/bot.ts`（discord.js）を置換。

- **`yuuka-gemini`**（AI 担当・[rpt-gemini-design-verify](../verification/rpt-gemini-design-verify.md), [rpt-gemini-rest-crates](../verification/rpt-gemini-rest-crates.md)）
  **reqwest 0.13 + serde + thiserror の自前薄ラッパ**（公式 Rust SDK 不在）。classic `generateContent` v1beta を camelCase struct に厳密固定して 1:1 移植。429 / `RetryInfo.retryDelay` を `GeminiError` の variant に載せ、現行 `generateWithRetry` のバックオフを移植。Function Calling（`functionDeclarations` / `toolConfig` / `functionCall`↔`functionResponse` 往復・並行呼び出し）を実装。

- **`yuuka-tools`**（拡張点・[rpt-plugins-wasm-extism](../verification/rpt-plugins-wasm-extism.md), [rpt-mcp-rmcp](../verification/rpt-mcp-rmcp.md)）
  **`ToolProvider` トレイト＋中央レジストリ**（`HashMap<String, Arc<dyn ToolProvider>>`、ソース別 namespace 接頭辞で衝突回避）。3 系統：**NativeProvider**（内蔵 Rust トレイト、現行 `src/functions/*` 相当）、**McpProvider**（**rmcp 2.0.0** 公式 SDK client、現行 `mcpDynamic`/`mcpClient` を吸収）、**WasmProvider**（**Extism 1.30.0** / wasmtime 上・deny-by-default manifest で**非信頼なユーザー製プラグイン**を実行）。動的 `.so` は非信頼コードに**不採用**。`ToolContext` に `UserId` を載せ能力スコープ／データ分離を強制。`PluginError` を持つ。絶対制約 4（拡張性）の実体。

- **`yuuka-services`**（services 担当）
  現行 `src/services/` の常駐タスク群（reminder / report / briefing / backup / birthday / payment / playbook / todo recurrence / metrics / clipboard cleanup / synapse 連携）を移植。各サービスは cron スケジュールを持つ長寿命タスクとして `yuuka-supervisor` の JoinSet 配下で回る。cron は HTTP 非公開。

- **`yuuka-supervisor`（bin `yuuka`）**（[rpt-resilience-tokio-backon-recloser](../verification/rpt-resilience-tokio-backon-recloser.md)）
  エントリポイント兼監督。**自前 `tokio::task::JoinSet` + `join_next()` ループ**で web/discord/services/db-writer 等を束ね、`Err(JoinError)`（`is_panic()`）を検知して当該タスクを**指数バックオフで再 spawn**。停止協調は **tokio-graceful-shutdown 0.19.3**（サブシステムツリー＋SIGTERM 伝播）だが、「再起動」ロジックは自前補完する。`AppError`（最上位）を持つ。絶対制約 2（自己復帰）の実体。

- **`yuuka-synapse` / `yuuka-crawler`**
  既存 Rust デーモンを workspace へ取り込む。当面は互換のため**同一 workspace 内の別クレート**として維持しつつ、`yuuka-supervisor` 配下の **supervised task**（あるいは子プロセスから同一バイナリ内タスクへ）に統合していく。将来は同一バイナリ内タスク化も検討（下記の IPC 削減利点を参照）。

- **`xtask`**（[rpt-typegen-tsrs-utoipa](../verification/rpt-typegen-tsrs-utoipa.md)）
  **ts-rs 12.0.1** による型生成（`cargo xtask gen-types` → `frontend/src/lib/api/generated.ts`）、CI ドリフト検査（`--check` で乖離時に非ゼロ終了）、`cargo-deny`/`clippy` 補助。build.rs ではなく xtask パターン（副作用でワークツリーを汚さず再現性を確保）。絶対制約 5（型連携）の生成器。

#### クレート間依存の向き（循環禁止）

依存は「下流が上流に依存しない DAG」を厳守する（[design-checkpoint §Phase F](../design-checkpoint.md)）。

```
                         ┌──────────────┐
                         │  yuuka-core  │  ← 全クレートが依存（config/error基盤/newtype/secret）
                         └──────┬───────┘
             ┌────────────┬─────┴──────┬─────────────┬──────────────┐
             ▼            ▼            ▼             ▼              ▼
        yuuka-db     yuuka-gemini  yuuka-tools   yuuka-synapse  yuuka-crawler
             │            │            │
             └─────┬──────┴─────┬──────┘
                   ▼            ▼
              yuuka-web    yuuka-discord    yuuka-services
                   └────────────┴────────────────┬──────────┘
                                                 ▼
                                        yuuka-supervisor (bin: yuuka)
                                        ── JoinSet で全タスクを束ねる ──
        xtask ── yuuka-core / DTO を参照して ts-rs 生成（実行時依存なし）
```

- `yuuka-core` を**全員が依存**する（最上位＝最も抽象な土台）。
- `yuuka-web` / `yuuka-discord` / `yuuka-gemini` / `yuuka-tools` は**上位**（アプリ層）で、`yuuka-core` と必要な下流（db/gemini 等）に依存する。
- `yuuka-supervisor` は全アプリクレートを束ねる**最下流**（依存の終端）。
- **循環依存は禁止**。web が db/ai に直接依存せず、`yuuka-core` に定義したトレイト（`SessionStore` / `UserRepo` 等）越しに `AppState` から受け取る設計（[design-checkpoint §C.1](../design-checkpoint.md)）で、web crate をドメインロジックのコンパイル時間・依存に巻き込まず、テストで in-memory 実装を差せるようにする。
- クレート＝層＝エラー型が一致する（`yuuka-db`→`DbError`、`yuuka-web`→`WebError`、…）ため、**エラーの伝播方向も依存の向きに一致**し、`#[from]` は真の層境界のみで使える（絶対制約 1）。

### 2.2 マルチスレッド実行モデル

ランタイムは **Tokio 1.52.x の multi-thread（work-stealing）スケジューラ**、`panic = "unwind"` を厳守する（`abort` にするとパニック隔離が無効化＝プロセス即死のため。[rpt-resilience §1](../verification/rpt-resilience-tokio-backon-recloser.md)）。

**単一プロセス内での同居構成**：

- **`yuuka-supervisor`（bin `yuuka`）が最上位タスク**。起動時に config/secret を厳密検証（不備なら**ここだけ fail-fast**＝絶対制約 2 の「本当に致命的」）、その後 JoinSet に以下の長寿命タスクを spawn する：
  - **web サーバ**（axum、`/ws/chat` を含む）
  - **discord bot**（twilight、テナントごとの Shard poll loop）
  - **services 群**（cron/reminder/report/briefing/backup/… の各常駐タスク）
  - **単一 writer actor**（DB 書き込みを直列化する 1 タスク・1 コネクション）
  - **synapse / crawler**（当面は supervised、将来は同一バイナリ内タスク）
- **read pool** は各タスクから共有され、読み取りは `spawn_blocking` でブロッキングスレッドプールに逃がす（rusqlite は同期 API）。書き込みは必ず単一 writer actor へメッセージ送信し直列化する。
- **work-stealing** により、web リクエスト処理・bot イベント処理・cron タスクが**空いたワーカースレッドに自動分散**され、現行 Node の実質シングルスレッド寄り実行に対しマルチコアを活用できる（絶対制約 3）。
- **パニック隔離**：`spawn` したタスク内のパニックはプロセスを殺さず、`JoinSet::join_next()` が `Err(JoinError)`（`is_panic()==true`）として返す（tokio 公式）。supervisor はこれを検知し当該タスクのみ**指数バックオフで再 spawn**する。`JoinSet::join_all()` は使わない（1 つの panic で全 abort されるため）。supervisor 本体の生存は別レイヤで保証する（[rpt-resilience §1](../verification/rpt-resilience-tokio-backon-recloser.md)）。

**「Rust デーモンを子プロセス spawn」→「同一バイナリ内 supervised task」統合の利点**：

現行は synapse/crawler を子プロセスとして起動し、**改行区切り JSON-RPC over stdin/stdout** で通信している（`src/services/synapseEngine.ts` が `pending` Map で id 相関・タイムアウト・EPIPE 監視・自動再起動を手書き）。これを同一バイナリ内のタスクへ統合すると：

- **IPC シリアライズの削減**：JSON エンコード／デコードとパイプ往復が消え、Rust の構造体を関数呼び出し・チャネル送受信で直接授受できる（型付き・ゼロコピー寄り）。
- **失敗モードの単純化**：プロセス境界の `exit`/`error`/EPIPE・タイムアウト・id 相関という失敗モードが消え、`JoinSet` の統一された `JoinError` 監督に一本化される。
- **監督の一元化**：テナント別・サービス別のバックオフ／劣化縮退ポリシーを supervisor が一括管理でき、子プロセス個別の再起動ロジックを手書きしなくてよい。
- **移行の安全性**：ただし synapse が既に rusqlite を read-only で使う（[rpt-dual-sqlite-hazard](../verification/rpt-dual-sqlite-hazard.md)）ため、DB writer は必ず 1 つに集約する。移行期は「Node が全書き込み・Rust は read-only、カットオーバー時に一度だけ writer を Rust へ移譲」を最優先する（詳細は第3部デプロイ章）。

> **劣化縮退（絶対制約 2）**：synapse ダウン→直近履歴のみ（現行踏襲）、Redis ダウン→インメモリセッション（移行期はプロセスローカルで他系に不可視＝断続ログアウトに注意）。回復可能／致命的を型・ポリシーで峻別し、致命的は起動時 config/secret 不備のみとする。

---

## 3. 技術選定サマリ表

以下は [00-decisions §技術選定サマリ](../00-decisions.md) を本計画の読者向けに要約・再掲したものである。**却下案の詳細な比較や各バージョンの根拠は 00-decisions と後続の各セクション（第2部・第3部）に譲る**。ここでは「何を・どのバージョンで採るか」を一望する用途に絞る。

| 領域 | 採用 | バージョン(2026-07実測) | 却下案 | 根拠（1 行） |
|---|---|---|---|---|
| 言語/ビルド | Rust stable + Cargo workspace | rustc **1.96.1** | — | 既存 Rust 資産と統一・マルチコア |
| 非同期ランタイム | Tokio multi-thread（`panic=unwind`） | **1.52.x** | — | work-stealing でサービス同時実行 |
| Web | axum + tower / tower-http | axum **0.8.9** / tower-http **0.6.x** | actix-web | 型付き extractor・tower エコシステム |
| エラー型 | thiserror（層別具体列挙型） | **2.0.18** | anyhow/eyre/color-eyre（禁止）, snafu | 絶対制約 1・型で追跡 |
| エラー機械強制 | clippy workspace lints + cargo-deny bans | clippy 1.96 / cargo-deny **0.19.9** | — | CI で握り潰しを機械的に拒否 |
| 自己復帰・監督 | 自前 JoinSet supervisor + tokio-graceful-shutdown | tgs **0.19.3** | — | 絶対制約 2・タスク個別再起動 |
| リトライ | backon（ジッタ必須） | **1.6.0** | **backoff（禁止=RUSTSEC-2025-0012）**, tryhard | 非メンテ crate 回避・ジッタ内蔵 |
| サーキットブレーカ | recloser または自前 | recloser **1.4.0** | failsafe（2 年停滞） | 外部依存ごとに遮断 |
| DB ドライバ | rusqlite（bundled SQLite）＋単一 writer actor＋read pool | rusqlite **0.40.1** / SQLite **3.53.2** | sqlx 0.9（代替記録） | synapse と統一・書込予測可 |
| read pool | deadpool-sqlite または r2d2_sqlite | deadpool **0.13.0** / r2d2_sqlite **0.34.0** | — | 読み取り並行・spawn_blocking |
| マイグレーション | refinery（前方専用・checksum・冪等 baseline） | **0.9.2** | sqlx migrate（代替）, 現行 DROP 方式（廃止） | データ喪失方式の完全撤廃 |
| Discord | twilight（マルチテナント・自前 poll loop） | gateway **0.17.1** | serenity 0.12.5 | テナント別バックオフを supervisor 統合 |
| Gemini | reqwest+serde+thiserror 自前薄ラッパ | reqwest **0.13.4** | gemini-rust 1.7.1（予備）, google-generative-ai-rs（不適） | 表面積小・429/RetryInfo を型で掌握 |
| Gemini API 面 | classic `generateContent` v1beta（1:1 移植） | model `gemini-3.1-flash-lite`(GA) | Interactions API（将来） | 現行コードを 1:1 移植可能 |
| ツール/プラグイン | ToolProvider trait レジストリ（Native/MCP/WASM） | — | 動的 .so（危険=不採用） | 絶対制約 4・拡張性 |
| MCP 統合 | rmcp（公式 SDK, client） | **2.0.0**（spec 2025-11-25） | rust-mcp-sdk | 現行 mcpDynamic/mcpClient を吸収 |
| 非信頼プラグイン実行 | Extism（wasmtime 上・deny-by-default manifest） | extism **1.30.0** / wasmtime **46** | 生 wasmtime（代替）, .so（不採用） | サンドボックス化した非信頼実行 |
| フロント型生成 | ts-rs（型のみ・単一真実源） | **12.0.1** | utoipa→openapi-typescript（上位互換）, specta（不適） | 絶対制約 5・手書き types.ts の二重管理解消 |
| 機密フェイルクローズ | 専用 DTO struct（機密はフィールドに存在させない）+ secrecy | secrecy 最新 | 現行 zod allowlist を型で置換 | 漏洩を型的に不可能化 |
| デプロイ/移行 | Docker 多段（既存 rust-builder 流用）+ nginx strangler | — | — | 既存資産流用・段階カットオーバー |

> **未解決の決定事項**（ユーザー判断が要る点：フロント型生成の ts-rs か utoipa か、DB の rusqlite か sqlx か、Gemini API 面、edition 2021/2024、プラグイン初期スコープ）は [00-decisions §未解決の決定事項](../00-decisions.md) にまとまっている。既定値（ts-rs / rusqlite / classic generateContent / Native+MCP 先行）を本書は採用している。
