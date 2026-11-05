CREATE TABLE IF NOT EXISTS sync_sets (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL,
    serial TEXT NOT NULL,
    captured_at TEXT NOT NULL,
    timestamp_us INTEGER NOT NULL,
    stereo INTEGER NOT NULL,
    minute_start_us INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_channels (
    set_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    media_type TEXT NOT NULL,
    payload BLOB NOT NULL,
    payload_length INTEGER NOT NULL,
    payload_ready INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (set_id, channel_id),
    FOREIGN KEY (set_id) REFERENCES sync_sets(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS sync_sets_minute
    ON sync_sets (minute_start_us, id);
