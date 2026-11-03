CREATE TABLE IF NOT EXISTS dvr_tracks (
    source_id TEXT NOT NULL,
    track_id TEXT NOT NULL,
    codec TEXT NOT NULL,
    codec_init BLOB NOT NULL DEFAULT X'',
    frame_period_us INTEGER NOT NULL,
    PRIMARY KEY (source_id, track_id)
);

CREATE TABLE IF NOT EXISTS dvr_segments (
    source_id TEXT NOT NULL,
    track_id TEXT NOT NULL,
    start_us INTEGER NOT NULL,
    end_us INTEGER NOT NULL,
    keyframe INTEGER NOT NULL,
    payload_relpath TEXT NOT NULL,
    payload_ready INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (source_id, track_id, start_us)
);

CREATE INDEX IF NOT EXISTS dvr_segments_window
    ON dvr_segments (source_id, track_id, start_us, end_us);
