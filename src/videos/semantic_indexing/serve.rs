//! Serve-time semantic worker and provider wiring.

use std::collections::HashSet;
use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, DecisionEvaluationCoverage, SemanticIndexingConfig, SemanticVideoSearchProvider,
};

use super::bedrock::live_semantic_indexing_enabled;
use super::factory::{
    build_deterministic_description_provider, build_deterministic_providers,
    build_deterministic_providers_for_source, build_deterministic_search_embedder,
    build_live_description_provider, build_live_providers, build_live_providers_for_source,
};
use super::media::EvidenceJpegMedia;
use crate::lab_config::ResolvedSemanticIndexing;
use crate::videos::dvr::{
    AnnotationQueueWorker, AnnotationQueueWorkerBundle, AnyDecisionEvaluationWorker,
    AnyIndexingAnnotationWorker, DecisionEvaluationQueueWorker, DeterministicEmbeddingOnlyWorker,
    DvrSearchEmbedder, DvrSemanticSearchProvider, IndexedSourceFilter, IndexedVideoDvr,
    IndexingWorkerFaults, IndexingWorkerLane, LiveEmbeddingOnlyWorker, SemanticAnnotationWorkers,
};
use crate::videos::semantic_indexing::{build_live_decision_worker, jev_live_worker_enabled};

