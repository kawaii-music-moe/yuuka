# Node → Rust 機能差分・重複の棚卸し（2026-10-08）

## 調査条件と結論

旧Node実装のAPI **180組（HTTP method + path）**と、静的な会話ツール **85件**を列挙し、現行実装の入口と照合した。すべてに対応する入口がある。しかし、入口の存在は機能の同等性を意味しない。起動時の依存注入と処理内容を追うと、以下の **27項目の欠落・縮小・挙動後退**、**8項目の重複**が確認できる。件数は整理上の項目数であり、独立した原因の数ではない。

- 旧版基準：Nodeソース撤去直前の `bb97bae5cb21a36fbddf6a700a7c76122d409b37`（`390df39^`）。以下の `旧 src/...` はこのコミットのファイルを指す。`git show bb97bae5:src/対象ファイル` で再確認できる。
- 現行基準：`93ee446` に2026-10-08の作業ツリー変更を加えた状態。Googleカレンダー同期、家計更新ツール、PWA添付などの未コミット実装を含む。コミット済み・デプロイ済みとは区別する。
- 対象：HTTP API、会話ツール、Discord、WebSocket、定期処理、通知、記憶、外部連携、設定、Svelte管理画面とVue PWA。
- 方法：旧ソースの一覧抽出、現行登録箇所との対応付け、ハンドラ・サービス・起動時接続の静的確認。既存の「未実装」コメントだけでは判定していない。
- 範囲の限界：実トークンを使う外部通信、本番稼働、ブラウザ操作、全入力組み合わせの実行検証は行っていない。これは全入口を対象にした静的棚卸しであり、未発見の差分がないことの証明ではない。
- 調査中にもPWA関連ファイルが更新された。API・ツール対応表は調査開始時のコピーを基準にしており、作業中のPWA修正の完了判定には使わない。

全件の入口対応表：

- [HTTP API 180件](2026-10-08-http-route-coverage.tsv)
- [会話ツール85件](2026-10-08-tool-coverage.tsv)

「高」は通知・記録・共有・保護機能などの実用上の支障が大きい項目、「中」は一部動作・品質・設定の後退、「低」は表示・補助機能の後退を表す。

## 確認済みの欠落・縮小・挙動後退

### 起動時に実処理へ接続されていない機能

| ID | 優先度 | 機能 | 旧Nodeの動作 → 現行の動作 | 根拠・復旧箇所 |
|---|---|---|---|---|
| R01 | 高 | Webhook受信後の処理一式 | 署名検証、リプレイ検出、キーワードフィルタ、ペイロード解釈、Discord通知、ToDo・リマインド化、受信履歴を実行していた。現在は200を返して空の処理を呼ぶ。管理画面の設定CRUDだけは残る。署名も読み取るだけで検証しない。ただし現在は通知などの副作用自体も起きない。 | 旧 `src/services/webhookProcessor.ts:110`。現行 `crates/yuuka-supervisor/src/main.rs:346` が `NullWebhookProcessor` を注入。`crates/yuuka-webhook/src/routes.rs:42,105`。 |
| R02 | 高 | 朝報・日報・週報の「テスト配信」 | Webのテスト操作から実配信していた。現在は `NullDeliveryRunner` が常にfalseを返す。定期配信サービスが存在しても、テスト配信の経路には接続されていない。 | 旧 `src/server/routes/deliveryRoutes.ts`。現行 `crates/yuuka-supervisor/src/lib.rs:89` と `crates/yuuka-briefing/src/routes.rs:43`。 |
| R03 | 高 | Bot共有招待DM | Webで共有招待を作ると承認・辞退ボタン付きDMを送っていた。現在はDB登録後のDM送信が常にfalse。共有データ自体の作成・一覧は残る。 | 旧 `src/server/routes/botRoutes.ts:441`。現行 `crates/yuuka-supervisor/src/lib.rs:92`、`crates/yuuka-orchestrator/src/share_routes.rs:40,54`。 |
| R04 | 高 | Web経由のBot利用申請・承認結果DM | 申請時にオーナーへ、決定時に申請者へ通知していた。HTTPルートには `NullMemberDmSender` が設定されている。Discordのボタン経由では実DM senderが使われるので、入口によって通知の有無が違う。 | 旧 `src/services/memberRequest.ts:64,131`。現行 `crates/yuuka-supervisor/src/lib.rs:91`、`crates/yuuka-orchestrator/src/member_routes.rs:30,53`。Discord側は `crates/yuuka-discord/src/interaction.rs:119,158`。 |

