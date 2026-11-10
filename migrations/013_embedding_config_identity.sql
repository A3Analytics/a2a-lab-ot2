CREATE TABLE IF NOT EXISTS semantic_embedding_config_identity (
    singleton_id INTEGER NOT NULL PRIMARY KEY CHECK (singleton_id = 1),
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    normalization TEXT NOT NULL,
    metric TEXT NOT NULL,
    config_digest TEXT NOT NULL,
    updated_at_us INTEGER NOT NULL
);
