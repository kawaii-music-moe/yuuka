# Yuuka ドキュメント インデックス

『Yuuka』（Discord Gemini 秘書ボット & Web 管理ダッシュボード）の設計・仕様・運用ドキュメントの目次です。
人間向けの導入・セットアップは [../README.md](../README.md) を参照してください。

---

## 📂 ディレクトリ構成

```
docs/
├── index.md                 ← このファイル（目次）
├── project_overview.md      ← AI/開発者向けの全体像・モジュールマップ
├── architecture/            ← 実装規範・設計（最優先で従う）
│   └── architecture_v2.md
├── spec/                    ← 機能仕様・要件
│   ├── discordbot_spec.md
│   └── bot_attributes_requirements.md
├── skills/                  ← LLM 実行時に注入されるスキル仕様
│   └── search_skills.md
├── design/                  ← 設計方針（一部は実装済み・架構規範へ昇格）
│   ├── function_modularization.md          ← 機能モジュール化（P1〜P6 実装済み）
│   ├── synapse_cognitive_architecture.md   ← 設計思想・研究裏付け（R0/R1 実装済み）
│   ├── architecture_renewal_v3.md          ← 全体アーキテクチャ一新案（Rustエンジン/トポロジ）
│   └── desktop_client/                     ← 汎用チャットAPI/デスクトップ（Phase 0/1 実装済み）
└── rust-rewrite/            ← バックエンド Rust 全面書き換え（実装完了・dev 稼働中・prod カットオーバー待ち）
```

---

## 📖 目的別の入口

| 知りたいこと | 読む文書 |
|---|---|
| プロジェクト全体像・どのファイルを触ればよいか | [project_overview.md](project_overview.md) |
| 実装の規範・不変条件・DB スキーマ・ファイル所有マップ | [architecture/architecture_v2.md](architecture/architecture_v2.md) |
| 各機能の詳細仕様（ToDo・家計・ブラウザ操作 等） | [spec/discordbot_spec.md](spec/discordbot_spec.md) |
| Bot の動作モード（秘書 / MCP アシスタント）・能力 | [spec/bot_attributes_requirements.md](spec/bot_attributes_requirements.md) |
| 検索クロール時の LLM 指示（天気・運行・ニュース） | [skills/search_skills.md](skills/search_skills.md) |
| 機能モジュールのユーザー×Bot単位ON/OFF（設計・実装済み） | [design/function_modularization.md](design/function_modularization.md) |
| シナプス認知アーキテクチャ（記憶層 R0/R1 実装済み） | [architecture/architecture_v2.md](architecture/architecture_v2.md) §13 |
| 汎用チャットAPI・デスクトップクライアント（Phase 0/1 実装済み） | [architecture/architecture_v2.md](architecture/architecture_v2.md) §15 / [design/desktop_client/](design/desktop_client/index.md) |
| **バックエンド Rust 全面書き換え**（実装完了・dev 稼働中・残=prod カットオーバー） | [rust-rewrite/README.md](rust-rewrite/README.md)（索引）/ [rust-rewrite/remaining-work.md](rust-rewrite/remaining-work.md)（現況・実施記録） |

---

## 📚 文書一覧と権威順序

矛盾した場合は **上にある文書が優先**します。

### 1. [architecture/architecture_v2.md](architecture/architecture_v2.md) — 実装規範（最優先）
仕様を既存コードへ落とし込むための実装コントラクト。**§0 の不変条件（do-not-change）**、§1 共有型、§2 DB スキーマ、§3 暗号、§10 ファイル所有マップを定義。仕様書と矛盾する場合は本書が優先。

### 2. [spec/bot_attributes_requirements.md](spec/bot_attributes_requirements.md) — Bot 属性拡張要件
Bot の動作モード（**secretary 秘書 / mcp_assistant 汎用**）の能力プリセット、2 層メモリ、汎用モードの分離スコープ（`bot_id × user_id` / `bot_id × guild_id`）を規定。architecture_v2 を Bot モード向けに精緻化。

