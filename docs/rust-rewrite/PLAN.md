# yuuka バックエンド Rust 全面移行 — 詳細マスタープラン

> **対象**: Discord Gemini 秘書ボット + Web 管理ダッシュボード「yuuka」のバックエンド（現行 Node.js/TypeScript）を Rust へ全面書き換えする詳細計画。
> **作成**: 2026-07-01 / ブランチ `feature/rust-rewrite`。
> **根拠**: 既存棚卸し（6領域）＋設計・敵対的検証（[design-checkpoint.md](design-checkpoint.md) 22セクション）＋技術検証15領域の一次ソース照合（[verification/](verification/)）＋確定技術選定（[00-decisions.md](00-decisions.md)）。
> 本書の技術選定は全て [00-decisions.md](00-decisions.md)（ADR）に従う。各主張は verification レポートおよび現行コードへ相対リンクで裏取り済み。

## 絶対制約（ユーザー要件・全設計の上位規範）
1. **厳格エラー**: `anyhow`/`eyre`/`Box<dyn Error>` 等の握り潰し禁止。`thiserror` の層別具体列挙型のみ。clippy/cargo-deny で機械強制。
2. **常時稼働・自己復帰**: 些細な障害で落ちない。supervisor + 指数バックオフ再起動 + 劣化縮退。致命的設定不備のみ fail-fast。
3. **高速・堅牢**: 現行 Node/TS より高速、マルチコア活用。
4. **拡張性**: 将来のユーザー製カスタムモジュールを初期設計に織り込む。
5. **型連携**: フロント(Svelte/TS)⇄Rust 型を単一真実源(Rust)から自動生成。

## 目次
- **第1部** 概要・全体アーキテクチャ・技術選定サマリ（§1〜3）
- **第4〜5部** 厳格エラーアーキテクチャ・自己復帰/スーパーバイザ（§4〜5）
- **第6〜7部** Web ランタイム/認証/静的配信/WS・DB/マイグレーション/データ分離（§6〜7）
- **第8部** Discord + Gemini/Function Calling（§8）
- **第9〜10部** ユーザー拡張モジュール基盤・フロント型連携（§9〜10）
- **第11〜12部** 段階移行ロードマップ・サブエージェント並行実装ワークフロー（§11〜12）
- **第13〜14部** リスク一覧と対策・オープンな決定事項（§13〜14）

> 各部は `parts/` 配下の個別ファイルとしても保守されている（本書はその結合ビュー）。

---

> 本書は最終マスタープランの **第1部（セクション 1〜3）**。
> 全内容は確定 ADR [`../00-decisions.md`](00-decisions.md) と一次ソース照合レポート群
> [`../verification/`](verification/) に厳密整合させており、ここで技術を再決定することはしない。
> 詳細な設計材料は [`../design-checkpoint.md`](design-checkpoint.md)（22 セクション）にある。
>
> **本書全体を貫く前提（絶対制約・[00-decisions §絶対制約](00-decisions.md)）**：
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

以下の 5 制約（[00-decisions §絶対制約](00-decisions.md)）を**成功条件**とし、本マスタープランのどのセクションで実現されるかを対応付ける。制約 1・2 は本書全体の前提でもある。

| # | 絶対制約（成功条件） | 実現される主なセクション | 根拠レポート |
|---|---|---|---|
| 1 | **厳格エラー**：`anyhow`/`eyre`/`Box<dyn Error>` 禁止、`thiserror` 層別列挙型のみ、`clippy`/`cargo-deny` で機械強制 | §2.1（クレート＝層＝エラー型の一致）／第2部エラー処理章 | [rpt-errors-thiserror](verification/rpt-errors-thiserror.md), [rpt-clippy-cargodeny](verification/rpt-clippy-cargodeny.md) |
| 2 | **常時稼働・自己復帰**：supervisor＋バックオフ再起動＋劣化縮退、致命的設定不備のみ fail-fast | §2.2（`yuuka-supervisor` / JoinSet 監督モデル）／第2部レジリエンス章 | [rpt-resilience-tokio-backon-recloser](verification/rpt-resilience-tokio-backon-recloser.md) |
| 3 | **現行より高速・堅牢・マルチスレッド** | §2.2（Tokio multi-thread 実行モデル・子プロセス統合）／§3（Web/DB/Discord 選定） | [rpt-axum-web-runtime](verification/rpt-axum-web-runtime.md), [rpt-db-sqlx-vs-rusqlite](verification/rpt-db-sqlx-vs-rusqlite.md) |
| 4 | **ユーザー製カスタムモジュール拡張性を初期設計に** | §2.1（`yuuka-tools`：`ToolProvider` レジストリ Native/MCP/WASM）／第2部プラグイン章 | [rpt-plugins-wasm-extism](verification/rpt-plugins-wasm-extism.md), [rpt-mcp-rmcp](verification/rpt-mcp-rmcp.md) |
| 5 | **フロント(Svelte/TS)⇄Rust 型を単一真実源(Rust)から自動生成** | §2.1（`yuuka-core`/DTO＋`xtask` 型生成）／第2部型連携章 | [rpt-typegen-tsrs-utoipa](verification/rpt-typegen-tsrs-utoipa.md) |

> 5 制約はいずれも「実装したら達成」ではなく「**構造で保証されているか**」で判定する。例：制約 1 は grep や目視ではなく `cargo deny check`＋`cargo clippy -- -D warnings` が CI で緑であること、制約 5 は `cargo xtask gen-types --check` がドリフトゼロで通ることをもって成功とする。

---

## 2. 全体アーキテクチャ

### 2.1 Cargo workspace クレート構成

単一の `cargo workspace` に統合する。既存の独立クレート `src/rust_synapse/`・`src/rust_crawler/`（[design-checkpoint](design-checkpoint.md) で「ルート `Cargo.toml` は存在せず 2 クレートが独立」と実確認済み）を workspace に取り込み、サーバ本体・bot・AI・サービス群を新規クレートとして追加する。

> **命名について**：本マスタープランでは以下のクレート名を確定名とする。設計ドラフト（[design-checkpoint §C.1](design-checkpoint.md)）では `yuuka-foundation` / `yuuka-domain` / `yuuka-engine` / `bins/gen-types` という暫定名を用いた箇所があるが、本書の命名（`yuuka-core` / `yuuka-services` / `yuuka-supervisor` / `xtask`）に読み替える。責務の切り方は同一である。

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
  config.yaml → 型付き構造体への厳密検証、**層別エラー enum の基底**（`ConfigError` 等の最下段）、`UserId` をはじめとする newtype（データ分離キーを型で強制）、機密の `secrecy::SecretString` ラッパ、telemetry/ログ初期化。最初に凍結しないと全クレートが型で衝突するため、移行 Phase F で単独先行する（[design-checkpoint §Phase F](design-checkpoint.md)）。

- **`yuuka-db`**（DB 担当・[rpt-db-sqlx-vs-rusqlite](verification/rpt-db-sqlx-vs-rusqlite.md), [rpt-migrations-sqlx-refinery](verification/rpt-migrations-sqlx-refinery.md)）
  **rusqlite 0.40.1（bundled SQLite 3.53.2）**。全書き込みを 1 タスク・1 コネクションに直列化する**単一 writer actor**＋読み取り用の **read pool（deadpool-sqlite 0.13 / r2d2_sqlite 0.34）**。同期呼び出しは `spawn_blocking`。PRAGMA（`journal_mode=WAL` / `busy_timeout=5000` / `foreign_keys=ON` / `synchronous=NORMAL`）を明示、全書き込み Tx は `BEGIN IMMEDIATE`。マイグレーションは **refinery 0.9.2**（前方専用・checksum 管理・現行 v17 スキーマを `CREATE TABLE IF NOT EXISTS` の冪等 baseline V1 に凍結）。現行 `src/db/migrations.ts` の DROP 再作成方式は**完全撤廃**。`DbError` を持つ。全リポジトリ署名に `UserId` を通す。

- **`yuuka-web`**（Web 担当・[rpt-axum-web-runtime](verification/rpt-axum-web-runtime.md)）
  **axum 0.8.9 / tower-http 0.6.x**。ルータ・middleware・静的配信（`ServeDir` + `precompressed_br/gzip` + SPA フォールバック）・`/ws/chat`（`axum::extract::ws`）。認可レベル（none/user/admin）は `FromRequestParts` 実装のカスタム型（`AuthenticatedUser` / `AdminUser`）で**型強制**、任意認証は `OptionalFromRequestParts`。Cookie(`__Host-yuuka-session`)＋Bearer(desktop) の二経路。`WebError` を持ち、`IntoResponse` を手実装して内部 Display をクライアントに漏らさない。現行 `src/server.ts` の生 http 実装を置換。

- **`yuuka-discord`**（bot 担当・[rpt-discord-serenity-twilight](verification/rpt-discord-serenity-twilight.md)）
  **twilight 0.17.1**（`twilight-gateway`/`twilight-http`/`twilight-model`/`twilight-cache-inmemory`/`twilight-util`）。1 プロセスで多数のボットトークンを扱うマルチテナント。`Shard` を **caller 駆動の poll loop** で回し、supervisor と統合してテナント別バックオフ／再起動を掛ける。`DiscordError` を持つ。現行 `src/bot.ts`（discord.js）を置換。

- **`yuuka-gemini`**（AI 担当・[rpt-gemini-design-verify](verification/rpt-gemini-design-verify.md), [rpt-gemini-rest-crates](verification/rpt-gemini-rest-crates.md)）
  **reqwest 0.13 + serde + thiserror の自前薄ラッパ**（公式 Rust SDK 不在）。classic `generateContent` v1beta を camelCase struct に厳密固定して 1:1 移植。429 / `RetryInfo.retryDelay` を `GeminiError` の variant に載せ、現行 `generateWithRetry` のバックオフを移植。Function Calling（`functionDeclarations` / `toolConfig` / `functionCall`↔`functionResponse` 往復・並行呼び出し）を実装。

- **`yuuka-tools`**（拡張点・[rpt-plugins-wasm-extism](verification/rpt-plugins-wasm-extism.md), [rpt-mcp-rmcp](verification/rpt-mcp-rmcp.md)）
  **`ToolProvider` トレイト＋中央レジストリ**（`HashMap<String, Arc<dyn ToolProvider>>`、ソース別 namespace 接頭辞で衝突回避）。3 系統：**NativeProvider**（内蔵 Rust トレイト、現行 `src/functions/*` 相当）、**McpProvider**（**rmcp 2.0.0** 公式 SDK client、現行 `mcpDynamic`/`mcpClient` を吸収）、**WasmProvider**（**Extism 1.30.0** / wasmtime 上・deny-by-default manifest で**非信頼なユーザー製プラグイン**を実行）。動的 `.so` は非信頼コードに**不採用**。`ToolContext` に `UserId` を載せ能力スコープ／データ分離を強制。`PluginError` を持つ。絶対制約 4（拡張性）の実体。

- **`yuuka-services`**（services 担当）
  現行 `src/services/` の常駐タスク群（reminder / report / briefing / backup / birthday / payment / playbook / todo recurrence / metrics / clipboard cleanup / synapse 連携）を移植。各サービスは cron スケジュールを持つ長寿命タスクとして `yuuka-supervisor` の JoinSet 配下で回る。cron は HTTP 非公開。

- **`yuuka-supervisor`（bin `yuuka`）**（[rpt-resilience-tokio-backon-recloser](verification/rpt-resilience-tokio-backon-recloser.md)）
  エントリポイント兼監督。**自前 `tokio::task::JoinSet` + `join_next()` ループ**で web/discord/services/db-writer 等を束ね、`Err(JoinError)`（`is_panic()`）を検知して当該タスクを**指数バックオフで再 spawn**。停止協調は **tokio-graceful-shutdown 0.19.3**（サブシステムツリー＋SIGTERM 伝播）だが、「再起動」ロジックは自前補完する。`AppError`（最上位）を持つ。絶対制約 2（自己復帰）の実体。

- **`yuuka-synapse` / `yuuka-crawler`**
  既存 Rust デーモンを workspace へ取り込む。当面は互換のため**同一 workspace 内の別クレート**として維持しつつ、`yuuka-supervisor` 配下の **supervised task**（あるいは子プロセスから同一バイナリ内タスクへ）に統合していく。将来は同一バイナリ内タスク化も検討（下記の IPC 削減利点を参照）。

- **`xtask`**（[rpt-typegen-tsrs-utoipa](verification/rpt-typegen-tsrs-utoipa.md)）
  **ts-rs 12.0.1** による型生成（`cargo xtask gen-types` → `frontend/src/lib/api/generated.ts`）、CI ドリフト検査（`--check` で乖離時に非ゼロ終了）、`cargo-deny`/`clippy` 補助。build.rs ではなく xtask パターン（副作用でワークツリーを汚さず再現性を確保）。絶対制約 5（型連携）の生成器。

#### クレート間依存の向き（循環禁止）

依存は「下流が上流に依存しない DAG」を厳守する（[design-checkpoint §Phase F](design-checkpoint.md)）。

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
- **循環依存は禁止**。web が db/ai に直接依存せず、`yuuka-core` に定義したトレイト（`SessionStore` / `UserRepo` 等）越しに `AppState` から受け取る設計（[design-checkpoint §C.1](design-checkpoint.md)）で、web crate をドメインロジックのコンパイル時間・依存に巻き込まず、テストで in-memory 実装を差せるようにする。
- クレート＝層＝エラー型が一致する（`yuuka-db`→`DbError`、`yuuka-web`→`WebError`、…）ため、**エラーの伝播方向も依存の向きに一致**し、`#[from]` は真の層境界のみで使える（絶対制約 1）。

### 2.2 マルチスレッド実行モデル

ランタイムは **Tokio 1.52.x の multi-thread（work-stealing）スケジューラ**、`panic = "unwind"` を厳守する（`abort` にするとパニック隔離が無効化＝プロセス即死のため。[rpt-resilience §1](verification/rpt-resilience-tokio-backon-recloser.md)）。

**単一プロセス内での同居構成**：

- **`yuuka-supervisor`（bin `yuuka`）が最上位タスク**。起動時に config/secret を厳密検証（不備なら**ここだけ fail-fast**＝絶対制約 2 の「本当に致命的」）、その後 JoinSet に以下の長寿命タスクを spawn する：
  - **web サーバ**（axum、`/ws/chat` を含む）
  - **discord bot**（twilight、テナントごとの Shard poll loop）
  - **services 群**（cron/reminder/report/briefing/backup/… の各常駐タスク）
  - **単一 writer actor**（DB 書き込みを直列化する 1 タスク・1 コネクション）
  - **synapse / crawler**（当面は supervised、将来は同一バイナリ内タスク）
- **read pool** は各タスクから共有され、読み取りは `spawn_blocking` でブロッキングスレッドプールに逃がす（rusqlite は同期 API）。書き込みは必ず単一 writer actor へメッセージ送信し直列化する。
- **work-stealing** により、web リクエスト処理・bot イベント処理・cron タスクが**空いたワーカースレッドに自動分散**され、現行 Node の実質シングルスレッド寄り実行に対しマルチコアを活用できる（絶対制約 3）。
- **パニック隔離**：`spawn` したタスク内のパニックはプロセスを殺さず、`JoinSet::join_next()` が `Err(JoinError)`（`is_panic()==true`）として返す（tokio 公式）。supervisor はこれを検知し当該タスクのみ**指数バックオフで再 spawn**する。`JoinSet::join_all()` は使わない（1 つの panic で全 abort されるため）。supervisor 本体の生存は別レイヤで保証する（[rpt-resilience §1](verification/rpt-resilience-tokio-backon-recloser.md)）。

**「Rust デーモンを子プロセス spawn」→「同一バイナリ内 supervised task」統合の利点**：

現行は synapse/crawler を子プロセスとして起動し、**改行区切り JSON-RPC over stdin/stdout** で通信している（`src/services/synapseEngine.ts` が `pending` Map で id 相関・タイムアウト・EPIPE 監視・自動再起動を手書き）。これを同一バイナリ内のタスクへ統合すると：

- **IPC シリアライズの削減**：JSON エンコード／デコードとパイプ往復が消え、Rust の構造体を関数呼び出し・チャネル送受信で直接授受できる（型付き・ゼロコピー寄り）。
- **失敗モードの単純化**：プロセス境界の `exit`/`error`/EPIPE・タイムアウト・id 相関という失敗モードが消え、`JoinSet` の統一された `JoinError` 監督に一本化される。
- **監督の一元化**：テナント別・サービス別のバックオフ／劣化縮退ポリシーを supervisor が一括管理でき、子プロセス個別の再起動ロジックを手書きしなくてよい。
- **移行の安全性**：ただし synapse が既に rusqlite を read-only で使う（[rpt-dual-sqlite-hazard](verification/rpt-dual-sqlite-hazard.md)）ため、DB writer は必ず 1 つに集約する。移行期は「Node が全書き込み・Rust は read-only、カットオーバー時に一度だけ writer を Rust へ移譲」を最優先する（詳細は第3部デプロイ章）。

> **劣化縮退（絶対制約 2）**：synapse ダウン→直近履歴のみ（現行踏襲）、Redis ダウン→インメモリセッション（移行期はプロセスローカルで他系に不可視＝断続ログアウトに注意）。回復可能／致命的を型・ポリシーで峻別し、致命的は起動時 config/secret 不備のみとする。

---

## 3. 技術選定サマリ表

以下は [00-decisions §技術選定サマリ](00-decisions.md) を本計画の読者向けに要約・再掲したものである。**却下案の詳細な比較や各バージョンの根拠は 00-decisions と後続の各セクション（第2部・第3部）に譲る**。ここでは「何を・どのバージョンで採るか」を一望する用途に絞る。

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

> **未解決の決定事項**（ユーザー判断が要る点：フロント型生成の ts-rs か utoipa か、DB の rusqlite か sqlx か、Gemini API 面、edition 2021/2024、プラグイン初期スコープ）は [00-decisions §未解決の決定事項](00-decisions.md) にまとまっている。既定値（ts-rs / rusqlite / classic generateContent / Native+MCP 先行）を本書は採用している。

---

> 対象: セクション4「厳格エラーアーキテクチャ」・セクション5「自己復帰／スーパーバイザ設計」。
> 上位決定 [`00-decisions.md`](00-decisions.md)（絶対制約1・2）に**厳密整合**。ここで技術選定を再決定はしない。数値は全て一次ソース照合済み（各節末のリンク参照）。
> 一次照合レポート: [errors-thiserror](verification/rpt-errors-thiserror.md) / [clippy-cargodeny](verification/rpt-clippy-cargodeny.md) / [resilience](verification/rpt-resilience-tokio-backon-recloser.md)。

---

## 4. 厳格エラーアーキテクチャ（anyhow 禁止の実現と機械的強制）

**目的（絶対制約1）**: `anyhow`/`eyre`/`color-eyre`/`Box<dyn Error>` 等の型消去＝「握り潰し」を、規約ではなく**コンパイラと CI で機械的に不可能にする**。エラーはすべて `thiserror 2.0.18` の**層別・具体列挙型**で表現し、層をまたぐ変換は明示化する。

### 4.1 層別エラー分類体系

各層（クレート／モジュール境界）ごとに 1 つの具体エラー enum を持たせる。下位層の enum は上位層の enum のバリアントとして**明示的に**畳み込む（§4.3 の「層境界のみ `#[from]`」を参照）。

| エラー型 | 所属層 | 主な原因 | fail-fast? |
|---|---|---|---|
| `ConfigError` | 起動・設定 | config.yaml 欠落／型不一致／必須 secret 不備 | **致命的**（§5.6） |
| `DbError` | DB ドライバ（rusqlite） | SQLITE_BUSY、制約違反、I/O、シリアライズ | 回復可能（リトライ／縮退） |
| `RepoError` | リポジトリ（データアクセス） | `DbError` の意味付け、`NotFound`、`UserId` 欠落 | 回復可能 |
| `AuthError` | 認証・認可 | セッション無効、権限不足、トークン不正 | 回復可能（401/403 化） |
| `ValidationError` | 入力検証 | DTO 制約違反、範囲外、必須欠落 | 回復可能（400 化） |
| `GeminiError` | Gemini 薄ラッパ | 429/`RetryInfo`、5xx、パース失敗、safety block | 回復可能（backon＋ブレーカ） |
| `DiscordError` | Discord（twilight） | ゲートウェイ断、HTTP 4xx/5xx、レート制限 | 回復可能（テナント別再起動） |
| `PluginError` | ツール／プラグイン | Native/MCP/WASM 呼び出し失敗、能力スコープ違反、タイムアウト | 回復可能（当該ツールのみ失敗） |
| `IpcError` | プロセス間・synapse | synapse 接続断、プロトコル不整合 | 回復可能（縮退＝直近履歴のみ） |
| `WebError` | Web 面（axum ハンドラ） | 上記各層を HTTP へ写像する統合型 | 回復可能（`IntoResponse`） |

原則:
- **1 層 = 1 enum**。層内の全失敗モードをバリアントで列挙する（`Other(String)` 的な逃げ道を作らない）。
- クレート／モジュール**境界を越えて公開**する enum には `#[non_exhaustive]` を付ける（§4.4）。
- `WebError` は Web クレート内に置き、`Auth/Validation/Repo/Gemini/Plugin/Ipc` を**明示 match で**畳み込む統合層とする（§4.5 の `IntoResponse` で網羅 match）。

### 4.2 thiserror 2.0.18 での定義例

`thiserror` は**手続きマクロのみ**を提供する軽量クレート（Display/Error/From の導出）。ランタイム機能は最小限、という設計思想を前提にする。MSRV = Rust 1.68、edition 2021。属性は 2.0.18 の docs.rs / GitHub README で確認済み。

```rust
// crates/db/src/error.rs — 最下段（ドライバ層）
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DbError {
    // 真の層境界: rusqlite -> DbError は #[from] で自動 From 生成。
    // #[from] は #[source] を暗黙に含むため #[source] は書かない。
    #[error("sqlite operation failed")]
    Sqlite(#[from] rusqlite::Error),

    #[error("write transaction is busy after retries")]
    Busy,

    // spawn_blocking の join 失敗など。transparent は「最下段で素通し」限定。
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}
```

```rust
// crates/repo/src/error.rs — 一段上（意味付け層）
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RepoError {
    // 層境界: DbError -> RepoError も #[from]。
    #[error("database error")]
    Db(#[from] DbError),

    // 追加コンテキストを持つバリアントは #[from] 不可（後述の衝突制約）。
    #[error("record not found: {kind} id={id}")]
    NotFound { kind: &'static str, id: i64 },

    // データ分離キー欠落を型で顕在化（UserId newtype と連動）。
    #[error("user scope missing for repository operation")]
    UserScopeMissing,
}
```

```rust
// crates/gemini/src/error.rs — 外部依存ラッパ（429/RetryInfo を掌握）
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GeminiError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    // 現行の rate-limit バックオフを 1:1 移植するための構造化バリアント。
    // retry_after は RetryInfo(retryDelay) 由来。§5.3 のブレーカ/backon が参照。
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<std::time::Duration> },

    #[error("gemini returned status {status}")]
    Status { status: u16 },

    #[error("response decode failed")]
    Decode { #[source] source: serde_json::Error },

    #[error("content blocked by safety filter")]
    SafetyBlocked,
}
```

属性の使い分け（一次確認済み）:
- `#[error("{field}")]` → `write!("{}", self.field)` / `#[error("{0}")]` → tuple 要素。`:?` で Debug 補間。
- **`#[from]`**: 付与バリアントごとに `From` impl を自動生成し、**そのバリアントはソースエラー（＋任意で `#[backtrace]` フィールド）以外のフィールドを持てない**。かつ `#[source]` を暗黙に含む（両方書かない）。
- **`#[source]`**: 下位エラーを `Error::source()` として公開しつつ、**追加コンテキストのフィールドを同居**させたいとき（`From` は生成されない）。
- **`#[error(transparent)]`**: Display と source を下位へ素通し（メッセージを足さない）。**最下段の "そのまま素通し" 用途に限定**。上位で多用するとコンテキストが失われ握り潰しに近づく。
- **`#[non_exhaustive]`**: これは thiserror ではなく**標準 Rust の言語属性**（§4.4）。derive の上に併記する。

> ⚠️ **1.x → 2.0 の非互換**: 2.0 で `#[error("{x}")]` のフィールド補間解決が厳格化された。1.x 前提のコード片をそのまま貼らない。新規は 2.0.18 固定で問題なし。

### 4.3 `#[from]` は層境界のみ・層跨ぎは明示 match

**規則**: `#[from]` を使ってよいのは「**下位層 enum → 直上位層 enum**」の 1 段の畳み込みだけ。それ以外（複数下位型の吸い込み、層を 2 段以上スキップする変換、HTTP 面への写像）は**明示的な `match`／`map_err` で変換**する。

理由:
1. **握り潰し回避**: `#[from]` を多用すると「どこでどの層のエラーが混入したか」の意味論が薄れ、anyhow 的な "何でも吸い込む" 型に退化する。層境界で意図的に変換することで、禁止方針（絶対制約1）と構造的に整合させる。
2. **`#[from]` の衝突制約**: 同じ下位型（例: `std::io::Error`）を複数バリアントで `#[from]` すると `From` impl が衝突し**コンパイル不能**。素通しが複数必要なら片方を `#[source]` に落とすか、newtype で下位型を分ける。→ そもそも「1 バリアント = 1 ソース型」が実質前提なので、層をまたぐ多対多の吸い込みは `#[from]` では表現できない。

明示変換の例（層を跨ぐので `#[from]` を使わない）:

```rust
// Web ハンドラ内: AuthError と ValidationError を WebError へ明示畳み込み。
// これらは異なる層なので from ではなく match で意味付けする。
let user = authenticate(&parts)
    .map_err(|e: AuthError| match e {
        AuthError::SessionInvalid | AuthError::TokenMalformed => WebError::Unauthorized,
        AuthError::Forbidden => WebError::Forbidden,
    })?;
```

**Result 型エイリアス方針**: 各クレートで `pub type Result<T, E = ThisLayerError> = std::result::Result<T, E>;` を定義してよい。ただし**デフォルト型引数を 1 種類に固定**し、別層のエラーを返す関数では**明示的に完全型（`std::result::Result<T, OtherError>`）を書く**こと。「万能 `Result`」を作らない（それが anyhow の入口になる）。

### 4.4 `#[non_exhaustive]` の運用

- **標準 Rust の言語属性**（安定版・Rust Reference 記載）。thiserror とは独立。
- **クレート／モジュール境界を越えて公開**するエラー enum に付ける。付けると**定義クレート外の `match` はワイルドカード `_ =>` が必須**になり、将来のバリアント追加が下位コードを壊さない（非破壊）。
- **定義クレート内では効果なし**（自クレート内の網羅 match は従来どおり可）。→ これが §4.5 の設計と噛み合う: **`IntoResponse` を `WebError` と同一クレートに置けば `#[non_exhaustive]` でも完全網羅 match が書け**、バリアント追加漏れをコンパイルエラーで検知できる。
- 諸刃の剣: 別クレートで `WebError` を完全網羅マッピングしたい消費側には `_ =>` が強制され漏れが隠れる。→ **HTTP 写像は必ず定義クレート内に置く**ことでこの罠を回避する（下記）。

### 4.5 axum `IntoResponse` の手書き写像

**役割分担**: 型定義は `thiserror`、HTTP 写像は**手書きの `IntoResponse`**。`thiserror` は HTTP 写像を一切提供しない。`WebError` は `IntoResponse` と**同一クレート**に置き、`match self` を**網羅**（ワイルドカード無し）で書く。これによりバリアント追加時にコンパイルエラーで写像漏れを検知する。

```rust
// crates/web/src/error.rs — WebError と IntoResponse を同一クレートに置く
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum WebError {          // ← ここでは #[non_exhaustive] を付けない
    #[error("unauthorized")] Unauthorized,     //   （同一クレート網羅 match を保つため）
    #[error("forbidden")]    Forbidden,
    #[error("not found")]    NotFound,
    #[error("invalid request: {0}")] Validation(String),
    #[error("upstream unavailable")] Upstream,   // Gemini/Discord/synapse 障害の丸め
    #[error("internal error")]       Internal,   // DbError/RepoError 等の機微を隠蔽
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        // 網羅 match（_ アームを書かない）→ バリアント追加漏れ = コンパイルエラー。
        let (status, client_msg) = match self {
            WebError::Unauthorized  => (StatusCode::UNAUTHORIZED, "unauthorized"),
            WebError::Forbidden     => (StatusCode::FORBIDDEN, "forbidden"),
            WebError::NotFound      => (StatusCode::NOT_FOUND, "not found"),
            // Validation のみ Display を露出してよい（ユーザー入力由来・機微なし）。
            WebError::Validation(ref m) => {
                return (StatusCode::BAD_REQUEST, m.clone()).into_response();
            }
            WebError::Upstream      => (StatusCode::BAD_GATEWAY, "upstream unavailable"),
            // 内部エラーは Display をクライアントへ漏らさず "internal" に丸める。
            WebError::Internal      => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        (status, client_msg).into_response()
    }
}
```

内部エラー（`DbError`/`RepoError` 等）から `WebError` への写像は**明示関数**で行い、`Db(_)` 系は必ず `WebError::Internal` に丸めて Display を漏らさない:

