//! Serve-time semantic worker and provider wiring over the generic kit engine.

use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, ChunkDvrSemanticSearchProvider, DecisionEvaluationCoverage,
    DescriptionStageWorker, EmbeddingStageWorker, FakeFrameDescriptionProvider,
    FakeTextEmbeddingProvider, IndexingWorkerBatchReport, IndexingWorkerFaults, IndexingWorkerLane,
    SemanticChunkDvrStore, SemanticIndexingConfig, SemanticVideoSearchProvider,
};

use super::bedrock::live_semantic_indexing_enabled;
use super::factory::{
    build_deterministic_providers, build_live_description_provider, build_live_providers,
};
use super::media::EvidenceJpegMedia;
use crate::lab_config::ResolvedSemanticIndexing;
use crate::videos::dvr::{
    AnnotationQueueWorker, AnnotationQueueWorkerBundle, AnyDecisionEvaluationWorker,
    DecisionEvaluationQueueWorker, IndexedSourceFilter, IndexedVideoDvr, SemanticAnnotationWorkers,
};
use crate::videos::semantic_indexing::{
    ConfiguredTextEmbeddingProvider, LunaFrameDescriptionProvider, build_live_decision_worker,
    jev_live_worker_enabled,
};

/// Injectable serve overrides for deterministic tests.
#[derive(Default)]
pub struct SemanticServeInjections {
    pub workers: Option<SemanticAnnotationWorkers>,
    pub search: Option<Arc<dyn SemanticVideoSearchProvider>>,
    pub force_deterministic_worker: bool,
    pub decision_worker: Option<DecisionEvaluationQueueWorker>,
    pub force_deterministic_decision_worker: bool,
}

/// Providers and worker handles registered alongside the live catalog.
#[derive(Clone)]
pub struct SemanticServeBundle {
    pub workers: Option<SemanticAnnotationWorkers>,
    pub decision_worker: Option<DecisionEvaluationQueueWorker>,
    pub decision_unavailable: Option<(DecisionEvaluationCoverage, &'static str)>,
    pub search: Option<Arc<dyn SemanticVideoSearchProvider>>,
}

impl SemanticServeBundle {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            workers: None,
            decision_worker: None,
            decision_unavailable: None,
            search: None,
        }
    }
}

/// Kit workers injected with OT-2 Luna/Bedrock/Cohere adapters.
pub enum Ot2IndexingWorker {
    DeterministicDescription(DescriptionStageWorker<FakeFrameDescriptionProvider>),
    LiveDescription(DescriptionStageWorker<LunaFrameDescriptionProvider<EvidenceJpegMedia>>),
    DeterministicEmbedding(EmbeddingStageWorker<FakeTextEmbeddingProvider>),
    LiveEmbedding(EmbeddingStageWorker<ConfiguredTextEmbeddingProvider>),
}

impl Ot2IndexingWorker {
    #[must_use]
    pub const fn uses_live_adapters(&self) -> bool {
        matches!(self, Self::LiveDescription(_) | Self::LiveEmbedding(_))
    }

    pub async fn run_batch(
        &self,
        store: &SemanticChunkDvrStore,
        semantic: &SemanticIndexingConfig,
    ) -> Result<IndexingWorkerBatchReport, A2aLabError> {
        match self {
            Self::DeterministicDescription(worker) => worker.run_batch(store, semantic).await,
            Self::LiveDescription(worker) => worker.run_batch(store, semantic).await,
            Self::DeterministicEmbedding(worker) => worker.run_batch(store, semantic).await,
            Self::LiveEmbedding(worker) => worker.run_batch(store, semantic).await,
        }
    }
}

