//! Ranking, paging, and hit construction for DVR semantic search.

use sqlx::Row;

use a2a_lab_dev_kit::{
    A2aLabError, EmbeddingMetric, FrameEmbeddingVector, FrameVectorIndexAttachment, Page,
    PageRequest, SavedFrameRef, SearchSemanticFramesRequest, SemanticFrameHit,
    SemanticFrameIndexState, SemanticFrameMediaAvailability, SemanticFramePlaybackNeighbors,
    SemanticRetrievalScore, SemanticSearchQuery, TimeRange,
};

use super::embedder::DvrSearchEmbedder;
use crate::videos::dvr::{
    IndexedVideoDvr, IndexingCheckpoint, ManifestRetainedState, MediaPayloadStatus,
    SemanticFrameManifestRecord,
};

/// Ranked manifest revision before paging.
pub(crate) struct RankedCandidate {
    pub(crate) record: SemanticFrameManifestRecord,
    pub(crate) attachment: FrameVectorIndexAttachment,
    pub(crate) score: SemanticRetrievalScore,
}

pub(crate) async fn resolve_query_vector(
    dvr: &IndexedVideoDvr,
    embedder: &DvrSearchEmbedder,
    request: &SearchSemanticFramesRequest,
) -> Result<FrameEmbeddingVector, A2aLabError> {
    match request.query() {
        SemanticSearchQuery::Text { text } => {
            let (_space, vector) = embedder.embed_query_text(text).await?;
            Ok(vector)
        }
        SemanticSearchQuery::ReferenceFrame { frame } => {
            if frame.source_id() != request.scope().source_id() {
                return Err(A2aLabError::invalid(
                    "query.frame.source_id",
                    "reference frame must belong to the search scope source",
                ));
            }
            if !request.scope().contains_capture(frame.captured_at()) {
                return Err(A2aLabError::invalid(
                    "query.frame.captured_at",
                    "reference frame must fall inside the search scope range",
                ));
            }
            let record = dvr.get_manifest_frame(frame.frame_id().as_str()).await?;
            let attachment = attachment_for_record(&record, request)?.ok_or_else(|| {
                A2aLabError::invalid(
                    "query.frame",
                    "reference frame is not indexed in the requested provenance space",
                )
            })?;
            Ok(attachment.vector().clone())
        }
    }
}

pub(crate) fn rank_candidates(
    request: &SearchSemanticFramesRequest,
    query: &FrameEmbeddingVector,
    records: Vec<SemanticFrameManifestRecord>,
    mut attachment_for: impl FnMut(
        &SemanticFrameManifestRecord,
    ) -> Result<Option<FrameVectorIndexAttachment>, A2aLabError>,
) -> Result<Vec<RankedCandidate>, A2aLabError> {
    let metric = request.provenance().model_space().metric();
    let query_values = query.values();
    let mut ranked = Vec::new();
    for record in records {
        let Some(attachment) = attachment_for(&record)? else {
            continue;
        };
        let doc = attachment.vector().values();
        let raw = score_vectors(metric, query_values, doc)?;
        let score = SemanticRetrievalScore::new(raw, metric)?;
        ranked.push(RankedCandidate {
            record,
            attachment,
            score,
        });
    }
    ranked.sort_by(|left, right| {
        right
            .score
            .value()
            .total_cmp(&left.score.value())
            .then_with(|| {
                left.record
                    .frame()
                    .frame_id()
                    .as_str()
                    .cmp(right.record.frame().frame_id().as_str())
            })
    });
    Ok(ranked)
}

pub(crate) async fn search_hit_page(
    dvr: &IndexedVideoDvr,
    request: &SearchSemanticFramesRequest,
    ranked: Vec<RankedCandidate>,
) -> Result<Page<SemanticFrameHit>, A2aLabError> {
    let mut hits = Vec::new();
    for candidate in &ranked {
        let hit = build_hit(dvr, request, candidate).await?;
        hits.push(hit);
    }
    slice_hits(&hits, request.page())
}

fn slice_hits(
    hits: &[SemanticFrameHit],
    page: &PageRequest,
) -> Result<Page<SemanticFrameHit>, A2aLabError> {
    page.check()?;
    let start = match page.cursor() {
        None => 0,
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| A2aLabError::invalid("cursor", "is not a valid page cursor"))?,
    };
    if start > hits.len() {
        return Err(A2aLabError::invalid("cursor", "is past the end"));
    }
    let end = start.saturating_add(usize::try_from(page.limit()).unwrap_or(usize::MAX));
    let end = end.min(hits.len());
    let next_cursor = (end < hits.len()).then(|| end.to_string());
    Ok(Page::new(hits[start..end].to_vec(), next_cursor))
}

async fn build_hit(
    dvr: &IndexedVideoDvr,
    request: &SearchSemanticFramesRequest,
    candidate: &RankedCandidate,
) -> Result<SemanticFrameHit, A2aLabError> {
    let record = &candidate.record;
    let description = record
        .description()
        .ok_or_else(|| A2aLabError::invalid("description", "ready hit missing description"))?;
    let index_state = semantic_index_state(record);
    let media = media_availability(record);
    let neighbors = playback_neighbors(dvr, record.frame(), request, record.end_us()).await?;
    SemanticFrameHit::new(
        record.frame().clone(),
        description.text(),
        &candidate.attachment,
        candidate.score,
        index_state,
        media,
        neighbors,
    )
}

