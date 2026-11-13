//! Embedding-only indexing worker lane.

use std::collections::HashMap;
use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, FakeTextEmbeddingProvider, FrameDescriptionKey, KeyedTextEmbedding,
    SemanticIndexingConfig, SemanticIndexingStage, SemanticWorkerLimits, TextEmbeddingKey,
    TextEmbeddingProvider, TextEmbeddingRequest, validate_embedding_batch,
};

use super::annotation_worker::{IndexingWorkerBatchReport, map_indexing_failure};
use super::frame_manifest::{
    IndexingCheckpoint, ManifestRetainedState, MediaPayloadStatus, SemanticFrameManifestRecord,
};
use super::indexing_job_store::IndexingJobRecord;
use super::indexing_registration::{DEFAULT_RECONCILE_BATCH, IndexingWorkerLane};
use super::stage_batch::{provider_batch_limit, schedule_indexing_retry};
use super::store::IndexedVideoDvr;
use super::worker_faults::IndexingWorkerFaults;
use crate::videos::semantic_indexing::{
    ConfiguredTextEmbeddingProvider, indexing_claim_batch_size, text_embedding_limits,
};

pub type DeterministicEmbeddingOnlyWorker =
    EmbeddingOnlyAnnotationWorker<FakeTextEmbeddingProvider>;
pub type LiveEmbeddingOnlyWorker = EmbeddingOnlyAnnotationWorker<ConfiguredTextEmbeddingProvider>;

struct EmbeddingWorkItem {
    job: IndexingJobRecord,
    manifest: SemanticFrameManifestRecord,
    request: TextEmbeddingRequest,
    source_id: String,
}

enum EmbeddingPrep {
    Noop,
    IdempotentCommitted,
    Ready(Box<EmbeddingWorkItem>),
}

pub struct EmbeddingOnlyAnnotationWorker<E> {
    provider: E,
    source_providers: HashMap<String, E>,
    limits: SemanticWorkerLimits,
    worker_id: String,
    lane: IndexingWorkerLane,
    faults: Arc<IndexingWorkerFaults>,
}

