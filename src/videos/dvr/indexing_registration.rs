//! Durable indexing intent at the saved-frame metadata commit boundary.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use a2a_lab_dev_kit::{
    A2aLabError, DvrStoreNamespace, DvrTrackId, SavedFrameRef, UtcTimestamp, VideoSourceId,
};
use sqlx::Row;
use sqlx::{Sqlite, Transaction};

use super::frame_manifest::RegisterManifestOutcome;
use super::store::{IndexedVideoDvr, db_error};

/// Default semantic store namespace written at the commit boundary.
pub use super::frame_manifest::DEFAULT_SEMANTIC_STORE_NAMESPACE;

const FRAME_INDEXING_JOB_KIND: &str = "frame_indexing";
pub const DEFAULT_RECONCILE_BATCH: u32 = 256;

/// Test-only faults injected on the DVR save path.
#[derive(Debug, Default)]
pub struct SaveFaults {
    fail_payload_append: AtomicBool,
    fail_metadata_commit: AtomicBool,
}

impl SaveFaults {
    #[must_use]
    pub fn fail_payload_append(&self) -> bool {
        self.fail_payload_append.load(Ordering::Relaxed)
    }

    pub fn set_fail_payload_append(&self, value: bool) {
        self.fail_payload_append.store(value, Ordering::Relaxed);
    }

    #[must_use]
    pub fn fail_metadata_commit(&self) -> bool {
        self.fail_metadata_commit.load(Ordering::Relaxed)
    }

    pub fn set_fail_metadata_commit(&self, value: bool) {
        self.fail_metadata_commit.store(value, Ordering::Relaxed);
    }
}

/// Outcome of one bounded reconciliation pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexingReconcileReport {
    examined: u32,
    registered: u32,
    jobs_created: u32,
    idempotent: u32,
    missing_payload: u32,
    corrupt_payload: u32,
    has_more: bool,
}

impl IndexingReconcileReport {
    #[must_use]
    pub const fn examined(&self) -> u32 {
        self.examined
    }

    #[must_use]
    pub const fn registered(&self) -> u32 {
        self.registered
    }

    #[must_use]
    pub const fn jobs_created(&self) -> u32 {
        self.jobs_created
    }

    #[must_use]
    pub const fn idempotent(&self) -> u32 {
        self.idempotent
    }

    #[must_use]
    pub const fn missing_payload(&self) -> u32 {
        self.missing_payload
    }

    #[must_use]
    pub const fn corrupt_payload(&self) -> u32 {
        self.corrupt_payload
    }

    #[must_use]
    pub const fn has_more(&self) -> bool {
        self.has_more
    }
}

impl IndexedVideoDvr {
    /// Injectable faults for hardware-free save-path tests.
    #[must_use]
    pub fn save_faults(&self) -> &Arc<SaveFaults> {
        &self.save_faults
    }

    /// Registers manifest revision and indexing job for one committed DVR key.
    pub async fn ensure_indexing_at_dvr_key(
        &self,
        store: &DvrStoreNamespace,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        start_us: UtcTimestamp,
    ) -> Result<(SavedFrameRef, RegisterManifestOutcome, bool), A2aLabError> {
        let (frame, outcome) = self
            .register_manifest_at_dvr_key(store, source_id, track_id, start_us)
            .await?;
        let job_created = self
            .ensure_indexing_job_for_frame(frame.frame_id().as_str())
            .await?;
        Ok((frame, outcome, job_created))
    }

