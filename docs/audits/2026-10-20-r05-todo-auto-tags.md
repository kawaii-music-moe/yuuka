# R05 ToDo自動タグ付けの復旧（#102 / 親 #97）

## 基準と決定

- 調査基準: `origin/develop` の `662f46a`。#102・#97 の本文と全コメント（両方0件）、`CLAUDE.md`（`AGENTS.md` はなし）、機能差分監査とHTTP/ツール対応表、仕様書 §3.2.4、旧Node `bb97bae5` の `autoTagService.ts` / `todoFunctions.ts` / `todoRoutes.ts` / `todoRepo.ts` を確認した。
- 旧版: 会話ツールの追加・サブタスク追加・タイトル/説明変更後に、本人のキーとモデルで補助LLMを呼ぶ。未完了ToDoの既存語彙（使用回数順30件）を優先し、日本語の短いタグを生成する。JSON配列から文字列のみ抽出し、trim・空除去・重複除去・20 UTF-16単位以内・最大3件。固定の許容タグ集合はない。API経路には呼び出しがなかった。
- 旧版は生成中の手動変更まで上書きしていた。実装前に質問し、ユーザーが「案1: 明示タグを保持し、自動タグのみ再生成・併合。由来を保存。手動全削除で停止。既存タグは明示タグとして保護」を選択した。

## 実装と経路監査

| 経路 | 登録/変更先 | 自動付与 |
|---|---|---|
| Discord・PWAチャット・WebSocket・Playbook等の共通会話ツール | `addTodo` / `addSubtask` / `updateTodo` → `TodoRepo` | 作成時、タイトル/説明の実変更時 |
| 管理画面・HTTP API | `/api/tasks/add` / `/api/tasks/update` → `TodoRepo` | 同上。camelCase入力と既存レスポンスを維持 |
| PWAタスク画面 | `POST /api/client/todos` → `TodoRepo::add` | 作成時。PATCHは完了状態のみで再生成しない |
| Webhook変換 | `RealWebhookProcessor` → `TodoRepo::add` | 作成時。`webhook`・エンドポイント名は明示タグとして保持 |
| 支払い予定からの生成 | `FinanceRepo::link_todo` の同一トランザクション内の直接INSERT | 作成時。`支払い`・カテゴリなど既定タグを保持 |
| 繰り返しToDo | 初回は共通作成経路。次回は `advance_routine` が同じ行の期限・進捗・状態を更新 | 初回のみ。次回繰越ではタグを維持 |
| 完了・進捗・優先度・期限・開始日・ルーチン停止・タイムライン連動・支払い消込 | 既存repo更新 | 本文が変わらないため再生成しない |
| 既存ToDo | V27 migration | タグをそのまま明示タグとして保存。一括遡及・外部通信はなし。以後の本文変更から対象 |

V27のINSERT/本文UPDATEトリガーが、ToDo保存と同じトランザクションで `todo_tagging` に予約する。全本番INSERTを検索し、`TodoRepo::add` と `FinanceRepo::link_todo` の2か所を確認した（残りはテストデータ）。入口ごとの予約呼び出し忘れを防ぎ、HTTP・会話経路を一致させる。

supervisorが `AutoTagService` を1つ常時登録する。旧cron移行ゲート `YUUKA_RUST_CRON` には依存しない。1秒間隔で最大4件を取得し、最大4件を並行生成する。補助生成は既存 `GeminiAuxText`（本人の暗号化キー・本人の選択モデル、`GeminiClient::generate_aux`、1要求60秒・最大2回再試行）を再利用する。キー読込・語彙取得・再試行を含めた全体上限は200秒。240秒のリースと取得時の世代更新により、プロセス停止後の処理を回復し、古いリースの応答を破棄する。

キー/暗号鍵未設定、復号失敗、通信失敗、タイムアウト、不正/空応答ではタグを維持し、その予約を終了する。次の本文変更で再予約する。保存障害ではログを残してリース期限後に再試行する。ToDo作成/更新のレスポンスは生成を待たず、LLM失敗の影響を受けない。

## タグの保存・手動編集・競合

- 公開される `todos.tags` は従来のJSON文字列配列のまま。DTOの `Vec<String>`、API出力、`json_each` 検索、タグ別集計、管理画面のTagChipを維持。PWAのタスク画面は従来どおりタグを表示しない。
- 内部テーブルに `explicit_tags` / `auto_tags` / `disabled` / 世代 / 予約・リースを保存する。既存の壊れたタグ文字列もmigrationで変更せず、バックグラウンド処理で黙って消さない。
- 生成成功時は明示タグを先に残し、前回の自動タグだけを新しい最大3件で置換する。明示タグを件数上限で切り捨てない。生成結果と明示タグが重複した場合は明示を優先する。
- 手動setは指定された選択をすべて明示タグにする。add/removeは未変更の自動タグの由来を維持し、addで明示された既存自動タグは明示へ昇格する。入力の正規化・最大8件は従来どおりだが、addで既存タグを切り捨てない。
- 手動操作は現在の行をwriterトランザクション内で読み、併合/削除と由来の保存を一括実行する。生成中の手動操作は世代を進めて古い応答を破棄する。手動全削除は、元から空でも自動付与を停止する。再び非空の手動タグを設定すると停止解除し、次の本文変更から再生成する。
- 自動付与の対象取得・保存・語彙取得は同じ `user_id AND bot_id`。キーはそのユーザー本人のみ。別Bot・別ユーザーの語彙を渡さない。共有BotでもToDoは操作ユーザーのスコープ。
- ユーザーバックアップには親ToDoを介して由来と停止状態をコピーし、他人のToDoの由来を混入させない。

## 検証

- 回帰テストコード: V26→V27の既存タグ保護・遡及なし、生成解析、明示/自動の置換、手動set/add/removeと停止・再開、世代競合・同時手動追加、リース回復・削除、Bot/ユーザー分離、語彙・表示/検索、fake補助LLMの成功/不正応答/未設定/タイムアウト、会話ツール親子作成・本文更新、HTTP作成/更新、PWA作成、支払い予定、Webhook固定タグ、繰り返し時の非再生成、バックアップの由来/停止状態を対象とした。
- ローカル: `cargo fmt --all -- --check`、`cargo check -p yuuka-supervisor --all-targets`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。Clippyで全workspaceのテストコードもコンパイル確認する。
- 補足: Node内蔵SQLiteによるV27 SQL実行で既存タグ・予約・本文変更・全削除停止/再開・cascade削除を確認。NodeのSQLiteが拒否する既存V17の冗長FTS shadow table CREATE文のみ検証時に省き、V27は原文で実行した。
- `CLAUDE.md` とユーザー指示に従い、非developのローカルで `cargo build/test/run/nextest` は実行しない。追加Rustテストは実行済みと扱わない。外部LLM実通信・本番・ブラウザの実操作は未実施。
- PRの既存CIはfmt / cargo-deny / フロント等を実行する。Rust build/testジョブはdevelop→main限定なので、このPRではスキップされる。CI結果とcommitはPRを参照。
