# Yuuka プロジェクト AI オンボーディングガイド

> このファイル（`docs/project_overview.md`）は **AI コーディングエージェント（Claude Code 等）がリポジトリ全体を素早く正確に把握する**ための地図です。
> 仕様の根拠・詳細は [docs/](../docs/) の各文書に委ねます（重複させず、ポインタを張ります）。
> 人間向けの導入・セットアップは [README.md](../README.md) を参照。

> ⚠️ **Node 実装は撤去済み（`chore/remove-legacy-node-env`）**。バックエンドは `crates/`（Rust workspace・
> `axum` + `twilight` + `rusqlite` 等、詳細は §2/§3）、フロントエンドは `frontend/`（Svelte + Vite）に
> 置き換わっています。**§2・§3 は Rust 版に更新済み**ですが、**§4 以降（全体アーキテクチャ図・ディレクトリ/
> モジュールマップ・コーディング規約・落とし穴等）は撤去前の Node 実装（`src/*.ts`）をそのまま記述して
> おり、未更新です**。`crates/` 配下のクレート一覧は [Cargo.toml](../Cargo.toml) の `[workspace] members`
> を、実装規範は [docs/architecture/architecture_v2.md](architecture/architecture_v2.md)（同じく Node 前提
> のまま未更新）を参照してください。Rust 版に合わせた全面書き換えは
> [docs/rust-rewrite/remaining-work.md](rust-rewrite/remaining-work.md) が「Node 撤去後の後続タスク」として
> 既に指摘している通り、本 PR のスコープ外の別作業です。

---

## 1. これは何か（30秒サマリ）

**Yuuka** は Google **Gemini API** を使った **Discord 秘書ボット** と **Web 管理ダッシュボード** を 1 プロセスで統合運用するソフトウェアです。

- 1 つの Node.js プロセスが「Discord Bot ランタイム」「LLM 対話エンジン」「HTTP 管理サーバ」「多数のバックグラウンド常駐ジョブ」を同時に動かす。
- LLM は **Function Calling** で約 80 種のツール（ToDo・家計・予定・リマインド・ブラウザ操作・パスワードマネージャ・MCP・リッチ表示 等）を呼び出して秘書業務を遂行する。ツールは**機能モジュール単位（約14）でユーザー×Bot ごとに ON/OFF** でき、有効分の宣言のみ LLM へ渡る（[function_modularization.md](../docs/design/function_modularization.md)、architecture §14）。
- 全ユーザーデータは **Discord ユーザー ID 単位で完全分離**。これは設計の最重要不変条件（§8 参照）。
- Bot は 2 つの動作モードを持つ: **秘書モード（secretary）**＝個人 DM 中心・ユーザー自身の Gemini 鍵、**汎用モード（MCP アシスタント）**＝ギルド常駐・Bot 専用 Gemini 鍵。
- Discord に加え、**クライアント非依存の汎用チャット API**（WebSocket `/ws/chat` + OAuth デバイスフロー）をバックエンド実装済み（デスクトップクライアント向け。architecture §15）。
- 記憶は **シナプス認知アーキテクチャ**（Rust 製 `yuuka-synapse` エンジンによる L2 連想想起）を R0/R1 まで実装（architecture §13）。

---

## 2. 技術スタック / クイックファクト

| 項目 | 値 |
|---|---|
| 言語 / 実行系 | **Rust**（バックエンド本体・`crates/`、edition 2021）/ **TypeScript + Svelte**（管理画面 SPA・`frontend/`、Vite ビルド） |
| パッケージ管理 | **cargo**（workspace、`Cargo.toml`）/ **pnpm**（フロントエンドのみ、`pnpm-workspace.yaml`） |
| DB | **SQLite**（`rusqlite`、bundled）。read pool + 単一 writer actor（`crates/yuuka-db`） |
| キャッシュ / セッション | **Redis**（`redis` crate、`crates/yuuka-auth::SessionStore`）。到達不能でも起動継続＝Cookie のみへ縮退 |
| LLM | `crates/yuuka-gemini`（Gemini・`reqwest`/rustls 経由）。秘書=ユーザー鍵 / 汎用=Bot 鍵 |
| Discord | `twilight-gateway` / `twilight-http` / `twilight-model`（`crates/yuuka-discord`） |
| Web サーバ | **axum**（`ws` feature） + `tower-http`（`crates/yuuka-web`・`crates/yuuka-supervisor`） |
| フロントエンド | **Svelte 5 + Vite**（`frontend/`。旧バニラ JS SPA `src/public/` から移行済み） |
| ブラウザ自動操作 | **`crates/yuuka-browser`**（インプロセス。`find_chrome` で Chromium 実行ファイルを検出し CLI 起動。旧 Rust クローラーデーモン + Puppeteer フォールバックは撤去） |
| 記憶エンジン | **`crates/yuuka-synapse`**（埋め込み + KNN + 1st Hop 連想。旧・子プロセス IPC 版から**インプロセスライブラリ**へ統合済み） |
| 汎用チャット | **WebSocket**（axum `ws`）`/ws/chat` + OAuth デバイスフロー（`crates/yuuka-auth`。デスクトップクライアント用バックエンド） |
| グラフ描画 | `crates/yuuka-chart`（`image` + `ab_glyph` で PNG 生成） |
| 暗号 | `aes-gcm`（システム鍵・AES-256-GCM）+ `scrypt`（システム鍵導出）+ `argon2`（PW マネージャの per-user 鍵導出。`crates/yuuka-crypto`） |
| 認証 | `bcrypt`（旧 `bcryptjs` cost 12 と相互運用）+ Redis セッション（`crates/yuuka-auth`） |
| スケジューラ | `croner`（cron 式の次回発火計算。旧 `node-cron`/`cron-parser` 相当） |
| Lint / Format / 型 | Rust: `cargo fmt --check` + `cargo clippy -D warnings`（`rust-toolchain.toml` で 1.96.1 固定）。フロント: **Biome**（`pnpm lint`、対象は `frontend/src scripts` のみ）+ `svelte-check`（`pnpm typecheck:front`） |
| テスト | Rust: `cargo test --workspace`（各クレートに `#[cfg(test)]` 併置。`.github/workflows/rust-ci.yml`） |

