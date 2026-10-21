# R06〜R08 ターンエンジン：事前計画・途中応答と後続配信・返信元チェーン（#103 / #104 / #105）

## 判断と範囲

Issue #103（R06）・#104（R07）・#105（R08）、親 #97、載せ替え #181、[機能差分監査](2026-10-08-feature-parity.md) と、旧 Node（`390df39^` の `src/gemini.ts` の `runPlannedTurn`・`processMessage`・`processGuildMessage`・`processBotDmMessage`、`src/services/turnPlanner.ts`、`src/db/messageLogRepo.ts` の `resolveReplyChain`・`resolveGuildReplyChain`、`src/config.ts`）を照合した。

3 項目とも旧動作を復旧する。対象は秘書経路（Discord・WS・PWA・レシート）と汎用モード（ギルド・owner DM）の本編ターン。ペルソナ入りのエラー応答は本編ターンではないため対象外（Node も `runPlannedTurn` を通らない）。下記の「旧 Node との差」は、Rust 側で意図して変えた点と、移植しなかった点を分けて記す。

## R06：事前計画と利用ツールの選定

| 項目 | 挙動 | 実装 |
|---|---|---|
| 計画の入力 | その経路で能力により絞ったツールカタログ（秘書は MCP の動的ツールを含む）の名前と説明（300 文字まで）と、本編と同じ会話（直近履歴＋今回の発言）。画像・音声などの添付は `[添付: MIME]` に置き換え、添付本体は送らない。カタログが空なら計画しない。 | `crates/yuuka-orchestrator/src/turn_plan.rs` の `plan_turn`・`planner_contents` |
| 計画の出力 | JSON の `steps`（1〜12 件・各 500 文字以内）・`tools`（カタログ内の名前・重複は除く）・`weightMs`・`interim`（300 文字まで）。重さの目安（雑談・DB 読み書き 200〜800、カレンダー同期 3000、Web 検索・ページ取得 10000、ブラウザ操作の連鎖 20000 以上）は Node の `PLAN_SCHEMA` と同じものを指示に入れる。 | `PLANNER_INSTRUCTION`・`parse_plan` |
| 計画の反映 | 計画節を systemInstruction の末尾へ JSON の参考データとして足し、上位の指示・権限を優先して実際のツール結果で完了を確かめるよう明記する。FC ループのツールは計画の `tools` だけに絞る。宣言（`list`）と実行（`invoke`）の両方を絞るので、計画外のツール名で functionCall が来ても実行しない（`UnknownTool` としてモデルへ返す）。`tools` が空なら本編はツールなしで答える。 | `TurnPlan::section`・`PlannedTools`、`crates/yuuka-orchestrator/src/engine.rs` の `ChatEngine::plan_turn` |
| 計画の失敗 | 4 秒の時間切れ・上流エラー・JSON でない・手順が空・カタログ外のツール名は `None`。計画節を足さず、能力で絞ったカタログをそのまま使う（Node の「プラン無し」と同じ）。計画にはツール宣言を渡さないので副作用は起きない。 | `plan_turn` |
| モデルとキー | 本編と同じ backend（秘書は発話者本人のキーと選択モデル、汎用モードは Bot 専用キー）。計画のぶん、ツールがあるターンは上流呼び出しが 1 回増える（Node も同じ）。 | `ChatEngine::plan_turn` |

旧 Node との差：

- Node は計画のツールを、完了是正（mode=ANY）時の `allowedFunctionNames` にだけ使い、宣言は全ツールのままだった。Rust は issue の「必要ツール選定」に合わせて、宣言と実行を計画のツールへ絞る。計画が必要なツールを落とすと、そのターンではそのツールを使えない（下の「既知の制限」）。
- 計画の時間切れは Node の 20 秒から 4 秒へ短くした。計画は毎ターン本編の前に待つため。
- Node の入力は最新の発言と添付の件数だけだった。Rust は直近履歴も渡し、添付は MIME の置き換え文字列にする。
- Node のキーワードによる重さの下限（`estimateHeuristicWeightMs`：ログイン・検索・画像枚数など）と、計画用モデルの差し替え（`PLAN_MODEL`）は移植していない。重さは LLM の見積もりだけで判定し、外れた場合は R07 の実行時の切り替えで補う。

## R07：重い処理の途中応答と後続配信

