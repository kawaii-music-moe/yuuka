# D01 / D07 / フロントの API 契約整理（2026-10-20）

対象: #125（D01）、#131（D07）、#161 のフロント側。親 #97、載せ替え #181。#161 のバックエンド側（共有業務サービスと `/api/client/*` の扱い）は [domain-services 監査](2026-10-20-domain-services.md) に記録する。

稼働中の 2 つの UI はどちらも残す。UI の削除や公開 API path の変更は行わない。

## 両 UI の役割と機能差（D01）

| 領域 | Svelte 管理画面（`/admin/`） | Vue PWA | 判断 |
| --- | --- | --- | --- |
| タスク | 階層・進捗ログ・優先度・タグ・繰り返し・ガント | 簡易一覧、作成と完了、リスト（個人 / 仕事） | 共存。登録・更新の検証は Rust 側で行う |
| 予定 | 予定とリマインドの詳細管理 | 簡易カレンダー、ローカル / Google の予定表示と登録 | 共存。ローカル予定の登録・削除は `ScheduleService`（D03 #127）を通す |
| 家計 | 取引・予算・支払い予定・内訳 / 推移・レシート読み取り（秘書ターン経由の登録） | 月次集計・取引一覧・レシートの下書き（確認後に登録） | 共存。下書きと登録の区別（D08 #132）を保つ |
| ノート | コンテキストノート・クリップボード・連絡先 | 共有ノート（タイトル / 本文） | 共存。同じスコープのコンテキストノートを読み書きする |
| 設定 | Bot 設定・ペルソナ・連携・管理者向け設定 | モデル・ペルソナ本文・Google 連携 | 共存。PWA の設定契約は意図的に小さく保つ |
| チャット | 設定と観測のみ | 対話チャット | PWA 専用の通信方式を維持 |

業務ルールはどちらの UI にも、path ごとのハンドラーにも置かず、Rust の業務サービスに置く。各入口は認証済みのユーザーと選択中の Bot を解決してからユースケースを呼ぶ。UI 側の検証は入力補助であり、判定はサーバーが行う。ToDo・家計・ノートの共通サービス化は D02（#126）で行い、その経緯と path ごとの判断は [domain-services 監査](2026-10-20-domain-services.md) にある。PWA のモデル一覧（`client/pwa/src/api/models.ts`）とサーバーの許可リストの一致は、既存の `pwa_model_list_matches_server_allowlist`（`crates/yuuka-client-api/tests/settings.rs`）が検出する。

## 同じ wire 契約は生成型を使う（D07）

`frontend/src/lib/api/types.ts` は、Rust DTO がある契約を手書きせず ts-rs の生成型から作る。

- 個別の値: Todo（入れ子の `TodoWithSubtasks` を含む）・進捗ログ・予定・取引・予算・支払い予定・内訳 / 推移・リマインド・タイムラインの記録 / 計画ブロック・連絡先・クリップボード・ペルソナ・Playbook / 実行履歴 / スケジュール・認証情報・`SessionUser`・`Envelope`。
- レスポンス: `/api/tasks`・`/api/tasks/someday`・`/api/tasks/gantt`（`TaskListData`）、タスク詳細（`TaskDetailData`）、タスク追加 / 更新（`TaskData`）、取引一覧（`ExpenseListData`）、予算・支払い予定・予定・リマインド・タイムライン当日・連絡先・クリップボード・コンテキストノート・ペルソナ・Playbook・実行履歴・認証情報の一覧、`/api/me`（`MeData`）。いずれも Rust のハンドラーが `Envelope<…>` で返す型と同じもの。
- Rust DTO がまだ無い管理用の契約（Bot・Webhook・MCP・配信設定・統合オーバービュー・管理者 API など）は、手書きのまま残す。PWA と管理画面で同じ型だと見せかける置き換えはしない。
- 例外: Playbook のスケジュール一覧。`yuuka-playbook` の `NewSchedule` / `ScheduleListData` / `ScheduleData` は `yuuka-schedule` の同名 DTO と出力先が衝突し、ts-rs は先に書き出した予定側を残すため、Playbook 版は生成されない。`PlaybookSchedulesResponse` は手書きのまま残し、その理由を `types.ts` に書いた。Rust 側の型名の変更は本対応の範囲外。

生成器は Rust の `i64` を `bigint` と出力するが、serde_json は数値として送り、`fetch().json()` は `number` に読む。`frontend/src/lib/api/wire.ts` の `JsonWire<T>` は、この表現差だけを再帰的に `number` へ写し、フィールド名・optional・配列・null 許容は生成型のまま保つ。JavaScript の安全な整数範囲を超える精度は扱えない。範囲を超える ID を扱うなら、先にサーバーで文字列契約にする。

コミット済みの生成型は古くなっていた。再生成で、export 済みなのに未コミットだった DTO（`TaskDetailData`・`TaskProgressLog`・予算・計画ブロックなど）が加わる。生成型は ts-rs の出力のまま保ち、xtask が行末空白だけを除去する（`git diff --check` を通すため）。`gen-types --check` は内容を完全一致で比べるので、`biome.json` で生成ディレクトリを biome の対象から外した。生成型を biome で整形しない。

生成型に揃えたことで、実際の UI の不具合が見つかった。

