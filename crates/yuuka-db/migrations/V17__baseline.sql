CREATE TABLE IF NOT EXISTS system_settings (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );
CREATE TABLE IF NOT EXISTS users (
      discord_id TEXT PRIMARY KEY,
      username TEXT NOT NULL,
      password_hash TEXT NOT NULL,             -- bcrypt (cost 12)
      salt TEXT NOT NULL,                      -- CSPRNG hex。ユーザー鍵導出(Argon2id)用
      role TEXT NOT NULL DEFAULT 'user',       -- 'user' | 'admin'
      -- ユーザー個別の Gemini API 設定（§4.2: ユーザー間共有不可）
      gemini_api_key_encrypted TEXT,
      gemini_api_key_iv TEXT,
      gemini_api_key_tag TEXT,
      gemini_model TEXT DEFAULT 'gemini-3.1-flash-lite',
      -- ユーザー個別の Google OAuth（§3.2.2, §8: カレンダー/Drive はユーザー毎）
      google_refresh_token_encrypted TEXT,
      google_refresh_token_iv TEXT,
      google_refresh_token_tag TEXT,
      google_calendar_id TEXT,
      google_calendars TEXT DEFAULT '[]',      -- JSON: 同期対象カレンダーIDリスト
      -- ユーザー設定
      rich_reply_enabled INTEGER NOT NULL DEFAULT 1,   -- §3.0.5
      remind_default_minutes INTEGER NOT NULL DEFAULT 10, -- §3.3.2 通知前時間デフォルト
      notify_target_type TEXT NOT NULL DEFAULT 'dm',  -- 'dm' | 'channel'
      notify_target_id TEXT,
      active_persona_id INTEGER,               -- §4.1 適用中ペルソナ
      timezone TEXT NOT NULL DEFAULT 'Asia/Tokyo',
      -- バックアップ設定（§8: ユーザー個人のGoogle Driveへ）
      backup_enabled INTEGER NOT NULL DEFAULT 0,
      backup_interval_hours INTEGER NOT NULL DEFAULT 24, -- 最短1時間〜最長720時間(30日)
      backup_generations INTEGER NOT NULL DEFAULT 7,     -- 保持世代数
      backup_folder_id TEXT,
      backup_last_run_at TEXT,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_username ON users(username);
CREATE TABLE IF NOT EXISTS bots (
      id TEXT PRIMARY KEY,
      user_id TEXT NOT NULL,                   -- Bot作成者（オーナー）
      name TEXT NOT NULL,
      discord_token_encrypted TEXT,
      discord_token_iv TEXT,
      discord_token_tag TEXT,
      recommended_persona_id INTEGER,          -- §5.2: 推奨ペルソナ（is_public のみ可）
      discord_username TEXT,
      discord_avatar_url TEXT,
      suspended INTEGER NOT NULL DEFAULT 0,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), capabilities TEXT NOT NULL DEFAULT '["persona","memory","mcp","secretary"]', persona_id INTEGER, gemini_api_key_encrypted TEXT, gemini_api_key_iv TEXT, gemini_api_key_tag TEXT, discord_application_id TEXT, stopped INTEGER NOT NULL DEFAULT 0, enabled_modules TEXT,
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_bots_user ON bots(user_id);
CREATE TABLE IF NOT EXISTS bot_shares (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      bot_id TEXT NOT NULL,
      owner_id TEXT NOT NULL,                  -- Bot作成者
      shared_user_id TEXT NOT NULL,            -- 招待されたユーザー
      status TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'active' | 'revoked'
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      UNIQUE(bot_id, shared_user_id),
      FOREIGN KEY (bot_id) REFERENCES bots(id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_bot_shares_user ON bot_shares(shared_user_id, status);
CREATE TABLE IF NOT EXISTS personas (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      owner_id TEXT NOT NULL,
      name TEXT NOT NULL,
      prompt TEXT NOT NULL DEFAULT '',         -- 上限20,000文字（アプリ層で検証）
      is_public INTEGER NOT NULL DEFAULT 0,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      FOREIGN KEY (owner_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_personas_owner ON personas(owner_id);
CREATE INDEX IF NOT EXISTS idx_personas_public ON personas(is_public);
CREATE VIRTUAL TABLE IF NOT EXISTS message_logs_fts USING fts5(
      content,
      content='message_logs',
      content_rowid='id',
      tokenize='trigram'
    )
/* message_logs_fts(content) */;
CREATE TABLE IF NOT EXISTS 'message_logs_fts_data'(id INTEGER PRIMARY KEY, block BLOB);
CREATE TABLE IF NOT EXISTS 'message_logs_fts_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS 'message_logs_fts_docsize'(id INTEGER PRIMARY KEY, sz BLOB);
CREATE TABLE IF NOT EXISTS 'message_logs_fts_config'(k PRIMARY KEY, v) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS todos (
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
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL DEFAULT 'system_default', start_date TEXT, progress INTEGER NOT NULL DEFAULT 0, parent_id INTEGER, repeat_rule TEXT, repeat_until TEXT, repeat_count INTEGER,
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_todos_user_status ON todos(user_id, status);
CREATE TABLE IF NOT EXISTS schedules (
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
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL DEFAULT 'system_default',
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_schedules_user ON schedules(user_id);
CREATE INDEX IF NOT EXISTS idx_schedules_reminded ON schedules(reminded, start_at);
CREATE TABLE IF NOT EXISTS reminders (
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
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL DEFAULT 'system_default',
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_reminders_pending ON reminders(status, trigger_at);
CREATE INDEX IF NOT EXISTS idx_reminders_user ON reminders(user_id, status);
CREATE TABLE IF NOT EXISTS expenses (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      type TEXT NOT NULL DEFAULT 'expense',    -- 'income' | 'expense'
      amount INTEGER NOT NULL,                 -- 円単位
      category TEXT NOT NULL,
      memo TEXT,
      date TEXT NOT NULL,                      -- 'YYYY-MM-DD'
      time TEXT,
      source TEXT NOT NULL DEFAULT 'manual',   -- 'manual' | 'receipt_ocr'
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL DEFAULT 'system_default',
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_expenses_user_date ON expenses(user_id, date);
CREATE INDEX IF NOT EXISTS idx_expenses_category ON expenses(user_id, category, date);
CREATE TABLE IF NOT EXISTS planned_payments (
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
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL DEFAULT 'system_default',
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_planned_payments_user ON planned_payments(user_id, status, due_date);
CREATE TABLE IF NOT EXISTS playbook_schedules (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      bot_id TEXT NOT NULL DEFAULT 'system_default', -- 実行結果の通知に使うBot
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
CREATE INDEX IF NOT EXISTS idx_playbook_schedules_user ON playbook_schedules(user_id);
CREATE TABLE IF NOT EXISTS playbook_runs (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      schedule_id INTEGER NOT NULL,
      user_id TEXT NOT NULL,
      playbook_name TEXT NOT NULL,
      status TEXT NOT NULL DEFAULT 'running',  -- 'running' | 'success' | 'failed'
      output TEXT DEFAULT '',
      started_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      finished_at TEXT, bot_id TEXT NOT NULL DEFAULT 'system_default',
      FOREIGN KEY (schedule_id) REFERENCES playbook_schedules(id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_playbook_runs_user ON playbook_runs(user_id);
CREATE TABLE IF NOT EXISTS clipboard_entries (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      content TEXT NOT NULL,
      expires_at TEXT,                         -- NULL = 無期限
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL DEFAULT 'system_default',
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_clipboard_user ON clipboard_entries(user_id);
CREATE INDEX IF NOT EXISTS idx_clipboard_expires ON clipboard_entries(expires_at);
CREATE TABLE IF NOT EXISTS contacts (
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
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL DEFAULT 'system_default',
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_contacts_user ON contacts(user_id);
CREATE TABLE IF NOT EXISTS credentials (
      user_id TEXT NOT NULL,
      service_name TEXT NOT NULL,
      url TEXT,
      username TEXT NOT NULL,
      encrypted_password TEXT NOT NULL,
      iv TEXT NOT NULL,
      auth_tag TEXT NOT NULL,
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      PRIMARY KEY (user_id, service_name),
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE TABLE IF NOT EXISTS webhook_endpoints (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      name TEXT NOT NULL,
      token TEXT NOT NULL UNIQUE,              -- URLトークン（CSPRNG）
      secret_encrypted TEXT,                   -- HMAC検証用シークレット（暗号化保存）
      secret_iv TEXT,
      secret_tag TEXT,
      notify_target_type TEXT NOT NULL DEFAULT 'dm',
      notify_target_id TEXT,
      template TEXT,                           -- 通知テンプレート（任意）
      filter_keyword TEXT,                     -- 含まれる場合のみ通知（任意）
      create_todo INTEGER NOT NULL DEFAULT 0,
      create_reminder INTEGER NOT NULL DEFAULT 0,
      enabled INTEGER NOT NULL DEFAULT 1,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_webhook_endpoints_user ON webhook_endpoints(user_id);
CREATE TABLE IF NOT EXISTS webhook_deliveries (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      endpoint_id INTEGER NOT NULL,
      user_id TEXT NOT NULL,
      payload TEXT NOT NULL,
      status TEXT NOT NULL DEFAULT 'received', -- 'received'|'notified'|'filtered'|'failed'
      detail TEXT,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      FOREIGN KEY (endpoint_id) REFERENCES webhook_endpoints(id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_endpoint ON webhook_deliveries(endpoint_id);
CREATE TABLE IF NOT EXISTS mcp_servers (
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
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), bot_id TEXT NOT NULL DEFAULT 'system_default',
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_mcp_servers_user ON mcp_servers(user_id);
CREATE TABLE IF NOT EXISTS audit_logs (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      action TEXT NOT NULL,                    -- 例: 'credential.read', 'auth.login', 'admin.role_change'
      target TEXT,                             -- 対象（サービス名・ユーザーID等。秘密値は記録禁止）
      detail TEXT,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );
CREATE INDEX IF NOT EXISTS idx_audit_logs_user ON audit_logs(user_id, created_at);
CREATE TABLE IF NOT EXISTS invite_codes (
      code TEXT PRIMARY KEY,
      created_by TEXT,
      used_by TEXT,
      used_at TEXT,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    , revoked_at TEXT);
CREATE TABLE IF NOT EXISTS bot_context_notes (
      bot_id TEXT NOT NULL,
      user_id TEXT NOT NULL,
      content TEXT NOT NULL DEFAULT '',
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      PRIMARY KEY (bot_id, user_id),
      FOREIGN KEY (bot_id) REFERENCES bots(id) ON DELETE CASCADE
    );
CREATE TABLE IF NOT EXISTS bot_guild_notes (
      bot_id TEXT NOT NULL,
      guild_id TEXT NOT NULL,
      content TEXT NOT NULL DEFAULT '',
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      PRIMARY KEY (bot_id, guild_id),
      FOREIGN KEY (bot_id) REFERENCES bots(id) ON DELETE CASCADE
    );
CREATE TABLE IF NOT EXISTS bot_guilds (
      bot_id TEXT NOT NULL,
      guild_id TEXT NOT NULL,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      PRIMARY KEY (bot_id, guild_id),
      FOREIGN KEY (bot_id) REFERENCES bots(id) ON DELETE CASCADE
    );
CREATE TABLE IF NOT EXISTS bot_members (
      bot_id TEXT NOT NULL,
      guild_id TEXT NOT NULL,
      user_id TEXT NOT NULL,
      added_by TEXT NOT NULL,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      PRIMARY KEY (bot_id, guild_id, user_id),
      FOREIGN KEY (bot_id) REFERENCES bots(id) ON DELETE CASCADE
    );
CREATE TABLE IF NOT EXISTS "message_logs" (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        discord_msg_id TEXT,
        role TEXT NOT NULL,
        content TEXT NOT NULL,
        reply_to_msg_id TEXT,
        guild_id TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
      );
CREATE INDEX IF NOT EXISTS idx_message_logs_user ON message_logs(user_id, id);
CREATE INDEX IF NOT EXISTS idx_message_logs_discord_msg ON message_logs(discord_msg_id);
CREATE INDEX IF NOT EXISTS idx_message_logs_bot_guild ON message_logs(bot_id, guild_id, id);
CREATE TRIGGER IF NOT EXISTS message_logs_ai AFTER INSERT ON message_logs BEGIN
      INSERT INTO message_logs_fts(rowid, content) VALUES (new.id, new.content);
    END;
CREATE TRIGGER IF NOT EXISTS message_logs_ad AFTER DELETE ON message_logs BEGIN
      INSERT INTO message_logs_fts(message_logs_fts, rowid, content) VALUES ('delete', old.id, old.content);
    END;
CREATE TRIGGER IF NOT EXISTS message_logs_au AFTER UPDATE ON message_logs BEGIN
      INSERT INTO message_logs_fts(message_logs_fts, rowid, content) VALUES ('delete', old.id, old.content);
      INSERT INTO message_logs_fts(rowid, content) VALUES (new.id, new.content);
    END;
CREATE TABLE IF NOT EXISTS voice_devices (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      name TEXT NOT NULL,
      token_hash TEXT NOT NULL UNIQUE,         -- sha256Hex(token)（平文は保存しない）
      last_used_at TEXT,
      revoked_at TEXT,
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')), enabled INTEGER NOT NULL DEFAULT 1, wake_mode TEXT NOT NULL DEFAULT 'always', wake_word TEXT NOT NULL DEFAULT '', pending_command TEXT,
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_voice_devices_user ON voice_devices(user_id);
CREATE TABLE IF NOT EXISTS voice_pairings (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_code TEXT NOT NULL UNIQUE,          -- 表示用の短いコード（例: WXYZ-4821）
      device_code_hash TEXT NOT NULL UNIQUE,   -- sha256Hex(device_code)（平文は保存しない）
      status TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'approved'
      user_id TEXT,                            -- 承認したユーザー（承認時に設定）
      device_name TEXT,                        -- 承認時に設定
      token_encrypted TEXT,                    -- 承認時に発行したトークン（暗号化・受信後にNULL）
      token_iv TEXT,
      token_tag TEXT,
      expires_at TEXT NOT NULL,                -- 'YYYY-MM-DD HH:MM:SS'（localtime）
      created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );
CREATE INDEX IF NOT EXISTS idx_voice_pairings_user_code ON voice_pairings(user_code);
CREATE TABLE IF NOT EXISTS "budget_limits" (
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        category TEXT NOT NULL,
        limit_amount INTEGER NOT NULL DEFAULT 50000,
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        PRIMARY KEY (user_id, bot_id, category),
        FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
      );
CREATE TABLE IF NOT EXISTS "context_notes" (
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        content TEXT NOT NULL DEFAULT '',
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        PRIMARY KEY (user_id, bot_id),
        FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
      );
CREATE TABLE IF NOT EXISTS "briefing_configs" (
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
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
CREATE TABLE IF NOT EXISTS "report_configs" (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        type TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 0,
        schedule_cron TEXT NOT NULL DEFAULT '0 21 * * *',
        target_type TEXT NOT NULL DEFAULT 'dm',
        target_id TEXT,
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        UNIQUE(user_id, bot_id, type),
        FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
      );
CREATE TABLE IF NOT EXISTS "playbooks" (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
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
CREATE INDEX IF NOT EXISTS idx_playbooks_user ON playbooks(user_id);
CREATE INDEX IF NOT EXISTS idx_mcp_servers_user_bot ON mcp_servers(user_id, bot_id);
CREATE TABLE IF NOT EXISTS bot_credential_access (
      bot_id       TEXT NOT NULL,
      owner_id     TEXT NOT NULL,
      service_name TEXT NOT NULL,
      created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime')),
      PRIMARY KEY (bot_id, owner_id, service_name),
      FOREIGN KEY (bot_id)   REFERENCES bots(id)          ON DELETE CASCADE,
      FOREIGN KEY (owner_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_bot_cred_access_bot ON bot_credential_access(bot_id);
CREATE TABLE IF NOT EXISTS user_google_accounts (
      id                      INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id                 TEXT NOT NULL,
      email                   TEXT,
      refresh_token_encrypted TEXT NOT NULL,
      refresh_token_iv        TEXT NOT NULL,
      refresh_token_tag       TEXT NOT NULL,
      calendar_id             TEXT,
      calendars               TEXT NOT NULL DEFAULT '[]',
      is_primary              INTEGER NOT NULL DEFAULT 0,
      created_at              TEXT NOT NULL DEFAULT (datetime('now','localtime')),
      updated_at              TEXT NOT NULL DEFAULT (datetime('now','localtime')),
      UNIQUE(user_id, email),
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_user_google_accounts_user ON user_google_accounts(user_id);
CREATE TABLE IF NOT EXISTS "bot_google_account" (
        bot_id            TEXT    NOT NULL PRIMARY KEY,
        google_account_id INTEGER,
        created_at        TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
        FOREIGN KEY (bot_id)            REFERENCES bots(id)                  ON DELETE CASCADE,
        FOREIGN KEY (google_account_id) REFERENCES user_google_accounts(id) ON DELETE CASCADE
      );
CREATE TABLE IF NOT EXISTS "bot_mcp_access" (
        bot_id        TEXT    NOT NULL,
        owner_id      TEXT    NOT NULL,
        mcp_server_id INTEGER NOT NULL,
        created_at    TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
        PRIMARY KEY (bot_id, owner_id, mcp_server_id),
        FOREIGN KEY (bot_id)        REFERENCES bots(id)          ON DELETE CASCADE,
        FOREIGN KEY (owner_id)      REFERENCES users(discord_id) ON DELETE CASCADE,
        FOREIGN KEY (mcp_server_id) REFERENCES mcp_servers(id)   ON DELETE CASCADE
      );
CREATE INDEX IF NOT EXISTS idx_bot_mcp_access_server     ON bot_mcp_access(mcp_server_id);
CREATE INDEX IF NOT EXISTS idx_bot_mcp_access_bot_owner  ON bot_mcp_access(bot_id, owner_id);
CREATE TABLE IF NOT EXISTS bot_active_personas (
      user_id TEXT NOT NULL,
      bot_id TEXT NOT NULL DEFAULT 'system_default',
      persona_id INTEGER NOT NULL,
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      PRIMARY KEY (user_id, bot_id),
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE,
      FOREIGN KEY (persona_id) REFERENCES personas(id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_bot_active_personas_persona ON bot_active_personas(persona_id);
CREATE TABLE IF NOT EXISTS tool_outcomes (
      id          INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id     TEXT    NOT NULL,                 -- データ分離キー（必須）
      bot_id      TEXT    NOT NULL,
      guild_id    TEXT,                             -- NULL=DM/秘書、非NULL=汎用モードのギルド
      topic_id    TEXT,                             -- 連想結合キー（R2でシナプスから付与。R1ではNULL可）
      synapse_id  INTEGER,                          -- 任意: 関連シナプス（synapses.id）
      tool_name   TEXT    NOT NULL,
      args_digest TEXT,                             -- 引数要約（認証情報・秘匿系は除外。§6.3.2 継承）
      status      TEXT    NOT NULL,                 -- 'success' | 'error'
      latency_ms  INTEGER,
      created_at  TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
    );
CREATE INDEX IF NOT EXISTS idx_tool_outcomes_user  ON tool_outcomes(user_id, bot_id, tool_name);
CREATE INDEX IF NOT EXISTS idx_tool_outcomes_topic ON tool_outcomes(user_id, bot_id, topic_id);
CREATE TABLE IF NOT EXISTS topic_tool_stats (
      user_id      TEXT    NOT NULL,
      bot_id       TEXT    NOT NULL,
      topic_id     TEXT    NOT NULL,
      tool_name    TEXT    NOT NULL,
      success      INTEGER NOT NULL DEFAULT 0,
      total        INTEGER NOT NULL DEFAULT 0,
      success_rate REAL    NOT NULL DEFAULT 0,      -- success / total（事前計算）
      last_updated TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
      PRIMARY KEY (user_id, bot_id, topic_id, tool_name)
    );
CREATE TABLE IF NOT EXISTS synapses (
      id                      INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id                 TEXT    NOT NULL,     -- データ分離キー（必須）
      bot_id                  TEXT    NOT NULL,
      guild_id                TEXT,                 -- NULL=DM/秘書、非NULL=汎用モードのギルド
      content                 TEXT    NOT NULL,     -- 意味の最小単位（好み・制約・前提・事実）
      topic_id                TEXT,                 -- トピック（疑似グラフの結合キー）
      source_msg_id           INTEGER,              -- 抽出元 message_logs.id（任意）
      embedding               BLOB,                 -- float32 little-endian の連結。NULL=未埋め込み
      embedding_model_version TEXT,                 -- 埋め込みモデル世代（不一致は再埋め込み対象。v3 §7）
      created_at              TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
      last_used_at            TEXT,                 -- 想起されるたび更新（鮮度）
      use_count               INTEGER NOT NULL DEFAULT 0,
      decay_score             REAL    NOT NULL DEFAULT 1.0  -- 想起の強度/減衰（退避の駆動値）
    , ctx_tod INTEGER, ctx_dow INTEGER);
CREATE INDEX IF NOT EXISTS idx_synapses_user  ON synapses(user_id, bot_id);
CREATE INDEX IF NOT EXISTS idx_synapses_topic ON synapses(user_id, bot_id, topic_id);
CREATE INDEX IF NOT EXISTS idx_todos_parent ON todos(parent_id);
CREATE TABLE IF NOT EXISTS task_progress_logs (
      id         INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id    TEXT    NOT NULL,                 -- データ分離キー（必須）
      bot_id     TEXT    NOT NULL DEFAULT 'system_default',
      todo_id    INTEGER NOT NULL,
      progress   INTEGER NOT NULL,                 -- この時点の進捗 0-100
      note       TEXT,                             -- 進捗メモ（任意。例: 「設計が完了した」）
      created_at TEXT    NOT NULL DEFAULT (datetime('now', 'localtime')),
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE,
      FOREIGN KEY (todo_id) REFERENCES todos(id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_task_progress_logs_todo
      ON task_progress_logs(todo_id, created_at);
CREATE TABLE IF NOT EXISTS desktop_tokens (
      id           INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id      TEXT NOT NULL,                 -- Discord ユーザーID（データ分離キー）
      token_hash   TEXT NOT NULL UNIQUE,          -- sha256(生トークン)
      device_name  TEXT,
      created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime')),
      last_used_at TEXT,
      revoked      INTEGER NOT NULL DEFAULT 0,
      FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_desktop_tokens_user ON desktop_tokens(user_id);
CREATE TABLE IF NOT EXISTS bot_member_requests (
      id          INTEGER PRIMARY KEY AUTOINCREMENT,
      bot_id      TEXT NOT NULL,
      guild_id    TEXT NOT NULL,
      user_id     TEXT NOT NULL,                  -- 申請者の Discord ユーザーID
      status      TEXT NOT NULL DEFAULT 'pending',-- pending / approved / rejected
      note        TEXT,                           -- 申請メッセージ（任意）
      decided_by  TEXT,                           -- 承認/却下を行ったユーザー（owner/Admin）
      created_at  TEXT NOT NULL DEFAULT (datetime('now','localtime')),
      updated_at  TEXT NOT NULL DEFAULT (datetime('now','localtime')),
      UNIQUE (bot_id, guild_id, user_id),
      FOREIGN KEY (bot_id) REFERENCES bots(id) ON DELETE CASCADE
    );
CREATE INDEX IF NOT EXISTS idx_member_requests_bot_status
      ON bot_member_requests(bot_id, status);
CREATE INDEX IF NOT EXISTS idx_member_requests_user
      ON bot_member_requests(user_id);
CREATE TABLE IF NOT EXISTS bot_roles (
      bot_id     TEXT NOT NULL,
      guild_id   TEXT NOT NULL,
      role_id    TEXT NOT NULL,
      role_name  TEXT,                            -- 表示用キャッシュ（任意）
      added_by   TEXT NOT NULL,
      created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
      PRIMARY KEY (bot_id, guild_id, role_id),
      FOREIGN KEY (bot_id) REFERENCES bots(id) ON DELETE CASCADE
    );
CREATE TABLE IF NOT EXISTS bot_user_modules (
      bot_id TEXT NOT NULL,
      user_id TEXT NOT NULL,
      enabled_modules TEXT NOT NULL,
      updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
      PRIMARY KEY (bot_id, user_id),
      FOREIGN KEY (bot_id) REFERENCES bots(id) ON DELETE CASCADE
    );
CREATE TABLE IF NOT EXISTS day_plan_blocks (
      id           INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id      TEXT NOT NULL,
      bot_id       TEXT NOT NULL,
      date         TEXT NOT NULL,                       -- 'YYYY-MM-DD'
      start_time   TEXT,                                -- 'HH:MM'（null=時間未定）
      end_time     TEXT,                                -- 'HH:MM'（null=終了未定）
      type         TEXT NOT NULL DEFAULT 'event',       -- 'task'|'transit'|'event'|'free'
      title        TEXT NOT NULL,
      description  TEXT,
      todo_id      INTEGER,                             -- → todos.id（ON DELETE SET NULL）
      transit_from TEXT,
      transit_to   TEXT,
      transit_line TEXT,
      position     INTEGER NOT NULL DEFAULT 0,
      created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime')),
      updated_at   TEXT NOT NULL DEFAULT (datetime('now','localtime'))
    );
CREATE INDEX IF NOT EXISTS idx_day_plan_blocks_user_date
      ON day_plan_blocks(user_id, bot_id, date);
CREATE TABLE IF NOT EXISTS timeline_records (
      id               INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id          TEXT NOT NULL,
      bot_id           TEXT NOT NULL,
      date             TEXT NOT NULL,                   -- 'YYYY-MM-DD'
      recorded_at      TEXT NOT NULL DEFAULT (datetime('now','localtime')),
      type             TEXT NOT NULL DEFAULT 'memo',    -- 'memo'|'expense'|'task_done'|'media'|'location'
      title            TEXT,
      content          TEXT,
      todo_id          INTEGER,                         -- → todos.id
      expense_id       INTEGER,                         -- → expenses.id
      amount           REAL,
      expense_category TEXT,
      media_path       TEXT,                            -- ファイル名のみ（data/media/ 以下）
      media_type       TEXT,                            -- 'photo'|'video'
      location         TEXT,
      created_at       TEXT NOT NULL DEFAULT (datetime('now','localtime'))
    );
CREATE INDEX IF NOT EXISTS idx_timeline_records_user_date
      ON timeline_records(user_id, bot_id, date);
-- スキーマバージョンを刻印する（Node `migrations.ts:1574-1578` parity・移行期の必須ガード）。
-- これが無いと、Rust が新規作成した DB を Node が開いたとき getCurrentSchemaVersion() が
-- 既定の "1" を返し、DROP 条件（version!="17" && legacy テーブル在 && version=="1"）が成立して
-- users/bots/tasks/schedules/expenses/credentials/playbooks 等のコアテーブルを全 DROP する
-- （＝データ全喪失）。既存 DB では既に '17' が刻まれているため upsert で冪等に一致させる。
INSERT INTO system_settings (key, value) VALUES ('schema_version', '17')
  ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = datetime('now', 'localtime');