/// Validates storage, configures source filters, and prepares optional worker/search providers.
pub async fn prepare_semantic_serve_bundle(
    semantic: &ResolvedSemanticIndexing,
    dvr: &IndexedVideoDvr,
    injections: &SemanticServeInjections,
) -> Result<SemanticServeBundle, A2aLabError> {
    semantic.config.validate_before_startup()?;
    dvr.set_indexed_source_filter(IndexedSourceFilter::allow_only(
        semantic.indexed_source_ids.clone(),
    ));
    dvr.set_semantic_indexing_config(&semantic.config);

    let search = if semantic.config.embeddings.is_none() {
        None
    } else {
        Some(if let Some(provider) = injections.search.clone() {
            provider
        } else {
            prepare_search_provider(&semantic.config, dvr, injections.force_deterministic_worker)
                .await?
        })
    };

    let workers = match injections.workers.clone() {
        Some(workers) => Some(workers),
        None => {
            prepare_annotation_workers(&semantic.config, dvr, injections.force_deterministic_worker)
                .await?
        }
    };

    let (decision_worker, decision_unavailable) = if semantic.config.embeddings.is_none() {
        (
            None,
            Some((DecisionEvaluationCoverage::NotEvaluated, "description_only")),
        )
    } else {
        match injections.decision_worker.clone() {
            Some(worker) => (Some(worker), None),
            None => prepare_decision_evaluation_worker(&semantic.config, dvr),
        }
    };

    Ok(SemanticServeBundle {
        workers,
        decision_worker,
        decision_unavailable,
        search,
    })
}

async fn prepare_annotation_workers(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    force_deterministic: bool,
) -> Result<Option<SemanticAnnotationWorkers>, A2aLabError> {
    let base_id = format!("ot2-annotation-{}", std::process::id());
    let live_id = format!("{base_id}-{}", IndexingWorkerLane::Live.id_suffix());
    let backfill_id = format!("{base_id}-{}", IndexingWorkerLane::Backfill.id_suffix());
    let faults = IndexingWorkerFaults::new();
    let description_live = match prepare_description_lane_worker(
        semantic,
        dvr,
        force_deterministic,
        format!("{live_id}-description"),
        IndexingWorkerLane::Live,
        Arc::clone(&faults),
    )
    .await
    {
        Ok(worker) => worker,
        Err(error) => {
            eprintln!("semantic indexing description live worker unavailable: {error}");
            return Ok(None);
        }
    };
    let description_backfill = match prepare_description_lane_worker(
        semantic,
        dvr,
        force_deterministic,
        format!("{backfill_id}-description"),
        IndexingWorkerLane::Backfill,
        Arc::clone(&faults),
    )
    .await
    {
        Ok(worker) => worker,
        Err(error) => {
            eprintln!("semantic indexing description backfill worker unavailable: {error}");
            return Ok(None);
        }
    };
    let description = AnnotationQueueWorkerBundle::new(
        AnnotationQueueWorker::new(description_live, semantic.clone()),
        AnnotationQueueWorker::new(description_backfill, semantic.clone()),
    );
    let embedding = if semantic.embeddings.is_some() {
        let embedding_live = match prepare_embedding_lane_worker(
            semantic,
            dvr,
            force_deterministic,
            format!("{live_id}-embedding"),
            IndexingWorkerLane::Live,
            Arc::clone(&faults),
        )
        .await
        {
            Ok(worker) => worker,
            Err(error) => {
                eprintln!("semantic indexing embedding live worker unavailable: {error}");
                return Ok(None);
            }
        };
        let embedding_backfill = match prepare_embedding_lane_worker(
            semantic,
            dvr,
            force_deterministic,
            format!("{backfill_id}-embedding"),
            IndexingWorkerLane::Backfill,
            faults,
        )
        .await
        {
            Ok(worker) => worker,
            Err(error) => {
                eprintln!("semantic indexing embedding backfill worker unavailable: {error}");
                return Ok(None);
            }
        };
        Some(AnnotationQueueWorkerBundle::new(
            AnnotationQueueWorker::new(embedding_live, semantic.clone()),
            AnnotationQueueWorker::new(embedding_backfill, semantic.clone()),
        ))
    } else {
        None
    };
    Ok(Some(SemanticAnnotationWorkers::new(description, embedding)))
}

