#!/usr/bin/env python3
"""SD07 SQL regression checks, without building Rust. Run from any directory.

Requires SQLite with FTS5/trigram and DROP COLUMN (e.g. SQLite 3.46/3.51).
Rust/refinery runner coverage lives in yuuka-db/src/schema.rs tests.
"""
import pathlib
import re
import sqlite3
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
MIGRATIONS = ROOT / 'crates/yuuka-db/migrations'
TABLES = '''todos schedules reminders expenses planned_payments playbook_schedules
playbook_runs clipboard_entries contacts mcp_servers message_logs budget_limits
context_notes briefing_configs report_configs playbooks bot_active_personas
 task_progress_logs'''.split()
V27 = (MIGRATIONS / 'V27__remove_bot_id_defaults.sql').read_text()


def before():
    db = sqlite3.connect(':memory:')
    db.execute('PRAGMA foreign_keys=ON')
    for path in sorted(MIGRATIONS.glob('V*.sql'), key=lambda p: int(p.name.split('__')[0][1:])):
        if int(path.name.split('__')[0][1:]) < 27:
            db.executescript(path.read_text())
    return db


def upgrade(db):
    # execute_batch in refinery is enclosed by the driver's transaction. Unlike
    # execute(), Python executescript commits first, so include BEGIN explicitly.
    db.execute('PRAGMA foreign_keys=OFF')
    try:
        db.executescript('BEGIN IMMEDIATE;\n' + V27 + '\nCOMMIT;')
    except sqlite3.Error:
        db.rollback()
        raise
    finally:
        db.execute('PRAGMA foreign_keys=ON')


def columns(db, table):
    return db.execute(f'PRAGMA table_info({table})').fetchall()


def rows(db, table):
    return db.execute(f'SELECT * FROM {table} ORDER BY rowid').fetchall()


def schema(db):
    return dict(db.execute("SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL"))


def seed(db):
    db.executescript("""
    INSERT INTO users(discord_id,username,salt,sk_account_id) VALUES('u','u','salt','sk-u');
    INSERT INTO bots(id,user_id,name) VALUES('real-bot','u','bot');
    INSERT INTO personas(id,owner_id,name) VALUES(1,'u','persona');
    """)
    for table in TABLES:
        info = columns(db, table)
        for k in (1, 2):
            values = []
            for _, name, kind, required, default, pk in info:
                if name in ('id', 'todo_id', 'schedule_id'):
                    value = k
                elif name == 'persona_id':
                    value = 1
                elif name == 'user_id':
                    value = None if table == 'mcp_servers' and k == 2 else 'u'
                elif name == 'bot_id':
                    value = 'system_default' if k == 1 else 'real-bot'
                elif kind in ('INTEGER', 'REAL'):
                    value = k
                else:
                    value = None if not required and not pk and k == 2 else f'{table}-{name}-{k}'
                values.append(value)
            names = ','.join('"' + c[1] + '"' for c in info)
            db.execute(f'INSERT INTO {table} ({names}) VALUES ({",".join("?" for _ in info)})', values)
    db.executescript("""
    INSERT INTO message_attachments(message_log_id,name,mime_type,bytes)
      VALUES(1,'test.bin','application/octet-stream',X'0001FF');
    INSERT INTO bot_mcp_access(bot_id,owner_id,mcp_server_id) VALUES('real-bot','u',1);
    UPDATE sqlite_sequence SET seq=500 WHERE name IN
      ('todos','schedules','reminders','expenses','planned_payments','playbook_schedules',
       'playbook_runs','clipboard_entries','contacts','mcp_servers','message_logs','report_configs','playbooks','task_progress_logs');
    """)
    assert not db.execute('PRAGMA foreign_key_check').fetchall()


