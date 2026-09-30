# Yuuka プロジェクト AI オンボーディングガイド

> このファイル（`docs/project_overview.md`）は **AI コーディングエージェント（Claude Code 等）がリポジトリ全体を素早く正確に把握する**ための地図です。
> 仕様の根拠・詳細は [docs/](../docs/) の各文書に委ねます（重複させず、ポインタを張ります）。
> 人間向けの導入・セットアップは [README.md](../README.md) を参照。

> **更新状況**: 本書は現行構成（バックエンド = Rust workspace `crates/`、管理画面 = Svelte `frontend/`、PWA = Vue `client/pwa/`）に合わせて更新済みです。旧 Node.js/TypeScript 実装（`src/*.ts`）は [#68](https://github.com/kawaii-music-moe/yuuka/pull/68) で撤去されました。旧実装の設計・不変条件は [architecture/architecture_v2.md](architecture/architecture_v2.md)（**履歴資料**・現行の規範ではない）にのみ残っています。
> Rust 版には、旧実装から未移植・縮退中の箇所が残っています（例: 検索スキルの注入、ユーザー×Bot 別の有効モジュールによるツール絞り込み）。現況は **コードを正**とし、経緯は [rust-rewrite/remaining-work.md](rust-rewrite/remaining-work.md) を参照してください。
> 本書の詳細（§5 のクレート表など）も実装の変更で古くなり得ます。ズレを見つけたら本書を更新してください。

---

## 1. これは何か（30秒サマリ）

**Yuuka** は Google **Gemini API** を使った **Discord 秘書ボット** と **Web 管理ダッシュボード** を 1 つの Rust バイナリ（`yuuka`）で統合運用するソフトウェアです。

- 単一の Rust プロセス（`crates/yuuka-supervisor`。`Supervisor` が JoinSet で監督し、panic 隔離＋指数バックオフで再起動）が「Discord Bot ランタイム（twilight）」「LLM 対話エンジン」「HTTP 管理サーバ（axum）」「多数のバックグラウンド cron 常駐ジョブ」を同時に動かす。Node.js は実行時には使わない（Node は `frontend/` / `client/pwa/` のビルドにのみ必要）。
- LLM は **Function Calling** で多数のツール（ToDo・家計・予定・リマインド・ブラウザ操作・パスワードマネージャ・MCP・リッチ表示 等）を呼び出して秘書業務を遂行する。ツールの一覧は各ドメイン crate の `tools()` を [`crates/yuuka-supervisor/src/tool_registry.rs`](../crates/yuuka-supervisor/src/tool_registry.rs) が集約する。機能モジュール（[module_catalog.rs](../crates/yuuka-orchestrator/src/module_catalog.rs) の 14 種）をユーザー×Bot ごとに ON/OFF する設定 API/UI がある（設計は [function_modularization.md](design/function_modularization.md)。**LLM へ渡すツール宣言を有効モジュールで絞り込む処理は現状未移植**で、能力＋経路のみで絞られる。[`crates/yuuka-tools/src/native.rs`](../crates/yuuka-tools/src/native.rs) の注記参照）。
- 全ユーザーデータは **Discord ユーザー ID 単位で完全分離**。これは設計の最重要不変条件（§8 参照）。
- Bot は 2 つの動作モードを持つ: **秘書モード（secretary）**＝個人 DM 中心・ユーザー自身の Gemini 鍵、**汎用モード（MCP アシスタント）**＝ギルド常駐・Bot 専用 Gemini 鍵。
- Discord に加え、**クライアント非依存の汎用チャット API**（WebSocket `/ws/chat` + OAuth デバイスフロー。デスクトップクライアント向け）と、**PWA クライアント向け API**（`/api/client/*`、`crates/yuuka-client-api`）を提供する。
- 記憶は **シナプス認知アーキテクチャ**（`crates/yuuka-synapse` によるインプロセスの L2 連想想起）。

---

## 2. 技術スタック / クイックファクト

| 項目 | 値 |
|---|---|
| 言語 / 実行系 | **Rust**（バックエンド本体・`crates/`、edition 2021）/ **TypeScript + Svelte**（管理画面 SPA・`frontend/`）/ **TypeScript + Vue**（PWA・`client/pwa/`） |
| パッケージ管理 | **cargo**（workspace、`Cargo.toml`）/ **pnpm 9**（管理画面のみ、`pnpm-workspace.yaml` は `.` のみ）/ **npm**（`client/pwa/`。独立した `package-lock.json`・pnpm workspace の外） |
| DB | **SQLite**（`rusqlite`、bundled）。read pool + 単一 writer actor（`crates/yuuka-db`）。スキーマは `refinery` の前方専用マイグレーション（`crates/yuuka-db/migrations/`、現在 V17 baseline〜V22） |
| キャッシュ / セッション | **Redis**（`redis` crate、`crates/yuuka-auth::SessionStore`）。到達不能でも起動継続＝Cookie のみへ縮退 |
| LLM | `crates/yuuka-gemini`（Gemini・`reqwest`/rustls 経由）。秘書=ユーザー鍵 / 汎用=Bot 鍵 |
| Discord | `twilight-gateway` / `twilight-http` / `twilight-model`（`crates/yuuka-discord`） |
| Web サーバ | **axum**（`ws` feature） + `tower-http`（`crates/yuuka-web`・組立は `crates/yuuka-supervisor`） |
| 管理画面 | **Svelte 5 + Vite**（`frontend/`。`base: "/admin/"` で `/admin` 配下に配信。出力 `dist/public`）。UI 制約は [.cursorrules](../.cursorrules) |
| PWA | **Vue 3 + Vite**（`client/pwa/`。`/` に配信。出力 `dist/public/pwa`）。バックエンドの `/api/client/*` を利用 |
| デスクトップクライアント | **Rust / egui**（`clients/desktop/`。workspace の `exclude`・独立 `Cargo.toml`。現状は Phase 2 のスキャフォールド。`/ws/chat` を利用） |
| ブラウザ自動操作 | **`crates/yuuka-browser`**（インプロセス。`find_chrome` で Chromium 実行ファイルを検出し CLI 起動。対話操作は共有 `BrowserManager`） |
| 記憶エンジン | **`crates/yuuka-synapse`**（埋め込み + KNN + 1st Hop 連想のインプロセスライブラリ） |
| 汎用チャット | **WebSocket**（axum `ws`）`/ws/chat`（`crates/yuuka-supervisor/src/ws.rs`。Bearer デスクトップトークン専用）+ OAuth デバイスフロー（`crates/yuuka-auth`） |
| グラフ描画 | `crates/yuuka-chart`（`image` + `ab_glyph` で PNG 生成） |
| 暗号 | `aes-gcm`（システム鍵・AES-256-GCM）+ `scrypt`（システム鍵導出）+ `argon2`（PW マネージャの per-user 鍵導出。`crates/yuuka-crypto`） |
| 認証 | `bcrypt`（cost 12）+ Redis セッション（Cookie）/ SQLite のデスクトップトークン（Bearer）（`crates/yuuka-auth`） |
| スケジューラ | `croner`（cron 式の次回発火計算）。常駐ジョブは `crates/yuuka-services` |
| フロント⇄Rust の型共有 | `ts-rs`。DTO（`crates/yuuka-types` と各ドメイン crate の `dto.rs`）から `cargo run -p xtask -- gen-types` で `frontend/src/lib/api/generated/` へ生成 |
| Lint / Format / 型 | Rust: `cargo fmt --check` + `cargo clippy -D warnings`（`rust-toolchain.toml` で 1.96.1 固定）+ `cargo-deny`（`deny.toml`）。フロント: **Biome**（`pnpm lint`、対象は `frontend/src scripts` のみ）+ `svelte-check`（`pnpm typecheck:front`）。`client/pwa` の型チェックは `npm run check`（`vue-tsc`） |
| テスト | Rust: `cargo test --workspace`（各クレートに `#[cfg(test)]` 併置。`.github/workflows/rust-ci.yml`）。フロント: `pnpm test:front`（vitest。CI 未実行） |
| CI | `.github/workflows/rust-ci.yml`（fmt / clippy / build / test / cargo-deny）、`.github/workflows/ci.yml`（フロントの typecheck・build + biome lint） |

---

## 3. ビルド・実行コマンド

```bash
pnpm install            # 管理画面の依存のみ導入（バックエンドの依存は cargo が取得）
cargo run --bin yuuka   # バックエンド開発起動（config.yaml のある repo ルートで実行）
pnpm dev                # 管理画面の開発（Vite）。VITE_API_TARGET でバックエンドへ proxy
pnpm dev:client:mock    # PWA（client/pwa）+ モック API + 管理画面をまとめて起動（手順は client/pwa/README.md）
cargo test --workspace  # Rust テスト
pnpm build              # 本番ビルド: cargo build --release --bin yuuka + 管理画面の本番ビルド(dist/public)
pnpm build:front        # 管理画面のみ本番ビルド(dist/public)
pnpm build:pwa          # PWA のビルド（client/pwa。出力は dist/public/pwa。依存が無ければ npm ci を実行）
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings   # Rust lint/format
cargo run -p xtask -- gen-types [--check]   # ts-rs 型を frontend/src/lib/api/generated/ へ生成（--check はドリフト検査）
pnpm typecheck:front    # svelte-check
pnpm test:front         # 管理画面のユニットテスト（vitest）
pnpm lint / lint:fix    # Biome lint（--write で自動修正。対象は frontend/src scripts のみ）
pnpm check              # typecheck:front + lint をまとめて実行
```

- Rust ツールチェイン（`rust-toolchain.toml` で 1.96.1 固定）が `cargo run`/`cargo build`/`pnpm build` に必須。
- 起動には **環境変数 `YUUKA_ENCRYPTION_SECRET` が必須**（32 文字以上）。未設定・短すぎる場合は `crates/yuuka-supervisor` の起動シーケンスが `require_encryption_secret` で fail-fast する（§8）。バックエンドは `.env` を自動読込しないため、実環境変数として渡す必要がある（詳細は [docs/guide/setup.md](guide/setup.md)）。
- 設定は `config.yaml`（一般設定・git 管理外・cwd 相対で固定パス）と実環境変数（機密）。読み込み順は **config.yaml のキー → 同名の環境変数 → 既定値**（`crates/yuuka-core/src/config.rs` の `get_setting`）。テンプレは [example.yaml](../example.yaml) / [.env.example](../.env.example)。
- 既定ポートはコード上 `3000`（`crates/yuuka-core/src/config.rs`）だが、`example.yaml`/本デプロイは `config.yaml` で **7854** に上書き。
- バックエンドは既定では存在しない DB ファイルを新規作成しない。新規インスタンスの初回起動のみ環境変数 `YUUKA_INIT_DB=1` を設定すると、無ければ DB を新規作成して migrations を適用する（[#55](https://github.com/kawaii-music-moe/yuuka/issues/55) / [PR #64](https://github.com/kawaii-music-moe/yuuka/pull/64)。詳細は [docs/guide/setup.md](guide/setup.md) 参照）。
- 静的配信: `dist/public`（管理画面 SPA）が在れば `/admin` 配下に、`dist/public/pwa`（PWA）が在れば `/` に配信される（`crates/yuuka-supervisor/src/main.rs` の `DIST_DIR` / `PWA_DIST_DIR`、`crates/yuuka-web/src/static_files.rs`）。Docker イメージは両方をビルドして同梱する（[Dockerfile](../Dockerfile)）。
- Docker 運用は `deploy/instance.sh`（`pnpm run deploy` 等）。手順は [docs/guide/deployment.md](guide/deployment.md) / [deploy/README.md](../deploy/README.md)。

---

## 4. 全体アーキテクチャ

```
  Discord ユーザー
     │
     ▼
  crates/yuuka-discord（twilight。Bot ごとにテナント）
     message_flow: secretary_flow / assistant_flow
     │  TurnProcessor（注入ポート）
     ▼
  crates/yuuka-orchestrator::ChatEngine（会話の中核）  ◀── /ws/chat（デスクトップ）・/api/client/chat/*（PWA）
     ・システムプロンプト組立・会話ログ・ペルソナ・鍵復号
     ・シナプス想起（crates/yuuka-synapse）
     ・function-calling ループ（crates/yuuka-gemini）
     │
     ▼
  crates/yuuka-tools::ToolRegistry（Native ＋ MCP provider）
     │  各ドメイン crate の tools()   ← 集約: yuuka-supervisor::tool_registry
     ▼
  ドメイン crate（todo / finance / schedule / …）──▶ crates/yuuka-db（SQLite: read pool + 単一 writer actor）
  外部連携: yuuka-browser（chromium）/ yuuka-google / yuuka-mcp / yuuka-chart

  別系統（常時稼働）:
   ・axum HTTP（crates/yuuka-web ＋ 各ドメイン crate の routes。yuuka-supervisor::build_app が merge）
       ├ /api/*         管理 API ── 管理画面 SPA（frontend/）は /admin 配下に静的配信
       ├ /api/client/*  PWA 向け API（yuuka-client-api）── PWA（client/pwa）は / に静的配信
       ├ /hook/{token}  受信 Webhook（yuuka-webhook。認可なし・トークン URL）
       └ /ws/chat       汎用チャット WebSocket（yuuka-supervisor/src/ws.rs）
   ・crates/yuuka-services ── cron 常駐ジョブ（リマインド/朝報/日報/家計/バックアップ/ルーチン等）
```

実行の中心は次の 2 つのライフサイクル（§6）と、独立して回るバックグラウンドジョブ群（§5.3）。全体は `crates/yuuka-supervisor/src/main.rs` の起動シーケンス（config 読込 → 暗号シークレット検証 → DB オープン＋migrations → 鍵ローテーション → Redis → 認証/会話エンジン/シナプス → Discord テナント → 静的配信元の解決 → web と cron を supervisor 配下で起動）が組み立てる。

---

## 5. ディレクトリ / クレートマップ

> AI が「どのファイルを触ればよいか」を引くための索引。クレート名は grep 起点として有用。
> 依存の向き（DAG）は各 crate の `lib.rs` 冒頭コメントに書かれている（`core` は誰にも依存せず、`supervisor` が最下流でドメインを束ねる）。

### 5.1 リポジトリ直下

| 場所 | 内容 |
|---|---|
| [Cargo.toml](../Cargo.toml) / `crates/` / [xtask/](../xtask/) | Rust workspace。`members` にクレート一覧。`workspace.lints` で `unwrap`/`expect`/`panic`/`todo`/`unimplemented`/`unreachable`/インデックス直アクセスを deny。`xtask` は ts-rs の型生成 |
| [frontend/](../frontend/) | 管理画面 SPA（§5.4）。`pnpm dev` / `build:front` |
| [client/pwa/](../client/pwa/) | PWA クライアント（Vue 3）。開発手順は [client/pwa/README.md](../client/pwa/README.md) |
| [clients/desktop/](../clients/desktop/) | デスクトップクライアント（Rust / egui）。workspace 外の独立クレート |
| [scripts/](../scripts/) | `dev-client-mock.mjs`（PWA + モック API + 管理画面の同時起動）、`build-pwa.mjs`（PWA ビルド） |
| [Dockerfile](../Dockerfile) / [docker-compose.yml](../docker-compose.yml) / [deploy/](../deploy/) | 本番/開発インスタンス運用（`deploy/instance.sh`、`deploy/prod` / `deploy/dev` の設定テンプレート）。詳細は [docs/guide/deployment.md](guide/deployment.md) |
| [example.yaml](../example.yaml) / [.env.example](../.env.example) | 設定テンプレート（`config.yaml` / 環境変数） |
| [rust-toolchain.toml](../rust-toolchain.toml) / [clippy.toml](../clippy.toml) / [deny.toml](../deny.toml) / [biome.json](../biome.json) | ツールチェイン固定・lint 設定（deny.toml は `anyhow`/`eyre` 等の依存を禁止） |
| [docs/](../docs/) | ドキュメント（[index.md](index.md) が目次） |
| [.cursorrules](../.cursorrules) | **UI デザイン制約**: Material Design 2 ダーク。⚠️ **カードコンポーネント禁止**（フラットリスト + 下線区切り） |

旧 Node 実装のソース（`src/*.ts` 等）は撤去済みです。

### 5.2 `crates/` — Rust workspace（役割別）

各 crate の詳しい役割は `crates/<name>/Cargo.toml` の `description` と `src/lib.rs` 冒頭の doc コメントを参照。

**基盤**

| crate | 役割 |
|---|---|
| `yuuka-core` | 全 crate の土台: 層別エラー（`AppError`/`WebError`/`DbError` 等）・`Config`（`config.yaml`＋環境変数）・`UserId`/`BotId`/`GuildId`・`Tool`/`ToolProvider` 契約・`UserScope`/`CronScan`（データ分離）・secrets・telemetry |
| `yuuka-types` | フロント⇄Rust の wire DTO と `Envelope<T>`、ts-rs による型 export |
| `yuuka-crypto` | 保存時暗号化（システム鍵 scrypt + AES-256-GCM / ユーザー鍵 Argon2id + AES-256-GCM）と鍵ローテーション（`rotate`・`ENCRYPTED_COLUMNS`） |
| `yuuka-db` | SQLite: 単一 writer actor + read pool + `refinery` マイグレーション（`migrations/V17__baseline.sql` 〜 `V22__*.sql`） |

**Web / 認証**

| crate | 役割 |
|---|---|
| `yuuka-web` | axum の共通層: 型付き認可 extractor（`AuthenticatedUser`/`AdminUser`/`OptionalUser`/`BearerUser`）、`ApiError`、CSRF、body 上限、セキュリティヘッダ、静的配信（`/admin` の SPA・`/` の PWA）、`AppState`/`Db` |
| `yuuka-auth` | 認証バックエンド（Cookie=Redis セッション / Bearer=SQLite デスクトップトークン）、ログイン/登録/初期セットアップ、招待コード、パスワードポリシー（ブラックリストは `crates/yuuka-auth/assets/common-passwords-10k.txt` を `include_str!` で埋込）、OAuth デバイスフロー、監査ログ、レート制限 |
| `yuuka-supervisor` | **エントリポイント**（bin: `yuuka`）。`main.rs`（起動シーケンス・`--healthcheck`）、`lib.rs::build_app`（全ルータの merge）、`supervisor.rs`（JoinSet 監督）、`ws.rs`（`/ws/chat`）、`tool_registry.rs`（全ドメインのツール集約）、`tenants.rs`/`discord.rs`（Discord テナント配線）、`services.rs`（cron 配線）、`desktop_dist.rs`（デスクトップクライアント配布 API） |

**会話・LLM**

| crate | 役割 |
|---|---|
| `yuuka-discord` | twilight マルチテナント Bot。Shard poll ループ、`message_flow`（秘書/汎用の 2 経路）、返信送信、注入ポート（`ports.rs`: `TurnProcessor`/`BotDirectory`/`RateLimiter` 等） |
| `yuuka-gemini` | Gemini `generateContent` の薄いクライアント、リトライ、function-calling ループ（`run_function_calling_loop`）、完了是正 |
| `yuuka-orchestrator` | `ChatEngine`（秘書ターン `secretary_turn*`・汎用モード `generic_turn*`）、システムプロンプト組立（`system_prompt.rs`）、Bot 管理/属性/共有/利用申請の Web API、機能モジュールのカタログ（`module_catalog.rs`）、シナプス連携（`synapse_*`） |
| `yuuka-synapse` | シナプス認知エンジン（インプロセス。ハッシュ n-gram 埋め込み + RAM ベクトル索引 + KNN 想起） |
| `yuuka-tools` | `ToolProvider` 中央レジストリ（`ToolRegistry`/`NativeProvider`）。能力（capability）と経路（秘書/汎用）による露出制御 |
| `yuuka-mcp` | MCP サーバー管理 API、ダッシュボードプロキシ（`/api/mcp-servers*`・`/proxy/mcp/:id/mcp`）、MCP ツール provider（[mcp_dashboard_proxy.md](architecture/mcp_dashboard_proxy.md)） |

**ドメイン**（多くは `repo.rs`・`dto.rs`・`routes.rs`〔Web API〕・`tools.rs`〔LLM ツール〕を縦に持つ。参照実装は `yuuka-todo`）

| crate | 役割 |
|---|---|
| `yuuka-todo` / `yuuka-schedule` / `yuuka-timeline` / `yuuka-reminder` | ToDo / 予定 / デイリータイムライン / リマインド（Google カレンダー同期の一部は未移植。`yuuka-schedule` 冒頭コメント参照） |
| `yuuka-finance` | 家計（収支・予算・繰り返し支払い・消込・レシート解析） |
| `yuuka-personal` | 連絡先・コンテキストノート・クリップボード |
| `yuuka-credential` | パスワードマネージャ（暗号列は DTO に出さない。`browserFillCredential` は復号値をブラウザへ直接入力）、Bot ごとの利用許可 |
| `yuuka-playbook` / `yuuka-persona` | マクロ（Playbook）/ ペルソナ |
| `yuuka-briefing` | 朝報・日報・週報の配信設定（ツール + Web API） |
| `yuuka-conversation` / `yuuka-botassistant` / `yuuka-richcontent` / `yuuka-chart` | 会話ログ要約 / 汎用モード（ギルド）のノート系ツール / リッチ返信 Embed / グラフ PNG 生成（`sendChart`） |
| `yuuka-browser` | `searchWeb`/`fetchDynamicPage`/`takePageScreenshot`/対話ブラウザ操作（chromium CLI/CDP・SSRF ガード） |
| `yuuka-google` | Google OAuth・複数アカウント・Calendar/Drive 連携の共有層 |
| `yuuka-webhook` | 受信 Webhook（`POST /hook/{token}`）と管理 API |
| `yuuka-settings` / `yuuka-admin` / `yuuka-integrated` | 個人設定 API（`/api/settings/*`）/ 管理者 API（`/api/admin/*`）/ Bot 統合管理 API（`/api/integrated/*`） |
| `yuuka-client-api` | PWA 向け API（`/api/client/*`）。ToDo・カレンダー・家計・共有ノート・チャット |

### 5.3 常駐ジョブ（`crates/yuuka-services`）

各サービスは `CronService` を実装し、`yuuka-supervisor` が JoinSet 監督下で回す（通知は `Notifier` 経由）。cron は全ユーザーを跨いで走査するため、`CronScan` メソッド（`CrossUserAccess` 証憑必須）に隔離されている。Rust 版の cron は環境変数 `YUUKA_RUST_CRON=1` のとき起動し、Dockerfile がこれを焼き込む。

| サービス | スケジュール / 役割 |
|---|---|
| `reminder` | 毎分。期限リマインド・ToDo 期限・予定リマインドの配信 |
| `briefing` / `report` | 毎分に設定を確認し、朝報（天気 + RSS）・日報/週報を配信 |
| `payment_recurrence` / `todo_recurrence` | 毎日 0:05 / 0:10。繰り返し支払い・ルーチンタスクの次回生成 |
| `birthday` / `clipboard` | 毎日 8:00 誕生日通知 / 毎時 期限切れクリップボード削除 |
| `playbook_schedule` | 毎分。マクロの cron 定期実行 |
| `backup` | 毎時 :15。ユーザー別 SQLite 抽出 → ZIP → 各自の Google Drive |
| `metrics` | 定期的にメトリクスをログ出力 |

### 5.4 フロントエンド（`frontend/`・`client/pwa/`）

| 場所 | 内容 |
|---|---|
| `frontend/src/App.svelte` / `main.ts` | ルートコンポーネント（ルーター初期化・認証ゲート・admin ガード）とエントリ |
| `frontend/src/lib/` | `router.ts`（History ルーター。`/admin` 配下）、`api/`（API クライアントと `generated/` の ts-rs 生成型）、`stores/`（session・activeBot・theme・toast）、`components/ui/` |
| `frontend/src/routes/` | Bot ごとのタブ画面（`BotTasks`/`BotExpenses`/`BotSchedules`/`BotReminders`/`BotMcp`/`BotConfig` 等）とそのサブコンポーネント |
| `frontend/src/overlays/` | ログイン・アカウント・管理者・統合管理・デバイス・規約類のオーバーレイ画面 |
| `frontend/src/mock/`・`frontend/mock.html` | モック API（バックエンド無しの UI 開発用） |
| `client/pwa/src/` | PWA（Vue 3 + vue-router）。`api/`（`gateway.ts` の `AgentGateway` 契約と `httpAdapter.ts`）、`pages/`（Chat/Todo/Calendar/Finance/Notes/Dashboard/Settings）、`components/`、`composables/` |
| `client/pwa/mock/server.ts` | PWA 開発用のモック API（`npm run mock`。`pnpm dev:client:mock` が起動） |

---

## 6. 主要ランタイムフロー

### 6.1 Discord メッセージ → 返信（中核フロー）

1. **受信/振り分け** — [`crates/yuuka-discord/src/message_flow.rs`](../crates/yuuka-discord/src/message_flow.rs) `handle_message`: Bot（`author.bot`）の発言を無視 → 冪等ガード（`MessageDedup`）→ Bot 種別で分岐。秘書は `secretary_flow`（登録ユーザー・共有アクセス・メンション/返信）、汎用は `assistant_flow`（許可ギルド・メンバー制・Bot 専用キー・レート制限の防衛線）。判定は注入ポート（`BotDirectory`/`RateLimiter`）越し。
2. **ターン処理** — `TurnProcessor`（`crates/yuuka-discord/src/ports.rs`）を [`ChatEngine`](../crates/yuuka-orchestrator/src/engine.rs) が実装（`process_secretary` / `process_guild` / `process_bot_dm`）。同じ `ChatEngine` を `/ws/chat`（デスクトップ）と `/api/client/chat`（PWA）も使う。
3. **秘書ターン**（`secretary_turn_impl`）— リッチ返信フラグ → ユーザー発言を永続化 → 直近履歴ロード → `contents` 組立 → システムプロンプト組立（`system_prompt.rs`）→ **ユーザー自身の Gemini キー**を復号（無ければ ⚠️ 応答で実行しない）→ function-calling ループ → アシスタント応答を永続化 → `TurnReply`。**汎用モード**（`generic_turn_impl`）は **Bot 専用キー**と Bot ペルソナを使う。
4. **Function Calling ループ** — [`yuuka_gemini::run_function_calling_loop`](../crates/yuuka-gemini/src/fc_loop.rs): 生成（429/5xx はリトライ）→ functionCall を `ToolRegistry`（`yuuka-tools`）へ dispatch → 結果を contents へ追記 → 再生成。ツール未実行なのに「登録した」等と主張した場合は補正プロンプト（`COMPLETION_CORRECTION_PROMPT`）で 1 回再生成する。
5. **リッチ返信** — ツールが embeds/files（グラフ PNG 等）を `ToolOutcome` に載せる。`rich_reply_enabled=false` のときリッチ系ツールは生成せず失敗を返す。
6. **送信** — 応答を Discord 用に整形・分割して `crates/yuuka-discord/src/reply.rs` が送信する。

> 縮退中の箇所（ターンプランナー・非同期配信・能力ゲートの一部等）は [`engine.rs`](../crates/yuuka-orchestrator/src/engine.rs) 冒頭コメントと [rust-rewrite/remaining-work.md](rust-rewrite/remaining-work.md) を参照。
> 機能要件の詳細: [docs/spec/discordbot_spec.md](spec/discordbot_spec.md) §3.1（対話エンジン）。旧 Node 実装の設計: [architecture_v2.md](architecture/architecture_v2.md) §5（履歴資料）。

### 6.2 HTTP リクエスト → 応答（管理ダッシュボード / PWA / API）

1. ルータは [`yuuka_supervisor::build_app`](../crates/yuuka-supervisor/src/lib.rs) が組み立てる: `framework_routes`（`/api/me`）＋ 認証/管理/設定/webhook/Bot/認証情報/デバイス認証/WS/MCP/統合管理/各ドメインの `routes()` を merge し、PWA（`mount_pwa`・`/`）と管理画面（`mount_static`・`/admin`）を載せ、共通レイヤ（`apply_common_layers`: CSRF・ボディ上限 10MB・セキュリティヘッダ/CSP）を被せる。
2. **認可はハンドラ引数の型で強制**: `AuthenticatedUser`（要セッション）/ `AdminUser`（role 確認）/ `OptionalUser` / `BearerUser`（デスクトップトークン）（`crates/yuuka-web/src/auth.rs`）。認証は `CompositeAuth`（Cookie=Redis セッション / Bearer=SQLite）。`auth: user` のリソースは必ず認証済みユーザーの ID でスコープする（§8）。Cookie は `__Host-yuuka-session`（HTTPS）/ `yuuka-session`（HTTP）、HttpOnly。
3. エラーは `WebError` → `ApiError` が `{success:false,message}` に写像する（`Internal` は詳細を漏らさない）。認可失敗: 未認証 401、非管理者 403。
4. 状態変更（POST/PUT/PATCH/DELETE）× Cookie 認証には same-site を強制（CSRF ミドルウェア）。Bearer は対象外。
5. 未マッチ: `/api/*` は 404 JSON。静的配信は、ハッシュ付き `assets/` は immutable キャッシュ、それ以外は no-cache。拡張子なしのパスは SPA の `index.html`、拡張子ありで未存在は `404.html`（`crates/yuuka-web/src/static_files.rs`）。

---

## 7. データモデルの要点

- **正規の定義元は [`crates/yuuka-db/migrations/`](../crates/yuuka-db/migrations/)**（`V17__baseline.sql` ＋ `V18`〜`V22` の前方専用マイグレーション。`refinery` が適用し `refinery_schema_history` で追跡）。各ドメイン crate の repo はテーブルを再定義しない。テーブル一覧と列の概要は旧実装時点の [architecture_v2.md](architecture/architecture_v2.md) §2 にもあるが、v16 時点で古い（履歴資料）。
- 日時は一貫して **`'YYYY-MM-DD HH:MM:SS'`（ローカル時刻テキスト、`datetime('now','localtime')`）**。
- 暗号化列は `[encrypted, iv, auth_tag]` の 3 つ組。種類により鍵が異なる（§8）。
- `message_logs` / `bot_context_notes` / `bot_members` は **`users` への FK を持たない**（Web 未登録の Discord ユーザーも記録するため）。これは意図的（汎用モードの分離キー仕様）。
- スキーマ進化: `V17__baseline.sql` は旧 Node 実装の最終スキーマを冪等に作成する（既存 DB でも新規 DB でも適用でき、`schema_version='17'` を刻印）。以降の Rust 側の追加: V18 ギルド内チャンネル有効化（`bot_channels`）/ V19 `message_logs` のチャンネル ID / V20 発言禁止チャンネル / V21 PWA クライアント API（`message_logs.source`）/ V22 PWA チャットのリッチ返信永続化（`rich_content`）。

---

## 8. 絶対に守る不変条件（CRITICAL）

> 旧実装の [architecture_v2.md](architecture/architecture_v2.md) §0・[docs/spec/bot_attributes_requirements.md](spec/bot_attributes_requirements.md) が出典の考え方を、現行 Rust 実装での担保箇所とともに記す。新規実装はこれらを破ってはならない。

1. **データ分離**: 全ユーザーデータのクエリは `user_id` を必須とする。Rust では `UserScope`（`crates/yuuka-core/src/scope.rs`）が構築時に `UserId` を束縛し、repo メソッドは `&UserScope` を取ることで「user_id 無しクエリ」を型で不能化する。全ユーザー横断の走査（cron）は `CronScan`/`CrossUserAccess` に隔離する。**例外**: 汎用モードは `bot_id × user_id`（`bot_context_notes`）/ `bot_id × guild_id`（`bot_guild_notes` 等）を正規の分離キーとする。
2. **認証情報を LLM に渡さない**: PW マネージャの復号値は Function の戻り値・ログ・プロンプトに含めない。`yuuka-credential` は暗号列と `user_id` を DTO に存在させず（構造的フェイルクローズ）、`browserFillCredential` は復号値を対話ブラウザの入力欄へ直接入力する（`yuuka-browser` の共有 `BrowserManager` 経由）。Bot ごとの利用許可（`bot_credential_access`）を通らない資格情報は露出しない。監査ログ（`audit_logs`）にもパスワード・キー等の秘密値は書かない。
3. **暗号は 2 層**（`crates/yuuka-crypto`）:
   - システム鍵（`YUUKA_ENCRYPTION_SECRET` から `scrypt` 派生・固定ソルト `SYSTEM_SALT`）+ AES-256-GCM = **API キー・Discord トークン・OAuth・Webhook シークレット・MCP 認証**用。
   - **per-user 鍵**（`Argon2id(secret, user.salt)`）+ AES-256-GCM = **PW マネージャ専用**。`users.salt` と `SYSTEM_SALT` は不変（変更すると既存の暗号化データが復号不能になる）。
   - `YUUKA_ENCRYPTION_SECRET` が未設定/32 文字未満なら起動失敗。`YUUKA_ENCRYPTION_SECRET_NEW` 設定時は起動時に writer actor 上で全暗号化列を再暗号化する（1 件でも失敗すれば全ロールバックして起動中断）。
4. **DB スキーマは前方専用**: 適用済みマイグレーション（`V17__baseline.sql` 等）は**編集しない**（`refinery` のチェックサム不一致になる）。変更は `crates/yuuka-db/migrations/` に次番号の `V<n>__*.sql` を追加する。DDL の所有者は Rust 側で、書き込みは単一 writer actor に集約する（同一 DB への第二 writer 経路を作らない）。
5. **LLM 鍵のスコープ**: 秘書=ユーザー自身の鍵のみ（無ければ実行せずエラー応答、Bot 鍵へフォールバックしない）/ 汎用=Bot 専用鍵（発話者の個人鍵は使わない）。
6. **握り潰し禁止（Rust の厳格エラー方針）**: `anyhow`/`eyre` は `deny.toml` で禁止。`unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`unreachable!`/インデックス直アクセスは workspace lint で deny（テストコードのみ緩和）。エラーは `thiserror` の層別エラー型で返し、各 crate は `[lints] workspace = true` を宣言する。
7. **承認が必要な操作は 2 段階**: `applyTaskPriorities`/`settlePlannedPayment`/`runPlaybook`/`addCredential` 等は提案結果を返し、**LLM がユーザー確認 → 承認後に確定用ツールを再呼び出し**する。自動確定しない。ツールは `Result<ToolOutcome, ToolError>` を返す（`crates/yuuka-core/src/tool.rs`）。
8. **リッチ返信ゲート**: `ToolContext.rich_reply_enabled == false` のとき、リッチ系ツール（`showRichContent`/`sendChart` 等）は embeds/files を生成せず失敗を返す。
9. **コアの契約は慎重に**: `yuuka-core`・`yuuka-types` の契約（`Tool`/`ToolProvider`/`UserScope`/wire DTO 等）は全 crate に波及するため、変更は影響範囲を確認してから行う（Phase 0 で凍結した契約。各ファイル冒頭のコメント参照）。

---

## 9. コーディング規約

- **Rust**: `cargo fmt` + `cargo clippy -D warnings` が通ること（CI が強制）。各 crate は `[lints] workspace = true`。`unwrap`/`expect` を使わず、層別エラー（`yuuka-core::error`）で返す。新規依存は `deny.toml` の許可ライセンスと ban を確認（`anyhow`/`eyre`/`backoff` は禁止。リトライは `backon`）。
- **コメント・ログは日本語**。既存のセクション区切り（`// ─── ... ───`）や絵文字ログ（`🔔🌅📋💳🎂🧹💾✅❌` 等）のスタイルを踏襲する。
- **型の共有**: wire DTO には `ts_rs::TS` を導出し、変更後は `cargo run -p xtask -- gen-types` で `frontend/src/lib/api/generated/` を再生成する（`--check` でドリフト検査）。JSON のキー形式は既存 DTO（同じドメインの `dto.rs`）に合わせる。
- **Function（LLM ツール）**: 既存名は維持（UX 互換）、新規は lowerCamelCase。宣言の `description` は **日本語で具体的に**（LLM が使い分けられるよう）。名前衝突禁止（`NativeProvider::register` が重複を検出してエラーにする）。
- **新規 HTTP ルート**: ドメイン crate の `routes.rs` に axum `Router<AppState>` を定義し、認可は extractor（`AuthenticatedUser`/`AdminUser` 等）で表現する。新しい crate のルータは `yuuka_supervisor::build_app` へ merge する。
- **常駐サービス**: `CronService` を実装し `crates/yuuka-services` の `build_services` に登録する。通知は `Notifier` 経由。多重実行は逐次 `await` のループ（`run_cron`）で構造的に防がれる。
- **フロントエンド**: [.cursorrules](../.cursorrules) の UI 制約（カード禁止）を守る。`pnpm check`（型 + Biome）を通す。lint 対象は `frontend/src scripts` のみ（`client/pwa` は対象外）。
- **秘密の取り扱い**: ログ/エラー文字列に PW・トークンを出さない。機密は `SecretString`（`yuuka-core::secrets`）で保持する。

---

## 10. よくある作業の入口

| やりたいこと | 触る場所 |
|---|---|
| LLM ツールを追加 | 該当ドメイン crate の `tools.rs` に `Tool` 実装を追加し、crate の `tools()` に登録。新しいドメインなら [`tool_registry.rs`](../crates/yuuka-supervisor/src/tool_registry.rs) の `all_domain_tools` に足す。機能モジュールとして ON/OFF させるなら [`module_catalog.rs`](../crates/yuuka-orchestrator/src/module_catalog.rs)（永続化キーの ID は**リネーム禁止**） |
| DB テーブル/列を追加 | `crates/yuuka-db/migrations/` に次番号の `V<n>__<name>.sql` を追加（既存ファイルは編集しない）→ 対応 repo。§8-4 |
| HTTP API を追加 | ドメイン crate の `routes.rs`（認可は extractor）。新 crate なら `yuuka_supervisor::build_app` で merge。DTO を追加したら `cargo run -p xtask -- gen-types` |
| 定期ジョブを追加 | `crates/yuuka-services/src/<name>.rs` に `CronService` を実装 → `build_services` に登録（cron の起動条件は `crates/yuuka-supervisor/src/main.rs` の `YUUKA_RUST_CRON` ゲート） |
| 管理画面 UI を変更 | `frontend/src/routes/`・`overlays/`・`lib/`。⚠️ [.cursorrules](../.cursorrules)（カード禁止）厳守。API 型は `frontend/src/lib/api/generated/` |
| PWA を変更 | `client/pwa/src/`（API 境界は `api/gateway.ts` の `AgentGateway` と `api/httpAdapter.ts`）。手順は [client/pwa/README.md](../client/pwa/README.md) |
| Discord 応答の挙動を変更 | 受信・ゲート: `crates/yuuka-discord/src/message_flow.rs`、返信整形: `reply.rs`/`text.rs`、ターン処理・プロンプト: `crates/yuuka-orchestrator/src/engine.rs`・`system_prompt.rs` |
| 設定項目/環境変数を追加 | `crates/yuuka-core/src/config.rs`（型付き Config）＋ [example.yaml](../example.yaml) / [.env.example](../.env.example) と [docs/guide/setup.md](guide/setup.md) |

---

## 11. 既存ドキュメントの参照順序

矛盾時の優先順位（上が強い）:

1. **現行のコードとマイグレーション**（`crates/`・`frontend/`・`client/pwa/`・`crates/yuuka-db/migrations/`）— 一次情報。
2. [docs/guide/](guide/setup.md)（setup / deployment / features）— 現行の利用・運用手順。
3. [docs/spec/bot_attributes_requirements.md](spec/bot_attributes_requirements.md) — Bot 動作モード拡張（capability、2 層メモリ、汎用モードのスコープ）。
4. [docs/spec/discordbot_spec.md](spec/discordbot_spec.md) — **マスター機能仕様 v0.6.2**（§3 機能、§5 ユーザー/Bot、§6 PW マネージャ、§7 会話履歴、§8 バックアップ、§9 外部連携）。実装への言及は旧 Node 実装時点のもの。
5. [docs/skills/search_skills.md](skills/search_skills.md) — 検索クロール時の LLM 指示（天気=気象庁優先 等）。Rust 版では未配線。
6. [docs/architecture/architecture_v2.md](architecture/architecture_v2.md) — 旧 Node/TS 実装の規範（**履歴資料**。規範としては扱わない）。
7. [README.md](../README.md) — 人間向け概要・セットアップ（非規範）。

> 用語の対応に注意: 仕様の「マクロ」＝実装の「Playbook」（同一機能）。

---

## 12. 落とし穴（抜粋）

- **起動失敗**: `YUUKA_ENCRYPTION_SECRET` 未設定/32 文字未満で即終了。鍵を変えると既存の暗号化データは復号不能（ローテは `_NEW` 経由）。
- **DB が無いと起動失敗**: 新規インスタンスの初回のみ `YUUKA_INIT_DB=1` が必要（§3）。
- **マイグレーションの書き換え**: 適用済みの `V*.sql` を編集すると、適用済み DB で `refinery` のチェックサム不一致になる。必ず新しい番号で追加する。
- **Discord ゲートウェイの起動条件**: Rust 側の gateway（Shard poll ループ）は環境変数 `YUUKA_RUST_DISCORD=1` で有効になる（`crates/yuuka-supervisor/src/main.rs`）。未設定だと Bot は接続せず REST 送信のみ機能する。運用の設定は [docs/guide/deployment.md](guide/deployment.md) を参照。
- **会話の正は SQLite**: Redis はキャッシュ/セッション。`message_logs` は自動削除されない。
- **MCP 実行前確認**: `requires_confirmation` は DB のフラグのみ。実際の確認はエージェント/ツール呼出層の責務。
- **Google refresh_token は自動更新されない**: 失効時はカレンダー連携が静かに無効化される。
- **PWA の配信元**: PWA は `dist/public/pwa`（`pnpm build:pwa` または Docker の pwa-builder ステージの出力）から配信される。ビルドしていないと PWA 配信は無効（`/api/client/*` は動作）。

---

_最終更新: 2026-09-30 / このファイルはリポジトリ解析に基づく AI 向け索引です。実装が動けば、まず該当ファイルの実コードを正とし、本書とズレがあれば本書を更新してください。_
