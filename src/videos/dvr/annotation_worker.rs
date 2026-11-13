//! One bounded worker pass: Luna description then text embedding (retrieval only).

use std::collections::HashMap;
use std::sync::Arc;

use crate::videos::semantic_indexing::ConfiguredTextEmbeddingProvider;
use crate::videos::semantic_indexing::EvidenceJpegMedia;
use crate::videos::semantic_indexing::LunaFrameDescriptionProvider;
use a2a_lab_dev_kit::{
    A2aLabError, FakeFrameDescriptionProvider, FakeTextEmbeddingProvider, FrameDescriptionProvider,
    FrameDescriptionRequest, ProvenanceGeneration, SemanticIndexingConfig, SemanticIndexingFailure,
    SemanticWorkerLimits, TextEmbeddingKey, TextEmbeddingProvider, TextEmbeddingRequest,
    validate_description_batch, validate_embedding_batch,
};

use super::frame_manifest::{
    IndexingCheckpoint, ManifestRetainedState, MediaPayloadStatus, SemanticFrameManifestRecord,
};
use super::indexing_job_store::{IndexingJobRecord, IndexingJobState};
use super::indexing_registration::{DEFAULT_RECONCILE_BATCH, IndexingWorkerLane};
use super::store::IndexedVideoDvr;
use super::worker_faults::IndexingWorkerFaults;
use crate::videos::semantic_indexing::{SemanticIndexingProviders, build_deterministic_providers};
use crate::videos::semantic_indexing::{
    frame_description_limits, indexing_claim_batch_size, text_embedding_limits,
};
use futures_util::future::join_all;

/// Outcome of one worker batch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexingWorkerBatchReport {
    reclaimed_leases: u32,
    claimed: u32,
    descriptions_committed: u32,
    embeddings_committed: u32,
    failures: u32,
    backlog_over_limit: bool,
    lane: IndexingWorkerLane,
}

impl IndexingWorkerBatchReport {
    #[must_use]
    pub const fn reclaimed_leases(&self) -> u32 {
        self.reclaimed_leases
    }

    #[must_use]
    pub const fn claimed(&self) -> u32 {
        self.claimed
    }

    #[must_use]
    pub const fn descriptions_committed(&self) -> u32 {
        self.descriptions_committed
    }

    #[must_use]
    pub const fn embeddings_committed(&self) -> u32 {
        self.embeddings_committed
    }

    #[must_use]
    pub const fn failures(&self) -> u32 {
        self.failures
    }

    #[must_use]
    pub const fn backlog_over_limit(&self) -> bool {
        self.backlog_over_limit
    }

    #[must_use]
    pub const fn lane(&self) -> IndexingWorkerLane {
        self.lane
    }
}

/// Deterministic retrieval-only providers plus limits for one DVR worker loop.
pub struct IndexingAnnotationWorker<D, E> {
    providers: SemanticIndexingProviders<D, E>,
    source_providers: HashMap<String, SemanticIndexingProviders<D, E>>,
    limits: SemanticWorkerLimits,
    worker_id: String,
    lane: IndexingWorkerLane,
    faults: Arc<IndexingWorkerFaults>,
}

pub type DeterministicIndexingWorker =
    IndexingAnnotationWorker<FakeFrameDescriptionProvider, FakeTextEmbeddingProvider>;

pub type LiveIndexingWorker = IndexingAnnotationWorker<
    LunaFrameDescriptionProvider<EvidenceJpegMedia>,
    ConfiguredTextEmbeddingProvider,
>;

/// Description-only worker; it has no embedding provider type or instance.
pub type DeterministicDescriptionOnlyWorker =
    DescriptionOnlyAnnotationWorker<FakeFrameDescriptionProvider>;
pub type LiveDescriptionOnlyWorker =
    DescriptionOnlyAnnotationWorker<LunaFrameDescriptionProvider<EvidenceJpegMedia>>;

/// Unified worker handle for serve and tests.
pub enum AnyIndexingAnnotationWorker {
    Deterministic(Arc<DeterministicIndexingWorker>),
    Live(Arc<LiveIndexingWorker>),
    DeterministicDescriptionOnly(Arc<DeterministicDescriptionOnlyWorker>),
    LiveDescriptionOnly(Arc<LiveDescriptionOnlyWorker>),
}

