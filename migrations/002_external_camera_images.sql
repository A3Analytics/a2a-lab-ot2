CREATE TABLE IF NOT EXISTS image_history (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL,
    captured_at TEXT NOT NULL,
    captured_at_us INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    caption TEXT,
    attributes TEXT NOT NULL,
    data BLOB NOT NULL
);

CREATE INDEX IF NOT EXISTS image_history_order
    ON image_history (source_id, captured_at_us, id);
