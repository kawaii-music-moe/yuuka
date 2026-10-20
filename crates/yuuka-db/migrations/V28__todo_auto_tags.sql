-- tags remains the public, merged JSON array. Provenance is internal and never sent to clients.
CREATE TABLE todo_tagging (
    todo_id INTEGER PRIMARY KEY REFERENCES todos(id) ON DELETE CASCADE,
    explicit_tags TEXT NOT NULL DEFAULT '[]',
    auto_tags TEXT NOT NULL DEFAULT '[]',
    disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
    revision INTEGER NOT NULL DEFAULT 0,
    pending INTEGER NOT NULL DEFAULT 0 CHECK (pending IN (0, 1)),
    lease_until TEXT,
    applying INTEGER NOT NULL DEFAULT 0 CHECK (applying IN (0, 1))
);
-- Existing tags are protected; no retrospective LLM calls, including for untagged todos.
INSERT INTO todo_tagging (todo_id, explicit_tags) SELECT id, tags FROM todos;
CREATE INDEX idx_todo_tagging_pending ON todo_tagging(pending, lease_until, todo_id);

-- Captures every insert, including finance's transactional INSERT and webhook's fixed tags.
CREATE TRIGGER todo_tagging_insert AFTER INSERT ON todos BEGIN
    INSERT INTO todo_tagging (todo_id, explicit_tags, pending) VALUES (NEW.id, NEW.tags, 1);
END;
CREATE TRIGGER todo_tagging_content AFTER UPDATE OF title, description ON todos
WHEN NEW.title IS NOT OLD.title OR NEW.description IS NOT OLD.description BEGIN
    UPDATE todo_tagging SET revision = revision + 1, pending = 1 - disabled, lease_until = NULL
    WHERE todo_id = NEW.id;
END;
-- Any explicit tag write (even [] -> []) cancels an in-flight generation. Nonempty manual
-- selections become protected explicit tags; an empty selection persistently disables automation.
CREATE TRIGGER todo_tagging_manual AFTER UPDATE OF tags ON todos
WHEN (SELECT applying FROM todo_tagging WHERE todo_id = NEW.id) = 0 BEGIN
    UPDATE todo_tagging SET explicit_tags = NEW.tags, auto_tags = '[]',
        disabled = CASE WHEN json_array_length(NEW.tags) = 0 THEN 1 ELSE 0 END,
        revision = revision + 1, pending = 0, lease_until = NULL
    WHERE todo_id = NEW.id;
END;
-- Also works for connections without FK enforcement (normal production writer enables it).
CREATE TRIGGER todo_tagging_delete AFTER DELETE ON todos BEGIN
    DELETE FROM todo_tagging WHERE todo_id = OLD.id;
END;
