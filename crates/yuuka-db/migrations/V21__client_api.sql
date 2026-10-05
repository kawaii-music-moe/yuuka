-- V21: PWA クライアント API（/api/client/*・issue #33 port）向けスキーマ追加。
--
-- message_logs.source: Discord と PWA の会話コンテキストを分離するためのソース種別
-- （Node 側は `source TEXT NOT NULL DEFAULT 'discord'` を既に持つ・8/14 merge bb97bae。
-- Rust には未移植だった＝issue #33 本文で指摘の欠落）。既存行は全て Discord 発なので
-- DEFAULT 'discord' により後方互換（既存の Discord/WS コンテキストは無変更で動き続ける）。
ALTER TABLE message_logs ADD COLUMN source TEXT NOT NULL DEFAULT 'discord';

-- PWA 履歴一覧（`user_id × bot_id × source='pwa'` を id 順で引く）・秘書コンテキスト再構築
-- （`user_id × bot_id × source='discord'` を id 降順で N 件）の双方を support する複合 index。
CREATE INDEX IF NOT EXISTS idx_message_logs_user_bot_source
  ON message_logs (user_id, bot_id, source, id);

-- todos.list: PWA の「個人 / 仕事」リスト分け（issue #47: Node 実装は入力を保存せず一覧は常に
-- "Personal" 固定を返すバグだった）。既定値は PWA の選択肢の一方（個人）。
ALTER TABLE todos ADD COLUMN list TEXT NOT NULL DEFAULT '個人';

-- context_notes.title: PWA 共有ノートのタイトル（issue #47: Node の PUT /api/client/shared-note は
-- body.title を受け取りながら永続化せず、レスポンスでエコーするだけだった）。
ALTER TABLE context_notes ADD COLUMN title TEXT NOT NULL DEFAULT 'Shared note';
