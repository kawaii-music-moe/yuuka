ALTER TABLE bots ADD COLUMN name_linked_to_discord INTEGER NOT NULL DEFAULT 0 CHECK (name_linked_to_discord IN (0, 1));
