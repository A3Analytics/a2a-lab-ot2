//! Indexed on-disk DVR: devkit chunk store plus OT-2 semantic tables.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabError, DvrCommittedFrame, DvrFrameAtRequest, DvrFrameAtResult, DvrMediaCoverage,
    DvrMediaPayload, DvrMediaSegment, DvrMediaStore, DvrMediaTrack, DvrPersistHook,
    DvrRetentionBounds, DvrStoreNamespace, DvrTrackId, Page, ReadDvrMediaWindowRequest,
    SavedFrameRef, SemanticIndexingConfig, SemanticIndexingStage, SemanticSqliteStore,
    SqliteChunkDvrStore, TimeRange, UtcTimestamp, VideoCodec, VideoSourceId,
};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};
use tokio::task;

use super::chunk::{crc32, read_payload_range};
use super::frame_manifest::DEFAULT_SEMANTIC_STORE_NAMESPACE;
use super::h264_epoch;
use super::indexing_registration::{INDEXING_PRIORITY_LIVE, SaveFaults, commit_indexing_job_tx};
use super::indexing_scope::IndexedSourceFilter;
use super::semantic_persist_hook::SemanticPersistHook;

/// Default root directory for indexed video payloads and SQLite index.
pub const DEFAULT_VIDEO_DVR_ROOT: &str = ".a2a-lab-ot2/video-dvr";

/// Default on-disk retention (three days).
pub const DEFAULT_DISK_VIDEO_RETENTION_SECS: u64 = 3 * 24 * 60 * 60;

/// Hourly cleanup while serve is running.
pub const VIDEO_DVR_CLEANUP_INTERVAL: Duration = Duration::from_hours(1);

/// Restart-safe indexed DVR. Media and semantic databases are separate devkit stores.
#[derive(Clone)]
pub struct IndexedVideoDvr {
    chunk: SqliteChunkDvrStore,
    semantic: SemanticSqliteStore,
    pub(crate) save_faults: Arc<SaveFaults>,
    indexing_sources: Arc<RwLock<IndexedSourceFilter>>,
    semantic_indexing: Arc<RwLock<Option<SemanticIndexingConfig>>>,
    hook_error: Arc<Mutex<Option<A2aLabError>>>,
}

