//! Semantic frame manifest persistence beside the indexed DVR store.

use a2a_lab_dev_kit::{
    DvrMediaStore, DvrStoreNamespace, EmbeddingMetric, FrameDescriptionProvenance,
    FrameEmbeddingVector, FrameRoiTransform, MemoryVideos, ModelSpaceIdentity,
    ProvenanceGeneration, Sha256Digest, TextEmbeddingProvenance, TimeRange, UtcTimestamp,
    VectorNormalization, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, IndexingCheckpoint, ListManifestFramesRequest,
    ManifestPageRequest, ManifestRetainedState, MediaPayloadStatus, RegisterManifestOutcome,
};

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-manifest-{name}-{}",
        std::process::id()
    ))
}

fn store_ns() -> DvrStoreNamespace {
    DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

fn jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xD9]
}

async fn open_store(name: &str) -> (std::path::PathBuf, IndexedVideoDvr) {
    let dir = temp_root(name);
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    (dir, store)
}

fn fresh_capture(offset_us: i64) -> UtcTimestamp {
    a2a_lab_ot2::time::now()
        .unwrap()
        .saturating_add_micros(offset_us)
}

fn sample_description(text: &str) -> FrameDescriptionProvenance {
    FrameDescriptionProvenance::new(
        "bedrock",
        "global.openai.gpt-5.6-luna",
        Sha256Digest::hash_bytes(b"prompt-v1"),
        Sha256Digest::hash_bytes(b"preprocess-v1"),
        Some(FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap()),
        Sha256Digest::hash_bytes(text.as_bytes()),
        ProvenanceGeneration::new(1).unwrap(),
    )
    .unwrap()
}

