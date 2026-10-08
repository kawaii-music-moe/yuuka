# Yuuka ドキュメント インデックス

『Yuuka』（Discord Gemini 秘書ボット & Web 管理ダッシュボード）の設計・仕様・運用ドキュメントの目次です。
人間向けの導入・セットアップは [../README.md](../README.md) を参照してください。

---

## 📂 ディレクトリ構成

```
docs/
├── index.md                 ← このファイル（目次）
├── project_overview.md      ← AI/開発者向けの全体像・クレート/ディレクトリマップ（現行の入口）
├── guide/                   ← 現行の利用・運用ガイド（features / setup / deployment）
├── architecture/            ← 設計資料
│   ├── architecture_v2.md   ← ⚠️ 旧 Node/TS 実装（schema v16）時点の履歴資料（現行の規範ではない）
│   └── mcp_dashboard_proxy.md ← MCP ダッシュボード埋め込み方式（旧 Node 実装時点の設計。冒頭の注記参照）
├── spec/                    ← 機能仕様・要件
│   ├── discordbot_spec.md
│   └── bot_attributes_requirements.md
├── skills/                  ← LLM 向けスキル仕様（旧 Node 版は実行時にシステムプロンプトへ注入。Rust 版は未配線）
│   └── search_skills.md
├── design/                  ← 設計方針（旧 Node 実装時点の記述を含む。下記「設計文書」の注記参照）
│   ├── function_modularization.md          ← 機能モジュール化（P1〜P6 実装済み）
│   ├── synapse_cognitive_architecture.md   ← 設計思想・研究裏付け（R0/R1 実装済み）
│   └── desktop_client/                     ← 汎用チャットAPI/デスクトップ（Phase 0/1 実装済み）
└── rust-rewrite/            ← Rust 移行の計画・実施記録（2026-07-16 時点のアーカイブ）
```

---

## 📖 目的別の入口

| 知りたいこと | 読む文書 |
|---|---|
| **現行アーキテクチャを図で把握する（HTML）** | [architecture-current.html](architecture-current.html) |
| プロジェクト全体像・どのファイルを触ればよいか | [project_overview.md](project_overview.md) |
| Node → Rust の機能欠落・重複と全API/会話ツールの対応表 | [機能差分・重複の棚卸し（2026-10-08）](audits/2026-10-08-feature-parity.md) |
| 現行の実装（バックエンド）・DB スキーマ | `crates/`（クレート一覧は [../Cargo.toml](../Cargo.toml)）/ `crates/yuuka-db/migrations/`（V17 baseline〜V22） |
| ローカルセットアップ・Docker デプロイ・機能一覧 | [guide/setup.md](guide/setup.md) / [guide/deployment.md](guide/deployment.md) / [guide/features.md](guide/features.md) |
| PWA クライアント（Vue）の開発手順 | [../client/pwa/README.md](../client/pwa/README.md) |
| 旧 Node 実装時点の不変条件・DB スキーマ v16・ファイル所有マップ（**履歴資料**） | [architecture/architecture_v2.md](architecture/architecture_v2.md) |
| 各機能の詳細仕様（ToDo・家計・ブラウザ操作 等） | [spec/discordbot_spec.md](spec/discordbot_spec.md) |
| Bot の動作モード（秘書 / MCP アシスタント）・能力 | [spec/bot_attributes_requirements.md](spec/bot_attributes_requirements.md) |
| 検索クロール時の LLM 指示（天気・運行・ニュース） | [skills/search_skills.md](skills/search_skills.md) |
| 機能モジュールのユーザー×Bot単位ON/OFF（設計・実装済み） | [design/function_modularization.md](design/function_modularization.md) |
| シナプス認知アーキテクチャ（記憶層 R0/R1 実装済み） | [design/synapse_cognitive_architecture.md](design/synapse_cognitive_architecture.md)（現行実装は `crates/yuuka-synapse`。旧 Node 版の規範は [architecture_v2.md](architecture/architecture_v2.md) §13） |
| 汎用チャットAPI・デスクトップクライアント（Phase 0/1 実装済み） | [design/desktop_client/](design/desktop_client/index.md)（現行実装は `/ws/chat`＝`crates/yuuka-supervisor/src/ws.rs`、デバイス認証＝`crates/yuuka-auth`。旧 Node 版の規範は [architecture_v2.md](architecture/architecture_v2.md) §15） |
| バックエンド Rust 移行の意思決定・実施経緯（履歴資料。2026-07-16 時点） | [rust-rewrite/README.md](rust-rewrite/README.md) |

---

## 📚 文書一覧と参照の優先順位

