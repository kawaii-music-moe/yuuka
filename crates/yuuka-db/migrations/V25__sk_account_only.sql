-- 本人認証を sk-account に一本化する（Yuuka 側アカウントは持たない）。
-- sk-account に結び付いていない旧 Yuuka アカウントは、所有データごと削除する（users への FK は
-- すべて ON DELETE CASCADE）。以後のログインは sk-account ID だけで利用者を引き当てる。
DELETE FROM users WHERE sk_account_id IS NULL;

-- パスワードは持たないので列ごと落とす。
ALTER TABLE users DROP COLUMN password_hash;

-- 招待コードは Yuuka 独自登録のための仕組みで、登録は sk-account 側が担うため不要。
DROP TABLE IF EXISTS invite_codes;
