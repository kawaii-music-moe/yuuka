-- 日報・週報に載せる内容（区分キーの JSON 配列: tasks / schedules / payments / finance / topics）。
-- NULL は未設定で、全区分を載せる（本列追加前の設定もこれまで通り全区分）。
ALTER TABLE report_configs ADD COLUMN sections TEXT;