impl IndexedVideoDvr {
    /// Persists the active embedding model space and schedules re-embedding when it changes.
    pub async fn sync_embedding_config(
        &self,
        config: &a2a_lab_dev_kit::SemanticEmbeddingsConfig,
    ) -> Result<bool, A2aLabError> {
        let model_space = config.model_space_identity()?;
        let space = config.space();
        let identity = format!(
            "{}|{}|{}|{:?}|{:?}|{}",
            model_space.provider(),
            model_space.model(),
            space.dimensions,
            space.normalization,
            space.metric,
            model_space.generation().get()
        );
        let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
        let generation = i64::try_from(model_space.generation().get()).unwrap_or(i64::MAX);
        let prior: Option<String> = sqlx::query_scalar(
            "SELECT config_digest FROM semantic_embedding_config_identity WHERE singleton_id = 1",
        )
        .fetch_optional(self.semantic_pool())
        .await
        .map_err(db_error)?;
        if prior.as_deref() == Some(digest.as_str()) {
            return Ok(false);
        }
        let now_us = crate::time::now()?.as_micros();
        let mut tx = self.semantic_pool().begin().await.map_err(db_error)?;
        if prior.is_some() {
            self.wipe_retained_embedding_vectors_tx(&mut tx).await?;
            self.persist_embedding_config_identity_tx(
                &mut tx,
                &model_space,
                space,
                generation,
                &digest,
                now_us,
            )
            .await?;
            self.requeue_retained_indexing_for_rebuild_tx(&mut tx)
                .await?;
        } else {
            self.persist_embedding_config_identity_tx(
                &mut tx,
                &model_space,
                space,
                generation,
                &digest,
                now_us,
            )
            .await?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(prior.is_some())
    }

    async fn persist_embedding_config_identity_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        model_space: &a2a_lab_dev_kit::ModelSpaceIdentity,
        space: &a2a_lab_dev_kit::EmbeddingSpaceConfig,
        generation: i64,
        digest: &str,
        now_us: i64,
    ) -> Result<(), A2aLabError> {
        sqlx::query(
            "INSERT INTO semantic_embedding_config_identity
             (singleton_id, provider, model, dimensions, normalization, metric,
              model_space_generation, config_digest, updated_at_us)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(singleton_id) DO UPDATE SET provider=excluded.provider,
               model=excluded.model, dimensions=excluded.dimensions,
               normalization=excluded.normalization, metric=excluded.metric,
               model_space_generation=excluded.model_space_generation,
               config_digest=excluded.config_digest, updated_at_us=excluded.updated_at_us",
        )
        .bind(model_space.provider())
        .bind(model_space.model())
        .bind(i64::from(space.dimensions))
        .bind(format!("{:?}", space.normalization))
        .bind(format!("{:?}", space.metric))
        .bind(generation)
        .bind(digest)
        .bind(now_us)
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn wipe_retained_embedding_vectors_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<(), A2aLabError> {
        sqlx::query(
            "UPDATE semantic_frame_embeddings
             SET embedding_vector = NULL
             WHERE frame_id IN (
                 SELECT frame_id FROM semantic_frame_revisions
                 WHERE search_current = 1 AND retained_state = 'retained'
             )",
        )
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
        sqlx::query(
            "UPDATE semantic_frame_revisions
             SET embedding_checkpoint = 'pending'
             WHERE search_current = 1 AND retained_state = 'retained'",
        )
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
        sqlx::query(
            "UPDATE semantic_indexing_job_runtime
             SET job_state = 'pending',
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 completed_at_us = NULL
             WHERE job_state = 'ready'
               AND job_id IN (
                 SELECT j.job_id FROM semantic_indexing_jobs j
                 JOIN semantic_frame_revisions rev ON rev.frame_id = j.frame_id
                 WHERE rev.search_current = 1 AND rev.retained_state = 'retained'
               )",
        )
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn requeue_retained_indexing_for_rebuild_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<(), A2aLabError> {
        sqlx::query(
            "UPDATE semantic_indexing_job_runtime
             SET job_state = CASE
                   WHEN description_generation IS NOT NULL THEN 'described'
                   ELSE 'pending'
                 END,
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 completed_at_us = NULL
             WHERE job_id IN (
               SELECT j.job_id FROM semantic_indexing_jobs j
               JOIN semantic_frame_revisions rev ON rev.frame_id = j.frame_id
               WHERE rev.search_current = 1 AND rev.retained_state = 'retained'
             )",
        )
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
        Ok(())
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

    /// Shared persist hook slot for [`DeclarativeVideoRuntime::start`].
    #[must_use]
    pub fn runtime_persist_hooks() -> (Vec<Arc<dyn DvrPersistHook>>, Arc<OnceLock<IndexedVideoDvr>>)
    {
        let store_slot = Arc::new(OnceLock::new());
        let hook = Arc::new(SemanticPersistHook::new(Arc::clone(&store_slot)));
        (vec![hook], store_slot)
    }

    /// Wraps an already-opened devkit chunk store and opens the semantic database beside it.
    pub async fn adopt_opened(
        chunk: SqliteChunkDvrStore,
        store_slot: Arc<OnceLock<IndexedVideoDvr>>,
    ) -> Result<Self, A2aLabError> {
        let semantic_path = chunk.root().join("semantic.sqlite");
        Self::adopt_opened_at(chunk, store_slot, semantic_path).await
    }

    /// Wraps media storage and opens the semantic database at `semantic_sqlite`.
    pub async fn adopt_opened_at(
        chunk: SqliteChunkDvrStore,
        store_slot: Arc<OnceLock<IndexedVideoDvr>>,
        semantic_sqlite: impl AsRef<Path>,
    ) -> Result<Self, A2aLabError> {
        let semantic = SemanticSqliteStore::open(semantic_sqlite).await?;
        let store = Self {
            chunk,
            semantic,
            save_faults: Arc::new(SaveFaults::default()),
            indexing_sources: Arc::new(RwLock::new(IndexedSourceFilter::all_sources())),
            semantic_indexing: Arc::new(RwLock::new(None)),
            hook_error: Arc::new(Mutex::new(None)),
        };
        let _ = store_slot.set(store.clone());
        store.finish_opening().await?;
        Ok(store)
    }

    /// Opens or creates the store. Validates storage before capture starts.
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
        if retain_secs == 0 {
            return Err(A2aLabError::invalid(
                "video_disk_retention_secs",
                "must be greater than zero",
            ));
        }
        let root = root.as_ref();
        if root.as_os_str().is_empty() {
            return Err(A2aLabError::invalid("video_dvr_root", "must not be empty"));
        }
        let store_slot: Arc<OnceLock<IndexedVideoDvr>> = Arc::new(OnceLock::new());
        let hook = Arc::new(SemanticPersistHook::new(Arc::clone(&store_slot)));
        let chunk = SqliteChunkDvrStore::open_with_hooks(root, retain_secs, vec![hook]).await?;
        Self::adopt_opened_at(chunk, store_slot, semantic_sqlite).await
    }

    async fn finish_opening(&self) -> Result<(), A2aLabError> {
        h264_epoch::migrate_h264_epochs(self).await?;
        let _ = self.reconcile_frame_decision_tombstones().await?;
        Ok(())
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        self.chunk.root()
    }

    pub async fn append_mjpeg_frame(
        &self,
        source_id: &VideoSourceId,
        captured_at: UtcTimestamp,
        frame_period_us: u64,
        jpeg: &[u8],
    ) -> Result<(), A2aLabError> {
        if self.save_faults.fail_payload_append() {
            return Err(A2aLabError::unavailable(
                "video DVR test fault: payload append blocked",
            ));
        }
        *self
            .hook_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.chunk
            .append_mjpeg_frame(source_id, captured_at, frame_period_us, jpeg)
            .await?;
        self.propagate_hook_failure()
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
        if self.save_faults.fail_payload_append() {
            return Err(A2aLabError::unavailable(
                "video DVR test fault: payload append blocked",
            ));
        }
        *self
            .hook_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.chunk
            .append_h264_sample(
                source_id,
                captured_at,
                frame_period_us,
                keyframe,
                codec_init,
                sample,
            )
            .await?;
        self.propagate_hook_failure()
    }

    fn propagate_hook_failure(&self) -> Result<(), A2aLabError> {
        self.hook_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .map_or(Ok(()), Err)
    }

    pub(crate) async fn on_frame_committed(
        &self,
        frame: &DvrCommittedFrame,
    ) -> Result<(), A2aLabError> {
        if self.save_faults.fail_metadata_commit() {
            return Err(A2aLabError::unavailable(
                "video DVR test fault: metadata commit blocked",
            ));
        }
        let row = self
            .chunk_frame_row(frame.source_id(), frame.track_id(), frame.start())
            .await?;
        let bytes = self.load_payload_bytes_from_retained_row(&row).await?;
        let checksum = crc32(&bytes);
        if checksum != frame.payload_crc32() {
            return Err(A2aLabError::unavailable(
                "committed frame checksum mismatch",
            ));
        }
        let end = crate::time::from_unix_microseconds(row.try_get("end_us").map_err(db_error)?)?;
        let store_ns = DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE)?;
        let saved = SavedFrameRef::from_payload_bytes(
            store_ns,
            frame.source_id().clone(),
            frame.track_id().clone(),
            frame.start(),
            &bytes,
            None,
        )?;
        let registered_at = crate::time::now()?;
        let create_indexing_job = self.permits_indexing_source(frame.source_id().as_str())
            && self
                .indexing_job_needed_for_frame(
                    frame.source_id().as_str(),
                    frame.track_id().as_str(),
                    frame.start().as_micros(),
                )
                .await?;
        let mut tx = self.semantic_pool().begin().await.map_err(db_error)?;
        self.register_manifest_revision_tx(
            &mut tx,
            &saved,
            end,
            checksum,
            registered_at.as_micros(),
        )
        .await?;
        if create_indexing_job {
            commit_indexing_job_tx(
                &mut tx,
                saved.frame_id().as_str(),
                registered_at.as_micros(),
                INDEXING_PRIORITY_LIVE,
            )
            .await?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }

    pub(crate) fn record_hook_failure(&self, error: A2aLabError) {
        *self
            .hook_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
    }

    pub async fn cleanup_source(
        &self,
        source_id: &VideoSourceId,
        now: UtcTimestamp,
    ) -> Result<(), A2aLabError> {
        self.chunk.cleanup_source(source_id, now).await
    }

    pub async fn cleanup_at(&self, now: UtcTimestamp) -> Result<(), A2aLabError> {
        self.chunk.cleanup_at(now).await
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

    async fn chunk_frame_row(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        start_us: UtcTimestamp,
    ) -> Result<SqliteRow, A2aLabError> {
        let key = start_us.as_micros();
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
        frame.ok_or_else(|| {
            A2aLabError::not_found("DVR frame", format!("{source_id}/{track_id}/{key}"))
        })
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

    /// DVR media metadata. Semantic tables are not in this database.
    #[must_use]
    pub fn media_pool(&self) -> &SqlitePool {
        self.chunk.media_pool()
    }

    /// Semantic jobs, manifests, embeddings, and decisions.
    #[must_use]
    pub fn semantic_pool(&self) -> &SqlitePool {
        self.semantic.pool()
    }

    pub(crate) async fn list_ready_media_keys(
        &self,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>, A2aLabError> {
        self.chunk.list_ready_media_keys(limit).await
    }

    pub(crate) async fn retained_media_keys(
        &self,
        keys: &[(String, String, i64)],
    ) -> Result<std::collections::HashSet<(String, String, i64)>, A2aLabError> {
        self.chunk.retained_media_keys(keys).await
    }

    pub(crate) async fn current_semantic_revision_keys(
        &self,
        keys: &[(String, String, i64)],
    ) -> Result<std::collections::HashSet<(String, String, i64)>, A2aLabError> {
        self.semantic.current_revision_keys(keys).await
    }

    /// Attaches resolved semantic-indexing schedules for schedule-aware job registration.
    pub fn set_semantic_indexing_config(&self, config: SemanticIndexingConfig) {
        *self
            .semantic_indexing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config);
    }

    pub(crate) fn semantic_indexing_config(&self) -> Option<SemanticIndexingConfig> {
        self.semantic_indexing
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether a durable indexing job is needed for one saved frame key.
    pub(crate) async fn indexing_job_needed_for_frame(
        &self,
        source_id: &str,
        track_id: &str,
        start_us: i64,
    ) -> Result<bool, A2aLabError> {
        let Some(config) = self.semantic_indexing_config() else {
            return Ok(true);
        };
        if config.embeddings.is_none() {
            let schedule = config.schedule_for(source_id);
            return self
                .select_semantic_stage_frame(
                    source_id,
                    track_id,
                    SemanticIndexingStage::Description.id(),
                    start_us,
                    schedule.description(),
                )
                .await;
        }
        let schedule = config.schedule_for(source_id);
        let description_due = self
            .select_semantic_stage_frame(
                source_id,
                track_id,
                SemanticIndexingStage::Description.id(),
                start_us,
                schedule.description(),
            )
            .await?;
        let embedding_due = self
            .select_semantic_stage_frame(
                source_id,
                track_id,
                SemanticIndexingStage::Embeddings.id(),
                start_us,
                schedule.embeddings(),
            )
            .await?;
        Ok(description_due || embedding_due)
    }

    pub fn set_indexed_source_filter(&self, filter: IndexedSourceFilter) {
        if let Ok(mut guard) = self.indexing_sources.write() {
            *guard = filter;
        }
    }

    pub(crate) fn permits_indexing_source(&self, source_id: &str) -> bool {
        self.indexing_sources
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .permits(source_id)
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
        self.chunk.retention_bounds(source_id).await
    }

    async fn media_coverage(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<DvrMediaCoverage, A2aLabError> {
        self.chunk.media_coverage(source_id).await
    }

    async fn list_tracks(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<Vec<DvrMediaTrack>, A2aLabError> {
        self.chunk.list_tracks(source_id).await
    }

    async fn read_media_window(
        &self,
        request: ReadDvrMediaWindowRequest,
    ) -> Result<Page<DvrMediaSegment>, A2aLabError> {
        self.chunk.read_media_window(request).await
    }

    async fn read_media_spans(
        &self,
        request: ReadDvrMediaWindowRequest,
    ) -> Result<Page<DvrMediaSegment>, A2aLabError> {
        self.chunk.read_media_spans(request).await
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
