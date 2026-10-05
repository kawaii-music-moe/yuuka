-- V20: ギルド内チャンネル発言禁止（メンション/返信があっても一切応答しないチャンネル）。
--
-- 発言禁止チャンネルに登録された (bot_id, guild_id, channel_id) では、Bot はメンション・Bot への返信が
-- あっても応答しない（有効化チャンネル `bot_channels` の逆＝「傍受」ではなく「黙殺」ゲート）。判定は
-- 利用資格・レート制限などより手前で行い、当該チャンネルのメッセージは記録もしない。Node 版
-- (`src/bot.ts`) には無い Rust 新機能のため baseline(V17) 後の前方専用マイグレーションとして追加する。
--
-- スコープは有効化チャンネル（bot_channels）と同じく (bot_id, guild_id) 起点に channel_id を加えた
-- 複合主キー。bots 削除で CASCADE 掃除される。
CREATE TABLE IF NOT EXISTS bot_muted_channels (
  bot_id     TEXT NOT NULL,
  guild_id   TEXT NOT NULL,
  channel_id TEXT NOT NULL,
  added_by   TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
  PRIMARY KEY (bot_id, guild_id, channel_id),
  FOREIGN KEY (bot_id) REFERENCES bots(id) ON DELETE CASCADE
);
