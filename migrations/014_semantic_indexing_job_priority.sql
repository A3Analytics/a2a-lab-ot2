ALTER TABLE semantic_indexing_jobs
    ADD COLUMN enqueue_priority INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS semantic_indexing_jobs_claim_order
    ON semantic_indexing_jobs (enqueue_priority, created_at_us, job_id);