async fn prepare_description_lane_worker(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    force_deterministic: bool,
    worker_id: String,
    lane: IndexingWorkerLane,
    faults: Arc<IndexingWorkerFaults>,
) -> Result<Ot2IndexingWorker, A2aLabError> {
    if force_deterministic || !live_semantic_indexing_enabled() {
        return Ok(Ot2IndexingWorker::DeterministicDescription(
            DescriptionStageWorker::deterministic(semantic, worker_id, faults, lane)?,
        ));
    }
    let provider = build_live_description_provider(
        semantic,
        Arc::new(EvidenceJpegMedia::new(Arc::new(dvr.clone()))),
        Some(semantic.description.model()),
    )
    .await?;
    Ok(Ot2IndexingWorker::LiveDescription(
        DescriptionStageWorker::new(provider, semantic.limits.worker.clone(), worker_id, faults)
            .with_lane(lane),
    ))
}

async fn prepare_embedding_lane_worker(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    force_deterministic: bool,
    worker_id: String,
    lane: IndexingWorkerLane,
    faults: Arc<IndexingWorkerFaults>,
) -> Result<Ot2IndexingWorker, A2aLabError> {
    if force_deterministic || !live_semantic_indexing_enabled() {
        return Ok(Ot2IndexingWorker::DeterministicEmbedding(
            EmbeddingStageWorker::deterministic(semantic, worker_id, faults, lane)?,
        ));
    }
    let media = Arc::new(EvidenceJpegMedia::new(Arc::new(dvr.clone())));
    let providers = build_live_providers(semantic, media).await?;
    Ok(Ot2IndexingWorker::LiveEmbedding(
        EmbeddingStageWorker::new(
            providers.embedding,
            semantic.limits.worker.clone(),
            worker_id,
            faults,
        )
        .with_lane(lane),
    ))
}

async fn prepare_search_provider(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    force_deterministic: bool,
) -> Result<Arc<dyn SemanticVideoSearchProvider>, A2aLabError> {
    if force_deterministic || !live_semantic_indexing_enabled() {
        let providers = build_deterministic_providers(semantic).await?;
        return Ok(Arc::new(
            ChunkDvrSemanticSearchProvider::new(
                dvr.engine().clone(),
                semantic.clone(),
                providers.embedding,
            )
            .await?,
        ));
    }
    let media = Arc::new(EvidenceJpegMedia::new(Arc::new(dvr.clone())));
    let providers = build_live_providers(semantic, media).await?;
    Ok(Arc::new(
        ChunkDvrSemanticSearchProvider::new(
            dvr.engine().clone(),
            semantic.clone(),
            providers.embedding,
        )
        .await?,
    ))
}

fn prepare_decision_evaluation_worker(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
) -> (
    Option<DecisionEvaluationQueueWorker>,
    Option<(DecisionEvaluationCoverage, &'static str)>,
) {
    if !jev_live_worker_enabled(semantic) {
        eprintln!("Jev live decision worker disabled; decision coverage will be not-evaluated");
        return (
            None,
            Some((
                DecisionEvaluationCoverage::NotEvaluated,
                "jev_live_disabled",
            )),
        );
    }
    match build_live_decision_worker(semantic, Arc::new(dvr.clone())) {
        Ok(worker) => (
            Some(DecisionEvaluationQueueWorker::new(
                AnyDecisionEvaluationWorker::Live(tokio::sync::Mutex::new(worker)),
                semantic.clone(),
            )),
            None,
        ),
        Err(failure) => {
            eprintln!(
                "Jev live decision worker unavailable: {failure}; decision coverage will be failed"
            );
            (
                None,
                Some((DecisionEvaluationCoverage::Failed, "jev_unavailable")),
            )
        }
    }
}
