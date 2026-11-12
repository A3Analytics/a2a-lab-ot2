//! Immutable H.264 DVR epochs bound to decoder initialization and picture size.

use std::path::Path;

use a2a_lab_dev_kit::{
    A2aLabError, DvrMediaTrack, DvrPictureProperties, DvrTrackId, TimeRange, UtcTimestamp,
    VideoCodec, VideoSourceId,
};
use sqlx::{Row, SqlitePool};
use tokio::task;

use super::store::{IndexedVideoDvr, db_error};

const SCHEMA_H264_EPOCHS: &str = include_str!("../../../migrations/016_dvr_h264_epochs.sql");
const CUT_OVER_KEY: &str = "h264_epoch_storage_cutover";

pub(crate) fn epoch_id_from_track(track_id: &DvrTrackId) -> Option<i64> {
    track_id
        .as_str()
        .strip_prefix("h264-")
        .and_then(|suffix| suffix.parse::<i64>().ok())
}

pub(crate) async fn migrate_h264_epochs(store: &IndexedVideoDvr) -> Result<(), A2aLabError> {
    for statement in SCHEMA_H264_EPOCHS.split(';') {
        let statement = statement.trim();
        if statement.is_empty() {
            continue;
        }
        sqlx::query(statement)
            .execute(store.sqlite_pool())
            .await
            .map_err(db_error)?;
    }
    ensure_frames_epoch_column(store).await?;
    if cutover_complete(store).await? {
        return Ok(());
    }
    scoped_h264_cutover_delete(store).await?;
    mark_cutover_complete(store).await
}

async fn ensure_frames_epoch_column(store: &IndexedVideoDvr) -> Result<(), A2aLabError> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM pragma_table_info('dvr_frames') WHERE name = 'h264_epoch_id' LIMIT 1",
    )
    .fetch_optional(store.sqlite_pool())
    .await
    .map_err(db_error)?;
    if exists.is_some() {
        return Ok(());
    }
    sqlx::query("ALTER TABLE dvr_frames ADD COLUMN h264_epoch_id INTEGER REFERENCES dvr_h264_epochs(epoch_id)")
        .execute(store.sqlite_pool())
        .await
        .map_err(db_error)?;
    Ok(())
}

async fn cutover_complete(store: &IndexedVideoDvr) -> Result<bool, A2aLabError> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT meta_value FROM dvr_store_meta WHERE meta_key = ?1")
            .bind(CUT_OVER_KEY)
            .fetch_optional(store.sqlite_pool())
            .await
            .map_err(db_error)?;
    Ok(value.as_deref() == Some("1"))
}

async fn mark_cutover_complete(store: &IndexedVideoDvr) -> Result<(), A2aLabError> {
    sqlx::query(
        "INSERT INTO dvr_store_meta (meta_key, meta_value) VALUES (?1, '1')
         ON CONFLICT(meta_key) DO UPDATE SET meta_value = excluded.meta_value",
    )
    .bind(CUT_OVER_KEY)
    .execute(store.sqlite_pool())
    .await
    .map_err(db_error)?;
    Ok(())
}

async fn scoped_h264_cutover_delete(store: &IndexedVideoDvr) -> Result<(), A2aLabError> {
    let chunks: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT chunk_relpath FROM dvr_frames
         WHERE track_id = 'h264' AND chunk_relpath != ''",
    )
    .fetch_all(store.sqlite_pool())
    .await
    .map_err(db_error)?;
    sqlx::query("DELETE FROM dvr_frames WHERE track_id = 'h264'")
        .execute(store.sqlite_pool())
        .await
        .map_err(db_error)?;
    sqlx::query("DELETE FROM dvr_chunks WHERE track_id = 'h264'")
        .execute(store.sqlite_pool())
        .await
        .map_err(db_error)?;
    sqlx::query(
        "DELETE FROM dvr_segments
         WHERE track_id = 'h264'
            OR (source_id, track_id) IN (
                SELECT source_id, track_id FROM dvr_tracks WHERE track_id = 'h264'
            )",
    )
    .execute(store.sqlite_pool())
    .await
    .map_err(db_error)?;
    sqlx::query("DELETE FROM dvr_tracks WHERE track_id = 'h264'")
        .execute(store.sqlite_pool())
        .await
        .map_err(db_error)?;
    sqlx::query("DELETE FROM dvr_h264_epochs")
        .execute(store.sqlite_pool())
        .await
        .map_err(db_error)?;
    remove_chunk_paths(store.root(), &chunks).await?;
    remove_h264_payload_dirs(store.root()).await
}

async fn remove_chunk_paths(root: &Path, chunks: &[String]) -> Result<(), A2aLabError> {
    let root = root.to_path_buf();
    let paths: Vec<std::path::PathBuf> = chunks
        .iter()
        .flat_map(|relpath| [root.join(relpath), root.join(format!("{relpath}.tmp"))])
        .collect();
    task::spawn_blocking(move || {
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
    })
    .await
    .map_err(super::store::join_error)?;
    Ok(())
}

