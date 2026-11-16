//! Indexed on-disk DVR wrapping the generic semantic chunk store.

use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use a2a_lab_dev_kit::videos::{SaveFaults, SemanticFrameManifestRecord};
use a2a_lab_dev_kit::{
    A2aLabError, DvrFrameAtRequest, DvrFrameAtResult, DvrMediaCoverage, DvrMediaPayload,
    DvrMediaSegment, DvrMediaStore, DvrMediaTrack, DvrPersistHook, DvrRetentionBounds,
    DvrStoreNamespace, DvrTrackId, Page, ReadDvrMediaWindowRequest, SemanticChunkDvrStore,
    SemanticIndexingConfig, TimeRange, UtcTimestamp, VideoCodec, VideoSourceId,
};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};
use tokio::task;

use super::chunk::{crc32, read_payload_range};
use super::h264_epoch;
use super::indexing_scope::IndexedSourceFilter;

/// Default DVR store namespace for semantic manifest rows in this agent.
pub const DEFAULT_SEMANTIC_STORE_NAMESPACE: &str = "ot2.video-dvr";

/// Default root directory for indexed video payloads and SQLite index.
pub const DEFAULT_VIDEO_DVR_ROOT: &str = ".a2a-lab-ot2/video-dvr";

/// Default on-disk retention (three days).
pub const DEFAULT_DISK_VIDEO_RETENTION_SECS: u64 = 3 * 24 * 60 * 60;

/// Hourly cleanup while serve is running.
pub const VIDEO_DVR_CLEANUP_INTERVAL: Duration = Duration::from_hours(1);

/// Restart-safe indexed DVR. Semantic persistence lives in the generic kit engine.
#[derive(Clone)]
pub struct IndexedVideoDvr {
    engine: SemanticChunkDvrStore,
    indexing_sources: Arc<RwLock<IndexedSourceFilter>>,
}

impl IndexedVideoDvr {
    #[must_use]
    pub fn engine(&self) -> &SemanticChunkDvrStore {
        &self.engine
    }

    #[must_use]
    pub fn save_faults(&self) -> &Arc<SaveFaults> {
        self.engine.save_faults()
    }

    /// Persist hooks plus engine slot for [`DeclarativeVideoRuntime::start`].
    #[must_use]
    pub fn runtime_persist_hooks() -> (
        Vec<Arc<dyn DvrPersistHook>>,
        Arc<OnceLock<SemanticChunkDvrStore>>,
    ) {
        SemanticChunkDvrStore::runtime_persist_hooks()
    }

    /// Wraps an already-opened kit semantic store and runs OT-2 media cutover.
    pub async fn from_engine(engine: SemanticChunkDvrStore) -> Result<Self, A2aLabError> {
        let store = Self {
            engine,
            indexing_sources: Arc::new(RwLock::new(IndexedSourceFilter::all_sources())),
        };
        store.finish_opening().await?;
        Ok(store)
    }

    /// Opens media at `root` and semantic state at `root/semantic.sqlite`.
    pub async fn open(root: impl AsRef<Path>, retain_secs: u64) -> Result<Self, A2aLabError> {
        let root = root.as_ref();
        Self::open_with_semantic_path(root, retain_secs, root.join("semantic.sqlite")).await
    }

