//! Read-only discovery of retained DVR sources into manifest records.

use a2a_lab_dev_kit::{A2aLabError, VideoSourceId};
use sha2::{Digest, Sha256};

use super::manifest::{EvidenceAvailability, RetainedSourceRecord};
use crate::videos::dvr::IndexedVideoDvr;
use crate::videos::dvr::inventory::DvrTrackInventory;

fn codec_label(inventory: &DvrTrackInventory) -> &'static str {
    use a2a_lab_dev_kit::VideoCodec;
    match inventory.codec {
        VideoCodec::Mjpeg => "mjpeg",
        VideoCodec::H264 => "h264",
        VideoCodec::H265 => "h265",
    }
}

/// Builds retained-source manifest rows from the DVR index without mutating capture state.
pub async fn discover_retained_sources(
    store: &IndexedVideoDvr,
) -> Result<Vec<RetainedSourceRecord>, A2aLabError> {
    let mut records = Vec::new();
    for source_id in store.list_indexed_source_ids().await? {
        let tracks = store
            .track_inventories(&source_id)
            .await
            .unwrap_or_default();
        for track in tracks {
            if track.frame_count == 0 {
                continue;
            }
            records.push(track_inventory_record(&source_id, &track));
        }
    }
    records.sort_by(|left, right| {
        (&left.source_id, &left.track_id).cmp(&(&right.source_id, &right.track_id))
    });
    Ok(records)
}

fn track_inventory_record(
    source_id: &VideoSourceId,
    track: &DvrTrackInventory,
) -> RetainedSourceRecord {
    let _ = source_id;
    RetainedSourceRecord {
        source_id: track.source_id.as_str().to_owned(),
        track_id: track.track_id.as_str().to_owned(),
        codec: codec_label(track).to_owned(),
        capture_start_utc: track.capture_start.to_string(),
        capture_end_utc: track.capture_end.to_string(),
        frame_count: track.frame_count,
        evidence_availability: EvidenceAvailability::Available,
        aggregate_payload_sha256: None,
    }
}

/// Merges DVR discovery into an existing manifest, marking missing tracks unavailable.
pub async fn refresh_retained_sources(
    store: &IndexedVideoDvr,
    existing: &[RetainedSourceRecord],
) -> Result<Vec<RetainedSourceRecord>, A2aLabError> {
    let discovered = discover_retained_sources(store).await?;
    let mut merged = discovered;
    for record in existing {
        if merged
            .iter()
            .any(|row| row.source_id == record.source_id && row.track_id == record.track_id)
        {
            continue;
        }
        merged.push(RetainedSourceRecord {
            evidence_availability: EvidenceAvailability::Unavailable,
            ..record.clone()
        });
    }
    merged.sort_by(|left, right| {
        (&left.source_id, &left.track_id).cmp(&(&right.source_id, &right.track_id))
    });
    Ok(merged)
}

/// Deterministic aggregate digest for a bounded list of per-frame SHA-256 hex strings.
#[must_use]
pub fn aggregate_frame_digests(digests: &[String]) -> String {
    let mut hasher = Sha256::new();
    for digest in digests {
        hasher.update(digest.as_bytes());
    }
    hex_digest(&hasher.finalize())
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
