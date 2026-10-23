-- 利用申請の申請者名・ギルド名（Discord のボタンから申請したときに解決した表示名）。決定後にオーナー宛の
-- 受付 DM を決定済みの embed へ編集するときに表示する。Web 経由の申請や本列追加前の申請は NULL（ID で表示）。
ALTER TABLE bot_member_requests ADD COLUMN applicant_label TEXT;
ALTER TABLE bot_member_requests ADD COLUMN guild_label TEXT;
