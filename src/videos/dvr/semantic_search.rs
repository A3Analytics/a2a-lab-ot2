//! DVR-backed semantic coverage and deferred vector retrieval.

use a2a_lab_dev_kit::{
    A2aLabError, GetSemanticIndexingCoverageRequest, ModelSpaceIdentity, Page,
    SearchSemanticFramesRequest, SemanticFrameSearchResult, SemanticIndexingCoverage,
    SemanticIndexingFreshness, SemanticIndexingStateCounts, SemanticVideoSearchProvider,
};
use async_trait::async_trait;
use sqlx::Row;

use super::indexing_scope::IndexedSourceFilter;
use super::store::{IndexedVideoDvr, db_error};
use crate::lab_config::ResolvedSemanticIndexing;

/// Read-only semantic APIs scoped to configured sources and SQLite manifest state.
#[derive(Clone)]
pub struct DvrSemanticSearchProvider {
    dvr: IndexedVideoDvr,
    model_space: ModelSpaceIdentity,
    source_filter: IndexedSourceFilter,
}

impl DvrSemanticSearchProvider {
    /// Builds a provider from resolved serve settings.
    pub fn new(
        dvr: IndexedVideoDvr,
        semantic: &ResolvedSemanticIndexing,
    ) -> Result<Self, A2aLabError> {
        let embeddings = semantic.config.embeddings.as_ref().ok_or_else(|| {
            A2aLabError::unavailable("semantic vector search is disabled")
        })?;
        Ok(Self {
            dvr,
            model_space: embeddings.model_space_identity()?,
            source_filter: IndexedSourceFilter::allow_only(semantic.indexed_source_ids.clone()),
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
        if !self
            .source_filter
            .permits(request.scope().source_id().as_str())
        {
            return Err(A2aLabError::invalid(
                "scope.source_id",
                "source is outside configured semantic indexing scope",
            ));
        }
        Ok(SemanticFrameSearchResult::new(
            Page::new(vec![], None),
            true,
            false,
        ))
    }

    async fn get_semantic_indexing_coverage(
        &self,
        request: GetSemanticIndexingCoverageRequest,
    ) -> Result<SemanticIndexingCoverage, A2aLabError> {
        if !self
            .source_filter
            .permits(request.scope().source_id().as_str())
        {
            return Err(A2aLabError::invalid(
                "scope.source_id",
                "source is outside configured semantic indexing scope",
            ));
        }
        let (counts, freshness, scope_complete) = self
            .coverage_snapshot(
                request.scope().source_id().as_str(),
                request.scope().range(),
            )
            .await?;
        Ok(SemanticIndexingCoverage::new(
            request.scope().clone(),
            self.model_space.clone(),
            counts,
            freshness,
            scope_complete,
        ))
    }
}

impl DvrSemanticSearchProvider {
    async fn coverage_snapshot(
        &self,
        source_id: &str,
        range: a2a_lab_dev_kit::TimeRange,
    ) -> Result<(SemanticIndexingStateCounts, SemanticIndexingFreshness, bool), A2aLabError> {
        let start = range.start().as_micros();
        let end = range.end().as_micros();
        let row = sqlx::query(
            "SELECT
                SUM(CASE WHEN COALESCE(rt.job_state, 'pending') IN ('pending','leased','described','vector_pending','retryable_failed') THEN 1 ELSE 0 END) AS pending,
                SUM(CASE WHEN COALESCE(rt.job_state, 'ready') = 'ready' AND rev.embedding_checkpoint = 'ready' THEN 1 ELSE 0 END) AS ready,
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