async fn list_manifest_ids(
    store: &IndexedVideoDvr,
    source: &VideoSourceId,
    track: &a2a_lab_dev_kit::DvrTrackId,
) -> Vec<String> {
    let mut cursor = None;
    let mut ids = Vec::new();
    loop {
        let request = ListManifestFramesRequest::new(
            source.clone(),
            track.clone(),
            None,
            true,
            ManifestPageRequest::new(cursor.clone(), 2).unwrap(),
        )
        .unwrap();
        let page = store.list_manifest_frames(&request).await.unwrap();
        ids.extend(
            page.items()
                .iter()
                .map(|item| item.frame().frame_id().as_str().to_owned()),
        );
        cursor = page.next_cursor().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    ids
}

fn sample_model_space() -> ModelSpaceIdentity {
    ModelSpaceIdentity::new(
        "bedrock",
        "text-embed-v1",
        4,
        VectorNormalization::L2Unit,
        EmbeddingMetric::Cosine,
        ProvenanceGeneration::new(1).unwrap(),
    )
    .unwrap()
}

fn sample_embedding(text: &str) -> TextEmbeddingProvenance {
    TextEmbeddingProvenance::new(
        "bedrock",
        "text-embed-v1",
        4,
        VectorNormalization::L2Unit,
        EmbeddingMetric::Cosine,
        ProvenanceGeneration::new(1).unwrap(),
        Sha256Digest::hash_bytes(text.as_bytes()),
    )
    .unwrap()
}

fn sample_vector() -> FrameEmbeddingVector {
    FrameEmbeddingVector::new(vec![0.1, 0.2, 0.3, 0.4], 4).unwrap()
}

#[tokio::test]
async fn migration_preserves_existing_dvr_rows() {
    let (dir, store) = open_store("migrate").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let capture = fresh_capture(0);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(1))
        .await
        .unwrap();
    store
        .append_mjpeg_frame(&source, fresh_capture(1_000_000), 1_000_000, &jpeg(9))
        .await
        .unwrap();
    let coverage = store.media_coverage(&source).await.unwrap();
    assert_eq!(coverage.intervals().len(), 1);
    let reopened = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    assert_eq!(
        reopened
            .media_coverage(&source)
            .await
            .unwrap()
            .intervals()
            .len(),
        1
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn duplicate_registration_is_idempotent() {
    let (dir, store) = open_store("dup").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(1_000_000);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(2))
        .await
        .unwrap();
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 1);
    let (frame, first) = store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    assert_eq!(first, RegisterManifestOutcome::Idempotent);
    let (_, second) = store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    assert_eq!(second, RegisterManifestOutcome::Idempotent);
    let current = store
        .get_manifest_frame(frame.frame_id().as_str())
        .await
        .unwrap();
    assert!(current.search_current());
    assert_eq!(current.revision_seq(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn same_timestamp_replacement_supersedes_prior_revision() {
    let (dir, store) = open_store("replace").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(2_000_000);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(3))
        .await
        .unwrap();
    let (first_frame, _) = store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(4))
        .await
        .unwrap();
    let (second_frame, outcome) = store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    assert_eq!(outcome, RegisterManifestOutcome::Idempotent);
    assert_ne!(first_frame.frame_id(), second_frame.frame_id());
    let prior = store
        .get_manifest_frame(first_frame.frame_id().as_str())
        .await
        .unwrap();
    assert!(!prior.search_current());
    assert_eq!(prior.retained_state(), ManifestRetainedState::Superseded);
    let current_only = ListManifestFramesRequest::new(
        source.clone(),
        track.clone(),
        None,
        true,
        ManifestPageRequest::new(None, 10).unwrap(),
    )
    .unwrap();
    let page = store.list_manifest_frames(&current_only).await.unwrap();
    assert_eq!(page.items().len(), 1);
    assert_eq!(page.items()[0].frame().frame_id(), second_frame.frame_id());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn enumeration_is_deterministic_across_restart() {
    let (dir, store) = open_store("enum").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let base = fresh_capture(10_000_000);
    let stamps = [
        base,
        base.saturating_add_micros(1_000_000),
        base.saturating_add_micros(2_000_000),
    ];
    for (index, stamp) in stamps.iter().enumerate() {
        store
            .append_mjpeg_frame(
                &source,
                *stamp,
                1_000_000,
                &jpeg(10 + u8::try_from(index).unwrap()),
            )
            .await
            .unwrap();
        store
            .register_manifest_at_dvr_key(&store_ns(), &source, &track, *stamp)
            .await
            .unwrap();
    }
    let before = list_manifest_ids(&store, &source, &track).await;
    drop(store);
    let reopened = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let after = list_manifest_ids(&reopened, &source, &track).await;
    assert_eq!(before, after);
    assert_eq!(before.len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn manifest_stores_description_and_embedding_checkpoints() {
    let (dir, store) = open_store("prov").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(20_000_000);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(20))
        .await
        .unwrap();
    let (frame, _) = store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let text = "pipette above plate";
    store
        .upsert_frame_description(&frame, text, &sample_description(text))
        .await
        .unwrap();
    store
        .upsert_frame_embedding(
            &frame,
            &sample_model_space(),
            &sample_embedding(text),
            &sample_vector(),
            IndexingCheckpoint::Ready,
        )
        .await
        .unwrap();
    let record = store
        .get_manifest_frame(frame.frame_id().as_str())
        .await
        .unwrap();
    assert_eq!(record.description_checkpoint(), IndexingCheckpoint::Ready);
    assert_eq!(record.embedding_checkpoint(), IndexingCheckpoint::Ready);
    assert_eq!(
        record
            .description()
            .map(a2a_lab_ot2::videos::dvr::StoredFrameDescription::text),
        Some(text)
    );
    assert_eq!(
        record.embedding().and_then(|item| item.vector()),
        Some(&sample_vector())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn embedding_vector_survives_dvr_reopen() {
    let (dir, store) = open_store("vector-reopen").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(50_000_000);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(50))
        .await
        .unwrap();
    let (frame, _) = store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let text = "tips aligned on deck";
    let vector = sample_vector();
    store
        .upsert_frame_description(&frame, text, &sample_description(text))
        .await
        .unwrap();
    store
        .upsert_frame_embedding(
            &frame,
            &sample_model_space(),
            &sample_embedding(text),
            &vector,
            IndexingCheckpoint::Ready,
        )
        .await
        .unwrap();
    drop(store);
    let reopened = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let record = reopened
        .get_manifest_frame(frame.frame_id().as_str())
        .await
        .unwrap();
    assert_eq!(record.embedding().and_then(|e| e.vector()), Some(&vector));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn legacy_provenance_without_vector_blob_round_trips() {
    let (dir, store) = open_store("legacy-vector").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(60_000_000);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(60))
        .await
        .unwrap();
    let (frame, _) = store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let text = "legacy checkpoint";
    store
        .upsert_frame_description(&frame, text, &sample_description(text))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO semantic_frame_embeddings (
            frame_id, embedding_provider, embedding_model, dimensions, normalization, metric,
            embedding_generation, model_space_generation, source_text_hash
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(frame.frame_id().as_str())
    .bind("bedrock")
    .bind("text-embed-v1")
    .bind(4_i64)
    .bind("l2_unit")
    .bind("cosine")
    .bind(1_i64)
    .bind(1_i64)
    .bind(sample_embedding(text).source_text_hash().as_str())
    .execute(store.semantic_pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE semantic_frame_revisions SET embedding_checkpoint = 'ready' WHERE frame_id = ?1",
    )
    .bind(frame.frame_id().as_str())
    .execute(store.semantic_pool())
    .await
    .unwrap();
    let record = store
        .get_manifest_frame(frame.frame_id().as_str())
        .await
        .unwrap();
    assert!(record.embedding().is_some());
    assert!(record.embedding().unwrap().vector().is_none());
    drop(store);
    let reopened = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let again = reopened
        .get_manifest_frame(frame.frame_id().as_str())
        .await
        .unwrap();
    assert!(again.embedding().unwrap().vector().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rejects_non_finite_and_dimension_mismatched_vectors() {
    let (_dir, store) = open_store("reject-vector").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(70_000_000);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(70))
        .await
        .unwrap();
    let (frame, _) = store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let text = "reject path";
    store
        .upsert_frame_description(&frame, text, &sample_description(text))
        .await
        .unwrap();
    let space = sample_model_space();
    let embed = sample_embedding(text);
    let short = FrameEmbeddingVector::new(vec![0.1, 0.2], 4).expect_err("short");
    assert!(short.to_string().contains("length"));
    let nan = FrameEmbeddingVector::new(vec![f32::NAN, 0.2, 0.3, 0.4], 4).expect_err("nan");
    assert!(nan.to_string().contains("finite"));
    assert!(
        store
            .upsert_frame_embedding(
                &frame,
                &space,
                &embed,
                &sample_vector(),
                IndexingCheckpoint::Ready
            )
            .await
            .is_ok()
    );
    let wrong_hash = TextEmbeddingProvenance::new(
        "bedrock",
        "text-embed-v1",
        4,
        VectorNormalization::L2Unit,
        EmbeddingMetric::Cosine,
        ProvenanceGeneration::new(1).unwrap(),
        Sha256Digest::hash_bytes(b"other text"),
    )
    .unwrap();
    let mismatch = store
        .upsert_frame_embedding(
            &frame,
            &space,
            &wrong_hash,
            &sample_vector(),
            IndexingCheckpoint::Ready,
        )
        .await
        .expect_err("hash");
    assert!(mismatch.to_string().contains("source_text_hash"));
    let wrong_space = ModelSpaceIdentity::new(
        "bedrock",
        "text-embed-v1",
        4,
        VectorNormalization::L2Unit,
        EmbeddingMetric::Cosine,
        ProvenanceGeneration::new(2).unwrap(),
    )
    .unwrap();
    let space_err = store
        .upsert_frame_embedding(
            &frame,
            &wrong_space,
            &embed,
            &sample_vector(),
            IndexingCheckpoint::Ready,
        )
        .await
        .expect_err("space");
    assert!(space_err.to_string().contains("generation"));
}

#[tokio::test]
async fn payload_status_reports_missing_and_corrupt_distinctly() {
    let (dir, store) = open_store("payload").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(30_000_000);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(30))
        .await
        .unwrap();
    store
        .append_mjpeg_frame(&source, fresh_capture(90_000_000), 1_000_000, &jpeg(31))
        .await
        .unwrap();
    let (frame, _) = store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let ok = store
        .get_manifest_frame(frame.frame_id().as_str())
        .await
        .unwrap();
    assert_eq!(ok.payload_status(), MediaPayloadStatus::Available);
    let minute = a2a_lab_ot2::videos::dvr::chunk::minute_start_us(capture);
    let chunk_relpath =
        a2a_lab_ot2::videos::dvr::chunk::chunk_relpath(source.as_str(), track.as_str(), minute);
    let chunk_path = store.root().join(&chunk_relpath);
    let tmp_path = store.root().join(format!("{chunk_relpath}.tmp"));
    let path = if chunk_path.exists() {
        chunk_path
    } else {
        tmp_path
    };
    let mut bytes = std::fs::read(&path).unwrap();
    if let Some(byte) = bytes.first_mut() {
        *byte ^= 0xFF;
    }
    std::fs::write(&path, bytes).unwrap();
    let corrupt = store
        .get_manifest_frame(frame.frame_id().as_str())
        .await
        .unwrap();
    assert_eq!(corrupt.payload_status(), MediaPayloadStatus::Corrupt);
    std::fs::remove_file(&path).unwrap();
    let missing = store
        .get_manifest_frame(frame.frame_id().as_str())
        .await
        .unwrap();
    assert_eq!(missing.payload_status(), MediaPayloadStatus::Missing);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn reopen_after_partial_registration_preserves_manifest() {
    let dir = temp_root("reopen");
    let _ = std::fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(40_000_000);
    {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        store
            .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(40))
            .await
            .unwrap();
        store
            .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
            .await
            .unwrap();
    }
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let window = TimeRange::new(
        capture.saturating_add_micros(-1_000_000),
        capture.saturating_add_micros(120_000_000),
    )
    .unwrap();
    let request = ListManifestFramesRequest::new(
        source,
        track,
        Some(window),
        true,
        ManifestPageRequest::new(None, 10).unwrap(),
    )
    .unwrap();
    let page = store.list_manifest_frames(&request).await.unwrap();
    assert_eq!(page.items().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
