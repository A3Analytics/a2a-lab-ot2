//! Serve-time semantic worker and provider wiring.

use std::collections::HashSet;
use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, DecisionEvaluationCoverage, SemanticIndexingConfig, SemanticVideoSearchProvider,
};

use super::factory::{
    build_deterministic_description_provider, build_deterministic_providers,
    build_deterministic_providers_for_source, build_live_description_provider,
    build_live_providers, build_live_providers_for_source,
};
use super::media::EvidenceJpegMedia;
use crate::lab_config::ResolvedSemanticIndexing;
use crate::videos::dvr::{
    AnnotationQueueWorker, AnyDecisionEvaluationWorker, AnyIndexingAnnotationWorker,
    DecisionEvaluationQueueWorker, DeterministicIndexingWorker, DvrSemanticSearchProvider,
    IndexedSourceFilter, IndexedVideoDvr, IndexingWorkerFaults, LiveIndexingWorker,
};
use crate::videos::semantic_indexing::{build_live_decision_worker, jev_live_worker_enabled};

/// Injectable serve overrides for deterministic tests.
#[derive(Default)]
pub struct SemanticServeInjections {
    pub worker: Option<AnnotationQueueWorker>,
    pub search: Option<Arc<dyn SemanticVideoSearchProvider>>,
    pub force_deterministic_worker: bool,
    pub decision_worker: Option<DecisionEvaluationQueueWorker>,
    pub force_deterministic_decision_worker: bool,
}

/// Providers and worker handles registered alongside the live catalog.
#[derive(Clone)]
pub struct SemanticServeBundle {
    pub worker: Option<AnnotationQueueWorker>,
    pub decision_worker: Option<DecisionEvaluationQueueWorker>,
    pub decision_unavailable: Option<(DecisionEvaluationCoverage, &'static str)>,
    pub search: Option<Arc<dyn SemanticVideoSearchProvider>>,
}

impl SemanticServeBundle {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            worker: None,
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
    if let Some(embeddings) = semantic.config.embeddings.as_ref() {
        let _ = dvr.sync_embedding_config(embeddings).await?;
    }

    let search = if semantic.config.embeddings.is_none() {
        None
    } else {
        Some(match injections.search.clone() {
            Some(provider) => provider,
            None => Arc::new(DvrSemanticSearchProvider::new(dvr.clone(), semantic)?),
        })
    };

    let worker = match injections.worker.clone() {
        Some(worker) => Some(worker),
        None => {
            prepare_annotation_worker(
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
        worker,
        decision_worker,
        decision_unavailable,
        search,
    })
}

async fn prepare_annotation_worker(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    sources: &HashSet<String>,
    force_deterministic: bool,
) -> Result<Option<AnnotationQueueWorker>, A2aLabError> {
    if sources.is_empty() {
        return Ok(None);
    }
    let worker_id = format!("ot2-annotation-{}", std::process::id());
    let faults = IndexingWorkerFaults::new();
    if semantic.embeddings.is_none() {
        let inner = if force_deterministic || !live_indexing_enabled() {
            build_description_only_deterministic_worker(semantic, sources, worker_id, faults)
                .await?
        } else {
            AnyIndexingAnnotationWorker::LiveDescriptionOnly(Arc::new(
                build_description_only_live_worker(semantic, dvr, sources, worker_id, faults)
                    .await?,
            ))
        };
        return Ok(Some(AnnotationQueueWorker::new(inner, semantic.clone())));
    }
    let inner = if force_deterministic || !live_indexing_enabled() {
        build_deterministic_worker(semantic, sources, worker_id, Arc::clone(&faults)).await?
    } else {
        match build_live_worker(semantic, dvr, sources, worker_id, Arc::clone(&faults)).await {
            Ok(worker) => AnyIndexingAnnotationWorker::Live(Arc::new(worker)),
            Err(error) => {
                eprintln!("semantic indexing live worker unavailable: {error}");
                return Ok(None);
            }
        }
    };
    Ok(Some(AnnotationQueueWorker::new(inner, semantic.clone())))
}

async fn build_description_only_deterministic_worker(
    semantic: &SemanticIndexingConfig,
    sources: &HashSet<String>,
    worker_id: String,
    faults: Arc<IndexingWorkerFaults>,
) -> Result<AnyIndexingAnnotationWorker, A2aLabError> {
    let provider =
        build_deterministic_description_provider(semantic, Some(semantic.description.model()))
            .await?;
    let mut worker = crate::videos::dvr::DeterministicDescriptionOnlyWorker::new(
        provider,
        semantic.limits.worker.clone(),
        worker_id,
        faults,
    );
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
    );
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

async fn build_deterministic_worker(
    semantic: &SemanticIndexingConfig,
    sources: &HashSet<String>,
    worker_id: String,
    faults: Arc<IndexingWorkerFaults>,
) -> Result<AnyIndexingAnnotationWorker, A2aLabError> {
    let providers = build_deterministic_providers(semantic).await?;
    let mut worker = DeterministicIndexingWorker::new(
        providers,
        semantic.limits.worker.clone(),
        worker_id,
        faults,
    );
    for source_id in sources {
        let schedule = semantic.schedule_for(source_id);
        if schedule.description_model().is_some() || schedule.embedding_model().is_some() {
            worker = worker.with_source_provider(
                source_id,
                build_deterministic_providers_for_source(semantic, &schedule).await?,
            );
        }
    }
    Ok(AnyIndexingAnnotationWorker::Deterministic(Arc::new(worker)))
}

async fn build_live_worker(
    semantic: &SemanticIndexingConfig,
    dvr: &IndexedVideoDvr,
    sources: &HashSet<String>,
    worker_id: String,
    faults: Arc<IndexingWorkerFaults>,
) -> Result<LiveIndexingWorker, A2aLabError> {
    let media = Arc::new(EvidenceJpegMedia::new(Arc::new(dvr.clone())));
    let providers = build_live_providers(semantic, media).await?;
    let mut worker =
        LiveIndexingWorker::new(providers, semantic.limits.worker.clone(), worker_id, faults);
    for source_id in sources {
        let schedule = semantic.schedule_for(source_id);
        if schedule.description_model().is_some() || schedule.embedding_model().is_some() {
            let source_media = Arc::new(EvidenceJpegMedia::new(Arc::new(dvr.clone())));
            worker = worker.with_source_provider(
                source_id,
                build_live_providers_for_source(semantic, source_media, &schedule).await?,
            );
        }
    }
    Ok(worker)
}

fn live_indexing_enabled() -> bool {
    std::env::var("A2ALAB_SEMANTIC_INDEXING_LIVE")
        .is_ok_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
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