    /// Discovers ready DVR rows missing manifest/job pairs up to `batch` keys.
    #[allow(clippy::too_many_lines)]
    pub async fn reconcile_indexing_jobs(
        &self,
        store: &DvrStoreNamespace,
        batch: u32,
    ) -> Result<IndexingReconcileReport, A2aLabError> {
        let limit = i64::from(batch.max(1));
        let rows = sqlx::query(
            "SELECT f.source_id, f.track_id, f.start_us
             FROM dvr_frames f
             WHERE f.payload_ready = 1
               AND NOT EXISTS (
                 SELECT 1 FROM semantic_indexing_jobs j
                 JOIN semantic_frame_revisions r ON r.frame_id = j.frame_id
                 WHERE r.source_id = f.source_id
                   AND r.track_id = f.track_id
                   AND r.start_us = f.start_us
                   AND r.search_current = 1
               )
             ORDER BY f.source_id, f.track_id, f.start_us
             LIMIT ?1",
        )
        .bind(limit + 1)
        .fetch_all(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        let has_more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
        let mut report = IndexingReconcileReport {
            examined: 0,
            registered: 0,
            jobs_created: 0,
            idempotent: 0,
            missing_payload: 0,
            corrupt_payload: 0,
            has_more,
        };
        let mut last_cursor: Option<(String, String, i64)> = None;
        for row in rows
            .iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
        {
            report.examined = report.examined.saturating_add(1);
            let source_id =
                VideoSourceId::new(row.try_get::<String, _>("source_id").map_err(db_error)?)?;
            if !self.permits_indexing_source(source_id.as_str()) {
                continue;
            }
            let track_id =
                DvrTrackId::new(row.try_get::<String, _>("track_id").map_err(db_error)?)?;
            let start_us = crate::time::from_unix_microseconds(
                row.try_get::<i64, _>("start_us").map_err(db_error)?,
            )?;
            last_cursor = Some((
                source_id.as_str().to_owned(),
                track_id.as_str().to_owned(),
                start_us.as_micros(),
            ));
            let register = self
                .register_manifest_at_dvr_key(store, &source_id, &track_id, start_us)
                .await;
            match register {
                Ok((frame, outcome)) => {
                    let job_created = self
                        .ensure_indexing_job_for_frame(frame.frame_id().as_str())
                        .await?;
                    if job_created {
                        report.jobs_created = report.jobs_created.saturating_add(1);
                    }
                    match outcome {
                        RegisterManifestOutcome::Registered
                        | RegisterManifestOutcome::ReplacedPrior => {
                            report.registered = report.registered.saturating_add(1);
                        }
                        RegisterManifestOutcome::Idempotent => {
                            report.idempotent = report.idempotent.saturating_add(1);
                        }
                    }
                    if let Ok(record) = self.get_manifest_frame(frame.frame_id().as_str()).await {
                        match record.payload_status() {
                            super::frame_manifest::MediaPayloadStatus::Missing => {
                                report.missing_payload = report.missing_payload.saturating_add(1);
                            }
                            super::frame_manifest::MediaPayloadStatus::Corrupt => {
                                report.corrupt_payload = report.corrupt_payload.saturating_add(1);
                            }
                            super::frame_manifest::MediaPayloadStatus::Available => {}
                        }
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    if message.contains("not found") || message.contains("checksum mismatch") {
                        report.missing_payload = report.missing_payload.saturating_add(1);
                    } else if message.contains("checksum") {
                        report.corrupt_payload = report.corrupt_payload.saturating_add(1);
                    } else {
                        return Err(error);
                    }
                }
            }
        }
        if let Some((source_id, track_id, start_us)) = last_cursor {
            self.reconcile_cursor_save(&source_id, &track_id, start_us)
                .await?;
        }
        Ok(report)
    }

    pub(crate) async fn ensure_indexing_job_for_frame(
        &self,
        frame_id: &str,
    ) -> Result<bool, A2aLabError> {
        let registered_at = crate::time::now()?.as_micros();
        let mut tx = self.sqlite_pool().begin().await.map_err(db_error)?;
        let created = commit_indexing_job_tx(&mut tx, frame_id, registered_at).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(created)
    }

    #[doc(hidden)]
    pub async fn count_indexing_jobs(&self) -> Result<u64, A2aLabError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM semantic_indexing_jobs")
            .fetch_one(self.sqlite_pool())
            .await
            .map_err(db_error)?;
        Ok(u64::try_from(count.max(0)).unwrap_or(0))
    }

    #[doc(hidden)]
    pub async fn indexing_job_exists(&self, frame_id: &str) -> Result<bool, A2aLabError> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM semantic_indexing_jobs WHERE frame_id = ?1")
                .bind(frame_id)
                .fetch_one(self.sqlite_pool())
                .await
                .map_err(db_error)?;
        Ok(count > 0)
    }
}

#[must_use]
pub fn indexing_job_id(frame_id: &str) -> String {
    format!("{frame_id}:{FRAME_INDEXING_JOB_KIND}")
}

pub(crate) async fn commit_indexing_job_tx(
    tx: &mut Transaction<'_, Sqlite>,
    frame_id: &str,
    registered_at_us: i64,
) -> Result<bool, A2aLabError> {
    let job_id = indexing_job_id(frame_id);
    let result = sqlx::query(
        "INSERT INTO semantic_indexing_jobs (job_id, frame_id, job_kind, created_at_us)
         SELECT ?1, ?2, ?3, ?4
         WHERE EXISTS (SELECT 1 FROM semantic_frame_revisions WHERE frame_id = ?2)
         ON CONFLICT(frame_id, job_kind) DO NOTHING",
    )
    .bind(&job_id)
    .bind(frame_id)
    .bind(FRAME_INDEXING_JOB_KIND)
    .bind(registered_at_us)
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    if result.rows_affected() > 0 {
        super::indexing_job_store::ensure_job_runtime_tx(tx, &job_id).await?;
    }
    Ok(result.rows_affected() > 0)
}
