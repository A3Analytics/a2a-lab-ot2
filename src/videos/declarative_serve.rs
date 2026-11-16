//! Serve-mode [`DeclarativeVideoRuntime`] wiring for OT-2.

use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use a2a_lab_dev_kit::videos::WarmStillCache;
use a2a_lab_dev_kit::{
    A2aLabError, DeclarativeVideoRuntime, DvrReady, DvrStoreNamespace, DvrVideoProvider,
    FrameDecisionLedgerProvider, MemoryVideos, SemanticChunkDvrStore, SqliteChunkDvrStore,
    VideoCaptureSettingsHandle, VideoConfig, VideoRuntimeConfig,
};
use async_trait::async_trait;
use tokio::sync::watch;

use crate::camera::{CAMERA_SOURCE_ID, EXTERNAL_CAMERA_SOURCE_ID};
use crate::lab_config::{ResolvedSemanticIndexing, ResolvedVideoSettings};
use crate::videos::VideoServeConfig;
use crate::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DvrFrameDecisionLedger, IndexedVideoDvr,
    VideoDvrAnnotationQueue, VideoDvrCatalog, VideoDvrDecisionEvaluationQueue,
};
use crate::videos::injections::LiveVideoInjections;
use crate::videos::semantic_indexing::{
    SemanticServeBundle, SemanticServeInjections, prepare_semantic_serve_bundle,
};

/// Owning declarative runtime plus optional semantic serve sidecars.
pub struct DeclarativeVideoServe {
    runtime: DeclarativeVideoRuntime,
    indexed: IndexedVideoDvr,
    semantic: SemanticServeBundle,
    #[allow(dead_code)]
    annotation: Option<VideoDvrAnnotationQueue>,
    #[allow(dead_code)]
    decision_evaluation: Option<VideoDvrDecisionEvaluationQueue>,
    stop: watch::Sender<bool>,
}

impl DeclarativeVideoServe {
    #[must_use]
    pub fn provider(&self) -> DvrVideoProvider {
        self.runtime.provider()
    }

    #[must_use]
    pub fn settings_handle(&self) -> VideoCaptureSettingsHandle {
        self.runtime.settings_handle()
    }

    #[must_use]
    pub fn still_cache(&self) -> &WarmStillCache {
        self.runtime.still_cache()
    }

    #[must_use]
    pub fn indexed_dvr(&self) -> &IndexedVideoDvr {
        &self.indexed
    }

    /// OT-2 semantic store view over the runtime chunk DVR.
    #[must_use]
    pub fn dvr(&self) -> &IndexedVideoDvr {
        &self.indexed
    }

    #[must_use]
    pub fn semantic_bundle(&self) -> &SemanticServeBundle {
        &self.semantic
    }

    pub fn frame_decision_ledger(
        &self,
    ) -> Result<Option<Arc<dyn FrameDecisionLedgerProvider>>, A2aLabError> {
        if self.semantic.search.is_none() {
            return Ok(None);
        }
        DvrFrameDecisionLedger::pilot_authorized_producer(Arc::new(self.indexed.clone()))
            .map(|ledger| Some(ledger as Arc<dyn FrameDecisionLedgerProvider>))
    }

    pub fn shutdown(&self) {
        let _ = self.stop.send(true);
        self.runtime.shutdown_handle().stop();
    }
}

impl Drop for DeclarativeVideoServe {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct SemanticDvrReady {
    engine_slot: Arc<OnceLock<SemanticChunkDvrStore>>,
    indexed_slot: Arc<OnceLock<IndexedVideoDvr>>,
    semantic_sqlite: std::path::PathBuf,
}

#[async_trait]
impl DvrReady for SemanticDvrReady {
    async fn on_dvr_opened(&self, store: &SqliteChunkDvrStore) -> Result<(), A2aLabError> {
        SemanticChunkDvrStore::adopt_opened_at(
            store.clone(),
            Arc::clone(&self.engine_slot),
            &self.semantic_sqlite,
            DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE)?,
        )
        .await?;
        let engine =
            self.engine_slot.get().cloned().ok_or_else(|| {
                A2aLabError::unavailable("semantic DVR engine init before workers")
            })?;
        let indexed = IndexedVideoDvr::from_engine(engine).await?;
        let _ = self.indexed_slot.set(indexed);
        Ok(())
    }
}

