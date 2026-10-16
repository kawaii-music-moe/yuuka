-- sk-account を唯一の本人認証元にする。既存の所有者キー（Discord ID）はデータ互換のため維持する。
ALTER TABLE users ADD COLUMN sk_account_id TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_sk_account_id
  ON users(sk_account_id) WHERE sk_account_id IS NOT NULL;

-- 既存本番行の password_hash は、外部 sk-account ID を照合する本番移行トランザクションで
-- 不可逆に消去する（部分 users スキーマを使う旧DB/テストも前方移行できるようDDLと分離）。
