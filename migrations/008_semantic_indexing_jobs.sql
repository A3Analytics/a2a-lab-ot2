CREATE TABLE IF NOT EXISTS semantic_indexing_jobs (
    job_id TEXT NOT NULL,
    frame_id TEXT NOT NULL,
    job_kind TEXT NOT NULL DEFAULT 'frame_indexing',
    created_at_us INTEGER NOT NULL,
    PRIMARY KEY (job_id),
    UNIQUE (frame_id, job_kind),
    FOREIGN KEY (frame_id) REFERENCES semantic_frame_revisions (frame_id)
);

CREATE INDEX IF NOT EXISTS semantic_indexing_jobs_frame
    ON semantic_indexing_jobs (frame_id);
