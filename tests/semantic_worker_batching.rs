//! Provider batching and partial-batch failure handling for split stage workers.

use std::sync::{Arc, Mutex};

use a2a_lab_dev_kit::{
    A2aLabConfig, DvrStoreNamespace, FakeDescriptionBehavior, FakeEmbeddingBehavior,
    FakeFrameDescriptionProvider, FakeTextEmbeddingProvider, FrameDescriptionProvider,
    FrameDescriptionRequest, KeyedFrameDescription, SemanticStreamSchedule, TextEmbeddingProvider,
    TextEmbeddingRequest, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DescriptionOnlyAnnotationWorker,
    DeterministicDescriptionOnlyWorker, EmbeddingOnlyAnnotationWorker, IndexingCheckpoint,
    IndexingJobState, IndexingWorkerFaults, IndexingWorkerLane,
};
use a2a_lab_ot2::videos::semantic_indexing::{
    build_deterministic_providers, description_provider_config, embedding_provider_config,
    frame_description_limits, text_embedding_limits,
};

struct DescribeBatchSizes {
    inner: FakeFrameDescriptionProvider,
    sizes: Arc<Mutex<Vec<usize>>>,
}

impl DescribeBatchSizes {
    fn new(inner: FakeFrameDescriptionProvider) -> (Self, Arc<Mutex<Vec<usize>>>) {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                inner,
                sizes: Arc::clone(&sizes),
            },
            sizes,
        )
    }
}

#[allow(clippy::unused_async_trait_impl)]
impl FrameDescriptionProvider for DescribeBatchSizes {
    fn config(&self) -> &a2a_lab_dev_kit::FrameDescriptionProviderConfig {
        self.inner.config()
    }

    fn limits(&self) -> a2a_lab_dev_kit::FrameDescriptionLimits {
        self.inner.limits()
    }

    async fn describe_frames(
        &self,
        requests: Vec<FrameDescriptionRequest>,
    ) -> Result<Vec<KeyedFrameDescription>, a2a_lab_dev_kit::SemanticIndexingFailure> {
        self.sizes.lock().unwrap().push(requests.len());
        self.inner.describe_frames(requests).await
    }
}

struct EmbedBatchSizes {
    inner: FakeTextEmbeddingProvider,
    sizes: Arc<Mutex<Vec<usize>>>,
}

impl EmbedBatchSizes {
    fn new(inner: FakeTextEmbeddingProvider) -> (Self, Arc<Mutex<Vec<usize>>>) {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                inner,
                sizes: Arc::clone(&sizes),
            },
            sizes,
        )
    }
}

#[allow(clippy::unused_async_trait_impl)]
impl TextEmbeddingProvider for EmbedBatchSizes {
    fn config(&self) -> &a2a_lab_dev_kit::TextEmbeddingProviderConfig {
        self.inner.config()
    }

    fn limits(&self) -> a2a_lab_dev_kit::TextEmbeddingLimits {
        self.inner.limits()
    }

    async fn embed_texts(
        &self,
        requests: Vec<TextEmbeddingRequest>,
    ) -> Result<Vec<a2a_lab_dev_kit::KeyedTextEmbedding>, a2a_lab_dev_kit::SemanticIndexingFailure>
    {
        self.sizes.lock().unwrap().push(requests.len());
        self.inner.embed_texts(requests).await
    }
}

fn semantic_every_frame() -> a2a_lab_dev_kit::SemanticIndexingConfig {
    const BASE: &str = concat!(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../a2a-lab-dev-kit-rs/tests/fixtures/a2a-lab-v2-semantic-valid.json"
    )),);
    let mut config = A2aLabConfig::parse(BASE).unwrap();
    let semantic = config.semantic_indexing.as_mut().unwrap();
    semantic.limits.worker.max_batch_items = 4;
    semantic.limits.worker.max_concurrency = 4;
    semantic.limits.worker.max_queue_backlog = 64;
    semantic
        .clone()
        .with_schedules(vec![SemanticStreamSchedule::every_saved_frame("camera")])
}

fn store_ns() -> DvrStoreNamespace {
    DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

fn jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xD9]
}

async fn open_store(name: &str) -> (std::path::PathBuf, IndexedVideoDvr) {
    let dir = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-worker-batch-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    (dir, store)
}