### 3. [spec/discordbot_spec.md](spec/discordbot_spec.md) — マスター機能仕様（v0.6.2）
全機能の要件定義。§3 Bot 機能（対話・タスク・リマインド・家計・ブラウザ・マクロ・メモリ・日報・連絡先・会話ログ要約・Webhook・音声）、§4 ペルソナ/API/MCP、§5 ユーザー/Bot 管理、§6 PW マネージャ、§7 会話履歴、§8 バックアップ、§9 外部連携、§10 非機能要件。

### 4. [skills/search_skills.md](skills/search_skills.md) — 検索クロールスキル
LLM が `searchWeb` / `fetchDynamicPage` を使う際の推奨ドメイン・クエリ・巡回フロー（天気=気象庁優先、運行=Yahoo、ニュース=一次ソース）。
**⚠️ このファイルは [../src/gemini.ts](../src/gemini.ts) が実行時に読み込みシステムプロンプトへインライン注入します。移動・改名する場合は `loadSearchSkills()` の候補パスも更新すること。**

### （横断）[project_overview.md](project_overview.md) — AI オンボーディングガイド
リポジトリ全体のアーキテクチャ図・ディレクトリ/モジュールマップ・主要ランタイムフロー・不変条件・コーディング規約を 1 枚に集約。新規参加者（人間・AI）の最初の入口。

---

## 🧪 設計文書（一部実装済み / 一部は将来構想）

設計の why/what を記す文書群。**実装済み部分の規範は architecture へ昇格済み**（下記の参照先）。未実装部分は現行コードへの拘束力を持たない。

- [design/function_modularization.md](design/function_modularization.md) — **機能モジュール化 設計書**（**P1〜P6 実装済み**）。ユーザーが Bot ごとに有効な機能モジュール（todo/finance/…約14）を選び、有効分の宣言のみ LLM へ渡し UI も該当設定のみ表示する。ユーザー×Bot 単位の上書き層（`bot_user_modules`）でデフォルト Bot 含む全 Bot を個別切替。**実装規範は [architecture_v2.md §14](architecture/architecture_v2.md)**。
- [design/synapse_cognitive_architecture.md](design/synapse_cognitive_architecture.md) — **シナプス駆動 認知アーキテクチャ設計方針書**（思想・研究裏付け）。思考補助を「生履歴注入」から「データ層へ外付けしたシナプス記憶＋2-Hop連想」へ刷新。**記憶層 R0/R1（ツール実績記録・Rust エンジン＋L2 想起注入・シナプス抽出）は実装済み**で、規範は [architecture_v2.md §13](architecture/architecture_v2.md)。R2（2nd Hop 勝率提示）以降は将来フェーズ。
- [design/architecture_renewal_v3.md](design/architecture_renewal_v3.md) — **全体アーキテクチャ一新案（v3 提案）**（システム実装面）。シナプス関連を **Rust の独立プロセス**（`src/rust_synapse/` として実装済み）に切り出し、Node(V8)のメモリトラブルを回避する 3 プロセス構成（Node / Rustシナプスエンジン / ローカルSLM）。プロセストポロジ・Rustエンジン設計・メモリ安全設計・Strangler 段階移行を記述。ローカル SLM ハイブリッド等は未実装。
- [design/desktop_client/](design/desktop_client/index.md) — **Yuuka Desktop / 汎用チャット API 設計資料**。Discord 以外の対話入口として、**クライアント非依存の汎用チャット API**（OAuth デバイスフロー認証 + WebSocket `/ws/chat`）を新設し、会話コア `processMessage()` を**無改修で再利用**する。**バックエンド（Phase 0 認証 / Phase 1 WS チャット）は実装済み**（規範は [architecture_v2.md §15](architecture/architecture_v2.md)）。Windows 向け egui クライアント本体（Phase 2 以降）は未実装。収録: [index](design/desktop_client/index.md) / [requirements](design/desktop_client/requirements.md) / [architecture](design/desktop_client/architecture.md) / [backend_api](design/desktop_client/backend_api.md) / [client_design](design/desktop_client/client_design.md) / [roadmap](design/desktop_client/roadmap.md)。
