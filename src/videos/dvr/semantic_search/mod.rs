//! DVR-backed semantic retrieval and indexing coverage.

mod embedder;
mod retrieval;

pub use embedder::DvrSearchEmbedder;

use a2a_lab_dev_kit::{
    A2aLabError, FrameVectorIndexAttachment, GetSemanticIndexingCoverageRequest,
    ModelSpaceIdentity, SearchSemanticFramesRequest, SemanticFrameSearchResult,
    SemanticIndexingCoverage, SemanticIndexingFreshness, SemanticIndexingStateCounts,
    SemanticProvenanceSelector, SemanticVideoSearchProvider, TimeRange,
};
use async_trait::async_trait;
use sqlx::Row;

use self::retrieval::{rank_candidates, resolve_query_vector, search_hit_page};
use super::frame_manifest::ManifestSearchCandidateFilter;
use super::indexing_scope::IndexedSourceFilter;
use super::store::{IndexedVideoDvr, db_error};
use crate::lab_config::ResolvedSemanticIndexing;

pub(crate) const RETAINED_LOWER_BOUND_US: i64 = 0;
pub(crate) const RETAINED_UPPER_BOUND_US: i64 = i64::MAX;

fn candidate_micros_bounds(range: Option<TimeRange>) -> (i64, i64) {
    match range {
        Some(range) => (range.start().as_micros(), range.end().as_micros()),
        None => (RETAINED_LOWER_BOUND_US, RETAINED_UPPER_BOUND_US),
    }
}

/// Read-only semantic APIs scoped to configured sources and SQLite manifest state.
#[derive(Clone)]
pub struct DvrSemanticSearchProvider {
    dvr: IndexedVideoDvr,
    model_space: ModelSpaceIdentity,
    source_filter: IndexedSourceFilter,
    embedder: DvrSearchEmbedder,
}

impl DvrSemanticSearchProvider {
    /// Builds a provider from resolved serve settings and a query embedder.
    pub fn new(
        dvr: IndexedVideoDvr,
        semantic: &ResolvedSemanticIndexing,
        embedder: DvrSearchEmbedder,
    ) -> Result<Self, A2aLabError> {
        let embeddings = semantic
            .config
            .embeddings
            .as_ref()
            .ok_or_else(|| A2aLabError::unavailable("semantic vector search is disabled"))?;
        Ok(Self {
            dvr,
            model_space: embeddings.model_space_identity()?,
            source_filter: IndexedSourceFilter::allow_only(semantic.indexed_source_ids.clone()),
            embedder,
        })
    }
}

#[async_trait]
impl SemanticVideoSearchProvider for DvrSemanticSearchProvider {
    async fn search_semantic_frames(
        &self,
        request: SearchSemanticFramesRequest,
    ) -> Result<SemanticFrameSearchResult, A2aLabError> {
        request.check()?;
        self.ensure_configured_scope(request.scope().source_id().as_str())?;
        self.ensure_active_model_space(request.provenance())?;
        let incomplete_indexing = self
            .incomplete_indexing(
                request.scope().source_id().as_str(),
                request.scope().range(),
            )
            .await?;
        let query = resolve_query_vector(&self.dvr, &self.embedder, &request).await?;
        let filter = ManifestSearchCandidateFilter {
            description_generation: request.provenance().description_generation().get(),
            model_space: request.provenance().model_space().clone(),
        };
        let (range_start_us, range_end_us) = candidate_micros_bounds(request.scope().range());
        let scan = self
            .dvr
            .scan_semantic_search_candidates(
                request.scope().source_id().as_str(),
                range_start_us,
                range_end_us,
                &filter,
                request.limits().candidate_limit(),
            )
            .await?;
        let ranked = rank_candidates(&request, &query, scan.records, |record| {
            attachment_for_candidate(record, request.provenance())
        })?;
        let candidate_budget_exhausted = scan.truncated;
        let ranked = ranked
            .into_iter()
            .take(usize::try_from(request.limits().top_k()).unwrap_or(usize::MAX))
            .collect::<Vec<_>>();
        let hits = search_hit_page(&self.dvr, &request, ranked).await?;
        Ok(SemanticFrameSearchResult::new(
            hits,
            incomplete_indexing,
            candidate_budget_exhausted,
        ))
    }

    async fn get_semantic_indexing_coverage(
        &self,
        request: GetSemanticIndexingCoverageRequest,
    ) -> Result<SemanticIndexingCoverage, A2aLabError> {
        self.ensure_configured_scope(request.scope().source_id().as_str())?;
        self.ensure_active_model_space(request.provenance())?;
        let (counts, freshness, scope_complete) = self
            .coverage_snapshot(
                request.scope().source_id().as_str(),
                request.scope().range(),
            )
            .await?;
        Ok(SemanticIndexingCoverage::new(
            request.scope().clone(),
            request.provenance().model_space().clone(),
            counts,
            freshness,
            scope_complete,
        ))
    }
}

impl DvrSemanticSearchProvider {
    fn ensure_configured_scope(&self, source_id: &str) -> Result<(), A2aLabError> {
        if !self.source_filter.permits(source_id) {
            return Err(A2aLabError::invalid(
                "scope.source_id",
                "source is outside configured semantic indexing scope",
            ));
        }
        Ok(())
    }

    fn ensure_active_model_space(
        &self,
        provenance: &SemanticProvenanceSelector,
    ) -> Result<(), A2aLabError> {
        if provenance.model_space() != &self.model_space {
            return Err(A2aLabError::invalid(
                "provenance.model_space",
                "does not match the configured semantic embedding model space",
            ));
        }
        if self.embedder.config().model_space() != provenance.model_space() {
            return Err(A2aLabError::invalid(
                "provenance.model_space",
                "does not match the active query embedding provider model space",
            ));
        }
        Ok(())
    }

