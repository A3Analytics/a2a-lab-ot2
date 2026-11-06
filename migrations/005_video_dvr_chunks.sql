CREATE TABLE IF NOT EXISTS dvr_chunks (
    source_id TEXT NOT NULL,
    track_id TEXT NOT NULL,
    minute_start_us INTEGER NOT NULL,
    chunk_relpath TEXT NOT NULL,
    chunk_ready INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (source_id, track_id, minute_start_us)
);

CREATE TABLE IF NOT EXISTS dvr_frames (
    source_id TEXT NOT NULL,
    track_id TEXT NOT NULL,
    start_us INTEGER NOT NULL,
    end_us INTEGER NOT NULL,
    keyframe INTEGER NOT NULL,
    minute_start_us INTEGER NOT NULL,
    chunk_relpath TEXT NOT NULL,
    payload_offset INTEGER NOT NULL,
    payload_length INTEGER NOT NULL,
    payload_crc32 INTEGER NOT NULL,
    payload_ready INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (source_id, track_id, start_us)
);

CREATE INDEX IF NOT EXISTS dvr_frames_window
    ON dvr_frames (source_id, track_id, start_us, end_us);

CREATE INDEX IF NOT EXISTS dvr_frames_source_start
    ON dvr_frames (source_id, start_us, track_id)
    WHERE payload_ready = 1;

CREATE INDEX IF NOT EXISTS dvr_frames_chunk
    ON dvr_frames (source_id, track_id, chunk_relpath, payload_offset);
