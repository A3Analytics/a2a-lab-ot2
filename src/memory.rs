//! SQLite history for Rig conversations, keyed by A2A context id.

use std::path::Path;
use std::time::Duration;

use a2a_lab_dev_kit::A2aLabError;
use rig::completion::Message;
use rig::memory::{ConversationMemory, MemoryError};
use rig::wasm_compat::WasmBoxedFuture;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{SqlitePool, query, query_scalar};

const SCHEMA: &str = include_str!("../migrations/001_conversations.sql");

/// Ordered Rig messages for each A2A conversation.
#[derive(Clone)]
pub struct ConversationStore {
    pool: SqlitePool,
    history_limit: usize,
}

impl ConversationStore {
    /// Opens or creates `path` and keeps the newest `history_limit` messages per context.
    pub async fn open(path: &Path, history_limit: usize) -> Result<Self, A2aLabError> {
        if history_limit == 0 {
            return Err(A2aLabError::invalid(
                "history_limit",
                "must keep at least one message",
            ));
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                A2aLabError::unavailable(format!("create {}: {error}", parent.display()))
            })?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(|error| A2aLabError::unavailable(error.to_string()))?;
        query(SCHEMA)
            .execute(&pool)
            .await
            .map_err(|error| A2aLabError::unavailable(error.to_string()))?;
        Ok(Self {
            pool,
            history_limit,
        })
    }
}

impl ConversationMemory for ConversationStore {
    fn load<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> WasmBoxedFuture<'a, Result<Vec<Message>, MemoryError>> {
        Box::pin(async move {
            let rows: Vec<String> = query_scalar(
                "SELECT body FROM conversation_messages WHERE context_id = ? ORDER BY sequence",
            )
            .bind(conversation_id)
            .fetch_all(&self.pool)
            .await
            .map_err(MemoryError::backend)?;
            rows.into_iter()
                .map(|body| serde_json::from_str(&body).map_err(MemoryError::backend))
                .collect()
        })
    }

    fn append<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: Vec<Message>,
    ) -> WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(MemoryError::backend)?;
            let current: i64 = query_scalar(
                "SELECT COALESCE(MAX(sequence), 0) FROM conversation_messages WHERE context_id = ?",
            )
            .bind(conversation_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(MemoryError::backend)?;
            for (offset, message) in messages.iter().enumerate() {
                let body = serde_json::to_string(message).map_err(MemoryError::backend)?;
                let sequence = current + i64::try_from(offset).map_err(MemoryError::backend)? + 1;
                query(
                    "INSERT INTO conversation_messages (context_id, sequence, body) VALUES (?, ?, ?)",
                )
                .bind(conversation_id)
                .bind(sequence)
                .bind(body)
                .execute(&mut *tx)
                .await
                .map_err(MemoryError::backend)?;
            }
            query(
                "DELETE FROM conversation_messages
                 WHERE context_id = ?1
                   AND sequence <= (
                     SELECT sequence FROM conversation_messages
                     WHERE context_id = ?1
                     ORDER BY sequence DESC
                     LIMIT 1 OFFSET ?2
                   )",
            )
            .bind(conversation_id)
            .bind(i64::try_from(self.history_limit).map_err(MemoryError::backend)?)
            .execute(&mut *tx)
            .await
            .map_err(MemoryError::backend)?;
            tx.commit().await.map_err(MemoryError::backend)?;
            Ok(())
        })
    }

    fn clear<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            query("DELETE FROM conversation_messages WHERE context_id = ?")
                .bind(conversation_id)
                .execute(&self.pool)
                .await
                .map_err(MemoryError::backend)?;
            Ok(())
        })
    }
}