---

## 3. ビルド・実行コマンド

```bash
pnpm install            # フロントエンド依存のみ導入（バックエンドの依存は cargo が取得）
cargo run --bin yuuka   # バックエンド開発起動（config.yaml のある repo ルートで実行）
pnpm dev                # フロントエンド開発（Vite）。VITE_API_TARGET でバックエンドへ proxy
cargo test --workspace  # Rust テスト
pnpm build              # 本番ビルド: cargo build --release --bin yuuka + フロントエンド本番ビルド(dist/public)
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings   # Rust lint/format
pnpm typecheck:front    # svelte-check
pnpm lint / lint:fix    # Biome lint（--write で自動修正。対象は frontend/src scripts のみ）
pnpm check              # typecheck:front + lint をまとめて実行
```

- Rust ツールチェイン（`rust-toolchain.toml` で 1.96.1 固定）が `cargo run`/`cargo build`/`pnpm build` に必須。
- 起動には **環境変数 `YUUKA_ENCRYPTION_SECRET` が必須**。未設定だと `crates/yuuka-supervisor` の起動シーケンスが `require_encryption_secret` で fail-fast する（§8）。バックエンドは `.env` を自動読込しないため、実環境変数として渡す必要がある（詳細は [docs/guide/setup.md](guide/setup.md)）。
- 設定は `config.yaml`（一般設定・git 管理外・cwd 相対で固定パス）と実環境変数（機密）。テンプレは [example.yaml](../example.yaml) / [.env.example](../.env.example)。
- 既定ポートはコード上 `3000`（`crates/yuuka-core/src/config.rs`）だが、`example.yaml`/本デプロイは `config.yaml` で **7854** に上書き。
- テストは **`cargo test --workspace`**（Rust 側のみ。フロントエンド `frontend/` には現状テストコマンドは未設定）。
- 既知の制限: バックエンドは存在しない DB ファイルを新規作成しない（[#55](https://github.com/kawaii-music-moe/yuuka/issues/55)）。新規環境では事前に空の SQLite ファイルを用意する（[docs/guide/setup.md](guide/setup.md) 参照）。

---

## 4. 全体アーキテクチャ

```
                         ┌──────────────────────────────────────────┐
   Discord ユーザー ──▶  │  src/bot.ts  (複数Botクライアント管理)      │
                         │   ├ 秘書モード → processMessage            │
                         │   └ 汎用モード → processGuildMessage        │
                         └───────────────┬──────────────────────────┘
                                         ▼
                         ┌──────────────────────────────────────────┐
                         │  src/gemini.ts  (LLM対話エンジン)          │
                         │   ・システムプロンプト組立                  │
                         │   ・Function-Calling ループ(最大10反復)     │
                         │   ・補完ハルシネーション検出/補正           │
                         └──┬───────────────────────┬───────────────┘
                            ▼                       ▼
               src/functions/* (LLMツール)   src/services/llmClient.ts
                  registry.dispatch()           (Gemini鍵の払い出し)
                            │
              ┌─────────────┼───────────────────────────────┐
              ▼             ▼                                ▼
        src/db/* (SQLite)  src/services/* (外部連携)   src/services/browserService.ts
        ユーザー単位分離    Google/MCP/Webhook/通知       → Rust crawler / Puppeteer

   別系統(常時稼働):
     ・src/server.ts (生http) ─ Web管理ダッシュボード(SPA) / Webhook受信
                              └ WebSocket /ws/chat ─ 汎用チャットAPI(デスクトップ等)
                                  → chatChannelService → processMessage (会話コア無改修で再利用)
     ・src/services/synapseEngine.ts ─ Rust yuuka-synapse 子プロセス(記憶/連想想起)
     ・src/services/*Service.ts ─ node-cron 常駐ジョブ(リマインド/朝報/日報/家計/バックアップ/ルーチン等)
```

実行の中心は次の 2 つのライフサイクル（§6）と、独立して回るバックグラウンドジョブ群（§7「services」）。

---

## 5. ディレクトリ / モジュールマップ

> AI が「どのファイルを触ればよいか」を引くための索引。関数名・型名は grep 起点として有用。
> **どの機能がどのファイル群を「所有」するか**の正規表は [docs/architecture/architecture_v2.md](../docs/architecture/architecture_v2.md) §10「ファイル所有マップ」。

### 5.1 ルート（横断・統合層 — 編集は慎重に）

| ファイル | 役割 |
|---|---|
| [src/index.ts](../src/index.ts) | エントリポイント。起動シーケンス（secret 検証 → `runMigrations` → `rotateSecretKey` → 招待コード投入 → Redis → Web サーバ → Bot → 常駐サービス群）と 15 秒 watchdog 付きグレースフルシャットダウン |
| [src/config.ts](../src/config.ts) | `config.yaml` + `.env` を優先順位（**env > config.yaml > 既定値**）でマージし `config` を export |
| [src/gemini.ts](../src/gemini.ts) | **LLM 対話エンジンの中核**。`processMessage`/`processGuildMessage`/`processBotDmMessage`、`buildSystemInstruction`、`runFunctionCallingLoop`、補完ハルシネーション検出 |
| [src/bot.ts](../src/bot.ts) | 複数 Discord Bot のライフサイクル（`startBot`/`startCustomBot`/`restartDefaultBot`）、`setupMessageListener`、添付検出、共有招待 DM |
| [src/server.ts](../src/server.ts) | 生 `node:http` サーバ。静的配信（SPA）・CORS/HTTPS・`dispatchRoute` への振り分け |
| [src/types/contracts.ts](../src/types/contracts.ts) | **共有契約型**: `ToolContext` / `FunctionModule` / `RouteDef` / `SessionUser`、`sendJson`。⚠️ **変更禁止**（統合フェーズのみ） |

### 5.2 `src/functions/` — LLM ツール（Function Calling 宣言＋ハンドラ）

各モジュールは `FunctionModule { declarations, handlers }` を export。モジュールのカタログ（ID・能力・UI メタ・解決関数）は [src/functions/moduleCatalog.ts](../src/functions/moduleCatalog.ts) が一元管理し、能力 + 有効モジュールでフィルタした結果を [src/functions/registry.ts](../src/functions/registry.ts) の `dispatch` が実行する。[src/functions/index.ts](../src/functions/index.ts) は import 互換のための再エクスポートファサード。

| ファイル | 主なツール |
|---|---|
| [todoFunctions.ts](../src/functions/todoFunctions.ts) | `addTodo` / `listTodos` / `completeTodo` / `updateTodo` / `organizeTaskPriorities`→`applyTaskPriorities`（2 段階承認） |
| [scheduleFunctions.ts](../src/functions/scheduleFunctions.ts) | `addSchedule` / `listSchedules` / `deleteSchedule`（Google カレンダー双方向同期） |
| [reminderFunctions.ts](../src/functions/reminderFunctions.ts) | `addReminder` / `listReminders` / `cancelReminder`（cron 繰り返し・過去日自動補正） |
| [financeFunctions.ts](../src/functions/financeFunctions.ts) | 家計の最大モジュール。`addExpense` / 予算 / `addPlannedPayment` / `findSettlementCandidates` / `settlePlannedPayment`（消込・自動再生成） |
| [noteFunctions.ts](../src/functions/noteFunctions.ts) | コンテキストノート `appendContextNote` / `getContextNote` / `setContextNote`（システムプロンプトへ常時注入） |
| [clipboardFunctions.ts](../src/functions/clipboardFunctions.ts) | TTL 付き揮発メモ `addClipboardEntry` / `listClipboardEntries` / `deleteClipboardEntry` |
| [contactFunctions.ts](../src/functions/contactFunctions.ts) | 連絡先 `addContact` / `searchContacts`（言及時のみ動的注入）/ 誕生日リマインド連携 |
| [browserFunctions.ts](../src/functions/browserFunctions.ts) | `fetchDynamicPage` / `takePageScreenshot` / `searchWeb` / `browserInteractive*`（open/click/type/wait/status/close） |
| [credentialFunctions.ts](../src/functions/credentialFunctions.ts) | PW マネージャ `listCredentialServices` / `addCredential` / `browserFillCredential`（**復号値は LLM に返さずブラウザへ直接注入**） |
| [playbookFunctions.ts](../src/functions/playbookFunctions.ts) | マクロ `savePlaybook` / `findPlaybooks` / `runPlaybook` / `getRecentActionHistory` |
| [conversationFunctions.ts](../src/functions/conversationFunctions.ts) | 会話ログ `summarizeConversationTopic`（トピック要約・時系列順・FTS5）。受動的キーワード検索 `searchConversationLogs` はシナプス L2 連想想起へ統合し廃止 |
| [chartFunctions.ts](../src/functions/chartFunctions.ts) | `sendChart`（PNG を `ctx.files` へ push、最大 30 データ点） |
| [briefingFunctions.ts](../src/functions/briefingFunctions.ts) | 朝報・日報・週報の設定 `configureBriefing` / `configureReport` / `runBriefingNow` |
| [botAssistantFunctions.ts](../src/functions/botAssistantFunctions.ts) | 汎用モード専用。ギルドメンバー管理 / 個人ノート / ギルド共有ノート / ギルド内会話要約（`requireGuild` ガード） |
| [mcpDynamic.ts](../src/functions/mcpDynamic.ts) | 登録済み MCP サーバの Tool を動的に `FunctionDeclaration` 化（JSON Schema→Gemini 変換、実行前確認フラグ、呼出時の可用性再チェック） |
| [browserModule.ts](../src/functions/browserModule.ts) | ブラウザ操作アダプタ（カタログ用に browserFunctions を `FunctionModule` 化） |
| [richContentModule.ts](../src/functions/richContentModule.ts) | リッチ返信（`core` capability・`selectable=false`＝常時有効） |
| [moduleCatalog.ts](../src/functions/moduleCatalog.ts) | `MODULE_CATALOG`（約14モジュールの ID/能力/UI メタ）＋ 解決関数（`getFunctionModulesForCapabilities` / `getGuildAssistantFunctionModules` / `listSelectableModules` / `getBaseFunctionModules`）。永続化キー（ID）は**リネーム禁止** |
| [registry.ts](../src/functions/registry.ts) / [index.ts](../src/functions/index.ts) | レジストリ構築・宣言の重複排除・能力別フィルタ・`dispatch` ループ / 再エクスポートファサード |

### 5.3 `src/db/` — リポジトリ層（SQLite, ユーザー単位分離）

スキーマの**唯一の定義元は** [src/db/migrations.ts](../src/db/migrations.ts)（**schema v16** / 各 Repo はテーブルを再定義しない）。各 Repo は `xxxRepo.ts` + 型 `xxxRecord`。

| ファイル | 役割 |
|---|---|
| [migrations.ts](../src/db/migrations.ts) | スキーマ v16 全定義。冪等な `migrate*` 段階移行（v3 bot_id 化 → v4 MCP bot 化 → v5 owner リソース許可/Google複数 → v8 ペルソナ bot 化 → v9 手動停止 → **v10 シナプス記憶層 → v11 時刻文脈 → v12 タスク進捗/サブタスク → v13 デスクトップトークン → v14 ギルド利用申請 → v15 利用可能ロール → v16 ルーチンタスク**）・機能モジュール化（enabled_modules / bot_user_modules）・暗号化列レジストリ（鍵ローテ用）。⚠️ **変更は統合フェーズのみ** |
| [database.ts](../src/db/database.ts) | `better-sqlite3` 初期化、WAL / `foreign_keys=ON`、`getDb()` / `closeDb()` |
| [redis.ts](../src/db/redis.ts) | Redis クライアント・再接続バックオフ。未接続時は `null` を返しフォールバック誘導 |
| [userRepo.ts](../src/db/userRepo.ts) | ユーザー CRUD、bcrypt(cost12)、role(RBAC)、Gemini 鍵/Google OAuth(暗号化)、salt、通知先・各種設定 |
| [messageLogRepo.ts](../src/db/messageLogRepo.ts) | 全会話の永続化（SQLite が正）+ Redis コンテキスト二重書き、**FTS5**、`(user_id, bot_id[, guild_id])` スコープ |
| [todoRepo.ts](../src/db/todoRepo.ts) / [expenseRepo.ts](../src/db/expenseRepo.ts) / [plannedPaymentRepo.ts](../src/db/plannedPaymentRepo.ts) | ToDo / 収支台帳・月次集計・予算 / 繰り返し支払い・消込リンク |
| [reminderRepo.ts](../src/db/reminderRepo.ts) / [scheduleRepo.ts](../src/db/scheduleRepo.ts) | リマインド（cron・複数 source）/ Google カレンダー同期予定 |
| [contactRepo.ts](../src/db/contactRepo.ts) / [contextNoteRepo.ts](../src/db/contextNoteRepo.ts) / [clipboardRepo.ts](../src/db/clipboardRepo.ts) | 連絡先 / コンテキストノート(≤10k) / 揮発クリップボード(TTL) |
| [credentialRepo.ts](../src/db/credentialRepo.ts) / [credentialAccessRepo.ts](../src/db/credentialAccessRepo.ts) | PW マネージャ永続化（Argon2id+AES-256-GCM、一覧は暗号列を SELECT しない）/ Bot ごとの認証情報利用許可（`bot_credential_access`） |
| [botRepo.ts](../src/db/botRepo.ts) | Bot インスタンス CRUD、トークン暗号化、Bot 共有（pending/active/revoked）、`hasBotAccess`、手動停止フラグ `stopped`（v9）、Bot 専用 Gemini 鍵・Bot 単位ペルソナ・Bot 既定の有効モジュール `enabled_modules` |
| [botUserModulesRepo.ts](../src/db/botUserModulesRepo.ts) | **機能モジュール化のユーザー×Bot 上書き層**（`bot_user_modules`）。行があれば採用、無ければ Bot 既定へフォールバック |
| [botAttributesRepo.ts](../src/db/botAttributesRepo.ts) / [botNoteRepo.ts](../src/db/botNoteRepo.ts) / [botMemberRequestRepo.ts](../src/db/botMemberRequestRepo.ts) | Bot 能力プリセット・ギルド許可/メンバー・利用可能ロール（v15）/ Bot スコープのノート（個人・ギルド共有）/ ギルド利用申請（v14） |
| [synapseRepo.ts](../src/db/synapseRepo.ts) / [toolOutcomeRepo.ts](../src/db/toolOutcomeRepo.ts) | シナプス記憶（v10。content/embedding BLOB/鮮度）/ ツール実行実績・トピック別勝率（Node のみ書き手、Rust は read-only） |
| [desktopTokenRepo.ts](../src/db/desktopTokenRepo.ts) | デスクトップクライアントの長命トークン（v13。OAuth デバイスフロー、ハッシュ保存） |
| [googleAccountRepo.ts](../src/db/googleAccountRepo.ts) | Google 複数アカウント連携（`user_google_accounts` 〔owner 単位・primary フラグ〕、Bot ごとのアカウント割り当て `bot_google_account`。v5） |
| [personaRepo.ts](../src/db/personaRepo.ts) | ペルソナ（≤20k、公開フラグ、マーケットプレイス） |
| [webhookRepo.ts](../src/db/webhookRepo.ts) / [mcpRepo.ts](../src/db/mcpRepo.ts) | 受信 Webhook エンドポイント・配信監査 / MCP サーバ（system/user スコープ・tools キャッシュ） |
| [briefingConfigRepo.ts](../src/db/briefingConfigRepo.ts) / [reportConfigRepo.ts](../src/db/reportConfigRepo.ts) | 朝報設定 / 日報・週報設定 |
| [auditRepo.ts](../src/db/auditRepo.ts) | 監査ログ（**パスワード/鍵本体は記録禁止**） |
| [inviteRepo.ts](../src/db/inviteRepo.ts) / [systemSettingsRepo.ts](../src/db/systemSettingsRepo.ts) | 招待コード（1 回限り・失効可）/ key-value（`schema_version` 等） |

### 5.4 `src/services/` — バックグラウンドジョブ・外部連携・基盤

| ファイル | 役割 / トリガ |
|---|---|
| [llmClient.ts](../src/services/llmClient.ts) | Gemini クライアント払い出し: `getUserGenAI`(秘書=ユーザー鍵) / `getBotGenAI`(汎用=Bot 鍵) / `generateAuxText`(補助生成・リトライ) |
| [notifier.ts](../src/services/notifier.ts) | 送信基盤 `sendToUser(userId, payload, target?, botId?)`。クライアント解決＋チャンネル/DM 振り分け |
| [sessionService.ts](../src/services/sessionService.ts) | Redis セッション（SHA256 鍵・7 日スライディング・PW 変更で全失効） |
| [secretService.ts](../src/services/secretService.ts) | PW マネージャ高レベル API（登録/復号＋監査フック） |
| [passwordPolicy.ts](../src/services/passwordPolicy.ts) | パスワードポリシー（8 字以上・2 種以上・1 万件ブラックリスト） |
| [pendingRegistration.ts](../src/services/pendingRegistration.ts) | ユーザー登録の DM チャレンジ（Discord ID 所有確認）。ワンタイムコードを DM 送信し、検証成功時のみ実ユーザーを作成 |
| [browserService.ts](../src/services/browserService.ts) | **ブラウザ自動操作の中核**（1054 行）。Rust デーモン IPC / Puppeteer フォールバック / `data-yuuka-id` 注釈 / 永続セッション。⚠️ §8 不変層 |
| [botCapabilities.ts](../src/services/botCapabilities.ts) / [botRateLimit.ts](../src/services/botRateLimit.ts) | Bot 能力プリセット解決（secretary / mcp_assistant）/ 3 段レート制限 |
| [botModules.ts](../src/services/botModules.ts) | **機能モジュールの有効/無効解決＋キャッシュ**。`resolveEnabledModulesForUser`（ユーザー上書き→Bot 既定→全有効の 3 段）/ `setUserModules` / `invalidate*Cache` |
| [memberRequest.ts](../src/services/memberRequest.ts) | 汎用モードのギルド利用申請（承認制）。`bot_member_requests` |
| [actionRecorder.ts](../src/services/actionRecorder.ts) | マクロ学習用に直近 Function Call 履歴を記録（認証系・記録系は除外） |
| [reminderEngine.ts](../src/services/reminderEngine.ts) | 🔔 毎分。期限・ToDo・予定リマインド配信（全ユーザー横断 = cron 例外） |
| [briefingService.ts](../src/services/briefingService.ts) | 🌅 朝報。Open-Meteo 天気 + RSS を LLM 要約して配信 |
| [reportService.ts](../src/services/reportService.ts) | 📋 日報・週報。ToDo/予定/収支/会話トピックを集約・LLM 要約 |
| [paymentRecurrenceService.ts](../src/services/paymentRecurrenceService.ts) | 💳 毎日 00:05。繰り返し支払いを次回期日へ前進 |
| [playbookScheduleService.ts](../src/services/playbookScheduleService.ts) | マクロの cron 定期実行（user×playbook 単位） |
| [backupService.ts](../src/services/backupService.ts) | 💾 毎時。ユーザー単位 SQLite 抽出→ZIP→各自の Google Drive へ世代管理 |
| [birthdayReminderService.ts](../src/services/birthdayReminderService.ts) / [clipboardCleanupService.ts](../src/services/clipboardCleanupService.ts) | 🎂 毎日 08:00 誕生日通知 / 🧹 毎時 期限切れ削除 |
| [autoTagService.ts](../src/services/autoTagService.ts) | ToDo 作成/更新後に LLM でタグ自動付与（非同期・非ブロッキング） |
| [webhookProcessor.ts](../src/services/webhookProcessor.ts) | 受信 Webhook 処理（HMAC 検証 → LLM 解釈 → 通知 → 任意で ToDo/リマインド化） |
| [receiptParser.ts](../src/services/receiptParser.ts) | レシート画像を Gemini で解析し家計簿登録（Function Calling 経由） |
| [googleCalendarService.ts](../src/services/googleCalendarService.ts) / [googleDriveService.ts](../src/services/googleDriveService.ts) | Google OAuth2・カレンダー双方向同期 / Drive バックアップ |
| [mcpClient.ts](../src/services/mcpClient.ts) | MCP クライアント（JSON-RPC 2.0 over HTTP/SSE: initialize / tools/list / tools/call） |
| [chartService.ts](../src/services/chartService.ts) | chart.js + canvas でダークテーマ PNG 生成 |
| [playbookService.ts](../src/services/playbookService.ts) | マクロ（Playbook）CRUD の基盤 |
| [todoRecurrenceService.ts](../src/services/todoRecurrenceService.ts) | 🔁 ルーチン（繰り返し）タスクの次回生成（v16。repeat_rule/until/count） |
| [synapseEngine.ts](../src/services/synapseEngine.ts) / [synapseExtractor.ts](../src/services/synapseExtractor.ts) / [metrics.ts](../src/services/metrics.ts) | **シナプス記憶**（§架構 §13）: Rust `yuuka-synapse` の子プロセス IPC（health/assemble/index/forget/reindex）/ 会話ターンからのヒューリスティック抽出 / メトリクス。エンジン不在時は直近15件の生履歴注入へデグレード |
| [chatChannelService.ts](../src/services/chatChannelService.ts) / [componentInteractionService.ts](../src/services/componentInteractionService.ts) | **汎用チャット API**（§架構 §15）: WS フレーム ↔ `processMessage` 橋渡し / ボタン等コンポーネント操作の処理 |
| [desktopAuthService.ts](../src/services/desktopAuthService.ts) | デスクトップクライアントの OAuth デバイスフロー認証・トークン検証（`desktop_tokens`） |
| [turnPlanner.ts](../src/services/turnPlanner.ts) | 対話ターンの計画補助（gemini.ts の Function Calling ループ周辺。会話コアにつき改修慎重） |

### 5.5 `src/server/` — HTTP ルーティング

[src/server/routeRegistry.ts](../src/server/routeRegistry.ts) が `RouteDef[]` を集約しパス照合・ボディ解析・認可・`ctx` 構築。[src/server/httpHelpers.ts](../src/server/httpHelpers.ts) がセッション Cookie 解決。各機能のルートは `src/server/routes/*.ts`:

`authRoutes`（ログイン/登録・DM チャレンジ）, `settingsRoutes`（個人設定・アカウント管理〔表示名/テーマ/パスワード/本人によるアカウント削除〕・最大）, `botRoutes`（Bot 管理・招待リンク導出）, `botAttributeRoutes`（Bot 属性・**有効モジュール `GET/POST /api/bots/modules`**）, `integratedRoutes`（統合管理: Bot 起動/停止/再起動・会話履歴クリア・MCP/認証情報/Google アカウントの Bot 別利用許可）, `memberRequestRoutes`（ギルド利用申請の承認）, `adminRoutes`（管理・監査ログ）, `todoRoutes`, `scheduleRoutes`, `financeRoutes`, `reminderRoutes`, `personalRoutes`（ノート/クリップボード/連絡先）, `personaRoutes`, `playbookRoutes`, `credentialRoutes`, `deliveryRoutes`（朝報/日報）, `webhookRoutes`（`POST /hook/:token` のみ `auth:"none"`）, `mcpRoutes`, **`deviceAuthRoutes`（OAuth デバイスフロー認証 `auth:"none"`）, `deviceMgmtRoutes`（接続デバイス管理）, `desktopClientRoutes`（クライアント配布）**。WebSocket `GET /ws/chat` は [src/server/chatWebSocket.ts](../src/server/chatWebSocket.ts) が `server.ts` の upgrade で受理（Bearer + `?botId=` 検証）。

認可は `RouteAuth`: `"none"` / `"user"`（セッション必須）/ `"admin"`（role 確認）。`auth:"user"` のリソースは必ず `ctx.user.discordId` でスコープする。

### 5.6 フロントエンド / クローラー / ユーティリティ

| 場所 | 内容 |
|---|---|
| [src/public/](../src/public/) | **依存ゼロのバニラ JS SPA**。`app.js`（History API ルーティング・`fetch` ラッパが `botId` を自動注入・タブ別データ取得）、`index.html`、`styles.css`、`sw.js`（PWA）、`manifest.json` |
| [.cursorrules](../.cursorrules) | **UI デザイン制約**: Material Design 2 ダーク。⚠️ **カードコンポーネント禁止**（フラットリスト + 下線区切り） |
| [src/rust_crawler/](../src/rust_crawler/) | Rust 製クローラー（`src/main.rs`: デーモン IPC・fetch・fetch-js・screenshot・Google+DuckDuckGo 検索を RRF ランキング） |
| [src/rust_synapse/](../src/rust_synapse/) | Rust 製シナプスエンジン（`main.rs`/`embedder.rs`/`index.rs`/`storage.rs`: 埋め込み生成・per-scope ブルートフォース cosine KNN・1st Hop 連想。SQLite は read-only 参照） |
| [src/utils/](../src/utils/) | `crypto.ts`(暗号), `embeds.ts`(Discord Embed・色規約), `formatters.ts`, `datetime.ts`/`timezone.ts`(`YYYY-MM-DD HH:MM:SS`・タイムゾーン), `discordMarkdown.ts`, `yamlParser.ts`(依存ゼロ YAML), `secretGuard.ts`/`toolArgRedaction.ts`(秘匿値の形状/キー名マスク), `ssrfGuard.ts`(SSRF 防御), `webhookSignature.ts`(HMAC), `oauthStateStore.ts`, `googleHttpFix.ts` |
| [src/assets/](../src/assets/) | `common-passwords-10k.txt`（PW ブラックリスト） |

---

## 6. 主要ランタイムフロー

### 6.1 Discord メッセージ → 返信（中核フロー）

1. **受信/振り分け** — [src/bot.ts](../src/bot.ts) `setupMessageListener`: Bot 自身/未 ready を除外、メンション/リプライ/DM 判定、登録・権限・レート制限ゲート。
2. **モード分岐** — `isGuildAssistantBot` で 秘書(`processMessage`) か 汎用(`processGuildMessage`/`processBotDmMessage`) を選択。自メンション除去、リプライ連鎖から文脈接頭辞、添付（`image/*`→画像, `audio/*`→音声）を Base64 化。
3. **入口/分離** — [src/gemini.ts](../src/gemini.ts): `ToolContext` に `userId`×`botId`（汎用は `guildId`）で分離確立。秘書は `getUserGenAI`、汎用は `getBotGenAI` で Gemini ハンドル取得（**鍵が無ければ実行しない**）。
4. **文脈組立** — 発話を `addMessageLog` で記録 → Redis 直近（秘書 15 / 汎用 30 件、ミス時 SQLite から再構築）→ リプライ連鎖解決 → `Contents[]` 構築。
5. **システムプロンプト** — `buildSystemInstruction`: ペルソナ → メモリ規則 → 承認フロー → 検索スキル → 現在日時(JST) → カレンダー → コンテキストノート の順（順序は契約）。
6. **ツール集合** — `resolveBotCapabilities` で能力フィルタ → 静的モジュール + MCP 動的モジュールを `buildFunctionRegistry` でマージ。
7. **Function Calling ループ** — `runFunctionCallingLoop`（最大 10 反復）: `generateWithRetry`（429/5xx は指数バックオフ）→ functionCall を `registry.dispatch(ctx, name, args)` で実行 → 結果（JSON 文字列）を contents へ追記 → 再生成。
8. **補完ハルシネーション補正** — ツール未実行なのに「登録した/やっておいた」等と主張した場合のみ、補正プロンプトを 1 回注入し再生成。
9. **リッチ返信** — ハンドラが `ctx.embeds` / `ctx.files`（グラフ PNG）を積む。`richReplyEnabled=false` なら抑制。
10. **永続化/送信** — 応答を `addMessageLog('assistant')` → `toDiscordMarkdown` → 2000 字分割（embeds/files は最終チャンクのみ）→ `safeReply`（例外を握り潰しプロセス死を防止）。

> 詳細: [docs/spec/discordbot_spec.md](../docs/spec/discordbot_spec.md) §3.1（対話エンジン）, [docs/architecture/architecture_v2.md](../docs/architecture/architecture_v2.md) §5（LLM 層）。

### 6.2 HTTP リクエスト → 応答（管理ダッシュボード）

1. [src/server.ts](../src/server.ts) `serverHandler`: HTTPS リダイレクト確認・CORS（baseUrl ホスト一致のみ反映）→ `dispatchRoute`。
2. [src/server/routeRegistry.ts](../src/server/routeRegistry.ts): メソッド/パス照合 → `RouteAuth` 認可 → Cookie からセッション解決（[sessionService](../src/services/sessionService.ts)、Redis or インメモリ）→ アクセス毎に TTL 延長。
3. ボディ解析（POST/DELETE、最大 10MB、JSON）→ パスパラメータ抽出（`:name`）→ `RouteRequestCtx` 構築 → ハンドラ実行 → `sendJson`。
4. 未マッチ: `/api/*` は 404 JSON、その他は静的配信（拡張子無しは SPA の `index.html` へフォールバック）。
5. 認可失敗: `auth:"user"` 無セッション=401、`auth:"admin"` 非管理者=403。Cookie は `__Host-yuuka-session`(HTTPS)/`yuuka-session`(HTTP)、HttpOnly。

---

## 7. データモデルの要点

- **正規の定義元は [src/db/migrations.ts](../src/db/migrations.ts)（schema v16）**。Repo はテーブルを再定義しない。テーブル一覧と列の概要は [docs/architecture/architecture_v2.md](../docs/architecture/architecture_v2.md) §2 にも表がある。
- 日時は一貫して **`'YYYY-MM-DD HH:MM:SS'`（ローカル時刻テキスト、`datetime('now','localtime')`）**。
- 暗号化列は `[encrypted, iv, auth_tag]` の 3 つ組。種類により鍵が異なる（§8）。
- `message_logs` / `bot_context_notes` / `bot_members` は **`users` への FK を持たない**（Web 未登録の Discord ユーザーも記録するため）。これは意図的（汎用モードの分離キー仕様）。
- スキーマ進化: 初版 v2 を基盤に、Bot スコープ拡張で `(user_id)` 制約を `(user_id, bot_id)` へ再構築（既定 `bot_id='system_default'`）。以降 v4=MCP bot 化 / v5=owner リソース許可・Google 複数アカウント / v8=ペルソナ bot 化 / v9=手動停止フラグ / v10=シナプス記憶層 / v12=タスク進捗・サブタスク / v13=デスクトップトークン / v14=ギルド利用申請 / v15=利用可能ロール / v16=ルーチンタスク と段階移行し、**現在 v16**（履歴は [architecture_v2.md](../docs/architecture/architecture_v2.md) §2 末尾）。

---

## 8. 絶対に守る不変条件（CRITICAL）

> 出典は [docs/architecture/architecture_v2.md](../docs/architecture/architecture_v2.md) §0・[docs/spec/bot_attributes_requirements.md](../docs/spec/bot_attributes_requirements.md)。新規実装はこれらを破ってはならない。

1. **データ分離**: 全ユーザーデータクエリは `WHERE user_id = ?` を必須とする。`user_id` 無しのワイルドカード走査禁止（cron の全件走査のみ例外＝明示コメント必須）。**例外**: 汎用モードは `bot_id × user_id`（`bot_context_notes`）/ `bot_id × guild_id`（`bot_guild_notes` 等）を正規の分離キーとする。
2. **ブラウザ操作層は不変**: [src/services/browserService.ts](../src/services/browserService.ts) / [src/rust_crawler/](../src/rust_crawler/) / [src/functions/browserFunctions.ts](../src/functions/browserFunctions.ts) の既存方式（Rust デーモン→Puppeteer、ユーザー別永続セッション、`data-yuuka-id` 数値 ID 注釈）は変更しない。新機能はこの上に載せる。
3. **認証情報を LLM に渡さない**: PW マネージャの復号値は `browserService` へ直接渡す。Function の戻り値・ログ・プロンプトに含めない。旧 `getCredential`（平文返却）は廃止。全アクセスは監査ログへ（PW 本体は記録しない）。
4. **変更禁止ファイル**（統合フェーズのみ可）: [src/types/contracts.ts](../src/types/contracts.ts)・[src/db/migrations.ts](../src/db/migrations.ts)・[src/utils/crypto.ts](../src/utils/crypto.ts)。横断ファイル（`gemini.ts`/`bot.ts`/`index.ts`/`server.ts`/`functions/index.ts`/`public/*`）も統合時のみ編集。
5. **暗号は 2 層**:
   - システム鍵（`YUUKA_ENCRYPTION_SECRET` から scrypt 派生）+ AES-256-GCM = **API キー・Discord トークン・OAuth・Webhook シークレット・MCP 認証**用 → `encryptText`/`decryptText`。
   - **per-user 鍵**（`Argon2id(secret, user.salt)`）+ AES-256-GCM = **PW マネージャ専用** → `encryptForUser`/`decryptForUser`。`users.salt` は不変（変更すると全認証情報が復号不能）。
   - `YUUKA_ENCRYPTION_SECRET` 未設定で起動失敗。`YUUKA_ENCRYPTION_SECRET_NEW` 設定時は起動時 `rotateSecretKey` で全再暗号化。
6. **LLM 鍵のスコープ**: 秘書=`getUserGenAI`（ユーザー自身の鍵のみ、無ければエラー、Bot 鍵へフォールバックしない）/ 汎用=`getBotGenAI`（Bot 鍵、発話者の個人鍵は使わない）。
7. **Function 戻り値は JSON 文字列**。承認が必要な操作（`applyTaskPriorities`/`settlePlannedPayment`/`runPlaybook`/`addCredential` 等）は提案 JSON を返し、**LLM がユーザー確認 → 承認後に確定 Function を再呼び出し**する 2 段階方式。自動確定しない。
8. **リッチ返信ゲート**: `ctx.richReplyEnabled === false` のとき embeds/files を生成せず、その旨の `{success:false,...}` を返す。

---

## 9. コーディング規約

- **ESM**: 相対 import は **`.js` 拡張子付き**（例 `import { x } from "./foo.js"`）。型は明示。
- **新規 npm 依存の追加は原則禁止**（導入済みのみ使用: `bcryptjs`, `@node-rs/argon2`, `rss-parser`, `@napi-rs/canvas`, `chart.js`, `cron-parser` 等。lint/format は `@biomejs/biome`、型チェックは `tsgo`）。`pnpm check` で型 + lint を通すこと。
- **コメント・ログは日本語**。セクション区切り `// ─── ... ───`、絵文字ログ（`🔔🌅📋💳🎂🧹💾✅❌` 等）の既存スタイルを踏襲。
- **Function 命名**: 既存名は維持（UX 互換）、新規は lowerCamelCase。`declarations` の `description` は **日本語で具体的に**（LLM が使い分けられるよう）。名前衝突禁止。
- **新規 HTTP ルート**: `src/server/routes/*.ts` に `RouteDef[]` を export → `server.ts` の `registerRoutes()` に登録。
- **cron 系サービス**: `node-cron` でジョブ登録、ユーザー設定の繰り返しは `cron-parser` で次回時刻計算。多重実行を `ticking` フラグでガード。通知は必ず `notifier.sendToUser` 経由。
- **秘密の取り扱い**: ログ/エラー文字列に PW・トークンを出さない（`sanitizeErrorMessage` / 引数マスク）。

---

## 10. よくある作業の入口

| やりたいこと | 触る場所 |
|---|---|
| LLM ツールを追加 | `src/functions/<domain>Functions.ts` に宣言+ハンドラ → [src/functions/index.ts](../src/functions/index.ts) でマージ（必要なら能力マップ更新）。データは `src/db/<domain>Repo.ts` |
| DB テーブル/列を追加 | [src/db/migrations.ts](../src/db/migrations.ts)（唯一の定義元）→ 対応 Repo。⚠️ 統合フェーズ扱い |
| HTTP API を追加 | `src/server/routes/*.ts` に `RouteDef[]` → `server.ts` で `registerRoutes`。`ctx.user.discordId` でスコープ |
| 定期ジョブを追加 | `src/services/<name>Service.ts` に `start/stop` を実装 → [src/index.ts](../src/index.ts) の起動/終了シーケンスへ登録 |
| ダッシュボード UI を変更 | [src/public/app.js](../src/public/app.js) / `index.html` / `styles.css`。⚠️ [.cursorrules](../.cursorrules)（カード禁止）厳守 |
| Discord 応答整形を変更 | [src/bot.ts](../src/bot.ts)（分割・送信）/ [src/utils/embeds.ts](../src/utils/embeds.ts)（色・Embed）/ [src/utils/discordMarkdown.ts](../src/utils/discordMarkdown.ts) |
| ペルソナ/対話の挙動を変更 | [src/gemini.ts](../src/gemini.ts)（システムプロンプト組立・ループ）。⚠️ 横断ファイル |

---

## 11. 既存ドキュメントの権威順序

矛盾時の優先順位（上が強い）:

1. [docs/architecture/architecture_v2.md](../docs/architecture/architecture_v2.md) — **実装規範・不変条件**（§0 do-not-change、§2 スキーマ、§10 ファイル所有マップ）。仕様と矛盾したら**こちらが優先**。
2. [docs/spec/bot_attributes_requirements.md](../docs/spec/bot_attributes_requirements.md) — Bot 動作モード拡張（capability、2 層メモリ、汎用モードのスコープ）。
3. [docs/spec/discordbot_spec.md](../docs/spec/discordbot_spec.md) — **マスター機能仕様 v0.6.2**（§3 機能、§5 ユーザー/Bot、§6 PW マネージャ、§7 会話履歴、§8 バックアップ、§9 外部連携）。
4. [docs/skills/search_skills.md](../docs/skills/search_skills.md) — 検索クロール時の LLM 指示（システムプロンプトへ注入。天気=気象庁優先 等）。
5. [README.md](../README.md) — 人間向け概要・セットアップ（非規範）。

> 用語の対応に注意: 仕様の「マクロ」＝実装の「Playbook」（同一機能）。

---

## 12. 落とし穴（抜粋）

- **起動失敗**: `YUUKA_ENCRYPTION_SECRET` 未設定で即終了。鍵を変えると既存の暗号化データは復号不能（ローテは `_NEW` 経由）。
- **データ消失**: v1 レガシースキーマ検出時、`migrations.ts` は旧テーブルを **DROP**（不可逆）。v3 以降は冪等な ALTER 中心で破壊的再構築は行わない。
- **discord.js v14**: destroy 済みクライアントは再ログイン不可。`restartDefaultBot` は**新インスタンスを生成**して live binding を差し替える。
- **会話の正は SQLite**: Redis はキャッシュ。`message_logs` は自動削除されない（`clearContext` は Redis 境界マークのみ）。
- **MCP 実行前確認**: `requires_confirmation` は DB のフラグのみ。実際の確認はエージェント/ツール呼出層の責務。
- **autoTag は `setImmediate` の fire-and-forget**: ToDo 削除と競合しうる（`getTodoById` で防御）。
- **Google refresh_token は自動更新されない**: 失効時 `isCalendarEnabled` は静かに false。

---

_最終更新: 2026-06-30 / このファイルはリポジトリ解析に基づく AI 向け索引です。実装が動けば、まず該当ファイルの実コードを正とし、本書とズレがあれば本書を更新してください。_
