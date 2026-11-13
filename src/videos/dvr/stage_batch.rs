//! Shared provider-batch helpers for split semantic stage workers.

use a2a_lab_dev_kit::{A2aLabError, SemanticWorkerLimits};

use super::indexing_job_store::IndexingJobRecord;
use super::store::IndexedVideoDvr;

pub(crate) async fn schedule_indexing_retry(
    dvr: &IndexedVideoDvr,
    worker_id: &str,
    job: &IndexingJobRecord,
    limits: &SemanticWorkerLimits,
    reason: &str,
    now_us: i64,
) -> Result<(), A2aLabError> {
    let backoff = i64::try_from(limits.retry_backoff.as_secs())
        .unwrap_or(0)
        .saturating_mul(1_000_000);
    dvr.fail_indexing_job_lease(
        job.job_id(),
        worker_id,
        job.revision_generation(),
        reason,
        true,
        now_us.saturating_add(backoff),
        now_us,
        limits,
    )
    .await
}

pub(crate) fn provider_batch_limit(max_batch_items: u32) -> usize {
    usize::try_from(max_batch_items.max(1)).unwrap_or(1).max(1)
}
