-- R13: original bytes and provenance commit atomically with the timeline record.
-- Legacy disk files remain readable. No V27/V28 changes are included in this PR.
CREATE TABLE timeline_media_assets (
    record_id INTEGER PRIMARY KEY REFERENCES timeline_records(id) ON DELETE CASCADE,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    bytes BLOB NOT NULL CHECK(length(bytes) > 0),
    user_id TEXT NOT NULL,
    bot_id TEXT NOT NULL,
    source_message_id TEXT,
    source_attachment_id TEXT,
    save_key TEXT NOT NULL DEFAULT '',
    position INTEGER NOT NULL DEFAULT 0 CHECK(position >= 0),
    CHECK ((source_message_id IS NULL) = (source_attachment_id IS NULL)),
    UNIQUE(user_id, bot_id, source_message_id, source_attachment_id, save_key)
);
CREATE INDEX idx_timeline_media_assets_filename ON timeline_media_assets(filename);
CREATE INDEX idx_timeline_media_assets_scope ON timeline_media_assets(user_id, bot_id);

-- timeline_records has no user/Bot FK in the baseline. Limit cleanup to R13 assets.
CREATE TRIGGER timeline_media_user_delete AFTER DELETE ON users BEGIN
    DELETE FROM timeline_records WHERE id IN (SELECT record_id FROM timeline_media_assets WHERE user_id=OLD.discord_id);
END;
CREATE TRIGGER timeline_media_bot_delete AFTER DELETE ON bots BEGIN
    DELETE FROM timeline_records WHERE id IN (SELECT record_id FROM timeline_media_assets WHERE bot_id=OLD.id);
END;
