CREATE TABLE IF NOT EXISTS frame_set_schema (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS frame_sets (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL,
    captured_at TEXT NOT NULL,
    captured_at_us INTEGER NOT NULL,
    attributes TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS frame_channels (
    frame_set_id TEXT NOT NULL REFERENCES frame_sets(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    channel_id TEXT NOT NULL,
    role TEXT NOT NULL,
    media_type TEXT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    bit_depth INTEGER NOT NULL,
    device_timestamp_us INTEGER NOT NULL,
    frame_number INTEGER NOT NULL,
    depth_json TEXT,
    calibration_json TEXT NOT NULL,
    attributes TEXT NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY (frame_set_id, channel_id)
);

CREATE INDEX IF NOT EXISTS frame_sets_order
    ON frame_sets (source_id, captured_at_us, id);

CREATE TABLE IF NOT EXISTS frame_set_counter (
    next_id INTEGER NOT NULL
);