### 会話・記憶・生成処理

| ID | 優先度 | 機能 | 旧Nodeの動作 → 現行の動作 | 根拠・復旧箇所 |
|---|---|---|---|---|
| R05 | 高 | ToDoの自動タグ付け | 追加やタイトル・説明変更後に補助LLMでタグを付け直していた。現在は自動タグ処理がなく、追加時は空タグ。それでもツール結果とプロンプトが「バックグラウンドで自動付与」と案内する。手動タグ編集は残る。 | 旧 `src/services/autoTagService.ts:87,129`、`src/functions/todoFunctions.ts`。現行 `crates/yuuka-todo/src/tools.rs:282,302`、`crates/yuuka-orchestrator/src/system_prompt.rs`。 |
| R06 | 中 | ターンの事前計画・必要ツール選定 | `planTurn` で手順・利用ツール・重い処理を判定し、計画をプロンプトに注入していた。現在のengineには対応する実行経路がない。 | 旧 `src/gemini.ts:883`、`src/services/turnPlanner.ts`。現行 `crates/yuuka-orchestrator/src/engine.rs` のツール選定～FCループ。 |
| R07 | 中 | 重い処理の途中応答と後続配信 | 先に途中応答を返し、最終結果を後で配信していた。現在は同期処理が基本で、engineが渡された `TurnDelivery` を使用しない。PWAのHTTP 202＋ポーリングは別の仕組みで、Discord/WSのこの機能を補わない。 | 旧 `src/gemini.ts:901,946`。現行 `crates/yuuka-orchestrator/src/engine.rs:998,1030`、`crates/yuuka-supervisor/src/ws.rs:342`。 |
| R08 | 中 | 返信元チェーンの文脈・想起への利用 | 返信元IDから過去の会話を遡り、本文と記憶検索に使っていた。現在はIDを受け取って保存するが、会話構築時にチェーンを復元しない。想起関数にも空配列を渡す。遠い発言への「あれについて」の理解が後退する。 | 旧 `src/gemini.ts:1030,1406,1514`。現行 `crates/yuuka-orchestrator/src/engine.rs:392,432`、`crates/yuuka-orchestrator/src/synapse_recall.rs:35`。 |
| R09 | 中 | デスクトップ/WSのボタン操作・メッセージ更新 | WSの `interaction` を処理し、`update` や `push` を返していた。現在はinteractionフレームを無視し、返信のcomponentsも常に空。Discordのボタン処理は存在する。 | 旧 `src/server/chatWebSocket.ts:174`、`src/services/componentInteractionService.ts`。現行 `crates/yuuka-supervisor/src/ws.rs:143,269,341`。 |
| R10 | 中 | 朝報ニュースのLLM要約 | RSSを補助LLMで3～5件にまとめていた。現在はタイトル一覧のみで、要約を呼ばない。天気取得・RSS取得・定期配信自体は存在する。 | 旧 `src/services/briefingService.ts:221`。現行 `crates/yuuka-briefing/src/service.rs:74`。 |
| R11 | 中 | 日報・週報のLLMまとめ | 活動・会話サンプルなどから自然文を生成し、失敗時のみ生データに戻していた。現在は常に生データの定型出力。 | 旧 `src/services/reportService.ts:241`。現行 `crates/yuuka-services/src/report.rs:150,312`。`crates/yuuka-gemini/src/client.rs:251` の補助生成APIはあるが、この処理から呼ばれない。 |
| R12 | 中 | 検索スキル資料のプロンプト注入 | 資料が存在する環境では検索先・巡回方針を読み込んで注入していた。Rustには読み込み・埋め込み・注入がない。検索ツール自体は残る。旧Dockerイメージで資料が同梱されていたかは、この静的比較では確定していない。 | 旧 `src/gemini.ts:80,223`。現行 `crates/yuuka-orchestrator/src/system_prompt.rs:142`、`docs/skills/search_skills.md`。 |
| R13 | 中 | 会話からタイムラインへ写真・動画を保存 | `discord_attachment_url` / `discord_attachment_mime` を受け取り保存していた。現在の `addTimelineRecord` は添付URL取得・保存を行わず、対応する引数も宣言にない。WebのメディアアップロードAPIは存在する。 | 旧 `src/functions/timelineFunctions.ts:195`。現行 `crates/yuuka-timeline/src/tools.rs:201`、`crates/yuuka-timeline/src/media.rs`。 |

