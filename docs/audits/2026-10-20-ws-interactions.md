# R09: デスクトップ / WebSocket の対話コンポーネント復旧（2026-10-20）

対象: #106。関連する途中応答・後続配信は #104（R07）、返信チェーンは #105（R08）。

## 判断と実装

旧実装のボタン往復を復旧する。`/ws/chat` は Bearer 認証と接続時の Bot アクセス検証を維持し、`interaction` の `messageId` / `customId` を受け取る。Discord の判定処理を `interaction::decide_component`（Discord REST クライアントを要求しない共通関数。Discord 経路の `decide` もこれに委譲する）として切り出し、同じ DB ポート・通知ポートで呼び出す。業務処理を二重実装しない。共有・申請・ペルソナ操作の権限確認はこの共通処理が担当する。本番の supervisor は `ws_routes_with_interactions` で Discord と同じ `DbMembership`・`DbBotDirectory`（暗号層あり）・Discord Messenger（申請 DM の送信）を注入する。通知ポートを渡さない `ws_routes` は DM を送らない `NullMemberDmSender` を使う。

`TurnReply.components` は Discord API JSON（action row type 1、button type 2）に変換する。`Update` は元の messageId と本文、`components: []` を送信し、`Reply` / followup は新しい messageId の `push` を送信する。操作対象は同じ接続へ実際に配信した最大128件のボタンに限り、IDのすり替えと更新済みボタンの再押下を拒否する。システムDMをデスクトップへ新規転送する機能は従来設計どおり対象外。

`done.messageId` はサーバーが付ける応答の ID で、`TurnReply.message_id`（保存済みログ ID）があればそれを、無ければ従来どおりランダムな ID を使う。クライアントが `msg` フレームに付ける `messageId`（R13 の添付保存で使う送信フレームの ID）とは別物で、ボタンの照合には使わない。

`TurnProcessor::process_secretary` に WebSocket の `TurnDelivery` を渡し、途中応答は `interim`、最終配信は `push` とする。最終処理が直ちに完了しても `done(deferred:true)` より `push` が先に届かないように保留する。通常の応答では `TurnReply.deferred` をそのまま送信する。切断時は writer を終了し、バックグラウンド配信ハンドルが残っても接続タスクを残さない。

## develop の現状との差（R07 / R08 の PR が未マージの間）

- develop の `ChatEngine` は `process_secretary` に渡された `TurnDelivery` を使わず同期で処理する（縮退シーム）。WS の `interim` / `push`（途中応答・後続配信）が実際に流れるのは、R07（#104）の PR がマージされてからになる。それまでは `done` だけが流れ、`deferred` は常に false。
- `TurnReply.message_id` は本 PR で型に追加したが、develop のエンジンはまだ値を入れない。保存済みログ ID（`log:<id>`）を `done.messageId` に載せて返信チェーンの対象にするのは R07 / R08（#104 / #105）の PR がマージされてから。それまではランダムな ID になる。ボタンの照合は接続内で配信した ID で行うため、どちらの ID でも動作する。
- ボタン付きの応答を返すのは、現時点では共有承認後のペルソナ導入確認（followup）と往復確認用のデモボタンだけ。

往復確認用 `DESKTOP_DEMO_COMPONENTS` は既定 false。環境変数または config.yaml で true を明示した場合のみ `/__demo_buttons` が確認ボタンを返す。Gemini を呼ばずに done → interaction → update の往復を確認できる。

## 回帰確認

- WSフレームの形、配信コンポーネント、update時のボタン消去、followupの新しいIDを単体検証（`ws.rs` の `interaction_tests`）。
- 即時に完了する最終配信も done → push の順序を保つことを検証（同上。`WsDelivery` を直接呼ぶため、エンジン側の R07 を待たずに検証できる）。
- 実サーバー・実WebSocket・fake認証/LLMでボタンの往復、偽装action、再押下を検証（`tests/ws_chat.rs`）。
- 既存の ready → msg → done、Cookieのみの認証拒否を継続検証。
- `DESKTOP_DEMO_COMPONENTS` の既定値と明示有効化を検証（`yuuka-core` の config テスト）。

テストは `cargo check --all-targets` で型検査のみ行った。`CLAUDE.md` により develop 以外のブランチでは `cargo test` を実行しないため、develop へマージ後に develop で `cargo test --workspace` を実行して確認する。