async fn remove_h264_payload_dirs(root: &Path) -> Result<(), A2aLabError> {
    let root = root.to_path_buf();
    task::spawn_blocking(move || {
        if !root.is_dir() {
            return Ok::<_, A2aLabError>(());
        }
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let source_dir = entry.path().join("h264");
            if source_dir.is_dir() {
                let _ = std::fs::remove_dir_all(source_dir);
            }
        }
        Ok(())
    })
    .await
    .map_err(super::store::join_error)??;
    Ok(())
}

pub(crate) async fn resolve_epoch_id(
    pool: &SqlitePool,
    source_id: &VideoSourceId,
    codec_init: &[u8],
    opened_at: UtcTimestamp,
) -> Result<i64, A2aLabError> {
    let picture = declared_picture_properties(codec_init)?;
    let width = i64::from(picture.width());
    let height = i64::from(picture.height());
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT epoch_id FROM dvr_h264_epochs
         WHERE source_id = ?1 AND codec_init = ?2 AND width = ?3 AND height = ?4",
    )
    .bind(source_id.as_str())
    .bind(codec_init)
    .bind(width)
    .bind(height)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    if let Some(epoch_id) = existing {
        return Ok(epoch_id);
    }
    let result = sqlx::query(
        "INSERT INTO dvr_h264_epochs (source_id, codec_init, width, height, opened_at_us)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(source_id.as_str())
    .bind(codec_init)
    .bind(width)
    .bind(height)
    .bind(opened_at.as_micros())
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(result.last_insert_rowid())
}

pub(crate) async fn codec_init_for_frame(
    pool: &SqlitePool,
    source_id: &VideoSourceId,
    track_id: &DvrTrackId,
    captured_at: UtcTimestamp,
) -> Result<Option<Vec<u8>>, A2aLabError> {
    if track_id.as_str() != "h264" {
        return Ok(None);
    }
    let init: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT e.codec_init FROM dvr_frames f
         JOIN dvr_h264_epochs e ON e.epoch_id = f.h264_epoch_id
         WHERE f.source_id = ?1 AND f.track_id = ?2 AND f.start_us = ?3 AND f.payload_ready = 1",
    )
    .bind(source_id.as_str())
    .bind(track_id.as_str())
    .bind(captured_at.as_micros())
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    Ok(init)
}

pub(crate) async fn list_h264_epoch_tracks(
    pool: &SqlitePool,
    source_id: &VideoSourceId,
) -> Result<Vec<DvrMediaTrack>, A2aLabError> {
    let rows = sqlx::query(
        "SELECT e.epoch_id, e.codec_init, e.width, e.height,
                MIN(f.start_us) AS min_start, MAX(f.end_us) AS max_end
         FROM dvr_h264_epochs e
         JOIN dvr_frames f ON f.h264_epoch_id = e.epoch_id AND f.payload_ready = 1
         WHERE e.source_id = ?1
         GROUP BY e.epoch_id",
    )
    .bind(source_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    let mut tracks = Vec::new();
    for row in rows {
        let epoch_id: i64 = row.try_get("epoch_id").map_err(db_error)?;
        let track_id = DvrTrackId::new(format!("h264-{epoch_id}"))?;
        let init: Vec<u8> = row.try_get("codec_init").map_err(db_error)?;
        let width: i64 = row.try_get("width").map_err(db_error)?;
        let height: i64 = row.try_get("height").map_err(db_error)?;
        let start =
            crate::time::from_unix_microseconds(row.try_get("min_start").map_err(db_error)?)?;
        let end = crate::time::from_unix_microseconds(row.try_get("max_end").map_err(db_error)?)?;
        let picture = DvrPictureProperties::new(
            u32::try_from(width.max(0)).unwrap_or(640),
            u32::try_from(height.max(0)).unwrap_or(480),
        )?;
        tracks.push(DvrMediaTrack::with_picture(
            track_id,
            VideoCodec::H264,
            init,
            TimeRange::new(start, end)?,
            picture,
        )?);
    }
    Ok(tracks)
}

pub(crate) async fn prune_empty_epochs(
    pool: &SqlitePool,
    source_id: &str,
) -> Result<(), A2aLabError> {
    sqlx::query(
        "DELETE FROM dvr_h264_epochs
         WHERE source_id = ?1
           AND epoch_id NOT IN (
             SELECT h264_epoch_id FROM dvr_frames
             WHERE source_id = ?1 AND h264_epoch_id IS NOT NULL AND payload_ready = 1
           )",
    )
    .bind(source_id)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

pub(crate) fn declared_picture_properties(
    codec_init: &[u8],
) -> Result<DvrPictureProperties, A2aLabError> {
    if let Some((width, height)) = coded_dimensions(codec_init) {
        let width = u32::from(width);
        let height = u32::from(height);
        if let Ok(picture) = DvrPictureProperties::new(width, height) {
            return Ok(picture);
        }
    }
    DvrPictureProperties::new(640, 480)
}

fn coded_dimensions(init: &[u8]) -> Option<(u16, u16)> {
    let nals = annex_b_nals(init).ok()?;
    let sps = nals
        .iter()
        .find(|nal| nal.first().is_some_and(|header| header & 0x1F == 7))?;
    sps_pixel_size(sps)
}

fn annex_b_nals(init: &[u8]) -> Result<Vec<Vec<u8>>, A2aLabError> {
    let mut nals = Vec::new();
    let mut index = 0_usize;
    while index < init.len() {
        let Some((start, advance)) = annex_b_start_code(&init[index..]) else {
            index += 1;
            continue;
        };
        index += advance;
        let payload_start = index + start;
        index = payload_start;
        while index < init.len() && annex_b_start_code(&init[index..]).is_none() {
            index += 1;
        }
        if payload_start < index {
            nals.push(init[payload_start..index].to_vec());
        }
    }
    if nals.is_empty() {
        return Err(A2aLabError::invalid(
            "codec_initialization",
            "must be Annex B H.264 initialization",
        ));
    }
    Ok(nals)
}

fn annex_b_start_code(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.len() >= 4 && bytes[..4] == [0, 0, 0, 1] {
        return Some((0, 4));
    }
    if bytes.len() >= 3 && bytes[..3] == [0, 0, 1] {
        return Some((0, 3));
    }
    None
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl BitReader<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.bit / 8)?;
        let shift = 7 - (self.bit % 8);
        self.bit += 1;
        Some(u32::from((byte >> shift) & 1))
    }

    fn bits(&mut self, count: usize) -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | self.bit()?;
        }
        Some(value)
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        if zeros == 0 {
            return Some(0);
        }
        let rest = self.bits(zeros)?;
        Some(((1_u32) << zeros) - 1 + rest)
    }
}