### 家計・通知・定期処理

| ID | 優先度 | 機能 | 旧Nodeの動作 → 現行の動作 | 根拠・復旧箇所 |
|---|---|---|---|---|
| R14 | 高 | 既存支出を使う支払い予定の消込 | `settlePlannedPayment(plan_id, expense_id)` で既存の支出に紐付けられた。現在は `expense_id` を宣言・処理せず、常に新しい支出を作る。先にレシート等で記帳した支払いを消し込むと二重計上になり得る。 | 旧 `src/functions/financeFunctions.ts:952`。現行 `crates/yuuka-finance/src/tools.rs:1072,1111`。 |
| R15 | 高 | 消込・キャンセル時の関連リマインド停止 | 支払い予定の消込または取消で `linked_reminder_id` の通知を止めていた。現在は連動取消をしない。支払済み・取消済みの予定の通知が残り得る。関連ToDoの消込時完了は実装済み。 | 旧 `src/functions/financeFunctions.ts:1018,1108`。現行 `crates/yuuka-finance/src/tools.rs:1111,1178`、`crates/yuuka-finance/src/repo.rs` の `settle_plan` / `cancel_plan`。 |
| R16 | 中 | 支出記録後の予算消化率・消込候補提示 | `addExpense` が `budget_usage` と `settlement_candidates` を返し、ユーザーへの確認につなげていた。現在は記帳結果だけ。個別の予算照会・候補検索ツールは残るが、自動で返す経路がない。 | 旧 `src/functions/financeFunctions.ts:513,572`。現行 `crates/yuuka-finance/src/tools.rs:303`。 |
| R17 | 高 | ユーザー既定の通知先 | 送信先省略時にユーザー設定のチャンネル/DMを解決していた。現在のservices→Discord変換は `NotifyTarget::Default` をDMに固定。ToDo期限・予定・誕生日などの既定通知先が設定に従わない。`addReminder` もユーザー設定の宛先を保存しない。明示チャンネルIDを持つ通知は別。 | 旧 `src/services/notifier.ts:78`、`src/functions/reminderFunctions.ts:246`。現行 `crates/yuuka-discord/src/notify_bridge.rs:22`、`crates/yuuka-reminder/src/tools.rs:262`、`crates/yuuka-services/src/reminder.rs:100`。 |
| R18 | 中 | 過去日時のリマインド登録保護 | 単発の過去日時を拒否し、繰り返しの場合は次回時刻に補正していた。現在は日時の形式を確認するだけで保存するため、次の走査で即時通知される可能性がある。 | 旧 `src/functions/reminderFunctions.ts:230`。現行 `crates/yuuka-reminder/src/tools.rs:244`～登録処理。 |
| R19 | 中 | cron式の厳密な登録時検証 | `cron-parser` / `node-cron` で検証していた。現在はToDo・支払い予定・リマインド・配信設定の一部が「空白区切り5個」だけ、PlaybookのWeb登録は非空だけ。保存成功後に実行側で不正式として停止・スキップする状態が起こり得る。 | 旧 `src/functions/todoFunctions.ts:105`、`financeFunctions.ts:894`、`reminderFunctions.ts:220`、`briefingFunctions.ts:120,222`、`src/services/playbookScheduleService.ts:77`。現行各 `tools.rs` の `is_valid_cron_basic`、`crates/yuuka-playbook/src/routes.rs:144`。 |
| R20 | 中 | Botの名前・アバターの定期同期 | 稼働中のBotプロフィールを1時間ごとにDBへ同期していた。現在はReady時と手動同期の経路があり、定期同期タイマーはない。長時間接続中の変更反映が遅れる。 | 旧 `src/bot.ts:195,220,574`。現行 `crates/yuuka-discord/src/tenant.rs:235`、`crates/yuuka-supervisor/src/tenants.rs`。 |
| R21 | 低 | Botが保持する一般ロールへのメンション検出 | 旧実装はdiscord.jsの `mentions.has(botClient.user)` を使用。現在は直接・everyone/here・Bot統合ロールに対応するが、Botへ手動付与した一般ロールを対象とするメンションは判定しない。 | 旧 `src/bot.ts` の `mentions.has`。現行 `crates/yuuka-discord/src/message_flow.rs:767`。一般ロールに対する実Discordイベントでの比較は未実施。 |