    /// Opens media storage at `root` and the semantic database at `semantic_sqlite`.
    pub async fn open_with_semantic_path(
        root: impl AsRef<Path>,
        retain_secs: u64,
        semantic_sqlite: impl AsRef<Path>,
    ) -> Result<Self, A2aLabError> {
        let engine = SemanticChunkDvrStore::open_at(
            root,
            retain_secs,
            semantic_sqlite,
            DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE)?,
        )
        .await?;
        Self::from_engine(engine).await
    }

    async fn finish_opening(&self) -> Result<(), A2aLabError> {
        h264_epoch::migrate_h264_epochs(self).await?;
        let _ = self.reconcile_frame_decision_tombstones().await?;
        Ok(())
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        self.engine.root()
    }

    pub async fn append_mjpeg_frame(
        &self,
        source_id: &VideoSourceId,
        captured_at: UtcTimestamp,
        frame_period_us: u64,
        jpeg: &[u8],
    ) -> Result<(), A2aLabError> {
        self.engine
            .append_mjpeg_frame(source_id, captured_at, frame_period_us, jpeg)
            .await
    }

    pub async fn append_h264_sample(
        &self,
        source_id: &VideoSourceId,
        captured_at: UtcTimestamp,
        frame_period_us: u64,
        keyframe: bool,
        codec_init: &[u8],
        sample: &[u8],
    ) -> Result<(), A2aLabError> {
        self.engine
            .append_h264_sample(
                source_id,
                captured_at,
                frame_period_us,
                keyframe,
                codec_init,
                sample,
            )
            .await
    }

    pub async fn cleanup_source(
        &self,
        source_id: &VideoSourceId,
        now: UtcTimestamp,
    ) -> Result<(), A2aLabError> {
        self.engine.chunk().cleanup_source(source_id, now).await
    }

    pub async fn cleanup_at(&self, now: UtcTimestamp) -> Result<(), A2aLabError> {
        self.engine.chunk().cleanup_at(now).await
    }

    #[doc(hidden)]
    pub async fn write_legacy_segment(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        captured_at: UtcTimestamp,
        frame_period_us: u64,
        keyframe: bool,
        payload: &[u8],
    ) -> Result<(), A2aLabError> {
        let codec = if track_id.as_str() == "h264" {
            VideoCodec::H264
        } else {
            VideoCodec::Mjpeg
        };
        sqlx::query(
            "INSERT INTO dvr_tracks (source_id, track_id, codec, codec_init, frame_period_us)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source_id, track_id) DO NOTHING",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(codec_label(codec))
        .bind::<&[u8]>(&[])
        .bind(i64::try_from(frame_period_us).unwrap_or(i64::MAX))
        .execute(self.media_pool())
        .await
        .map_err(db_error)?;
        let end =
            captured_at.saturating_add_micros(i64::try_from(frame_period_us.max(1)).unwrap_or(1));
        let relpath = format!(
            "{}/{}/{}.mjpeg",
            source_id.as_str(),
            track_id.as_str(),
            captured_at.as_micros()
        );
        let abs = self.root().join(&relpath);
        if let Some(parent) = abs.parent() {
            task::spawn_blocking({
                let parent = parent.to_path_buf();
                move || std::fs::create_dir_all(&parent)
            })
            .await
            .map_err(join_error)?
            .map_err(|error| {
                A2aLabError::unavailable(format!("create {}: {error}", parent.display()))
            })?;
        }
        let tmp = abs.with_extension("mjpeg.tmp");
        task::spawn_blocking({
            let tmp = tmp.clone();
            let payload = payload.to_vec();
            move || std::fs::write(&tmp, payload)
        })
        .await
        .map_err(join_error)?
        .map_err(|error| A2aLabError::unavailable(format!("write {}: {error}", tmp.display())))?;
        task::spawn_blocking({
            let tmp = tmp.clone();
            let abs = abs.clone();
            move || std::fs::rename(&tmp, &abs)
        })
        .await
        .map_err(join_error)?
        .map_err(|error| {
            let _ = std::fs::remove_file(&tmp);
            A2aLabError::unavailable(format!("rename {}: {error}", abs.display()))
        })?;
        sqlx::query(
            "INSERT INTO dvr_segments
                (source_id, track_id, start_us, end_us, keyframe, payload_relpath, payload_ready)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)
             ON CONFLICT(source_id, track_id, start_us) DO UPDATE SET
                end_us = excluded.end_us,
                keyframe = excluded.keyframe,
                payload_relpath = excluded.payload_relpath,
                payload_ready = 1",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(captured_at.as_micros())
        .bind(end.as_micros())
        .bind(i64::from(keyframe))
        .bind(&relpath)
        .execute(self.media_pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    pub(crate) async fn load_retained_payload_at_key(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        start_us: UtcTimestamp,
    ) -> Result<(Vec<u8>, u32, UtcTimestamp), A2aLabError> {
        let key = start_us.as_micros();
        let segment = sqlx::query(
            "SELECT end_us, payload_relpath, '' AS chunk_relpath, NULL AS payload_offset,
                    NULL AS payload_length, NULL AS payload_crc32
             FROM dvr_segments
             WHERE source_id = ?1 AND track_id = ?2 AND start_us = ?3 AND payload_ready = 1",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(key)
        .fetch_optional(self.media_pool())
        .await
        .map_err(db_error)?;
        let frame = sqlx::query(
            "SELECT end_us, '' AS payload_relpath, chunk_relpath, payload_offset, payload_length,
                    payload_crc32
             FROM dvr_frames
             WHERE source_id = ?1 AND track_id = ?2 AND start_us = ?3 AND payload_ready = 1",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(key)
        .fetch_optional(self.media_pool())
        .await
        .map_err(db_error)?;
        let mut candidates = Vec::with_capacity(2);
        if segment.is_some() {
            candidates.push(RetainedStorageKind::LegacySegment);
        }
        if frame.is_some() {
            candidates.push(RetainedStorageKind::ChunkFrame);
        }
        if candidates.is_empty() {
            return Err(A2aLabError::not_found(
                "DVR frame",
                format!("{source_id}/{track_id}/{key}"),
            ));
        }
        let mut last_err: Option<A2aLabError> = None;
        for kind in candidates {
            let row = match kind {
                RetainedStorageKind::LegacySegment => segment.as_ref(),
                RetainedStorageKind::ChunkFrame => frame.as_ref(),
            };
            let Some(row) = row else {
                continue;
            };
            match self.payload_tuple_from_retained_row(row).await {
                Ok(tuple) => return Ok(tuple),
                Err(error) if retained_payload_fallback_allowed(&error) => {
                    last_err = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            A2aLabError::not_found("DVR frame", format!("{source_id}/{track_id}/{key}"))
        }))
    }

    async fn payload_tuple_from_retained_row(
        &self,
        row: &SqliteRow,
    ) -> Result<(Vec<u8>, u32, UtcTimestamp), A2aLabError> {
        let end_us: i64 = row.try_get("end_us").map_err(db_error)?;
        let bytes = self.load_payload_bytes_from_retained_row(row).await?;
        let crc = crc32(&bytes);
        Ok((bytes, crc, crate::time::from_unix_microseconds(end_us)?))
    }

    async fn load_payload_bytes_from_retained_row(
        &self,
        row: &SqliteRow,
    ) -> Result<Vec<u8>, A2aLabError> {
        let chunk_relpath: String = row.try_get("chunk_relpath").map_err(db_error)?;
        if chunk_relpath.is_empty() {
            let relpath: String = row.try_get("payload_relpath").map_err(db_error)?;
            return self.load_payload(&relpath).await;
        }
        let offset: i64 = row.try_get("payload_offset").map_err(db_error)?;
        let length: i64 = row.try_get("payload_length").map_err(db_error)?;
        let crc32_value: i64 = row.try_get("payload_crc32").map_err(db_error)?;
        self.load_chunk_payload(
            &chunk_relpath,
            u64::try_from(offset.max(0)).unwrap_or(0),
            u64::try_from(length.max(0)).unwrap_or(0),
            u32::try_from(crc32_value.max(0)).unwrap_or(0),
        )
        .await
    }

    async fn load_payload(&self, relpath: &str) -> Result<Vec<u8>, A2aLabError> {
        let path = self.root().join(relpath);
        let display = path.display().to_string();
        task::spawn_blocking(move || std::fs::read(path))
            .await
            .map_err(join_error)?
            .map_err(|error| A2aLabError::unavailable(format!("read {display}: {error}")))
    }

    pub(crate) async fn load_chunk_payload(
        &self,
        relpath: &str,
        offset: u64,
        length: u64,
        expected_crc32: u32,
    ) -> Result<Vec<u8>, A2aLabError> {
        let root = self.root().to_path_buf();
        let final_path = root.join(relpath);
        let tmp_path = root.join(format!("{relpath}.tmp"));
        let path = if tmp_path.exists() {
            tmp_path
        } else {
            final_path
        };
        read_payload_range(&path, offset, length, expected_crc32).await
    }

    #[must_use]
    pub fn media_pool(&self) -> &SqlitePool {
        self.engine.media_pool()
    }

    #[must_use]
    pub fn semantic_pool(&self) -> &SqlitePool {
        self.engine.semantic_pool()
    }

    pub fn set_semantic_indexing_config(&self, config: &SemanticIndexingConfig) {
        self.engine.set_stream_schedules(config.schedules.clone());
    }

    pub fn set_indexed_source_filter(&self, filter: IndexedSourceFilter) {
        if let Ok(mut guard) = self.indexing_sources.write() {
            *guard = filter;
        }
    }

    #[must_use]
    pub fn indexed_source_filter(&self) -> IndexedSourceFilter {
        self.indexing_sources
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub async fn get_manifest_frame(
        &self,
        frame_id: &str,
    ) -> Result<SemanticFrameManifestRecord, A2aLabError> {
        self.engine.get_manifest_frame(frame_id).await
    }

    pub async fn list_manifest_frames(
        &self,
        request: &a2a_lab_dev_kit::videos::ListManifestFramesRequest,
    ) -> Result<Page<SemanticFrameManifestRecord>, A2aLabError> {
        self.engine.list_manifest_frames(request).await
    }

    pub async fn count_indexing_jobs(&self) -> Result<u64, A2aLabError> {
        self.engine.count_indexing_jobs().await
    }

    pub async fn ensure_indexing_at_dvr_key(
        &self,
        store: &DvrStoreNamespace,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        start: UtcTimestamp,
    ) -> Result<
        (
            a2a_lab_dev_kit::SavedFrameRef,
            a2a_lab_dev_kit::videos::RegisterManifestOutcome,
            bool,
        ),
        A2aLabError,
    > {
        self.engine
            .ensure_indexing_at_dvr_key(store, source_id, track_id, start)
            .await
    }

    pub async fn register_manifest_at_dvr_key(
        &self,
        store: &DvrStoreNamespace,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        start: UtcTimestamp,
    ) -> Result<
        (
            a2a_lab_dev_kit::SavedFrameRef,
            a2a_lab_dev_kit::videos::RegisterManifestOutcome,
        ),
        A2aLabError,
    > {
        self.engine
            .register_manifest_at_dvr_key(store, source_id, track_id, start)
            .await
    }

    #[doc(hidden)]
    pub async fn count_searchable_embedding_vectors(&self) -> Result<u64, A2aLabError> {
        let active: i64 = sqlx::query_scalar(
            "SELECT COALESCE(model_space_generation, 0)
             FROM semantic_embedding_config_identity WHERE singleton_id = 1",
        )
        .fetch_optional(self.semantic_pool())
        .await
        .map_err(db_error)?
        .unwrap_or(0);
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM semantic_frame_embeddings emb
             JOIN semantic_frame_revisions rev ON rev.frame_id = emb.frame_id
             WHERE rev.search_current = 1
               AND rev.retained_state = 'retained'
               AND rev.embedding_checkpoint = 'ready'
               AND emb.embedding_vector IS NOT NULL
               AND length(emb.embedding_vector) > 0
               AND emb.model_space_generation = ?1",
        )
        .bind(active)
        .fetch_one(self.semantic_pool())
        .await
        .map_err(db_error)?;
        Ok(u64::try_from(count.max(0)).unwrap_or(0))
    }

    #[doc(hidden)]
    pub async fn count_nonempty_embedding_vectors(&self) -> Result<u64, A2aLabError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM semantic_frame_embeddings
             WHERE embedding_vector IS NOT NULL AND length(embedding_vector) > 0",
        )
        .fetch_one(self.semantic_pool())
        .await
        .map_err(db_error)?;
        Ok(u64::try_from(count.max(0)).unwrap_or(0))
    }

    pub async fn active_model_space_generation(&self) -> Result<Option<u64>, A2aLabError> {
        let generation: Option<i64> = sqlx::query_scalar(
            "SELECT model_space_generation FROM semantic_embedding_config_identity WHERE singleton_id = 1",
        )
        .fetch_optional(self.semantic_pool())
        .await
        .map_err(db_error)?;
        Ok(generation.and_then(|value| u64::try_from(value.max(0)).ok()))
    }

    async fn containing_frame(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        at: i64,
    ) -> Result<Option<SqliteRow>, A2aLabError> {
        let segment = self
            .lookup_frame(
                "SELECT track_id, start_us, end_us, keyframe, payload_relpath,
                        '' AS chunk_relpath, NULL AS payload_offset, NULL AS payload_length,
                        NULL AS payload_crc32, codec
                 FROM dvr_segments
                 JOIN dvr_tracks USING (source_id, track_id)
                 WHERE source_id = ?1 AND track_id = ?2 AND payload_ready = 1
                   AND start_us <= ?3 AND end_us > ?3
                 ORDER BY start_us DESC
                 LIMIT 1",
                source_id,
                track_id,
                at,
            )
            .await?;
        let frame = self
            .lookup_frame(
                "SELECT track_id, start_us, end_us, keyframe, '' AS payload_relpath, chunk_relpath,
                        payload_offset, payload_length, payload_crc32, codec
                 FROM dvr_frames
                 JOIN dvr_tracks USING (source_id, track_id)
                 WHERE source_id = ?1 AND track_id = ?2 AND payload_ready = 1
                   AND start_us <= ?3 AND end_us > ?3
                 ORDER BY start_us DESC
                 LIMIT 1",
                source_id,
                track_id,
                at,
            )
            .await?;
        Ok(match (segment, frame) {
            (Some(left), Some(right)) => {
                let left_start: i64 = left.try_get("start_us").map_err(db_error)?;
                let right_start: i64 = right.try_get("start_us").map_err(db_error)?;
                Some(if left_start >= right_start {
                    left
                } else {
                    right
                })
            }
            (Some(row), None) | (None, Some(row)) => Some(row),
            (None, None) => None,
        })
    }

    async fn lookup_frame(
        &self,
        sql: &str,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        at: i64,
    ) -> Result<Option<SqliteRow>, A2aLabError> {
        sqlx::query(sql)
            .bind(source_id.as_str())
            .bind(track_id.as_str())
            .bind(at)
            .fetch_optional(self.media_pool())
            .await
            .map_err(db_error)
    }

    async fn missing_frame(
        &self,
        source_id: &VideoSourceId,
        at: UtcTimestamp,
    ) -> Result<DvrFrameAtResult, A2aLabError> {
        let bounds = self.retention_bounds(source_id).await?;
        if at < bounds.oldest() || at >= bounds.newest() {
            Err(A2aLabError::no_recording(
                "requested instant is not retained",
            ))
        } else {
            Err(A2aLabError::recording_gap("requested instant is in a gap"))
        }
    }

    #[doc(hidden)]
    pub async fn query_plan(&self, sql: &str) -> Result<Vec<String>, A2aLabError> {
        let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
            .fetch_all(self.media_pool())
            .await
            .map_err(db_error)?;
        rows.into_iter()
            .map(|row| row.try_get::<String, _>(3).map_err(db_error))
            .collect()
    }
}

impl DvrMediaStore for IndexedVideoDvr {
    async fn retention_bounds(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<DvrRetentionBounds, A2aLabError> {
        self.engine.chunk().retention_bounds(source_id).await
    }

    async fn media_coverage(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<DvrMediaCoverage, A2aLabError> {
        self.engine.chunk().media_coverage(source_id).await
    }

    async fn list_tracks(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<Vec<DvrMediaTrack>, A2aLabError> {
        self.engine.chunk().list_tracks(source_id).await
    }

    async fn read_media_window(
        &self,
        request: ReadDvrMediaWindowRequest,
    ) -> Result<Page<DvrMediaSegment>, A2aLabError> {
        self.engine.chunk().read_media_window(request).await
    }

    async fn read_media_spans(
        &self,
        request: ReadDvrMediaWindowRequest,
    ) -> Result<Page<DvrMediaSegment>, A2aLabError> {
        self.engine.chunk().read_media_spans(request).await
    }

    async fn frame_at(&self, request: DvrFrameAtRequest) -> Result<DvrFrameAtResult, A2aLabError> {
        let _ = self.retention_bounds(request.source_id()).await?;
        let at = request.at();
        let row = self
            .containing_frame(request.source_id(), request.track_id(), at.as_micros())
            .await?;
        let Some(row) = row else {
            return self.missing_frame(request.source_id(), at).await;
        };
        let start_us: i64 = row.try_get("start_us").map_err(db_error)?;
        let actual = crate::time::from_unix_microseconds(start_us)?;
        if let Some(limit) = request.max_delta_us()
            && actual.as_micros().abs_diff(at.as_micros()) > limit
        {
            return Err(A2aLabError::not_found("DVR frame", at.to_string()));
        }
        let segment = segment_from_row(self, request.source_id(), &row).await?;
        Ok(DvrFrameAtResult::new(at, actual, segment.payload().clone()))
    }
}

async fn segment_from_row(
    store: &IndexedVideoDvr,
    source_id: &VideoSourceId,
    row: &SqliteRow,
) -> Result<DvrMediaSegment, A2aLabError> {
    let track_id = DvrTrackId::new(row.try_get::<String, _>("track_id").map_err(db_error)?)?;
    let start = crate::time::from_unix_microseconds(row.try_get("start_us").map_err(db_error)?)?;
    let keyframe: i64 = row.try_get("keyframe").map_err(db_error)?;
    let codec = parse_codec(&row.try_get::<String, _>("codec").map_err(db_error)?)?;
    let (bytes, _, end) = store
        .load_retained_payload_at_key(source_id, &track_id, start)
        .await?;
    let payload = match codec {
        VideoCodec::Mjpeg => DvrMediaPayload::MjpegFrame(bytes),
        VideoCodec::H264 | VideoCodec::H265 => DvrMediaPayload::EncodedSample { codec, bytes },
    };
    DvrMediaSegment::new(
        track_id,
        TimeRange::new(start, end)?,
        keyframe != 0,
        payload,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetainedStorageKind {
    LegacySegment,
    ChunkFrame,
}

fn retained_payload_fallback_allowed(error: &A2aLabError) -> bool {
    let message = error.to_string();
    message.contains("checksum mismatch")
        || message.contains("not found")
        || message.contains("read ")
        || message.contains("open ")
        || message.contains("seek ")
}

pub(crate) fn db_error(error: impl std::fmt::Display) -> A2aLabError {
    A2aLabError::unavailable(format!("video DVR database: {error}"))
}

pub(crate) fn join_error(error: impl std::fmt::Display) -> A2aLabError {
    A2aLabError::unavailable(format!("video DVR task: {error}"))
}

fn codec_label(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::Mjpeg => "mjpeg",
        VideoCodec::H264 => "h264",
        VideoCodec::H265 => "h265",
    }
}

pub(crate) fn parse_codec(label: &str) -> Result<VideoCodec, A2aLabError> {
    match label {
        "mjpeg" => Ok(VideoCodec::Mjpeg),
        "h264" => Ok(VideoCodec::H264),
        "h265" => Ok(VideoCodec::H265),
        _ => Err(A2aLabError::invalid("codec", "unknown stored codec")),
    }
}
