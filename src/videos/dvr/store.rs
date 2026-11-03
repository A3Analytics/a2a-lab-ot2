//! Indexed on-disk DVR implementing [`DvrMediaStore`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabError, DvrFrameAtRequest, DvrFrameAtResult, DvrMediaCoverage, DvrMediaPayload,
    DvrMediaSegment, DvrMediaStore, DvrMediaTrack, DvrRetentionBounds, DvrTrackId, MemoryVideos,
    Page, ReadDvrMediaWindowRequest, TimeRange, UtcTimestamp, VideoCodec, VideoSourceId,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use tokio::task;

use super::coverage::coverage_from_starts;

const SCHEMA: &str = include_str!("../../../migrations/004_video_dvr.sql");

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
}

impl IndexedVideoDvr {
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
        for statement in SCHEMA.split(';') {
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
        };
        store.recover_partials().await?;
        store.cleanup_at(crate::time::now()?).await?;
        Ok(store)
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
        self.write_segment(
            source_id,
            &track_id,
            captured_at,
            frame_period_us,
            true,
            jpeg,
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
        let sources = sqlx::query("SELECT DISTINCT source_id FROM dvr_segments")
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

    async fn write_segment(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        captured_at: UtcTimestamp,
        frame_period_us: u64,
        keyframe: bool,
        payload: &[u8],
    ) -> Result<(), A2aLabError> {
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
        Ok(())
    }

    async fn require_source(&self, source_id: &VideoSourceId) -> Result<(), A2aLabError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM dvr_segments WHERE source_id = ?1 AND payload_ready = 1",
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

    async fn segment_starts(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<Vec<UtcTimestamp>, A2aLabError> {
        let rows = sqlx::query(
            "SELECT start_us FROM dvr_segments
             WHERE source_id = ?1 AND payload_ready = 1
             ORDER BY start_us",
        )
        .bind(source_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                let micros: i64 = row.try_get("start_us").map_err(db_error)?;
                crate::time::from_unix_microseconds(micros)
            })
            .collect()
    }

    async fn load_payload(&self, relpath: &str) -> Result<Vec<u8>, A2aLabError> {
        let path = self.root.join(relpath);
        let display = path.display().to_string();
        task::spawn_blocking(move || std::fs::read(path))
            .await
            .map_err(join_error)?
            .map_err(|error| A2aLabError::unavailable(format!("read {display}: {error}")))
    }
}

impl DvrMediaStore for IndexedVideoDvr {
    async fn retention_bounds(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<DvrRetentionBounds, A2aLabError> {
        self.require_source(source_id).await?;
        let oldest: i64 = sqlx::query_scalar(
            "SELECT MIN(start_us) FROM dvr_segments WHERE source_id = ?1 AND payload_ready = 1",
        )
        .bind(source_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        let newest: i64 = sqlx::query_scalar(
            "SELECT MAX(end_us) FROM dvr_segments WHERE source_id = ?1 AND payload_ready = 1",
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
        let starts = self.segment_starts(source_id).await?;
        coverage_from_starts(&starts, period)
    }

    async fn list_tracks(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<Vec<DvrMediaTrack>, A2aLabError> {
        self.require_source(source_id).await?;
        let rows = sqlx::query(
            "SELECT track_id, codec, codec_init, MIN(start_us) AS min_start, MAX(end_us) AS max_end
             FROM dvr_segments
             JOIN dvr_tracks USING (source_id, track_id)
             WHERE source_id = ?1 AND payload_ready = 1
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
            "SELECT track_id, start_us, end_us, keyframe, payload_relpath, codec
             FROM dvr_segments
             JOIN dvr_tracks USING (source_id, track_id)
             WHERE source_id = ?1 AND track_id = ?2
               AND start_us < ?4 AND end_us > ?3
               AND payload_ready = 1
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

    async fn frame_at(&self, request: DvrFrameAtRequest) -> Result<DvrFrameAtResult, A2aLabError> {
        self.require_source(request.source_id()).await?;
        let coverage = self.media_coverage(request.source_id()).await?;
        let at = request.at();
        if !coverage
            .intervals()
            .iter()
            .any(|interval| interval.contains(at))
        {
            if coverage
                .discontinuities()
                .iter()
                .any(|gap| gap.gap().contains(at))
            {
                return Err(A2aLabError::recording_gap("requested instant is in a gap"));
            }
            return Err(A2aLabError::no_recording(
                "requested instant is not retained",
            ));
        }
        let row = sqlx::query(
            "SELECT track_id, start_us, end_us, keyframe, payload_relpath, codec
             FROM dvr_segments
             JOIN dvr_tracks USING (source_id, track_id)
             WHERE source_id = ?1 AND track_id = ?2 AND payload_ready = 1
               AND start_us <= ?3
             ORDER BY ABS(start_us - ?3), start_us
             LIMIT 1",
        )
        .bind(request.source_id().as_str())
        .bind(request.track_id().as_str())
        .bind(at.as_micros())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?
        .ok_or_else(|| A2aLabError::not_found("DVR frame", at.to_string()))?;
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

async fn segment_from_row(
    store: &IndexedVideoDvr,
    row: &SqliteRow,
) -> Result<DvrMediaSegment, A2aLabError> {
    let track_id = DvrTrackId::new(row.try_get::<String, _>("track_id").map_err(db_error)?)?;
    let start = crate::time::from_unix_microseconds(row.try_get("start_us").map_err(db_error)?)?;
    let end = crate::time::from_unix_microseconds(row.try_get("end_us").map_err(db_error)?)?;
    let keyframe: i64 = row.try_get("keyframe").map_err(db_error)?;
    let relpath: String = row.try_get("payload_relpath").map_err(db_error)?;
    let codec = parse_codec(&row.try_get::<String, _>("codec").map_err(db_error)?)?;
    let bytes = store.load_payload(&relpath).await?;
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
            remove(&path)?;
        }
    }
    Ok(())
}

fn walk_incomplete_index(_root: &Path, _stale: &mut Vec<(String, String, i64)>) {}

fn db_error(error: impl std::fmt::Display) -> A2aLabError {
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

fn parse_codec(label: &str) -> Result<VideoCodec, A2aLabError> {
    match label {
        "mjpeg" => Ok(VideoCodec::Mjpeg),
        "h264" => Ok(VideoCodec::H264),
        "h265" => Ok(VideoCodec::H265),
        _ => Err(A2aLabError::invalid("codec", "unknown stored codec")),
    }
}