- タスクのタグ: Rust のクリーンビューでは既に配列。配列を受け付け、キャッシュに残る旧 JSON 文字列も読めるようにした（`parseTaskTags`）。
- `Persona.is_public` は boolean。公開状態の表示と公開 / 非公開の切り替えを boolean で判定する。
- タスク詳細は「フラットな `task`」と兄弟キーの `subtasks` / `progressLogs` / `effectiveProgress` を返し、追加 / 更新は `TaskData`（`{ task }`）を返す。`TaskDetailResponse` と `TaskResponse` を分けた。
- 予定のクリーンビューは `google_calendar_id` を返さない。到達しない「Google同期済み」表示を削除した。表示を戻すなら、先に公開 DTO にフィールドを足す。
- `/api/tasks` 系は `tags`（タグ集計）を返さない。手書き型にだけあった `tags?: TagCount[]` を削除した。
- 認証情報一覧の手書き型（`id` / `name` / `type` / `created_at`）はサーバーの形（`service_name` / `username` / `url` / `updated_at`）と食い違っていた。生成型に置き換えた（画面は独自の型で受けていたので表示は変わらない）。
- 内部のユーザー / Bot 帰属列はドメイン DTO に無く、フロントの型からも外した。

## PWA の wire 境界

PWA は `client/pwa/src/api/contracts.ts`・`AgentGateway`（`gateway.ts`）・`httpAdapter.ts` を独自に持ち続ける。サーバー側の `yuuka-client-api` が、ドメインの値を PWA 用 view（string ID・camelCase・boolean の `completed`・`Envelope` を使わない素の JSON）へ明示的に変換する。これは別の wire 契約であり、管理画面の型の重複ではない。公開する型は `client/pwa/src/api/index.ts` から再エクスポートする。管理画面の生成型を PWA のページから直接 import しない。この境界は `wire.ts` のコメントにも書いた。

`client/pwa/src/api/httpAdapter.contract.test.ts` は、完了の PATCH の body・選択中 Bot の `?botId=`・string ID と素のレスポンス・レシートの下書き（読み取りだけで取引を登録しない）を固定する。API 間の永続化と変換の境界は、D02 の PR のバックエンドテストで確かめる。

## Bot endpoint（#161 フロント側）

`/api/bots` 一覧・プリセット・使用量・共有・利用申請・Discord 同期・プロフィールの endpoint は `botApi.ts` だけで定義する。`botAttributeApi.ts` は既存の呼び出し名を `botApi` のメソッドへの参照として残し、属性・アシスタント設定に固有の path だけを定義する。公開 HTTP path の廃止はしない。

- 利用申請の承認 / 却下 `POST /api/bots/member-requests/{id}/decide` は `{ decision: "approved" | "rejected" }` に統一した。サーバー（`crates/yuuka-orchestrator/src/member_routes.rs` の `decide`）は `decision`（旧形式の `action: "approve" | "reject"` も受理）を読み、`botApi` が送っていた `{ approve: boolean }` は 400 になる。
- 共有の招待・取り消しはどちらも `{ botId, targetUserId }`（`share_routes.rs`）。`botApi` の旧形式（`granteeUsername` / `granteeId`）を直した。共有一覧の型も `shared_user_id` / `shared_username` / `status` に合わせた。
- 受信した申請の一覧は `status=pending` の絞り込みと明示の `botId` を保つ。いずれも user スコープ。
- `botApi.test.ts` は両方の呼び出し名で送る body・path・スコープを確かめ、重複していた呼び出し名が `botApi` と同じ関数を指すことも確かめる。

## 検証

develop 起点のブランチ（`refactor/d01-d07-frontend-contracts`）で実行した。

- `pnpm install --frozen-lockfile`、`pnpm run typecheck:front`（svelte-check）: 649 ファイル、エラー・警告 0。`wire.test.ts` の型アサーション（`expectTypeOf`）もここで検査される。
- `pnpm run test:front`（Vitest）: 3 ファイル 37 件成功（`JsonWire` の型変換、タスクのタグ、Bot endpoint の契約と一本化）。
- `client/pwa` で `npm ci`、`npm run check`（vue-tsc）: エラー 0。`npm test`: 28 件成功（契約テスト 2 件を含む）。
- `pnpm lint`（CI と同じ `biome check frontend/src scripts`）: エラー 0、警告 752 件・info 4 件。develop の同じ範囲と同数で、増えていない。PWA は biome の対象外。
- `cargo check` / `cargo clippy -- -D warnings`（`-p xtask --all-targets`、共有ターゲット）: 成功。
- 生成型: develop の各 `dto.rs` と export 一覧から ts-rs 12 の出力規則（doc コメント・rename・optional・同名衝突時は先勝ち）と xtask の行末空白除去を再現して期待値を作り、コミットする 72 ファイルと比べた。70 ファイルが完全一致。残る `Envelope.ts`（`where` 句付きジェネリック）と `EmptyData.ts`（空構造体は `Record<symbol, never>`）は再現側が扱えない形で、Rust の定義と目視で照合した。

未実施: CLAUDE.md により develop 以外では `cargo run` を実行できないため、`cargo run -p xtask -- gen-types` での再生成と `gen-types --check` は、develop へのマージ後に develop で実行する。
