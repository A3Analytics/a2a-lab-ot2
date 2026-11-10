//! Indexed on-disk DVR implementing [`DvrMediaStore`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabError, DvrFrameAtRequest, DvrFrameAtResult, DvrMediaCoverage, DvrMediaPayload,
    DvrMediaSegment, DvrMediaStore, DvrMediaTrack, DvrRetentionBounds, DvrStoreNamespace,
    DvrTrackId, MemoryVideos, Page, ReadDvrMediaWindowRequest, SavedFrameRef, TimeRange,
    UtcTimestamp, VideoCodec, VideoSourceId,
};
use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use tokio::task;

use super::chunk::{
    append_payload, chunk_relpath, chunk_tmp_relpath, finalize_chunk, minute_start_us,
    read_payload_range, truncate_chunk_files,
};
use super::coverage::IntervalFold;
use super::frame_manifest::DEFAULT_SEMANTIC_STORE_NAMESPACE;
use super::indexing_registration::{INDEXING_PRIORITY_LIVE, SaveFaults, commit_indexing_job_tx};
use super::indexing_scope::IndexedSourceFilter;

const SCHEMA: &str = include_str!("../../../migrations/004_video_dvr.sql");
const SCHEMA_CHUNKS: &str = include_str!("../../../migrations/005_video_dvr_chunks.sql");
const SCHEMA_SEMANTIC_MANIFEST: &str =
    include_str!("../../../migrations/007_semantic_frame_manifest.sql");
const SCHEMA_INDEXING_JOBS: &str =
    include_str!("../../../migrations/008_semantic_indexing_jobs.sql");
const SCHEMA_FRAME_DECISIONS: &str = include_str!("../../../migrations/009_frame_decisions.sql");
const SCHEMA_INDEXING_JOB_LIFECYCLE: &str =
    include_str!("../../../migrations/010_semantic_indexing_job_lifecycle.sql");
const SCHEMA_DECISION_EVALUATION_JOBS: &str =
    include_str!("../../../migrations/011_frame_decision_evaluation_jobs.sql");
const SCHEMA_SEMANTIC_STAGE_SCHEDULES: &str =
    include_str!("../../../migrations/012_semantic_stage_schedules.sql");
const SCHEMA_EMBEDDING_CONFIG_IDENTITY: &str =
    include_str!("../../../migrations/013_embedding_config_identity.sql");
const SCHEMA_INDEXING_JOB_PRIORITY: &str =
    include_str!("../../../migrations/014_semantic_indexing_job_priority.sql");
const SCHEMA_EMBEDDING_CONFIG_GENERATION: &str =
    include_str!("../../../migrations/015_embedding_config_generation.sql");

/// Default root directory for indexed video payloads and SQLite index.
pub const DEFAULT_VIDEO_DVR_ROOT: &str = ".a2a-lab-ot2/video-dvr";

/// Default on-disk retention (three days).
pub const DEFAULT_DISK_VIDEO_RETENTION_SECS: u64 = 3 * 24 * 60 * 60;

/// Hourly cleanup while serve is running.
pub const VIDEO_DVR_CLEANUP_INTERVAL: Duration = Duration::from_hours(1);

/// Restart-safe indexed DVR with SQLite metadata and atomic payload files.
#[derive(Clone)]
pub struct IndexedVideoDvr {
    root: PathBuf,
    pool: SqlitePool,
    retain_us: i64,
    pub(crate) save_faults: Arc<SaveFaults>,
    indexing_sources: Arc<RwLock<IndexedSourceFilter>>,
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
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        if prior.as_deref() == Some(digest.as_str()) {
            return Ok(false);
        }
        let now_us = crate::time::now()?.as_micros();
        let mut tx = self.pool.begin().await.map_err(db_error)?;
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

    /// Clears searchable vector payloads and ready eligibility before identity rotation.
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

    /// Requeues durable indexing jobs after vectors are gone and identity is current.
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
        .fetch_optional(self.sqlite_pool())
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
        .fetch_one(self.sqlite_pool())
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
        .fetch_one(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(u64::try_from(count.max(0)).unwrap_or(0))
    }

