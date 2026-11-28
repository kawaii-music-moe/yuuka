-- 招待コード（REGISTRATION_MODE=invite・docs/design/optional_discord.md §4.1）。
--
-- 管理者が発行し、初めてログインする人が 1 回だけ使える。使われたら used_at / used_by を記録する。
-- 発行者・利用者が削除されても、コードの記録は残す（ON DELETE SET NULL）。
-- V25 で落とした旧 invite_codes（Yuuka 独自登録用）とは別物。
CREATE TABLE invite_codes (
    code TEXT PRIMARY KEY,
    note TEXT,
    created_by TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
    expires_at TEXT,
    used_by TEXT,
    used_at TEXT,
    FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL,
    FOREIGN KEY (used_by) REFERENCES users(id) ON DELETE SET NULL
);