impl AnyIndexingAnnotationWorker {
    /// Whether indexing uses live Luna/embedding adapters instead of fakes.
    #[must_use]
    pub fn uses_live_adapters(&self) -> bool {
        matches!(self, Self::Live(_) | Self::LiveDescriptionOnly(_))
    }

    /// Runs one bounded worker pass for the configured provider pair.
    pub async fn run_batch(
        &self,
        dvr: &IndexedVideoDvr,
        semantic: &SemanticIndexingConfig,
    ) -> Result<IndexingWorkerBatchReport, A2aLabError> {
        match self {
            Self::Deterministic(worker) => worker.as_ref().run_batch(dvr, semantic).await,
            Self::Live(worker) => worker.as_ref().run_batch(dvr, semantic).await,
            Self::DeterministicDescriptionOnly(worker) => {
                worker.as_ref().run_batch(dvr, semantic).await
            }
            Self::LiveDescriptionOnly(worker) => worker.as_ref().run_batch(dvr, semantic).await,
        }
    }
}

impl<D, E> IndexingAnnotationWorker<D, E>
where
    D: FrameDescriptionProvider,
    E: TextEmbeddingProvider,
{
    /// Wires description and embedding providers with worker limits.
    #[must_use]
    pub fn new(
        providers: SemanticIndexingProviders<D, E>,
        limits: SemanticWorkerLimits,
        worker_id: impl Into<String>,
        faults: Arc<IndexingWorkerFaults>,
    ) -> Self {
        Self {
            providers,
            source_providers: HashMap::new(),
            limits,
            worker_id: worker_id.into(),
            lane: IndexingWorkerLane::Live,
            faults,
        }
    }

    /// Pins this worker to one durable enqueue-priority lane.
    #[must_use]
    pub const fn with_lane(mut self, lane: IndexingWorkerLane) -> Self {
        self.lane = lane;
        self
    }

    /// Adds a source-specific provider pair while retaining one queue.
    #[must_use]
    pub fn with_source_provider(
        mut self,
        source_id: impl Into<String>,
        providers: SemanticIndexingProviders<D, E>,
    ) -> Self {
        self.source_providers.insert(source_id.into(), providers);
        self
    }
}

/// One bounded Luna-description pass with no embedding provider.
pub struct DescriptionOnlyAnnotationWorker<D> {
    provider: D,
    source_providers: HashMap<String, D>,
    limits: SemanticWorkerLimits,
    worker_id: String,
    lane: IndexingWorkerLane,
    faults: Arc<IndexingWorkerFaults>,
}

