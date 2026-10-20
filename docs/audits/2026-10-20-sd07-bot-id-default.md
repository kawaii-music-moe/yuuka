# SD07 / #145: bot_id の暗黙の帰属を除去

対象: [#145](https://github.com/kawaii-music-moe/yuuka/issues/145) のみ。
Issue 本文・コメント（0件）、親 #138 の方針、既存監査文書、V17〜V26、
`schema.rs` / `pool.rs` / `writer.rs`、ドメイン・サービス・ツール・バックアップ・投入経路を調査した。
V17〜V26 は変更せず、V27 を追加する。他 Issue の API フォールバック等は変更しない。

## 対象と保存する構造

V26 到達時の実効スキーマを SQLite に適用して列・制約・索引・トリガーを抽出した。
下記17テーブルの `bot_id` はすべて **TEXT NOT NULL、DEFAULT なし**にする。
18番目の `mcp_servers` はユーザーの追加指示に従い、退役列 `bot_id` 自体を削除する。
それ以外の列は順序・型・NULL可否・既定値も含めて保持する。

| テーブル | 列数（前→後） | 外向きFK数 | 明示index数 | UNIQUE / 複合PK |
|---|---:|---:|---:|---|
| todos | 20→20 | 1 | 2 | id |
| schedules | 12→12 | 1 | 2 | id |
| reminders | 12→12 | 1 | 2 | id |
| expenses | 11→11 | 1 | 2 | id |
| planned_payments | 15→15 | 1 | 1 | id |
| playbook_schedules | 11→11 | 1 | 1 | id; UNIQUE(user_id,playbook_name) |
| playbook_runs | 9→9 | 1 | 1 | id |
| clipboard_entries | 6→6 | 1 | 2 | id |
| contacts | 12→12 | 1 | 1 | id |
| mcp_servers | 13→12 | 1 | 2→1 | id |
| message_logs | 13→13 | 0 | 5 | id |
| budget_limits | 5→5 | 1 | 0 | (user_id,bot_id,category) |
| context_notes | 5→5 | 1 | 0 | (user_id,bot_id) |
| briefing_configs | 12→12 | 1 | 0 | (user_id,bot_id) |
| report_configs | 9→9 | 1 | 0 | id; UNIQUE(user_id,bot_id,type) |
| playbooks | 10→10 | 1 | 1 | id; UNIQUE(user_id,bot_id,name) |
| bot_active_personas | 4→4 | 2 | 1 | (user_id,bot_id) |
| task_progress_logs | 7→7 | 2 | 1 | id |

- 各FKの列・参照先・ON DELETE CASCADEを保持する。`bot_id` に新しいFKは追加しない。
- 内向きFKは `task_progress_logs.todo_id → todos.id`、
  `playbook_runs.schedule_id → playbook_schedules.id`、
  `bot_mcp_access.mcp_server_id → mcp_servers.id`、
  `message_attachments.message_log_id → message_logs.id`。すべてCASCADEを維持する。
- `message_logs_ai/ad/au` の3トリガーを再作成する。FTS5の実体は残し、
  移行コピーではトリガーを実行しないので、検索索引の二重投入・削除を防ぐ。
- V19のchannel索引、V21のsource索引・todos.list・context_notes.title、
  V22のrich_content/is_noticeを含む。V26のwebhook列には変更を加えない。
- 削除するindexは `idx_mcp_servers_user_bot(user_id,bot_id)` のみ。
  `idx_mcp_servers_user`、許可テーブル側の索引・FKは保持する。
- UNIQUE/PKの自動indexはCREATE TABLEで再作成される。
- SQLiteのスキーマ全体を移行前後で比較し、許容する差を上記2点
  （DEFAULT除去、MCP列とその専用indexの削除）だけに固定して検証する。

## MCP列の参照監査

`rg` でリポジトリ全体の `mcp_servers` / 専用index / INSERTを調べ、次を確認した。

- `yuuka-mcp::repo`: user_id所有 + bot_mcp_accessによる許可モデル。
  行マッパは列名を使い、退役列を読まない。登録・更新・削除も同列を使わない。
  システム登録は `user_id IS NULL` のまま対応し、互換Bot値は挿入しない。
- `yuuka-mcp::provider/routes`、`yuuka-integrated::repo/routes`、
  `yuuka-orchestrator::assistant_config/bot_attr_routes`: Bot別利用許可は
  `bot_mcp_access.bot_id` を使い、`mcp_servers.bot_id` は使わない。
- `yuuka-crypto::rotate`:暗号文3列とid/user_idを使用し、退役列は使わない。
- `yuuka-google::backup`: MCPはuser_idでコピーする。出力DBは入力DBのDDLをコピーし、
  その列順で行をコピーするため、削除後の列を要求しない。
- `xtask::salvage`:コピー元と投入先の**共通列**からINSERTを生成する。
  旧MCP列は投入先に無いため除外される。user_id所有と許可行はそのまま扱う。
- frontendの `mcp_servers` はレスポンスの配列名であり、退役列への参照は無い。
- 過去migration・設計資料には旧列の記述を残す（過去DDLのchecksumは変えない）。

## INSERT監査と既存データ

他17テーブルの本番INSERT（repo、HTTP/LLMツール、cron、Webhook、通知・会話保存）は
bot_idを明示している。動的コピーは全列または共通列から列リストを作る。
サービス単体テストの省略INSERT（ToDo3箇所、連絡先、支払い予定、クリップボード）を
`test-bot` 明示へ修正した。バックアップ単体テストの最小独立DDLにはbot_id列自体が無く、
本番INSERT経路ではないため変更しない。

移行は17テーブルの全列を明示コピーし、旧 `system_default` 行もID・内容・帰属を変えず保持する。
MCPは退役列以外の全データ（システム登録のNULL user_id、暗号文、キャッシュ等）を保持する。
このIssueでは既存個人データの再帰属・削除やシステムBot全体の撤去は行わない。
DEFAULT除去は省略をDBで拒否する変更であり、明示されたBotの認可は既存APIの責務のまま。

## 再作成・失敗時・down方針

[SQLite公式の一般的な再作成手順](https://www.sqlite.org/lang_altertable.html#making_other_kinds_of_table_schema_changes)
の新テーブル作成→コピー→旧テーブル削除→新テーブル改名を使う。
旧テーブルを先に改名すると子FKの参照先も変わるため、その方法は使わない。

1. refineryをV26まで通常モードで実行する。V25の削除CASCADEを壊さない。
2. 未適用のV27だけ、接続のFKモードを保存し、**トランザクション外**でOFFにする。
   外側のトランザクション内からの呼び出しは拒否する。
3. refineryの単一V27トランザクション内で18テーブルを再作成する。
   明示列でコピーし、削除済み最大IDを含むsqlite_sequenceの採番位置も保持する。
4. 同一トランザクションの末尾でforeign_key_checkを実行し、違反があればCHECK制約で失敗させる。
   refinery/rusqliteのトランザクションがDDL・行・履歴・一時オブジェクトをロールバックする。
5. 成功/失敗どちらも元のFKモードを復元する。後続migrationは通常モードで実行する。
   生SQLをFK有効のまま流す誤操作も、冒頭のCHECKで変更前に拒否する。

既存の前方専用refinery運用に合わせ、**down migrationは追加しない**。
旧アプリはMCP列を使わず、他17テーブルは既にbot_idを明示する。
データの再帰属は行わないので、旧Bot名への変換や互換値の注入は不要。
適用後のスキーマを戻す必要が生じた場合は、運用停止下で適用前バックアップへ復元する。
再作成ではデータ量に応じた時間・一時ディスク領域が必要。

## 検証

- `scripts/verify-sd07.py`: fresh/upgrade、全スキーマ比較、全行比較、NULLを含む列値、
  17テーブルの省略/明示NULL拒否・実Bot明示INSERT成功、採番位置、外向き・内向きFK、UNIQUE、FTS検索・更新・削除、
  FK不整合時の原子的ロールバック/修復後再試行、FK有効時の生SQL拒否。
  SQLite 3.46.1 / 3.51.2で4ケース成功。Rustがbundledで使うSQLiteは3.46.0。
  Rust CIの軽量jobでも同スクリプトを実行する。
- Rustランナーの回帰テストを追加:子行の保持とCASCADE、再起動時の冪等性、
  FK復元、V27履歴のロールバック/再試行、元のFK=OFF保持、外側Tx拒否。
  既存V25テストで旧ユーザー削除のCASCADEも引き続き検証する。
- 既存のFKなし単体fixtureは、実スキーマのmigrationを完了してから設置する。
  その単体fixtureにもbot_id既定値を残さない。DB全体の制約は上記migration回帰で検証する。
- 部分スキーマ29定義を実migration後に設置でき、bot_id既定値が残らないこともSQLiteで確認。
- Node由来DBの回帰fixtureはV17 baselineから直接作り、全migrationを適用する形へ整理。
- `cargo check -p yuuka-db --all-targets`、`cargo clippy -p yuuka-db --all-targets -- -D warnings`:成功。
- `CARGO_BUILD_JOBS=1 cargo check --workspace --all-targets`:成功（ts-rsの既存serde属性に対する警告あり）。
- `cargo fmt --all -- --check` / `git diff --check`:確認。
- Rustのbuild/test/run/nextestは未実行。CLAUDE.mdとユーザー指定で、非developでは禁止されている。
  Rustテスト追加分の実行結果を主張しない。PRの重いRust CIもdevelop→main限定でskipされる。