async fn start_declarative_serve(
    video: VideoConfig,
    storage_root: &Path,
    max_frame_bytes: u64,
    injections: LiveVideoInjections,
    semantic: Option<&ResolvedSemanticIndexing>,
    semantic_injections: SemanticServeInjections,
) -> Result<DeclarativeVideoServe, A2aLabError> {
    let (hooks, engine_slot) = IndexedVideoDvr::runtime_persist_hooks();
    let indexed_slot = Arc::new(OnceLock::new());
    let semantic_sqlite = semantic.map_or_else(
        || storage_root.join("semantic.sqlite"),
        |settings| settings.sqlite_path.clone(),
    );
    let mut runtime_config = VideoRuntimeConfig::new(video, storage_root)
        .with_hooks(hooks)
        .with_dvr_ready(Arc::new(SemanticDvrReady {
            engine_slot,
            indexed_slot: Arc::clone(&indexed_slot),
            semantic_sqlite,
        }));
    runtime_config.max_frame_bytes = max_frame_bytes;
    let runtime =
        DeclarativeVideoRuntime::start(runtime_config, injections.into_runtime_injections())
            .await?;
    let indexed = indexed_slot
        .get()
        .cloned()
        .ok_or_else(|| A2aLabError::unavailable("indexed DVR init before workers"))?;
    let semantic_bundle = match semantic {
        Some(settings) => {
            prepare_semantic_serve_bundle(settings, &indexed, &semantic_injections).await?
        }
        None => SemanticServeBundle::empty(),
    };
    let maintenance_catalog = VideoDvrCatalog::open(
        MemoryVideos::new(),
        indexed.clone(),
        Duration::from_secs(60),
    )?;
    let (stop, rx) = watch::channel(false);
    let annotation =
        maintenance_catalog.maintain_annotation_jobs(rx.clone(), semantic_bundle.workers.clone());
    if let Some((coverage, reason)) = semantic_bundle.decision_unavailable {
        let now_us = crate::time::now()?.as_micros();
        let closed = indexed
            .mark_decision_evaluation_jobs_unavailable(coverage, reason, now_us)
            .await?;
        if closed > 0 {
            eprintln!("video DVR: Jev unavailable; closed {closed} decision jobs as {reason}");
        }
    }
    let decision_evaluation = maintenance_catalog
        .maintain_decision_evaluation_jobs(rx, semantic_bundle.decision_worker.clone());
    Ok(DeclarativeVideoServe {
        runtime,
        indexed,
        semantic: semantic_bundle,
        annotation: Some(annotation),
        decision_evaluation: Some(decision_evaluation),
        stop,
    })
}

/// Starts declarative runtime for resolved configuration.
pub async fn start_resolved_declarative_serve(
    settings: &ResolvedVideoSettings,
    injections: LiveVideoInjections,
    semantic: Option<&ResolvedSemanticIndexing>,
    semantic_injections: SemanticServeInjections,
) -> Result<DeclarativeVideoServe, A2aLabError> {
    let video = settings.declarative_video_config()?;
    start_declarative_serve(
        video,
        &settings.dvr_storage.path,
        settings.max_image_bytes,
        injections,
        semantic,
        semantic_injections,
    )
    .await
}

/// Maps legacy CLI serve slots into declarative configuration.
pub fn legacy_serve_video_config(config: &VideoServeConfig) -> Result<VideoConfig, A2aLabError> {
    use crate::videos::source::LiveVideoSourceDescriptor;
    use a2a_lab_dev_kit::ConfigDuration;
    use a2a_lab_dev_kit::VideoDvrConfig;
    use a2a_lab_dev_kit::VideoInputFormat;
    use a2a_lab_dev_kit::VideoSourceConfig;

    let mut sources = Vec::new();
    if let Some(index) = config.camera_index() {
        sources.push(LiveVideoSourceDescriptor::legacy_external(
            index,
            config.description().to_owned(),
        ));
    }
    if let Some(index) = config.webcam_index() {
        sources.push(LiveVideoSourceDescriptor::legacy_camera(index));
    }
    let file_sources = sources
        .iter()
        .map(|descriptor| {
            let mut entry = descriptor.to_video_source_config()?;
            if matches!(
                entry.id.as_str(),
                EXTERNAL_CAMERA_SOURCE_ID | CAMERA_SOURCE_ID
            ) {
                entry.format = VideoInputFormat::Mjpeg;
            }
            Ok(entry)
        })
        .collect::<Result<Vec<VideoSourceConfig>, A2aLabError>>()?;
    Ok(VideoConfig {
        sources: file_sources,
        fps: Some(config.fps()),
        live_retention: Some(ConfigDuration::parse(
            "videos.live_retention",
            &format!("{}s", config.live_retention_secs()),
        )?),
        dvr: Some(VideoDvrConfig {
            retention: Some(ConfigDuration::parse(
                "videos.dvr.retention",
                &format!("{}s", config.disk_retention_secs()),
            )?),
            save_interval: None,
            storage_path: None,
        }),
    })
}

/// Legacy two-slot declarative startup.
pub async fn start_legacy_declarative_serve(
    config: &VideoServeConfig,
    injections: LiveVideoInjections,
) -> Result<DeclarativeVideoServe, A2aLabError> {
    let video = legacy_serve_video_config(config)?;
    if video.sources.is_empty() {
        return Err(A2aLabError::invalid(
            "video",
            "no live video sources configured",
        ));
    }
    start_declarative_serve(
        video,
        config.disk_root(),
        config.max_image_bytes(),
        injections,
        None,
        SemanticServeInjections::default(),
    )
    .await
}