### 観測・スコープ・外部通信・設定

| ID | 優先度 | 機能 | 旧Nodeの動作 → 現行の動作 | 根拠・復旧箇所 |
|---|---|---|---|---|
| R22 | 中 | ツール実行実績の永続保存 | ツール名・引数ダイジェスト・成否・所要時間を `tool_outcomes` へ保存していた。現在はテーブルはあるがRustの実行処理に書き込みがない。操作履歴 `ActionRecorder` は別物で、成否・所要時間の永続記録を代替しない。 | 旧 `src/gemini.ts:724`、`src/db/toolOutcomeRepo.ts`。現行 `crates/yuuka-gemini/src/fc_loop.rs:237`、`crates/yuuka-db/migrations/V17__baseline.sql`。 |
| R23 | 中 | 実利用メトリクスの計測 | 会話数、ツール数、想起ヒット、応答時間などを更新していた。現在はレジストリと定期ログはあるが、実処理から増加・記録しない。更新呼び出しはレジストリのテスト内にしか見つからない。 | 旧 `src/gemini.ts:342,724,879,929`、`src/services/metrics.ts`。現行 `crates/yuuka-services/src/metrics.rs:6,35,42`。 |
| R24 | 高 | 日報・週報の支払い予定のBot別分離 | 旧SQLは `user_id AND bot_id`。Rustの日報集計は `user_id` だけで、別Botの支払い予定が混ざる。同じユーザー内のBot分離の後退であり、別ユーザーへの漏洩を確認したわけではない。「bot_id列がない」という現行コメントは、V17の実スキーマと矛盾する。 | 旧 `src/services/reportService.ts:110`。現行 `crates/yuuka-services/src/report.rs:240`付近、`crates/yuuka-db/migrations/V17__baseline.sql:169`。 |
| R25 | 高 | RSS取得直前のDNS解決を含むSSRF検査 | Nodeは取得直前に `assertSafeOutboundUrl` でDNS解決先を確認していた。RustはURL文字列・IPリテラルの簡易検査だけで、ドメインを公開扱いにして通常のHTTPクライアントで取得する。内部IPへ解決されるホストの拒否機能が失われている。 | 旧 `src/services/briefingService.ts:132`、`src/utils/ssrfGuard.ts:167`。現行 `crates/yuuka-briefing/src/service.rs:200`、`crates/yuuka-briefing/src/tools.rs:46`。 |
| R26 | 中 | 運用設定の反映 | `REMINDER_CRON` は読まれて画面表示に使われるが実行周期は毎分固定。`SYNAPSE_RECALL_K`・`SYNAPSE_RECENCY_WEIGHT`・`SYNAPSE_RECENCY_HALFLIFE_HOURS` は定数化。`DESKTOP_TOKEN_TTL_DAYS` は90日、`DESKTOP_DEVICE_CODE_TTL_SEC` は600秒固定。旧設定を渡しても調整できない。計画・返信関連の設定消失はR06～R08に含む。 | 旧 `src/config.ts:83,95,138,144,147,168,174`、`src/services/reminderEngine.ts:221`。現行 `crates/yuuka-services/src/reminder.rs:34`、`crates/yuuka-orchestrator/src/synapse_recall.rs:19`、`crates/yuuka-auth/src/lib.rs:42`、`crates/yuuka-auth/src/device_auth.rs:37`。 |
| R27 | 低 | Googleサイト所有権確認metaの注入 | Nodeは設定値をHTMLのプレースホルダーへ差し込んでいた。Rustでは設定とプレースホルダーは残るが配信時に置換しない。 | 旧 `src/server.ts:230`。現行 `crates/yuuka-core/src/config.rs:37`、`crates/yuuka-web/src/static_files.rs:25`、`frontend/index.html:7`。 |

