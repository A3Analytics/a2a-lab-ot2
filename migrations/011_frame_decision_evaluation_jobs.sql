CREATE TABLE IF NOT EXISTS frame_decision_evaluation_jobs (
    job_id TEXT NOT NULL,
    frame_id TEXT NOT NULL,
    evaluation_id TEXT NOT NULL,
    predicate_id TEXT NOT NULL,
    predicate_version INTEGER NOT NULL,
    subject_digest TEXT NOT NULL,
    config_digest TEXT NOT NULL,
    producer_revision TEXT NOT NULL,
    coverage_policy TEXT NOT NULL,
    created_at_us INTEGER NOT NULL,
    PRIMARY KEY (job_id),
    UNIQUE (
        evaluation_id,
        predicate_id,
        predicate_version,
        subject_digest,
        config_digest,
        producer_revision
    ),
    FOREIGN KEY (frame_id) REFERENCES semantic_frame_revisions (frame_id)
);

CREATE INDEX IF NOT EXISTS frame_decision_evaluation_jobs_frame
    ON frame_decision_evaluation_jobs (frame_id);

CREATE TABLE IF NOT EXISTS frame_decision_evaluation_job_runtime (
    job_id TEXT NOT NULL,
    job_state TEXT NOT NULL DEFAULT 'pending',
    coverage TEXT,
    gap_reason TEXT,
    revision_generation INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_expires_at_us INTEGER,
    evaluation_attempts INTEGER NOT NULL DEFAULT 0,
    last_retry_reason TEXT,
    next_retry_at_us INTEGER NOT NULL DEFAULT 0,
    last_error_at_us INTEGER,
    completed_at_us INTEGER,
    cancelled_at_us INTEGER,
    idempotency_key TEXT,
    decision_id TEXT,
    PRIMARY KEY (job_id),
    FOREIGN KEY (job_id) REFERENCES frame_decision_evaluation_jobs (job_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS frame_decision_evaluation_job_runtime_work_queue
    ON frame_decision_evaluation_job_runtime (job_state, next_retry_at_us);