impl<E> EmbeddingOnlyAnnotationWorker<E>
where
    E: TextEmbeddingProvider,
{
    #[must_use]
    pub fn new(
        provider: E,
        limits: SemanticWorkerLimits,
        worker_id: impl Into<String>,
        faults: Arc<IndexingWorkerFaults>,
    ) -> Self {
        Self {
            provider,
            source_providers: HashMap::new(),
            limits,
            worker_id: worker_id.into(),
            lane: IndexingWorkerLane::Live,
            faults,
        }
    }

    #[must_use]
    pub const fn with_lane(mut self, lane: IndexingWorkerLane) -> Self {
        self.lane = lane;
        self
    }

    #[must_use]
    pub fn with_source_provider(mut self, source_id: impl Into<String>, provider: E) -> Self {
        self.source_providers.insert(source_id.into(), provider);
        self
    }

    fn provider_for(&self, source_id: &str) -> &E {
        self.source_providers
            .get(source_id)
            .unwrap_or(&self.provider)
    }

    pub async fn run_batch(
        &self,
        dvr: &IndexedVideoDvr,
        semantic: &SemanticIndexingConfig,
    ) -> Result<IndexingWorkerBatchReport, A2aLabError> {
        let now_us = crate::time::now()?.as_micros();
        let reclaimed = dvr.reclaim_expired_indexing_leases(now_us).await?;
        let queue = dvr.indexing_worker_queue_stats(&self.limits).await?;
        dvr.clear_legacy_materialization_pause().await?;
        let deadline = now_us.saturating_add(
            i64::try_from(self.limits.job_deadline.as_secs())
                .unwrap_or(i64::MAX)
                .saturating_mul(1_000_000),
        );
        let claim_batch = indexing_claim_batch_size(&self.limits);
        let classify_batch = claim_batch.saturating_mul(4).max(DEFAULT_RECONCILE_BATCH);
        let _ = dvr
            .classify_expired_indexing_jobs_missing_retained_media(now_us, classify_batch)
            .await?;
        let claimed = dvr
            .claim_indexing_jobs_for_lane(&self.worker_id, claim_batch, deadline, now_us, self.lane)
            .await?;
        let mut report = IndexingWorkerBatchReport::after_claim(
            reclaimed,
            u32::try_from(claimed.len()).unwrap_or(u32::MAX),
            queue.backlog_over_limit,
            self.lane,
        );
        let embedding_limits = text_embedding_limits(&semantic.limits);
        let batch_limit = provider_batch_limit(embedding_limits.max_batch_items());
        let concurrency = usize::try_from(self.limits.max_concurrency)
            .unwrap_or(1)
            .max(1);
        for chunk in claimed.chunks(concurrency) {
            let mut work: Vec<EmbeddingWorkItem> = Vec::new();
            for job in chunk {
                match self
                    .prepare_embedding_work(dvr, semantic, job, now_us, embedding_limits)
                    .await
                {
                    Ok(EmbeddingPrep::IdempotentCommitted) => report.record_embedding_committed(),
                    Ok(EmbeddingPrep::Noop) => {}
                    Ok(EmbeddingPrep::Ready(item)) => work.push(*item),
                    Err(error) => {
                        eprintln!("embedding worker {}: {error}", self.worker_id);
                        report.record_failure();
                    }
                }
            }
            work.sort_by(|a, b| a.source_id.cmp(&b.source_id));
            let mut start = 0usize;
            while start < work.len() {
                let mut end = start + 1;
                while end < work.len() && work[end].source_id == work[start].source_id {
                    end += 1;
                }
                let group = &work[start..end];
                for batch in group.chunks(batch_limit) {
                    self.execute_embedding_provider_batch(
                        dvr,
                        batch,
                        now_us,
                        embedding_limits,
                        &mut report,
                    )
                    .await;
                }
                start = end;
            }
        }
        Ok(report)
    }

    async fn prepare_embedding_work(
        &self,
        dvr: &IndexedVideoDvr,
        semantic: &SemanticIndexingConfig,
        job: &IndexingJobRecord,
        now_us: i64,
        embedding_limits: a2a_lab_dev_kit::TextEmbeddingLimits,
    ) -> Result<EmbeddingPrep, A2aLabError> {
        let manifest = dvr.get_manifest_frame(job.frame_id()).await?;
        let frame = manifest.frame();
        if manifest.retained_state() != ManifestRetainedState::Retained
            || !manifest.search_current()
            || manifest.payload_status() != MediaPayloadStatus::Available
        {
            dvr.expire_indexing_job(job.job_id(), now_us).await?;
            return Ok(EmbeddingPrep::Noop);
        }
        let schedule = semantic.schedule_for(frame.source_id().as_str());
        if !dvr
            .select_semantic_stage_frame(
                frame.source_id().as_str(),
                frame.track_id().as_str(),
                SemanticIndexingStage::Embeddings.id(),
                frame.captured_at().as_micros(),
                schedule.embeddings(),
            )
            .await?
        {
            dvr.release_indexing_job(job.job_id(), &self.worker_id)
                .await?;
            return Ok(EmbeddingPrep::Noop);
        }
        if manifest.description().is_none() {
            dvr.release_indexing_job(job.job_id(), &self.worker_id)
                .await?;
            return Ok(EmbeddingPrep::Noop);
        }
        if manifest.embedding_checkpoint() == IndexingCheckpoint::Ready {
            let _ = dvr
                .complete_indexing_embedding_cas(
                    job.job_id(),
                    &self.worker_id,
                    job.revision_generation(),
                    now_us,
                )
                .await?;
            return Ok(EmbeddingPrep::IdempotentCommitted);
        }
        if self.faults.force_embedding_failure() {
            schedule_indexing_retry(
                dvr,
                &self.worker_id,
                job,
                &self.limits,
                "injected_embedding_failure",
                now_us,
            )
            .await?;
            return Ok(EmbeddingPrep::Noop);
        }
        let description = manifest.description().ok_or_else(|| {
            A2aLabError::unavailable("embedding stage requires persisted description")
        })?;
        let description_key =
            FrameDescriptionKey::new(frame.clone(), None, description.provenance().generation());
        let embed_key = TextEmbeddingKey::PersistedDescription(description_key);
        let text = description.text();
        let token_count = u32::try_from(text.len()).unwrap_or(u32::MAX);
        let request = TextEmbeddingRequest::new(embedding_limits, embed_key, text, token_count)
            .map_err(map_indexing_failure)?;
        let source_id = frame.source_id().as_str().to_owned();
        Ok(EmbeddingPrep::Ready(Box::new(EmbeddingWorkItem {
            job: job.clone(),
            manifest,
            request,
            source_id,
        })))
    }

    async fn execute_embedding_provider_batch(
        &self,
        dvr: &IndexedVideoDvr,
        batch: &[EmbeddingWorkItem],
        now_us: i64,
        limits: a2a_lab_dev_kit::TextEmbeddingLimits,
        report: &mut IndexingWorkerBatchReport,
    ) {
        if batch.is_empty() {
            return;
        }
        let provider = self.provider_for(batch[0].source_id.as_str());
        let requests: Vec<TextEmbeddingRequest> =
            batch.iter().map(|item| item.request.clone()).collect();
        let validated = match provider.embed_texts(requests.clone()).await {
            Ok(responses) => {
                match validate_embedding_batch(provider.config(), limits, &requests, &responses) {
                    Ok(validated) => validated,
                    Err(failure) => {
                        for item in batch {
                            let _ = schedule_indexing_retry(
                                dvr,
                                &self.worker_id,
                                &item.job,
                                &self.limits,
                                &failure.to_string(),
                                now_us,
                            )
                            .await;
                            report.record_failure();
                        }
                        return;
                    }
                }
            }
            Err(failure) => {
                let message = failure.to_string();
                for item in batch {
                    let _ = schedule_indexing_retry(
                        dvr,
                        &self.worker_id,
                        &item.job,
                        &self.limits,
                        &message,
                        now_us,
                    )
                    .await;
                    report.record_failure();
                }
                return;
            }
        };
        if self.faults.crash_before_embedding_commit() {
            for item in batch {
                let _ = schedule_indexing_retry(
                    dvr,
                    &self.worker_id,
                    &item.job,
                    &self.limits,
                    "injected crash before embedding commit",
                    now_us,
                )
                .await;
                report.record_failure();
            }
            return;
        }
        for (item, embedding) in batch.iter().zip(validated.iter()) {
            match self.commit_embedding(dvr, item, embedding, now_us).await {
                Ok(true) => report.record_embedding_committed(),
                Ok(false) => {}
                Err(error) => {
                    eprintln!("embedding worker {}: {error}", self.worker_id);
                    let _ = schedule_indexing_retry(
                        dvr,
                        &self.worker_id,
                        &item.job,
                        &self.limits,
                        &error.to_string(),
                        now_us,
                    )
                    .await;
                    report.record_failure();
                }
            }
        }
    }

    async fn commit_embedding(
        &self,
        dvr: &IndexedVideoDvr,
        item: &EmbeddingWorkItem,
        embedding: &KeyedTextEmbedding,
        now_us: i64,
    ) -> Result<bool, A2aLabError> {
        dvr.upsert_frame_embedding(
            item.manifest.frame(),
            embedding.model_space(),
            embedding.provenance(),
            embedding.vector(),
            IndexingCheckpoint::Ready,
        )
        .await?;
        let committed = dvr
            .complete_indexing_embedding_cas(
                item.job.job_id(),
                &self.worker_id,
                item.job.revision_generation(),
                now_us,
            )
            .await?;
        if !committed {
            return Err(A2aLabError::invalid(
                "indexing job",
                "stale worker lost embedding completion race",
            ));
        }
        Ok(true)
    }
}
