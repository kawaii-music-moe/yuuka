# yuuka バックエンド Rust 移行 — ドキュメント索引

このディレクトリは、yuuka バックエンド（Node.js/TypeScript）を **Rust へ全面書き換え**する計画・実装ドキュメント一式。
ブランチ `feature/rust-rewrite`（2026-07-01 起票）。**実装フェーズは完了（2026-07-16）**: 機能パリティ 100%（Web ルート **152/152**・LLM ツール **85/85 + MCP 動的**・常駐サービス予約シーム 0・synapse 吸収）・全ゲート緑（build / clippy -D / **test 717** / deny exit 0）・**dev 環境は web/API/cron/Discord すべて Rust で本稼働中**（`yuuka:dev-rust`・:7855）。**残るのは prod カットオーバー（ユーザー最終判断）のみ**。現況・指摘・残オペレーションは **[remaining-work.md](remaining-work.md)** 冒頭の「移行完了宣言（2026-07-16c）」を参照。

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

## 残作業ロードマップ（→ 移行完了・実施記録）

- **[remaining-work.md](remaining-work.md)** — **移行の進捗トラッカー兼実施記録**（2026-07-16 移行完了宣言に更新）。P0〜P3 のチェックリストはほぼ全消化＝機能パリティ 100%（Web **152/152**・ツール **85/85 + MCP 動的**・常駐 cron 全実装・synapse 吸収）・全ゲート緑（**test 717**）・**dev カットオーバー完了**（経路 B を dev で実証・bot 鬼方カヨコ live・prod 無影響）。**残**: prod カットオーバー（push → CI 緑 → `yuuka:latest` Rust ビルド → dev と同手順・ユーザー最終判断）／Gemini `generateAuxText` 上位層（意図的 defer）／live HTTP 実クレデンシャル疎通の実運用検証。日付付き追記が各セッションの実装記録（新しい順）。

## 実装レビュー

- [review-2026-07-06-phase1.md](review-2026-07-06-phase1.md) — Phase 0〜1 / T1 実装（16クレート・約9,250行）の精読レビュー。HIGH 3件（入力DTOのcamelCase欠落・CSP欠落・/api/tasks形状乖離）＋MED 12件、機械検査実測（clippy/test/deny 通過、fmt 差分あり）、推奨対応順つき。
- [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md) — 上記レビューの**修正方針（唯一の基準）**。7バッチの実施順・各所見の処置（対応／fail-closed＋deferred／却下）・検証戦略。確定事項: wire 契約は「入力camelCase・出力snake_case」の非対称（レビューの「出力DTOもcamelCase」は事実誤認と訂正。ただし reminder の入力のみ snake_case 例外）／移行期データ整合は fail-closed 優先で完全parityは deferred（M-6 連鎖削除のみ parity 実装）／応答形状は Node 厳密一致。
- [review-2026-07-07-batch1.md](review-2026-07-07-batch1.md) — **Batch 1（`9c8688d`）の修正レビュー: 承認**。完了条件全達成・テスト 90→97・gen-types ドリフトなし。reminder の snake_case 例外を Node 実測で正当と確認。所見は LOW 1 件（float priority の受理幅）と記録事項のみ。
- [review-2026-07-09-batch4-6.md](review-2026-07-09-batch4-6.md) — **Batch 4/5/6（M-6）＋migration hazard の修正レビュー: 承認**（M-1 認証縮退・M-2 413/空ボディ・M-4 Auth fatality・M-5 writer panic 隔離・M-6 連鎖削除・baseline 冪等化＋schema_version 刻印）。clippy -D クリーン・対象 4 クレート 60 テスト全緑。運用注意 1 件: V17 書き換えによる refinery checksum divergence（旧バイナリ適用済み DB は履歴削除が必要・カットオーバー後は baseline 変更禁止）。

## 次のアクション（prod カットオーバー・ユーザー判断）

1. **push → CI 緑確認** — ブランチは origin より 5 コミット先行の未 push（P2 完遂コミット群では `rust-ci.yml` 未実行・ローカルゲート緑のみ）。
2. **`yuuka:latest` を Rust でビルド** — `docker build -t yuuka:latest -f Dockerfile .`（現 Dockerfile は Rust CMD・chromium 同梱で実測 ~932MB・Node 版 1.4GB より約 470MB 減）。
3. **prod カットオーバー** — [deploy/cutover-dev-rust.sh](../../deploy/cutover-dev-rust.sh) の手順を prod へ写像（DB バックアップ → Node 停止〔SQLite 単一ライター・P0-2〕→ Rust 起動 → `/` + `/api/setup/status` ヘルス → 失敗時ロールバック）。`YUUKA_RUST_DISCORD=1` は Node bot 停止後に有効化。
4. **カットオーバー後の実運用検証** — live HTTP（Google OAuth/Drive backup/MCP・code-complete/live-unverified）・briefing/report/backup 常駐 cron の実配信・V17 冪等適用の確認（dev では実証済み）。

> 実装開始前の意思決定記録（オープン事項 5 件の判断・scaffold 展開・並行実装ワークフロー）は [00-decisions.md](00-decisions.md) / [PLAN.md](PLAN.md) §12–14 に完了済みの記録として残る。
