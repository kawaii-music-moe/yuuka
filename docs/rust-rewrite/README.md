# yuuka バックエンド Rust 移行 — ドキュメント索引

このディレクトリは、yuuka バックエンド（現行 Node.js/TypeScript）を **Rust へ全面書き換え**する計画一式。
ブランチ `feature/rust-rewrite` で作成（2026-07-01）。実装はまだ開始していない（本ディレクトリは**計画・準備フェーズ**の成果物）。

## 読む順序

1. **[00-decisions.md](00-decisions.md)** — 確定技術選定（ADR）。全バージョン・採用/却下理由の**唯一の基準**。まずここ。
2. **[PLAN.md](PLAN.md)** — 詳細マスタープラン（§1〜14、結合ビュー）。設計の本体。
3. **[design-checkpoint.md](design-checkpoint.md)** — 設計ドラフト＋敵対的検証（22セクション、生成過程の記録）。
4. **[verification/](verification/)** — 技術検証15領域の一次ソース照合レポート（crates.io/docs.rs/公式ドキュメント実測）。
5. **[scaffold/](scaffold/)** — 即利用可能な設定テンプレート（厳格エラーの機械強制など）。実装開始時にコピーして使う。

## マスタープランの構成（[PLAN.md](PLAN.md)）

| 部 | 節 | 内容 | 元パート |
|---|---|---|---|
| 第1部 | §1〜3 | 目的/ゴール・全体アーキテクチャ(crate workspace)・技術選定サマリ | [parts/01-overview.md](parts/01-overview.md) |
| 第4〜5部 | §4〜5 | 厳格エラーアーキテクチャ・自己復帰/スーパーバイザ | [parts/02-errors-resilience.md](parts/02-errors-resilience.md) |
| 第6〜7部 | §6〜7 | Web ランタイム/認証/静的配信/WS・DB/マイグレーション/データ分離 | [parts/03-web-db.md](parts/03-web-db.md) |
| 第8部 | §8 | Discord(twilight) + Gemini/Function Calling | [parts/04-discord-gemini.md](parts/04-discord-gemini.md) |
| 第9〜10部 | §9〜10 | ユーザー拡張モジュール基盤・フロント型連携 | [parts/05-plugins-types.md](parts/05-plugins-types.md) |
| 第11〜12部 | §11〜12 | 段階移行ロードマップ・サブエージェント並行実装ワークフロー | [parts/06-migration-workflow.md](parts/06-migration-workflow.md) |
| 第13〜14部 | §13〜14 | リスク一覧(R-1〜R-24)・オープンな決定事項 | [parts/07-risks-open.md](parts/07-risks-open.md) |

## 検証レポート（[verification/](verification/)）

| ファイル | 領域 | 主要な結論 |
|---|---|---|
| [rpt-errors-thiserror.md](verification/rpt-errors-thiserror.md) | 厳格エラー | thiserror 2.0.18・`#[from]`/`#[non_exhaustive]`・IntoResponse 手書き |
| [rpt-clippy-cargodeny.md](verification/rpt-clippy-cargodeny.md) | エラー機械強制 | 8 restriction lint 個別 deny・cargo-deny `crate=` bans |
| [rpt-resilience-tokio-backon-recloser.md](verification/rpt-resilience-tokio-backon-recloser.md) | 自己復帰 | JoinSet supervisor・backon 1.6.0（backoff は RUSTSEC で禁止）・recloser |
| [rpt-axum-web-runtime.md](verification/rpt-axum-web-runtime.md) | Web | axum 0.8.9・tower-http 0.6.x 固定・FromRequestParts 認可 |
| [rpt-db-sqlx-vs-rusqlite.md](verification/rpt-db-sqlx-vs-rusqlite.md) | DB | rusqlite 0.40.1 推奨・単一writer/split-pool・WAL 単一writer原則 |
| [rpt-migrations-sqlx-refinery.md](verification/rpt-migrations-sqlx-refinery.md) | マイグレーション | refinery 0.9.2・冪等 baseline・DROP 撤廃 |
| [rpt-dual-sqlite-hazard.md](verification/rpt-dual-sqlite-hazard.md) | 移行ハザード | **二重writer即-BUSY**・単一writer集約・BEGIN IMMEDIATE（最重要） |
| [rpt-discord-serenity-twilight.md](verification/rpt-discord-serenity-twilight.md) | Discord | twilight 0.17.1（マルチテナント・caller駆動 poll loop） |
| [rpt-gemini-design-verify.md](verification/rpt-gemini-design-verify.md) | Gemini | generateContent 1:1移植・gemini-3.1-flash-lite(GA)・自前ラッパ |
| [rpt-gemini-funccalling-mcp.md](verification/rpt-gemini-funccalling-mcp.md) | Gemini FC | parametersJsonSchema・ネイティブ MCP(mcpServers)・動的ツール |
| [rpt-gemini-rest-crates.md](verification/rpt-gemini-rest-crates.md) | Gemini crate | 公式SDK不在・Interactions API GA 化・reqwest 薄ラッパ |
| [rpt-plugins-wasm-extism.md](verification/rpt-plugins-wasm-extism.md) | プラグイン | Extism 1.30.0（非信頼はWASM）・.so 不採用・rmcp |
| [rpt-mcp-rmcp.md](verification/rpt-mcp-rmcp.md) | MCP | rmcp 2.0.0 公式SDK・aggregator パターン |
| [rpt-typegen-tsrs-utoipa.md](verification/rpt-typegen-tsrs-utoipa.md) | 型連携 | ts-rs 12.0.1（型のみ）／utoipa 上位互換・専用DTO フェイルクローズ |
| [rpt-nginx-session-strangler.md](verification/rpt-nginx-session-strangler.md) | 移行 | nginx strangler・共有Redis署名鍵不要・WS プロキシ |

> `verification/verify-*.md`（小サイズ）は親エージェントの中間メモ。詳細は上記 `rpt-*.md` を参照。

## 次のアクション（実装開始前）

1. [00-decisions.md](00-decisions.md) 末尾および [PLAN.md §14](PLAN.md) の**オープンな決定事項5件**をユーザーが判断（ts-rs/utoipa, rusqlite/sqlx, generateContent/Interactions, edition, プラグイン初期スコープ）。
2. [scaffold/](scaffold/) を実クレートへ展開し、foundation クレート（core/db/型契約）を凍結。
3. [PLAN.md §12](PLAN.md) のワークフロー分解に従い、サブエージェント並行実装を開始。