class SD07(unittest.TestCase):
    def assert_structure(self, db, old):
        current = schema(db)
        expected = dict(old)
        del expected['idx_mcp_servers_user_bot']
        for table in TABLES:
            sql = expected[table]
            if table == 'mcp_servers':
                sql = sql.replace(", bot_id TEXT NOT NULL DEFAULT 'system_default'", '')
            else:
                sql = sql.replace(" DEFAULT 'system_default'", '')
            expected[table] = sql
        normalize = lambda s: re.sub(r'CREATE TABLE "(\w+)"', r'CREATE TABLE \1', s)
        self.assertEqual({k: normalize(v) for k, v in current.items()},
                         {k: normalize(v) for k, v in expected.items()})
        self.assertFalse(db.execute('PRAGMA foreign_key_check').fetchall())
        self.assertEqual(db.execute('PRAGMA integrity_check').fetchone()[0], 'ok')
        self.assertEqual(db.execute('PRAGMA foreign_keys').fetchone()[0], 1)
        self.assertFalse(db.execute("SELECT name FROM sqlite_temp_schema WHERE name LIKE 'sd07_%'").fetchall())
        self.assertFalse(db.execute("SELECT name FROM sqlite_schema WHERE name LIKE 'sd07_%'").fetchall())

    def test_fresh(self):
        db = before()
        old = schema(db)
        upgrade(db)
        self.assert_structure(db, old)
        for table in TABLES:
            bot = [c for c in columns(db, table) if c[1] == 'bot_id']
            if table == 'mcp_servers':
                self.assertEqual(bot, [])
            else:
                self.assertEqual((bot[0][3], bot[0][4]), (1, None))

    def test_upgrade_data_constraints_sequences_and_fts(self):
        db = before()
        seed(db)
        old_schema = schema(db)
        old_rows = {t: rows(db, t) for t in TABLES + ['message_attachments', 'bot_mcp_access']}
        old_cols = {t: columns(db, t) for t in TABLES}
        old_seq = rows(db, 'sqlite_sequence')
        old_fts = db.execute("SELECT rowid FROM message_logs_fts WHERE message_logs_fts MATCH 'content' ORDER BY rowid").fetchall()
        upgrade(db)
        self.assert_structure(db, old_schema)
        for table, data in old_rows.items():
            if table == 'mcp_servers':
                idx = next(i for i, c in enumerate(old_cols[table]) if c[1] == 'bot_id')
                data = [r[:idx] + r[idx+1:] for r in data]
            self.assertEqual(rows(db, table), data, table)
        self.assertEqual(sorted(rows(db, 'sqlite_sequence')), sorted(old_seq))
        self.assertEqual(db.execute("SELECT rowid FROM message_logs_fts WHERE message_logs_fts MATCH 'content' ORDER BY rowid").fetchall(), old_fts)
        for table in TABLES:
            if table == 'mcp_servers':
                continue
            info = columns(db, table)
            # Omit bot_id while satisfying all other required columns. The error
            # must specifically name bot_id, rather than another NOT NULL/FK.
            names = [c[1] for c in info if c[1] not in ('id', 'bot_id')]
            vals = [db.execute(f'SELECT "{n}" FROM {table} LIMIT 1').fetchone()[0] for n in names]
            sql = f'INSERT INTO {table} ({",".join(names)}) VALUES ({",".join("?" for _ in names)})'
            with self.assertRaisesRegex(sqlite3.IntegrityError, f'NOT NULL constraint failed: {table}.bot_id'):
                db.execute(sql, vals)
            # Explicit NULL must also fail.
            with self.assertRaisesRegex(sqlite3.IntegrityError, f'NOT NULL constraint failed: {table}.bot_id'):
                db.execute(f'UPDATE {table} SET bot_id=NULL')
        db.execute("INSERT INTO users(discord_id,username,salt) VALUES('u2','u2','salt')")
        db.execute("INSERT INTO bots(id,user_id,name) VALUES('second-real-bot','u2','bot')")
        for table in TABLES:
            if table == 'mcp_servers':
                continue
            names = [c[1] for c in columns(db, table) if c[1] != 'id']
            vals = [db.execute(f'SELECT "{n}" FROM {table} LIMIT 1').fetchone()[0] for n in names]
            vals[names.index('bot_id')] = 'second-real-bot'
            vals[names.index('user_id')] = 'u2'
            db.execute(f'INSERT INTO {table} ({",".join(names)}) VALUES ({",".join("?" for _ in names)})', vals)
        # User and system MCP registration no longer need any Bot identity.
        db.execute("INSERT INTO mcp_servers(user_id,name,endpoint_url) VALUES(NULL,'system','https://system')")
        db.execute("INSERT INTO mcp_servers(user_id,name,endpoint_url) VALUES('u','user','https://user')")
        self.assertEqual(db.execute("SELECT max(id) FROM mcp_servers").fetchone()[0], 502)
        db.execute("INSERT INTO message_logs(user_id,bot_id,role,content) VALUES('u','real-bot','user','freshneedle')")
        self.assertEqual(db.execute("SELECT rowid FROM message_logs_fts WHERE message_logs_fts MATCH 'freshneedle'").fetchall(), [(502,)])
        db.execute("UPDATE message_logs SET content='updatedneedle' WHERE id=502")
        self.assertFalse(db.execute("SELECT rowid FROM message_logs_fts WHERE message_logs_fts MATCH 'freshneedle'").fetchall())
        self.assertEqual(db.execute("SELECT rowid FROM message_logs_fts WHERE message_logs_fts MATCH 'updatedneedle'").fetchall(), [(502,)])
        db.execute('DELETE FROM message_logs WHERE id=502')
        self.assertFalse(db.execute("SELECT rowid FROM message_logs_fts WHERE message_logs_fts MATCH 'updatedneedle'").fetchall())
        # Incoming CASCADEs survived all four parent rebuilds.
        for parent, child, key in [('todos','task_progress_logs','todo_id'),
                                    ('playbook_schedules','playbook_runs','schedule_id'),
                                    ('mcp_servers','bot_mcp_access','mcp_server_id'),
                                    ('message_logs','message_attachments','message_log_id')]:
            db.execute(f'DELETE FROM {parent} WHERE id=1')
            self.assertFalse(db.execute(f'SELECT * FROM {child} WHERE {key}=1').fetchall())
        self.assertFalse(db.execute('PRAGMA foreign_key_check').fetchall())
        # Recreated UNIQUE constraints still reject duplicate composite keys.
        with self.assertRaises(sqlite3.IntegrityError):
            db.execute("INSERT INTO context_notes(user_id,bot_id) VALUES('u','real-bot')")

    def test_failure_rolls_back_and_retry_succeeds(self):
        db = before()
        seed(db)
        db.execute('PRAGMA foreign_keys=OFF')
        db.execute("INSERT INTO task_progress_logs(user_id,bot_id,todo_id,progress) VALUES('u','real-bot',999,0)")
        db.commit()
        old = schema(db)
        data = {t: rows(db, t) for t in TABLES}
        with self.assertRaises(sqlite3.IntegrityError):
            upgrade(db)
        self.assertEqual(schema(db), old)
        self.assertEqual({t: rows(db, t) for t in TABLES}, data)
        self.assertEqual(db.execute('PRAGMA foreign_keys').fetchone()[0], 1)
        db.execute('DELETE FROM task_progress_logs WHERE todo_id=999')
        db.commit()
        upgrade(db)
        self.assert_structure(db, old)

    def test_raw_sql_refuses_foreign_keys_on(self):
        db = before()
        old = schema(db)
        with self.assertRaises(sqlite3.IntegrityError):
            db.executescript('BEGIN;\n' + V27 + '\nCOMMIT;')
        db.rollback()
        self.assertEqual(schema(db), old)


if __name__ == '__main__':
    print('SQLite', sqlite3.sqlite_version)
    unittest.main(verbosity=2)