```rust
fn map_repo(e: RepoError) -> WebError {
    match e {
        RepoError::NotFound { .. }     => WebError::NotFound,
        RepoError::UserScopeMissing    => WebError::Forbidden,
        RepoError::Db(_)               => WebError::Internal, // 機微を丸める
    }
}
```

落とし穴（一次確認済み）:
- ハンドラ戻り値を `Result<impl IntoResponse, E>` にすると `?` の型推論が壊れやすい。**戻り値は具象化**（`Result<Json<T>, WebError>` 等）する。
- axum は **0.8.9**（`#[async_trait]` 不要・RPITIT）。パターンは 0.7/0.8 共通。詳細な extractor 設計は [axum-web-runtime](verification/rpt-axum-web-runtime.md) 参照。

### 4.6 機械的強制の完全レシピ（コピペ可）

検証環境: Rust stable **1.96.1** / cargo-deny **0.19.9**。以下は一次ソース（clippy 生 HTML / Cargo Book / cargo-deny 公式テンプレート）で検証済み。

**(a) root `Cargo.toml` — restriction lint を 8 個「個別に」deny**

```toml
[workspace.lints.clippy]
# 注意: [workspace.lints.clippy] 内では clippy:: プレフィックス無しの裸の lint 名。
# 8 個すべて restriction グループ・デフォルト allow。個別列挙が必須。
unwrap_used        = "deny"
expect_used        = "deny"
panic              = "deny"
todo               = "deny"
unimplemented      = "deny"
unreachable        = "deny"
indexing_slicing   = "deny"
panic_in_result_fn = "deny"
```

> **一括禁止しない理由**: `restriction` グループの全体有効化（`clippy::restriction = "deny"`）は**公式が明確に禁止**している。グループには相互矛盾する lint が含まれ、`blanket_clippy_restriction_lints` 専用 lint で警告される。**必ず 8 個を個別列挙**する。`panic` 単独 lint は `unreachable!` 等を捕捉しないため、8 個併用で初めてパニック経路が網羅される。

**(b) 各メンバー crate の `Cargo.toml` — workspace lint をオプトイン**

```toml
[lints]
workspace = true
```

> **メンバーは自動継承しない**（Rust 1.74 安定・RFC 3389）。新規 crate 追加時にこの 2 行を忘れると workspace lint が丸ごと無視される。かつ `[lints] workspace = true` と個別 lint を**同一テーブルに併記するのはハードエラー**。CI で「全メンバーがこの行を持つか」を検査する仕組みを併設すると安全。

**(c) `clippy.toml` — テストコードの緩和**

```toml
# unwrap_used / expect_used / indexing_slicing はテストでも発火する。
# テスト内に限り緩和（本番コードは deny のまま）。
allow-unwrap-in-tests           = true
allow-expect-in-tests           = true
allow-indexing-slicing-in-tests = true
```

**(d) `deny.toml` — anyhow/eyre/color-eyre を `[bans]` で全面 BAN**

```toml
[bans]
multiple-versions = "warn"
wildcards         = "allow"

# フィールド名は現行の `crate`（PackageSpec）。旧 `name`/`version` 形式は非推奨。
# eyre を BAN しても color-eyre は別 crate 名なので個別に列挙する。
deny = [
    { crate = "anyhow",     reason = "banned: 具体列挙型(thiserror)のみ許容。型消去禁止" },
    { crate = "eyre",       reason = "banned: dynamic error-context crate 禁止" },
    { crate = "color-eyre", reason = "banned: eyre 派生" },
    { crate = "backoff",    reason = "banned: RUSTSEC-2025-0012 非メンテ。backon を使用" },
]
```

> `[bans]` は依存グラフ（`cargo metadata`）全体を見るので、推移的依存に anyhow 等が混入しても捕捉する（意図通り）。本当に必要な wrapper 経由の利用を許すなら `wrappers = [...]` を使うが、今回は全面 BAN なので不要。`backoff` の BAN は §5.3 と連動（RUSTSEC-2025-0012）。

**(e) CI ゲーティングコマンド**

```bash
# clippy: 全ターゲット・全フィーチャで警告をエラー化（manifest の deny と二重化）
cargo clippy --all-targets --all-features -- -D warnings

# cargo-deny: 全チェック（advisories/bans/licenses/sources）
cargo deny check
# anyhow/eyre/backoff ゲートだけ高速に回すなら bans 単独（ネットワーク不要）:
cargo deny check bans
```

CI 落とし穴:
- **`cargo build` では clippy lint は発火しない**。必ず `cargo clippy` を回す（manifest の `[workspace.lints.clippy]` は clippy 実行時に効く）。
- `--all-targets` は **doctest を含まない**。doctest 内の `unwrap` も塞ぐなら別途 `cargo test --doc` 系／`RUSTDOCFLAGS` を検討。
- `cargo deny check` の `advisories` はネットワークが要る。`bans` はネットワーク不要なので、握り潰し crate ゲートは `cargo deny check bans` に分離して高速化できる。

詳細は [clippy-cargodeny](verification/rpt-clippy-cargodeny.md) を参照。

### 4.7 `panic = "unwind"` を保つ理由（セクション5との接続）

Cargo profile を `panic = "abort"` にすると、**tokio の spawn 境界によるパニック隔離が無効化され、単一タスクの panic でプロセスが即死**する。これは絶対制約2（些細な障害で落ちない）と真っ向から対立する。したがって**全プロファイルで `panic = "unwind"`（デフォルト）を厳守**する。

これが §5 の supervisor の前提: spawn したタスクの panic はプロセスを殺さず `JoinError(is_panic)` として観測でき、それを検知して**指数バックオフで再起動**できる。restriction lint（§4.6a）で明示的な `panic!`/`unwrap`/`unreachable!` はコンパイル時に排除しつつ、**それでも起きうる予期せぬ panic を実行時に unwind で受け止めて再起動する**——この二段構えが自己復帰の土台になる。

---

## 5. 自己復帰・スーパーバイザ設計（常時稼働）

**目的（絶対制約2）**: 些細な障害でプロセス全体を落とさない。全長寿命サービスを監督下に置き、panic/失敗を検知して**指数バックオフで個別再起動**、外部依存には**サーキットブレーカ**、依存断時は**劣化縮退**。fail-fast は**起動時の config/secret 不備のみ**。

検証環境: tokio **1.52.x** / tokio-graceful-shutdown **0.19.3** / backon **1.6.0** / recloser **1.4.0**。全て一次ソース照合済み（[resilience](verification/rpt-resilience-tokio-backon-recloser.md)）。

### 5.1 監督ツリー（supervision tree）の対象

以下の全長寿命サービスを**監督下タスク**にする。各々が独立に落ち・独立に再起動される（1 つの障害が他へ波及しない）:

- **web**（axum サーバ）
- **discord: テナント別ゲートウェイ接続**（twilight `Shard` の caller 駆動 poll loop を各テナント 1 タスク）
- **synapse**（IPC クライアント）
- **crawler**
- **各 cron ジョブ**（スケジュール実行のループ）
- **redis**（セッションストア接続）

Discord がテナント別に独立タスクなのは、twilight の caller 駆動 poll loop を supervisor と統合し**テナント別バックオフ／再起動**を掛けられるため（[discord](verification/rpt-discord-serenity-twilight.md)、[decisions §Discord](00-decisions.md)）。

### 5.2 自前 JoinSet supervisor + join_next ループ

**確定した土台事実（一次確認）**: `tokio::spawn` したタスク内の panic は**プロセスを殺さず当該タスクに隔離**され、`JoinHandle`／`JoinSet::join_next()` が `Err(JoinError)` を返す。`JoinError::is_panic()` が `true`、`into_panic()` でペイロードを取得できる。親へ伝播するかは**完全にプログラマ制御**（`resume_unwind` を呼ばない限り伝播しない）。→ `join_next` ループで検知して再 spawn する設計が成立する。

**設計**: `tokio::task::JoinSet`（標準 API・`rt` フィーチャのみ・unstable 不要）を `join_next()` で回し、`Err(JoinError)` を検知したら**該当サービスを指数バックオフで再 spawn** する自前スーパーバイザ。

厳守事項（一次確認済みの落とし穴）:
- **`JoinSet::join_all()` は使わない**。docs 明記: 1 つでも `JoinError` で失敗すると `join_all` は **panic し残り全タスクを cancel** する。監視では必ず `join_next()` を手動ループして各エラーを個別処理する。
- **`JoinSet` を drop すると配下タスクが即 abort**。supervisor 本体の生存を別レイヤで保証する（supervisor が落ちれば配下全滅）。
- **`panic = "unwind"` 必須**（§4.7）。`abort` だと隔離が無効化。
- `catch_unwind` を `.await` 跨ぎで使わない（`UnwindSafe` 制約で破綻しやすい）。パニック隔離は spawn 境界に委ねる。

```rust
use std::time::Duration;
use tokio::task::{JoinSet, JoinError};
use backon::{ExponentialBuilder, BackoffBuilder};

/// 監督対象サービスの識別子。再起動時にどれを起こし直すか判別する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Service {
    Web,
    Discord(TenantId),
    Synapse,
    Crawler,
    Cron(CronId),
    Redis,
}

/// 各サービスの本体（長寿命ループ）。Ok(()) は「意図的停止」、
/// Err はサービス固有の回復可能エラー。panic は JoinError として捕捉される。
async fn run_service(svc: Service, ctx: AppCtx) -> Result<(), ServiceError> {
    match svc {
        Service::Web           => web::serve(ctx).await,
        Service::Discord(t)    => discord::run_tenant(t, ctx).await,
        Service::Synapse       => synapse::run(ctx).await,
        Service::Crawler       => crawler::run(ctx).await,
        Service::Cron(id)      => cron::run(id, ctx).await,
        Service::Redis         => redis::run(ctx).await,
    }
}

/// supervisor ループ: join_next で終了/panic を検知し、指数バックオフで再 spawn。
async fn supervise(services: Vec<Service>, ctx: AppCtx, shutdown: ShutdownGuard) {
    let mut set: JoinSet<(Service, Result<(), ServiceError>)> = JoinSet::new();

    // 各サービスの再起動ごとの backoff 状態を保持（サービス別に独立）。
    let mut backoff: std::collections::HashMap<Service, _> = Default::default();

    let spawn_one = |set: &mut JoinSet<_>, svc: Service, ctx: AppCtx| {
        set.spawn(async move {
            // run_service 内の panic はここで JoinError 化される（プロセスは死なない）。
            (svc, run_service(svc, ctx).await)
        });
    };

    for svc in services.iter().copied() {
        spawn_one(&mut set, svc, ctx.clone());
    }

    while let Some(joined) = set.join_next().await {
        // 停止協調中なら再起動しない（§5.4）。
        if shutdown.is_shutting_down() {
            continue;
        }

        let svc = match joined {
            // 正常な JoinResult（サービスが Ok/Err を返して終了）。
            Ok((svc, Ok(()))) => {
                // 意図的停止: cron 一巡完了など。ポリシーに応じ再起動 or 放置。
                svc
            }
            Ok((svc, Err(e))) => {
                tracing::warn!(?svc, error = %e, "service returned recoverable error");
                svc
            }
            // タスクが panic した（is_panic）。プロセスは生きている。
            Err(join_err) if join_err.is_panic() => {
                // JoinError には Service が乗らないため、id() 等で対応付ける実装にする。
                let svc = resolve_service_of(&join_err);
                tracing::error!(?svc, "service PANICKED, isolated by spawn boundary");
                svc
            }
            // abort されたタスク（drop 等）。停止協調なら上で continue 済み。
            Err(_aborted) => continue,
        };

        // サービス別の指数バックオフ（ジッタ明示）で再 spawn。
        let bo = backoff
            .entry(svc)
            .or_insert_with(|| {
                ExponentialBuilder::default()
                    .with_jitter()                       // ← 明示 ON 必須（thundering herd 回避）
                    .with_min_delay(Duration::from_millis(200))
                    .with_max_delay(Duration::from_secs(30))
                    .build()
            });
        let delay = bo.next().unwrap_or(Duration::from_secs(30));
        tracing::info!(?svc, ?delay, "restarting service after backoff");
        tokio::time::sleep(delay).await;

        spawn_one(&mut set, svc, ctx.clone());
    }
}
```

> 実装メモ: `JoinError` にサービス識別子は乗らないので、`set.spawn` の `AbortHandle`/task id とサービスの対応表を別に持ち `resolve_service_of` で引く（上の擬似コードはその存在を前提にしている）。サービスが**安定稼働したら backoff をリセット**する（一定時間 Err なく回ったら `backoff.remove(&svc)`）と、断続障害で遅延が際限なく伸びるのを防げる。

### 5.3 リトライ: backon 1.6.0（ジッタ明示）と backoff 採用禁止

- **`backon 1.6.0`** を retry + 指数バックオフ + ジッタの第一選択とする。async/blocking 両対応、指数/定数/フィボナッチ、Retry-After 動的バックオフ、no-std/wasm 対応。
- **ジッタはデフォルト無効の戦略があるため `ExponentialBuilder::default().with_jitter()` を明示的に ON** にする（thundering herd 回避に必須）。§5.2 の supervisor でも §5.5 の外部依存リトライでも同様。
- **`backoff` クレートは採用禁止**: **RUSTSEC-2025-0012**（2025-03-07 発行）で公式に "no longer actively maintained" と宣言され、**代替として `backon` が明示推奨**されている。最終版 0.4.0 は 2021-12-14 で更新停止。`cargo audit` / `cargo deny check advisories` が警告を出す。→ §4.6(d) の `deny.toml` で crate 名 BAN 済み。

Gemini リトライは `GeminiError::RateLimited { retry_after }`（§4.2）の `retry_after` を backon の動的バックオフに渡し、現行の rate-limit 挙動を 1:1 で再現する。

### 5.4 tokio-graceful-shutdown 0.19.3 による停止協調（SIGTERM 伝播）

- **`tokio-graceful-shutdown 0.19.3`** をサブシステムツリー＋graceful shutdown 伝播に採用。SIGTERM 受信を各サブシステムへ伝播し、順序立てた停止を行う。
- **ただし「再起動」ロジックは自前補完**である点を明記する。tgs が提供するのは主に「**停止協調 + エラー伝播**」であり、Erlang 的な individual restart supervisor ではない（subsystem がエラー/panic したら**ツリーを畳んで graceful shutdown** する型）。→ **常時稼働のための個別再起動は §5.2 の自前 JoinSet supervisor が担い、tgs は停止フェーズの協調に用いる**、という役割分担にする。
- 0.x 系ゆえマイナー更新（0.17→0.18→0.19）で API 破壊があり得る。**バージョンをピン留め**し、更新時は CHANGELOG を確認する。
- supervisor ループは `ShutdownGuard`（上の擬似コードの `shutdown`）を参照し、停止協調中は**再起動しない**（`is_shutting_down()` で continue）。

### 5.5 外部依存ごとのサーキットブレーカ

外部依存（Gemini / Google / Discord）ごとに独立したサーキットブレーカを置き、連続失敗時に**即 fail（fast-fail）してバックオフ再試行の嵐を止める**。

- **`recloser 1.4.0`**: リングバッファ実装の並行サーキットブレーカ。Closed/Open/HalfOpen の 3 状態、`RecloserBuilder` で失敗率・バッファ長を設定、`AsyncRecloser` で futures 対応（`recloser.call(future)`）。2026 年に継続リリースされている本格クレート。
- **または自前 `AtomicU*` 状態機械**: ブレーカ生態系は backon ほど成熟しておらず、要件がシンプル（失敗率閾値＋open タイマ）なら数十〜百数十行の自前実装が**対等な選択肢**。0.x/準放置リスクを負いたくない場合はこちらが堅い（`failsafe` は機能成熟だが約 2 年更新停止のため非推奨）。
- 落とし穴: recloser の `AsyncRecloser` は「futures-aware」だが docs 上 **tokio 明示保証はない**（標準 futures で動作）。採用前に自タスク構成で軽く PoC 検証する。

ブレーカとリトライの合成: 各外部依存呼び出しは「**ブレーカ（open なら即エラー） → backon リトライ（ジッタ ON） → GeminiError/DiscordError で構造化**」の順に重ねる。ブレーカが open の間は backon を回さず即座に縮退（§5.6）へ落とす。

### 5.6 劣化縮退（degraded operation）の具体

依存が落ちても致命扱いにせず、機能を縮退して稼働を継続する:

- **synapse ダウン → 直近履歴のみで応答**（現行踏襲）。`IpcError` を捕捉し、フル履歴取得を諦めてローカルの直近分で継続する。
- **Redis ダウン → インメモリセッションへフォールバック**。`ServiceError`（redis 系）を捕捉し、プロセスローカルの in-memory ストアで受け付ける。
  - ⚠️ **移行期リスクの注記**: in-memory セッションは**プロセスローカルで他系（旧 Node バックエンド・他インスタンス）から不可視**。移行期は新旧バックエンドが共有 Redis の不透明トークンを参照する設計（[nginx-session-strangler](verification/rpt-nginx-session-strangler.md)、[decisions §デプロイ](00-decisions.md)）なので、Redis 断中の in-memory フォールバックは**断続ログアウト／セッション不一致**を生む。縮退はあくまで「完全ダウンよりまし」の一時措置と位置づけ、Redis 復帰を最優先で監視・再接続する。

### 5.7 「回復可能／致命的」の型・ポリシー境界

**fail-fast は起動時の config/secret 不備のみ**。それ以外は全て回復可能として supervisor/縮退で扱う。

- **致命的（fail-fast）= `ConfigError`（起動時）のみ**: config.yaml の欠落・型不一致・必須 secret 不備は、起動シーケンスで即座にプロセス終了（非ゼロ終了）させる。壊れた設定で中途半端に動くより即死が安全。secret は専用 DTO struct（機密をフィールドに存在させない）＋`secrecy` で扱う（[typegen](verification/rpt-typegen-tsrs-utoipa.md)、[decisions §機密](00-decisions.md)）。
- **回復可能 = 上記以外すべて**: `DbError`(BUSY 等)/`RepoError`/`AuthError`/`ValidationError`/`GeminiError`/`DiscordError`/`PluginError`/`IpcError`/`WebError`。これらは supervisor 再起動・backon リトライ・ブレーカ・劣化縮退・HTTP 4xx/5xx 化のいずれかで吸収し、**プロセス全体は落とさない**。

型・ポリシー境界の表現方針:
- 起動シーケンス（`main`）だけが `ConfigError` で早期 return して終了できる。**長寿命サービスのループ内では `ConfigError` を発生させない**（設定は起動時に検証済みの型付き構造体として渡す）。
- サービスループの戻り値型 `Result<(), ServiceError>` は**回復可能エラーのみ**を表す（致命エラーを混ぜない）。これにより「supervisor が受け取るエラー＝必ず再起動/縮退で対処可能」という不変条件を型で保証する。

```rust
// main の起動シーケンス: ここだけが fail-fast。
fn main() -> std::process::ExitCode {
    // config/secret 不備は即終了（唯一の致命ポイント）。
    let cfg = match Config::load_and_validate() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: config error: {e}");   // Display のみ、機微は出さない
            return std::process::ExitCode::FAILURE;
        }
    };
    // 以降はランタイムを起動し、supervise() へ。ここから先は「落ちない」。
    run_runtime(cfg)
}
```

---

### 参照レポート（一次ソース照合）
- [rpt-errors-thiserror](verification/rpt-errors-thiserror.md) — thiserror 2.0.18 / 属性構文 / non_exhaustive / IntoResponse
- [rpt-clippy-cargodeny](verification/rpt-clippy-cargodeny.md) — restriction lint 8 個 / workspace.lints / deny.toml bans / CI
- [rpt-resilience-tokio-backon-recloser](verification/rpt-resilience-tokio-backon-recloser.md) — JoinSet 隔離 / tgs / backon / recloser / backoff 禁止
- 上位決定: [00-decisions.md](00-decisions.md)（絶対制約1・2、§エラー処理、§自己復帰）

---

