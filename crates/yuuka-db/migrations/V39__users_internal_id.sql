-- 利用者 ID を Discord ID から切り離す（docs/design/optional_discord.md §3.1・§8-2）。
--
-- users の主キー列 discord_id を id に改名し、Discord ID は別の列 discord_id（NULL 可・一意）に移す。
-- 既存利用者は id = 旧 Discord ID のまま引き継ぐので、users を参照する側のテーブルの値は書き換えない。
-- SQLite 3.26 以降の RENAME COLUMN は、他テーブルの REFERENCES users(discord_id) と、トリガー
-- timeline_media_user_delete（V29）の OLD.discord_id も users(id)・OLD.id へ書き換える。
ALTER TABLE users RENAME COLUMN discord_id TO id;

-- Discord 連携（NULL = 未連携）。既存利用者は全員 Discord ID で登録されているので連携済みにする。
ALTER TABLE users ADD COLUMN discord_id TEXT;
ALTER TABLE users ADD COLUMN discord_linked_at TEXT;
UPDATE users SET discord_id = id, discord_linked_at = created_at;
CREATE UNIQUE INDEX idx_users_discord_id ON users(discord_id) WHERE discord_id IS NOT NULL;