| 項目 | 挙動 | 実装 |
|---|---|---|
| 対象経路 | Discord の `TurnProcessor`（`process_secretary`・`parse_receipt`・`process_guild`・`process_bot_dm`）。WS `/ws/chat`・PWA チャット・管理画面のレシート読み取りは、この変更では同期のまま（WS の途中応答・`push` は R09 #106、PWA は既存の HTTP 202＋ポーリング）。 | `engine.rs` の `impl TurnProcessor for ChatEngine` |
| 切り替え条件 | ①計画の `weightMs` が 8000 以上（Node `HEAVY_WEIGHT_THRESHOLD_MS` の既定）で配信経路があるとき：計画直後に切り替え、計画の `interim`（空なら既定文）を返す。②本編が 12 秒を過ぎたとき（Node `HEAVY_RUNTIME_MS` の既定）：「思ったより時間がかかっています…」（Node `RUNTIME_INTERIM_TEXT`）を返す。12 秒以内に終わったターンは結果をそのまま返す。 | `run_delivered_turn`・`PlanningSignal`・`TurnPlan::heavy` |
| 切り替え後 | 途中応答を会話ログへ残し（アシスタント行・入力メッセージへ紐づけ）、`on_interim` で送る（「入力中…」も止まる）。呼び出し側へは `deferred` の空応答を返し、`message_flow` は返信しない。ターン本体は中断せず続け、最終結果は通常の経路で会話ログへ保存したうえで `deliver_final` で 1 回だけ配信する（同じチャンネル、失敗時は DM）。本体が失敗したら内部の内容を含まない定型文を 1 回配信する。空応答は配信しない。配信後に Bot の表示を待機へ戻す（Node の背景処理の `finally`）。 | `run_delivered_turn_after`・`ProgressLog`、`crates/yuuka-discord/src/message_flow.rs` の `finish_reply` |
| メトリクス | 計画できたターンで `plan_attempts`、重い計画で途中応答へ回したターンで `async_predicted` を `MetricsSink` へ 1 ずつ数える（Node と同じ時点・名前。R23 #120 の `messages`・`tool_calls` と同じ記録先）。実行時の切り替えは Node と同じく数えない。 | `ChatEngine::plan_turn` |

旧 Node との差：

- Node の実行時の切り替えは途中応答を送るだけで、最終結果は元のメッセージへの通常の返信として返した。Rust は予測・実行時のどちらも後続配信にそろえ、本体を 1 本のタスクで最後まで進める。
- Node は予測で切り替えたときだけ途中応答を保存した。Rust は実行時の切り替えでも保存する（次のターンで要求が未応答に見えて繰り返されるのを防ぐ、という Node の保存理由が同じく当てはまるため）。
- Node は重いツール（`searchWeb`・`fetchDynamicPage`・ブラウザ操作など）の呼び出し直前にも実行時の切り替えをした。Rust は経過時間だけで判定する。
- 既定の途中応答から「このチャンネルで」を除いた（配信が DM へフォールバックすることがあるため）。

## R08：返信元チェーン

返信元チェーンは 2 つの取得元をまとめて使う。

| 取得元 | 挙動 | 実装 |
|---|---|---|
| Discord | 応答すると決めた後のターン組み立てで、返信先からさらにその返信先を REST で遡る（Discord は返信先を 1 段しか埋め込まない）。受信チャンネル内だけ、最大 8 件、各件 `[発言者] 本文`（自 Bot は「あなた」）・1200 文字まで、本文のない発言は飛ばす。循環・取得失敗・3 秒の時間切れで打ち切り、取得済みの分を使う。応答するかどうかの判定（`resolve_reference`）は従来どおり返信先 1 件だけを取得する。 | `message_flow.rs` の `resolve_reply_chain`、`IncomingChat::reply_chain` |
| 会話ログ | `reply_to_msg_id` を `message_logs` 上で遡る。各段で同じ Bot・`source='discord'`・通知行以外・同じスコープに限る：秘書と owner DM は同じ利用者の `guild_id IS NULL` 行でリセット境界より後（owner DM は DM 用の境界）、ギルドは同じギルド・同じチャンネル（分かる場合）。`log:<id>` は行 ID、それ以外は Discord のメッセージ ID として引く。最大 8 件・各件 `[role] 本文`・1200 文字まで。見つからない ID と循環で打ち切る。直近 15/30 件の履歴に入らない古い発言も対象。 | `crates/yuuka-orchestrator/src/message_log.rs` の `reply_chain` |