> 対象: Web ランタイム / ルーティング / 認証 / 静的配信 / WebSocket（第6部）と DB 層・マイグレーション・データ分離（第7部）。
> 上位決定は [`00-decisions.md`](00-decisions.md) に厳密整合。数値・型名は
> [`verification/rpt-axum-web-runtime.md`](verification/rpt-axum-web-runtime.md),
> [`rpt-db-sqlx-vs-rusqlite.md`](verification/rpt-db-sqlx-vs-rusqlite.md),
> [`rpt-migrations-sqlx-refinery.md`](verification/rpt-migrations-sqlx-refinery.md),
> [`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) の一次ソース照合に基づく。
> 現行不変条件は [`src/server.ts`](../../src/server.ts), [`src/server/routeRegistry.ts`](../../src/server/routeRegistry.ts),
> [`src/server/httpHelpers.ts`](../../src/server/httpHelpers.ts), [`src/db/database.ts`](../../src/db/database.ts),
> [`src/db/migrations.ts`](../../src/db/migrations.ts) から抽出。
>
> **絶対制約の再掲（[`00-decisions.md`](00-decisions.md) より）**: 厳格エラー（`thiserror` 具体列挙型のみ・`anyhow`/`eyre` 禁止）／常時稼働・自己復帰（致命的設定不備のみ fail-fast）／現行より高速・堅牢・マルチスレッド／カスタムモジュール拡張性／フロント⇄Rust 型の単一真実源。

---

## 6. Web ランタイム / ルーティング / 認証 / 静的配信 / WebSocket

### 6.0 クレート構成とバージョン運用ポリシー

| クレート | バージョン | feature（抜粋） | 役割 |
|---|---|---|---|
| `axum` | **0.8.9** | `ws`, `macros`, `http2`, `original-uri` | Web フレームワーク（hyper 1 の薄ラッパ・`tower::Service` 採用） |
| `hyper` | `^1.1`（axum 依存） | — | HTTP/1.1・HTTP/2 実装（**1.x 安定 API**・実務リスク低） |
| `tower` | `^0.5.2`（axum 依存） | `util` | ミドルウェア抽象（`Layer`/`Service`） |
| `tower-http` | **0.6.x 固定**（最新 0.6.11） | `fs`, `compression-br`, `compression-gzip`, `set-header`, `trace`, `cors`, `limit` | 静的配信・圧縮・ヘッダ・CORS |
| `tokio` | **1.52.x** | `rt-multi-thread`, `net`, `signal`, `macros` | 非同期ランタイム（`panic=unwind` 厳守） |
| `tokio-tungstenite`（間接） | axum `ws` に内包 | — | WebSocket（別クレート明示不要） |

**バージョン運用の絶対条件（[`rpt-axum-web-runtime.md`](verification/rpt-axum-web-runtime.md) §1, §3）**:

- **axum は 0.x（1.0 未到達）**。semver 上マイナー更新（0.7→0.8）に破壊的変更が入る前提で運用する。実例: 0.8 でパスパラメータ構文が `/:id` → `/{id}` へ変更された。**「マイナー＝メジャー」とみなし**、`Cargo.toml` は `axum = "0.8"` で 0.8 系のパッチ追随のみ許可、0.9 は移行タスクとして明示レビューする。
- **tower-http は 0.6.x に固定**。axum 0.8.9 は `tower-http = ^0.6.8` を pin しており、`ServeDir` を `Router` に組み込む際に両者が公開する `http`/`tower` 型が同一メジャーである必要がある。tower-http 0.7.0（2026-06-15 リリース・非常に新しい）は圧縮の identity/ワイルドカード処理変更や暗黙 feature 削除など破壊的変更を含み、axum 0.8 対応版のリリースを確認するまで**採用しない**（過去に `tokio-rs/axum#2416` で同型バージョン不整合の非互換事例あり）。`deny.toml` に tower-http 0.7 系を `[[bans.deny]]` で暫定禁止しておくと事故を防げる。

**エラー写像の原則（[`00-decisions.md`](00-decisions.md) エラー処理節と整合）**: HTTP レイヤの `WebError`（`thiserror` 列挙型）は**手書き `IntoResponse`** で写像する。`DbError`/`RepoError`/`AuthError` の内部 `Display` はクライアントへ漏らさず、`401`/`403`/`404`/`413`/`500` へ丸める（内部詳細は `tracing` にのみ出す）。`IntoResponse` の `match` は**同一クレート内で網羅**させ、バリアント追加漏れをコンパイルエラーで検知する（`#[non_exhaustive]` は付けない）。

---

### 6.1 現行自作ルータの不変条件 → axum への写像

現行は `registerRoutes`/`dispatchRoute`（[`routeRegistry.ts`](../../src/server/routeRegistry.ts)）による自作ディスパッチで、以下の不変条件を持つ。**すべて axum で等価に保つ**。

| 現行の不変条件（出典） | 現行実装 | axum 写像 |
|---|---|---|
| 認可レベル `none`/`user`/`admin`（`RouteAuth`, contracts.ts:74） | `dispatchRoute` の逐次判定（401/403） | **`FromRequestParts` 実装型** `AuthenticatedUser`/`AdminUser`／任意は `Option<AuthenticatedUser>`（`OptionalFromRequestParts`） |
| `:param` パスパラメータ（`matchPath`, routeRegistry.ts:91） | 自作 split 照合 | axum `Path<T>` extractor（構文は `/{id}`） |
| CSRF（Origin/Referer/`Sec-Fetch-Site`、routeRegistry.ts:37-60） | POST/DELETE かつ `auth!="none"` で cross-site 拒否 | **tower レイヤ**（`CsrfLayer` 自作・下記 6.4） |
| 10MB ボディ上限（`MAX_BODY_BYTES`, routeRegistry.ts:111） | 手動カウント→413 | `DefaultBodyLimit::max(10 * 1024 * 1024)` |
| プロトタイプ汚染除去（`stripProtoKeys`, routeRegistry.ts:66） | `__proto__`/`constructor`/`prototype` 削除 | **serde が構造的に無効化**（`#[serde(deny_unknown_fields)]` + 型付き struct で未知キー拒否＝汚染面が存在しない） |
| HTTPS リダイレクト＋HSTS（server.ts:257-281） | 301＋`Strict-Transport-Security` | 起動時に proxy 終端前提を確認しつつ `SetResponseHeaderLayer` で HSTS 付与（リダイレクトは nginx 終端に委譲可） |
| CORS 限定反射（server.ts:284-310） | baseUrl 同一ホストのみ ACAO 反射 | `tower_http::cors::CorsLayer`（`AllowOrigin::predicate` で同一ホスト判定） |

**20以上のルートモジュールの再現（[`server.ts`](../../src/server.ts):39-60）**: 現行は `authRoutes`〜`desktopClientRoutes` の 20 モジュールを `registerRoutes` で 1 レジストリに集約している。Rust では**モジュール別に `Router` を返す関数**を定義し、`Router::merge`（または `nest`）で合成する。ツリーが `merge` で平坦・型安全に構成でき、`AppState`（`Arc` 共有）を `with_state` で一括注入できる。

```rust
// crates/web/src/routes/mod.rs
pub fn app_router(state: AppState) -> Router {
    Router::new()
        .merge(auth::routes())          // 認証・登録（§5.4）
        .merge(settings::routes())      // ユーザー設定・Google OAuth
        .merge(bot::routes())           // Bot インスタンス・共有
        .merge(bot_attribute::routes())
        .merge(member_request::routes())
        .merge(todo::routes())          // ToDo（§3.2）
        .merge(schedule::routes())
        .merge(timeline::routes())
        .merge(finance::routes())       // 家計・予算・支払い予定
        .merge(playbook::routes())
        .merge(credential::routes())    // パスワードマネージャ（§6）
        .merge(admin::routes())
        .merge(reminder::routes())
        .merge(personal::routes())      // ノート・クリップボード・連絡先
        .merge(persona::routes())
        .merge(mcp::routes())
        .merge(integrated::routes())
        .merge(webhook::routes())       // 外部 Webhook 受信（auth: none）
        .merge(delivery::routes())      // 朝報・日報・週報
        .merge(device_auth::routes())   // desktop OAuth デバイスフロー
        .merge(device_mgmt::routes())
        .merge(desktop_client::routes())// Windows 版バイナリ配布
        .with_state(state)
        // ── 全体レイヤ（後入れ=外側）──
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024)) // 10MB
        .layer(CsrfLayer::new(cfg.allowed_host.clone())) // Origin/Referer/Sec-Fetch-Site
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP)))
        .layer(security_headers_layer())                 // X-Content-Type-Options 他 + HSTS
        .layer(cors_layer(&cfg))
        .layer(TraceLayer::new_for_http())
}
```

各モジュールは薄い `routes() -> Router<AppState>` を返す:

```rust
// crates/web/src/routes/todo.rs
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/todos",      get(list_todos).post(create_todo))
        .route("/api/todos/{id}", get(get_todo).delete(delete_todo))
}

// 認可は引数の型が強制する（AuthenticatedUser を取れば user 必須）
async fn list_todos(user: AuthenticatedUser, State(st): State<AppState>)
    -> Result<Json<ApiResponse<Vec<TodoDto>>>, WebError> { /* ... */ }
```

**`ApiResponse<T>` エンベロープ**（現行 `sendJson` の `{ success, message, ... }` 形）は Rust の単一 struct として型化し、ts-rs で TS へ生成する（[`00-decisions.md`](00-decisions.md) フロント型連携節）。data ラッパの有無は現行踏襲。

---

### 6.2 認可の型強制（`FromRequestParts` extractor）

現行の `none`/`user`/`admin`（[`contracts.ts`](../../src/types/contracts.ts):74）を、**型を引数に取ること自体が認可条件**になるパターンへ移す（[`rpt-axum-web-runtime.md`](verification/rpt-axum-web-runtime.md) §5）。axum 0.8 は **RPITIT 化で `#[async_trait]` 不要**（旧 0.6/0.7 サンプルの流用は不可）。

```rust
// crates/web/src/auth/extract.rs
pub struct AuthenticatedUser(pub SessionUser); // user
pub struct AdminUser(pub SessionUser);         // admin

impl FromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = WebError; // 401/403 を返す IntoResponse を実装済み
    async fn from_request_parts(parts: &mut Parts, st: &AppState)
        -> Result<Self, Self::Rejection>
    {
        // Cookie(__Host-yuuka-session) → Bearer(desktop) の順で解決（6.3）
        let user = resolve_request_user(parts, st).await
            .ok_or(WebError::Unauthorized)?;   // → 401
        Ok(Self(user))
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = WebError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState)
        -> Result<Self, Self::Rejection>
    {
        let AuthenticatedUser(user) = AuthenticatedUser::from_request_parts(parts, st).await?;
        if user.role != Role::Admin { return Err(WebError::Forbidden); } // → 403
        Ok(Self(user))
    }
}

// 任意認証（公開ルートでもセッションがあれば拾う。現行 dispatchRoute の else 分岐相当）
impl OptionalFromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, st: &AppState)
        -> Result<Option<Self>, Self::Rejection>
    {
        Ok(resolve_request_user(parts, st).await.map(AuthenticatedUser))
    }
}
```

- `AuthenticatedUser` を取るハンドラ＝`user` 必須、`AdminUser`＝`admin` 必須、`Option<AuthenticatedUser>`＝任意（現行 `auth:"none"` でセッションがあれば拾う挙動）。**認可レベルがハンドラ署名に可視化**され、付け忘れがコンパイル面で目立つ。
- `FromRequestParts`（ボディ非消費・順序自由）を使う。ボディ消費 extractor（`Json<T>` 等）は最後に 1 個だけ置く。
- `Rejection = WebError` にして 401/403 の `IntoResponse` を明示（設計しないと汎用 500 になる落とし穴。[`rpt-axum-web-runtime.md`](verification/rpt-axum-web-runtime.md) §5）。

---

### 6.3 二経路認証（Cookie ＋ Bearer）の extractor 解決

現行 [`httpHelpers.ts`](../../src/server/httpHelpers.ts) の `resolveRequestUser`（Cookie セッション → Bearer の順）を厳密移植する。

- **Cookie 経路**: `__Host-yuuka-session`（HTTPS 本番）／開発時のみ `yuuka-session` も受理（`getSessionToken` の分岐、httpHelpers.ts:50-57）。トークンは**不透明トークンを共有 Redis にハッシュ保存**する現行方式を維持（[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §3.2 と整合＝署名鍵共有不要）。Redis クライアントは `AppState` に載せる。
- **Bearer 経路**: `Authorization: Bearer <token>`（desktop。`getBearerUser`, httpHelpers.ts:138）。`desktop_tokens` 表を sha256 照合（第7部の repo 経由）。**Bearer はアンビエント資格情報でない＝CSRF 非該当**（Origin チェックを課さない・6.4）。

```rust
async fn resolve_request_user(parts: &Parts, st: &AppState) -> Option<SessionUser> {
    if let Some(tok) = session_cookie(parts, &st.cfg) {            // __Host-yuuka-session
        if let Some(u) = st.sessions.lookup(&tok).await { return Some(u); } // Redis ハッシュ照合
    }
    if let Some(bearer) = bearer_token(parts) {                    // Authorization: Bearer
        return st.desktop_auth.verify(&bearer).await.ok();        // desktop_tokens sha256
    }
    None
}
```

`__Host-` prefix Cookie は Domain 不可＝**同一オリジン必須**。nginx 単一オリジン背後（新旧バックエンドの出し分け）と整合する（[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §3.2）。Cookie 発行時の属性 `Path=/; HttpOnly; Secure; SameSite=Lax` は現行 `setSessionCookie`（httpHelpers.ts:89）を厳密踏襲。

---

### 6.4 CSRF ／ ボディ上限 ／ プロト汚染（tower レイヤ）

**CSRF（現行 `isCrossSiteStateChange`, routeRegistry.ts:37-60 の完全移植）**: `POST`/`DELETE` かつ認可必須ルートに対し、以下を多層で判定する自作 `CsrfLayer`。

1. `Sec-Fetch-Site: cross-site` を明示拒否。
2. `Origin`（`null` 以外）のホストが `config.baseUrl` と不一致なら拒否。
3. `Origin` 無ければ `Referer` のホストで同上判定。
4. 両方無い＝判定不能は `SameSite=Lax` に委ねて**許可**（現行と同一の緩和）。
5. `auth:"none"`（Webhook 受信等）は対象外＝クロスオリジンが正当。

```rust
// CsrfLayer::call の中核（tower::Service 実装）
fn is_cross_site_state_change(req: &Request, allowed_host: &str) -> bool {
    if req.headers().get("sec-fetch-site").is_some_and(|v| v == "cross-site") { return true; }
    if let Some(origin) = req.headers().get(ORIGIN).and_then(|v| v.to_str().ok()) {
        if origin != "null" { return host_of(origin).as_deref() != Some(allowed_host); }
    }
    if let Some(referer) = req.headers().get(REFERER).and_then(|v| v.to_str().ok()) {
        return host_of(referer).as_deref() != Some(allowed_host);
    }
    false // 判定不能 → SameSite=Lax に委ねる
}
```

メソッド・認可レベルの条件分岐はルート定義側のマーカー（拡張 `Extension` かパス prefix `/api/`）で判別する。レイヤ適用順序は tower の「後入れ＝外側」規約に注意し、CSRF は認可 extractor より外側に置く。

**ボディ上限**: `DefaultBodyLimit::max(10 * 1024 * 1024)`（現行 `MAX_BODY_BYTES` = 10MB・レシート画像 base64 考慮、routeRegistry.ts:111）。超過は axum が 413 を返す。

**プロト汚染**: 現行 `stripProtoKeys`（`__proto__`/`constructor`/`prototype` 除去）は**JS 固有のハザード**。Rust は serde で**型付き struct にデシリアライズ**するため、そもそもプロトタイプ連鎖が存在せず攻撃面が消滅する。DTO には `#[serde(deny_unknown_fields)]` を付け、未知キーを 422/400 で弾く（防御の明示化）。

---

### 6.5 静的配信（`ServeDir` プリコンプレス ＋ SPA フォールバック）

現行 [`server.ts`](../../src/server.ts):115-236 の `serveStaticFile` を移植する。不変条件: パストラバーサル防御（`PUBLIC_DIR + sep` 前方一致、server.ts:124）、拡張子なしパスの SPA フォールバック（index.html）、Vite ハッシュ付きアセットの `immutable` キャッシュ／それ以外 `no-cache`（server.ts:154-162）、gzip 事前圧縮（COMPRESSIBLE_EXTS）。

```rust
// dist/public を配信。.br/.gz サイドカーは Accept-Encoding に応じて自動選択。
let serve_dir = ServeDir::new(&cfg.public_dir)
    .precompressed_br()      // dist/public/foo.js.br
    .precompressed_gzip()    // dist/public/foo.js.gz
    .append_index_html_on_directories(true)
    // 拡張子なし SPA ルートは index.html へフォールバック
    .fallback(ServeFile::new(cfg.public_dir.join("index.html")));

let app = app_router(state).fallback_service(serve_dir);
```

- **プリコンプレス**（[`rpt-axum-web-runtime.md`](verification/rpt-axum-web-runtime.md) §3）: `.precompressed_br()`/`.precompressed_gzip()` は `.br`/`.gz` サイドカーを配信し、無ければ非圧縮へフォールバック。**サイドカーはビルド時に事前生成が前提**（`ServeDir` は動的圧縮しない）。Docker ビルド段（[`00-decisions.md`](00-decisions.md) デプロイ節）で Vite 出力後に `brotli`/`gzip` を全 `COMPRESSIBLE` 資産へ生成する。
- **パストラバーサル**: `ServeDir` は内部で正規化しルート外アクセスを弾くが、現行の明示 403 と等価な防御を保つ（現行不変条件・[`00-decisions.md`](00-decisions.md) セキュリティ不変条件）。
- **キャッシュ／CSP 差し込み**: Vite ハッシュ資産の `Cache-Control: public, max-age=31536000, immutable` と index.html 系の `no-cache` の出し分けは `ServeDir` 前段の薄いミドルウェア（パスで判定）で付与。CSP 等セキュリティヘッダは 6.6 の `SetResponseHeaderLayer` が全レスポンスに乗せる。
- **index.html への `google-site-verification` 差し込み**（server.ts:182）はビルド時に確定させるか、起動時に 1 度だけ読み込んでメモリキャッシュした `Html` を返す専用ハンドラで対応（都度 I/O を避ける）。

---

### 6.6 セキュリティヘッダ ／ 動的圧縮（役割別レイヤ）

現行 `CSP`/`SECURITY_HEADERS`（server.ts:82-89）を**そのままの値で**移植する（[`00-decisions.md`](00-decisions.md) セキュリティ不変条件）。**静的ヘッダは `SetResponseHeaderLayer`、圧縮は `CompressionLayer`＝責務が別**（[`rpt-axum-web-runtime.md`](verification/rpt-axum-web-runtime.md) §4）。

```rust
const CSP: &str = "default-src 'self'; script-src 'self' https://static.cloudflareinsights.com; \
style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://fonts.gstatic.com; \
font-src 'self' https://fonts.gstatic.com https://fonts.googleapis.com; \
img-src 'self' data: https://assets-global.website-files.com https://cdn.discordapp.com; \
connect-src 'self' https://cloudflareinsights.com; worker-src 'self'; frame-src 'self'; frame-ancestors 'self';";

fn security_headers_layer() -> impl Layer<...> {
    ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(header::CONTENT_SECURITY_POLICY, hv(CSP)))
        .layer(SetResponseHeaderLayer::overriding(HeaderName::from_static("x-content-type-options"), hv("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(header::X_FRAME_OPTIONS, hv("SAMEORIGIN")))
        .layer(SetResponseHeaderLayer::overriding(header::REFERRER_POLICY, hv("strict-origin-when-cross-origin")))
        // HSTS: HTTPS 本番のみ（config で分岐）。max-age=63072000; includeSubDomains（server.ts:277）
        .layer(SetResponseHeaderLayer::if_not_present(header::STRICT_TRANSPORT_SECURITY, hv("max-age=63072000; includeSubDomains")))
}

// 動的圧縮は別レイヤ。ServeDir のプリコンプレス済みレスポンスは content-encoding を持つため二重圧縮されない。
let compression = CompressionLayer::new().br(true).gzip(true);
```

- **CSP から `unsafe-inline` を script-src で外している**現行の実効的 XSS 多層防御（server.ts:79-83）を厳守。インライン JS を足す場合は nonce/hash 方式へ（現行コメントの制約を継承）。
- `overriding()`（既存同名ヘッダを置換）を CSP に使う。HSTS は本番のみで `if_not_present()` 相当。
- MCP ダッシュボードの隔離 iframe（`sandbox="allow-scripts"`＝不透明オリジン）に返す専用ルート（現行 `mcpRoutes`）は**そのルートが独自 CSP を返す**構成を維持（本体 CSP は `frame-src 'self'` で同一オリジン dashboard を許可、server.ts:72-83）。

---

### 6.7 WebSocket（`/ws/chat`）

現行 [`server.ts`](../../src/server.ts):385-407 の `upgrade` ハンドラ（Bearer 認証＋`?botId=` 所有/共有検証、`hasBotAccess`）を axum の内蔵 WS へ写像する（[`rpt-axum-web-runtime.md`](verification/rpt-axum-web-runtime.md) §2）。**`ws` feature 必須**（未指定はコンパイルエラー）。別クレート不要。

```rust
// crates/web/src/routes/chat_ws.rs
pub fn routes() -> Router<AppState> {
    Router::new().route("/ws/chat", get(ws_upgrade))
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,             // ボディ消費 extractor → 引数の最後
    Query(q): Query<ChatWsQuery>,     // ?botId=
    user: BearerUser,                 // ネイティブ Bearer 専用 extractor（Cookie 経路は使わない）
    State(st): State<AppState>,
) -> Result<Response, WebError> {
    // 接続時に 1 Bot へ束縛。未指定は system_default（server.ts:398）
    let bot_id = q.bot_id.unwrap_or_else(|| "system_default".into());
    if !st.bots.has_access(&user.0.discord_id, &bot_id).await { // hasBotAccess 相当
        return Err(WebError::Forbidden); // 403
    }
    Ok(ws.on_upgrade(move |socket| handle_chat(socket, user.0, bot_id, st)))
}
```

- **Bearer 認証**: WS upgrade は `getBearerUser` のみ（Cookie 経路を持たない・現行踏襲、server.ts:391）。`BearerUser` extractor は 401 を返す。ネイティブ Bearer は CSRF 非該当なので Origin チェックを課さない。
- **ping/pong・ターンキュー**: 現行 `chatWebSocket`（`chatWss`/`handleChatConnection`）のターン直列化（ユーザーごとの発話キュー）とアイドル切断防止の ping を `handle_chat` 内で維持。axum の `WebSocket` は `Stream + Sink`（`Message` enum）で、`futures` の分割により read/write 並行。
- nginx 側は `map $http_upgrade $connection_upgrade` ＋ `Upgrade`/`Connection` 転送 ＋ `proxy_read_timeout 3600s` ＋ **バックエンド ping**（[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §3.3）。移行期は WS を新旧同時に割らず 1 upstream に固定。
- 停止時: 現行 `stopWebServer`（server.ts:421）の「全 WS 接続を閉じてから HTTP を停止」を、tokio-graceful-shutdown のサブシステム停止フックで再現（[`00-decisions.md`](00-decisions.md) 自己復帰節と統合）。

---

### 6.8 config.yaml の起動時厳密検証

現行 `config`（`baseUrl`/`port`/`host`/`trustedProxies`/`sessionTtlDays`/`googleSiteVerification` 等を参照）を、**型付き構造体へ厳密デシリアライズ**する。`serde` + `figment`（または `config` crate）で `config.yaml` を読み、**必須欠落・型不一致は起動時に fail-fast**（[`00-decisions.md`](00-decisions.md) 絶対制約2「致命的設定不備のみ fail-fast」）。

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub base_url: Option<Url>,             // https:// なら Secure/__Host- + HSTS を強制
    pub host: IpAddr,
    pub port: u16,
    pub db_path: PathBuf,
    pub session_ttl_days: u32,
    pub trusted_proxies: Vec<IpAddr>,      // XFF 信頼判定（getClientIp, httpHelpers.ts:65）
    pub google_site_verification: Option<String>,
    // ...機密は本 struct に平文で持たず secrecy::SecretString（[`00-decisions.md`](00-decisions.md) 機密フェイルクローズ節）
}
```

- `base_url` が `https://` の場合のみ Cookie ハードニング（`__Host-` + Secure）と HSTS を有効化（現行 `isHttpsDeployment`, httpHelpers.ts:43 の判定を型で表現）。
- `ConfigError`（`thiserror`）で欠落・不正値を列挙し、起動時に `Err` を返して**プロセスを即終了**（ここだけは fail-fast が正しい）。

---

### 6.9 第6部で保つセキュリティ不変条件（チェックリスト）

[`00-decisions.md`](00-decisions.md) の「移植で保つ不変条件」を、上記の写像先とともに再掲する。

| 不変条件 | 現行の担保 | Rust の担保 |
|---|---|---|
| CSP | server.ts:82（script-src から unsafe-inline 除外） | `SetResponseHeaderLayer::overriding`・値は同一文字列 |
| セキュリティヘッダ | X-Content-Type-Options/X-Frame-Options/Referrer-Policy/HSTS | `SetResponseHeaderLayer`（6.6） |
| CSRF | Origin/Referer/Sec-Fetch-Site（routeRegistry.ts:37） | `CsrfLayer`（6.4・4段判定を厳密移植） |
| パストラバーサル防御 | PUBLIC_DIR 前方一致 403（server.ts:124） | `ServeDir` の正規化＋ルート外拒否（6.5） |
| 機密非漏洩 | zod allowlist / エラーの丸め | 専用 DTO（機密をフィールドに持たない）＋ `WebError` の内部 Display 非露出（6.0） |
| CORS 限定反射 | baseUrl 同一ホストのみ ACAO（server.ts:284） | `CorsLayer` の predicate（6.1） |
| プロト汚染除去 | stripProtoKeys（routeRegistry.ts:66） | serde 型付き＋`deny_unknown_fields`（6.4） |

---

## 7. DB 層 / マイグレーション / データ分離

### 7.0 現行 DB 実装の不変条件

現行 [`database.ts`](../../src/db/database.ts) は `better-sqlite3` の単一コネクションで `journal_mode=WAL` / `foreign_keys=ON` を設定（busy_timeout は better-sqlite3 既定 5000ms に暗黙依存、[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §0 で実測訂正済み）。**SQLite が唯一の真実源で書き手は Node のみ**、`rust_synapse` は read-only（同 §0）。この「単一 writer」不変条件を Rust でも構造的に保証する。

---

### 7.1 rusqlite ＋ 単一 writer actor ＋ read pool

**採用（[`00-decisions.md`](00-decisions.md) #9・[`rpt-db-sqlx-vs-rusqlite.md`](verification/rpt-db-sqlx-vs-rusqlite.md) §2, §3）**:

| クレート | バージョン | 役割 |
|---|---|---|
| `rusqlite` | **0.40.1**（`bundled` feature） | SQLite C API 薄ラッパ。**bundled = SQLite 3.53.2 を静的リンク**（再現性・システム SQLite 非依存） |
| `libsqlite3-sys`（間接） | `^0.38.1` | bundled amalgamation |
| `deadpool-sqlite` | **0.13.0** | read 専用の非同期プール（`interact()` で内部 blocking 実行。`spawn_blocking` 手書き不要） |
| （代替）`r2d2_sqlite` | 0.34.0 | 同期プール（`spawn_blocking` と併用する場合） |
| `backon` | 1.6.0（[`00-decisions.md`](00-decisions.md) #7） | `SQLITE_BUSY` リトライ（ジッタ必須。`backoff` は RUSTSEC-2025-0012 で禁止） |

rusqlite は**同期 API**。tokio 上で直呼びは禁止で、`spawn_blocking` か `deadpool-sqlite` の `interact()` でブロッキングプールへ逃がす（[`rpt-db-sqlx-vs-rusqlite.md`](verification/rpt-db-sqlx-vs-rusqlite.md) §2）。

**アーキテクチャ: 単一 writer actor ＋ read pool**（SQLite の「多 reader・単一 writer」モデルに 1:1 対応。[`rpt-db-sqlx-vs-rusqlite.md`](verification/rpt-db-sqlx-vs-rusqlite.md) §3 の推奨）。

```rust
// crates/db/src/writer.rs
// 全書き込みを 1 タスク・1 コネクションに直列化する actor。
pub struct WriterHandle { tx: mpsc::Sender<WriteJob> }

struct WriteJob {
    // クロージャで rusqlite::Transaction を受け取り任意の書き込みを実行。
    run: Box<dyn FnOnce(&mut rusqlite::Transaction) -> Result<(), DbError> + Send>,
    done: oneshot::Sender<Result<(), DbError>>,
}

impl WriterHandle {
    pub fn spawn(db_path: &Path) -> Result<Self, DbError> {
        let (tx, mut rx) = mpsc::channel::<WriteJob>(256);
        let conn = open_conn(db_path, /*read_only=*/false)?; // PRAGMA を明示設定（7.2）
        // 専用 blocking スレッドで受信ループ。panic は supervisor が JoinError で検知し再spawn。
        std::thread::Builder::new().name("db-writer".into()).spawn(move || {
            let mut conn = conn;
            while let Some(job) = rx.blocking_recv() {
                // 全書き込み Tx は BEGIN IMMEDIATE（7.2）。BUSY は backon で吸収。
                let res = with_immediate_retry(&mut conn, job.run);
                let _ = job.done.send(res);
            }
        })?;
        Ok(Self { tx })
    }

    pub async fn write<F, T>(&self, f: F) -> Result<T, DbError>
    where F: FnOnce(&mut rusqlite::Transaction) -> Result<T, DbError> + Send + 'static, T: Send + 'static
    { /* job を送り oneshot を await。writer が落ちていれば DbError::WriterGone */ }
}
```

```rust
// crates/db/src/reader.rs — 多コネクション read pool（WAL の reader はスケールする）
pub struct ReadPool(deadpool_sqlite::Pool);

impl ReadPool {
    pub fn open(db_path: &Path, size: usize) -> Result<Self, DbError> {
        let cfg = deadpool_sqlite::Config::new(db_path);
        let pool = cfg.builder(Runtime::Tokio1)?
            .max_size(size)
            .post_create(hook_apply_read_pragmas()) // 各接続に read 用 PRAGMA
            .build()?;
        Ok(Self(pool))
    }
    pub async fn read<F, T>(&self, f: F) -> Result<T, DbError>
    where F: FnOnce(&rusqlite::Connection) -> Result<T, DbError> + Send + 'static, T: Send + 'static
    {
        let conn = self.0.get().await?;
        conn.interact(move |c| f(c)).await.map_err(DbError::Interact)?
    }
}
```

- **書き込みは必ず `WriterHandle::write`**（型で単一直列化を強制＝複数 writer 競合が原理的に起きない）。読み取りは `ReadPool::read`。
- reader は**各クエリ後に statement を確実に finalize/reset**（長寿命 reader が checkpoint を阻害＝WAL 肥大。[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §1.5）。rusqlite は `Statement` の drop で finalize されるため、prepared statement を跨いで保持しない設計にする。
- writer スレッドの panic は **`panic=unwind`＋JoinSet supervisor** が検知して再 spawn（[`00-decisions.md`](00-decisions.md) 自己復帰節）。再 spawn 中の write 要求は `DbError::WriterGone` で明示エラー化し、backon 上位リトライへ委ねる。

---

### 7.2 PRAGMA の明示設定と BEGIN IMMEDIATE

現行は WAL/foreign_keys のみ明示で busy_timeout は既定依存だが、Rust は**全 PRAGMA を明示**する（rusqlite は既定を一切設定しない。[`rpt-db-sqlx-vs-rusqlite.md`](verification/rpt-db-sqlx-vs-rusqlite.md) §2）。

```rust
fn open_conn(path: &Path, read_only: bool) -> Result<Connection, DbError> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_URI
    };
    let conn = Connection::open_with_flags(path, flags)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;   // 現行踏襲
    conn.busy_timeout(Duration::from_millis(5000))?;    // Node 既定 5000 と一致（[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §2.3）
    conn.pragma_update(None, "foreign_keys", "ON")?;    // 現行踏襲（database.ts:18）
    conn.pragma_update(None, "synchronous", "NORMAL")?; // WAL 下で十分・耐障害性ほぼ問題なし
    Ok(conn)
}
```

- **全書き込み Tx は `BEGIN IMMEDIATE`**（DEFERRED→write アップグレードの即-`SQLITE_BUSY` を回避。busy_timeout では救えないハザード＝[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §1.4）:

```rust
fn with_immediate_retry<F, T>(conn: &mut Connection, f: F) -> Result<T, DbError>
where F: FnOnce(&mut Transaction) -> Result<T, DbError>
{
    // rusqlite: BEGIN IMMEDIATE を明示
    let backoff = ExponentialBuilder::default().with_jitter(); // backon・ジッタ必須
    (|| {
        let mut tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&mut tx)?;
        tx.commit()?;
        Ok(out)
    })
    .retry(backoff)
    .when(|e: &DbError| e.is_sqlite_busy()) // SQLITE_BUSY のみリトライ
    .call()
}
```

- `busy_timeout=5000` を両側で揃える（synapse は現行 3000。運用上 5000 へ統一推奨。[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §2.3）。
- 移行期の SQLite 二重アクセスは**「Node 全書き込み・Rust read-only、カットオーバー時に一度だけ writer 移譲」**を最優先（[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §2.1）。両 writer は原則許容しない。

---

### 7.3 全テーブルの型付き repo とデータ分離（`UserId` newtype）

現行 [`migrations.ts`](../../src/db/migrations.ts) の全 CREATE TABLE を、rusqlite の型付きリポジトリへ移す。**データ分離キー欠落をコンパイル時に防ぐため `UserId` newtype を全 repo 署名に通す**（[`00-decisions.md`](00-decisions.md) #19・DB 節）。

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)] // ts-rs で TS 化
pub struct UserId(String);   // Discord ユーザーID（データ分離キー）
pub struct BotId(String);    // 既定 "system_default"

// 分離キーは引数の型が強制する（生の &str を受け取らない）
impl TodoRepo<'_> {
    pub fn list(&self, user: &UserId, bot: &BotId) -> Result<Vec<Todo>, RepoError> { /* WHERE user_id=?1 AND bot_id=?2 */ }
    pub fn create(&self, user: &UserId, bot: &BotId, input: NewTodo) -> Result<TodoId, RepoError> { /* ... */ }
}
```

**移植対象テーブル一覧（[`migrations.ts`](../../src/db/migrations.ts) 現行 v17 スキーマ・全 46 表）**。分離キー欄は各行を型でスコープする際の必須引数を示す（`user_id` を持つ表は `UserId` を repo 署名に必須化）。

| 分類 | テーブル | 分離キー | 備考 |
|---|---|---|---|
| メタ | `system_settings` | — | schema_version 兼用（refinery 化で用途縮小・7.4） |
| ユーザー/認証 | `users` | PK=discord_id | 機密列（gemini/google 暗号化）は DTO に出さない |
| 招待 | `invite_codes` | — | |
| Bot | `bots` / `bot_shares` / `bot_user_modules` | user_id / owner_id / (bot_id,user_id) | |
| Bot 属性 | `bot_context_notes` / `bot_guild_notes` / `bot_guilds` / `bot_members` | (bot_id,user_id) / (bot_id,guild_id) 等 | `bot_context_notes`/`bot_user_modules` は user_id へ FK を張らない正式例外（汎用モードの未登録 Discord ユーザー） |
| ペルソナ | `personas` / `bot_active_personas` | owner_id / (user_id,bot_id) | |
| 会話履歴 | `message_logs` ＋ `message_logs_fts`（FTS5 trigram）＋トリガー ai/ad/au | user_id（**FK 無し**・未登録メンバー記録のため） | 7.5 参照 |
| 経験/記憶 | `tool_outcomes` / `topic_tool_stats` / `synapses` | user_id(+bot_id[+guild_id]) | synapse `embedding` BLOB は read-only 参照（[`00-decisions.md`](00-decisions.md) Discord/synapse 節）。FK 無し |
| ToDo | `todos` ＋ `task_progress_logs` | user_id | todos は自己参照 parent_id |
| 予定 | `schedules` | user_id | |
| リマインド | `reminders` | user_id | |
| 家計 | `expenses` / `budget_limits` / `planned_payments` | user_id(+bot_id) | budget_limits PK に bot_id |
| Playbook | `playbooks` / `playbook_schedules` / `playbook_runs` | user_id(+bot_id) | |
| 個人 | `context_notes` / `clipboard_entries` / `contacts` | (user_id,bot_id) / user_id | |
| 認証情報 | `credentials` ＋ `bot_credential_access` | (user_id,service_name) / (bot_id,owner_id,service_name) | ユーザー鍵 Argon2id + AES-256-GCM（機密列は DTO 非出力） |
| Webhook | `webhook_endpoints` / `webhook_deliveries` | user_id | token UNIQUE |
| 配信 | `briefing_configs` / `report_configs` | (user_id,bot_id) | |
| MCP | `mcp_servers` ＋ `bot_mcp_access` | (user_id,bot_id) / (bot_id,owner_id,mcp_server_id) | `bot_mcp_links` は v4 で廃止・再作成しない。owner_id 次元は v7 のクロステナント修正 |
| Google | `user_google_accounts` / `bot_google_account` | user_id / bot_id | 暗号化リフレッシュトークンは DTO 非出力 |
| 監査 | `audit_logs` | user_id | 秘密値は記録禁止（現行不変条件） |
| デスクトップ | `desktop_tokens` | user_id | token_hash=sha256・UNIQUE |
| メンバー申請/ロール | `bot_member_requests` / `bot_roles` | (bot_id,guild_id,user_id) 等 | |
| タイムライン | `day_plan_blocks` / `timeline_records` | (user_id,bot_id) | v17 |

- **FK を張らない正式例外**（現行コメント準拠）: `message_logs` / `synapses` / `tool_outcomes` / `bot_context_notes` / `bot_user_modules` は user_id に users への FK を張らない（汎用モードで Web 未登録の Discord ユーザー ID も user_id に入るため）。repo でも FK 前提のロジックを持ち込まない。
- **機密フェイルクローズ**（[`00-decisions.md`](00-decisions.md) #19）: `users`/`credentials`/`user_google_accounts`/`mcp_servers`/`bots` の暗号化列（`*_encrypted`/`*_iv`/`*_tag`）は**API DTO struct のフィールドに存在させない**→ ts-rs 生成 TS にも現れず、漏洩が型的に不可能。復号値のメモリ保持は `secrecy::SecretString`。

---

### 7.4 マイグレーション（refinery・前方専用・冪等 baseline）

**採用（[`00-decisions.md`](00-decisions.md) #11・[`rpt-migrations-sqlx-refinery.md`](verification/rpt-migrations-sqlx-refinery.md) §2, §3）**: `refinery 0.9.2`（`features = ["rusqlite"]`）。rusqlite ネイティブ・前方専用・`refinery_schema_history` で version+checksum 管理・`embed_migrations!` でバイナリ埋め込み。**rusqlite バージョン pin に注意**（refinery 0.9.2 が想定する rusqlite 系列に合わせる。[`rpt-migrations-sqlx-refinery.md`](verification/rpt-migrations-sqlx-refinery.md) §2）。

**現行の破壊的パターンを撤廃する**。現行 [`migrations.ts`](../../src/db/migrations.ts) は `SCHEMA_VERSION="17"`（:12）を `system_settings.schema_version` と比較し、**旧 v1 検出時に `PRAGMA foreign_keys=OFF` ＋ `LEGACY_TABLES` を `DROP TABLE`（:880-894）＝データ喪失**する分岐を持つ。これを**完全撤廃**する（[`00-decisions.md`](00-decisions.md) DB 節・[`rpt-migrations-sqlx-refinery.md`](verification/rpt-migrations-sqlx-refinery.md) §3）。

**baseline (V1) の凍結**: 現行 v17 の最終スキーマ（7.3 の全 46 表・全インデックス・FTS5・トリガー）を **`CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS` の冪等 DDL** として `migrations/V1__baseline.sql` に固める（[`rpt-migrations-sqlx-refinery.md`](verification/rpt-migrations-sqlx-refinery.md) §3「idempotent baseline」）。

- **既存 DB**（実データあり・migration ledger 無し）: V1 は全て既存＝無害に通り、`refinery_schema_history` に記録される。**DROP は一切走らない**。
- **新規 DB**: V1 が実際に全スキーマを生成する。
- 現行の逐次移行関数（v3〜v17: `migrateToBotScopedData` 等の `ADD COLUMN`/`IF NOT EXISTS` ガード済みステップ）は、**最終形が baseline に畳み込まれている**ため個別再現不要。ただし `message_logs` の FK 撤廃再構築（:1109）や `bot_mcp_access` owner_id 追加（v7・:452）のような「既存 DB を旧定義から作り替える」ステップは、**baseline は最終定義のみを持つ**（新規 DB は最初から正しい）。**旧定義が残る本番 DB に対しては、V1 適用前に一度きりの明示的・レビュー済み移行**（V2 以降ではなく、baseline 導入と同時のデータ移行スクリプト）で吸収する。version 不一致の自動 DROP は決して使わない。

```rust
// crates/db/src/migrate.rs
mod embedded { refinery::embed_migrations!("./migrations"); } // V1__baseline.sql, V2__*.sql ...

pub fn run_migrations(conn: &mut rusqlite::Connection) -> Result<(), MigrateError> {
    // 前方専用・冪等。適用済みは skip、適用済みファイルの改変は checksum 不一致で Err。
    let report = embedded::migrations::runner()
        .set_migration_table_name("refinery_schema_history")
        .run(conn)?;
    tracing::info!(applied = report.applied_migrations().len(), "migrations done");
    Ok(())
}
```

- **前方専用**（refinery は down を持たない。取り消しは新しい V を書く。[`rpt-migrations-sqlx-refinery.md`](verification/rpt-migrations-sqlx-refinery.md) §2）。以後の変更は `V2__…`, `V3__…` の追記のみ。**適用済み migration は二度と編集しない**（checksum 検証で Err になる＝改竄検知）。
- **FK OFF が必要なテーブル再構築**（create-copy-drop-rename）を V2 以降で行う場合、SQLite の `PRAGMA foreign_keys=OFF` はトランザクション内で効かないため `PRAGMA defer_foreign_keys=ON`（Tx 内可）を使う（[`rpt-migrations-sqlx-refinery.md`](verification/rpt-migrations-sqlx-refinery.md) §1）。
- **migration 所有権は単一プロセスに一元化**（[`00-decisions.md`](00-decisions.md) DB 節）。移行期は「どちらが migration を実行するか」を Rust か Node の一方に固定し、両者が同時に走らせない（[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §4.3・ロールバック窓ではスキーマ凍結）。migration は**単一 writer 経路**で実行する（writer actor 起動前の起動シーケンスで 1 度）。
- `system_settings.schema_version` 行は refinery 導入後は**マイグレーション判定に使わない**（version 判定は `refinery_schema_history` が担う）。`system_settings` 表自体は他用途（`v5_grants_backfilled` 等の marker）で残置。

---

### 7.5 FTS5 全文検索とトリガーの移植

`message_logs_fts`（FTS5・`tokenize='trigram'`・外部コンテンツ表 `content='message_logs'`, content_rowid='id'）とトリガー `message_logs_ai`/`ad`/`au`（[`migrations.ts`](../../src/db/migrations.ts):1143-1160）を baseline V1 にそのまま含める。

- rusqlite の `bundled` SQLite **3.53.2 は FTS5 を内蔵**（`bundled` は FTS5 有効でビルドされる）。追加 feature 不要だが、CI で `SELECT * FROM pragma_compile_options` に `ENABLE_FTS5` があることを起動時に検証しておくと安全。
- トリガー（INSERT/DELETE/UPDATE で FTS を同期）は DDL としてそのまま維持。`message_logs` は writer actor 経由でのみ書くため、FTS 同期の一貫性は単一 writer 直列化で自然に保たれる。
- 全文検索クエリ（`MATCH`）は read pool 側で実行。

---

### 7.6 代替: sqlx 0.9（コンパイル時クエリ検査が欲しい場合）

**採用はしない**が、[`00-decisions.md`](00-decisions.md) の未解決事項2（DB 選定）に対する代替として記録する。`sqlx 0.9.0`（`sqlx-sqlite 0.9.0`）は `query!`/`query_as!` の**コンパイル時クエリ検査**が魅力で、`.sqlx` オフラインモード（`sqlx-cli prepare`）で CI から実 DB 依存を外せる（[`rpt-db-sqlx-vs-rusqlite.md`](verification/rpt-db-sqlx-vs-rusqlite.md) §1）。マイグレーションも `sqlx::migrate!()` ＋ `_sqlx_migrations` で同等の前方専用・checksum を得られ、`Migrator::skip` で baseline を「適用済み」マークできる（[`rpt-migrations-sqlx-refinery.md`](verification/rpt-migrations-sqlx-refinery.md) §1, §3）。

**却下理由（1 段落）**: (1) **SQLite 書き込み footgun** — 既定の複数コネクション `SqlitePool` は WAL 書き込みで `busy_timeout` 競合・ロック飢餓を招き（実測 ~20x 劣化）、結局アプリ層で「read pool ＋ `max_connections(1)` write pool」の単一 writer 規律を手当てする必要があり、rusqlite の writer actor と同じ設計を別の抽象で再実装することになる（[`rpt-db-sqlx-vs-rusqlite.md`](verification/rpt-db-sqlx-vs-rusqlite.md) §3）。(2) **synapse との二重化** — 既存 `rust_synapse` が rusqlite を使っており（[`rpt-dual-sqlite-hazard.md`](verification/rpt-dual-sqlite-hazard.md) §0）、sqlx を足すと同一プロセス内に 2 系統の SQLite バインディング・依存が並立する。加えて SQLite のヌル可能性推論は Postgres より脆く（`LEFT JOIN` で `UnexpectedNull`）`as "col!"`/`col?` の冗長な override が要る。以上より rusqlite に一本化する。

---

### 7.7 第7部で保つ不変条件（チェックリスト）

| 不変条件 | 現行の担保 | Rust の担保 |
|---|---|---|
| WAL / FK ON | database.ts:17-18 | `open_conn` PRAGMA 明示（7.2） |
| busy_timeout 5000 | better-sqlite3 既定（暗黙） | `conn.busy_timeout(5000)` 明示（7.2） |
| 単一 writer | Node のみ書き込み | `WriterHandle` actor で型強制（7.1） |
| データ分離（user_id 必須） | repo が user_id をスコープ | `UserId` newtype を repo 署名に必須化（7.3） |
| 機密非漏洩 | zod allowlist | 専用 DTO（機密列をフィールドに持たない）＋ secrecy（7.3） |
| データ喪失の撤廃 | — （現行は DROP 分岐が残存） | refinery 前方専用・冪等 baseline・DROP 分岐撤廃（7.4） |
| FTS5 整合 | トリガー同期 | 単一 writer で同期一貫性・bundled FTS5（7.5） |

---

### 付録: 第6〜7部で確定した数値・型名（load-bearing）

- axum **0.8.9**（hyper `^1.1`, tower `^0.5.2`, tower-http **0.6.x 固定**・0.7.0 不採用）／`ws` feature／`FromRequestParts`・`OptionalFromRequestParts`／`ServeDir::precompressed_br()`/`precompressed_gzip()`／`SetResponseHeaderLayer::overriding`/`if_not_present`／`CompressionLayer`／`DefaultBodyLimit::max(10MB)`／`CorsLayer`。
- rusqlite **0.40.1**（bundled = SQLite **3.53.2**）／`deadpool-sqlite` **0.13.0**（代替 `r2d2_sqlite` 0.34.0）／PRAGMA: `journal_mode=WAL`・`busy_timeout=5000`・`foreign_keys=ON`・`synchronous=NORMAL`／`TransactionBehavior::Immediate`（BEGIN IMMEDIATE）／`backon` 1.6.0 で `SQLITE_BUSY` リトライ。
- refinery **0.9.2**（`rusqlite` feature・前方専用・`refinery_schema_history`・`embed_migrations!`）／baseline **V1** は `CREATE ... IF NOT EXISTS` 冪等／現行 SCHEMA_VERSION 不一致 DROP（migrations.ts:880-894）を**撤廃**。
- 現行全 **46 テーブル**（`bot_mcp_links` は v4 廃止で対象外）を型付き repo へ。`UserId` newtype でデータ分離を型保証。

---

> 本書は [`00-decisions.md`](00-decisions.md) の確定選定（#12 Discord=twilight 0.17.1 / #13 Gemini=自前 reqwest ラッパ / #14 API面=classic generateContent / #15 ToolProvider）を実装レベルへ落とす。一次ソース照合は [`verification/rpt-discord-serenity-twilight.md`](verification/rpt-discord-serenity-twilight.md), [`verification/rpt-gemini-design-verify.md`](verification/rpt-gemini-design-verify.md), [`verification/rpt-gemini-funccalling-mcp.md`](verification/rpt-gemini-funccalling-mcp.md), [`verification/rpt-gemini-rest-crates.md`](verification/rpt-gemini-rest-crates.md) を参照。現行実装は [`src/bot.ts`](../../src/bot.ts), [`src/gemini.ts`](../../src/gemini.ts), [`src/services/llmClient.ts`](../../src/services/llmClient.ts)。
>
> 絶対制約（[`00-decisions.md`](00-decisions.md) 冒頭）に整合させる：厳格エラー（thiserror 具体列挙型・握り潰し禁止）／常時稼働・自己復帰（supervisor + バックオフ、致命設定のみ fail-fast）／マルチスレッド／ユーザー製カスタムモジュール拡張性。本部は第5部（supervisor）・第9部（ToolProvider）と接続する。

---

## 8.0 現行アーキテクチャの要約（移植対象の1:1マップ）

移植の忠実度を担保するため、現行 TS の構造を先に確定させる。

**Discord 側（[`src/bot.ts`](../../src/bot.ts)）**
- `discord.js` v14。**共有デフォルトクライアント** `client`（`system_default`）＋ **ユーザー別カスタムクライアント** `customClients: Map<botId, Client>`（[`src/bot.ts:78-81`](../../src/bot.ts)）。
- `getBotClientForUser(botId)`：カスタムが ready なら優先、無ければデフォルト（[`src/bot.ts:87-93`](../../src/bot.ts)）。
- `startCustomBot` は `startInFlight: Map<botId, Promise>` で**起動を直列化**（連打・restart 競合で destroy されない Client が Gateway に残り二重応答するのを防ぐ [`src/bot.ts:1320-1406`](../../src/bot.ts)）。
- `restartDefaultBot(token)`：v14 は destroy 済み Client の再ログインを保証しないため**新インスタンスへ差し替え**、ESM live-binding で参照側へ反映（[`src/bot.ts:1427-1451`](../../src/bot.ts)）。→ **twilight ではこの制約自体が消える**（Shard は値、トークン差し替えは新 Shard を spawn するだけ）。
- `claimMessageOnce(botUserId, messageId)`：`bot user id : message id` の TTL 冪等ガード（同一 identity の Client 重複時の二重応答防止 [`src/bot.ts:939-956`](../../src/bot.ts)）。
- ボタン：`handleInteraction`（`share_accept`/`share_decline`/`memreq_*`/`persona_import`。`customId` を `action:id:extra` で分解、`interaction.update`/`reply({ephemeral})`/`followUp` [`src/bot.ts:377-532`](../../src/bot.ts)）。
- プレゼンス演出 `setBotStatus`（thinking/writing/idle [`src/bot.ts:151-188`](../../src/bot.ts)）、プロフィール同期（起動時＋1時間 [`src/bot.ts:195-232`](../../src/bot.ts)）、`sendTyping` 5秒維持、2000字分割 `splitMessage`。

**Gemini 側（[`src/gemini.ts`](../../src/gemini.ts), [`src/services/llmClient.ts`](../../src/services/llmClient.ts)）**
- `@google/generative-ai`（**廃止予定の旧 Node SDK**）＋ classic `generateContent`。モデル `gemini-3.1-flash-lite`（[`src/services/llmClient.ts:19,48`](../../src/services/llmClient.ts)）。
- ユーザー別／Bot別に API キーをキャッシュ（`userAICache`/`botAICache`、キー変更で無効化 [`src/services/llmClient.ts:6-16`](../../src/services/llmClient.ts)）。
- `generateWithRetry`：429/5xx を `RetryInfo.retryDelay` 優先＋指数バックオフでリトライ、120s タイムアウト（[`src/gemini.ts:399-464`](../../src/gemini.ts)）。
- `runFunctionCallingLoop`：`maxIterations=10`、`functionCall`↔`functionResponse` 往復、**完了ハルシネーション是正**（`claimsActionCompleted` 検知→`mode:ANY`+`allowedFunctionNames` で1回だけ強制、`maxCorrectionAttempts=2` [`src/gemini.ts:501-746`](../../src/gemini.ts)）。
- 3経路（秘書 `processMessage` / ギルド `processGuildMessage` / owner DM `processBotDmMessage`）が `runPlannedTurn` を共有（[`src/gemini.ts:793-926`](../../src/gemini.ts)）。
- レジストリ `buildFunctionRegistry`：`declarations` と `dispatch(ctx, name, args)`、名前重複を throw（[`src/functions/registry.ts`](../../src/functions/registry.ts)）。MCP は `getMcpFunctionModuleForBot` で動的マージ（[`src/functions/mcpDynamic.ts`](../../src/functions/mcpDynamic.ts)）。

---

## 8.1 Discord（twilight 0.17.1）

### 8.1.1 crate 構成と却下理由

**採用：twilight 0.17.1**（[`verification/rpt-discord-serenity-twilight.md`](verification/rpt-discord-serenity-twilight.md) で crates.io API 実測）。

| crate | version | 役割 |
|---|---|---|
| `twilight-gateway` | **0.17.1**（2025-12-13） | WebSocket。`Shard` = 1 ゲートウェイセッション。`create_recommended`/`create_iterator`、IDENTIFY レート制限 `Queue`。 |
| `twilight-http` | **0.17.1** | REST。`Client` / `Client::interaction(app_id)` → `InteractionClient`。 |
| `twilight-model` | **0.17.1** | 純データ型。`Interaction`, `Component`/`Button`, `http::interaction::{InteractionResponse, InteractionResponseType, InteractionResponseData}`。I/O 無し。 |
| `twilight-cache-inmemory` | **0.17.1**（任意） | 独立キャッシュ。**テナント別 or 無しを明示選択**（後述）。 |
| `twilight-util` | **0.17.0**（0.17.1 バンプ無し＝正） | builder（`InteractionResponseDataBuilder`）、permission 計算。MSRV 1.79。 |

**serenity 0.12.5 却下理由**（[`verification/rpt-discord-serenity-twilight.md`](verification/rpt-discord-serenity-twilight.md)）：
1. **1 Client = 1 token = 1 所有イベントループ**が前提。現行 `customClients`（N テナント）＝ N 個の独立 Client 構築で「against the grain」。
2. 各 Client が既定で**自前 `Arc<Cache>`＋`Arc<Http>`** を抱える → N テナントでメモリ倍増。
3. sharding/再接続がライブラリ内部に隠蔽され、**テナント個別の再起動・バックオフポリシーが掛けにくい**（絶対制約2＝自己復帰と衝突）。

twilight は gateway/http/model/cache が疎結合で、`Shard` は「値として所有し poll するだけ」。N テナント = N Shard を各 tokio task で保持し、**HTTP は共有 / cache はテナント別 or 無し**を明示制御できる。

### 8.1.2 caller 駆動 poll loop と supervisor 統合（絶対制約2の核）

twilight `Shard` は再接続・resume を**内蔵するが、caller が poll し続ける間のみ**動く（[`verification/rpt-discord-serenity-twilight.md`](verification/rpt-discord-serenity-twilight.md) の Shard docs 引用）：

> "Shards start out disconnected, but will on the first successful call to `poll_next` try to reconnect… `poll_next` must then be repeatedly called in order for the shard to maintain its connection and update its internal state."

これは自律バックグラウンド再接続では**なく caller 駆動**。イディオムは「per-shard ループでエラーを `Result` アイテムとして log-and-continue、次 poll で shard 自身が resume」。この形が**第5部 supervisor（自前 `JoinSet` + 指数バックオフ再 spawn）と直結**し、テナント別バックオフ／再起動／劣化縮退を掛けられる（serenity では困難）。

**テナント別 Shard を supervisor 配下の tokio task で poll する設計：**

```rust
// tenant = 現行 customClients の1エントリ（system_default 含む）
struct TenantBot {
    bot_id: BotId,                 // "system_default" | Uuid 等
    token: Secret<String>,         // secrecy（復号済みトークンをログに出さない）
    intents: Intents,             // GUILDS|GUILD_MESSAGES|MESSAGE_CONTENT|DIRECT_MESSAGES
    http: Arc<twilight_http::Client>, // ★共有可（下記）
    // cache: Option<Arc<InMemoryCache>>  // テナント別 or None（下記）
}

// supervisor（第5部）が各テナントごとに spawn。JoinError(is_panic) を検知したら
// 指数バックオフで再 spawn。ここでは shard 内部エラーはループ内で吸収する。
async fn run_tenant(bot: TenantBot, mut shutdown: ShutdownToken) -> Result<(), DiscordError> {
    let mut shard = Shard::new(ShardId::ONE, bot.token.expose_secret().to_owned(), bot.intents);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break, // tokio-graceful-shutdown からの伝播
            item = shard.next_event(EventTypeFlags::all()) => {
                match item {
                    Ok(event) => {
                        // spawn せず順次 or 明示 spawn。二重応答冪等ガード（8.1.5）を通す。
                        if let Err(e) = handle_event(&bot, event).await {
                            // ★握り潰さない：DiscordError variant をログ・メトリクス化して continue
                            tracing::warn!(bot_id = %bot.bot_id, error = %e, "event handling failed");
                        }
                    }
                    Err(source) => {
                        // 接続系エラーは致命ではない。次 poll で shard が resume/reconnect する。
                        tracing::warn!(bot_id = %bot.bot_id, error = %source, "shard error, continuing");
                        // fatal（例: 無効トークン）のみ break して supervisor に再起動判断を委ねる
                        if source.is_fatal() { return Err(DiscordError::ShardFatal { bot_id: bot.bot_id.clone() }); }
                    }
                }
            }
        }
    }
    Ok(())
}
```

**トークンローテーション（現行 `restartDefaultBot` [`src/bot.ts:1427`](../../src/bot.ts) の置換）**：v14 の「destroy 済み Client は再ログイン不可」制約が消えるため、**当該テナントの task を supervisor 経由で停止 → 新トークンで新 Shard を張る**だけ。ESM live-binding のトリックは不要。`restartDefaultBot` は「supervisor に `RestartTenant{bot_id, new_token}` を送る」IPC に置換。

**起動直列化（現行 `startInFlight` [`src/bot.ts:1320`](../../src/bot.ts)）**：Rust では supervisor が `HashMap<BotId, JoinHandle>` を単一 owner タスクで持ち、`spawn`/`abort` を直列化するので `startInFlight` は自然消滅（同一 bot の二重 Shard が構造的に起き得ない）。

### 8.1.3 HTTP クライアント共有・キャッシュ方針（明示）

- **`twilight_http::Client` は全テナントで1個を共有**（現行の各 Client 自前 Http を集約）。ただし**トークンはテナント別**なので、送信時にトークンを差し替える運用が要る。twilight-http は「proxying で多サービスがレート予算を共有」できる設計だが、**Bot API のレート制限はトークン単位**であるため、実装上は次のどちらか：
  - (a) **テナントごとに `twilight_http::Client` を1個**（トークン別・レート bucket 別。メモリは小さく、これが素直）。
  - (b) 共有1個＋リクエスト毎トークン注入（twilight の proxy 機構前提。複雑）。
  → **既定は (a)**（現行の「Client ごとに Http」を素直に踏襲・レート境界がトークンと一致）。共有は将来最適化。
- **キャッシュ：既定は「無し」**。現行 `getGuildOptionsForBot`（[`src/bot.ts:116-149`](../../src/bot.ts)）が `guild.roles.fetch()`＋`members.cache` を読む用途に限り、**テナント別 `InMemoryCache`（`ResourceType::{ROLE, MEMBER, GUILD}` に絞る）** を任意で持つ。ロールは Guilds インテントで完全取得（REST fetch でも可）、メンバーは GuildMembers 特権インテント無しでキャッシュ済みのみ（現行と同じ不完全性 → UI で ID 手入力フォールバック）。twilight のキャッシュは**明示 feed**（イベントを自分で入れる）なので、不要なテナントは持たない＝メモリ節約。

### 8.1.4 ボタンインタラクション（`InteractionResponse`）

現行 [`src/bot.ts:377-532`](../../src/bot.ts) の `handleInteraction` を twilight-model + `InteractionClient` へ 1:1 移植する。

| 現行（discord.js） | twilight | 用途 |
|---|---|---|
| `interaction.reply({ content, ephemeral: true })` | `InteractionResponseType::ChannelMessageWithSource` + `InteractionResponseData{ flags: MessageFlags::EPHEMERAL, .. }` | 申請情報不正・非対象招待の通知 |
| `interaction.update({ content, components: [] })` | `InteractionResponseType::UpdateMessage` | 承認/辞退でボタンを消して結果表示（`share_accept`/`memreq_*`/`persona_import`） |
| `interaction.followUp({ content, components })` | `InteractionClient::create_followup(token)` | 推奨ペルソナのインポート確認 |

```rust
let interaction_client = http.interaction(application_id);
let resp = InteractionResponse {
    kind: InteractionResponseType::UpdateMessage,
    data: Some(InteractionResponseDataBuilder::new()  // twilight-util
        .content(format!("✅ Bot「**{name}**」へのアクセスが有効になりました！"))
        .components([])                                // ボタン除去
        .build()),
};
interaction_client.create_response(interaction.id, &interaction.token, &resp).await?;
```

`customId` 分解（`action:id:extra`）はそのまま：`custom_id.splitn(3, ':')`。`isButton()` は `interaction.data` が `InteractionData::MessageComponent` かつ `component_type == Button` で判定。エラー時の握り潰し（現行の空 `catch{}`）は**しない**：`DiscordError` にして log、必要時のみ ephemeral エラー返信。

### 8.1.5 二重応答冪等ガード・その他ユーティリティ

- `claimMessageOnce`（[`src/bot.ts:939-956`](../../src/bot.ts)）：`Mutex<HashMap<(BotUserId, MessageId), Instant>>` + TTL 60s。または `moka`（TTL キャッシュ）で置換。supervisor が同一 Shard 重複を構造的に防ぐため**多重防御**として残す。
- `sendTyping` 5秒維持：`tokio::time::interval` + `shard`/`http` で `create_typing_trigger`。task ローカルで `select!` 終了アームと共に落とす（現行 finally の typingInterval clear に相当）。
- `splitMessage`（2000字・改行境界 [`src/bot.ts:1268-1288`](../../src/bot.ts)）・`setBotStatus`（presence [`src/bot.ts:151`](../../src/bot.ts)）はロジック直移植。
- 添付取得（画像/音声を fetch→base64）：`reqwest` で URL 取得 → `base64` encode（8.2.5 参照）。`SUPPORTED_AUDIO_TYPES`（[`src/bot.ts:61-72`](../../src/bot.ts)）は定数配列へ。

---

## 8.2 Gemini（自前 reqwest 0.13 ラッパ）

### 8.2.1 crate 選定と却下理由

**採用：reqwest 0.13.4 + serde/serde_json + thiserror の薄い自前クライアント**（[`verification/rpt-gemini-rest-crates.md`](verification/rpt-gemini-rest-crates.md)）。

- **公式 Rust SDK は不在**（Python/JS/Go/Java/C# のみ）。
- `google-generative-ai-rs`：**アーカイブ済（2025-07）・FC 未実装＝不適**。
- `gemini-rust 1.7.1`：機能豊富だが 3rd-party 保守依存、**厳格エラー方針（thiserror）と API 面固定（generateContent レガシー固定）が実コードで未確認＝予備**。
- 自前ラッパの理由：(1) 表面積が小さい（generateContent / streamGenerateContent の2エンドポイント＋Part/Content/Tool 群のみ）、(2) **429/`RetryInfo.retryDelay`/5xx/JSON deser/timeout を thiserror variant で完全掌握**（現行 rate-limit バックオフを 1:1 移植可）、(3) FC ループの並行実行・`mode:ANY` 是正・maxIterations といったアプリ固有制御を crate 抽象に縛られず書ける。

**依存 crate（薄いラッパ用）：** `reqwest`（`rustls-tls`,`json`,`stream`）／`serde`,`serde_json`／`thiserror 2.0.18`／`base64`／`backon 1.6.0`／`secrecy`（API キー）／（SSE 採用時）`futures-util`,`eventsource-stream 0.2.3`／`tokio`。JSON Schema を Rust 型から生成するなら `schemars` を任意採用（ネイティブツール宣言用）。

### 8.2.2 classic generateContent の型（camelCase 厳密固定）

⚠️ **2026年に Interactions API が GA 化し「正面玄関」に昇格、generateContent は "legacy" だが完全サポート継続**（[`verification/rpt-gemini-design-verify.md`](verification/rpt-gemini-design-verify.md), [`verification/rpt-gemini-rest-crates.md`](verification/rpt-gemini-rest-crates.md)）。**将来リスクとして記録**しつつ、現行を1:1移植できる generateContent を採用。Web 資料は両 API 混在（generateContent=camelCase `inlineData`/`functionResponse`、Interactions=snake `function_result`/`call_id`）のため、**struct を camelCase に厳密固定して Interactions 語彙の混入を防ぐ**。

```rust
// エンドポイント: POST https://generativelanguage.googleapis.com/v1beta/{model=models/*}:generateContent
// 認証: x-goog-api-key ヘッダ（?key= は URL がログに残るため避ける）

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<ToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Content { role: Role, parts: Vec<Part> }

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Role { User, Model, Function } // ★role:"function" も来る（tool 結果返送時, verify 済）

// Part は oneof。内部タグ無しで各フィールドを Option 化するのが最も堅牢
// （Gemini は1 Part に1フィールドのみ入れてくる）。
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(skip_serializing_if = "Option::is_none")] text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] inline_data: Option<InlineData>,
    #[serde(skip_serializing_if = "Option::is_none")] function_call: Option<FunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")] function_response: Option<FunctionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")] file_data: Option<FileData>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct InlineData { mime_type: String, data: String } // data = base64

#[derive(Serialize, Deserialize, Clone)]
struct FunctionCall { name: String, #[serde(default)] args: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")] id: Option<String> } // parallel 相関用

#[derive(Serialize, Deserialize, Clone)]
struct FunctionResponse { name: String, response: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")] id: Option<String> }

// tools[].functionDeclarations[]
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Tool { function_declarations: Vec<FunctionDeclaration> }

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FunctionDeclaration {
    name: String,                       // [a-zA-Z0-9_:.-], 最大128字
    description: String,
    // ★プラグイン由来はフル JSON Schema を parametersJsonSchema に載せる（8.4 / 第9部）
    #[serde(skip_serializing_if = "Option::is_none")] parameters: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")] parameters_json_schema: Option<serde_json::Value>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ToolConfig { function_calling_config: FunctionCallingConfig }

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FunctionCallingConfig {
    mode: FunctionCallingMode,
    #[serde(skip_serializing_if = "Option::is_none")] allowed_function_names: Option<Vec<String>>,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")] // AUTO / ANY / NONE / VALIDATED
enum FunctionCallingMode { Auto, Any, None, Validated }

// レスポンス
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentResponse {
    #[serde(default)] candidates: Vec<Candidate>,
    #[serde(default)] usage_metadata: Option<UsageMetadata>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate { content: Content, #[serde(default)] finish_reason: Option<String> }
```

> **既定は `AUTO`**（[`verification/rpt-gemini-funccalling-mcp.md`](verification/rpt-gemini-funccalling-mcp.md)）。`ANY`（+`allowedFunctionNames`）は現行の完了ハルシネーション是正でのみ使う（常用は無限ツール呼び出しループを招く）。`VALIDATED` は Preview 扱いのため当面不使用。

### 8.2.3 エラー型（thiserror）とバックオフ（backon）

現行 `isRateLimitError`/`isServerError`/`RetryInfo` パース（[`src/gemini.ts:372-463`](../../src/gemini.ts)）を thiserror variant へ 1:1 移植。**握り潰し禁止**（絶対制約1）。

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GeminiError {
    #[error("rate limited (429){}", retry_after.map(|d| format!(", retry after {}s", d.as_secs())).unwrap_or_default())]
    RateLimited { retry_after: Option<std::time::Duration> }, // RetryInfo.retryDelay を保持
    #[error("server error (status {status})")]
    ServerError { status: u16 },                              // 500/502/503/504
    #[error("http transport error")]
    Transport(#[from] reqwest::Error),
    #[error("response deserialize failed")]
    Deserialize(#[source] serde_json::Error),
    #[error("request timed out")]
    Timeout,
    #[error("api key missing or undecryptable for {scope}")]
    KeyUnavailable { scope: String },                         // ユーザー/Bot キー未設定
    #[error("max function-calling iterations exceeded")]
    MaxIterations,
}
```

- **429/RetryInfo**：`google.rpc.RetryInfo` の `retryDelay`（`"37s"` 形式）を error body の `error.details[]` から抽出し `RateLimited.retry_after` に載せる。
- **バックオフ**：`backon 1.6.0` の `ExponentialBuilder::default().with_jitter()`（ジッタ**明示**）。`RetryInfo` があればそれを尊重、無ければ指数（現行の `min(1000*2^(attempt+1), 60000)` 相当）。`RateLimited`/`ServerError`/`Timeout` のみリトライ対象（`when` 述語で判定）。`maxRetries=3`（現行同値）。
- **サーキットブレーカ**：Gemini エンドポイントに `recloser 1.4.0`（または `AtomicU*` 自前状態機械）を併用（[`00-decisions.md`](00-decisions.md) #8）。連続失敗で open→即 fail で無駄打ちを防ぎ、劣化縮退（現行のユーザー向け「混み合っています」定型応答）へ落とす。
- **タイムアウト**：`reqwest::Client` に `.timeout(Duration::from_secs(120))`（FC ループ用）／補助生成は 60s（現行 [`src/services/llmClient.ts:137`](../../src/services/llmClient.ts) と一致）。

### 8.2.4 Function Calling ループ（現行 `runFunctionCallingLoop` の型付き移植）

[`src/gemini.ts:501-746`](../../src/gemini.ts) を struct 化。往復・並行呼び出し・maxIterations・完了是正を保持。

```rust
struct LoopResult { text: String, browser_tool_called: bool, browser_tool_failed: bool }

async fn run_function_calling_loop(
    client: &GeminiClient,           // API キー・モデルを保持（getUserGenAI/getBotGenAI 相当）
    system_instruction: &str,
    registry: &ToolRegistry,         // 第9部 ToolProvider レジストリ（下記 8.4）
    contents: &mut Vec<Content>,
    ctx: &ToolContext,               // userId / botId / guildId（データ分離キー）
    on_status: &StatusCb,
    opts: LoopOptions,               // record_actions, on_heavy_detected, heavy_runtime_ms, allowed_tool_names
) -> Result<LoopResult, GeminiError> {
    on_status(Status::Thinking);
    let mut resp = client.generate(system_instruction, &registry.declarations(), contents, None).await?;

    let max_iterations = 10;
    let (mut iterations, mut total_calls, mut corrections) = (0usize, 0usize, 0usize);
    let max_corrections = 2;

    while iterations < max_iterations {
        let Some(cand) = resp.candidates.into_iter().next() else { break };
        let calls: Vec<FunctionCall> = cand.content.parts.iter()
            .filter_map(|p| p.function_call.clone()).collect();

        if calls.is_empty() {
            // 完了ハルシネーション是正: このターンで一度も関数を呼ばず「登録しました」等を主張
            if total_calls == 0 && corrections < max_corrections && !registry.is_empty() {
                let text = cand.content.parts.iter().filter_map(|p| p.text.as_deref()).collect::<String>();
                if claims_action_completed(&text) {
                    corrections += 1;
                    contents.push(cand.content.clone());
                    contents.push(Content::user_text(COMPLETION_CORRECTION_PROMPT));
                    on_status(Status::Thinking);
                    let tc = ToolConfig { function_calling_config: FunctionCallingConfig {
                        mode: FunctionCallingMode::Any,                 // ★構造的に関数呼び出しを強制
                        allowed_function_names: opts.allowed_tool_names.clone(), // プラン候補に限定
                    }};
                    resp = client.generate(system_instruction, &registry.declarations(), contents, Some(tc)).await?;
                    iterations += 1;
                    continue;
                }
            }
            break;
        }

        total_calls += calls.len();
        let mut response_parts: Vec<Part> = Vec::with_capacity(calls.len());

        // 並行呼び出し公式サポート。id を相関に保持し、独立ツールは join_all で並行実行してよい。
        for fc in &calls {
            // 実行時エスカレーション（重いツール or 規定時間超で一度だけ一時応答）
            maybe_escalate_heavy(&opts, fc, /* elapsed */);
            // ★第9部レジストリへ dispatch（Native/MCP/WASM を同一 trait で）
            let out = registry.invoke(ctx, &fc.name, fc.args.clone()).await
                .unwrap_or_else(|e| ToolOutput::error(e.to_string())); // 個別失敗は握らず JSON 化して返す
            record_tool_outcome(ctx, &fc.name, &out /* success/error, latency */);
            response_parts.push(Part { function_response: Some(FunctionResponse {
                name: fc.name.clone(),
                response: out.into_json(),
                id: fc.id.clone(),               // ★並行相関
            }), ..Default::default() });
        }

        contents.push(cand.content);                                     // model の functionCall 入り content
        contents.push(Content { role: Role::User, parts: response_parts }); // functionResponse を返送
        on_status(Status::Writing);
        resp = client.generate(system_instruction, &registry.declarations(), contents, None).await?;
        iterations += 1;
    }

    if iterations >= max_iterations { /* browser_tool_failed = true 相当 or GeminiError::MaxIterations */ }
    let text = resp.candidates.first().map(collect_text).unwrap_or_default();
    Ok(LoopResult { text, /* .. */ })
}
```

**完了是正の判定**（`claims_action_completed`、[`src/gemini.ts:480-495`](../../src/gemini.ts)）：現行の日本語正規表現 2 本（`(登録|追加|…)(し(ました|ておきました|…))` と `(やって|して)おき(ました|…)`）をそのまま Rust `regex` へ移植。`COMPLETION_CORRECTION_PROMPT` も文言直移植。

**3経路の共有（`runPlannedTurn` [`src/gemini.ts:793-926`](../../src/gemini.ts)）**：ターンプランナー（軽量 LLM でプラン→systemInstruction 注入）、予測非同期（`deferred`）／実行時エスカレーション（`onInterim`）、最終文面の会話ログ保存、シナプス抽出（`onFinal`）を共通クロージャ化。プランの候補ツールを実在ツールへ絞って `allowed_tool_names` に渡す点も保持。

### 8.2.5 マルチモーダル・SSE・モデル

- **マルチモーダル**：`inlineData{ mimeType, data(base64) }` でレシート／音声を渡す（現行 [`src/gemini.ts:1035-1050`](../../src/gemini.ts) と一致）。**inline は総リクエスト 20MB まで**（公式）。レシート1枚・ボイスメモは inline で十分。**20MB 超のみ Files API**（`files.upload`→`fileData{fileUri,mimeType}`、将来対応）。画像を `reqwest` で取得 → `base64::engine::general_purpose::STANDARD.encode(bytes)`。対応 MIME：`image/png|jpeg|webp|heic|heif`、音声は現行 `SUPPORTED_AUDIO_TYPES`。
- **SSE（任意・パリティ上は不要）**：現行は非ストリーミング（一括＋疑似「入力中…」）。導入時は `streamGenerateContent?alt=sse`（**`alt=sse` 必須**、無いと巨大 JSON 配列）＋ `reqwest::Response::bytes_stream()` → `eventsource-stream 0.2.3` → 各 `data:` 行を `serde_json::from_str::<GenerateContentResponse>`。**行バッファ必須**（チャンク境界で JSON が割れる）。FC 併用時は「functionCall はストリーム完結後に実行」する制御が要る。
- **モデル**：`gemini-3.1-flash-lite`（**GA・変更不要**、[`verification/rpt-gemini-design-verify.md`](verification/rpt-gemini-design-verify.md)）。`-preview` 名は使わない（`gemini-3.1-flash-lite-preview` は 2026-07-09 廃止）。モデルはユーザー設定（`conf.model`）／Bot 既定（`BOT_DEFAULT_MODEL` [`src/services/llmClient.ts:19`](../../src/services/llmClient.ts)）から解決。API キーは `secrecy::Secret<String>` で保持し、キャッシュ（現行 `userAICache`/`botAICache`）はキー変更で無効化。

### 8.2.6 補助生成（Function Call なし）

現行 `generateAuxText` / `generateAuxMultimodal`（[`src/services/llmClient.ts:122-198`](../../src/services/llmClient.ts)、タグ自動付与・要約・文字起こし等）は、同じ GeminiClient の `tools` 無し呼び出し＋短いリトライ（`maxRetries=2`）で移植。失敗時は現行同様 `None` を返しフォールバック（呼び出し側で縮退）。

---

## 8.3 第5部（supervisor）との接続点

- 各テナント Shard task は **supervisor の JoinSet 配下**。panic は `JoinError.is_panic()` で検知され指数バックオフ再 spawn（絶対制約2）。`panic = "unwind"` 厳守（[`00-decisions.md`](00-decisions.md)）。
- **停止協調**：`tokio-graceful-shutdown 0.19.3` の `ShutdownToken` を各 task の `select!` に配線（現行 `stopBot`/`stopCustomBot` [`src/bot.ts:1411-1508`](../../src/bot.ts) の置換）。SIGTERM 伝播でギルド/DM 応答を安全に打ち切る。
- **劣化縮退**：Gemini サーキットブレーカ open → ユーザーへ定型応答（現行 `guildErrorResult` [`src/gemini.ts:1358-1382`](../../src/gemini.ts)）。synapse/Redis ダウン時の縮退は第5部・DB 部の方針に従う。
- **致命 fail-fast の限定**：無効トークン・設定不備は起動時に検出（`ConfigError`）。実行時の一時障害（429/5xx/接続断）は自己復帰対象で落とさない。

## 8.4 第9部（ToolProvider）との接続点

- Gemini の function calling レジストリ（現行 `buildFunctionRegistry` [`src/functions/registry.ts`](../../src/functions/registry.ts)）は **第9部の `ToolProvider` トレイト＋中央レジストリ**へ置換。リクエスト毎に全 provider から `list()` → `FunctionDeclaration[]` を**動的生成**（現行の毎ターン再構築＝正しいパターン）。
- **スキーマは `parametersJsonSchema`（フル JSON Schema）を使用**（[`verification/rpt-gemini-funccalling-mcp.md`](verification/rpt-gemini-funccalling-mcp.md)）：`$ref`/`$defs`/`additionalProperties`/`prefixItems` を受け付けバックエンドへそのまま転送。ただし **sanitizer で `$schema` 除去・`default` 除去/変換・`oneOf`/`allOf`→`anyOf`・深すぎるネスト平坦化**を通す（現行 `jsonSchemaToGeminiSchema` [`src/functions/mcpDynamic.ts`](../../src/functions/mcpDynamic.ts) の最小変換を強化）。レガシー `parameters`（OpenAPI サブセット）は使わない方針。
- **ツール名**：`[a-zA-Z0-9_:.-]`・最大 **128字**（generateContent 制約。現行の 63字/`mcpDynamic` はやや保守的）。ソース別 **namespace 接頭辞**（例 `mcp__github__create_issue`）で衝突回避（現行 `mcpFunctionName`＋`disambiguateFunctionName` を踏襲）。
- **dispatch**：`functionCall.{name, args, id}` を registry でルックアップ → `ToolProvider::invoke(ctx, name, args)` → `functionResponse.{name, response, id}`。**`id` を並行相関に保持**。`ToolContext` に `UserId`（＋ `guildId`）を載せ、能力スコープ・データ分離を型で強制（現行 `ToolContext` [`src/types/contracts.ts:17-33`](../../src/types/contracts.ts) を newtype 化）。個別ツール失敗は握り潰さず `{success:false, message}` JSON にして Gemini へ返す（現行 `dispatch` の catch と同挙動、ただし `PluginError` を経由）。
- **MCP**：現行 `mcpDynamic`/`mcpClient` は第9部の `McpProvider`（rmcp 2.0.0 client）に吸収。ネイティブは `NativeProvider`、非信頼ユーザー製は `WasmProvider`（Extism）。3系統を同一 `ToolProvider` として Gemini ループから透過的に扱う。

---

> 対象: **第9部 ユーザー拡張モジュール基盤**（`ToolProvider` / Native / MCP / WASM の三系統）と **第10部 フロント⇄Rust 型連携**（単一真実源・自動生成・機密フェイルクローズ）。
>
> 本部の全決定は [`../00-decisions.md`](00-decisions.md) の確定選定（#15〜#19）に厳密整合させる。ここでは *再決定はしない* — クレート/バージョン/採否は確定済みとして、設計とコード片へ落とし込む。
> 一次ソース: [`rpt-plugins-wasm-extism`](verification/rpt-plugins-wasm-extism.md) / [`rpt-mcp-rmcp`](verification/rpt-mcp-rmcp.md) / [`rpt-gemini-funccalling-mcp`](verification/rpt-gemini-funccalling-mcp.md) / [`rpt-typegen-tsrs-utoipa`](verification/rpt-typegen-tsrs-utoipa.md)。
> 現行実装: [`src/functions/registry.ts`](../../src/functions/registry.ts) / [`src/functions/mcpDynamic.ts`](../../src/functions/mcpDynamic.ts) / [`src/services/mcpClient.ts`](../../src/services/mcpClient.ts) / [`src/types/contracts.ts`](../../src/types/contracts.ts) / [`src/types/apiViews.ts`](../../src/types/apiViews.ts) / [`frontend/src/lib/api/types.ts`](../../../frontend/src/lib/api/types.ts) / [`frontend/src/lib/api/client.ts`](../../../frontend/src/lib/api/client.ts)。

---

## 9. ユーザー拡張モジュール基盤（将来のカスタム拡張）

### 9.0 現行の姿と移行のゴール

現行 TS は「`FunctionModule` の集合を `buildFunctionRegistry` でマージ」する静的レジストリ（[`registry.ts`](../../src/functions/registry.ts)）と、「MCP サーバの `tools_cache` から `FunctionDeclaration` を動的生成」する `mcpDynamic`（[`mcpDynamic.ts`](../../src/functions/mcpDynamic.ts)）＋自前 JSON-RPC クライアント（[`mcpClient.ts`](../../src/services/mcpClient.ts)）の二層で成り立つ。両者は Gemini の宣言配列と名前→ハンドラの `Map` を吐く点で構造が同じだが、**別コードパスで重複**している。

Rust 版のゴールは、この二つ（＋将来の非信頼ユーザープラグイン）を **単一の `ToolProvider` トレイト**の背後に統一し、中央レジストリが「宣言生成」「ディスパッチ」「namespace 衝突回避」「能力スコープ/データ分離」を*一箇所で*強制することにある。これは確定選定 #15（`ToolProvider` trait レジストリ／Native・MCP・WASM の三系統）の実装計画である。

三系統の役割分担（[`rpt-plugins-wasm-extism`](verification/rpt-plugins-wasm-extism.md) の安全性ランキングに直結）:

| Provider | 実体 | 信頼境界 | 現行対応 |
|---|---|---|---|
| **NativeProvider** | 内蔵 Rust トレイト | 一級（自作コード） | `src/functions/*` |
| **McpProvider** | rmcp 2.0.0 client（外部 MCP サーバ接続） | **半信頼**（stdio=親権限を継承 / HTTP=別プロセス） | `mcpDynamic` + `mcpClient` |
| **WasmProvider** | Extism 1.30.0（wasmtime 上） | **非信頼**（deny-by-default サンドボックス） | 無（新規） |

> **動的 `.so`（libloading / abi_stable / stabby）は非信頼コードに不採用。** [`rpt-plugins-wasm-extism`](verification/rpt-plugins-wasm-extism.md) が明言する通り abi_stable は *"doesn't include a sandbox, so if the plugin developer was a malicious actor, they'd have full access to the computer"* — サンドボックス皆無でホスト完全侵害。加えて **panic-across-FFI は UB**、`repr(Rust)` レイアウトは非安定でバージョン不一致 `.so` は silent メモリ破損。abi_stable 自体が 0.11.3（2023-10-12 が最終）で低メンテ。一級（自作）プラグインなら許容だが、ユーザー製カスタム拡張（=非信頼）には**絶対に採用しない**。

---

### 9.1 `ToolProvider` トレイトと `ToolSpec` / `ToolContext` / `ToolOutput`

中核トレイト。`async fn in trait`（Rust 1.96 stable、RPITIT）を使い `#[async_trait]` は不要。エラーは確定選定 #4 に従い **`thiserror` の具体列挙型 `PluginError`**（`anyhow` 禁止）。

```rust
use std::sync::Arc;
use serde_json::Value;

/// ツール1件の宣言（Gemini functionDeclarations 生成の中間表現、provider 非依存）。
pub struct ToolSpec {
    /// 完全修飾ツール名（namespace 接頭辞込み・後述の制約を必ず満たす）。
    pub name: ToolName,
    pub description: String,
    /// フル JSON Schema（parametersJsonSchema へ流す。sanitizer 通過前の生スキーマ）。
    pub parameters_json_schema: Value,
    /// 実行前にユーザー承認を要するか（現行 requires_confirmation 相当）。
    pub requires_confirmation: bool,
}

/// ツール実行結果。テキスト/構造化/マルチモーダル parts を表現できる。
pub struct ToolOutput {
    /// Gemini functionResponse.response に載せる本文（JSON 文字列 or Struct）。
    pub payload: Value,
    /// マルチモーダル functionResponse.parts（Gemini 3。画像等。無ければ空）。
    pub parts: Vec<ResponsePart>,
}

#[async_trait_free] // = 素の async fn in trait（RPITIT）。表記は説明用。
pub trait ToolProvider: Send + Sync {
    /// この provider が現時点で公開する全ツール（namespace 接頭辞は provider 側で付与済み）。
    fn list(&self, ctx: &ToolContext) -> Vec<ToolSpec>;

    /// 単一ツールの実行。name は完全修飾名。args は sanitizer 通過後の引数。
    async fn invoke(
        &self,
        name: &ToolName,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, PluginError>;
}
```

`ToolContext` は現行 [`contracts.ts`](../../src/types/contracts.ts) の `ToolContext` を Rust newtype で写経しつつ、**データ分離キーを型で必須化**する（確定選定「`UserId` newtype を全リポジトリ署名に通す」の延長）。Discord 依存型（`EmbedBuilder` 等）は twilight の型に差し替える（第7部 Discord）。

```rust
/// ツール実行コンテキスト。UserId が全データ分離の必須キー（現行 contracts.ts:ToolContext）。
pub struct ToolContext {
    pub bot_id: BotId,
    /// データ分離キー。全リポジトリ呼び出し・能力スコープ判定に必須（newtype で欠落を型排除）。
    pub user_id: UserId,
    /// ギルド常駐 Bot のみ Some（DM/秘書利用では None）。
    pub guild_id: Option<GuildId>,
    /// この呼び出しに許された能力集合（後述 CapabilitySet。deny-by-default）。
    pub capabilities: CapabilitySet,
    /// リッチ返信キュー（richReplyEnabled=false のとき push 禁止は呼び出し側で担保）。
    pub embeds: Vec<Embed>,
    pub files: Vec<Attachment>,
    pub rich_reply_enabled: bool,
}
```

`PluginError`（層別具体列挙型・`#[non_exhaustive]`。`#[from]` は真の層境界のみ、乱用禁止 — 確定選定 #4）:

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    #[error("ツール名が制約に違反しています: {0}")]
    InvalidToolName(String),
    #[error("ツールが見つかりません: {0}")]
    UnknownTool(String),
    #[error("このツールは現在利用できません（無効化/削除/権限外）")]
    Unavailable,
    #[error("必要な能力が付与されていません: {capability}")]
    CapabilityDenied { capability: String },
    #[error("引数がスキーマに適合しません: {0}")]
    InvalidArguments(String),
    /// MCP client 層由来（rmcp）。層境界なので #[from] 可。
    #[error(transparent)]
    Mcp(#[from] McpError),
    /// WASM 実行層由来（Extism）。層境界なので #[from] 可。
    #[error(transparent)]
    Wasm(#[from] WasmError),
    #[error("ツール実行がタイムアウトしました")]
    Timeout,
}
```

> **設計上の要点:** 現行 `dispatch` はハンドラ内で `try/catch` してエラーを `{success:false,message}` の JSON 文字列に丸めている（[`registry.ts`](../../src/functions/registry.ts) L59-71）。Rust では `Result<ToolOutput, PluginError>` を返し、**JSON への丸め込みは第8部 Gemini 側の `functionResponse` 組み立てで一元化**する（provider は握り潰さない＝確定選定「エラー握り潰し禁止」に整合）。ただしクライアントへ返す `message` は `DbError` 等の内部 Display を漏らさず丸める。

---

### 9.2 ツール名 namespace と Gemini 制約の型強制

Gemini の `FunctionDeclaration.name` 制約は [`rpt-gemini-funccalling-mcp`](verification/rpt-gemini-funccalling-mcp.md) が REST リファレンスから確認済み: **`a-z / A-Z / 0-9 / _ : . -`、最大 128 文字**。現行 TS はここを **63 文字・`[a-zA-Z0-9_]` のみ**という*より厳しい旧制約*で切っている（[`mcpDynamic.ts`](../../src/functions/mcpDynamic.ts) L19-28）が、これは古い前提。Rust 版では現行の実測制約（128 字 / `:` `.` `-` 許容）へ緩和しつつ、**newtype `ToolName` のスマートコンストラクタで一元検証**して「無効名を型的に作れない」ようにする。

```rust
/// Gemini functionDeclarations.name 制約を満たすことを型で保証する newtype。
/// 制約: [a-zA-Z0-9_:.-] のみ・1..=128 文字（rpt-gemini-funccalling-mcp 実測）。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ToolName(String);

impl ToolName {
    pub const MAX_LEN: usize = 128;

    /// provider 種別ごとの namespace 接頭辞を付けて構築する。
    /// 例: Native → "native:contacts_save" / MCP → "mcp7:list_items" / WASM → "wasm:<plugin>:run"
    /// 衝突は provider 内で決定的短ハッシュ退避（現行 disambiguateFunctionName 相当）。
    pub fn namespaced(prefix: &str, raw: &str) -> Result<Self, PluginError> {
        // ':' は namespace 区切りとして使うが Gemini は ':' を許すのでそのまま採用可。
        let sanitized: String = raw
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '_' })
            .collect();
        let name = format!("{prefix}:{sanitized}");
        Self::checked(name)
    }

    fn checked(name: String) -> Result<Self, PluginError> {
        let ok = (1..=Self::MAX_LEN).contains(&name.len())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'));
        if ok { Ok(Self(name)) } else { Err(PluginError::InvalidToolName(name)) }
    }
    pub fn as_str(&self) -> &str { &self.0 }
}
```

**namespace 接頭辞規約**（衝突回避の核心。現行はサニタイズ/切り詰めで別ツールが同名化し 2 番目が silent に捨てられる事故を `disambiguateFunctionName` で防いでいる — [`mcpDynamic.ts`](../../src/functions/mcpDynamic.ts) L35-54）:

- `NativeProvider` → `native:<fn>`（例 `native:contacts_save`）
- `McpProvider` → `mcp<serverId>:<tool>`（現行 `mcp{serverId}_{tool}` の踏襲。`:` 区切りへ）
- `WasmProvider` → `wasm:<pluginId>:<export>`

128 字を超える場合／サニタイズ後に同一 provider 内で衝突した場合は、決定的 SHA-1 短ハッシュ（6 桁）を末尾付与して一意化する（現行ロジックを Rust へ 1:1 移植）。中央レジストリは *全 provider 横断*で `HashMap<ToolName, Arc<dyn ToolProvider>>` を張り、二重登録は起動時/生成時にエラー化する（現行 `registry.ts` の重複検出 L28-34 に対応）。

---

### 9.3 中央レジストリ

現行 `buildFunctionRegistry` の役割（宣言集約・重複検出・`has`・`dispatch`）を担う。ただし静的マージではなく **provider の集合を保持し、リクエスト毎に `list`/`invoke` を叩く**（MCP/WASM は動的だから）。

```rust
pub struct ToolRegistry {
    providers: Vec<Arc<dyn ToolProvider>>,
}

impl ToolRegistry {
    /// 全 provider の list を集約し、ToolName→provider の索引を張る。
    /// 名前衝突（provider 横断）は退避名で一意化しつつログ警告（現行 mcpDynamic の挙動）。
    pub fn snapshot(&self, ctx: &ToolContext) -> RegistrySnapshot {
        let mut specs = Vec::new();
        let mut index: HashMap<ToolName, Arc<dyn ToolProvider>> = HashMap::new();
        for p in &self.providers {
            for spec in p.list(ctx) {
                if index.contains_key(&spec.name) {
                    tracing::warn!(tool = spec.name.as_str(), "ツール名衝突: 退避します");
                    // 決定的ハッシュ退避（省略）
                }
                index.insert(spec.name.clone(), Arc::clone(p));
                specs.push(spec);
            }
        }
        RegistrySnapshot { specs, index }
    }
}

pub struct RegistrySnapshot {
    pub specs: Vec<ToolSpec>,           // → 第8部 functionDeclarations 生成へ
    index: HashMap<ToolName, Arc<dyn ToolProvider>>,
}

impl RegistrySnapshot {
    pub async fn dispatch(
        &self,
        name: &ToolName,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, PluginError> {
        let provider = self.index.get(name).ok_or_else(|| PluginError::UnknownTool(name.as_str().into()))?;
        provider.invoke(name, args, ctx).await
    }
}
```

> **リクエスト毎の snapshot が正しい理由:** [`rpt-gemini-funccalling-mcp`](verification/rpt-gemini-funccalling-mcp.md) が指摘する通り、Gemini は tools/config を**インタラクション毎に再送する前提**（Interactions API で `previous_interaction_id` を使っても再送必須）。動的 provider（MCP/WASM）はユーザーのスコープで見えるツールが変わるので、`snapshot(ctx)` を毎回作るのは*意図された設計*でありアンチパターンではない。

---

### 9.4 NativeProvider

現行 `src/functions/*` の内蔵機能（連絡先/家計/予定/配達 等）を束ねる。各機能は Rust トレイト実装で登録し、`ToolSpec` は `schemars` で Rust 引数型から JSON Schema を導出（`parametersJsonSchema` へ直行できるフル JSON Schema）。namespace は `native:`。ここは信頼コードなのでサンドボックス不要、`ctx.user_id` によるデータ分離のみ厳守する。

---

### 9.5 McpProvider — rmcp 2.0.0（公式SDK, client）

確定選定 #16。現行の**自前 JSON-RPC 実装**（[`mcpClient.ts`](../../src/services/mcpClient.ts) の `rpcRequest`/`parseSseBody`/`ensureInitialized`/`callRpc`）を **rmcp 2.0.0 の client へ全面置換**する。[`rpt-mcp-rmcp`](verification/rpt-mcp-rmcp.md) の通り rmcp は `modelcontextprotocol` org 所有の公式 SDK、2.0.0（2026-06-29）、spec **2025-11-25** をターゲット、server/client 両対応。

**トランスポート対応**（[`rpt-mcp-rmcp`](verification/rpt-mcp-rmcp.md)）:

- **stdio**: `TokioChildProcess::new(Command::new("npx")…)` で外部サーバを子プロセス起動。ローカル同梱の一級プラグイン向け。
- **Streamable HTTP**: `StreamableHttpClientTransport`。現行が実装している経路（`endpoint_url` への JSON-RPC over HTTP、SSE 応答パース）はこれに一致 — **自前 SSE パーサ（[`mcpClient.ts`](../../src/services/mcpClient.ts) L82-109）は rmcp が内包するので破棄**できる。

```rust
use rmcp::{ServiceExt, transport::{TokioChildProcess, StreamableHttpClientTransport}};

pub struct McpProvider {
    /// 接続済みクライアント（1 外部サーバ = 1 client）。tools/list_changed で再取得。
    clients: HashMap<McpServerId, McpClientHandle>,
    /// serverId×tool → 完全修飾 ToolName の対応表（namespace 済み）。
    catalog: HashMap<ToolName, (McpServerId, String /*raw tool name*/)>,
}
```

**現行資産の吸収マッピング:**

| 現行 TS（[`mcpClient.ts`](../../src/services/mcpClient.ts) / [`mcpDynamic.ts`](../../src/functions/mcpDynamic.ts)） | rmcp / Rust 版 |
|---|---|
| `rpcRequest` / `parseSseBody` / 手書き JSON-RPC | rmcp `Transport` + `serve()`（自前廃止） |
| `ensureInitialized`（initialize→initialized 通知） | rmcp のハンドシェイクが内包 |
| `listTools` → `tools_cache` | `list_all_tools()`（ページネーション集約込み） |
| `callTool`（content.text 連結・`isError` 判定） | `call_tool()` の結果 content を `ToolOutput` へ |
| `refreshToolsCache` + TTL 1h（[`mcpDynamic.ts`](../../src/functions/mcpDynamic.ts) L21,177-202） | TTL 再取得は維持しつつ、**`notifications/tools/list_changed` 購読で能動更新**（rmcp 対応。ホットリロード可） |
| `buildAuthHeader`（AES 復号 Bearer 注入） | rmcp transport のヘッダ設定へ移植。in-memory の平文資格情報は **`secrecy::SecretString`** で保持（第10部と共通方針） |
| `McpToolError`（`isError:true` は再試行しない） | `PluginError::Mcp` で表現。副作用ある `tools/call` の二重実行防止方針を維持 |
| SSRF 再検証（`assertSafeOutboundUrl`、DNS リバインディング対策） | **維持必須**。rmcp の HTTP transport 送出直前に宛先再検証フックを噛ませる（プライベート IP 到達遮断は自前 SSRF ガードを移植） |
| `requires_confirmation`（実行前ユーザー承認） | `ToolSpec.requires_confirmation` へ |

**スコープ/データ分離の吸収**: 現行の最重要ロジック — 共有秘書（`system_default`）では**発話者本人が付与した許可分＋システムレベル（`user_id IS NULL`）のみ**に絞り、他人の資格情報を抱えた MCP サーバが発話者の会話へ漏れないようにする（[`mcpDynamic.ts`](../../src/functions/mcpDynamic.ts) L326-361）— は `McpProvider::list(ctx)` 内で `ctx.user_id` を使って*リクエスト毎に*再評価する。**呼び出し時点の再検証**（現行 `isStillAvailable` クロージャ L271-279）も `invoke` 冒頭で同じスコープ判定を再実行して `PluginError::Unavailable` を返す（無効化/削除/権限外の TOCTOU を塞ぐ）。監査ログ（現行 `mcp.call` に `botId`/`credentialOwner` を記録 L294-302、秘密値は含めない）も踏襲。

**aggregator/gateway パターン**: [`rpt-mcp-rmcp`](verification/rpt-mcp-rmcp.md) の「virtual MCP server / gateway」= N サーバに接続しツールを集約して単一カタログとして再公開する構図は、まさに McpProvider の設計そのもの。namespace 接頭辞（`mcp<serverId>:`）が gateway の衝突回避に対応する。

---

### 9.6 WasmProvider — Extism 1.30.0（非信頼ユーザープラグイン）

確定選定 #17。**非信頼なユーザー製プラグイン**専用。[`rpt-plugins-wasm-extism`](verification/rpt-plugins-wasm-extism.md) が示す通り、Extism は wasmtime 上の高レベル抽象で「文字列/バイト/JSON をやり取りする既製 ABI」を提供し、*"fully sandboxes the execution of all plug-in code"*、WASI を superset として持ちつつ**システム資源アクセスは deny-by-default**。polyglot PDK（Rust/Go/JS/Zig 等）でユーザーは好きな言語でプラグインを書ける。

`extism` crate 1.30.0（2026-06-04、host SDK、wasmtime backing）を採用。

```rust
use extism::{Manifest, Wasm, Plugin, PTR};

pub struct WasmProvider {
    plugins: HashMap<PluginId, WasmPluginEntry>,
}

struct WasmPluginEntry {
    manifest: Manifest, // deny-by-default で構築（下記）
    spec: Vec<ToolSpec>,
}
```

**deny-by-default マニフェスト**（能力付与は明示のみ。[`rpt-plugins-wasm-extism`](verification/rpt-plugins-wasm-extism.md) の `allowed_hosts`/`allowed_paths`/memory/timeout。※ SDK バージョンで正確なフィールド名は docs.rs `Manifest` 要確認＝レポートで MEDIUM フラグ）:

```rust
fn build_sandbox_manifest(wasm: Wasm, grant: &CapabilityGrant) -> Manifest {
    Manifest::new([wasm])
        // 既定は空 = 何も触れない。付与された分だけ開ける（フェイルクローズ）。
        .with_allowed_hosts(grant.allowed_hosts.iter().cloned()) // 送信 HTTP 先の allowlist
        .with_allowed_paths(grant.allowed_paths.iter().cloned()) // FS マッピング（原則 空）
        .with_memory_max(grant.memory_max_pages)                 // 線形メモリ上限
        .with_timeout(grant.timeout)                             // 実行時間上限（無限ループ殺し）
}
```

**host functions**: ユーザープラグインへ「安全に露出してよい能力」だけを host function として渡す（DB 直アクセスは渡さず、`ctx.user_id` スコープ済みの高レベル API のみ）。`UserData` は共有プラグインプールで `Send + Sync` 必須。

**能力モデル / ライフサイクル:**
- プラグインは `Manifest` から `Plugin::new` で生成、更新版バイト列で作り直す＝ホットリロード（[`rpt-plugins-wasm-extism`](verification/rpt-plugins-wasm-extism.md) は専用 API 無しと注記＝MEDIUM。作り直しで対応）。
- 実行前に `ctx.capabilities`（`CapabilitySet`）と `CapabilityGrant` を突き合わせ、マニフェストへ反映。付与外の host/path はマニフェストに載らない＝**構造的に到達不能**。
- タイムアウト/メモリ超過は `WasmError` → `PluginError::Wasm`/`Timeout` へ。プロセスは殺さない（wasmtime のサンドボックス内で完結）。

> **標準志向の代替（記録のみ・初期不採用）:** 生 wasmtime 46 + Component Model + WASI 0.2（stable）。WIT + `wit-bindgen 0.58` で型付きインターフェース、fuel/epoch/`ResourceLimiter` で細粒度の資源制御。Extism より定型コードは増えるが標準トラック。確定選定は Extism 優先（ergonomics）。**Extism は独自 ABI で Component Model 非採用**（[`rpt-plugins-wasm-extism`](verification/rpt-plugins-wasm-extism.md) MEDIUM）である点は将来の移行検討事項として記録。

---

### 9.7 第8部 Gemini との接続（宣言生成・並行相関・sanitizer）

第8部（Gemini）から見た本基盤の接続点:

1. **リクエスト毎の宣言生成**: `RegistrySnapshot::specs` → 各 `ToolSpec.parameters_json_schema` を **`parametersJsonSchema`**（フル JSON Schema）へ載せる。[`rpt-gemini-funccalling-mcp`](verification/rpt-gemini-funccalling-mcp.md) の最重要事実 — `parametersJsonSchema` は `$ref`/`$defs`/`additionalProperties`/`prefixItems` を許容しバックエンドへ直送されるため、旧 `parameters`（OpenAPI 3.0.3 サブセット）路より sanitize が少なくて済む。現行 `jsonSchemaToGeminiSchema`（[`mcpDynamic.ts`](../../src/functions/mcpDynamic.ts) L60-174）の重い down-conversion は**大幅に不要化**する。

2. **sanitizer（それでも必要）**: `$schema` 除去（ルートで拒否される）、`default` の除去/変換、`oneOf`/`allOf`→`anyOf` 畳み込み、`additionalProperties` エッジケースのガード、過度なネストの平坦化。provider 非依存の共通 sanitizer を snapshot→宣言生成の間に一段挟む（LibreChat 等が実装する定石）。

3. **並行相関 `functionCall.id`**: [`rpt-gemini-funccalling-mcp`](verification/rpt-gemini-funccalling-mcp.md) の通り 2026-03-17 更新で per-call `id` が正式追加。並行呼び出しでは 1 応答に複数 `functionCall` パートが返る。各 `functionCall`(`name`,`args`,`id`) を `RegistrySnapshot::dispatch` へ回し、`ToolOutput` を `functionResponse`(`name`,`response`,同一 `id`；マルチモーダルは `parts[]`) へ組み立てる。`id` を保持して相関を崩さない。ループは `maxIterations` で制限（`ANY` モードの無限ループ失敗モード対策）。

4. **`functionCallingConfig` モード**: `AUTO`（既定）/`ANY`+`allowedFunctionNames`（完了ハルシネーション是正）/`NONE`/`VALIDATED`(Preview)。`allowedFunctionNames` には `RegistrySnapshot` の namespace 済み `ToolName` をそのまま流せる。

> **MCP のもう一つの配線（不採用の記録）:** Gemini REST には `Tool.mcpServers[]`（サーバ側 MCP 委譲）や SDK の `ClientSession` 直渡し（クライアント側 auto-FC）がある（[`rpt-gemini-funccalling-mcp`](verification/rpt-gemini-funccalling-mcp.md)）。しかし本基盤は **MCP を単なる `ToolProvider` として自前ループで扱う**方針（WASM/Native と統一的な sanitize・`id`・namespace・スコープ管理を効かせるため）。Google 委譲はコードは減るが per-call 制御を失い SDK experimental のため採らない。

---

### 9.8 初期スコープの提案（[`00-decisions.md`](00-decisions.md) 未解決 #5 への回答）

未解決事項 #5（3系統を最初から揃えるか、WASM を後続にするか）に対する本部の推奨:

- **フェーズ A（移行同時）**: NativeProvider ＋ McpProvider を先行。これで現行 `src/functions/*` と `mcpDynamic`/`mcpClient` のパリティを達成（機能後退ゼロ）。
- **フェーズ B（後続）**: WasmProvider を追加。非信頼プラグインは新機能であり現行に対応物が無く、サンドボックス/能力モデル/マニフェスト UI 等の追加設計を要するため、パリティ達成後に切り出すのがリスク最小。トレイト境界（`ToolProvider`）は最初から確定させておくので、B の追加は*レジストリへ provider を 1 つ足すだけ*で済む。

---

## 10. フロント⇄Rust 型連携（単一真実源・自動生成・機密フェイルクローズ）

### 10.0 現行の二重管理と移行のゴール

現行フロントは手書きの [`frontend/src/lib/api/types.ts`](../../../frontend/src/lib/api/types.ts)（655 行）に `BotView` 等を**手で写経**しており、サーバ側の zod ビュー（[`apiViews.ts`](../../src/types/apiViews.ts)）と**二重管理**になっている（`BotView` が両側に存在。ズレたら実行時まで気付けない）。[`rpt-typegen-tsrs-utoipa`](verification/rpt-typegen-tsrs-utoipa.md) を根拠に、これを **Rust を単一真実源とする自動生成**へ置換する（確定選定 #18）。

### 10.1 第一推奨: ts-rs 12.0.1（型のみ・単一真実源）

確定選定 #18。[`rpt-typegen-tsrs-utoipa`](verification/rpt-typegen-tsrs-utoipa.md) の通り ts-rs **12.0.1**（2026-01-31、stable、活発、`Aleph-Alpha/ts-rs`）は「Rust HTTP バックエンド + Vite SPA の共有型」に最も直接的に合致。`#[derive(TS)]` ＋ `#[ts(export)]` ＋ `cargo test` で `./bindings` へ `.ts` を書き出す（`TS_RS_EXPORT_DIR` で出力先変更可）。serde 属性（`rename`/`rename_all`/`tag`/`content`/`untagged`/`skip`/`flatten`/`default` 等）を尊重する。

```rust
use ts_rs::TS;
use serde::Serialize;

/// GET /api/bots 等が返す Bot ビュー DTO。現行 apiViews.ts:botViewSchema と 1:1。
/// 機密列（*_encrypted / *_iv / *_tag）は「フィールドとして存在しない」＝生成 TS にも現れない。
#[derive(Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")] // 現行 JSON キー（snake_case）を維持
pub struct BotView {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub recommended_persona_id: Option<i64>,
    pub persona_id: Option<i64>,
    pub capabilities: String,
    pub discord_username: Option<String>,
    pub discord_avatar_url: Option<String>,
    pub discord_application_id: Option<String>,
    pub suspended: i64,
    pub created_at: String,
    pub updated_at: String,
    pub preset: String,
    pub preset_display_name: String,
    pub has_gemini_key: bool,
    pub has_token: bool,
    pub running: bool,
    pub connected: bool,
    pub shared: bool,
}
```

> **ts-rs の落とし穴（[`rpt-typegen-tsrs-utoipa`](verification/rpt-typegen-tsrs-utoipa.md) HIGH）:** `skip_serializing`/`skip_serializing_if` は **`#[serde(default)]` 併用時のみ**生成型へ反映される。除外したいだけなら `#[ts(skip)]` を使う。ただし後述の通り、機密は**そもそも DTO のフィールドに置かない**方針なのでこの落とし穴には基本触れない。

**上位互換の選択肢（[`00-decisions.md`](00-decisions.md) 未解決 #1）**: エンドポイント契約＋型付きクライアントまで欲しい場合は `utoipa 5.5.0 → OpenAPI 3.1 → openapi-typescript 7.13.0(+openapi-fetch)`。utoipa は `utoipa-axum` で axum ハンドラ登録と spec 生成を同時に行える。ts-rs は「型のみ」なので契約（どのルートがどの型を返すか）は型で縛れない — 現行 [`client.ts`](../../../frontend/src/lib/api/client.ts) の `api.get<T>()` のように *呼び出し側が T を指定*する形は残る。**既定は ts-rs**（低リスク・二重管理解消が主目的）、契約まで必要になった時点で utoipa へ格上げ。

> **specta 却下理由（[`rpt-typegen-tsrs-utoipa`](verification/rpt-typegen-tsrs-utoipa.md)）:** v2 は 2026-07 時点で **RC のまま安定版なし**（`max_stable_version` は 1.0.5）。強みは Tauri 特化（typed commands/events）でブラウザ SPA には効かない。E2E RPC の要 rspc は **2025-03-12 に公式にメンテ終了**。本スタック（Rust HTTP + Svelte/Vite）には不適。

### 10.2 機密フェイルクローズ — 「専用 DTO struct」で構造的保証

確定選定 #19。現行の防御は zod allowlist（[`apiViews.ts`](../../src/types/apiViews.ts)）: `z.object()` がスキーマ外キーを strip し、生 DB レコードが紛れても機密列（`*_encrypted`/`*_iv`/`*_tag`・`password_hash`・`salt`）が応答に出ない — *実行時*のフェイルクローズ。

Rust 版はこれを **コンパイル時保証**へ格上げする。核心は「**機密列を DTO struct のフィールドに存在させない**」こと。DB/ドメインモデル（機密列を持つ）と応答 DTO（安全列のみ）を**別 struct**にし、`Serialize`/`TS` は DTO 側のみに実装する。機密フィールドは*型に存在しない*ので、

- **シリアライズできない**（存在しないフィールドは JSON に出しようがない＝真のフェイルクローズ by construction）、
- **生成 TS にも現れない**（漏洩が型的に不可能）、
- 将来 DB に機密列を足しても、DTO に手で足さない限り**露出経路が生まれない**（zod allowlist は「新機密列を strip し忘れなければ安全」だが、DTO 分離は「足し忘れる＝安全側」）。

```rust
// ドメイン/DB モデル（機密列を持つ。Serialize も TS も導出しない＝配線ミスでも wire に出せない）
pub struct BotRecord {
    pub id: String,
    pub discord_token_encrypted: Vec<u8>, // ← 機密。DTO には存在しない
    pub gemini_key_encrypted: Vec<u8>,    // ← 機密
    pub gemini_key_iv: Vec<u8>,
    // ...安全列...
}

impl BotRecord {
    /// 応答 DTO への明示投影。機密列はここで「単に載せない」。
    pub fn to_view(&self, runtime: &BotRuntimeState) -> BotView {
        BotView {
            id: self.id.clone(),
            // 機密列は has_* の真偽値のみ公開（現行 has_gemini_key / has_token と同じ思想）
            has_gemini_key: !self.gemini_key_encrypted.is_empty(),
            has_token: !self.discord_token_encrypted.is_empty(),
            running: runtime.running,
            connected: runtime.connected,
            // ...安全列を明示コピー...
        }
    }
}
```

**in-memory 機密は `secrecy`**（[`rpt-typegen-tsrs-utoipa`](verification/rpt-typegen-tsrs-utoipa.md)）: `SecretString`/`SecretBox` は `Debug` を redact し drop で zeroize、`expose_secret()` でのみ露出。復号後の Discord トークン・Gemini キー・MCP 資格情報（§9.5）をメモリ保持する間はこれで包む。ログ/`Debug` 経由の漏洩も塞ぐ。

**`ApiResponse<T>` の型化**: 現行はレスポンスが `{ success, message? } & payload` の**トップレベル直置き**（`data` ラッパ不在。[`frontend/src/lib/api/types.ts`](../../../frontend/src/lib/api/types.ts) L13）。この形をサーバ側 Rust の serde でそのまま出し、フロントの `ApiResponse<T>` エンベロープ型も生成物と整合させる（`#[serde(flatten)]` でエンベロープと payload を平坦化するか、共通ラッパ struct を ts-rs で export）。現行 [`client.ts`](../../../frontend/src/lib/api/client.ts) の「`res.ok` と `data.success` の複合成否判定」はそのまま維持できる。

### 10.3 ビルド統合と CI ドリフト検出

**生成**（[`rpt-typegen-tsrs-utoipa`](verification/rpt-typegen-tsrs-utoipa.md) の canonical レシピ）:

- ts-rs: `#[ts(export)]` を付けて `cargo test`（または `xtask` バイナリ）で `./bindings` へ書き出し → Vite/tsconfig をそのディレクトリへ向ける（または `frontend/src/lib/api/generated/` へコピー）。生成物は git 追跡する。
- utoipa 経路（採る場合）: `ApiDoc::openapi()` を `openapi.json` へ直列化 → `npx openapi-typescript ./openapi.json -o ./src/lib/api/schema.d.ts`。

**CI ドリフト検出**（確定選定 #19 の「生成→`git diff --exit-code`」）:

```bash
# ts-rs
cargo test export_bindings          # ./bindings を再生成
git diff --exit-code -- ./bindings  # 差分あれば非0終了＝生成物が古い
# utoipa 経路なら openapi.json と schema.d.ts の両層で同じガード
```

`xtask` にまとめて `cargo xtask gen-bindings` の一手で回せるようにし、CI ジョブと `pre-commit` の双方から呼ぶ。

### 10.4 移行中の手書き types.ts との併存（strangler 期）

第11部（デプロイ・段階移行、nginx strangler）と整合させる。移行はルート単位で進むので、型も**ルート単位で切り替える**:

1. **DTO を Rust 側に定義した順**に ts-rs で生成 → `frontend/src/lib/api/generated/*.ts` へ。
2. フロントは当該ルートの import を手書き [`types.ts`](../../../frontend/src/lib/api/types.ts) から `generated/` へ**1 型ずつ差し替え**る。手書き `types.ts` は「まだ Rust 化していないルートの型」だけを残す縮小するファイルとして併存させる。
3. 差し替え済み型は手書き側から削除（重複定義でのズレを防ぐ）。両側に同名 `BotView` が残る期間は、生成物を*正*とし手書きを段階的に消す。
4. 全ルート移行完了時点で手書き `types.ts` は `ApiResponse<T>` 等の生成不能な純ユーティリティ型のみへ縮小、または消滅。

これにより、Node と Rust が nginx 背後で混在する strangler 期でも「移行済みルートは Rust 生成型・未移行ルートは手書き型」がファイル分離で共存し、CI ドリフト検出は生成対象ディレクトリ（`generated/`）のみに掛けて誤検知を避ける。

---

## 付録: 本部が参照する確定選定・一次ソースの対応表

| 本部の設計要素 | 確定選定 | 一次ソース |
|---|---|---|
| `ToolProvider` trait / 三系統 | #15 | [`rpt-plugins-wasm-extism`](verification/rpt-plugins-wasm-extism.md) |
| McpProvider（rmcp 2.0.0 client） | #16 | [`rpt-mcp-rmcp`](verification/rpt-mcp-rmcp.md) |
| WasmProvider（Extism 1.30.0） | #17 | [`rpt-plugins-wasm-extism`](verification/rpt-plugins-wasm-extism.md) |
| `.so` 不採用 | #15 注記 | [`rpt-plugins-wasm-extism`](verification/rpt-plugins-wasm-extism.md) §3 |
| 宣言生成 / `parametersJsonSchema` / `id` 相関 | — | [`rpt-gemini-funccalling-mcp`](verification/rpt-gemini-funccalling-mcp.md) |
| ts-rs 12.0.1（型のみ） | #18 | [`rpt-typegen-tsrs-utoipa`](verification/rpt-typegen-tsrs-utoipa.md) |
| utoipa 上位互換 / specta 却下 | #18 | [`rpt-typegen-tsrs-utoipa`](verification/rpt-typegen-tsrs-utoipa.md) |
| 専用 DTO / secrecy | #19 | [`rpt-typegen-tsrs-utoipa`](verification/rpt-typegen-tsrs-utoipa.md) §fail-closed |

---

> 本パートは [00-decisions.md](00-decisions.md)（確定 ADR）に厳密整合する。技術選定は再決定しない。
> 一次ソース照合は以下を相対リンク引用する:
> [rpt-nginx-session-strangler](verification/rpt-nginx-session-strangler.md) /
> [rpt-dual-sqlite-hazard](verification/rpt-dual-sqlite-hazard.md) /
> [rpt-migrations-sqlx-refinery](verification/rpt-migrations-sqlx-refinery.md) /
> [rpt-axum-web-runtime](verification/rpt-axum-web-runtime.md) /
> [rpt-resilience-tokio-backon-recloser](verification/rpt-resilience-tokio-backon-recloser.md)
>
> **本パートの位置づけ（最重要級）**: 第1〜10部が「何を作るか」を確定したのに対し、本パートは「**壊さずにどう置き換えるか（第11部）**」と「**それをどう高速に実装するか（第12部）**」を定める。前者は本番データ喪失・断続ログアウト・二重書き込みという不可逆事故の防波堤であり、後者は前者を並行実装で成立させる運用手順である。

---

## 第11部. 段階移行ロードマップ（ストラングラーフィグ）

### 11.0 前提事実（実ファイル確認済み・再検証不要）

- `src/db/database.ts` の PRAGMA は現状 `journal_mode=WAL` と `foreign_keys=ON` の 2 つのみ（`busy_timeout` は明示未設定だが、**better-sqlite3 の既定 5000ms が有効**＝[rpt-dual-sqlite-hazard §0](verification/rpt-dual-sqlite-hazard.md) の重大訂正）。
- `src/db/migrations.ts` の `SCHEMA_VERSION = "17"`。`system_settings` 不在時に `"1"` を返し**レガシー全 DROP 分岐**（`:880-894`）へ落ちる破壊経路が実在（[rpt-migrations-sqlx-refinery](verification/rpt-migrations-sqlx-refinery.md)）。
- `src/rust_synapse/src/storage.rs` は `SQLITE_OPEN_READ_ONLY` で開き `busy_timeout(3000)` 済み＝**read-only リーダーの稼働実績**（writer 実績ではない。[rpt-dual-sqlite-hazard §0](verification/rpt-dual-sqlite-hazard.md)）。
- `nginx/nginx.conf` は**空ディレクトリ**（Docker volume マウント先の器のみ、config 未コミット）。→ リバースプロキシ設定は**本移行の成果物として新規に書く**。
- Cargo は `rust_crawler` / `rust_synapse` の **2 クレートが独立**。ルート `Cargo.toml`（workspace）は存在しない。

### 11.1 移行順序の依存グラフ — foundation を最初に凍結

移行順序は「下流が上流に依存しない DAG」を厳守する。**foundation クレート群を最初に凍結**しないと、後続の並行実装（第12部）で全エージェントが型で衝突する。

```
                    ┌─────────────────────────────────────────────┐
  Phase F           │ yuuka-core   : error / config / secret / UserId │
  (単独先行・凍結)   │ yuuka-db     : rusqlite pool / writer actor    │
                    │ yuuka-types  : wire DTO + ts-rs 生成基盤        │
                    └───────────────┬─────────────────────────────┘
                                    │ (全クレートがここに依存)
        ┌───────────────┬───────────┼───────────┬───────────────┐
   Phase A          Phase B      Phase C     Phase D          Phase E
   認証/session     静的配信+     ドメイン    gemini/         discord bot
   (auth)           単純GET      route群      functions       (twilight)
        │            (me/status)  (並行可)     (tool trait)         │
        └────────────┴───────────┴───────────┴───────────┬───────┘
                                                          │
                                            Phase G  services/cron
                                                          │
                                            Phase H  daemon 吸収
                                                     (crawler/synapse) → /ws/chat 最終カットオーバー
```

移行の実行順（＝nginx で Rust へ回すルートを増やす順）と各フェーズの完了判定:

| # | フェーズ | 対象 | nginx で Rust へ回すもの | 前提 | 完了判定（全て満たして初めて切替） |
|---|---|---|---|---|---|
| **F** | foundation | error/config/secret/UserId/db pool/wire 型の 3 クレート | （まだ何も回さない・Rust は起動だけ） | — | `cargo build/clippy -D/deny check` + ts-rs 生成が空でも通る。型が **FROZEN** |
| **A** | 認証/セッション | `resolveRequestUser`, Redis session, desktop token, device flow | `/api/login /logout /me /api/auth/device/*` | F + **Redis 共有** | 下記 3 層ゲート + Node/Rust が同一 Redis セッションを相互に読める検証 |
| **B** | 静的+単純ルート | `serveStaticFile`, `/api/status`, `/api/setup/status` | 静的 `/`, `/assets/`, `/api/status` | A | 3 層ゲート + CSP/immutable/SPA fallback パリティ |
| **C** | 各ルート群 | todo→finance→schedule→timeline→reminder→personal→credential→playbook→persona | ドメイン単位で `^~ /api/tasks/` 等を順次 | A,B | ドメインごとに 3 層ゲート + カナリア SLO |
| **D** | gemini/functions | tool registry, planner, recall 注入 | （WS/bot 経路が使う内部層。HTTP は増えない） | C の repo | registry 重複名テスト + planner responseSchema テスト |
| **E** | discord bot | twilight マルチ接続, interaction | Discord 側は Node bot 停止で**排他カットオーバー** | D | bot supervisor 稼働 + Discord 側で Node/Rust 二重起動しない |
| **G** | services/cron | reminder/report/briefing/backup/… | cron は HTTP 非公開。Node cron 停止→Rust cron 起動 | C,D | cron パリティ + reminder 起動時即時実行で取りこぼし復帰 |
| **H** | daemon 吸収 | crawler/synapse を workspace 化・supervisor 監督 | **`/ws/chat` を Rust へ最終カットオーバー** | E,G | 全ゲート + Node 全停止 + 整数連番マイグレーションランナー導入 |

**各フェーズの完了判定＝3 層ゲート**（[rpt-axum-web-runtime](verification/rpt-axum-web-runtime.md)・[rpt-migrations-sqlx-refinery](verification/rpt-migrations-sqlx-refinery.md) と整合）:

1. **型ゲート**: `cargo build --workspace` + `cargo clippy --workspace --all-targets --all-features -- -D warnings` + `cargo deny check`（anyhow/eyre/color-eyre 混入をブロック）+ `git diff --exit-code`（ts-rs 生成物 drift 無し）。
2. **テストゲート（機能パリティ）**: `cargo test --workspace`。各ルート群は**ゴールデン差分テスト** —— 同一リクエストを Node(:7854) と Rust(:7900) の両方へ投げ、レスポンス JSON（success エンベロープ・キー名・snake/camel）とステータスが一致することを assert。
3. **verify ゲート（カナリア SLO）**: `deploy/instance.sh` の verify を流用（CSP `script-src 'self'`・hashed asset immutable・`/api/me` 200 を curl 検証。サーバ実装非依存でカットオーバー後の回帰検出にそのまま効く）。加えてフェーズ固有の不変条件（例: todo なら CSRF 403・10MB 413・`stripProtoKeys`・user_id スコープ越境が 403）と、カナリア期間中の**エラー率／p95 レイテンシ／`SQLITE_BUSY` 発生数**が SLO 内であること。

**なぜこの順か**:
- **A（認証）を最初**に回すのは、以降の全 user-scoped ルートが `resolveRequestUser` に依存し、Node と Rust が**同一 Redis セッションを読める**ことを検証してからでないと、C 以降を Rust に回した瞬間にログインが割れるため。
- **D（gemini）を C の後・E の前**に置くのは、tool ハンドラが C で移した repo 層（`todoRepo` 等の Rust 版）を呼び、bot（E）は D の tool registry に依存するため。
- **H を最後**にするのは、`/ws/chat` がターンキュー・1接続1Bot束縛・添付上限という状態機械を持ち、gemini(D)+bot(E)+services(G) が揃わないと等価な応答を返せないため。WS を最後にカットオーバーするのが最も回帰リスクが低い。

### 11.2 nginx strangler — 切替の唯一の真実源

旧 Node は `127.0.0.1:7854`、新 Rust は `127.0.0.1:7900`（別ポート）で並走させ、nginx が `location` 単位でどちらへ流すかを制御する。これがストラングラーの「絞め殺しダイヤル」になる。設計判断として**振り分け粒度は「プレフィックス `location` ブロック」に固定し、アプリ層フィーチャフラグや動的ルータは作らない**（切替状態を nginx 1 ファイルに集中させ、ロールバックを `nginx -s reload` 1 発にするため）。

```nginx
# /home/suki/web/kawaii-music.moe/nginx/nginx.conf/yuuka.conf （新規・本移行の成果物）
upstream yuuka_node { server 127.0.0.1:7854; keepalive 32; }
upstream yuuka_rust { server 127.0.0.1:7900; keepalive 32; }

# WebSocket の hop-by-hop Upgrade/Connection を条件転送（http ブロックに 1 つ）
map $http_upgrade $connection_upgrade { default upgrade; '' close; }

server {
    listen 443 ssl http2;
    server_name yuuka.kawaii-music.moe;   # 両 upstream は同一オリジン（__Host- Cookie 前提）

    # nginx は X-Forwarded-Proto を必ず立てる（Rust の HTTPS 判定誤作動防止）
    proxy_set_header Host              $host;
    proxy_set_header X-Real-IP         $remote_addr;
    proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto https;

    # --- 移行済みルート群を Rust へ（フェーズ進行で 1 行ずつ増やす。URI 無し＝パス保存） ---
    location = /api/me      { proxy_pass http://yuuka_rust; }   # 末尾 URI 無し=書換なし
    location = /api/login   { proxy_pass http://yuuka_rust; }
    location ^~ /api/tasks/ { proxy_pass http://yuuka_rust; }
    # location ^~ /api/expenses/ { proxy_pass http://yuuka_rust; }  # 次フェーズで解禁

    # --- WebSocket は接続が長命なので明示。移行末期（Phase H）まで Node 固定 ---
    location = /ws/chat {
        proxy_pass http://yuuka_node;          # Phase H で yuuka_rust へ最終切替
        # proxy_http_version 1.1;              # nginx 1.29.7 未満なら必須
        proxy_set_header Upgrade    $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_read_timeout 3600s;              # 既定 60s だとアイドルで切断→ping 併用
    }

    # --- 既定フォールバック: 未指定は全て Node（フェイルセーフ） ---
    location / { proxy_pass http://yuuka_node; }
}
```

**`proxy_pass` の末尾 URI 規約（静かに壊れる罠）**（[rpt-nginx-session-strangler A-1](verification/rpt-nginx-session-strangler.md) 逐語確認）:
- **URI を付けない**（`proxy_pass http://yuuka_rust;`）→ リクエスト URI がそのまま渡り、両バックエンドが同一パス空間を共有できる。**本移行はこれを標準**とする。
- URI を付ける（`.../;` 等）と `location` にマッチした部分が置換される（`/api/tasks/x` → `/x`）。付ける／付けないでルーティングが静かに壊れるため、末尾スラッシュ規約を新旧で一致させる。
- 正規表現 `location` と named location では `proxy_pass` に URI を付けてはならない。

**WebSocket**（[rpt-nginx-session-strangler A-2](verification/rpt-nginx-session-strangler.md) 逐語）: `Upgrade`/`Connection` は hop-by-hop でデフォルト転送されないため明示必須。`map $http_upgrade $connection_upgrade` で非 WS リクエストの keep-alive を壊さない。既定 60s タイムアウトで無通信接続が切れるので `proxy_read_timeout 3600s` 延長 + **バックエンドから定期 ping フレーム**（nginx 公式推奨）を併用。yuuka チャットは長時間アイドルしうるため ping 送出を強く推奨。

**Cookie/CSRF 上の不変条件**: 両 upstream は同一 `server_name`・同一 TLS 終端＝**同一オリジン**なので、`__Host-yuuka-session` Cookie・CORS・`Sec-Fetch-Site` 判定は upstream をまたいでも壊れない。ただし後述 11.3 のセッション共有が絶対条件。

### 11.3 共有 Redis 不透明トークン＝署名鍵共有不要

yuuka のセッションは「**CSPRNG の不透明トークンを共有 Redis にハッシュ保存**」する方式（署名 Cookie / JWT ではない）。トークン自体には意味がなくストア参照するだけなので、**共有署名鍵／HMAC シークレットは不要**（[rpt-nginx-session-strangler B-2](verification/rpt-nginx-session-strangler.md)。JWT 方式なら検証鍵共有が必須になる決定的な差異）。両バックエンドは受け取った Cookie 値をハッシュ化して Redis を参照するだけで検証が成立する。

**両バックエンドで完全一致させるべき 4 項目**（一致必須。ズレると「片方が書いたセッションを他方が読めない」障害）:
1. **ハッシュ方式**: `sha256(token)` を同一エンコーディング（hex か base64、大小文字含め）で算出。
2. **Redis キー書式**: `session:{sha256(token)}` を一字一句同一に。**サービス別プレフィックスを付けない**（共有が目的なので同一キースペース）。
3. **値のシリアライズ**: JSON フィールド名・型・日時表現を共通化。Rust(serde)⇄Node で往復可能なスキーマにし、**契約テストで両方向のデシリアライズを検証**。
4. **TTL セマンティクス**: 期限秒数・スライディング更新（アクセス毎の `EXPIRE` リセット）の有無を揃える。片方だけスライディングだと片側で早期失効する。

**`__Host-` Cookie ゆえ同一オリジン必須**（[rpt-nginx-session-strangler B-1](verification/rpt-nginx-session-strangler.md)、RFC 6265bis の規範的 MUST）: `__Host-` プレフィックスは (1)Secure (2)HTTPS オリジン発行 (3)**Domain 属性なし**（host-only） (4)Path=/ の**すべて**を強制し、いずれか欠けるとブラウザが Cookie を丸ごと破棄する。**Domain 禁止＝サブドメイン共有不可**なので、旧 Node と新 Rust は必ず同一プロキシ配下の同一オリジンで動く（これが 11.2 のパス分岐リバースプロキシを必須にする理由）。開発環境が HTTP だと `__Host-` が拒否されるため、ローカルも HTTPS 化するか環境ごとに Cookie 名を切り替える。

**インメモリフォールバックの gotcha**（[rpt-nginx-session-strangler B-3](verification/rpt-nginx-session-strangler.md)）: Redis 断時のインメモリフォールバックは**プロセスローカル**で他バックエンドから不可視。フォールバック中に nginx が旧↔新をパスで分岐すると、同一ユーザーの連続リクエストが別プロセスへ渡り、ローカル Map のセッションが見えず**断続的ログアウト**（再現困難なフラッピング障害）が起きる。→ フォールバック発動を必ずメトリクス／アラート化し、Redis を HA 化してフォールバック依存を最小化する。フォールバック中は片系に固定（sticky）してセッションの見え方の分裂を防ぐ。

### 11.4 SQLite 移行ハザードの核心【最重要・強調】

同一ホスト・別プロセスの Node+Rust 同時アクセスは**公式に安全**（POSIX advisory lock + 共有 `-wal`/`-shm`、ただし **NFS 等ネットワーク FS 不可**）。使用ライブラリ（better-sqlite3 / rusqlite）は同一 SQLite C ライブラリをリンクし同一ロックプロトコルを喋るため、別プロセスで同一 DB を開くこと自体は単一プロセス複数コネクションと同じ扱いになる（[rpt-dual-sqlite-hazard §1](verification/rpt-dual-sqlite-hazard.md)）。

**ただし writer は同時に 1 つが絶対条件**（WAL 公式: "there can only be one writer at a time"）。そして最大の落とし穴は次の一点である:

> **両 writer が DEFERRED トランザクションで読み取り開始 → 両方が書き込みへアップグレード**すると、片方が先に write を握った時点で、もう片方のアップグレードは **busy_timeout を無視して即 `SQLITE_BUSY`** を返す（[rpt-dual-sqlite-hazard §1.4](verification/rpt-dual-sqlite-hazard.md)。SQLite はデッドロックを検知すると busy handler を呼ばず即 BUSY を返す）。**これは busy_timeout では解決不能**であり、`busy_timeout` 値をいくら上げても防げない。better-sqlite3 の `db.transaction()` は既定で `BEGIN DEFERRED` を発行し、yuuka は各 repo で多用しているため、移行期に Rust 側 writer が存在すればこの即-BUSY が現実化する。

**→ 移行期の設計原則（最優先・強く推奨）**: **移行期は「Node が全書き込み・Rust は read-only」を貫き、カットオーバー時に一度だけ writer を Rust へ移譲する＝単一 writer 集約**。これは synapse の read-only 実績と完全に整合する。フェーズ設計として「あるドメインの**書き込み**は Node か Rust の**どちらか一方のみ**」を不変条件にし、nginx でルート群を Rust に回した瞬間そのドメインの書き込みは Rust writer actor へ一本化される（例: `todos` を Rust に回したら Node の todo route は死んでいる＝nginx が Node へ流さない）。横断テーブル（`message_logs` / `tool_outcomes` / `synapses` / `system_settings`）が「ドメイン単位で片側に分割」できない問題も、単一 writer 集約なら発生しない。

**Rust 側 DB 層の必須設定**（[rpt-dual-sqlite-hazard §2](verification/rpt-dual-sqlite-hazard.md) と [00-decisions.md](00-decisions.md) の PRAGMA 決定）:

```rust
// yuuka-db/src/pool.rs
fn open_conn(path: &Path) -> Result<Connection, DbError> {
    let c = Connection::open(path)?;
    c.pragma_update(None, "journal_mode", "WAL")?;   // 既存と一致
    c.pragma_update(None, "foreign_keys", "ON")?;    // 既存と一致
    c.busy_timeout(Duration::from_millis(5000))?;    // Node 既定 5000 と揃える
    c.pragma_update(None, "synchronous", "NORMAL")?; // WAL では NORMAL が定石
    Ok(c)
}
```

- **読み取り**: `deadpool-sqlite` / `r2d2_sqlite` の read pool（N 本、WAL は複数リーダー並行可）→ `spawn_blocking` でマルチコア並列読み。
- **書き込み**: **専用の単一書き込みコネクションを 1 本の `tokio::task`（writer actor）に閉じ込め、`mpsc` でコマンドを直列受信**。これで「並行書き込み→即-BUSY」を構造的に排除しつつ、マルチコアは読み側で活かす（HTTP は読みが支配的）。

**両 writer をやむなく許容する場合の必須条件**（それでも非推奨。以下**全て**を課す。[rpt-dual-sqlite-hazard §2.2](verification/rpt-dual-sqlite-hazard.md)）:
1. **両プロセスに busy_timeout を明示**（推奨 5000ms に統一。片側だけ短いとそちらが先に諦める）。
2. **全書き込みトランザクションを `BEGIN IMMEDIATE` で開始**（DEFERRED 禁止）。これで §11.4 の即-BUSY を、待機可能な通常 BUSY に格下げできる。
3. **アプリ層で `SQLITE_BUSY` リトライ**（`backon` の指数バックオフ + ジッタ）を両側に実装。
4. 書き込みを短く保ち、reader の statement を確実に finalize/reset する。

**長寿命 reader の checkpoint 阻害注意**（[rpt-dual-sqlite-hazard §1.5](verification/rpt-dual-sqlite-hazard.md)）: 妨げるのは「接続の存在」ではなく「**アクティブな read トランザクション／未 reset の prepared statement**」。Rust reader（synapse や新規参照系）が長寿命の statement を握りっぱなしにすると、writer のチェックポイントが進まず WAL が無制限に肥大化（checkpoint starvation）する。Rust 側は各クエリ後に statement を確実に finalize/reset すること。Docker で DB を volume 共有する場合、両コンテナが**同一ノードの同一 bind mount**を見ること必須（ネットワーク volume 不可）。

**schema_version の破壊経路封じ込め**（[rpt-migrations-sqlx-refinery §3](verification/rpt-migrations-sqlx-refinery.md)）:
- **移行期間中、スキーマ移行の権限は Node に一本化**。Rust の `yuuka-db` は起動時に `SELECT value FROM system_settings WHERE key='schema_version'` を読み、**期待固定値 `"17"` と一致しなければ `DbError::Migration` で fail-fast**（回復不能＝起動時の致命に該当）。**Rust は DDL を一切発行しない**＝レガシー DROP 分岐も `mcp_servers` DROP 分岐も**そもそも Rust に移植しない**。これで「Rust が `system_settings` を引き継がず初期化→本番テーブル全 DROP」の事故が構造的に不可能になる。
- Node 全停止後（Phase H 完了時）に、Rust 側へ **refinery による整数連番・前方専用・非破壊のマイグレーションランナー**を導入し、現行 v17 スキーマを `CREATE TABLE IF NOT EXISTS` の冪等 baseline (V1) として凍結。`SCHEMA_VERSION="17"` → `schema_version=17`（整数）へ引き継ぐ 1 回きりの橋渡し migration を書く。以降は Rust が権限を持つ。

### 11.5 カナリア・シャドウ・ロールバック

- **カナリア**（[rpt-nginx-session-strangler C-3](verification/rpt-nginx-session-strangler.md)）: `split_clients` で新 Rust へ 1%→5%→25%→100% と段階配分。各段階で `X-Served-By` ヘッダ・エラー率・p95・**`SQLITE_BUSY` 発生数**を監視（BUSY 急増は writer 競合＝§11.4 違反の兆候）。**REST を先に、WS を最後に**移す。ステートフルな `/ws/chat` は接続の粘着性が要るため**パーセント配分せず、エンドポイント単位で一括切替**する。

  ```nginx
  # カナリア段階配分の例（split_clients。REST ルートのみに適用）
  split_clients "${remote_addr}${http_user_agent}" $tasks_backend {
      5%   yuuka_rust;   # まず 5%
      *    yuuka_node;
  }
  location ^~ /api/tasks/ { proxy_pass http://$tasks_backend; }  # 変数使用時は resolver 注意
  ```

- **shadow/mirror は読み取り専用／冪等のみ**（[rpt-nginx-session-strangler C-3](verification/rpt-nginx-session-strangler.md)）: nginx `ngx_http_mirror_module` で実トラフィックを複製し新 Rust へ送り応答を破棄できるが、**ミラー先が共有 DB／共有 Redis に書くと二重書き込み・セッション汚染・重複通知**を起こす。したがってシャドウは**読み取り専用／冪等なエンドポイント限定**、または Rust 側を dry-run（書き込み無効）／分離ストアにする。§11.3 の共有 Redis はまさに副作用対象なので、書き込み経路のシャドウは行わない。

  ```nginx
  location ^~ /api/tasks/ {
      mirror /shadow_rust;            # 応答は無視（GET 系のみ安全）
      proxy_pass http://yuuka_node;
  }
  location = /shadow_rust { internal; proxy_pass http://yuuka_rust$request_uri; }
  ```

- **レガシー削除は検証後の最終ステップ**（[rpt-nginx-session-strangler C-4](verification/rpt-nginx-session-strangler.md)、Microsoft 公式が明言）: 旧経路（旧 Node の `location`・旧テーブル・旧コード）は**検証完了まで削除しない**。早まると、ロールバックが「オブジェクト復元＋データ再生」になり工数・リスクが激増する。
- **旧経路 warm 維持で即ロールバック**: 旧 Node の該当 `location` を残し、`location` 1 行を `yuuka_node` に戻して `nginx -s reload`（無停止・無ビルド）で即座に旧経路へ戻せる状態を保つ。トラフィックは即ロールバック可だが**データ層のロールバックは非対称**なので、**ロールバック窓の間はスキーマを凍結**する（§11.4 の schema 固定＝Rust は DDL 不発行が効き、Rust が書いた行も Node が読める互換を保証）。Cookie/Redis セッションは新旧共通なので、トラフィックを戻してもユーザーは再ログイン不要。

---

## 第12部. 複数サブエージェント並行実装ワークフローの分解 ＋ オーケストレーション

第11部の実装を「複数サブエージェントのワークフロー」で高速並行実行するための実務ガイド。**本パート末尾の「実地教訓」は、この計画そのものの作成過程で得た一次経験であり、必ず織り込む。**

### 12.1 依存グラフに基づく並行／逐次の切り分け

foundation（core/db/型契約）は**逐次先行して凍結**し、その後 feature クレート（web/discord/gemini/tools/services）を並行投入する。どのクレートが独立かを表に示す:

| 並行性 | クレート／モジュール | 根拠（なぜその区分か） |
|---|---|---|
| **逐次（最初に単独・凍結）** | `yuuka-core`(error/config/secret/UserId), `yuuka-db`(pool/writer actor/UserScope trait), `yuuka-types`(wire DTO + ts-rs) | 全員が依存。ここが動くまで他は着手不可。**凍結後は触らせない** |
| **並行 T1**（互いに独立・別テーブル別 route） | todo / finance / schedule / timeline / reminder / personal / credential / playbook / persona（**9 ドメイン**） | 各ドメインは別テーブル・別 route ファイルで共有は foundation のみ。1 エージェントが「Repo impl + wire DTO + Tool impl + route handler + golden test」を縦に持つ。**最大の並行度** |
| **並行 T2**（横断だが T1 と独立） | auth/session, 静的配信+admin, webhook, MCP proxy | セッション・静的・監査は T1 のドメイン repo に依存しない |
| **逐次（T1 の後・合流点）** | gemini orchestrator, functions registry マージ, MCP dynamic | T1 の Repo/Tool 実装を集約。registry は全 Tool を集める合流点（重複名を `Result` で検知） |
| **逐次（gemini の後）** | discord bot(twilight), WS chat, componentInteraction 統合 | tool registry と gemini エントリに依存 |
| **並行 T3**（bot と独立） | services/cron(reminder/report/briefing/backup/…), notifier | Repo(T1)+notifier に依存するが gemini 内部には依存しない。bot と並行可 |
| **逐次（最後）** | daemon 吸収（crawler/synapse を workspace 化） | 既存 Rust クレートを workspace に統合。別 OS プロセスのまま supervisor 監督（障害分離維持） |

### 12.2 contract-first — トレイト／型を先に確定してから並行投入

並行化の前提は「**モジュール間の境界（trait/型）を Phase 0 で凍結し、以後変更しない**」こと。凍結対象は次の 5 つで、これを確定してからエージェントを並行投入すれば型の衝突が起きない:

1. **`yuuka-core::error` の層別エラー enum**（`ConfigError`/`DbError`/`AuthError`/`GeminiError`/… と最上位 `AppError`）— 全クレートが `#[from]` で依存。`AppError::status()` は網羅 `match`（`_ =>` 禁止）でバリアント追加漏れをコンパイルエラー化。
2. **`UserId` newtype** — 全リポジトリ署名に通し、データ分離キー欠落を型で防ぐ。
3. **`yuuka-types` の wire DTO + `Envelope<T>`** — HTTP とフロントの契約。機密列を DTO のフィールドに**持たせない**ことで漏洩を型的に不可能化。
4. **`ToolProvider` / `Tool` トレイト** — gemini/functions と全ドメインモジュールの契約:

   ```rust
   #[async_trait::async_trait]
   pub trait Tool: Send + Sync {
       fn declaration(&self) -> FunctionDeclaration;
       async fn call(&self, ctx: &ToolContext, args: serde_json::Value)
           -> Result<ToolOutcome, ToolError>;   // 文字列規約を型へ昇格
   }
   ```
5. **Repo トレイト（`UserScope` 束縛）** — user_id スコープを型で強制:

   ```rust
   pub struct UserScope { user_id: UserId }        // 構築時に user_id を必ず束縛
   impl TodoRepo {
       async fn list(&self, scope: &UserScope) -> Result<Vec<Todo>, DbError>;  // user_id 無しクエリを型で禁止
   }
   pub trait CronScan { async fn overdue_across_users(&self) -> Result<Vec<Todo>, DbError>; }  // 横断は別トレイトに隔離
   ```

これで過去に起きたクロステナント事故（`owner_id` 欠落）が型レベルで再発不能になる。

### 12.3 worktree 分離での衝突回避

ファイルを並行変更するエージェントは `git worktree` で分離する（`isolation: worktree`）。**別クレート＝別ファイルなら分離不要**だが、共有ファイル（`Cargo.toml` の members・foundation）を触るなら分離する。ユーザーの auto-memory の落とし穴を厳守する:

- **`worktree-node-modules-symlink-pitfall`**: worktree に node_modules symlink + `git add -A` で本体破壊。Rust クレートは node_modules 不要。`git add -A` は**使わず**常に明示パス `git add crates/todo/`。ts-rs 生成物は `frontend/src/lib/api/generated/` の生成専用ディレクトリで frontend ビルド worktree と分離。
- **`develop-no-history-delete`**: 作業の引越しは move（develop から削除＋force-push）。copy は重複で merge 衝突するため避ける。
- **workspace 化と Cargo.lock**: ルート `Cargo.toml`（現状不在）を `apps/yuuka/Cargo.toml` に Phase 0 で新設し `[workspace] members = ["crates/*", "src/rust_crawler", "src/rust_synapse"]`。**`Cargo.lock` はルート 1 つ**に集約し全 worktree で共有。**`CARGO_TARGET_DIR` は worktree 間で共有しない**（各 worktree 独立の target。衝突ゼロ優先。ビルド高速化は sccache で）。1 クレート 1 worktree 1 エージェントで編集ファイルが物理的に交わらない状態を作る。
- **共有ファイルの編集を Phase 0 に隔離**: members 追加のような共有編集が並行フェーズに漏れると衝突する → Phase 0 で全クレートの空スケルトン（`lib.rs` に最小 stub とトレイト空実装）を先に生成・コミットし、以後エージェントは**自クレート内のファイルのみ**を編集する。

### 12.4 検証ゲート（per-module 完了判定）

各エージェントは自クレートについて以下を**マージ前に全通過**させる（ローカル worktree で完結）:

```
cargo build -p yuuka-todo
cargo clippy -p yuuka-todo --all-targets --all-features -- -D warnings  # unwrap/expect/panic/todo を deny
cargo test  -p yuuka-todo                          # unit + golden(Node 差分) + scope 越境テスト
cargo deny check                                   # anyhow/eyre/color-eyre 混入ブロック（workspace 全体）
git diff --exit-code frontend/.../generated/       # ts-rs drift（DTO 変更時のみ）
```

**クレート単位の `-p` 指定**が並行性の鍵 —— エージェントは他クレートの未完成を待たずに自分のゲートを回せる（foundation は Phase 0 で確定済みなので依存は満たされている）。CI は最後に `cargo test --workspace` で統合を 1 回検証する。

### 12.5 ワークフローのフェーズ設計案

`understand → contract 凍結 → 並行実装 → per-module 検証 → 統合` の 5 段構成。pipeline / parallel / loop-until-dry の使い分けを併記する:

```
Phase 0  契約凍結（単一エージェント・逐次・最重要）           ← pipeline の起点。ここは絶対に並行化しない
  - apps/yuuka/Cargo.toml workspace 新設、Cargo.lock 集約、clippy.toml / deny.toml / workspace lints 配置
  - crates/core（error 層別 enum + Fatality + config + secret + UserId）
  - crates/db（rusqlite pool + writer actor + UserScope trait + assert_schema_compatible）
  - crates/types（wire DTO + Envelope + ts-rs export + drift test）
  - Tool / Repo トレイト凍結、全クレートの空 stub コミット
  ゲート: cargo build/clippy -D/deny check、ts-rs 生成が空でも通る → 以後 型は FROZEN

Phase 1  並行ファンアウト（サブエージェント N 体・parallel）  ← worktree 分離で fan-out
  T1: [todo][finance][schedule][timeline][reminder][personal][credential][playbook][persona]
       各 = Repo impl + DTO + Tool impl + route handler + golden test（別 worktree・別クレート）
  T2: [auth/session][static+admin][webhook][mcp-proxy]（別 worktree）
  T3: 着手可（services/cron の repo 依存が T1 で埋まり次第）
  各エージェントのゲート: 12.4 の -p 単位チェック
  合流: 完了ドメインから nginx location を :7900 へ 1 群ずつ解禁（3 層ゲート → location 追加 → verify → 次へ）

Phase 2  合流（逐次・pipeline）                              ← 全 Tool を集める合流点
  gemini orchestrator + functions registry マージ（重複名を Result で検知）+ MCP dynamic
  ゲート: registry 起動時の重複名テスト、planner responseSchema テスト

Phase 3  bot + WS（逐次・supervisor 前提）
  twilight マルチ接続（per-bot アクター、トークン差替は close→spawn の直列プロトコル）
  WS chat（ターンキュー・1 接続 1Bot 束縛・添付上限）、各 bot タスクを supervisor 配下で catch + 指数バックオフ再起動
  カットオーバー: Node bot 停止 → Rust bot 起動（Discord 側は排他）

Phase 4  services/cron カットオーバー（T3 完成品を投入）
  Node cron 停止 → Rust cron 起動（reminder は起動時即時実行で取りこぼし復帰）

Phase 5  daemon 吸収 + 最終カットオーバー
  crawler/synapse を workspace member 化（既に Rust。別 OS プロセスのまま supervisor 監督で障害分離維持）
  /ws/chat を nginx で :7900 へ最終切替 → Node 全停止
  Rust 側に refinery 整数連番マイグレーションランナー導入、schema_version=17 橋渡し
```

**pipeline / parallel / loop-until-dry の使い分け**:
- **pipeline**（逐次）: Phase 0 → 1 → 2 → 3 → 4 → 5 の骨格。前段の成果物（凍結型・registry）が後段の入力になる依存があるため。
- **parallel**（fan-out）: Phase 1 の T1/T2/T3。境界が foundation の凍結型のみで、別クレート別 worktree なら物理的に交わらない。
- **loop-until-dry**（収束反復）: 各エージェント内の「実装 → 12.4 ゲート → clippy/test/deny 赤を潰す → 再実行」を green になるまで反復。フェーズ跨ぎの統合検証（`cargo test --workspace`）も、パリティ差分が枯れるまで反復する。

### 12.6 今回の実地教訓（本計画作成で得た一次経験・必ず適用）

> 本教訓は抽象論ではなく、**この移行計画そのものを複数エージェントで作成した過程**で実際に踏んだ事象に基づく。証跡: [design-checkpoint.md](design-checkpoint.md) 冒頭は「ワークフロー wf_456f35ec-ec8 の**中断時スナップショット**。完了済み 22 エージェント分の生出力を保存。最終統合(Synthesize)は**未完了**。resume で完了可能」と記録している。

1. **バックグラウンド Workflow はセッション境界（ホストプロセス再起動）で "stopped" になり、最終統合が完走しないことがある。**
   → **journal + resumeFromRunId で再開可能に設計**する。かつ**各エージェント成果は即ファイルへ退避**する（transcript にしか無いと回収困難。実際 22/23 完了で中断し、生出力を design-checkpoint.md に退避していたからこそ resume 可能だった）。

2. **バックグラウンド Agent 呼び出しは完走・通知した（Workflow より堅牢だった）。**
   → 長時間ジョブは **Agent fan-out + ファイル退避**が安全。Workflow の自動オーケストレーションに全面依存せず、Agent 単位で成果を確定させながら進める。

3. **エージェントが最終メッセージに「他エージェント待ち」等のメタ文言を残すと、通知の result にレポート本体が入らない。**
   → 各エージェントに **「最終メッセージ＝成果物のみ、余談・進捗・メタ文言は禁止」** と明示指示し、**かつ Write でファイル出力**させる（result とファイルの二重化で回収漏れを防ぐ。本パートも「final text は Write 完了報告＋見出し一覧のみ」で運用している）。

4. **並行執筆は決定を先に凍結（ADR）してから投入すると齟齬が出ない。**
   → 本計画がその実例。[00-decisions.md](00-decisions.md)（技術選定 ADR）を先に確定し、各執筆エージェントに「再決定禁止・厳密整合」を課したことで、15 領域の並行執筆でも矛盾が出なかった。実装フェーズも 12.2 の contract-first がこれに対応する。

5. **巨大成果は synthesis 1 エージェントに集約させず、パート分割 → 機械的結合が安定。**
   → design-checkpoint.md（約 58 万バイト・22 エージェント出力）を単一 synthesize で束ねようとして中断した。最終マスタープランは**パート分割**（本パートは第11〜12部）して各パートを独立 Write し、後で機械的に結合する方式が安定する。

### 12.7 supervisor 基盤（全 Phase 共通・自己復帰）

長命サービス（bot 各タスク・cron・daemon）は例外なく supervisor 配下に置く（[rpt-resilience-tokio-backon-recloser](verification/rpt-resilience-tokio-backon-recloser.md)・[00-decisions.md](00-decisions.md) の resilience 決定）。panic をタスク境界で隔離し指数バックオフで再起動する。`panic = "unwind"` を厳守（`abort` だと 1 タスク panic がプロセスを落とす）。`Transient`（即バックオフ再起動）と `Permanent`（上限到達で停止 or 管理 UI へアラート）を**別扱い**にし、恒久障害の無限スピンを避ける。外部依存（Gemini/Discord/Google/Redis/DB）の一時障害は型付き `Err(...Recoverable)` を返し呼び出し側が劣化縮退へ落とす。**起動時のシークレット不備のみ** fail-fast。Discord Gateway 再接続は twilight の Shard 内蔵 resume に委ね、汎用 supervisor で丸ごと再 spawn しない（二重管理回避。本当に死んだ場合のみ介入）。

---

## 本パートの結論（最重要 3 点）

1. **nginx location 単位の絞め殺し**（アプリ層フラグ不採用）で切替状態を 1 ファイルに集中させ、ロールバックを `nginx -s reload` 1 発にする。共有 Redis 不透明トークンで**署名鍵共有不要**、`__Host-` Cookie ゆえ**同一オリジン必須**、WS は `Upgrade`/`Connection` 転送 + `proxy_read_timeout 3600s` + ping。
2. **SQLite は「Node が全書き込み・Rust は read-only、カットオーバーで一度だけ writer 移譲」の単一 writer 集約が最優先**。両 writer の DEFERRED→write アップグレードは busy_timeout を無視した即 `SQLITE_BUSY` を招き解決不能。Rust は DDL 不発行（schema_version 固定値チェックのみ）でレガシー全 DROP 破壊経路を封じる。
3. **Phase 0 で error 層別 enum・UserId・wire DTO・Tool/Repo/UserScope トレイトを凍結（contract-first）**してから T1 の 9 ドメインをサブエージェント並行展開する。凍結型 + 1 クレート 1 worktree + `-p` 単位ゲート + `cargo deny` が衝突ゼロ・絶対制約準拠を機械保証する。**実地教訓**（journal+resume 設計、成果の即ファイル退避、最終メッセージ＝成果物のみ、ADR 先行凍結、パート分割→機械的結合）を必ず適用する。

---

> 出典整合: [`00-decisions.md`](00-decisions.md)（特に末尾「未解決の決定事項」）／[`design-checkpoint.md`](design-checkpoint.md) 各領域「敵対的検証」節（A〜D）／[`verification/`](verification/) 各レポート末尾の確信度・落とし穴。
> 本部の全リスクは一次ソース照合済み（crates.io / docs.rs / 公式ドキュメント）。確率・影響は移行文脈での相対評価。

---

## 13. リスク一覧と対策

凡例 — **影響**: 致命/高/中/低（致命=データ喪失・常時稼働破綻）。**確率**: 高/中/低（対策未実施時）。

### 13.1 データ層・並行性（最優先クラスタ）

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-1 | **SQLite 二重 writer による即-BUSY**。Node(better-sqlite3)と Rust(rusqlite)が同一 `yuuka.db` を同時 open する移行期に、両側 DEFERRED Tx が write へアップグレード→ busy_timeout を**無視して即 `SQLITE_BUSY`**（busy_timeout では原理的に解決不能）。`db.transaction()` 多用箇所（personaRepo/userRepo/todoRepo/plannedPaymentRepo）が該当。 | 致命 | 高 | **単一 writer 集約を最優先**: 移行期は「Node が全書き込み・Rust は read-only、カットオーバー時に一度だけ writer を Rust へ移譲」。両writerやむなき場合のみ**全書き込みTxを `BEGIN IMMEDIATE`**（DEFERRED禁止）+ 両側 busy_timeout 明示 + アプリ層 `SQLITE_BUSY` backon リトライを**全て**課す。監視で `SQLITE_BUSY` 発生数を追い、急増=競合の兆候として検知。 | [dual-sqlite-hazard](verification/rpt-dual-sqlite-hazard.md) §1.4/§2（確信度:高）／[00-decisions](00-decisions.md) L103／checkpoint D-1, B-1 |
| R-2 | **アプリ内の並行 writer 競合**。単一プロセス内でも rusqlite 複数コネクションから同時書き込みを投げ busy→backoff を繰り返すと、公平性が無く**ライブロック/レイテンシ悪化**。マルチスレッド化（絶対制約3）が逆効果化。 | 高 | 中 | **単一 writer actor**（全書き込みを1タスク/1コネクションに直列化）+ **read pool**（deadpool-sqlite 0.13 / r2d2_sqlite 0.34）。同期呼び出しは `spawn_blocking`。PRAGMA 明示: `WAL`/`busy_timeout=5000`/`foreign_keys=ON`/`synchronous=NORMAL`。アプリ層 backoff は timeout 超過後の最終手段に限定。 | [db-sqlx-vs-rusqlite](verification/rpt-db-sqlx-vs-rusqlite.md)（split-pool footgun、確信度:高）／[00-decisions](00-decisions.md) L68-69／checkpoint B-1 |
| R-3 | **長寿命 reader が WAL checkpoint を阻害**。Rust 側の未 finalize statement が checkpoint をブロックし `-wal` が肥大化。 | 中 | 中 | Rust 側は statement を確実に `finalize`/`reset`。read pool のコネクション寿命を短く保つ。 | [dual-sqlite-hazard](verification/rpt-dual-sqlite-hazard.md)／[00-decisions](00-decisions.md) L103 |
| R-4 | **refinery baseline の既存DBへの適用ミス**。現行 v17 スキーマを V1 baseline に凍結する際、非冪等な baseline を既存DBに流すと二重生成/エラー。checkpoint は refinery を「動的 `PRAGMA table_info` 内省と相性が悪い」と指摘（→ 00-decisions は refinery 採用・冪等 baseline で解決する立場）。 | 高 | 中 | baseline(V1) を全て **`CREATE TABLE IF NOT EXISTS` の冪等 DDL** で書く（既存DB=無害・新規DB=生成）。**現行の SCHEMA_VERSION 不一致 DROP 再作成（データ喪失）は完全撤廃**。migration 所有権を**単一プロセスに一元化**（移行期の二重実行防止）。破壊的再構築が要る場合は `_new` テーブルへコピー→リネームの保持型のみ。 | [migrations](verification/rpt-migrations-sqlx-refinery.md)／[00-decisions](00-decisions.md) L71／checkpoint L180, L2251-2256 |

### 13.2 依存クレートの破壊的変更・保守リスク

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-5 | **axum/tower-http の 0.x 破壊的変更**。axum は 0.x のためマイナー更新=破壊的変更前提。tower-http 0.7.0（2026-06-15）は新しく axum 0.8.9 が `^0.6.8` を pin、0.7 の axum 0.8 互換は未検証。 | 高 | 中 | **バージョン固定運用**（マイナー=メジャー扱い）。**tower-http は 0.6.x 固定**（型互換確認まで 0.6 系）。hyper は 1.x 安定で問題なし＝実務リスクの主因は axum/tower-http の 0.x semver。更新は互換確認とテスト後にのみ。 | [axum-web-runtime](verification/rpt-axum-web-runtime.md) L37/L84/L153（確信度:高、0.7互換のみ中）／[00-decisions](00-decisions.md) L60 |
| R-6 | **Gemini `generateContent` が将来 legacy 化・非推奨リスク**。2026-06 に Interactions API が GA 化し「正面玄関」へ昇格、generateContent は公式に "legacy" 明記（ただし fully supported 継続）。新 frontier / 長時間エージェント機能は Interactions 側のみに載る。 | 中 | 中 | **薄ラッパで API 層を抽象化**（reqwest+serde+thiserror 自前）。表面積を小さく保ち、将来の Interactions 移行を局所化。**camelCase struct に厳密固定**（Interactions の snake_case/`function_result`/`call_id` を混入させない）。preview モデル名を避け GA 名 `gemini-3.1-flash-lite` 固定。 | [gemini-design-verify](verification/rpt-gemini-design-verify.md) L11/L60/L147（確信度:高）／[00-decisions](00-decisions.md) L79/L81 |
| R-7 | **公式 Rust SDK 不在（Gemini）による自前保守負担**。`google-generative-ai-rs` はアーカイブ済&FC未実装で不適、`gemini-rust 1.7.1` は機能豊富だがエラー方針・API面固定が未検証（予備）。 | 中 | 中 | **自前 reqwest 0.13 薄ラッパを第一推奨**。429/`RetryInfo(retryDelay)` を thiserror variant で完全掌握（現行 rate-limit バックオフを1:1移植）。SDK 依存より表面積が小さく厳格エラー方針を貫ける。`gemini-rust` は採用時のみ実コードでエラー型・API面を要確認。 | [gemini-rest-crates](verification/rpt-gemini-rest-crates.md)／[gemini-design-verify](verification/rpt-gemini-design-verify.md) L120-121（確信度:中〜高）／[00-decisions](00-decisions.md) L78 |
| R-8 | **backoff クレートの RUSTSEC**。`backoff` は RUSTSEC-2025-0012 で非メンテ宣言＝採用禁止（`cargo audit` が警告）。 | 中 | 低 | **backon 1.6.0 採用**（`ExponentialBuilder::default().with_jitter()` を**明示**、ジッタ必須）。`cargo deny check advisories` を CI に組み込み。 | [resilience](verification/rpt-resilience-tokio-backon-recloser.md) L59-63（確信度:高）／[00-decisions](00-decisions.md) L55 |
| R-9 | **サーキットブレーカ生態系の薄さ**。recloser 1.4.0 は tokio 明示保証がなく、他クレートは未成熟（failsafe 2年停滞、circuit_breaker 168行）。 | 中 | 中 | recloser 採用時は tokio 上で軽く PoC 検証。要件がシンプル（失敗率閾値+openタイマ）なら **`AtomicU*` の自前状態機械も対等な選択肢**（コア数十〜百数十行）。自前ブレーカ + backon の組合せが堅い。 | [resilience](verification/rpt-resilience-tokio-backon-recloser.md) L83-87（確信度:中〜高）／[00-decisions](00-decisions.md) L56 |

### 13.3 プラグイン・拡張性・型連携

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-10 | **非信頼プラグインのサンドボックス境界崩壊**。動的 `.so`（libloading/abi_stable）はサンドボックス無し＝ホスト完全侵害、panic-across-FFI が UB で supervisor が隔離しきれない。 | 致命 | 低 | **Extism 1.30.0（wasmtime 上）で非信頼プラグインを実行**。`Manifest` の `allowed_hosts`/`allowed_paths`/memory/timeout で **deny-by-default** 能力付与。wasmtime の trap を `Result`（`PluginError::Trap`）へ変換し制約1と親和。**動的 `.so` は非信頼コードに不採用**を明記。 | [plugins-wasm-extism](verification/rpt-plugins-wasm-extism.md) L35/L61/L118（確信度:高）／[00-decisions](00-decisions.md) L91-92／checkpoint D-3 |
| R-11 | **cargo-component 停滞リスク**。cargo-component 0.21.1（2025-03、~15ヶ月更新なし）。Extism の PDK crate cadence も slowish、Manifest フィールド名は SDK 版で要確認（中確信度）。 | 中 | 低 | Extism は独自 ABI で cargo-component 非依存。標準志向なら 生 wasmtime 46 + Component Model/WASI 0.2（`wasm32-wasip2` + wit-bindgen）が代替。Manifest フィールド名は docs.rs `Manifest` で採用時確認。 | [plugins-wasm-extism](verification/rpt-plugins-wasm-extism.md) L48/L71（確信度:高、Manifest詳細のみ中） |
| R-12 | **型生成ドリフト**。Rust DTO と commit 済み TS（現行 `types.ts` 655行の二重管理）が乖離し、フロント⇄バックの型不整合。 | 高 | 中 | **CI git diff ゲート**: 生成（`cargo run --bin gen-types` / xtask）→`git diff --exit-code`。Rust DTO と commit 済み TS の乖離をブロックし現行「コメント頼み」を機械保証へ。build.rs でなく **xtask パターン**（副作用で drift 検査と競合しない）。 | [typegen-tsrs-utoipa](verification/rpt-typegen-tsrs-utoipa.md)／[00-decisions](00-decisions.md) L98／checkpoint L1032, L3489 |
| R-13 | **成功パス機密漏洩**。エラーの `public_message` allowlist は成功レスポンスの機密列 strip を代替しない。現行 zod allowlist（`discord_token_encrypted` 等を落とす）を型で置換しないと漏洩。 | 高 | 中 | **専用 DTO struct で構造的保証**: 機密列を DTO の**フィールドに存在させない**→生成 TS にも現れず漏洩は型的に不可能。DB row 構造体を直接 `Json` で返すことを clippy `disallowed-types` またはモジュール境界で禁止。in-memory 機密は `secrecy`。 | [00-decisions](00-decisions.md) L97／checkpoint B-2, L2055 |

### 13.4 常時稼働・自己復帰（絶対制約2）

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-14 | **移行中セッション/インメモリフォールバックの他系不可視**。Redis ダウン→インメモリセッションはプロセスローカルで、移行期は Node/Rust 間で他系に不可視＝**断続ログアウト**。 | 中 | 中 | フォールバック発動を**メトリクス/アラート化**（Redis 断は SPOF、即検知）。共有 Redis 復旧を優先経路とし、インメモリ縮退は短時間限定。移行期はセッション検証を「トークンの sha256 ハッシュ方式・Redis キー書式 `session:{sha256(token)}`・シリアライズ・TTL の完全一致」で両側成立させる（署名鍵共有は不要）。 | [nginx-session-strangler](verification/rpt-nginx-session-strangler.md) L119/L249（確信度:高）／[00-decisions](00-decisions.md) L57/L102 |
| R-15 | **twilight caller駆動 poll loop の supervise ミス**。twilight `Shard` は再接続/resume を内蔵するが caller 駆動 poll loop のため、supervisor で正しく回さないと**再接続しない**。N個の動的増減ボットで1ボット障害の隔離（他ボットは生存）が型に無い。 | 高 | 中 | supervisor を「名前付き単一タスク」から**per-bot supervisor tree（動的子タスク集合）**へ拡張。`DiscordError` に bot_id を載せ、1ボットの gateway 切断は**そのボットのみ**テナント別バックオフ再接続。各 Bot=1アクター（1タスク+コマンド mpsc）で二重 Client・トークン差替レースを型で消す。 | [discord-serenity-twilight](verification/rpt-discord-serenity-twilight.md)（確信度:高）／[00-decisions](00-decisions.md) L74／checkpoint B-4, L3728 |
| R-16 | **supervisor が `Permanent` を無限リトライ**。恒久障害（設定ミス等）をタスクが返し続けると 500ms→30s で永久リトライ＝**自己復帰ではなく自己ループ**。CPU スピン・ログ汚染。 | 高 | 中 | `Retryability` を supervisor 動作へ**別扱い**: `Transient`=即バックオフ再起動、`Permanent`=バックオフ上限到達で停止 or degraded 状態へ落とし管理UI通知。N回連続 Permanent で当該サービスを circuit open。 | checkpoint A-3（確信度:高）／[resilience](verification/rpt-resilience-tokio-backon-recloser.md) L37-39 |
| R-17 | **稼働中 Fatal でプロセス即死**。`CryptoError::Fatal` を起動時と稼働時（リクエスト毎の復号）で共用すると、**1ユーザーの1トークン復号失敗でプロセス全体が落ちる**＝制約2の真逆。`panic=abort` にすると全 panic 隔離が無効化。 | 致命 | 低 | `CryptoError` を**起動時鍵検証用**（`Fatal` 可）と**稼働時復号用**（常に `Permanent`＝当該ボット停止/管理UI通知）に**型で分割**。`panic="unwind"` を厳守（`abort` 禁止）。致命的=起動時 config/secret 不備のみ。 | checkpoint A-4（確信度:中）／[resilience](verification/rpt-resilience-tokio-backon-recloser.md) L23-28／[00-decisions](00-decisions.md) L52/L57 |

### 13.5 開発プロセス・ツールチェーン

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-18 | **厳格 lint/clippy deny がテスト・doctest・数値カーネルで誤発火**。`unwrap_used`/`expect_used`/`panic`/`indexing_slicing` deny がテストコードで誤発火。`indexing_slicing` deny は synapse 数値カーネル（`vec[i]`/`slice[a..b]` 多用）と衝突し大量 `#[allow]` を生む（allow 上限と正面衝突・制約3の高速性にも反する）。 | 中 | 高 | `clippy.toml` で `allow-unwrap-in-tests=true`/`allow-expect-in-tests=true`/`allow-indexing-slicing-in-tests=true`。**restriction グループ一括禁止は厳禁**（`blanket_clippy_restriction_lints` 警告）＝8個を**個別に** deny。`indexing_slicing` は**層別ポリシー**: 信頼境界クレート（HTTP/認可/DB）のみ deny、数値カーネル（synapse/embedder）はクレート単位で `allow`。allow カウントはクレート別上限 or 新規追加禁止（ベースライン比較）。 | [clippy-cargodeny](verification/rpt-clippy-cargodeny.md) L29/L34/L64（確信度:高）／checkpoint A-2 |
| R-19 | **cargo-deny の古い `name=` 記法混入**。古い記事の `{ name="anyhow", version="..." }` 形式は非推奨、コピペで deprecation 警告/将来エラー。 | 低 | 中 | `deny.toml` の `[bans].deny` は**現行 `crate=` フィールド**を使用（`crate="anyhow"` / `crate="anyhow@<1"`）。`cargo deny check bans` はネットワーク不要＝高速ゲート、`advisories` はネットワーク要。CI: `cargo clippy --all-targets --all-features -- -D warnings` + `cargo deny check`。 | [clippy-cargodeny](verification/rpt-clippy-cargodeny.md) L94/L118/L121（確信度:高）／[00-decisions](00-decisions.md) L49 |
| R-20 | **thiserror 1.x→2.0 補間非互換**。2.0 で `#[error("{x}")]` のフィールド補間解決順序が厳格化、1.x 前提コードで補間対象がずれる。 | 低 | 低 | 新規は **2.0.18 固定**で問題なし。`#[from]` は真の層境界のみ（同型複数バリアント衝突を回避）、層跨ぎは明示 `match`。`#[error(transparent)]` は最下段限定。 | [errors-thiserror](verification/rpt-errors-thiserror.md) L20-23（確信度:高）／[00-decisions](00-decisions.md) L45-46 |
| R-21 | **`let _ = ...(` grep ゲートの形骸化**。誤検知（`tx.send`/guard 束縛/`writeln!`）だらけで CI 常時赤→開発者が `// intentional-ignore:` を機械貼付、本物の黙殺（`.ok()`/`.unwrap_or_default()`）は捕捉できない。 | 中 | 中 | grep をやめ clippy の `let_underscore_must_use`/`let_underscore_future`/`let_underscore_untyped` を deny 昇格し `#[must_use]`（`Result` は自動）破棄をコンパイラ検出。捨てて良い箇所は名前付きヘルパー（`ignore_cache_miss`）へ一本化。正確を期すなら dylint カスタム lint。 | checkpoint A-1（確信度:高） |

### 13.6 移行戦略・組織

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-22 | **移行の長期化・ファサード恒久化（strangler アンチパターン）**。過渡的アーキテクチャが恒久化し、ロールバック無し/ビッグバン/共有 DB 密結合/可観測性欠如/プロキシ SPOF に陥る。 | 高 | 中 | **完了基準を per-slice で明文化**（機能パリティ=契約テスト合格・カナリアで所定期間 SLO 内・セッション/データ整合性検証済み→done）。**レガシー削除は検証後の最終ステップ**、旧経路を warm 維持し nginx 1行で即ロールバック。ロールバック窓ではスキーマ凍結。カナリアは REST 先・WS 最後。shadow/mirror は**読み取り専用/冪等のみ**（書込バックエンドへ mirror=二重書込）。 | [nginx-session-strangler](verification/rpt-nginx-session-strangler.md) L265/L323/L384/L427（確信度:高）／[00-decisions](00-decisions.md) L104 |
| R-23 | **`proxy_pass` 末尾スラッシュ罠**。nginx `proxy_pass` の末尾 URI 有無で書換挙動が変わり、新旧でパス prefix がずれる。 | 中 | 中 | **per-location `proxy_pass`（末尾 URI 無しでパス保存）**、スラッシュ規約を新旧で一致。WebSocket は `map $http_upgrade $connection_upgrade` + `Upgrade`/`Connection` 転送 + `proxy_read_timeout 3600s` + バックエンド ping。`__Host-` Cookie は Domain 不可＝**同一オリジン必須**（nginx 単一オリジン背後で出し分け）。 | [nginx-session-strangler](verification/rpt-nginx-session-strangler.md) L114/L184（確信度:高）／[00-decisions](00-decisions.md) L102 |
| R-24 | **Rust 学習コスト/借用チェッカーによる速度低下**。特に `ToolContext.embeds/files` への `&mut` push が非信頼 WASM に渡せず・並行ツール実行で借用衝突。 | 中 | 中 | 副作用を**戻り値へ寄せる**（`ToolOutcome.attachments`）＝`ctx` は `&ToolContext`（不変借用）で済み並行実行でも衝突しない。段階移行（strangler）で1スライスずつ習熟。既存 rust_synapse/crawler の知見を流用。 | checkpoint L776/L4058/L4378／[00-decisions](00-decisions.md) 絶対制約4 |

---

## 14. オープンな決定事項（ユーザー判断が要る点）

> [`00-decisions.md`](00-decisions.md) 末尾5項目を判断材料（トレードオフ）とともに展開。各項目に**既定（推奨）／対抗案／判断の分かれ目**を明示。

### 14.1 フロント型生成 — ts-rs vs utoipa+openapi-typescript

| 観点 | 既定（推奨）= **ts-rs 12.0.1** | 対抗案 = **utoipa 5.5.0 → OpenAPI 3.1 → openapi-typescript 7.13.0（+openapi-fetch）** |
|---|---|---|
| 生成範囲 | 型のみ（`ApiResponse<T>` エンベロープを型化） | エンドポイント契約＋**型付きクライアント**まで |
| 工数 | 低（現行 `types.ts` 655行の二重管理を直接解消） | 増（OpenAPI アノテーション+2段パイプライン） |
| 保守 | 活発（repo 非アーカイブ、2026-06-17 コミット、serde-compat 安定） | OpenAPI 標準に載る＝将来的な相互運用性 |
| 却下 | — | **specta は不適**（v2 が RC 継続・Tauri 志向・rspc 終了） |

- **判断の分かれ目**: エンドポイント契約と型付きクライアント（`openapi-fetch`）まで欲しいか。型の単一真実源だけなら ts-rs で十分・低リスク。REST 契約テストやクライアント自動生成の投資対効果を取るなら utoipa（上位互換）。
- **推奨既定値**: **ts-rs**。契約まで欲しくなった時点で utoipa へ拡張可能（後戻り不可ではない）。
- 参照: [typegen-tsrs-utoipa](verification/rpt-typegen-tsrs-utoipa.md)（確信度:高）／[00-decisions](00-decisions.md) L96, L109

### 14.2 DB ドライバ — rusqlite vs sqlx

| 観点 | 既定（推奨）= **rusqlite 0.40.1（bundled SQLite 3.53.2）** | 対抗案 = **sqlx 0.9（sqlx-sqlite）** |
|---|---|---|
| synapse 統一 | ○ 既存 `rust_synapse` が rusqlite 使用＝統一 | × 二重ドライバ化 |
| 書込予測性 | ○ 単一 writer thread を自然に構造化・明示的 | △ default プール（5〜50接続）は WAL 書込で**アンチパターン**（~20倍性能差、要 split-pool: read pool + `max_connections(1)` write pool） |
| コンパイル時検査 | × 実行時のみ | ○ compile-time-checked queries（動的 `ALTER`/`PRAGMA table_info` 内省とは相性悪） |
| async | 要 `spawn_blocking` | ネイティブ async |

- **判断の分かれ目**: コンパイル時クエリ検査の価値 vs 書込並行の footgun と synapse 二重化。sqlx を選ぶ場合は **split-pool パターン**（read pool + 単一接続 write pool）+ `WAL`/明示 busy_timeout/`synchronous=NORMAL` を**必須**採用しないと `SQLITE_BUSY`/ロック飢餓（sqlx-SQLite の最頻報告バグ）。
- **推奨既定値**: **rusqlite**。synapse 統一・書込経路の予測可能性・SQLite 機能の深い制御を優先。sqlx 選択でも single-writer 規律は同様に必須。
- 参照: [db-sqlx-vs-rusqlite](verification/rpt-db-sqlx-vs-rusqlite.md)（split-pool footgun L60/L70/L75、確信度:高）／[00-decisions](00-decisions.md) L67-69, L110

### 14.3 Gemini API 面 — classic generateContent（1:1移植）vs Interactions API 先行

| 観点 | 既定（推奨）= **classic `generateContent` v1beta で開始** | 対抗案 = **最初から Interactions API を狙う** |
|---|---|---|
| 移植コスト | ○ 現行 TS コード（camelCase・`src/gemini.ts:579` の ANY モード等）を 1:1 移植 | × Rust 生態系ほぼ未対応＝薄ラッパ自前必須・実装未知数 |
| サポート状況 | "legacy" だが **fully supported 継続**、メインライン model は当面投入継続 | GA「正面玄関」・新 frontier / 長時間エージェント機能はこちらのみ |
| リスク | 将来の legacy 縮退（記録済リスク R-6） | 早期採用リスク・移行資料が両 API 混在で誤りやすい |

- **判断の分かれ目**: 移植の確実性（既存挙動 1:1）を取るか、将来の frontier 機能アクセスを先取りするか。両 API は JSON フィールド名が別物（generateContent=`inlineData`/`functionResponse` camelCase、Interactions=`function_result`/`call_id`/`previous_interaction_id` snake系）で混在が事故源。
- **推奨既定値**: **generateContent で開始し、Interactions 移行を将来課題**。薄ラッパで API 層を抽象化し移行を局所化（R-6）。camelCase struct に厳密固定。
- 参照: [gemini-design-verify](verification/rpt-gemini-design-verify.md) L11/L15/L60/L147（確信度:高）／[00-decisions](00-decisions.md) L79, L111

### 14.4 edition — 2021 vs 2024

| 観点 | 既定（推奨）= **edition 2021** | 対抗案 = **edition 2024** |
|---|---|---|
| MSRV | 低め（幅広いツールチェーンで動作） | 2024 は新しめの rustc 必須（MSRV 引き上げ） |
| 安定性 | 枯れた挙動・移行資料豊富 | 新機能（RPIT lifetime capture 変更・`unsafe` 属性・`gen` 予約等）の破壊的差分に注意 |
| 現行整合 | checkpoint の Cargo.toml 例が `edition = "2021"` | — |

- **判断の分かれ目**: rustc 1.96.1 は 2024 対応済みで MSRV 制約は実質軽微だが、2024 は semantics 変更（クロージャキャプチャ・`static mut` 参照・match ergonomics 等）を含む。安定運用重視なら 2021、新エディション機能を積極活用するなら 2024。
- **推奨既定値**: **edition 2021**（枯れた挙動・現行 checkpoint 例と整合）。全 workspace メンバーで統一。新機能が必要になれば `cargo fix --edition` で移行可能。
- 参照: [00-decisions](00-decisions.md) L112／checkpoint L3242（`edition = "2021"` 例）／rustc 1.96.1（[00-decisions](00-decisions.md) L19）

### 14.5 プラグイン初期スコープ — Native+MCP 先行 vs 初期から3系統

| 観点 | 既定（推奨）= **Native+MCP を先行、WASM(Extism) は後続フェーズ** | 対抗案 = **初期から3系統（Native/MCP/WASM）揃える** |
|---|---|---|
| 初期工数 | 低（`ToolProvider` trait + Native/McpProvider のみ） | 高（Extism/wasmtime サンドボックス・Manifest 能力設計を初期に） |
| 現行パリティ | Native=現行 `src/functions/*`、MCP=現行 `mcpDynamic`/`mcpClient` を吸収＝**現行機能を即カバー** | 同上＋非信頼ユーザープラグイン |
| 非信頼拡張 | 後続まで不可（絶対制約4の完全達成は遅延） | 初期から可（ただし cargo-component 停滞・Manifest 詳細要確認 R-11） |

- **判断の分かれ目**: 絶対制約4（ユーザー製カスタムモジュール拡張）を**初期設計に織り込む**ことは必須だが、`ToolProvider` trait レジストリを最初に確立すれば WasmProvider は後から差し込める（trait 境界が拡張点）。非信頼プラグインの需要タイミングが分かれ目。
- **推奨既定値**: **Native+MCP 先行**、ただし `ToolProvider` trait とツール名 namespace（Gemini 128字/`[a-zA-Z0-9_:.-]` 準拠）を初期に確立し WASM を差し込み可能に。非信頼実行は Extism（deny-by-default Manifest）で後続フェーズ。
- 参照: [plugins-wasm-extism](verification/rpt-plugins-wasm-extism.md)／[mcp-rmcp](verification/rpt-mcp-rmcp.md)（確信度:高）／[00-decisions](00-decisions.md) L84-92, L113

---

### 決定事項サマリ（推奨既定値の一覧）

| # | 決定事項 | 推奨既定値 | 主リスク |
|---|---|---|---|
| 1 | フロント型生成 | **ts-rs 12.0.1**（契約要件出現時に utoipa へ） | R-12 型ドリフト |
| 2 | DB ドライバ | **rusqlite 0.40.1**（single-writer 規律必須） | R-1/R-2 SQLite BUSY |
| 3 | Gemini API 面 | **classic generateContent で開始**（薄ラッパ抽象化） | R-6 legacy 化 |
| 4 | edition | **2021**（枯れた挙動・現行整合） | MSRV/semantics 差分 |
| 5 | プラグイン初期スコープ | **Native+MCP 先行**（trait で WASM 差込可） | R-10/R-11 サンドボックス |

---

