CREATE TABLE IF NOT EXISTS dvr_store_meta (
    meta_key TEXT PRIMARY KEY,
    meta_value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS dvr_h264_epochs (
    epoch_id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id TEXT NOT NULL,
    codec_init BLOB NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    opened_at_us INTEGER NOT NULL,
    UNIQUE (source_id, codec_init, width, height)
);

CREATE INDEX IF NOT EXISTS dvr_h264_epochs_source
    ON dvr_h264_epochs (source_id, epoch_id);
