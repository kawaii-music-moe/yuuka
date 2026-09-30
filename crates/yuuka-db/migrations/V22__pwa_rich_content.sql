-- V22: PWA チャット送信のリッチ返信永続化（issue #41 PR #75 レビュー・P2）。
--
-- 従来 `secretary_turn_pwa` の戻り値（`TurnReply`）が持つ `embeds`/`files` は `message_logs` へ
-- 保存されず `reply.text` だけが永続化されていたため、PWA のチャット履歴（GET /api/client/chat/messages）
-- は常にプレーンテキストしか返せなかった。
--
-- message_logs.rich_content: アシスタント応答の embed 群を JSON 配列で保持する（[`RichEmbed`]・
-- `serde(rename_all="camelCase")`）。バイナリを含まない小さな構造化データのみ（画像等は下の
-- message_attachments へ分離）。既存行は NULL（プレーンテキストのみ・後方互換）。
ALTER TABLE message_logs ADD COLUMN rich_content TEXT;

-- message_logs.is_notice: システム通知（⚠️ レート制限・サーバー混雑・内部エラー・タイムアウト等の定型応答）。
-- PWA の非同期配信（202 + 履歴ポーリング）では、ターンが確定した印として**必ず**アシスタント行が
-- 履歴に現れる必要がある（そうしないとクライアントが応答待ちのまま止まる）。一方エンジンは
-- 「⚠️ 定型応答は履歴に保存しない＝以後の LLM 文脈を汚染しない」不変条件を持つため、通知行は履歴
-- 表示には出すが LLM コンテキストの再構築（recent_context 系）からは除外する。既存行は全て 0。
ALTER TABLE message_logs ADD COLUMN is_notice INTEGER NOT NULL DEFAULT 0;

-- message_attachments: アシスタント応答のファイル添付（グラフ PNG 等・`FileAttachment`）の実バイト列。
-- `message_logs` 1 行（1 ターンのアシスタント応答）に対し 0..N 件。所有者スコープの認可
-- （`GET /api/client/chat/attachments/:id`）は `message_logs.user_id` を経由して行う。
CREATE TABLE IF NOT EXISTS message_attachments (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  message_log_id INTEGER NOT NULL REFERENCES message_logs(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  mime_type TEXT NOT NULL,
  bytes BLOB NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

-- `message_log_id` からの添付一覧取得（履歴表示時）・所有者スコープ確認の両方に使う。
CREATE INDEX IF NOT EXISTS idx_message_attachments_message_log_id
  ON message_attachments (message_log_id);
