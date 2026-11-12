-- 会話記憶（synapses）の評価と選別（docs/design/memory_retention.md）。
-- 抽出した発話は候補（status='candidate'）として入り、評価器が短い事実文に書き直して kept にする。
-- 既存行はそのまま kept・strength=1.0 で残す（消さない・再評価しない）。
-- strength は想起と有用で強まり毎日減衰する「強度」。decay_score は読まなくなるが列は残す。
ALTER TABLE synapses ADD COLUMN status TEXT NOT NULL DEFAULT 'kept';
ALTER TABLE synapses ADD COLUMN kind TEXT;
ALTER TABLE synapses ADD COLUMN durability TEXT NOT NULL DEFAULT 'medium';
ALTER TABLE synapses ADD COLUMN strength REAL NOT NULL DEFAULT 1.0;
ALTER TABLE synapses ADD COLUMN useful_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE synapses ADD COLUMN last_useful_at TEXT;
ALTER TABLE synapses ADD COLUMN evaluated_at TEXT;
ALTER TABLE synapses ADD COLUMN content_hash TEXT;
CREATE INDEX idx_synapses_status ON synapses(user_id, bot_id, status);
CREATE INDEX idx_synapses_hash ON synapses(user_id, bot_id, content_hash);
