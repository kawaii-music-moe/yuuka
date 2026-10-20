// Lightweight schema regression; no Rust build/test execution.
// Optional arguments: paths to the reviewed V27 and V28 SQL (never copied into this branch).
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';

const root = new URL('../crates/yuuka-db/migrations/', import.meta.url);
const preceding = process.argv.slice(2);
assert.ok(preceding.length === 0 || preceding.length === 2, 'supply both V27 and V28, or neither');
const db = new DatabaseSync(':memory:');
db.exec('PRAGMA foreign_keys=ON');
for (const name of readdirSync(root).filter(n => /^V\d+__/.test(n) && Number(n.match(/^V(\d+)/)[1]) < 29)
  .sort((a,b) => Number(a.match(/^V(\d+)/)[1])-Number(b.match(/^V(\d+)/)[1]))) {
  // Node SQLite reserves FTS shadow names. The virtual table creates these itself;
  // omit only the four redundant shadow-table CREATEs in the unchanged V17 dump.
  const sql = readFileSync(new URL(name, root), 'utf8').replace(/^CREATE TABLE IF NOT EXISTS 'message_logs_fts_.*;\n/gm, '');
  db.exec(sql);
}
db.exec(`INSERT INTO users(discord_id,username,salt,sk_account_id) VALUES ('u','U','x','sk-u'),('other','Other','x','sk-other');
  INSERT INTO bots(id,user_id,name) VALUES ('b','u','B'),('b2','u','B2'),('foreign','other','Foreign');
  INSERT INTO timeline_records(user_id,bot_id,date,type,title,media_path,media_type)
    VALUES ('u','b','2026-10-20','media','legacy','legacy.png','photo');`);
const legacy = db.prepare('SELECT * FROM timeline_records').all();
if (preceding.length) {
  // PR #188's V27 runner requires FK=OFF outside its transaction.
  db.exec('PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE');
  db.exec(readFileSync(preceding[0], 'utf8'));
  db.exec('COMMIT; PRAGMA foreign_keys=ON');
  db.exec(readFileSync(preceding[1], 'utf8'));
}
db.exec(readFileSync(new URL('V29__timeline_conversation_attachments.sql', root), 'utf8'));
assert.deepEqual(db.prepare('SELECT * FROM timeline_records').all(), legacy, 'upgrade preserves legacy metadata');
const insertRecord = db.prepare(`INSERT INTO timeline_records(user_id,bot_id,date,type,media_path,media_type)
  VALUES (?,?, '2026-10-20','media',?,'photo') RETURNING id`);
const insertAsset = db.prepare(`INSERT INTO timeline_media_assets(record_id,filename,mime_type,bytes,user_id,bot_id,source_message_id,source_attachment_id,save_key,position)
  VALUES (?,?, 'image/png',?,?,?,?,?,?,?)`);
function insert(uid,bot,source,attachment,key,position,bytes=Buffer.from([1,2,3])) {
  db.exec("SAVEPOINT insert_media");
  try {
  const filename = `${uid}-${bot}-${source}-${attachment}-${key}.png`;
  const {id} = insertRecord.get(uid,bot,filename);
  insertAsset.run(id,filename,bytes,uid,bot,source,attachment,key,position);
  db.exec("RELEASE insert_media");
  return id;
  } catch (error) { db.exec("ROLLBACK TO insert_media; RELEASE insert_media"); throw error; }
}
const first = insert('u','b','discord:42','0','',0);
assert.throws(() => insert('u','b','discord:42','0','',0), /UNIQUE/);
insert('u','b','discord:42','0','explicit-resave',0);
insert('u','b2','discord:42','0','',0);
insert('other','foreign','discord:42','0','',0);
const before = db.prepare('SELECT count(*) AS n FROM timeline_records').get().n;
db.exec('BEGIN IMMEDIATE');
try { insert('u','b','batch','1','',1); insert('u','b','batch','2','',2,Buffer.alloc(0)); assert.fail('empty bytes must fail'); }
catch (e) { assert.match(e.message,/CHECK/); db.exec('ROLLBACK'); }
assert.equal(db.prepare('SELECT count(*) AS n FROM timeline_records').get().n,before,'failed batch leaves no partial record');
assert.equal(db.prepare("SELECT count(*) AS n FROM timeline_media_assets WHERE source_message_id='batch'").get().n,0);
assert.deepEqual([...db.prepare('SELECT bytes FROM timeline_media_assets WHERE record_id=?').get(first).bytes],[1,2,3]);
db.prepare('DELETE FROM timeline_records WHERE id=?').run(first);
assert.equal(db.prepare('SELECT count(*) AS n FROM timeline_media_assets WHERE record_id=?').get(first).n,0,'record deletion removes bytes and replay key');
db.exec("DELETE FROM bots WHERE id='b2'");
assert.equal(db.prepare("SELECT count(*) AS n FROM timeline_media_assets WHERE bot_id='b2'").get().n,0,'Bot deletion removes bytes');
db.exec("DELETE FROM users WHERE discord_id='other'");
assert.equal(db.prepare("SELECT count(*) AS n FROM timeline_media_assets WHERE user_id='other'").get().n,0,'account deletion removes bytes');
assert.deepEqual(db.prepare('PRAGMA foreign_key_check').all(),[]);
console.log(`R13 SQLite ${db.prepare('SELECT sqlite_version() AS v').get().v}: legacy preservation, provenance uniqueness/scope/resave, bytes, batch rollback and deletion PASS (${preceding.length ? 'V27 → V28 → V29' : 'V26 → V29'})`);