fn rbsp(nal_body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal_body.len());
    let mut zeros = 0_u8;
    for &byte in nal_body {
        if zeros >= 2 && byte == 0x03 {
            zeros = 0;
            continue;
        }
        out.push(byte);
        zeros = if byte == 0 {
            zeros.saturating_add(1)
        } else {
            0
        };
    }
    out
}

fn sps_pixel_size(nal: &[u8]) -> Option<(u16, u16)> {
    if nal.len() < 4 {
        return None;
    }
    let body = rbsp(&nal[1..]);
    let mut bits = BitReader {
        data: &body,
        bit: 0,
    };
    let profile = bits.bits(8)?;
    let _constraints = bits.bits(8)?;
    let _level = bits.bits(8)?;
    let _sps_id = bits.ue()?;
    skip_high_profile(&mut bits, profile)?;
    skip_order_count(&mut bits)?;
    let _refs = bits.ue()?;
    let _gaps = bits.bit()?;
    let width_mbs = bits.ue()?;
    let height_units = bits.ue()?;
    let frame_mbs_only = bits.bit()?;
    if frame_mbs_only == 0 {
        let _adaptive = bits.bit()?;
    }
    let _direct = bits.bit()?;
    let (crop_left, crop_right, crop_top, crop_bottom) = frame_crop(&mut bits)?;
    let width = (width_mbs + 1).saturating_mul(16) - (crop_left + crop_right).saturating_mul(2);
    let rows = 2 - frame_mbs_only;
    let height = rows.saturating_mul(height_units + 1).saturating_mul(16)
        - (crop_top + crop_bottom)
            .saturating_mul(2)
            .saturating_mul(rows);
    Some((u16::try_from(width).ok()?, u16::try_from(height).ok()?))
}

fn skip_high_profile(bits: &mut BitReader<'_>, profile: u32) -> Option<()> {
    if !matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        return Some(());
    }
    let chroma = bits.ue()?;
    if chroma == 3 {
        let _separate = bits.bit()?;
    }
    let _depth_luma = bits.ue()?;
    let _depth_chroma = bits.ue()?;
    let _qpprime = bits.bit()?;
    if bits.bit()? == 1 {
        return None;
    }
    Some(())
}

fn skip_order_count(bits: &mut BitReader<'_>) -> Option<()> {
    let _frame_num = bits.ue()?;
    let poc = bits.ue()?;
    if poc == 0 {
        let _lsb = bits.ue()?;
    } else if poc == 1 {
        let _delta = bits.bit()?;
        let _offset_a = bits.ue()?;
        let _offset_b = bits.ue()?;
        let cycles = bits.ue()?;
        for _ in 0..cycles {
            let _offset = bits.ue()?;
        }
    }
    Some(())
}

fn frame_crop(bits: &mut BitReader<'_>) -> Option<(u32, u32, u32, u32)> {
    if bits.bit()? == 0 {
        return Some((0, 0, 0, 0));
    }
    Some((bits.ue()?, bits.ue()?, bits.ue()?, bits.ue()?))
}
