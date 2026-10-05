-- V18: ギルド内チャンネル有効化（メンション不要で応答するチャンネル）。
--
-- 有効化チャンネルに登録された (bot_id, guild_id, channel_id) では、Bot はメンション/返信が無くても
-- 応答する（通常はメンション or Bot への返信が必須）。利用資格（メンバー/ロール）・レート制限・Gemini
-- キー必須などの他ゲートは従来どおり適用される。Node 版 (`src/bot.ts`) には無い Rust 新機能のため
-- baseline(V17) 後の前方専用マイグレーションとして追加する。
--
-- スコープは既存の許可リスト群（bot_guilds / bot_members / bot_roles）と同じく (bot_id, guild_id) 起点で、
-- さらに channel_id を加えた複合主キー。bots 削除で CASCADE 掃除される。
CREATE TABLE IF NOT EXISTS bot_channels (
  bot_id     TEXT NOT NULL,
  guild_id   TEXT NOT NULL,
  channel_id TEXT NOT NULL,
  added_by   TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
  PRIMARY KEY (bot_id, guild_id, channel_id),
  FOREIGN KEY (bot_id) REFERENCES bots(id) ON DELETE CASCADE
);
