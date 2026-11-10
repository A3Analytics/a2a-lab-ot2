//! Durable indexing job leases, retries, and compare-and-set completion.

use a2a_lab_dev_kit::{A2aLabError, SemanticSchedule, SemanticWorkerLimits};
use sqlx::Row;
use sqlx::{Sqlite, Transaction};

use super::store::{IndexedVideoDvr, db_error};

/// Observable indexing job lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexingJobState {
    Pending,
    Leased,
    Described,
    VectorPending,
    Ready,
    RetryableFailed,
    TerminalFailed,
    Expired,
}

impl IndexingJobState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Leased => "leased",
            Self::Described => "described",
            Self::VectorPending => "vector_pending",
            Self::Ready => "ready",
            Self::RetryableFailed => "retryable_failed",
            Self::TerminalFailed => "terminal_failed",
            Self::Expired => "expired",
        }
    }

    fn parse(value: &str) -> Result<Self, A2aLabError> {
        match value {
            "pending" => Ok(Self::Pending),
            "leased" => Ok(Self::Leased),
            "described" => Ok(Self::Described),
            "vector_pending" => Ok(Self::VectorPending),
            "ready" => Ok(Self::Ready),
            "retryable_failed" => Ok(Self::RetryableFailed),
            "terminal_failed" => Ok(Self::TerminalFailed),
            "expired" => Ok(Self::Expired),
            _ => Err(A2aLabError::invalid(
                "job_state",
                format!("unknown {value}"),
            )),
        }
    }
}

/// One durable job row joined with runtime state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexingJobRecord {
    job_id: String,
    frame_id: String,
    state: IndexingJobState,
    revision_generation: u32,
    lease_owner: Option<String>,
    lease_expires_at_us: Option<i64>,
    description_generation: Option<u64>,
    description_attempts: u32,
    embedding_attempts: u32,
    last_retry_reason: Option<String>,
    next_retry_at_us: i64,
    last_error_at_us: Option<i64>,
    completed_at_us: Option<i64>,
}

impl IndexingJobRecord {
    #[must_use]
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    #[must_use]
    pub fn frame_id(&self) -> &str {
        &self.frame_id
    }

    #[must_use]
    pub const fn state(&self) -> IndexingJobState {
        self.state
    }

    #[must_use]
    pub const fn revision_generation(&self) -> u32 {
        self.revision_generation
    }

    #[must_use]
    pub fn lease_owner(&self) -> Option<&str> {
        self.lease_owner.as_deref()
    }

    #[must_use]
    pub const fn description_attempts(&self) -> u32 {
        self.description_attempts
    }

    #[must_use]
    pub const fn embedding_attempts(&self) -> u32 {
        self.embedding_attempts
    }

    #[must_use]
    pub fn last_retry_reason(&self) -> Option<&str> {
        self.last_retry_reason.as_deref()
    }

    #[must_use]
    pub const fn next_retry_at_us(&self) -> i64 {
        self.next_retry_at_us
    }

    #[must_use]
    pub fn description_generation(&self) -> Option<u64> {
        self.description_generation
    }
}

/// Bounded work-queue snapshot for one worker tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexingWorkerQueueStats {
    pub active_jobs: u64,
    pub materialization_paused: bool,
}

pub(crate) async fn ensure_job_runtime_tx(
    tx: &mut Transaction<'_, Sqlite>,
    job_id: &str,
) -> Result<(), A2aLabError> {
    sqlx::query(
        "INSERT INTO semantic_indexing_job_runtime (job_id)
         VALUES (?1)
         ON CONFLICT(job_id) DO NOTHING",
    )
    .bind(job_id)
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
}

