CREATE TABLE IF NOT EXISTS semantic_indexing_job_runtime (
    job_id TEXT NOT NULL,
    job_state TEXT NOT NULL DEFAULT 'pending',
    revision_generation INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_expires_at_us INTEGER,
    description_generation INTEGER,
    description_attempts INTEGER NOT NULL DEFAULT 0,
    embedding_attempts INTEGER NOT NULL DEFAULT 0,
    last_retry_reason TEXT,
    next_retry_at_us INTEGER NOT NULL DEFAULT 0,
    last_error_at_us INTEGER,
    completed_at_us INTEGER,
    PRIMARY KEY (job_id),
    FOREIGN KEY (job_id) REFERENCES semantic_indexing_jobs (job_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS semantic_indexing_job_runtime_work_queue
    ON semantic_indexing_job_runtime (job_state, next_retry_at_us);

CREATE TABLE IF NOT EXISTS semantic_indexing_worker_meta (
    meta_key TEXT NOT NULL PRIMARY KEY,
    meta_value TEXT NOT NULL
);