async fn seed_jobs(store: &IndexedVideoDvr, count: usize) {
    let source = VideoSourceId::new("camera").unwrap();
    for index in 0..count {
        let capture = a2a_lab_ot2::time::now()
            .unwrap()
            .saturating_add_micros(i64::try_from(index).unwrap_or(i64::MAX) * 1_000_000);
        store
            .append_mjpeg_frame(
                &source,
                capture,
                1_000_000,
                &jpeg(u8::try_from(index).unwrap_or(u8::MAX)),
            )
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM semantic_indexing_job_runtime")
        .execute(store.semantic_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_indexing_jobs")
        .execute(store.semantic_pool())
        .await
        .unwrap();
    let report = store
        .reconcile_retained_indexing_until_idle(&store_ns())
        .await
        .unwrap();
    assert_eq!(report.jobs_created(), u32::try_from(count).unwrap());
}

#[tokio::test]
async fn description_worker_invokes_multi_item_describe_frames_batch() {
    let (_dir, store) = open_store("desc-multi").await;
    let semantic = semantic_every_frame();
    store.set_semantic_indexing_config(semantic.clone());
    seed_jobs(&store, 4).await;

    let description_config = description_provider_config(&semantic.description).unwrap();
    let limits = frame_description_limits(&semantic.limits);
    let inner =
        FakeFrameDescriptionProvider::new(description_config, limits, FakeDescriptionBehavior::Ok);
    let (provider, sizes) = DescribeBatchSizes::new(inner);

    let worker = DescriptionOnlyAnnotationWorker::new(
        provider,
        semantic.limits.worker.clone(),
        "desc-batch",
        IndexingWorkerFaults::new(),
    )
    .with_lane(IndexingWorkerLane::Backfill);
    let report = worker.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(report.descriptions_committed(), 4);
    let recorded = sizes.lock().unwrap().clone();
    assert!(
        recorded.iter().any(|size| *size > 1),
        "expected at least one provider batch with multiple requests, got {recorded:?}"
    );
    assert_eq!(recorded.iter().sum::<usize>(), 4);
}

#[tokio::test]
async fn partial_description_batch_retries_without_persisting_any_frame() {
    let (_dir, store) = open_store("desc-partial").await;
    let semantic = semantic_every_frame();
    store.set_semantic_indexing_config(semantic.clone());
    seed_jobs(&store, 3).await;

    let description_config = description_provider_config(&semantic.description).unwrap();
    let limits = frame_description_limits(&semantic.limits);
    let provider = FakeFrameDescriptionProvider::new(
        description_config,
        limits,
        FakeDescriptionBehavior::Partial,
    );
    let worker = DeterministicDescriptionOnlyWorker::new(
        provider,
        semantic.limits.worker.clone(),
        "desc-partial",
        IndexingWorkerFaults::new(),
    )
    .with_lane(IndexingWorkerLane::Backfill);
    let report = worker.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(report.descriptions_committed(), 0);
    assert!(report.failures() >= 1);

    let jobs: Vec<(String, String)> = sqlx::query_as(
        "SELECT j.job_id, j.frame_id FROM semantic_indexing_jobs j
         JOIN semantic_indexing_job_runtime r ON r.job_id = j.job_id",
    )
    .fetch_all(store.semantic_pool())
    .await
    .unwrap();
    assert_eq!(jobs.len(), 3);
    for (job_id, frame_id) in jobs {
        let job = store.get_indexing_job(&job_id).await.unwrap();
        assert_eq!(job.state(), IndexingJobState::RetryableFailed);
        let manifest = store.get_manifest_frame(&frame_id).await.unwrap();
        assert!(manifest.description().is_none());
        assert_eq!(
            manifest.description_checkpoint(),
            IndexingCheckpoint::Pending
        );
    }
}

#[tokio::test]
async fn embedding_worker_invokes_multi_item_embed_texts_batch() {
    let (_dir, store) = open_store("embed-multi").await;
    let semantic = semantic_every_frame();
    store.set_semantic_indexing_config(semantic.clone());
    seed_jobs(&store, 4).await;

    let desc_providers = build_deterministic_providers(&semantic).await.unwrap();
    let description_worker = DeterministicDescriptionOnlyWorker::new(
        desc_providers.description,
        semantic.limits.worker.clone(),
        "desc-prime",
        IndexingWorkerFaults::new(),
    )
    .with_lane(IndexingWorkerLane::Backfill);
    description_worker
        .run_batch(&store, &semantic)
        .await
        .unwrap();

    let embeddings = semantic.embeddings.as_ref().unwrap();
    let embedding_config = embedding_provider_config(embeddings).unwrap();
    let limits = text_embedding_limits(&semantic.limits);
    let inner = FakeTextEmbeddingProvider::new(embedding_config, limits, FakeEmbeddingBehavior::Ok);
    let (provider, sizes) = EmbedBatchSizes::new(inner);

    let worker = EmbeddingOnlyAnnotationWorker::new(
        provider,
        semantic.limits.worker.clone(),
        "embed-batch",
        IndexingWorkerFaults::new(),
    )
    .with_lane(IndexingWorkerLane::Backfill);
    let report = worker.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(report.embeddings_committed(), 4);
    let recorded = sizes.lock().unwrap().clone();
    assert!(
        recorded.iter().any(|size| *size > 1),
        "expected at least one embedding provider batch with multiple requests, got {recorded:?}"
    );
    assert_eq!(recorded.iter().sum::<usize>(), 4);
}
