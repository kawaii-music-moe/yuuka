-- Webhook エンドポイントを Bot ごとの設定にする（管理画面では Bot 設定の中で作る）。
-- 受信で作る通知・ToDo・リマインドはこの Bot に帰属させる。system_default は個人データを
-- 持たない案内役（#138）なので使わない。
ALTER TABLE webhook_endpoints ADD COLUMN bot_id TEXT;

-- 既存のエンドポイントは、持ち主が所有する Bot のうち最も古いもの（作成日時・id 順）へ寄せる。
-- 所有 Bot が無い利用者の行は NULL のまま残し、受信時に「Bot 未設定」として失敗を記録する。
UPDATE webhook_endpoints
   SET bot_id = (
     SELECT b.id FROM bots b
      WHERE b.user_id = webhook_endpoints.user_id AND b.id != 'system_default'
      ORDER BY b.created_at, b.id
      LIMIT 1
   );

CREATE INDEX IF NOT EXISTS idx_webhook_endpoints_user_bot ON webhook_endpoints(user_id, bot_id);