/// Injectable serve overrides for deterministic tests.
#[derive(Default)]
pub struct SemanticServeInjections {
    pub workers: Option<SemanticAnnotationWorkers>,
    pub search: Option<Arc<dyn SemanticVideoSearchProvider>>,
    pub search_embedder: Option<DvrSearchEmbedder>,
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

/// Validates storage, configures source filters, and prepares optional worker/search providers.
pub async fn prepare_semantic_serve_bundle(
    semantic: &ResolvedSemanticIndexing,
    dvr: &IndexedVideoDvr,
    injections: &SemanticServeInjections,
) -> Result<SemanticServeBundle, A2aLabError> {
    dvr.set_indexed_source_filter(IndexedSourceFilter::allow_only(
        semantic.indexed_source_ids.clone(),
    ));
    dvr.set_semantic_indexing_config(semantic.config.clone());
    if let Some(embeddings) = semantic.config.embeddings.as_ref() {
        let _ = dvr.sync_embedding_config(embeddings).await?;
    }

    let search = if semantic.config.embeddings.is_none() {
        None
    } else {
        Some(if let Some(provider) = injections.search.clone() {
            provider
        } else {
            let embedder = match injections.search_embedder.clone() {
                Some(embedder) => embedder,
                None => {
                    prepare_search_embedder(
                        &semantic.config,
                        dvr,
                        injections.force_deterministic_worker,
                    )
                    .await?
                }
            };
            Arc::new(DvrSemanticSearchProvider::new(
                dvr.clone(),
                semantic,
                embedder,
            )?)
        })
    };

    let workers = match injections.workers.clone() {
        Some(workers) => Some(workers),
        None => {
            prepare_annotation_workers(
                &semantic.config,
                dvr,
                &semantic.indexed_source_ids,
                injections.force_deterministic_worker,
            )
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
            None => prepare_decision_evaluation_worker(
                &semantic.config,
                dvr,
                &semantic.indexed_source_ids,
            ),
        }
    };

    Ok(SemanticServeBundle {
        workers,
        decision_worker,
        decision_unavailable,
        search,
    })
}

async fn prepare_annotation_lane_worker(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    sources: &HashSet<String>,
    force_deterministic: bool,
    worker_id: String,
    lane: IndexingWorkerLane,
    faults: Arc<IndexingWorkerFaults>,
) -> Result<AnyIndexingAnnotationWorker, A2aLabError> {
    if force_deterministic || !live_semantic_indexing_enabled() {
        return build_description_only_deterministic_worker(
            semantic, sources, worker_id, faults, lane,
        )
        .await;
    }
    Ok(AnyIndexingAnnotationWorker::LiveDescriptionOnly(Arc::new(
        build_description_only_live_worker(semantic, dvr, sources, worker_id, faults, lane).await?,
    )))
}

async fn prepare_annotation_workers(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    sources: &HashSet<String>,
    force_deterministic: bool,
) -> Result<Option<SemanticAnnotationWorkers>, A2aLabError> {
    if sources.is_empty() {
        return Ok(None);
    }
    let base_id = format!("ot2-annotation-{}", std::process::id());
    let live_id = format!("{base_id}-{}", IndexingWorkerLane::Live.id_suffix());
    let backfill_id = format!("{base_id}-{}", IndexingWorkerLane::Backfill.id_suffix());
    let faults = IndexingWorkerFaults::new();
    let description_live = match prepare_description_lane_worker(
        semantic,
        dvr,
        sources,
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
        sources,
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
            sources,
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
            sources,
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
    sources: &HashSet<String>,
    force_deterministic: bool,
    worker_id: String,
    lane: IndexingWorkerLane,
    faults: Arc<IndexingWorkerFaults>,
) -> Result<AnyIndexingAnnotationWorker, A2aLabError> {
    prepare_annotation_lane_worker(
        semantic,
        dvr,
        sources,
        force_deterministic,
        worker_id,
        lane,
        faults,
    )
    .await
}

async fn prepare_embedding_lane_worker(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    sources: &HashSet<String>,
    force_deterministic: bool,
    worker_id: String,
    lane: IndexingWorkerLane,
    faults: Arc<IndexingWorkerFaults>,
) -> Result<AnyIndexingAnnotationWorker, A2aLabError> {
    if force_deterministic || !live_semantic_indexing_enabled() {
        return build_embedding_only_deterministic_worker(
            semantic, sources, worker_id, faults, lane,
        )
        .await;
    }
    Ok(AnyIndexingAnnotationWorker::LiveEmbeddingOnly(Arc::new(
        build_embedding_only_live_worker(semantic, dvr, sources, worker_id, faults, lane).await?,
    )))
}

async fn build_description_only_deterministic_worker(
    semantic: &SemanticIndexingConfig,
    sources: &HashSet<String>,
    worker_id: String,
    faults: Arc<IndexingWorkerFaults>,
    lane: IndexingWorkerLane,
) -> Result<AnyIndexingAnnotationWorker, A2aLabError> {
    let provider =
        build_deterministic_description_provider(semantic, Some(semantic.description.model()))
            .await?;
    let mut worker = crate::videos::dvr::DeterministicDescriptionOnlyWorker::new(
        provider,
        semantic.limits.worker.clone(),
        worker_id,
        faults,
    )
    .with_lane(lane);
    for source_id in sources {
        let schedule = semantic.schedule_for(source_id);
        if schedule.description_model().is_some() {
            worker = worker.with_source_provider(
                source_id,
                build_deterministic_description_provider(semantic, schedule.description_model())
                    .await?,
            );
        }
    }
    Ok(AnyIndexingAnnotationWorker::DeterministicDescriptionOnly(
        Arc::new(worker),
    ))
}

async fn build_description_only_live_worker(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    sources: &HashSet<String>,
    worker_id: String,
    faults: Arc<IndexingWorkerFaults>,
    lane: IndexingWorkerLane,
) -> Result<crate::videos::dvr::LiveDescriptionOnlyWorker, A2aLabError> {
    let provider = build_live_description_provider(
        semantic,
        Arc::new(EvidenceJpegMedia::new(Arc::new(dvr.clone()))),
        Some(semantic.description.model()),
    )
    .await?;
    let mut worker = crate::videos::dvr::LiveDescriptionOnlyWorker::new(
        provider,
        semantic.limits.worker.clone(),
        worker_id,
        faults,
    )
    .with_lane(lane);
    for source_id in sources {
        let schedule = semantic.schedule_for(source_id);
        if schedule.description_model().is_some() {
            worker = worker.with_source_provider(
                source_id,
                build_live_description_provider(
                    semantic,
                    Arc::new(EvidenceJpegMedia::new(Arc::new(dvr.clone()))),
                    schedule.description_model(),
                )
                .await?,
            );
        }
    }
    Ok(worker)
}

async fn build_embedding_only_deterministic_worker(
    semantic: &SemanticIndexingConfig,
    sources: &HashSet<String>,
    worker_id: String,
    faults: Arc<IndexingWorkerFaults>,
    lane: IndexingWorkerLane,
) -> Result<AnyIndexingAnnotationWorker, A2aLabError> {
    let providers = build_deterministic_providers(semantic).await?;
    let mut worker = DeterministicEmbeddingOnlyWorker::new(
        providers.embedding,
        semantic.limits.worker.clone(),
        worker_id,
        faults,
    )
    .with_lane(lane);
    for source_id in sources {
        let schedule = semantic.schedule_for(source_id);
        if schedule.embedding_model().is_some() {
            worker = worker.with_source_provider(
                source_id,
                build_deterministic_providers_for_source(semantic, &schedule)
                    .await?
                    .embedding,
            );
        }
    }
    Ok(AnyIndexingAnnotationWorker::DeterministicEmbeddingOnly(
        Arc::new(worker),
    ))
}

async fn build_embedding_only_live_worker(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    sources: &HashSet<String>,
    worker_id: String,
    faults: Arc<IndexingWorkerFaults>,
    lane: IndexingWorkerLane,
) -> Result<LiveEmbeddingOnlyWorker, A2aLabError> {
    let media = Arc::new(EvidenceJpegMedia::new(Arc::new(dvr.clone())));
    let providers = build_live_providers(semantic, media).await?;
    let mut worker = LiveEmbeddingOnlyWorker::new(
        providers.embedding,
        semantic.limits.worker.clone(),
        worker_id,
        faults,
    )
    .with_lane(lane);
    for source_id in sources {
        let schedule = semantic.schedule_for(source_id);
        if schedule.embedding_model().is_some() {
            let source_media = Arc::new(EvidenceJpegMedia::new(Arc::new(dvr.clone())));
            worker = worker.with_source_provider(
                source_id,
                build_live_providers_for_source(semantic, source_media, &schedule)
                    .await?
                    .embedding,
            );
        }
    }
    Ok(worker)
}

async fn prepare_search_embedder(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    force_deterministic: bool,
) -> Result<DvrSearchEmbedder, A2aLabError> {
    if force_deterministic || !live_semantic_indexing_enabled() {
        return build_deterministic_search_embedder(semantic).await;
    }
    let media = Arc::new(EvidenceJpegMedia::new(Arc::new(dvr.clone())));
    Ok(DvrSearchEmbedder::Live(Arc::new(
        build_live_providers(semantic, media).await?.embedding,
    )))
}

fn prepare_decision_evaluation_worker(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    sources: &HashSet<String>,
) -> (
    Option<DecisionEvaluationQueueWorker>,
    Option<(DecisionEvaluationCoverage, &'static str)>,
) {
    if sources.is_empty() {
        return (None, None);
    }
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
