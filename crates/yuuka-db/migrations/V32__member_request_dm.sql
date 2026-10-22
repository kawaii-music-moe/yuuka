-- 利用申請のオーナー宛 DM（承認/却下ボタン付き）の所在。Web から決定したときにその DM を「承認済み/却下済み」へ
-- 編集してボタンを外すために控える。DM 送信に失敗した申請や本列追加前の申請は NULL のまま。
ALTER TABLE bot_member_requests ADD COLUMN dm_channel_id TEXT;
ALTER TABLE bot_member_requests ADD COLUMN dm_message_id TEXT;
