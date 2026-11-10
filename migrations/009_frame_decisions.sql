CREATE TABLE IF NOT EXISTS frame_decisions (
    decision_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    frame_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    captured_at_us INTEGER NOT NULL,
    predicate_id TEXT NOT NULL,
    predicate_version INTEGER NOT NULL,
    subject_digest TEXT NOT NULL,
    producer_id TEXT NOT NULL,
    model_revision TEXT NOT NULL,
    evaluated_at_us INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    payload_fingerprint TEXT NOT NULL,
    supersedes TEXT,
    append_seq INTEGER NOT NULL,
    tombstoned INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (decision_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS frame_decisions_idempotency
    ON frame_decisions (idempotency_key);

CREATE INDEX IF NOT EXISTS frame_decisions_frame_append
    ON frame_decisions (frame_id, append_seq);

CREATE INDEX IF NOT EXISTS frame_decisions_lookup
    ON frame_decisions (
        frame_id,
        predicate_id,
        predicate_version,
        subject_digest,
        producer_id,
        model_revision
    );

CREATE INDEX IF NOT EXISTS frame_decisions_scope_time
    ON frame_decisions (source_id, captured_at_us, evaluated_at_us);

CREATE TABLE IF NOT EXISTS frame_decision_tombstones (
    frame_id TEXT NOT NULL,
    retained_state TEXT NOT NULL,
    tombstoned_at_us INTEGER NOT NULL,
    PRIMARY KEY (frame_id)
);

CREATE TABLE IF NOT EXISTS human_corpus_labels (
    label_id TEXT NOT NULL,
    frame_id TEXT NOT NULL,
    predicate_id TEXT NOT NULL,
    label_json TEXT NOT NULL,
    labeled_at_us INTEGER NOT NULL,
    PRIMARY KEY (label_id)
);

CREATE INDEX IF NOT EXISTS human_corpus_labels_frame
    ON human_corpus_labels (frame_id, predicate_id);