fn semantic_index_state(record: &SemanticFrameManifestRecord) -> SemanticFrameIndexState {
    if record.retained_state() != ManifestRetainedState::Retained
        || !record.search_current()
        || record.description_checkpoint() != IndexingCheckpoint::Ready
        || record.embedding_checkpoint() != IndexingCheckpoint::Ready
    {
        return SemanticFrameIndexState::Expired;
    }
    SemanticFrameIndexState::Ready
}

fn media_availability(record: &SemanticFrameManifestRecord) -> SemanticFrameMediaAvailability {
    match record.payload_status() {
        MediaPayloadStatus::Available => SemanticFrameMediaAvailability::Ready,
        MediaPayloadStatus::Missing => SemanticFrameMediaAvailability::Unavailable {
            reason: "payload_missing".to_owned(),
        },
        MediaPayloadStatus::Corrupt => SemanticFrameMediaAvailability::Unavailable {
            reason: "payload_corrupt".to_owned(),
        },
    }
}

async fn playback_neighbors(
    dvr: &IndexedVideoDvr,
    frame: &SavedFrameRef,
    request: &SearchSemanticFramesRequest,
    hit_end: a2a_lab_dev_kit::UtcTimestamp,
) -> Result<SemanticFramePlaybackNeighbors, A2aLabError> {
    let max_neighbors = request.limits().max_neighbor_frames();
    let scope_start = super::RETAINED_LOWER_BOUND_US;
    let scope_end = super::RETAINED_UPPER_BOUND_US;
    let hit_us = frame.captured_at().as_micros();
    let before = sqlx::query(
        "SELECT frame_id FROM semantic_frame_revisions
         WHERE source_id = ?1 AND track_id = ?2
           AND search_current = 1 AND retained_state = 'retained'
           AND start_us >= ?3 AND start_us < ?4
           AND start_us < ?5
         ORDER BY start_us DESC, frame_id DESC
         LIMIT ?6",
    )
    .bind(frame.source_id().as_str())
    .bind(frame.track_id().as_str())
    .bind(scope_start)
    .bind(scope_end)
    .bind(hit_us)
    .bind(i64::from(max_neighbors))
    .fetch_all(dvr.sqlite_pool())
    .await
    .map_err(crate::videos::dvr::store::db_error)?;
    let after = sqlx::query(
        "SELECT frame_id FROM semantic_frame_revisions
         WHERE source_id = ?1 AND track_id = ?2
           AND search_current = 1 AND retained_state = 'retained'
           AND start_us >= ?3 AND start_us < ?4
           AND start_us > ?5
         ORDER BY start_us ASC, frame_id ASC
         LIMIT ?6",
    )
    .bind(frame.source_id().as_str())
    .bind(frame.track_id().as_str())
    .bind(scope_start)
    .bind(scope_end)
    .bind(hit_us)
    .bind(i64::from(max_neighbors))
    .fetch_all(dvr.sqlite_pool())
    .await
    .map_err(crate::videos::dvr::store::db_error)?;

    let mut refs = Vec::new();
    for row in before.iter().rev() {
        let frame_id: String = row
            .try_get("frame_id")
            .map_err(crate::videos::dvr::store::db_error)?;
        let record = dvr.get_manifest_frame(&frame_id).await?;
        refs.push(record.frame().clone());
    }
    refs.push(frame.clone());
    for row in &after {
        let frame_id: String = row
            .try_get("frame_id")
            .map_err(crate::videos::dvr::store::db_error)?;
        let record = dvr.get_manifest_frame(&frame_id).await?;
        refs.push(record.frame().clone());
    }
    if refs.len() > usize::try_from(max_neighbors).unwrap_or(usize::MAX) {
        let keep = usize::try_from(max_neighbors).unwrap_or(refs.len());
        let start = refs.len().saturating_sub(keep);
        refs = refs[start..].to_vec();
    }
    let window_start = refs
        .first()
        .map_or(frame.captured_at(), SavedFrameRef::captured_at);
    let window_end = refs
        .last()
        .map_or(hit_end, |neighbor| hit_end.max(neighbor.captured_at()));
    let window = TimeRange::new(window_start, window_end)?;
    SemanticFramePlaybackNeighbors::new(window, refs, max_neighbors)
}

fn score_vectors(metric: EmbeddingMetric, query: &[f32], doc: &[f32]) -> Result<f32, A2aLabError> {
    if query.len() != doc.len() {
        return Err(A2aLabError::invalid(
            "vector",
            "query and document vectors must share dimensions",
        ));
    }
    match metric {
        EmbeddingMetric::Cosine | EmbeddingMetric::Dot => {
            let mut sum = 0.0_f32;
            for (left, right) in query.iter().zip(doc.iter()) {
                sum += left * right;
            }
            Ok(sum)
        }
        EmbeddingMetric::Euclidean => {
            let mut sum_sq = 0.0_f32;
            for (left, right) in query.iter().zip(doc.iter()) {
                let delta = left - right;
                sum_sq += delta * delta;
            }
            Ok(-sum_sq.sqrt())
        }
    }
}

fn attachment_for_record(
    record: &SemanticFrameManifestRecord,
    request: &SearchSemanticFramesRequest,
) -> Result<Option<FrameVectorIndexAttachment>, A2aLabError> {
    super::attachment_for_candidate(record, request.provenance())
}
