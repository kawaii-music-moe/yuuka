-- SD07 (#145): preserve rows; remove implicit Bot ownership.
-- mcp_servers.bot_id is retired: ownership is user_id + bot_mcp_access.
-- run_migrations disables FK enforcement OUTSIDE refinery's V27 transaction.
-- SQLite generalized ALTER TABLE: create new, copy, drop old, rename new.
-- Never rename the old table: child FKs must retain their original targets.
CREATE TEMP TABLE sd07_guard (violations INTEGER CHECK (violations = 0));
INSERT INTO sd07_guard SELECT * FROM pragma_foreign_keys;
CREATE TEMP TABLE sd07_sequence AS SELECT name, seq FROM sqlite_sequence;

-- todos
CREATE TABLE sd07_todos (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      title TEXT NOT NULL,
      description TEXT,
      due_date TEXT,                           -- ISO 8601 (日時または日付)
      priority TEXT,                           -- 'high' | 'medium' | 'low' | NULL (LLM自動付与)
      tags TEXT NOT NULL DEFAULT '[]',         -- JSON string[] (LLM自動付与)
      status TEXT NOT NULL DEFAULT 'open',     -- 'open' | 'done'
      linked_payment_id INTEGER,               -- 支払い予定との紐付け（§3.4）
      due_reminded INTEGER NOT NULL DEFAULT 0, -- 期限接近リマインド送信済みフラグ
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL, start_date TEXT, progress INTEGER NOT NULL DEFAULT 0, parent_id INTEGER, repeat_rule TEXT, repeat_until TEXT, repeat_count INTEGER, list TEXT NOT NULL DEFAULT '個人',
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
INSERT INTO sd07_todos ("id", "user_id", "title", "description", "due_date", "priority", "tags", "status", "linked_payment_id", "due_reminded", "created_at", "updated_at", "bot_id", "start_date", "progress", "parent_id", "repeat_rule", "repeat_until", "repeat_count", "list")
  SELECT id, user_id, title, description, due_date, priority, tags, status, linked_payment_id, due_reminded, created_at, updated_at, bot_id, start_date, progress, parent_id, repeat_rule, repeat_until, repeat_count, list FROM todos;
DROP TABLE todos;
ALTER TABLE sd07_todos RENAME TO todos;
CREATE INDEX idx_todos_user_status ON todos(user_id, status);
CREATE INDEX idx_todos_parent ON todos(parent_id);

-- schedules
CREATE TABLE sd07_schedules (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      title TEXT NOT NULL,
      description TEXT,
      start_at TEXT NOT NULL,
      end_at TEXT,
      remind_before_minutes INTEGER NOT NULL DEFAULT 10,
      reminded INTEGER NOT NULL DEFAULT 0,
      google_event_id TEXT,
      google_calendar_id TEXT,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL,
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
INSERT INTO sd07_schedules ("id", "user_id", "title", "description", "start_at", "end_at", "remind_before_minutes", "reminded", "google_event_id", "google_calendar_id", "created_at", "bot_id")
  SELECT id, user_id, title, description, start_at, end_at, remind_before_minutes, reminded, google_event_id, google_calendar_id, created_at, bot_id FROM schedules;
DROP TABLE schedules;
ALTER TABLE sd07_schedules RENAME TO schedules;
CREATE INDEX idx_schedules_user ON schedules(user_id);
CREATE INDEX idx_schedules_reminded ON schedules(reminded, start_at);

-- reminders
CREATE TABLE sd07_reminders (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      message TEXT NOT NULL,
      trigger_at TEXT NOT NULL,                -- 'YYYY-MM-DD HH:MM:SS'
      repeat_rule TEXT,                        -- cron式（繰り返しの場合）
      target_type TEXT NOT NULL DEFAULT 'dm',  -- 'dm' | 'channel'
      target_id TEXT,
      status TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'sent' | 'cancelled'
      source TEXT NOT NULL DEFAULT 'manual',   -- 'manual'|'todo'|'schedule'|'payment'|'birthday'|'webhook'
      source_id TEXT,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL,
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
INSERT INTO sd07_reminders ("id", "user_id", "message", "trigger_at", "repeat_rule", "target_type", "target_id", "status", "source", "source_id", "created_at", "bot_id")
  SELECT id, user_id, message, trigger_at, repeat_rule, target_type, target_id, status, source, source_id, created_at, bot_id FROM reminders;
DROP TABLE reminders;
ALTER TABLE sd07_reminders RENAME TO reminders;
CREATE INDEX idx_reminders_pending ON reminders(status, trigger_at);
CREATE INDEX idx_reminders_user ON reminders(user_id, status);

-- expenses
CREATE TABLE sd07_expenses (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      type TEXT NOT NULL DEFAULT 'expense',    -- 'income' | 'expense'
      amount INTEGER NOT NULL,                 -- 円単位
      category TEXT NOT NULL,
      memo TEXT,
      date TEXT NOT NULL,                      -- 'YYYY-MM-DD'
      time TEXT,
      source TEXT NOT NULL DEFAULT 'manual',   -- 'manual' | 'receipt_ocr'
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL,
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
INSERT INTO sd07_expenses ("id", "user_id", "type", "amount", "category", "memo", "date", "time", "source", "created_at", "bot_id")
  SELECT id, user_id, type, amount, category, memo, date, time, source, created_at, bot_id FROM expenses;
DROP TABLE expenses;
ALTER TABLE sd07_expenses RENAME TO expenses;
CREATE INDEX idx_expenses_user_date ON expenses(user_id, date);
CREATE INDEX idx_expenses_category ON expenses(user_id, category, date);

-- planned_payments
CREATE TABLE sd07_planned_payments (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      title TEXT NOT NULL,
      amount INTEGER NOT NULL,
      category TEXT NOT NULL,
      memo TEXT,
      due_date TEXT NOT NULL,                  -- 'YYYY-MM-DD'
      repeat_rule TEXT,                        -- cron式（家賃・サブスク等の繰り返し）
      status TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'settled' | 'cancelled'
      settled_expense_id INTEGER,
      linked_todo_id INTEGER,
      linked_reminder_id INTEGER,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL,
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
INSERT INTO sd07_planned_payments ("id", "user_id", "title", "amount", "category", "memo", "due_date", "repeat_rule", "status", "settled_expense_id", "linked_todo_id", "linked_reminder_id", "created_at", "updated_at", "bot_id")
  SELECT id, user_id, title, amount, category, memo, due_date, repeat_rule, status, settled_expense_id, linked_todo_id, linked_reminder_id, created_at, updated_at, bot_id FROM planned_payments;
DROP TABLE planned_payments;
ALTER TABLE sd07_planned_payments RENAME TO planned_payments;
CREATE INDEX idx_planned_payments_user ON planned_payments(user_id, status, due_date);

-- playbook_schedules
CREATE TABLE sd07_playbook_schedules (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      bot_id TEXT NOT NULL, -- 実行結果の通知に使うBot
      playbook_name TEXT NOT NULL,
      cron_expression TEXT NOT NULL,
      description TEXT DEFAULT '',
      enabled INTEGER NOT NULL DEFAULT 1,
      last_run_at TEXT,
      next_run_at TEXT,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      UNIQUE(user_id, playbook_name),
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
INSERT INTO sd07_playbook_schedules ("id", "user_id", "bot_id", "playbook_name", "cron_expression", "description", "enabled", "last_run_at", "next_run_at", "created_at", "updated_at")
  SELECT id, user_id, bot_id, playbook_name, cron_expression, description, enabled, last_run_at, next_run_at, created_at, updated_at FROM playbook_schedules;
DROP TABLE playbook_schedules;
ALTER TABLE sd07_playbook_schedules RENAME TO playbook_schedules;
CREATE INDEX idx_playbook_schedules_user ON playbook_schedules(user_id);

-- playbook_runs
CREATE TABLE sd07_playbook_runs (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      schedule_id INTEGER NOT NULL,
      user_id TEXT NOT NULL,
      playbook_name TEXT NOT NULL,
      status TEXT NOT NULL DEFAULT 'running',  -- 'running' | 'success' | 'failed'
      output TEXT DEFAULT '',
      started_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      finished_at TEXT, bot_id TEXT NOT NULL,
      FOREIGN KEY (schedule_id) REFERENCES playbook_schedules(id) ON DELETE CASCADE
    );
INSERT INTO sd07_playbook_runs ("id", "schedule_id", "user_id", "playbook_name", "status", "output", "started_at", "finished_at", "bot_id")
  SELECT id, schedule_id, user_id, playbook_name, status, output, started_at, finished_at, bot_id FROM playbook_runs;
DROP TABLE playbook_runs;
ALTER TABLE sd07_playbook_runs RENAME TO playbook_runs;
CREATE INDEX idx_playbook_runs_user ON playbook_runs(user_id);

-- clipboard_entries
CREATE TABLE sd07_clipboard_entries (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      content TEXT NOT NULL,
      expires_at TEXT,                         -- NULL = 無期限
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL,
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
INSERT INTO sd07_clipboard_entries ("id", "user_id", "content", "expires_at", "created_at", "bot_id")
  SELECT id, user_id, content, expires_at, created_at, bot_id FROM clipboard_entries;
DROP TABLE clipboard_entries;
ALTER TABLE sd07_clipboard_entries RENAME TO clipboard_entries;
CREATE INDEX idx_clipboard_user ON clipboard_entries(user_id);
CREATE INDEX idx_clipboard_expires ON clipboard_entries(expires_at);

-- contacts
CREATE TABLE sd07_contacts (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      name TEXT NOT NULL,
      birthday TEXT,                           -- 'YYYY-MM-DD' または '--MM-DD'（年不明）
      relationship TEXT,
      contact_info TEXT,
      notes TEXT,
      tags TEXT NOT NULL DEFAULT '[]',         -- JSON string[]
      birthday_reminded_year INTEGER,          -- 当年の誕生日リマインド生成済み判定
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL,
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
INSERT INTO sd07_contacts ("id", "user_id", "name", "birthday", "relationship", "contact_info", "notes", "tags", "birthday_reminded_year", "created_at", "updated_at", "bot_id")
  SELECT id, user_id, name, birthday, relationship, contact_info, notes, tags, birthday_reminded_year, created_at, updated_at, bot_id FROM contacts;
DROP TABLE contacts;
ALTER TABLE sd07_contacts RENAME TO contacts;
CREATE INDEX idx_contacts_user ON contacts(user_id);

-- mcp_servers
CREATE TABLE sd07_mcp_servers (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT,                            -- NULL = システムレベル登録（Adminのみ）
      name TEXT NOT NULL,
      endpoint_url TEXT NOT NULL,
      auth_credential_encrypted TEXT,
      auth_credential_iv TEXT,
      auth_credential_tag TEXT,
      tools_cache TEXT DEFAULT '[]',           -- tools/list の取得結果キャッシュ(JSON)
      tools_cache_updated TEXT,
      requires_confirmation INTEGER NOT NULL DEFAULT 1,
      enabled INTEGER NOT NULL DEFAULT 1,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
INSERT INTO sd07_mcp_servers ("id", "user_id", "name", "endpoint_url", "auth_credential_encrypted", "auth_credential_iv", "auth_credential_tag", "tools_cache", "tools_cache_updated", "requires_confirmation", "enabled", "created_at")
  SELECT id, user_id, name, endpoint_url, auth_credential_encrypted, auth_credential_iv, auth_credential_tag, tools_cache, tools_cache_updated, requires_confirmation, enabled, created_at FROM mcp_servers;
DROP TABLE mcp_servers;
ALTER TABLE sd07_mcp_servers RENAME TO mcp_servers;
CREATE INDEX idx_mcp_servers_user ON mcp_servers(user_id);

-- message_logs
CREATE TABLE sd07_message_logs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL,
        discord_msg_id TEXT,
        role TEXT NOT NULL,
        content TEXT NOT NULL,
        reply_to_msg_id TEXT,
        guild_id TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
      , channel_id TEXT, source TEXT NOT NULL DEFAULT 'discord', rich_content TEXT, is_notice INTEGER NOT NULL DEFAULT 0);
INSERT INTO sd07_message_logs ("id", "user_id", "bot_id", "discord_msg_id", "role", "content", "reply_to_msg_id", "guild_id", "created_at", "channel_id", "source", "rich_content", "is_notice")
  SELECT id, user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id, guild_id, created_at, channel_id, source, rich_content, is_notice FROM message_logs;
DROP TRIGGER message_logs_ai;
DROP TRIGGER message_logs_ad;
DROP TRIGGER message_logs_au;
DROP TABLE message_logs;
ALTER TABLE sd07_message_logs RENAME TO message_logs;
CREATE INDEX idx_message_logs_user ON message_logs(user_id, id);
CREATE INDEX idx_message_logs_discord_msg ON message_logs(discord_msg_id);
CREATE INDEX idx_message_logs_bot_guild ON message_logs(bot_id, guild_id, id);
CREATE TRIGGER message_logs_ai AFTER INSERT ON message_logs BEGIN
      INSERT INTO message_logs_fts(rowid, content) VALUES (new.id, new.content);
    END;
CREATE TRIGGER message_logs_ad AFTER DELETE ON message_logs BEGIN
      INSERT INTO message_logs_fts(message_logs_fts, rowid, content) VALUES ('delete', old.id, old.content);
    END;
CREATE TRIGGER message_logs_au AFTER UPDATE ON message_logs BEGIN
      INSERT INTO message_logs_fts(message_logs_fts, rowid, content) VALUES ('delete', old.id, old.content);
      INSERT INTO message_logs_fts(rowid, content) VALUES (new.id, new.content);
    END;
CREATE INDEX idx_message_logs_guild_channel
  ON message_logs (bot_id, guild_id, channel_id, id);
CREATE INDEX idx_message_logs_user_bot_source
  ON message_logs (user_id, bot_id, source, id);

-- budget_limits
CREATE TABLE sd07_budget_limits (
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL,
        category TEXT NOT NULL,
        limit_amount INTEGER NOT NULL DEFAULT 50000,
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        PRIMARY KEY (user_id, bot_id, category),
        FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
      );
INSERT INTO sd07_budget_limits ("user_id", "bot_id", "category", "limit_amount", "updated_at")
  SELECT user_id, bot_id, category, limit_amount, updated_at FROM budget_limits;
DROP TABLE budget_limits;
ALTER TABLE sd07_budget_limits RENAME TO budget_limits;

-- context_notes
CREATE TABLE sd07_context_notes (
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL,
        content TEXT NOT NULL DEFAULT '',
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), title TEXT NOT NULL DEFAULT 'Shared note',
        PRIMARY KEY (user_id, bot_id),
        FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
      );
INSERT INTO sd07_context_notes ("user_id", "bot_id", "content", "updated_at", "title")
  SELECT user_id, bot_id, content, updated_at, title FROM context_notes;
DROP TABLE context_notes;
ALTER TABLE sd07_context_notes RENAME TO context_notes;

-- briefing_configs
CREATE TABLE sd07_briefing_configs (
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 0,
        schedule_cron TEXT NOT NULL DEFAULT '0 7 * * *',
        target_type TEXT NOT NULL DEFAULT 'dm',
        target_id TEXT,
        weather_lat REAL,
        weather_lng REAL,
        location_name TEXT,
        news_feeds TEXT NOT NULL DEFAULT '[]',
        news_keywords TEXT NOT NULL DEFAULT '[]',
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        PRIMARY KEY (user_id, bot_id),
        FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
      );
INSERT INTO sd07_briefing_configs ("user_id", "bot_id", "enabled", "schedule_cron", "target_type", "target_id", "weather_lat", "weather_lng", "location_name", "news_feeds", "news_keywords", "updated_at")
  SELECT user_id, bot_id, enabled, schedule_cron, target_type, target_id, weather_lat, weather_lng, location_name, news_feeds, news_keywords, updated_at FROM briefing_configs;
DROP TABLE briefing_configs;
ALTER TABLE sd07_briefing_configs RENAME TO briefing_configs;

-- report_configs
CREATE TABLE sd07_report_configs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL,
        type TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 0,
        schedule_cron TEXT NOT NULL DEFAULT '0 21 * * *',
        target_type TEXT NOT NULL DEFAULT 'dm',
        target_id TEXT,
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        UNIQUE(user_id, bot_id, type),
        FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
      );
INSERT INTO sd07_report_configs ("id", "user_id", "bot_id", "type", "enabled", "schedule_cron", "target_type", "target_id", "updated_at")
  SELECT id, user_id, bot_id, type, enabled, schedule_cron, target_type, target_id, updated_at FROM report_configs;
DROP TABLE report_configs;
ALTER TABLE sd07_report_configs RENAME TO report_configs;

-- playbooks
CREATE TABLE sd07_playbooks (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL,
        name TEXT NOT NULL,
        title TEXT NOT NULL,
        keywords TEXT DEFAULT '[]',
        description TEXT DEFAULT '',
        steps TEXT NOT NULL DEFAULT '',
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        UNIQUE(user_id, bot_id, name),
        FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
      );
INSERT INTO sd07_playbooks ("id", "user_id", "bot_id", "name", "title", "keywords", "description", "steps", "created_at", "updated_at")
  SELECT id, user_id, bot_id, name, title, keywords, description, steps, created_at, updated_at FROM playbooks;
DROP TABLE playbooks;
ALTER TABLE sd07_playbooks RENAME TO playbooks;
CREATE INDEX idx_playbooks_user ON playbooks(user_id);

-- bot_active_personas
CREATE TABLE sd07_bot_active_personas (
      user_id TEXT NOT NULL,
      bot_id TEXT NOT NULL,
      persona_id INTEGER NOT NULL,
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      PRIMARY KEY (user_id, bot_id),
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE,
      FOREIGN KEY (persona_id) REFERENCES personas(id) ON DELETE CASCADE
    );
INSERT INTO sd07_bot_active_personas ("user_id", "bot_id", "persona_id", "updated_at")
  SELECT user_id, bot_id, persona_id, updated_at FROM bot_active_personas;
DROP TABLE bot_active_personas;
ALTER TABLE sd07_bot_active_personas RENAME TO bot_active_personas;
CREATE INDEX idx_bot_active_personas_persona ON bot_active_personas(persona_id);

-- task_progress_logs
CREATE TABLE sd07_task_progress_logs (
      id         INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id    TEXT    NOT NULL,                 -- データ分離キー（必須）
      bot_id     TEXT    NOT NULL,
      todo_id    INTEGER NOT NULL,
      progress   INTEGER NOT NULL,                 -- この時点の進捗 0-100
      note       TEXT,                             -- 進捗メモ（任意。例: 「設計が完了した」）
      created_at TEXT    NOT NULL DEFAULT (datetime('now', 'localtime')),
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE,
      FOREIGN KEY (todo_id) REFERENCES todos(id) ON DELETE CASCADE
    );
INSERT INTO sd07_task_progress_logs ("id", "user_id", "bot_id", "todo_id", "progress", "note", "created_at")
  SELECT id, user_id, bot_id, todo_id, progress, note, created_at FROM task_progress_logs;
DROP TABLE task_progress_logs;
ALTER TABLE sd07_task_progress_logs RENAME TO task_progress_logs;
CREATE INDEX idx_task_progress_logs_todo
      ON task_progress_logs(todo_id, created_at);

-- Preserve AUTOINCREMENT high water marks, including previously deleted IDs.
UPDATE sqlite_sequence SET seq = max(seq, COALESCE(
  (SELECT saved.seq FROM sd07_sequence saved WHERE saved.name = sqlite_sequence.name), seq));
DROP TABLE sd07_sequence;
-- Fail inside the same transaction: refinery rolls back DDL, data and history.
INSERT INTO sd07_guard SELECT count(*) FROM pragma_foreign_key_check;
DROP TABLE sd07_guard;