    /// Active embedding model-space generation from the persisted config identity row.
    pub async fn active_model_space_generation(&self) -> Result<Option<u64>, A2aLabError> {
        let generation: Option<i64> = sqlx::query_scalar(
            "SELECT model_space_generation FROM semantic_embedding_config_identity WHERE singleton_id = 1",
        )
        .fetch_optional(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(generation.and_then(|value| u64::try_from(value.max(0)).ok()))
    }

    /// Opens or creates the store. Validates storage before capture starts.
    pub async fn open(root: impl AsRef<Path>, retain_secs: u64) -> Result<Self, A2aLabError> {
        if retain_secs == 0 {
            return Err(A2aLabError::invalid(
                "video_disk_retention_secs",
                "must be greater than zero",
            ));
        }
        let root = root.as_ref().to_path_buf();
        if root.as_os_str().is_empty() {
            return Err(A2aLabError::invalid("video_dvr_root", "must not be empty"));
        }
        if let Some(parent) = root.parent()
            && !parent.as_os_str().is_empty()
        {
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
        task::spawn_blocking({
            let root = root.clone();
            move || std::fs::create_dir_all(&root)
        })
        .await
        .map_err(join_error)?
        .map_err(|error| A2aLabError::unavailable(format!("create {}: {error}", root.display())))?;
        let db_path = root.join("index.sqlite");
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(db_error)?;
        for statement in SCHEMA
            .split(';')
            .chain(SCHEMA_CHUNKS.split(';'))
            .chain(SCHEMA_SEMANTIC_MANIFEST.split(';'))
            .chain(SCHEMA_INDEXING_JOBS.split(';'))
            .chain(SCHEMA_FRAME_DECISIONS.split(';'))
            .chain(SCHEMA_INDEXING_JOB_LIFECYCLE.split(';'))
            .chain(SCHEMA_DECISION_EVALUATION_JOBS.split(';'))
            .chain(SCHEMA_SEMANTIC_STAGE_SCHEDULES.split(';'))
            .chain(SCHEMA_EMBEDDING_CONFIG_IDENTITY.split(';'))
        {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&pool)
                .await
                .map_err(db_error)?;
        }
        let store = Self {
            root,
            pool,
            retain_us: i64::try_from(retain_secs.saturating_mul(1_000_000)).unwrap_or(i64::MAX),
            save_faults: Arc::new(SaveFaults::default()),
            indexing_sources: Arc::new(RwLock::new(IndexedSourceFilter::all_sources())),
        };
        store.migrate_semantic_embedding_vectors().await?;
        store.migrate_indexing_job_priority().await?;
        store.migrate_embedding_config_generation().await?;
        store.backfill_indexing_job_runtime().await?;
        store.recover_partials().await?;
        store.cleanup_at(crate::time::now()?).await?;
        store.reconcile_frame_decision_tombstones().await?;
        Ok(store)
    }

    async fn migrate_semantic_embedding_vectors(&self) -> Result<(), A2aLabError> {
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM pragma_table_info('semantic_frame_embeddings')
             WHERE name = 'embedding_vector' LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        if exists.is_none() {
            sqlx::query("ALTER TABLE semantic_frame_embeddings ADD COLUMN embedding_vector BLOB")
                .execute(&self.pool)
                .await
                .map_err(db_error)?;
        }
        Ok(())
    }

    async fn migrate_embedding_config_generation(&self) -> Result<(), A2aLabError> {
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM pragma_table_info('semantic_embedding_config_identity')
             WHERE name = 'model_space_generation' LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        if exists.is_some() {
            return Ok(());
        }
        for statement in SCHEMA_EMBEDDING_CONFIG_GENERATION.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&self.pool)
                .await
                .map_err(db_error)?;
        }
        Ok(())
    }

    async fn migrate_indexing_job_priority(&self) -> Result<(), A2aLabError> {
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM pragma_table_info('semantic_indexing_jobs')
             WHERE name = 'enqueue_priority' LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        if exists.is_some() {
            return Ok(());
        }
        for statement in SCHEMA_INDEXING_JOB_PRIORITY.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&self.pool)
                .await
                .map_err(db_error)?;
        }
        Ok(())
    }

    async fn backfill_indexing_job_runtime(&self) -> Result<(), A2aLabError> {
        sqlx::query(
            "INSERT INTO semantic_indexing_job_runtime (job_id)
             SELECT job_id FROM semantic_indexing_jobs
             WHERE job_id NOT IN (SELECT job_id FROM semantic_indexing_job_runtime)",
        )
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Indexes one MJPEG frame for `source_id` on the implicit track.
    pub async fn append_mjpeg_frame(
        &self,
        source_id: &VideoSourceId,
        captured_at: UtcTimestamp,
        frame_period_us: u64,
        jpeg: &[u8],
    ) -> Result<(), A2aLabError> {
        let track_id = MemoryVideos::mjpeg_track_id(source_id)?;
        self.ensure_track(
            source_id,
            &track_id,
            VideoCodec::Mjpeg,
            &[],
            frame_period_us,
        )
        .await?;
        self.write_chunk_frame(
            source_id,
            &track_id,
            captured_at,
            frame_period_us,
            true,
            jpeg,
        )
        .await
    }

    /// Indexes one H.264 access unit on the `h264` track.
    pub async fn append_h264_sample(
        &self,
        source_id: &VideoSourceId,
        captured_at: UtcTimestamp,
        frame_period_us: u64,
        keyframe: bool,
        codec_init: &[u8],
        sample: &[u8],
    ) -> Result<(), A2aLabError> {
        let track_id = DvrTrackId::new("h264")?;
        self.ensure_track(
            source_id,
            &track_id,
            VideoCodec::H264,
            codec_init,
            frame_period_us,
        )
        .await?;
        sqlx::query(
            "UPDATE dvr_tracks SET codec_init = ?3
             WHERE source_id = ?1 AND track_id = ?2 AND codec_init != ?3",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(codec_init)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        self.write_chunk_frame(
            source_id,
            &track_id,
            captured_at,
            frame_period_us,
            keyframe,
            sample,
        )
        .await
    }

    /// Removes expired rows, payloads, temp files, and incomplete writes for `source_id` only.
    pub async fn cleanup_source(
        &self,
        source_id: &VideoSourceId,
        now: UtcTimestamp,
    ) -> Result<(), A2aLabError> {
        self.cleanup_source_id(source_id.as_str(), now).await
    }

    /// Global cleanup across every indexed source.
    pub async fn cleanup_at(&self, now: UtcTimestamp) -> Result<(), A2aLabError> {
        let sources = sqlx::query(
            "SELECT DISTINCT source_id FROM dvr_segments
             UNION
             SELECT DISTINCT source_id FROM dvr_frames",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        for row in sources {
            let source_id: String = row.try_get("source_id").map_err(db_error)?;
            self.cleanup_source_id(&source_id, now).await?;
        }
        self.recover_partials().await?;
        Ok(())
    }

    async fn ensure_track(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        codec: VideoCodec,
        codec_init: &[u8],
        frame_period_us: u64,
    ) -> Result<(), A2aLabError> {
        sqlx::query(
            "INSERT INTO dvr_tracks (source_id, track_id, codec, codec_init, frame_period_us)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source_id, track_id) DO NOTHING",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(codec_label(codec))
        .bind(codec_init)
        .bind(i64::try_from(frame_period_us).unwrap_or(i64::MAX))
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn write_chunk_frame(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        captured_at: UtcTimestamp,
        frame_period_us: u64,
        keyframe: bool,
        payload: &[u8],
    ) -> Result<(), A2aLabError> {
        if self.save_faults.fail_payload_append() {
            return Err(A2aLabError::unavailable(
                "video DVR test fault: payload append blocked",
            ));
        }
        let minute = minute_start_us(captured_at);
        self.finalize_chunks_before(source_id, track_id, minute)
            .await?;
        let relpath = chunk_relpath(source_id.as_str(), track_id.as_str(), minute);
        let tmp_relpath = chunk_tmp_relpath(source_id.as_str(), track_id.as_str(), minute);
        sqlx::query(
            "INSERT INTO dvr_chunks (source_id, track_id, minute_start_us, chunk_relpath, chunk_ready)
             VALUES (?1, ?2, ?3, ?4, 0)
             ON CONFLICT(source_id, track_id, minute_start_us) DO NOTHING",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(minute)
        .bind(&relpath)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        let tmp_path = self.root.join(&tmp_relpath);
        let (offset, checksum) = append_payload(&tmp_path, payload).await?;
        let end =
            captured_at.saturating_add_micros(i64::try_from(frame_period_us.max(1)).unwrap_or(1));
        let store_ns = DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE)?;
        let frame = SavedFrameRef::from_payload_bytes(
            store_ns,
            source_id.clone(),
            track_id.clone(),
            captured_at,
            payload,
            None,
        )?;
        let registered_at = crate::time::now()?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        sqlx::query(
            "INSERT INTO dvr_frames
                (source_id, track_id, start_us, end_us, keyframe, minute_start_us, chunk_relpath,
                 payload_offset, payload_length, payload_crc32, payload_ready)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1)
             ON CONFLICT(source_id, track_id, start_us) DO UPDATE SET
                end_us = excluded.end_us,
                keyframe = excluded.keyframe,
                minute_start_us = excluded.minute_start_us,
                chunk_relpath = excluded.chunk_relpath,
                payload_offset = excluded.payload_offset,
                payload_length = excluded.payload_length,
                payload_crc32 = excluded.payload_crc32,
                payload_ready = 1",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(captured_at.as_micros())
        .bind(end.as_micros())
        .bind(i64::from(keyframe))
        .bind(minute)
        .bind(&relpath)
        .bind(i64::try_from(offset).unwrap_or(i64::MAX))
        .bind(i64::try_from(payload.len()).unwrap_or(i64::MAX))
        .bind(i64::from(checksum))
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        self.register_manifest_revision_tx(
            &mut tx,
            &frame,
            end,
            checksum,
            registered_at.as_micros(),
        )
        .await?;
        if self
            .indexing_sources
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .permits(source_id.as_str())
        {
            commit_indexing_job_tx(
                &mut tx,
                frame.frame_id().as_str(),
                registered_at.as_micros(),
                INDEXING_PRIORITY_LIVE,
            )
            .await?;
        }
        if self.save_faults.fail_metadata_commit() {
            return Err(A2aLabError::unavailable(
                "video DVR test fault: metadata commit blocked",
            ));
        }
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }

    async fn finalize_chunks_before(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        minute: i64,
    ) -> Result<(), A2aLabError> {
        let rows = sqlx::query(
            "SELECT minute_start_us, chunk_relpath FROM dvr_chunks
             WHERE source_id = ?1 AND track_id = ?2 AND chunk_ready = 0 AND minute_start_us < ?3",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(minute)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        for row in rows {
            let minute_start: i64 = row.try_get("minute_start_us").map_err(db_error)?;
            let relpath: String = row.try_get("chunk_relpath").map_err(db_error)?;
            self.finalize_chunk_minute(source_id, track_id, minute_start, &relpath)
                .await?;
        }
        Ok(())
    }

    async fn finalize_chunk_minute(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        minute_start: i64,
        relpath: &str,
    ) -> Result<(), A2aLabError> {
        let tmp_relpath = chunk_tmp_relpath(source_id.as_str(), track_id.as_str(), minute_start);
        let tmp = self.root.join(&tmp_relpath);
        let final_path = self.root.join(relpath);
        finalize_chunk(&tmp, &final_path).await?;
        sqlx::query(
            "UPDATE dvr_chunks SET chunk_ready = 1, chunk_relpath = ?4
             WHERE source_id = ?1 AND track_id = ?2 AND minute_start_us = ?3",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(minute_start)
        .bind(relpath)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
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
        self.ensure_track(source_id, track_id, VideoCodec::Mjpeg, &[], frame_period_us)
            .await?;
        let end =
            captured_at.saturating_add_micros(i64::try_from(frame_period_us.max(1)).unwrap_or(1));
        let relpath = format!(
            "{}/{}/{}.mjpeg",
            source_id.as_str(),
            track_id.as_str(),
            captured_at.as_micros()
        );
        let abs = self.root.join(&relpath);
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
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn recover_partials(&self) -> Result<(), A2aLabError> {
        let root = self.root.clone();
        let removed_rows = task::spawn_blocking(move || recover_tmp_files(&root))
            .await
            .map_err(join_error)??;
        if !removed_rows.is_empty() {
            for (source_id, track_id, start_us) in removed_rows {
                sqlx::query(
                    "DELETE FROM dvr_segments
                     WHERE source_id = ?1 AND track_id = ?2 AND start_us = ?3 AND payload_ready = 0",
                )
                .bind(source_id)
                .bind(track_id)
                .bind(start_us)
                .execute(&self.pool)
                .await
                .map_err(db_error)?;
            }
        }
        sqlx::query(
            "DELETE FROM dvr_segments
             WHERE payload_ready = 0 OR payload_relpath = ''",
        )
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        sqlx::query("DELETE FROM dvr_frames WHERE payload_ready = 0")
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        let indexed = sqlx::query(
            "SELECT chunk_relpath, MAX(payload_offset + payload_length) AS chunk_end
             FROM dvr_frames
             WHERE payload_ready = 1 AND chunk_relpath != ''
             GROUP BY chunk_relpath",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        let mut truncate_to = Vec::new();
        for row in indexed {
            let relpath: String = row.try_get("chunk_relpath").map_err(db_error)?;
            let end: i64 = row.try_get("chunk_end").map_err(db_error)?;
            truncate_to.push((relpath, u64::try_from(end.max(0)).unwrap_or(0)));
        }
        let root = self.root.clone();
        task::spawn_blocking(move || truncate_chunk_files(&root, &truncate_to))
            .await
            .map_err(join_error)??;
        sqlx::query(
            "DELETE FROM dvr_chunks
             WHERE chunk_ready = 0
               AND NOT EXISTS (
                 SELECT 1 FROM dvr_frames
                 WHERE dvr_frames.source_id = dvr_chunks.source_id
                   AND dvr_frames.track_id = dvr_chunks.track_id
                   AND dvr_frames.minute_start_us = dvr_chunks.minute_start_us
                   AND dvr_frames.payload_ready = 1
               )",
        )
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn cleanup_source_id(
        &self,
        source_id: &str,
        now: UtcTimestamp,
    ) -> Result<(), A2aLabError> {
        let cutoff = now.saturating_add_micros(-self.retain_us);
        let rows = sqlx::query(
            "SELECT track_id, start_us, payload_relpath FROM dvr_segments
             WHERE source_id = ?1 AND start_us < ?2",
        )
        .bind(source_id)
        .bind(cutoff.as_micros())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        for row in rows {
            let relpath: String = row.try_get("payload_relpath").map_err(db_error)?;
            let path = self.root.join(relpath);
            let _ = task::spawn_blocking(move || std::fs::remove_file(path))
                .await
                .map_err(join_error)?;
        }
        sqlx::query("DELETE FROM dvr_segments WHERE source_id = ?1 AND start_us < ?2")
            .bind(source_id)
            .bind(cutoff.as_micros())
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        let frame_rows = sqlx::query(
            "SELECT DISTINCT chunk_relpath FROM dvr_frames
             WHERE source_id = ?1 AND start_us < ?2 AND chunk_relpath != ''",
        )
        .bind(source_id)
        .bind(cutoff.as_micros())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        sqlx::query("DELETE FROM dvr_frames WHERE source_id = ?1 AND start_us < ?2")
            .bind(source_id)
            .bind(cutoff.as_micros())
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        for row in frame_rows {
            let relpath: String = row.try_get("chunk_relpath").map_err(db_error)?;
            self.remove_chunk_if_empty(relpath).await?;
        }
        Ok(())
    }

    async fn remove_chunk_if_empty(&self, relpath: String) -> Result<(), A2aLabError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM dvr_frames WHERE chunk_relpath = ?1 AND payload_ready = 1",
        )
        .bind(&relpath)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        if count != 0 {
            return Ok(());
        }
        sqlx::query("DELETE FROM dvr_chunks WHERE chunk_relpath = ?1")
            .bind(&relpath)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        let path = self.root.join(&relpath);
        let tmp = self.root.join(format!("{relpath}.tmp"));
        task::spawn_blocking(move || {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(tmp);
        })
        .await
        .map_err(join_error)?;
        Ok(())
    }

    async fn require_source(&self, source_id: &VideoSourceId) -> Result<(), A2aLabError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT (
                (SELECT COUNT(*) FROM dvr_segments WHERE source_id = ?1 AND payload_ready = 1)
              + (SELECT COUNT(*) FROM dvr_frames WHERE source_id = ?1 AND payload_ready = 1)
             )",
        )
        .bind(source_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        if count == 0 {
            return Err(A2aLabError::not_found(
                "video source",
                source_id.to_string(),
            ));
        }
        Ok(())
    }

    async fn frame_period_us(&self, source_id: &VideoSourceId) -> Result<u64, A2aLabError> {
        let period: i64 = sqlx::query_scalar(
            "SELECT frame_period_us FROM dvr_tracks WHERE source_id = ?1 LIMIT 1",
        )
        .bind(source_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(u64::try_from(period.max(1)).unwrap_or(1))
    }

    async fn fold_source_starts(
        &self,
        source_id: &VideoSourceId,
        fold: &mut IntervalFold,
    ) -> Result<(), A2aLabError> {
        let mut segments = sqlx::query(
            "SELECT start_us FROM dvr_segments
             WHERE source_id = ?1 AND payload_ready = 1
             ORDER BY start_us",
        )
        .bind(source_id.as_str())
        .fetch(&self.pool);
        let mut frames = sqlx::query(
            "SELECT start_us FROM dvr_frames
             WHERE source_id = ?1 AND payload_ready = 1
             ORDER BY start_us",
        )
        .bind(source_id.as_str())
        .fetch(&self.pool);
        let mut segment = next_start(&mut segments).await?;
        let mut frame = next_start(&mut frames).await?;
        loop {
            let next = match (segment, frame) {
                (Some(left), Some(right)) if left <= right => {
                    segment = next_start(&mut segments).await?;
                    left
                }
                (Some(left), None) => {
                    segment = next_start(&mut segments).await?;
                    left
                }
                (Some(_) | None, Some(right)) => {
                    frame = next_start(&mut frames).await?;
                    right
                }
                (None, None) => break,
            };
            fold.push(crate::time::from_unix_microseconds(next)?)?;
        }
        Ok(())
    }

    /// Loads verified bytes for one retained DVR key (chunk frames or legacy per-file segments).
    pub(crate) async fn load_retained_payload_at_key(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        start_us: UtcTimestamp,
    ) -> Result<(Vec<u8>, u32, UtcTimestamp), A2aLabError> {
        let row = sqlx::query(
            "SELECT end_us, chunk_relpath, payload_offset, payload_length, payload_crc32
             FROM dvr_frames
             WHERE source_id = ?1 AND track_id = ?2 AND start_us = ?3 AND payload_ready = 1",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(start_us.as_micros())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        if let Some(row) = row {
            let end_us: i64 = row.try_get("end_us").map_err(db_error)?;
            let chunk_relpath: String = row.try_get("chunk_relpath").map_err(db_error)?;
            let offset: i64 = row.try_get("payload_offset").map_err(db_error)?;
            let length: i64 = row.try_get("payload_length").map_err(db_error)?;
            let expected_crc: i64 = row.try_get("payload_crc32").map_err(db_error)?;
            let bytes = self
                .load_chunk_payload(
                    &chunk_relpath,
                    u64::try_from(offset.max(0)).unwrap_or(0),
                    u64::try_from(length.max(0)).unwrap_or(0),
                    u32::try_from(expected_crc.max(0)).unwrap_or(0),
                )
                .await?;
            let crc = super::chunk::crc32(&bytes);
            return Ok((bytes, crc, crate::time::from_unix_microseconds(end_us)?));
        }
        let segment = sqlx::query(
            "SELECT end_us, payload_relpath
             FROM dvr_segments
             WHERE source_id = ?1 AND track_id = ?2 AND start_us = ?3 AND payload_ready = 1",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .bind(start_us.as_micros())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        let Some(segment) = segment else {
            return Err(A2aLabError::not_found(
                "DVR frame",
                format!("{source_id}/{track_id}/{}", start_us.as_micros()),
            ));
        };
        let end_us: i64 = segment.try_get("end_us").map_err(db_error)?;
        let relpath: String = segment.try_get("payload_relpath").map_err(db_error)?;
        let bytes = self.load_payload(&relpath).await?;
        let crc = super::chunk::crc32(&bytes);
        Ok((bytes, crc, crate::time::from_unix_microseconds(end_us)?))
    }

    async fn load_payload(&self, relpath: &str) -> Result<Vec<u8>, A2aLabError> {
        let path = self.root.join(relpath);
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
        let root = self.root.clone();
        let final_path = root.join(relpath);
        let tmp_path = root.join(format!("{relpath}.tmp"));
        let path = if tmp_path.exists() {
            tmp_path
        } else {
            final_path
        };
        read_payload_range(&path, offset, length, expected_crc32).await
    }
}

impl DvrMediaStore for IndexedVideoDvr {
    async fn retention_bounds(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<DvrRetentionBounds, A2aLabError> {
        self.require_source(source_id).await?;
        let oldest: i64 = sqlx::query_scalar(
            "SELECT MIN(start_us) FROM (
                SELECT start_us FROM dvr_segments WHERE source_id = ?1 AND payload_ready = 1
                UNION ALL
                SELECT start_us FROM dvr_frames WHERE source_id = ?1 AND payload_ready = 1
             )",
        )
        .bind(source_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        let newest: i64 = sqlx::query_scalar(
            "SELECT MAX(end_us) FROM (
                SELECT end_us FROM dvr_segments WHERE source_id = ?1 AND payload_ready = 1
                UNION ALL
                SELECT end_us FROM dvr_frames WHERE source_id = ?1 AND payload_ready = 1
             )",
        )
        .bind(source_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        DvrRetentionBounds::new(
            crate::time::from_unix_microseconds(oldest)?,
            crate::time::from_unix_microseconds(newest)?,
        )
    }

    async fn media_coverage(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<DvrMediaCoverage, A2aLabError> {
        self.require_source(source_id).await?;
        let period = self.frame_period_us(source_id).await?;
        let mut fold = IntervalFold::new(period);
        self.fold_source_starts(source_id, &mut fold).await?;
        let intervals = fold.finish()?;
        if intervals.is_empty() {
            return Err(A2aLabError::no_recording(
                "source has no retained DVR media",
            ));
        }
        DvrMediaCoverage::new(intervals)
    }

    async fn list_tracks(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<Vec<DvrMediaTrack>, A2aLabError> {
        self.require_source(source_id).await?;
        let rows = sqlx::query(
            "SELECT track_id, codec, codec_init, MIN(start_us) AS min_start, MAX(end_us) AS max_end
             FROM (
                SELECT source_id, track_id, start_us, end_us, codec, codec_init
                FROM dvr_segments
                JOIN dvr_tracks USING (source_id, track_id)
                WHERE source_id = ?1 AND payload_ready = 1
                UNION ALL
                SELECT source_id, track_id, start_us, end_us, codec, codec_init
                FROM dvr_frames
                JOIN dvr_tracks USING (source_id, track_id)
                WHERE source_id = ?1 AND payload_ready = 1
             )
             GROUP BY track_id, codec, codec_init",
        )
        .bind(source_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| track_from_row(&row, source_id))
            .collect()
    }

    async fn read_media_window(
        &self,
        request: ReadDvrMediaWindowRequest,
    ) -> Result<Page<DvrMediaSegment>, A2aLabError> {
        self.require_source(request.source_id()).await?;
        let window = request.window();
        let page = request.page();
        page.check()?;
        let offset = match page.cursor() {
            None => 0_i64,
            Some(cursor) => cursor
                .parse::<i64>()
                .map_err(|_| A2aLabError::invalid("cursor", "is not a valid page cursor"))?,
        };
        let limit = i64::from(page.limit());
        let rows = sqlx::query(
            "SELECT track_id, start_us, end_us, keyframe, payload_relpath, chunk_relpath,
                    payload_offset, payload_length, payload_crc32, codec
             FROM (
                SELECT track_id, start_us, end_us, keyframe, payload_relpath,
                       '' AS chunk_relpath, NULL AS payload_offset, NULL AS payload_length,
                       NULL AS payload_crc32, codec
                FROM dvr_segments
                JOIN dvr_tracks USING (source_id, track_id)
                WHERE source_id = ?1 AND track_id = ?2
                  AND start_us < ?4 AND end_us > ?3
                  AND payload_ready = 1
                UNION ALL
                SELECT track_id, start_us, end_us, keyframe, '' AS payload_relpath, chunk_relpath,
                       payload_offset, payload_length, payload_crc32, codec
                FROM dvr_frames
                JOIN dvr_tracks USING (source_id, track_id)
                WHERE source_id = ?1 AND track_id = ?2
                  AND start_us < ?4 AND end_us > ?3
                  AND payload_ready = 1
             )
             ORDER BY start_us
             LIMIT ?5 OFFSET ?6",
        )
        .bind(request.source_id().as_str())
        .bind(request.track_id().as_str())
        .bind(window.start().as_micros())
        .bind(window.end().as_micros())
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        let mut items = Vec::new();
        for row in rows
            .iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
        {
            items.push(segment_from_row(self, row).await?);
        }
        let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);
        let next_cursor = (rows.len() > limit_usize).then(|| (offset + limit).to_string());
        Ok(Page::new(items, next_cursor))
    }

    async fn read_media_spans(
        &self,
        request: ReadDvrMediaWindowRequest,
    ) -> Result<Page<DvrMediaSegment>, A2aLabError> {
        self.require_source(request.source_id()).await?;
        let window = request.window();
        let page = request.page();
        page.check()?;
        let offset = match page.cursor() {
            None => 0_i64,
            Some(cursor) => cursor
                .parse::<i64>()
                .map_err(|_| A2aLabError::invalid("cursor", "is not a valid page cursor"))?,
        };
        let limit = i64::from(page.limit());
        let rows = sqlx::query(
            "SELECT track_id, start_us, end_us, keyframe, codec
             FROM (
                SELECT track_id, start_us, end_us, keyframe, codec
                FROM dvr_segments
                JOIN dvr_tracks USING (source_id, track_id)
                WHERE source_id = ?1 AND track_id = ?2
                  AND start_us < ?4 AND end_us > ?3
                  AND payload_ready = 1
                UNION ALL
                SELECT track_id, start_us, end_us, keyframe, codec
                FROM dvr_frames
                JOIN dvr_tracks USING (source_id, track_id)
                WHERE source_id = ?1 AND track_id = ?2
                  AND start_us < ?4 AND end_us > ?3
                  AND payload_ready = 1
             )
             ORDER BY start_us
             LIMIT ?5 OFFSET ?6",
        )
        .bind(request.source_id().as_str())
        .bind(request.track_id().as_str())
        .bind(window.start().as_micros())
        .bind(window.end().as_micros())
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        let mut items = Vec::new();
        for row in rows
            .iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
        {
            items.push(span_from_row(row)?);
        }
        let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);
        let next_cursor = (rows.len() > limit_usize).then(|| (offset + limit).to_string());
        Ok(Page::new(items, next_cursor))
    }

    async fn frame_at(&self, request: DvrFrameAtRequest) -> Result<DvrFrameAtResult, A2aLabError> {
        self.require_source(request.source_id()).await?;
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
        let segment = segment_from_row(self, &row).await?;
        Ok(DvrFrameAtResult::new(at, actual, segment.payload().clone()))
    }
}

impl IndexedVideoDvr {
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
            .fetch_optional(&self.pool)
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
    #[must_use]
    pub fn sqlite_pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Restricts durable indexing jobs to configured live sources when semantic indexing is enabled.
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

    /// Returns SQLite's plan rows for a regression test.
    #[doc(hidden)]
    pub async fn query_plan(&self, sql: &str) -> Result<Vec<String>, A2aLabError> {
        let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?;
        rows.into_iter()
            .map(|row| row.try_get::<String, _>(3).map_err(db_error))
            .collect()
    }
}

async fn next_start(
    stream: &mut (impl futures_util::Stream<Item = Result<SqliteRow, sqlx::Error>> + Unpin),
) -> Result<Option<i64>, A2aLabError> {
    let Some(row) = stream.try_next().await.map_err(db_error)? else {
        return Ok(None);
    };
    Ok(Some(row.try_get("start_us").map_err(db_error)?))
}

fn track_from_row(
    row: &SqliteRow,
    source_id: &VideoSourceId,
) -> Result<DvrMediaTrack, A2aLabError> {
    let track_id = DvrTrackId::new(row.try_get::<String, _>("track_id").map_err(db_error)?)?;
    let codec = parse_codec(&row.try_get::<String, _>("codec").map_err(db_error)?)?;
    let init: Vec<u8> = row.try_get("codec_init").map_err(db_error)?;
    let start = crate::time::from_unix_microseconds(row.try_get("min_start").map_err(db_error)?)?;
    let end = crate::time::from_unix_microseconds(row.try_get("max_end").map_err(db_error)?)?;
    let _ = source_id;
    DvrMediaTrack::new(track_id, codec, init, TimeRange::new(start, end)?)
}

fn span_from_row(row: &SqliteRow) -> Result<DvrMediaSegment, A2aLabError> {
    let track_id = DvrTrackId::new(row.try_get::<String, _>("track_id").map_err(db_error)?)?;
    let start = crate::time::from_unix_microseconds(row.try_get("start_us").map_err(db_error)?)?;
    let end = crate::time::from_unix_microseconds(row.try_get("end_us").map_err(db_error)?)?;
    let keyframe: i64 = row.try_get("keyframe").map_err(db_error)?;
    let codec = parse_codec(&row.try_get::<String, _>("codec").map_err(db_error)?)?;
    let payload = match codec {
        VideoCodec::Mjpeg => DvrMediaPayload::MjpegFrame(Vec::new()),
        VideoCodec::H264 | VideoCodec::H265 => DvrMediaPayload::EncodedSample {
            codec,
            bytes: Vec::new(),
        },
    };
    DvrMediaSegment::new(
        track_id,
        TimeRange::new(start, end)?,
        keyframe != 0,
        payload,
    )
}

async fn segment_from_row(
    store: &IndexedVideoDvr,
    row: &SqliteRow,
) -> Result<DvrMediaSegment, A2aLabError> {
    let track_id = DvrTrackId::new(row.try_get::<String, _>("track_id").map_err(db_error)?)?;
    let start = crate::time::from_unix_microseconds(row.try_get("start_us").map_err(db_error)?)?;
    let end = crate::time::from_unix_microseconds(row.try_get("end_us").map_err(db_error)?)?;
    let keyframe: i64 = row.try_get("keyframe").map_err(db_error)?;
    let codec = parse_codec(&row.try_get::<String, _>("codec").map_err(db_error)?)?;
    let chunk_relpath: String = row.try_get("chunk_relpath").map_err(db_error)?;
    let bytes = if chunk_relpath.is_empty() {
        let relpath: String = row.try_get("payload_relpath").map_err(db_error)?;
        store.load_payload(&relpath).await?
    } else {
        let offset: i64 = row.try_get("payload_offset").map_err(db_error)?;
        let length: i64 = row.try_get("payload_length").map_err(db_error)?;
        let crc32: i64 = row.try_get("payload_crc32").map_err(db_error)?;
        store
            .load_chunk_payload(
                &chunk_relpath,
                u64::try_from(offset.max(0)).unwrap_or(0),
                u64::try_from(length.max(0)).unwrap_or(0),
                u32::try_from(crc32.max(0)).unwrap_or(0),
            )
            .await?
    };
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

fn recover_tmp_files(root: &Path) -> Result<Vec<(String, String, i64)>, A2aLabError> {
    let mut stale = Vec::new();
    walk_tmp_files(root, &mut |path| {
        let _ = std::fs::remove_file(path);
        Ok(())
    })?;
    walk_incomplete_index(root, &mut stale);
    Ok(stale)
}

fn walk_tmp_files(
    root: &Path,
    remove: &mut dyn FnMut(&Path) -> Result<(), A2aLabError>,
) -> Result<(), A2aLabError> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root)
        .map_err(|error| A2aLabError::unavailable(format!("read {}: {error}", root.display())))?
    {
        let entry = entry.map_err(|error| {
            A2aLabError::unavailable(format!("read {}: {error}", root.display()))
        })?;
        let path = entry.path();
        if path.is_dir() {
            walk_tmp_files(&path, remove)?;
        } else if path.extension().is_some_and(|ext| ext == "tmp") {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".chunk.tmp"))
            {
                continue;
            }
            remove(&path)?;
        }
    }
    Ok(())
}

fn walk_incomplete_index(_root: &Path, _stale: &mut Vec<(String, String, i64)>) {}

pub(crate) fn db_error(error: impl std::fmt::Display) -> A2aLabError {
    A2aLabError::unavailable(format!("video DVR database: {error}"))
}

fn join_error(error: impl std::fmt::Display) -> A2aLabError {
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