impl<D> DescriptionOnlyAnnotationWorker<D>
where
    D: FrameDescriptionProvider,
{
    #[must_use]
    pub fn new(
        provider: D,
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
    pub fn with_source_provider(mut self, source_id: impl Into<String>, provider: D) -> Self {
        self.source_providers.insert(source_id.into(), provider);
        self
    }

    fn provider_for(&self, source_id: &str) -> &D {
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
        let mut report = IndexingWorkerBatchReport {
            reclaimed_leases: reclaimed,
            claimed: u32::try_from(claimed.len()).unwrap_or(u32::MAX),
            backlog_over_limit: queue.backlog_over_limit,
            lane: self.lane,
            ..Default::default()
        };
        let description_limits = frame_description_limits(&semantic.limits);
        let concurrency = usize::try_from(self.limits.max_concurrency)
            .unwrap_or(1)
            .max(1);
        for chunk in claimed.chunks(concurrency) {
            let outcomes = join_all(
                chunk
                    .iter()
                    .map(|job| self.process_one(dvr, semantic, job, now_us, description_limits)),
            )
            .await;
            for outcome in outcomes {
                match outcome {
                    Ok(committed) => {
                        if committed {
                            report.descriptions_committed =
                                report.descriptions_committed.saturating_add(1);
                        }
                    }
                    Err(error) => {
                        eprintln!("description worker {}: {error}", self.worker_id);
                        report.failures = report.failures.saturating_add(1);
                    }
                }
            }
        }
        Ok(report)
    }

    async fn process_one(
        &self,
        dvr: &IndexedVideoDvr,
        semantic: &SemanticIndexingConfig,
        job: &IndexingJobRecord,
        now_us: i64,
        limits: a2a_lab_dev_kit::FrameDescriptionLimits,
    ) -> Result<bool, A2aLabError> {
        let manifest = dvr.get_manifest_frame(job.frame_id()).await?;
        let frame = manifest.frame();
        if manifest.retained_state() != ManifestRetainedState::Retained
            || !manifest.search_current()
            || manifest.payload_status() != MediaPayloadStatus::Available
        {
            dvr.expire_indexing_job(job.job_id(), now_us).await?;
            return Ok(false);
        }
        let schedule = semantic.schedule_for(frame.source_id().as_str());
        if !dvr
            .select_semantic_stage_frame(
                frame.source_id().as_str(),
                frame.track_id().as_str(),
                "description",
                frame.captured_at().as_micros(),
                schedule.description(),
            )
            .await?
        {
            dvr.release_indexing_job(job.job_id(), &self.worker_id)
                .await?;
            return Ok(false);
        }
        if let Some(stored) = manifest.description() {
            let committed = dvr
                .complete_indexing_description_cas(
                    job.job_id(),
                    &self.worker_id,
                    job.revision_generation(),
                    stored.provenance().generation().get(),
                    now_us,
                )
                .await?;
            return Ok(committed);
        }
        let generation = ProvenanceGeneration::new(1)
            .map_err(|error| A2aLabError::invalid("generation", error.to_string()))?;
        let key = a2a_lab_dev_kit::FrameDescriptionKey::new(frame.clone(), None, generation);
        let request = FrameDescriptionRequest::new(
            limits,
            key,
            limits.max_input_bytes().min(16_384),
            limits.max_pixels().min(640 * 480),
        )
        .map_err(map_indexing_failure)?;
        let provider = self.provider_for(frame.source_id().as_str());
        let responses = provider
            .describe_frames(vec![request.clone()])
            .await
            .map_err(map_indexing_failure)?;
        let validated =
            validate_description_batch(limits, std::slice::from_ref(&request), &responses)
                .map_err(map_indexing_failure)?;
        let item = validated
            .first()
            .ok_or_else(|| A2aLabError::unavailable("empty description batch"))?;
        if self.faults.crash_before_description_commit() {
            return Err(A2aLabError::unavailable(
                "injected crash before description commit",
            ));
        }
        let provenance = item.to_provenance()?;
        dvr.upsert_frame_description(frame, item.text(), &provenance)
            .await?;
        let committed = dvr
            .complete_indexing_description_cas(
                job.job_id(),
                &self.worker_id,
                job.revision_generation(),
                provenance.generation().get(),
                now_us,
            )
            .await?;
        Ok(committed)
    }
}

impl DeterministicIndexingWorker {
    /// Wires deterministic dev-kit fakes from validated semantic configuration.
    pub async fn deterministic(
        semantic: &SemanticIndexingConfig,
        worker_id: impl Into<String>,
        faults: Arc<IndexingWorkerFaults>,
    ) -> Result<Self, A2aLabError> {
        Self::deterministic_lane(semantic, worker_id, faults, IndexingWorkerLane::Live).await
    }

    pub async fn deterministic_lane(
        semantic: &SemanticIndexingConfig,
        worker_id: impl Into<String>,
        faults: Arc<IndexingWorkerFaults>,
        lane: IndexingWorkerLane,
    ) -> Result<Self, A2aLabError> {
        let providers = build_deterministic_providers(semantic).await?;
        Ok(IndexingAnnotationWorker::new(
            providers,
            semantic.limits.worker.clone(),
            worker_id,
            faults,
        )
        .with_lane(lane))
    }
}

impl<D, E> IndexingAnnotationWorker<D, E>
where
    D: FrameDescriptionProvider,
    E: TextEmbeddingProvider,
{
    #[must_use]
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    #[must_use]
    pub fn faults(&self) -> &Arc<IndexingWorkerFaults> {
        &self.faults
    }

    #[must_use]
    pub fn limits(&self) -> &SemanticWorkerLimits {
        &self.limits
    }

    fn providers_for(&self, source_id: &str) -> &SemanticIndexingProviders<D, E> {
        self.source_providers
            .get(source_id)
            .unwrap_or(&self.providers)
    }

    /// Runs reclaim, claim, describe, and embed for up to `max_batch_items`.
    pub async fn run_batch(
        &self,
        dvr: &IndexedVideoDvr,
        semantic: &SemanticIndexingConfig,
    ) -> Result<IndexingWorkerBatchReport, A2aLabError> {
        let now = crate::time::now()?;
        let now_us = now.as_micros();
        let reclaimed = dvr.reclaim_expired_indexing_leases(now_us).await?;
        let queue = dvr.indexing_worker_queue_stats(&self.limits).await?;
        dvr.clear_legacy_materialization_pause().await?;
        let lease_deadline_us = now_us.saturating_add(
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
            .claim_indexing_jobs_for_lane(
                &self.worker_id,
                claim_batch,
                lease_deadline_us,
                now_us,
                self.lane,
            )
            .await?;
        let mut report = IndexingWorkerBatchReport {
            reclaimed_leases: reclaimed,
            claimed: u32::try_from(claimed.len()).unwrap_or(u32::MAX),
            backlog_over_limit: queue.backlog_over_limit,
            lane: self.lane,
            ..IndexingWorkerBatchReport::default()
        };
        let description_limits = frame_description_limits(&semantic.limits);
        let embedding_limits = text_embedding_limits(&semantic.limits);
        let concurrency = usize::try_from(self.limits.max_concurrency)
            .unwrap_or(1)
            .max(1);
        for chunk in claimed.chunks(concurrency) {
            let outcomes = join_all(chunk.iter().map(|job| {
                self.process_one_job(
                    dvr,
                    semantic,
                    job,
                    now_us,
                    description_limits,
                    embedding_limits,
                )
            }))
            .await;
            for outcome in outcomes {
                match outcome {
                    Ok(job_outcome) => {
                        if job_outcome.description_committed {
                            report.descriptions_committed =
                                report.descriptions_committed.saturating_add(1);
                        }
                        if job_outcome.embedding_committed {
                            report.embeddings_committed =
                                report.embeddings_committed.saturating_add(1);
                        }
                    }
                    Err(error) => {
                        eprintln!("indexing worker {}: {error}", self.worker_id);
                        report.failures = report.failures.saturating_add(1);
                    }
                }
            }
        }
        Ok(report)
    }

    async fn process_one_job(
        &self,
        dvr: &IndexedVideoDvr,
        semantic: &SemanticIndexingConfig,
        job: &IndexingJobRecord,
        now_us: i64,
        description_limits: a2a_lab_dev_kit::FrameDescriptionLimits,
        embedding_limits: a2a_lab_dev_kit::TextEmbeddingLimits,
    ) -> Result<JobOutcome, A2aLabError> {
        let manifest = dvr.get_manifest_frame(job.frame_id()).await?;
        if manifest.retained_state() != ManifestRetainedState::Retained
            || !manifest.search_current()
            || manifest.payload_status() != MediaPayloadStatus::Available
        {
            dvr.expire_indexing_job(job.job_id(), now_us).await?;
            return Ok(JobOutcome::default());
        }
        let schedule = semantic.schedule_for(manifest.frame().source_id().as_str());
        let start_us = manifest.frame().captured_at().as_micros();
        let description_due = dvr
            .select_semantic_stage_frame(
                manifest.frame().source_id().as_str(),
                manifest.frame().track_id().as_str(),
                "description",
                start_us,
                schedule.description(),
            )
            .await?;
        let embedding_due = dvr
            .select_semantic_stage_frame(
                manifest.frame().source_id().as_str(),
                manifest.frame().track_id().as_str(),
                "embeddings",
                start_us,
                schedule.embeddings(),
            )
            .await?;
        if !description_due && !embedding_due {
            dvr.release_indexing_job(job.job_id(), self.worker_id())
                .await?;
            return Ok(JobOutcome::default());
        }
        if manifest.description_checkpoint() == IndexingCheckpoint::Ready {
            if embedding_due {
                self.run_embedding_stage(dvr, semantic, job, &manifest, now_us, embedding_limits)
                    .await
            } else {
                dvr.release_indexing_job(job.job_id(), self.worker_id())
                    .await?;
                Ok(JobOutcome::default())
            }
        } else if !description_due {
            // Embedding selection never creates Luna text. The description
            // stage will make this frame eligible when its own schedule is due.
            dvr.release_indexing_job(job.job_id(), self.worker_id())
                .await?;
            Ok(JobOutcome::default())
        } else {
            self.run_description_stage(
                dvr,
                job,
                &manifest,
                now_us,
                description_limits,
                embedding_limits,
                semantic,
                embedding_due,
            )
            .await
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn run_description_stage(
        &self,
        dvr: &IndexedVideoDvr,
        job: &IndexingJobRecord,
        manifest: &SemanticFrameManifestRecord,
        now_us: i64,
        description_limits: a2a_lab_dev_kit::FrameDescriptionLimits,
        embedding_limits: a2a_lab_dev_kit::TextEmbeddingLimits,
        semantic: &SemanticIndexingConfig,
        run_embedding: bool,
    ) -> Result<JobOutcome, A2aLabError> {
        let frame = manifest.frame();
        let providers = self.providers_for(frame.source_id().as_str());
        if let Some(stored) = manifest.description() {
            let generation = stored.provenance().generation().get();
            let committed = dvr
                .complete_indexing_description_cas(
                    job.job_id(),
                    self.worker_id(),
                    job.revision_generation(),
                    generation,
                    now_us,
                )
                .await?;
            if !committed {
                return Err(A2aLabError::invalid(
                    "indexing job",
                    "stale worker lost idempotent description race",
                ));
            }
            let refreshed = dvr.get_manifest_frame(job.frame_id()).await?;
            let mut outcome = JobOutcome {
                description_committed: true,
                ..JobOutcome::default()
            };
            if run_embedding {
                let embed = self
                    .run_embedding_stage(dvr, semantic, job, &refreshed, now_us, embedding_limits)
                    .await?;
                outcome.embedding_committed = embed.embedding_committed;
            } else {
                dvr.release_indexing_job(job.job_id(), self.worker_id())
                    .await?;
            }
            return Ok(outcome);
        }

        let generation = ProvenanceGeneration::new(1)
            .map_err(|error| A2aLabError::invalid("generation", error.to_string()))?;
        let key = a2a_lab_dev_kit::FrameDescriptionKey::new(frame.clone(), None, generation);
        let request = FrameDescriptionRequest::new(
            description_limits,
            key,
            description_limits.max_input_bytes().min(16_384),
            description_limits.max_pixels().min(640 * 480),
        )
        .map_err(map_indexing_failure)?;
        let responses = providers
            .description
            .describe_frames(vec![request.clone()])
            .await
            .map_err(map_indexing_failure)?;
        let validated = validate_description_batch(
            description_limits,
            std::slice::from_ref(&request),
            &responses,
        )
        .map_err(map_indexing_failure)?;
        let item = validated
            .first()
            .ok_or_else(|| A2aLabError::unavailable("empty description batch"))?;
        if self.faults.crash_before_description_commit() {
            return Err(A2aLabError::unavailable(
                "injected crash before description commit",
            ));
        }
        let provenance = item.to_provenance()?;
        dvr.upsert_frame_description(frame, item.text(), &provenance)
            .await?;
        let committed = dvr
            .complete_indexing_description_cas(
                job.job_id(),
                self.worker_id(),
                job.revision_generation(),
                provenance.generation().get(),
                now_us,
            )
            .await?;
        if !committed {
            return Err(A2aLabError::invalid(
                "indexing job",
                "stale worker lost description completion race",
            ));
        }
        if self.faults.crash_after_description_commit() {
            return Err(A2aLabError::unavailable(
                "injected crash after description commit",
            ));
        }
        let refreshed = dvr.get_manifest_frame(job.frame_id()).await?;
        let mut outcome = JobOutcome {
            description_committed: true,
            ..JobOutcome::default()
        };
        if run_embedding {
            let embed = self
                .run_embedding_stage(dvr, semantic, job, &refreshed, now_us, embedding_limits)
                .await?;
            outcome.embedding_committed = embed.embedding_committed;
        } else {
            dvr.release_indexing_job(job.job_id(), self.worker_id())
                .await?;
        }
        Ok(outcome)
    }

    async fn run_embedding_stage(
        &self,
        dvr: &IndexedVideoDvr,
        semantic: &SemanticIndexingConfig,
        job: &IndexingJobRecord,
        manifest: &SemanticFrameManifestRecord,
        now_us: i64,
        embedding_limits: a2a_lab_dev_kit::TextEmbeddingLimits,
    ) -> Result<JobOutcome, A2aLabError> {
        let job = self.active_job_lease(dvr, job, now_us).await?;
        if manifest.embedding_checkpoint() == IndexingCheckpoint::Ready {
            let _ = dvr
                .complete_indexing_embedding_cas(
                    job.job_id(),
                    self.worker_id(),
                    job.revision_generation(),
                    now_us,
                )
                .await?;
            return Ok(JobOutcome {
                embedding_committed: true,
                ..JobOutcome::default()
            });
        }
        let description = manifest.description().ok_or_else(|| {
            A2aLabError::unavailable("embedding stage requires persisted description")
        })?;
        let text = description.text();
        if self.faults.force_embedding_failure() {
            self.schedule_retry(dvr, &job, "injected_embedding_failure", now_us)
                .await?;
            return Ok(JobOutcome::default());
        }
        let description_key = a2a_lab_dev_kit::FrameDescriptionKey::new(
            manifest.frame().clone(),
            None,
            description.provenance().generation(),
        );
        let embed_key = TextEmbeddingKey::PersistedDescription(description_key);
        let token_count = u32::try_from(text.len()).unwrap_or(u32::MAX);
        let request = TextEmbeddingRequest::new(embedding_limits, embed_key, text, token_count)
            .map_err(map_indexing_failure)?;
        let responses = self
            .providers_for(manifest.frame().source_id().as_str())
            .embedding
            .embed_texts(vec![request.clone()])
            .await
            .map_err(map_indexing_failure)?;
        let validated = validate_embedding_batch(
            self.providers_for(manifest.frame().source_id().as_str())
                .embedding
                .config(),
            embedding_limits,
            std::slice::from_ref(&request),
            &responses,
        )
        .map_err(map_indexing_failure)?;
        let item = validated
            .first()
            .ok_or_else(|| A2aLabError::unavailable("empty embedding batch"))?;
        if self.faults.crash_before_embedding_commit() {
            return Err(A2aLabError::unavailable(
                "injected crash before embedding commit",
            ));
        }
        dvr.upsert_frame_embedding(
            manifest.frame(),
            item.model_space(),
            item.provenance(),
            item.vector(),
            IndexingCheckpoint::Ready,
        )
        .await?;
        let committed = dvr
            .complete_indexing_embedding_cas(
                job.job_id(),
                self.worker_id(),
                job.revision_generation(),
                now_us,
            )
            .await?;
        if !committed {
            return Err(A2aLabError::invalid(
                "indexing job",
                "stale worker lost embedding completion race",
            ));
        }
        let _ = semantic;
        Ok(JobOutcome {
            embedding_committed: true,
            ..JobOutcome::default()
        })
    }

    async fn active_job_lease(
        &self,
        dvr: &IndexedVideoDvr,
        job: &IndexingJobRecord,
        now_us: i64,
    ) -> Result<IndexingJobRecord, A2aLabError> {
        let current = dvr.get_indexing_job(job.job_id()).await?;
        if current.state() == IndexingJobState::Leased
            && current.lease_owner() == Some(self.worker_id())
        {
            return Ok(current);
        }
        let lease_deadline_us = now_us.saturating_add(
            i64::try_from(self.limits.job_deadline.as_secs())
                .unwrap_or(i64::MAX)
                .saturating_mul(1_000_000),
        );
        dvr.try_claim_one_job(
            job.job_id(),
            job.frame_id(),
            self.worker_id(),
            lease_deadline_us,
            now_us,
        )
        .await?
        .ok_or_else(|| {
            A2aLabError::unavailable(format!(
                "could not lease indexing job {} for embedding",
                job.job_id()
            ))
        })
    }

    async fn schedule_retry(
        &self,
        dvr: &IndexedVideoDvr,
        job: &IndexingJobRecord,
        reason: &str,
        now_us: i64,
    ) -> Result<(), A2aLabError> {
        let backoff = i64::try_from(self.limits.retry_backoff.as_secs())
            .unwrap_or(0)
            .saturating_mul(1_000_000);
        dvr.fail_indexing_job_lease(
            job.job_id(),
            self.worker_id(),
            job.revision_generation(),
            reason,
            true,
            now_us.saturating_add(backoff),
            now_us,
            &self.limits,
        )
        .await
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct JobOutcome {
    description_committed: bool,
    embedding_committed: bool,
}

fn map_indexing_failure(failure: SemanticIndexingFailure) -> A2aLabError {
    failure.into_lab_error()
}