impl IndexedVideoDvr {
    /// Selects the first saved frame at or after an interval due instant.
    /// Selection is durable, so retries and process restarts do not duplicate it.
    pub async fn select_semantic_stage_frame(
        &self,
        source_id: &str,
        track_id: &str,
        stage: &str,
        start_us: i64,
        schedule: SemanticSchedule,
    ) -> Result<bool, A2aLabError> {
        let Some(interval_us) = schedule.interval_micros() else {
            return Ok(true);
        };
        let interval_us = i64::try_from(interval_us).map_err(|_| {
            A2aLabError::invalid("semantic_indexing.schedules", "interval is too large")
        })?;
        let mut tx = self.sqlite_pool().begin().await.map_err(db_error)?;
        sqlx::query(
            "INSERT INTO semantic_indexing_stage_schedules
             (source_id, track_id, stage, interval_us, next_due_us)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source_id, track_id, stage) DO NOTHING",
        )
        .bind(source_id)
        .bind(track_id)
        .bind(stage)
        .bind(interval_us)
        .bind(start_us)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        let row = sqlx::query(
            "SELECT next_due_us, last_selected_start_us
             FROM semantic_indexing_stage_schedules
             WHERE source_id = ?1 AND track_id = ?2 AND stage = ?3",
        )
        .bind(source_id)
        .bind(track_id)
        .bind(stage)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        let next_due: i64 = row.try_get("next_due_us").map_err(db_error)?;
        let last_selected: Option<i64> = row.try_get("last_selected_start_us").map_err(db_error)?;
        let selected = last_selected == Some(start_us) || start_us >= next_due;
        if selected && last_selected != Some(start_us) {
            sqlx::query(
                "UPDATE semantic_indexing_stage_schedules
                 SET interval_us = ?4,
                     last_selected_start_us = ?5,
                     next_due_us = ?5 + ?4
                 WHERE source_id = ?1 AND track_id = ?2 AND stage = ?3",
            )
            .bind(source_id)
            .bind(track_id)
            .bind(stage)
            .bind(interval_us)
            .bind(start_us)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(selected)
    }