    async fn incomplete_indexing(
        &self,
        source_id: &str,
        range: Option<TimeRange>,
    ) -> Result<bool, A2aLabError> {
        let (counts, scope_complete) = self.coverage_counts(source_id, range).await?;
        Ok(!scope_complete || counts.pending() > 0 || counts.failed() > 0)
    }

    async fn coverage_snapshot(
        &self,
        source_id: &str,
        range: Option<TimeRange>,
    ) -> Result<(SemanticIndexingStateCounts, SemanticIndexingFreshness, bool), A2aLabError> {
        let (counts, freshness, scope_complete) = self.coverage_query(source_id, range).await?;
        Ok((counts, freshness, scope_complete))
    }

    async fn coverage_counts(
        &self,
        source_id: &str,
        range: Option<TimeRange>,
    ) -> Result<(SemanticIndexingStateCounts, bool), A2aLabError> {
        let (counts, _, scope_complete) = self.coverage_query(source_id, range).await?;
        Ok((counts, scope_complete))
    }

    async fn coverage_query(
        &self,
        source_id: &str,
        range: Option<TimeRange>,
    ) -> Result<(SemanticIndexingStateCounts, SemanticIndexingFreshness, bool), A2aLabError> {
        let (start, end) = candidate_micros_bounds(range);
        let row = sqlx::query(
            "SELECT
                SUM(CASE WHEN COALESCE(rt.job_state, 'pending') IN ('pending','leased','described','vector_pending','retryable_failed') THEN 1 ELSE 0 END) AS pending,
                SUM(CASE WHEN COALESCE(rt.job_state, 'ready') = 'ready'
                          AND rev.embedding_checkpoint = 'ready'
                          AND EXISTS (
                            SELECT 1 FROM semantic_frame_embeddings emb
                            WHERE emb.frame_id = rev.frame_id
                              AND emb.model_space_generation = COALESCE(
                                (SELECT model_space_generation FROM semantic_embedding_config_identity WHERE singleton_id = 1),
                                0
                              )
                              AND emb.embedding_vector IS NOT NULL
                              AND length(emb.embedding_vector) > 0
                          ) THEN 1 ELSE 0 END) AS ready,
                SUM(CASE WHEN COALESCE(rt.job_state, 'terminal_failed') = 'terminal_failed' THEN 1 ELSE 0 END) AS failed,
                SUM(CASE WHEN rev.retained_state != 'retained' OR rev.search_current = 0 THEN 1 ELSE 0 END) AS expired,
                MIN(CASE WHEN COALESCE(rt.job_state, 'pending') != 'ready' THEN rev.start_us END) AS oldest_pending,
                MAX(CASE WHEN COALESCE(rt.job_state, 'ready') = 'ready' AND rev.embedding_checkpoint = 'ready' THEN rev.start_us END) AS newest_ready,
                MAX(CASE WHEN COALESCE(rt.job_state, 'terminal_failed') = 'terminal_failed' THEN rev.start_us END) AS newest_failed
             FROM semantic_frame_revisions rev
             LEFT JOIN semantic_indexing_jobs j ON j.frame_id = rev.frame_id
             LEFT JOIN semantic_indexing_job_runtime rt ON rt.job_id = j.job_id
             WHERE rev.source_id = ?1
               AND rev.start_us >= ?2
               AND rev.start_us < ?3
               AND rev.search_current = 1",
        )
        .bind(source_id)
        .bind(start)
        .bind(end)
        .fetch_one(self.dvr.sqlite_pool())
        .await
        .map_err(db_error)?;

        let pending: i64 = row.try_get("pending").unwrap_or(0);
        let ready: i64 = row.try_get("ready").unwrap_or(0);
        let failed: i64 = row.try_get("failed").unwrap_or(0);
        let expired: i64 = row.try_get("expired").unwrap_or(0);
        let counts = SemanticIndexingStateCounts::new(
            u64::try_from(pending.max(0)).unwrap_or(0),
            u64::try_from(ready.max(0)).unwrap_or(0),
            u64::try_from(failed.max(0)).unwrap_or(0),
            u64::try_from(expired.max(0)).unwrap_or(0),
        );
        let freshness = SemanticIndexingFreshness::new(
            row.try_get::<Option<i64>, _>("oldest_pending")
                .ok()
                .flatten()
                .and_then(|micros| crate::time::from_unix_microseconds(micros).ok()),
            row.try_get::<Option<i64>, _>("newest_ready")
                .ok()
                .flatten()
                .and_then(|micros| crate::time::from_unix_microseconds(micros).ok()),
            row.try_get::<Option<i64>, _>("newest_failed")
                .ok()
                .flatten()
                .and_then(|micros| crate::time::from_unix_microseconds(micros).ok()),
        );
        let scope_complete = pending == 0 && failed == 0;
        Ok((counts, freshness, scope_complete))
    }
}

pub(crate) fn attachment_for_candidate(
    record: &super::frame_manifest::SemanticFrameManifestRecord,
    selector: &SemanticProvenanceSelector,
) -> Result<Option<FrameVectorIndexAttachment>, A2aLabError> {
    let (Some(description), Some(embedding)) = (record.description(), record.embedding()) else {
        return Ok(None);
    };
    let Some(vector) = embedding.vector() else {
        return Ok(None);
    };
    let attachment = FrameVectorIndexAttachment::new(
        selector.model_space().clone(),
        description.provenance().clone(),
        embedding.provenance().clone(),
        vector.clone(),
    )?;
    if selector.accepts_attachment(&attachment) {
        Ok(Some(attachment))
    } else {
        Ok(None)
    }
}