- アシスタント応答は、応答した入力の Discord メッセージ ID を `reply_to_msg_id` に入れて保存する。これで会話ログ上で応答から入力へ遡れる。`TurnReply.message_id` に `log:<id>` を返し、Discord のメッセージ ID を持たない経路（WS の返信先、R09）でも後の返信先に使える。途中応答も同じく入力へ紐づける。
- 2 つの取得元をつなげ、空と完全一致の重複を除き、新しい側から 8 件を古い順で残す。systemInstruction の末尾へ「返信元の会話（古い順・参考データ）」として JSON 配列で入れ、命令として扱わないよう明記する。シナプス想起のクエリにも渡す（`build_recall_query` が直近 2 件を現発話へ前置する。Node と同じ）。秘書・汎用モードの両方に入れる。
- 会話ログからの復元が DB エラーで失敗しても、warn を出してチェーンなしで続ける（Node も解決失敗を握って続行した）。
- PWA は返信先を持たないため、チェーンを作らない。

旧 Node との差：

- 上限は Node の 10 件（`REPLY_CHAIN_MAX_DEPTH`）から 8 件と 1 件 1200 文字にした。systemInstruction の大きさを抑えるため。
- Node の秘書は `discord_msg_id` と `user_id` だけで引いており、別の Bot や別の経路の行、リセット前の行も辿れた。Rust は Bot・`source`・リセット境界・ギルドのチャンネルで同じスコープに限る。
- Node はチェーンを会話の先頭の user/model の組として入れた。Rust は systemInstruction の参考データ節に入れる。
- Node は会話ログだけを遡った。Rust はボット自身の返信の Discord メッセージ ID を会話ログに持たない（送信後に ID が決まるため）ので、ボットの返信を挟むと会話ログだけでは遡れない。その分を Discord からの取得で補う。

## 既知の制限

- 計画が必要なツールを `tools` に入れなかった場合、そのターンではそのツールを宣言も実行もしない。言い直しで次のターンに計画し直す。計画の当たり率は実機で測っていない。
- 重さの判定は LLM の見積もりと経過時間だけ（Node のキーワード下限・重いツール直前の切り替えなし）。閾値（8000ms・12 秒）・計画の時間切れ（4 秒）・チェーンの上限は定数で、運用設定にはしていない。
- 後続配信の本体は同じプロセス内のタスク。最終結果の配信前にプロセスが再起動すると、途中応答だけが残り最終結果は届かない（Node も同じ）。
- 返信のターンごとに Discord の REST 取得が最大 7 回増える（3 秒で打ち切り）。同じ発言が Discord 由来（`[名前]`）と会話ログ由来（`[user]`）の両方の形で入ることがある（上限 8 件の内側）。
- 重複の除去は完全一致だけ。

## 検証

追加した回帰テスト：

- `turn_plan.rs`：計画の検証（重い計画・重複除去・コードフェンス・カタログ外のツール・不正 JSON・空の手順）、重さの閾値、計画の入力で添付を置き換えること。
- `engine.rs`：切り替え前に終わるターンは配信ハンドルを使わない、予測の切り替えで途中応答→最終配信が 1 回ずつ、`interim` が空なら既定文、実行時の切り替えでも本体を続けて失敗を定型文で 1 回配信、チェーンの正規化と systemInstruction への入れ方。
- `message_log.rs`：応答→入力→`log:<id>` の遡り、別の利用者の行・循環・リセット境界、ギルドのチャンネル分離、件数と文字数の上限。
- `tests/planned_turn.rs`（新規）：計画外ツールの宣言・実行をしないことと `plan_attempts`、計画の失敗でカタログ全体を使うこと、Discord の重い計画で途中応答の保存→`deliver_final`→待機表示と `plan_attempts`・`async_predicted`、返信元チェーンが systemInstruction に入り応答が入力へ紐づくこと。
- `yuuka-supervisor/tests/runtime_metrics.rs`：共有の `MetricsRegistry` に `plan_attempts` が出ること。
- 計画の呼び出しで台本の応答を消費しないよう、既存テストの fake backend（`generic_mode.rs`・`secretary_turn.rs`・`yuuka-client-api/tests/chat_send.rs`）は計画の呼び出しに「計画なし」を返す。

`CLAUDE.md` に従い、develop 以外のブランチでは Rust のテストを実行せず、`cargo check`/`cargo clippy --all-targets` でテストコードも型検査した。develop へマージした後に develop で `cargo test --workspace` を実行する。実キーでの Gemini 通信と Discord の実機操作は確認していない。