    /// Releases a stage-ineligible claim without changing retry state.
    pub async fn release_indexing_job(
        &self,
        job_id: &str,
        worker_id: &str,
    ) -> Result<(), A2aLabError> {
        sqlx::query(
            "UPDATE semantic_indexing_job_runtime
             SET job_state = CASE
                 WHEN description_generation IS NOT NULL THEN 'described'
                 ELSE 'pending'
             END,
             lease_owner = NULL, lease_expires_at_us = NULL
             WHERE job_id = ?1 AND job_state = 'leased' AND lease_owner = ?2",
        )
        .bind(job_id)
        .bind(worker_id)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    /// Loads one job with runtime state when present.
    pub async fn get_indexing_job(&self, job_id: &str) -> Result<IndexingJobRecord, A2aLabError> {
        let row = sqlx::query(
            "SELECT j.job_id, j.frame_id, COALESCE(r.job_state, 'pending') AS job_state,
                    COALESCE(r.revision_generation, 0) AS revision_generation,
                    r.lease_owner, r.lease_expires_at_us, r.description_generation,
                    COALESCE(r.description_attempts, 0) AS description_attempts,
                    COALESCE(r.embedding_attempts, 0) AS embedding_attempts,
                    r.last_retry_reason, COALESCE(r.next_retry_at_us, 0) AS next_retry_at_us,
                    r.last_error_at_us, r.completed_at_us
             FROM semantic_indexing_jobs j
             LEFT JOIN semantic_indexing_job_runtime r ON r.job_id = j.job_id
             WHERE j.job_id = ?1",
        )
        .bind(job_id)
        .fetch_optional(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        let Some(row) = row else {
            return Err(A2aLabError::not_found("indexing job", job_id));
        };
        record_from_row(&row)
    }

    /// Counts jobs that are not terminal, for backlog limits.
    pub async fn count_active_indexing_jobs(&self) -> Result<u64, A2aLabError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM semantic_indexing_job_runtime
             WHERE job_state NOT IN ('ready', 'terminal_failed', 'expired')",
        )
        .fetch_one(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(u64::try_from(count.max(0)).unwrap_or(0))
    }

    pub async fn indexing_worker_queue_stats(
        &self,
        limits: &SemanticWorkerLimits,
    ) -> Result<IndexingWorkerQueueStats, A2aLabError> {
        let active = self.count_active_indexing_jobs().await?;
        let paused = self.worker_meta_flag("materialization_paused").await?;
        let backlog = u64::from(limits.max_queue_backlog);
        let materialization_paused =
            paused || active >= backlog.saturating_add(u64::from(limits.max_batch_items));
        Ok(IndexingWorkerQueueStats {
            active_jobs: active,
            materialization_paused,
        })
    }

    pub async fn set_materialization_paused(&self, paused: bool) -> Result<(), A2aLabError> {
        self.set_worker_meta_flag("materialization_paused", paused)
            .await
    }

    /// Reclaims leases past their deadline back to a claimable state.
    pub async fn reclaim_expired_indexing_leases(&self, now_us: i64) -> Result<u32, A2aLabError> {
        let result = sqlx::query(
            "UPDATE semantic_indexing_job_runtime
             SET job_state = CASE
                   WHEN job_state = 'leased' AND description_generation IS NOT NULL THEN 'described'
                   ELSE 'pending'
                 END,
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 last_retry_reason = COALESCE(last_retry_reason, 'lease_expired'),
                 last_error_at_us = ?1
             WHERE job_state = 'leased'
               AND lease_expires_at_us IS NOT NULL
               AND lease_expires_at_us <= ?1",
        )
        .bind(now_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(u32::try_from(result.rows_affected()).unwrap_or(0))
    }

    /// Claims up to `batch` jobs for one worker id.
    pub async fn claim_indexing_jobs(
        &self,
        worker_id: &str,
        batch: u32,
        lease_deadline_us: i64,
        now_us: i64,
    ) -> Result<Vec<IndexingJobRecord>, A2aLabError> {
        let limit = i64::from(batch.max(1));
        let candidates: Vec<(String, String)> = sqlx::query(
            "SELECT j.job_id, j.frame_id
             FROM semantic_indexing_jobs j
             JOIN semantic_indexing_job_runtime r ON r.job_id = j.job_id
             JOIN semantic_frame_revisions f ON f.frame_id = j.frame_id
             WHERE f.search_current = 1
               AND f.retained_state = 'retained'
               AND r.job_state IN ('pending', 'retryable_failed', 'described')
               AND r.next_retry_at_us <= ?1
               AND (r.lease_expires_at_us IS NULL OR r.lease_expires_at_us < ?1)
             ORDER BY j.created_at_us ASC, j.job_id ASC
             LIMIT ?2",
        )
        .bind(now_us)
        .bind(limit)
        .fetch_all(self.sqlite_pool())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|row| {
            let job_id: String = row.try_get("job_id").map_err(db_error)?;
            let frame_id: String = row.try_get("frame_id").map_err(db_error)?;
            Ok((job_id, frame_id))
        })
        .collect::<Result<Vec<_>, A2aLabError>>()?;

        let mut claimed = Vec::new();
        for (job_id, frame_id) in candidates {
            if let Some(record) = self
                .try_claim_one_job(&job_id, &frame_id, worker_id, lease_deadline_us, now_us)
                .await?
            {
                claimed.push(record);
            }
        }
        Ok(claimed)
    }

    pub(crate) async fn try_claim_one_job(
        &self,
        job_id: &str,
        frame_id: &str,
        worker_id: &str,
        lease_deadline_us: i64,
        now_us: i64,
    ) -> Result<Option<IndexingJobRecord>, A2aLabError> {
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision_seq FROM semantic_frame_revisions WHERE frame_id = ?1",
        )
        .bind(frame_id)
        .fetch_optional(self.sqlite_pool())
        .await
        .map_err(db_error)?
        .unwrap_or(0);
        let result = sqlx::query(
            "UPDATE semantic_indexing_job_runtime
             SET job_state = 'leased',
                 lease_owner = ?2,
                 lease_expires_at_us = ?3,
                 revision_generation = ?4,
                 description_attempts = description_attempts + CASE
                   WHEN job_state IN ('pending', 'retryable_failed') THEN 1 ELSE 0 END,
                 embedding_attempts = embedding_attempts + CASE
                   WHEN job_state = 'described' THEN 1 ELSE 0 END
             WHERE job_id = ?1
               AND job_state IN ('pending', 'retryable_failed', 'described')
               AND (lease_expires_at_us IS NULL OR lease_expires_at_us < ?5)
               AND next_retry_at_us <= ?5",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(lease_deadline_us)
        .bind(revision)
        .bind(now_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_indexing_job(job_id).await.map(Some)
    }

    /// Marks description committed with lease ownership and revision generation checks.
    pub async fn complete_indexing_description_cas(
        &self,
        job_id: &str,
        worker_id: &str,
        revision_generation: u32,
        description_generation: u64,
        now_us: i64,
    ) -> Result<bool, A2aLabError> {
        let result = sqlx::query(
            "UPDATE semantic_indexing_job_runtime
             SET job_state = 'described',
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 description_generation = ?4,
                 last_retry_reason = NULL
             WHERE job_id = ?1
               AND job_state = 'leased'
               AND lease_owner = ?2
               AND revision_generation = ?3
               AND EXISTS (
                 SELECT 1 FROM semantic_indexing_jobs j
                 JOIN semantic_frame_revisions f ON f.frame_id = j.frame_id
                 WHERE j.job_id = ?1
                   AND f.revision_seq = ?3
                   AND f.search_current = 1
               )",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(i64::from(revision_generation))
        .bind(i64::try_from(description_generation).unwrap_or(i64::MAX))
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }
        let _ = now_us;
        Ok(true)
    }

    /// Marks embedding provenance committed; transitions to ready (vector backend out of scope).
    pub async fn complete_indexing_embedding_cas(
        &self,
        job_id: &str,
        worker_id: &str,
        revision_generation: u32,
        now_us: i64,
    ) -> Result<bool, A2aLabError> {
        let result = sqlx::query(
            "UPDATE semantic_indexing_job_runtime
             SET job_state = 'ready',
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 completed_at_us = ?4,
                 last_retry_reason = NULL
             WHERE job_id = ?1
               AND job_state = 'leased'
               AND lease_owner = ?2
               AND revision_generation = ?3
               AND EXISTS (
                 SELECT 1 FROM semantic_indexing_jobs j
                 JOIN semantic_frame_revisions f ON f.frame_id = j.frame_id
                 WHERE j.job_id = ?1
                   AND f.revision_seq = ?3
                   AND f.search_current = 1
               )",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(i64::from(revision_generation))
        .bind(now_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected() > 0)
    }

    pub(crate) async fn fail_indexing_job_lease(
        &self,
        job_id: &str,
        worker_id: &str,
        revision_generation: u32,
        reason: &str,
        retryable: bool,
        next_retry_at_us: i64,
        now_us: i64,
        limits: &SemanticWorkerLimits,
    ) -> Result<(), A2aLabError> {
        let attempts: i64 = sqlx::query_scalar(
            "SELECT description_attempts + embedding_attempts
             FROM semantic_indexing_job_runtime WHERE job_id = ?1",
        )
        .bind(job_id)
        .fetch_optional(self.sqlite_pool())
        .await
        .map_err(db_error)?
        .unwrap_or(0);
        let terminal =
            !retryable || attempts >= i64::from(limits.max_retry_attempts.saturating_add(1));
        let next_state = if terminal {
            IndexingJobState::TerminalFailed.as_str()
        } else {
            IndexingJobState::RetryableFailed.as_str()
        };
        sqlx::query(
            "UPDATE semantic_indexing_job_runtime
             SET job_state = ?4,
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 last_retry_reason = ?5,
                 next_retry_at_us = ?6,
                 last_error_at_us = ?7
             WHERE job_id = ?1
               AND lease_owner = ?2
               AND revision_generation = ?3",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(i64::from(revision_generation))
        .bind(next_state)
        .bind(reason)
        .bind(next_retry_at_us)
        .bind(now_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    pub(crate) async fn expire_indexing_job(
        &self,
        job_id: &str,
        now_us: i64,
    ) -> Result<(), A2aLabError> {
        sqlx::query(
            "UPDATE semantic_indexing_job_runtime
             SET job_state = 'expired',
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 completed_at_us = ?2,
                 last_retry_reason = 'frame_expired'
             WHERE job_id = ?1",
        )
        .bind(job_id)
        .bind(now_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn worker_meta_flag(&self, key: &str) -> Result<bool, A2aLabError> {
        let value: Option<String> = sqlx::query_scalar(
            "SELECT meta_value FROM semantic_indexing_worker_meta WHERE meta_key = ?1",
        )
        .bind(key)
        .fetch_optional(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(matches!(value.as_deref(), Some("1" | "true")))
    }

    async fn set_worker_meta_flag(&self, key: &str, enabled: bool) -> Result<(), A2aLabError> {
        let value = if enabled { "true" } else { "false" };
        sqlx::query(
            "INSERT INTO semantic_indexing_worker_meta (meta_key, meta_value)
             VALUES (?1, ?2)
             ON CONFLICT(meta_key) DO UPDATE SET meta_value = excluded.meta_value",
        )
        .bind(key)
        .bind(value)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    pub(crate) async fn reconcile_cursor_save(
        &self,
        source_id: &str,
        track_id: &str,
        start_us: i64,
    ) -> Result<(), A2aLabError> {
        let cursor = format!("{source_id}|{track_id}|{start_us}");
        self.set_worker_meta_string("reconcile_cursor", &cursor)
            .await
    }

    /// Last reconcile cursor for deferred discovery when materialization is paused.
    pub async fn reconcile_cursor(&self) -> Result<Option<(String, String, i64)>, A2aLabError> {
        let value = self.worker_meta_string("reconcile_cursor").await?;
        let Some(value) = value else {
            return Ok(None);
        };
        let mut parts = value.split('|');
        let source = parts
            .next()
            .ok_or_else(|| A2aLabError::invalid("reconcile_cursor", "missing source_id"))?
            .to_owned();
        let track = parts
            .next()
            .ok_or_else(|| A2aLabError::invalid("reconcile_cursor", "missing track_id"))?
            .to_owned();
        let start = parts
            .next()
            .ok_or_else(|| A2aLabError::invalid("reconcile_cursor", "missing start_us"))?
            .parse()
            .map_err(|_| A2aLabError::invalid("reconcile_cursor", "invalid start_us"))?;
        Ok(Some((source, track, start)))
    }

    async fn worker_meta_string(&self, key: &str) -> Result<Option<String>, A2aLabError> {
        sqlx::query_scalar(
            "SELECT meta_value FROM semantic_indexing_worker_meta WHERE meta_key = ?1",
        )
        .bind(key)
        .fetch_optional(self.sqlite_pool())
        .await
        .map_err(db_error)
    }

    async fn set_worker_meta_string(&self, key: &str, value: &str) -> Result<(), A2aLabError> {
        sqlx::query(
            "INSERT INTO semantic_indexing_worker_meta (meta_key, meta_value)
             VALUES (?1, ?2)
             ON CONFLICT(meta_key) DO UPDATE SET meta_value = excluded.meta_value",
        )
        .bind(key)
        .bind(value)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

fn record_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<IndexingJobRecord, A2aLabError> {
    let job_state: String = row.try_get("job_state").map_err(db_error)?;
    let description_generation: Option<i64> =
        row.try_get("description_generation").map_err(db_error)?;
    Ok(IndexingJobRecord {
        job_id: row.try_get("job_id").map_err(db_error)?,
        frame_id: row.try_get("frame_id").map_err(db_error)?,
        state: IndexingJobState::parse(&job_state)?,
        revision_generation: u32::try_from(
            row.try_get::<i64, _>("revision_generation")
                .map_err(db_error)?
                .max(0),
        )
        .unwrap_or(0),
        lease_owner: row.try_get("lease_owner").map_err(db_error)?,
        lease_expires_at_us: row.try_get("lease_expires_at_us").map_err(db_error)?,
        description_generation: description_generation.and_then(|value| u64::try_from(value).ok()),
        description_attempts: u32::try_from(
            row.try_get::<i64, _>("description_attempts")
                .map_err(db_error)?
                .max(0),
        )
        .unwrap_or(0),
        embedding_attempts: u32::try_from(
            row.try_get::<i64, _>("embedding_attempts")
                .map_err(db_error)?
                .max(0),
        )
        .unwrap_or(0),
        last_retry_reason: row.try_get("last_retry_reason").map_err(db_error)?,
        next_retry_at_us: row
            .try_get::<i64, _>("next_retry_at_us")
            .map_err(db_error)?,
        last_error_at_us: row.try_get("last_error_at_us").map_err(db_error)?,
        completed_at_us: row.try_get("completed_at_us").map_err(db_error)?,
    })
}