## 重複している機能・実装

重複があることだけを理由に削除するべきではない。用途別の画面・通信方式として必要なものと、同じ仕様を複数箇所で保守するため不一致を生むものがある。

| ID | 重複箇所 | 現状・影響 | 整理の方向 |
|---|---|---|---|
| D01 | Svelte管理画面とVue PWAのタスク・予定・家計・ノート・設定 | 同じ業務データを別の画面で操作する。両方とも稼働する用途があり、PWAは機能を絞った個人画面。`frontend/src/routes/BotShell.svelte:36` と `client/pwa/src/router.ts`。 | UIを残すなら業務ルールを共有し、機能差を明示する。画面が2つあるだけで不要とはしない。 |
| D02 | ドメインHTTP API、`/api/client/*`、LLMツール | 同じToDo・家計・ノートに3つの入口がある。repoは共有するものが多いが、入力検証・既定値・レスポンスが別実装。例：`yuuka-todo/src/routes.rs`、`yuuka-client-api/src/routes.rs` の `todos_add`、`yuuka-todo/src/tools.rs`。 | repoだけでなく、登録・更新・取消の業務処理を共通サービスに集約する。 |
| D03 | 予定登録・削除の経路 | PWA登録は `addSchedule` ツールを呼ぶ一方、管理APIは `ScheduleRepo::add/delete` を直接呼ぶ。作業中のGoogle同期はツール側にあるため、管理画面経由では同じ外部反映を行わない。旧NodeでもHTTPとツールの経路差があったため、新たな移植欠落とは数えない。 | Google同期を含む予定サービスを全入口から呼ぶ。根拠：`yuuka-client-api/src/routes.rs` の `calendar_add`、`yuuka-schedule/src/routes.rs:76`、`yuuka-schedule/src/tools.rs`。 |
| D04 | cron式の検証 | `todo/finance/reminder/briefing` に簡易検証が重複し、実行時だけ `yuuka-services/src/cron_util.rs` が本物のパーサを使う。R19の原因。 | 循環依存を作らない共通層に検証・次回時刻計算を移し、登録時と実行時を揃える。 |
| D05 | 日時の解釈・保存形式変換 | `yuuka-reminder/src/datetime.rs`、`yuuka-schedule/src/datetime.rs`、`yuuka-client-api` 内の日時変換、`yuuka-services/src/cron_util.rs` が並存。入力ISO、DBローカル時刻、PWA向けUTCという用途差もある。 | 用途差を保った名前付き関数として共通化し、受理形式・オフセットの差を管理する。 |
| D06 | 外向きURL/IPのSSRF検査 | browser、MCP、briefingが個別実装。briefingの検査だけDNS確認がない（R25）。`yuuka-browser/src/ssrf.rs`、`yuuka-mcp/src/http_client.rs`、`yuuka-briefing/src/tools.rs`。 | 通信直前の検査を共有する。URL登録時の簡易検査と通信時検査を区別する。 |
| D07 | API型・契約 | Rust DTO＋ts-rs生成型、管理画面の手書き `frontend/src/lib/api/types.ts`、PWAの `api/contracts.ts` / `api/models.ts` が並存。管理画面では生成型からのimportが見つからず、生成だけではUIとの型ずれを検出できない。PWAは別wire契約なので単純に同一型にできない。 | 同じwire契約の型は生成物を利用し、PWA用viewは明示的な変換と契約管理を行う。`xtask/src/main.rs` に生成・差分比較処理あり。 |
| D08 | レシート読み取りの2つのワークフロー | 管理画面の `/api/expenses/upload-receipt` は秘書ターンに渡す経路、PWAの `/api/client/finance/receipt` は登録前の下書き抽出。目的と保存のタイミングが違う。`yuuka-finance/src/routes.rs`、`yuuka-client-api/src/routes.rs` の `finance_receipt`、`yuuka-supervisor/src/main.rs` の `ReceiptParserAdapter`。 | OCR抽出の共通化は可能だが、下書きと実行の違いは維持し、同じ「読み取り」表示で副作用を混同させない。 |

