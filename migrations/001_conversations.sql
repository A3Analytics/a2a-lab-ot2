CREATE TABLE IF NOT EXISTS conversation_messages (
    context_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (context_id, sequence)
);
