-- Web 通知（アプリ内の受信箱・docs/design/optional_discord.md §6.1）。
--
-- 通知先が Web の利用者（Discord 未連携の新規利用者の既定）へのリマインド・定期通知などを、
-- Discord の代わりにここへ保存する。管理画面・PWA の通知一覧から読み、既読にする。
CREATE TABLE web_notifications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    bot_id TEXT,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
    read_at TEXT,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX idx_web_notifications_user ON web_notifications(user_id, id DESC);
