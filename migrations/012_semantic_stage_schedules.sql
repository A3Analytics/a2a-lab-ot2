CREATE TABLE IF NOT EXISTS semantic_indexing_stage_schedules (
    source_id TEXT NOT NULL,
    track_id TEXT NOT NULL,
    stage TEXT NOT NULL,
    interval_us INTEGER,
    next_due_us INTEGER,
    last_selected_start_us INTEGER,
    PRIMARY KEY (source_id, track_id, stage)
);