## 設計上の変更・縮小（復旧要否を判断する項目）

| 項目 | 差分と影響 | 根拠 |
|---|---|---|
| 操作履歴のRedis保存 | NodeはRedisを通常経路、メモリをフォールバックにしていた。Rustの `ActionRecorder` はメモリのみ。取得・マクロ化機能はあるが、アプリ再起動を跨ぐ短期履歴の保持を失う。 | 旧 `src/services/actionRecorder.ts:85,122`、現行 `crates/yuuka-core/src/action_history.rs:121`。 |
| レート制限カウンタのRedis共有 | NodeのTTL付きRedisカウンタがRustでは `InMemoryRateLimiter`。単一プロセス内の制限はあるが、再起動・複数プロセスでのカウンタ共有はない。 | 旧 `src/services/botRateLimit.ts:63`、現行 `crates/yuuka-orchestrator/src/discord_ports.rs:319`。 |
| 会話履歴のRedisキャッシュ | RustはSQLiteを直接読む。履歴が失われたわけではないので機能欠落には数えない。性能上の違い。 | 旧 `src/db/messageLogRepo.ts`、現行 `crates/yuuka-orchestrator/src/message_log.rs:3`。 |
| `runBriefingNow` の配信先 | Nodeは設定された通知先へ送る。Rustは呼び出した会話へのインラインEmbedとして返す。定期朝報とは独立した挙動変更で、設定先への手動配信は代替できない。 | 旧 `src/functions/briefingFunctions.ts`、現行 `crates/yuuka-briefing/src/tools.rs:365`。 |
| 定期配信の見た目 | NodeのEmbed通知に対し、Rustのservices通知ポートはtext中心。朝報・日報の装飾や構造が簡略化されている。 | 旧 `src/services/reportService.ts` / `briefingService.ts`、現行 `crates/yuuka-services/src/notifier.rs:25`。 |

## 失われた機能と誤判定しないもの

- **会話ツールの入口は85件すべて存在**。現行で増えた `updateExpense` / `deleteExpense` を合わせ、検出した静的ツール名は87件。MCPの動的ツールはこの件数に含めない。
- **Googleカレンダー同期**は作業ツリーに追加・削除・取り込み・定期取り込みの実装と起動時接続がある。`crates/yuuka-schedule/src/google_sync.rs`、`tools.rs`、`crates/yuuka-services/src/calendar_sync.rs`、`crates/yuuka-supervisor/src/main.rs`。未コミットなのでデプロイ済みとは扱わない。管理APIの経路差はD03。
- **コンテキストノートとカレンダー情報のプロンプト注入、シナプス想起・抽出**は接続済み。engine冒頭の古い「空移植」コメントは現状と一致しない。返信チェーンはR08として残る。
- **MCP探索・動的ツール実行、Google OAuth・Driveバックアップ、レシート解析、Bot起動停止**には実クライアントの接続経路がある。資格情報なしでNullに落ちること自体を未実装とは判定しない。
- **通常のDiscord会話、音声・画像入力、共有・申請ボタン処理、登録コードDM**は実装あり。Web招待・申請のDMはR03/R04、WSボタンはR09として区別する。
- **ToDo階層・進捗・手動タグ・優先度・ルーチン、支払い予定・予算・候補検索、タイムラインの家計/ToDo連動、Playbook定期実行、朝報/日報/バックアップの定期実行**は実装あり。古いコメントに残る一括「未移植」は根拠にしない。
- **会話要約ツール**は旧版もログを返し、LLM本体にまとめさせる方式。Rust側が独立した要約LLMを呼ばないことは欠落ではない。
- **PWAの会話分離、共有ノートのタイトル保存、ToDoのlist保存、レシート下書き抽出**は現行の追加・改善が含まれる。
- **デスクトップの自動起動・トレイ・ホットキー等のTODO**はクライアント自体の未完了項目。Nodeバックエンドから失われた機能と断定する根拠がないので今回の後退件数に含めない。
- **WASMプラグイン・ローカルSLM・ニューラル埋め込み等の設計案**を、Node版で稼働していた機能として数えない。

