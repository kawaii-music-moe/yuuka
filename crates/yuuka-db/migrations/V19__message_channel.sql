-- V19: message_logs にチャンネル ID を追加（会話コンテキストのチャンネル分離）。
--
-- 従来のギルドコンテキストは (bot_id, guild_id) スコープで、同一ギルド内の複数チャンネルで
-- 並行する会話が 1 本の履歴へ混ざり、返信が互いの話題を巻き込む問題があった（V18 の
-- 有効化チャンネル導入で複数チャンネル並行会話が常態化し顕在化）。channel_id を記録し、
-- ギルドコンテキストを (bot_id, guild_id, channel_id) で引く。
--
-- 既存行は channel_id = NULL のまま（バックフィル不能: 従来スキーマにチャンネル情報が無い）。
-- 厳密一致で引くため、移行直後はチャンネルごとに履歴がリセットされた状態から始まる。
ALTER TABLE message_logs ADD COLUMN channel_id TEXT;

-- ギルドコンテキスト取得（bot × guild × channel の直近 N 件を id 降順で引く）用。
CREATE INDEX IF NOT EXISTS idx_message_logs_guild_channel
  ON message_logs (bot_id, guild_id, channel_id, id);
