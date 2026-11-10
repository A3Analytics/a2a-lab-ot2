CREATE TABLE IF NOT EXISTS semantic_frame_revisions (
    frame_id TEXT NOT NULL,
    store_namespace TEXT NOT NULL,
    source_id TEXT NOT NULL,
    track_id TEXT NOT NULL,
    start_us INTEGER NOT NULL,
    end_us INTEGER NOT NULL,
    payload_sha256 TEXT NOT NULL,
    payload_crc32 INTEGER NOT NULL,
    capture_group TEXT,
    retained_state TEXT NOT NULL,
    search_current INTEGER NOT NULL DEFAULT 1,
    revision_seq INTEGER NOT NULL,
    description_checkpoint TEXT NOT NULL DEFAULT 'pending',
    embedding_checkpoint TEXT NOT NULL DEFAULT 'pending',
    registered_at_us INTEGER NOT NULL,
    PRIMARY KEY (frame_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS semantic_frame_revisions_dvr_key_digest
    ON semantic_frame_revisions (source_id, track_id, start_us, payload_sha256);

CREATE INDEX IF NOT EXISTS semantic_frame_revisions_current_enum
    ON semantic_frame_revisions (
        source_id,
        track_id,
        start_us,
        frame_id
    )
    WHERE search_current = 1;

CREATE INDEX IF NOT EXISTS semantic_frame_revisions_history_enum
    ON semantic_frame_revisions (
        source_id,
        track_id,
        start_us,
        revision_seq,
        frame_id
    );

CREATE TABLE IF NOT EXISTS semantic_frame_descriptions (
    frame_id TEXT NOT NULL,
    description_text TEXT NOT NULL,
    description_provider TEXT NOT NULL,
    description_model TEXT NOT NULL,
    prompt_digest TEXT NOT NULL,
    preprocessing_digest TEXT NOT NULL,
    roi_x_min REAL,
    roi_y_min REAL,
    roi_x_max REAL,
    roi_y_max REAL,
    text_hash TEXT NOT NULL,
    description_generation INTEGER NOT NULL,
    PRIMARY KEY (frame_id),
    FOREIGN KEY (frame_id) REFERENCES semantic_frame_revisions (frame_id)
);

CREATE TABLE IF NOT EXISTS semantic_frame_embeddings (
    frame_id TEXT NOT NULL,
    embedding_provider TEXT NOT NULL,
    embedding_model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    normalization TEXT NOT NULL,
    metric TEXT NOT NULL,
    embedding_generation INTEGER NOT NULL,
    model_space_generation INTEGER NOT NULL,
    source_text_hash TEXT NOT NULL,
    PRIMARY KEY (frame_id),
    FOREIGN KEY (frame_id) REFERENCES semantic_frame_revisions (frame_id)
);