## 確認した機能群

| 機能群 | 入口・主経路 | 残る主な差分 |
|---|---|---|
| ログイン・招待・セッション・デバイス認証 | HTTP対応あり、発行/検証の接続あり | デスクトップTTL設定（R26） |
| Bot管理・共有・利用申請・ペルソナ | HTTP対応あり、Discord側も実装あり | Web DM（R03/R04）、プロフィール同期（R20） |
| 秘書会話・汎用Bot・Discord/WS/PWA | engineへの接続あり | 計画・途中応答・返信チェーン・WSボタン（R06～R09） |
| ToDo・階層・ガント・タグ・ルーチン | ツール15件、HTTP対応あり | 自動タグ（R05）、cron（R19） |
| 予定・Google Calendar | ツール3件、HTTP対応あり、作業中の同期あり | 経路重複（D03） |
| リマインド・期限通知・誕生日 | ツール3件、定期サービスあり | 通知先・過去日時・cron（R17～R19） |
| 家計・予算・支払い予定 | 旧ツール14件に更新/削除追加、HTTP対応あり | 消込・連動取消・補足結果（R14～R16） |
| タイムライン・当日計画・メディア | ツール4件、HTTP対応あり | 会話添付の保存（R13） |
| 個人ノート・クリップボード・連絡先 | ツール3+3+5件、HTTP対応あり | 今回の比較では主要機能の欠落を確認せず |
| 汎用Botノート等 | ツール9件、HTTP対応あり | 今回の比較では主要機能の欠落を確認せず |
| Playbook・操作履歴 | ツール5件、HTTP/定期処理あり | cron（R19）、履歴のメモリ化 |
| 朝報・日報・週報 | ツール4件、定期処理あり | 手動テスト・LLM生成・集計分離（R02/R10/R11/R24） |
| Webhook | HTTP対応あり、管理CRUDあり | 実受信処理の全面未接続（R01） |
| ブラウザ・資格情報・チャート・リッチ返信 | ツール9+5+1+1件、実装接続あり | 個々の外部サイト動作は未検証、WS componentsはR09 |
| MCP・Google認可・バックアップ | HTTPと実クライアントの接続あり | 実サービスへの通信成功は未検証 |
| 記憶・過去会話検索/要約 | シナプスと要約ツール1件あり | 返信文脈・実績保存・検索資料・設定（R08/R12/R22/R26） |
| 運用・計測・静的配信 | 起動監督・メトリクス器・配信あり | 計測更新、設定、所有権meta（R23/R26/R27） |

## 修正の推奨順

1. R14/R15/R17/R24：記録の二重計上、誤通知、Bot間のデータ混在を防ぐ。
2. R01/R02/R03/R04：存在する画面・APIが実処理を呼べるよう接続する。
3. R25/R19/R18：外部通信と登録時の保護・入力検証を戻す。
4. R05/R08/R09/R10/R11：ユーザーに約束している会話・自動処理を復旧する。
5. 残る生成補助・運用設定・計測を戻し、D02/D03/D04/D06/D07の共通化で経路間の再発を抑える。

本調査では機能修正・削除・デプロイは行っていない。対応表とこの報告書を追加した。
