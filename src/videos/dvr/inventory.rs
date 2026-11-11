//! Read-only retained-source inventory for semantic evaluation manifests.

use a2a_lab_dev_kit::{A2aLabError, DvrTrackId, UtcTimestamp, VideoCodec, VideoSourceId};
use sha2::{Digest, Sha256};
use sqlx::Row;

use super::store::IndexedVideoDvr;

/// Per-track retained frame statistics from the DVR index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DvrTrackInventory {
    pub source_id: VideoSourceId,
    pub track_id: DvrTrackId,
    pub codec: VideoCodec,
    pub frame_count: u64,
    pub capture_start: UtcTimestamp,
    pub capture_end: UtcTimestamp,
}

/// Payload integrity digests for one saved frame revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePayloadEvidence {
    pub payload_length: u64,
    pub payload_crc32: u32,
    pub payload_sha256: String,
}

impl IndexedVideoDvr {
    /// Lists every `source_id` present in the DVR index (including sources with no ready payloads).
    pub async fn list_indexed_source_ids(&self) -> Result<Vec<VideoSourceId>, A2aLabError> {
        let labels: Vec<String> =
            sqlx::query_scalar("SELECT source_id FROM dvr_tracks ORDER BY source_id")
                .fetch_all(self.sqlite_pool())
                .await
                .map_err(super::store::db_error)?;
        labels.into_iter().map(VideoSourceId::new).collect()
    }

    /// Inventory statistics for each track on `source_id` with at least one ready payload row.
    pub async fn track_inventories(
        &self,
        source_id: &VideoSourceId,
    ) -> Result<Vec<DvrTrackInventory>, A2aLabError> {
        let rows = sqlx::query(
            "SELECT f.track_id, t.codec,
                    COUNT(*) AS frame_count,
                    MIN(f.start_us) AS min_start,
                    MAX(f.end_us) AS max_end
             FROM dvr_frames f
             JOIN dvr_tracks t USING (source_id, track_id)
             WHERE f.source_id = ?1 AND f.payload_ready = 1
             GROUP BY f.track_id, t.codec
             UNION ALL
             SELECT s.track_id, t.codec,
                    COUNT(*) AS frame_count,
                    MIN(s.start_us) AS min_start,
                    MAX(s.end_us) AS max_end
             FROM dvr_segments s
             JOIN dvr_tracks t USING (source_id, track_id)
             WHERE s.source_id = ?1 AND s.payload_ready = 1
             GROUP BY s.track_id, t.codec",
        )
        .bind(source_id.as_str())
        .fetch_all(self.sqlite_pool())
        .await
        .map_err(super::store::db_error)?;
        let mut out = Vec::new();
        for row in rows {
            let track_id = DvrTrackId::new(
                row.try_get::<String, _>("track_id")
                    .map_err(super::store::db_error)?,
            )?;
            let codec = super::store::parse_codec(
                &row.try_get::<String, _>("codec")
                    .map_err(super::store::db_error)?,
            )?;
            let frame_count: i64 = row.try_get("frame_count").map_err(super::store::db_error)?;
            let min_start: i64 = row.try_get("min_start").map_err(super::store::db_error)?;
            let max_end: i64 = row.try_get("max_end").map_err(super::store::db_error)?;
            out.push(DvrTrackInventory {
                source_id: source_id.clone(),
                track_id,
                codec,
                frame_count: u64::try_from(frame_count.max(0)).unwrap_or(0),
                capture_start: crate::time::from_unix_microseconds(min_start)?,
                capture_end: crate::time::from_unix_microseconds(max_end)?,
            });
        }
        out.sort_by(|left, right| left.track_id.as_str().cmp(right.track_id.as_str()));
        Ok(out)
    }

    /// Loads one ready frame payload and returns length, CRC32, and SHA-256 hex digest.
    pub async fn frame_payload_evidence(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        start_us: i64,
    ) -> Result<FramePayloadEvidence, A2aLabError> {
        let at = crate::time::from_unix_microseconds(start_us)?;
        let (bytes, crc32, _) = self
            .load_retained_payload_at_key(source_id, track_id, at)
            .await?;
        let digest = Sha256::digest(&bytes);
        Ok(FramePayloadEvidence {
            payload_length: u64::try_from(bytes.len()).unwrap_or(0),
            payload_crc32: crc32,
            payload_sha256: hex_digest(&digest),
        })
    }
}

fn hex_digest(digest: &[u8]) -> String {
    digest
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write;
            let _ = write!(out, "{byte:02x}");
            out
        })
}