**現行の実装・運用については、コード（`crates/`・`frontend/`・`client/pwa/`）とマイグレーション（`crates/yuuka-db/migrations/`）を一次情報とします。** 下記の文書は意図・要件・経緯を補うもので、コードと食い違う場合は**コードが正**です（食い違いを見つけたら文書側を更新してください）。旧 Node/TS 実装向けに書かれた文書（`architecture_v2.md` など）は、Rust への全面移行（[#68](https://github.com/kawaii-music-moe/yuuka/pull/68) で Node 撤去）後は履歴資料であり、規範ではありません。これらの文書が挙げる `src/*.ts` などのパスは現行リポジトリに存在しません（リンク切れを含む）。撤去前のソースは `git show 390df39^:<path>`（例: `git show 390df39^:src/gemini.ts`）で参照できます。

### 1. 現行の実装（一次情報）
- **バックエンド**: `crates/`（Rust workspace。クレート構成は [../Cargo.toml](../Cargo.toml)、各クレートの役割は [project_overview.md](project_overview.md) §5）。
- **DB スキーマ**: `crates/yuuka-db/migrations/`（`V17__baseline.sql` ＋ `V18`〜`V22` の前方専用マイグレーション）。
- **管理画面 / PWA**: `frontend/`（Svelte 5 + Vite）/ `client/pwa/`（Vue 3 + Vite）。
- **セットアップ・運用**: [guide/setup.md](guide/setup.md) / [guide/deployment.md](guide/deployment.md)。

### 2. [spec/bot_attributes_requirements.md](spec/bot_attributes_requirements.md) — Bot 属性拡張要件
Bot の動作モード（**secretary 秘書 / mcp_assistant 汎用**）の能力プリセット、2 層メモリ、汎用モードの分離スコープ（`bot_id × user_id` / `bot_id × guild_id`）を規定。要件としては有効だが、実装への言及（`gemini.ts`・`migrations.ts` 等）は旧 Node 実装時点のもの。

### 3. [spec/discordbot_spec.md](spec/discordbot_spec.md) — マスター機能仕様（v0.6.2）
全機能の要件定義。§3 Bot 機能（対話・タスク・リマインド・家計・ブラウザ・マクロ・メモリ・日報・連絡先・会話ログ要約・Webhook・音声）、§4 ペルソナ/API/MCP、§5 ユーザー/Bot 管理、§6 PW マネージャ、§7 会話履歴、§8 バックアップ、§9 外部連携、§10 非機能要件。技術選定の記述（Chart.js / Node.js 等）は旧実装時点のもの。

### 4. [skills/search_skills.md](skills/search_skills.md) — 検索クロールスキル
LLM が `searchWeb` / `fetchDynamicPage` を使う際の推奨ドメイン・クエリ・巡回フロー（天気=気象庁優先、運行=Yahoo、ニュース=一次ソース）。
**⚠️ 旧 Node 版では `src/gemini.ts` が実行時に読み込んでシステムプロンプトへ注入していました。現行の Rust 版では未配線です**（`crates/yuuka-orchestrator/src/system_prompt.rs` の検索スキル節は空のまま。配線方針は [rust-rewrite/remaining-work.md](rust-rewrite/remaining-work.md) の P2-E を参照）。配線時は Docker イメージに docs/ が含まれないため `include_str!` での埋め込みが想定されています。

### 5. [architecture/architecture_v2.md](architecture/architecture_v2.md) — 旧 Node/TS 実装のアーキテクチャ規範（**履歴資料**）
Rust 移行前（DB schema v16）の不変条件・共有型・DB スキーマ・暗号・ファイル所有マップを定義していた文書。**現行の規範ではなく、他の文書に優先しません。** 冒頭に現行の参照先（`crates/`・マイグレーション等）をまとめています。全面的な書き直しは未実施。

### （横断）[project_overview.md](project_overview.md) — AI オンボーディングガイド
リポジトリ全体のアーキテクチャ図・クレート/ディレクトリマップ・主要ランタイムフロー・不変条件・コーディング規約を 1 枚に集約。新規参加者（人間・AI）の最初の入口。

---

## 🧪 設計文書（一部実装済み / 一部は将来構想）

設計の why/what を記す文書群。多くは旧 Node/TS 実装の時点で書かれており、本文中の `src/*.ts` 等のパスは現在存在しません（現行の実装は `crates/`）。設計意図の参照用であり、未実装部分は現行コードへの拘束力を持ちません。

- [design/function_modularization.md](design/function_modularization.md) — **機能モジュール化 設計書**（**P1〜P6 実装済み**）。ユーザーが Bot ごとに有効な機能モジュール（todo/finance/…約14）を選び、有効分の宣言のみ LLM へ渡し UI も該当設定のみ表示する。ユーザー×Bot 単位の上書き層（`bot_user_modules`）でデフォルト Bot 含む全 Bot を個別切替。現行の実装は `crates/yuuka-orchestrator/src/module_catalog.rs`（カタログ）と `bot_user_modules` テーブル（`crates/yuuka-db/migrations/V17__baseline.sql`）。旧 Node 版の規範は [architecture_v2.md §14](architecture/architecture_v2.md)（履歴）。
- [design/synapse_cognitive_architecture.md](design/synapse_cognitive_architecture.md) — **シナプス駆動 認知アーキテクチャ設計方針書**（思想・研究裏付け）。思考補助を「生履歴注入」から「データ層へ外付けしたシナプス記憶＋2-Hop連想」へ刷新。**記憶層 R0/R1（ツール実績記録・Rust エンジン＋L2 想起注入・シナプス抽出）は実装済み**で、現行の実装は `crates/yuuka-synapse`（インプロセス）と `crates/yuuka-orchestrator` の `synapse_*`。旧 Node 版の規範は [architecture_v2.md §13](architecture/architecture_v2.md)（履歴）。R2（2nd Hop 勝率提示）以降は将来フェーズ。
- [design/desktop_client/](design/desktop_client/index.md) — **Yuuka Desktop / 汎用チャット API の設計資料**。OAuth デバイスフロー認証と WebSocket `/ws/chat` の設計経緯を収録。現行コードは `clients/desktop/`、`crates/yuuka-supervisor/src/ws.rs`、`crates/yuuka-auth` を参照してください。Node 版のパスや Phase 計画は履歴情報です。収録: [index](design/desktop_client/index.md) / [requirements](design/desktop_client/requirements.md) / [architecture](design/desktop_client/architecture.md) / [backend_api](design/desktop_client/backend_api.md) / [client_design](design/desktop_client/client_design.md) / [roadmap](design/desktop_client/roadmap.md)。
