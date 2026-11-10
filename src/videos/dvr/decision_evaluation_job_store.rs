//! Durable Jev evaluation job leases, retries, and terminal coverage.

use a2a_lab_dev_kit::{
    A2aLabError, DecisionEvaluationCoverage, LabPredicate, SemanticDecisionLimits,
};
use sqlx::Row;

use super::store::{IndexedVideoDvr, db_error};

/// Observable decision-evaluation job lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionEvaluationJobState {
    Pending,
    Leased,
    Committed,
    RetryableFailed,
    TerminalFailed,
    Cancelled,
}

impl DecisionEvaluationJobState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Leased => "leased",
            Self::Committed => "committed",
            Self::RetryableFailed => "retryable_failed",
            Self::TerminalFailed => "terminal_failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Result<Self, A2aLabError> {
        match value {
            "pending" => Ok(Self::Pending),
            "leased" => Ok(Self::Leased),
            "committed" => Ok(Self::Committed),
            "retryable_failed" => Ok(Self::RetryableFailed),
            "terminal_failed" => Ok(Self::TerminalFailed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(A2aLabError::invalid(
                "decision_job_state",
                format!("unknown {value}"),
            )),
        }
    }
}

/// One durable job row with evaluator/predicate/config identity and coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionEvaluationJobRecord {
    job_id: String,
    frame_id: String,
    evaluation_id: String,
    predicate_id: String,
    predicate_version: u32,
    subject_digest: String,
    config_digest: String,
    producer_revision: String,
    coverage_policy: String,
    job_state: DecisionEvaluationJobState,
    coverage: Option<DecisionEvaluationCoverage>,
    gap_reason: Option<String>,
    revision_generation: u32,
    lease_owner: Option<String>,
    lease_expires_at_us: Option<i64>,
    evaluation_attempts: u32,
    last_retry_reason: Option<String>,
    next_retry_at_us: i64,
    last_error_at_us: Option<i64>,
    completed_at_us: Option<i64>,
    cancelled_at_us: Option<i64>,
    idempotency_key: Option<String>,
    decision_id: Option<String>,
}

impl DecisionEvaluationJobRecord {
    #[must_use]
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    #[must_use]
    pub fn frame_id(&self) -> &str {
        &self.frame_id
    }

    #[must_use]
    pub fn evaluation_id(&self) -> &str {
        &self.evaluation_id
    }

    #[must_use]
    pub const fn job_state(&self) -> DecisionEvaluationJobState {
        self.job_state
    }

    #[must_use]
    pub const fn coverage(&self) -> Option<DecisionEvaluationCoverage> {
        self.coverage
    }

    #[must_use]
    pub fn gap_reason(&self) -> Option<&str> {
        self.gap_reason.as_deref()
    }

    #[must_use]
    pub const fn revision_generation(&self) -> u32 {
        self.revision_generation
    }

    #[must_use]
    pub fn producer_revision(&self) -> &str {
        &self.producer_revision
    }

    #[must_use]
    pub fn config_digest(&self) -> &str {
        &self.config_digest
    }
}

fn coverage_label(coverage: DecisionEvaluationCoverage) -> &'static str {
    match coverage {
        DecisionEvaluationCoverage::Evaluated => "evaluated",
        DecisionEvaluationCoverage::NotEvaluated => "not_evaluated",
        DecisionEvaluationCoverage::Failed => "failed",
        DecisionEvaluationCoverage::TimedOut => "timed_out",
        DecisionEvaluationCoverage::Cancelled => "cancelled",
        DecisionEvaluationCoverage::Unsupported => "unsupported",
    }
}

fn parse_coverage(value: Option<&str>) -> Result<Option<DecisionEvaluationCoverage>, A2aLabError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed = match value {
        "evaluated" => DecisionEvaluationCoverage::Evaluated,
        "not_evaluated" => DecisionEvaluationCoverage::NotEvaluated,
        "failed" => DecisionEvaluationCoverage::Failed,
        "timed_out" => DecisionEvaluationCoverage::TimedOut,
        "cancelled" => DecisionEvaluationCoverage::Cancelled,
        "unsupported" => DecisionEvaluationCoverage::Unsupported,
        _ => {
            return Err(A2aLabError::invalid("coverage", format!("unknown {value}")));
        }
    };
    Ok(Some(parsed))
}

fn record_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<DecisionEvaluationJobRecord, A2aLabError> {
    let job_state = DecisionEvaluationJobState::parse(
        row.try_get::<String, _>("job_state")
            .map_err(db_error)?
            .as_str(),
    )?;
    let predicate_version = u32::try_from(
        row.try_get::<i64, _>("predicate_version")
            .map_err(db_error)?
            .max(0),
    )
    .unwrap_or(u32::MAX);
    let revision_generation = u32::try_from(
        row.try_get::<i64, _>("revision_generation")
            .map_err(db_error)?
            .max(0),
    )
    .unwrap_or(u32::MAX);
    let evaluation_attempts = u32::try_from(
        row.try_get::<i64, _>("evaluation_attempts")
            .map_err(db_error)?
            .max(0),
    )
    .unwrap_or(u32::MAX);
    Ok(DecisionEvaluationJobRecord {
        job_id: row.try_get("job_id").map_err(db_error)?,
        frame_id: row.try_get("frame_id").map_err(db_error)?,
        evaluation_id: row.try_get("evaluation_id").map_err(db_error)?,
        predicate_id: row.try_get("predicate_id").map_err(db_error)?,
        predicate_version,
        subject_digest: row.try_get("subject_digest").map_err(db_error)?,
        config_digest: row.try_get("config_digest").map_err(db_error)?,
        producer_revision: row.try_get("producer_revision").map_err(db_error)?,
        coverage_policy: row.try_get("coverage_policy").map_err(db_error)?,
        job_state,
        coverage: parse_coverage(
            row.try_get::<Option<String>, _>("coverage")
                .map_err(db_error)?
                .as_deref(),
        )?,
        gap_reason: row.try_get("gap_reason").map_err(db_error)?,
        revision_generation,
        lease_owner: row.try_get("lease_owner").map_err(db_error)?,
        lease_expires_at_us: row.try_get("lease_expires_at_us").map_err(db_error)?,
        evaluation_attempts,
        last_retry_reason: row.try_get("last_retry_reason").map_err(db_error)?,
        next_retry_at_us: row.try_get("next_retry_at_us").map_err(db_error)?,
        last_error_at_us: row.try_get("last_error_at_us").map_err(db_error)?,
        completed_at_us: row.try_get("completed_at_us").map_err(db_error)?,
        cancelled_at_us: row.try_get("cancelled_at_us").map_err(db_error)?,
        idempotency_key: row.try_get("idempotency_key").map_err(db_error)?,
        decision_id: row.try_get("decision_id").map_err(db_error)?,
    })
}

impl IndexedVideoDvr {
    pub(crate) async fn insert_decision_evaluation_job(
        &self,
        job_id: &str,
        frame_id: &str,
        evaluation_id: &str,
        predicate: &LabPredicate,
        subject_digest: &str,
        config_digest: &str,
        producer_revision: &str,
        coverage_policy: &str,
        created_at_us: i64,
    ) -> Result<bool, A2aLabError> {
        let result = sqlx::query(
            "INSERT INTO frame_decision_evaluation_jobs (
                job_id, frame_id, evaluation_id, predicate_id, predicate_version,
                subject_digest, config_digest, producer_revision, coverage_policy, created_at_us
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(
                evaluation_id, predicate_id, predicate_version,
                subject_digest, config_digest, producer_revision
             ) DO NOTHING",
        )
        .bind(job_id)
        .bind(frame_id)
        .bind(evaluation_id)
        .bind(predicate.id())
        .bind(i64::from(predicate.version()))
        .bind(subject_digest)
        .bind(config_digest)
        .bind(producer_revision)
        .bind(coverage_policy)
        .bind(created_at_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO frame_decision_evaluation_job_runtime (job_id)
             VALUES (?1)
             ON CONFLICT(job_id) DO NOTHING",
        )
        .bind(job_id)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(true)
    }

    /// Loads one decision evaluation job.
    pub async fn get_decision_evaluation_job(
        &self,
        job_id: &str,
    ) -> Result<DecisionEvaluationJobRecord, A2aLabError> {
        let row = sqlx::query(
            "SELECT j.job_id, j.frame_id, j.evaluation_id, j.predicate_id, j.predicate_version,
                    j.subject_digest, j.config_digest, j.producer_revision, j.coverage_policy,
                    COALESCE(r.job_state, 'pending') AS job_state, r.coverage, r.gap_reason,
                    COALESCE(r.revision_generation, 0) AS revision_generation,
                    r.lease_owner, r.lease_expires_at_us,
                    COALESCE(r.evaluation_attempts, 0) AS evaluation_attempts,
                    r.last_retry_reason, COALESCE(r.next_retry_at_us, 0) AS next_retry_at_us,
                    r.last_error_at_us, r.completed_at_us, r.cancelled_at_us,
                    r.idempotency_key, r.decision_id
             FROM frame_decision_evaluation_jobs j
             LEFT JOIN frame_decision_evaluation_job_runtime r ON r.job_id = j.job_id
             WHERE j.job_id = ?1",
        )
        .bind(job_id)
        .fetch_optional(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        let Some(row) = row else {
            return Err(A2aLabError::not_found("decision evaluation job", job_id));
        };
        record_from_row(&row)
    }

    /// Reclaims expired leases for decision jobs.
    pub async fn reclaim_expired_decision_evaluation_leases(
        &self,
        now_us: i64,
    ) -> Result<u32, A2aLabError> {
        let result = sqlx::query(
            "UPDATE frame_decision_evaluation_job_runtime
             SET job_state = 'pending',
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 last_retry_reason = COALESCE(last_retry_reason, 'lease_expired'),
                 last_error_at_us = ?1
             WHERE job_state = 'leased'
               AND lease_expires_at_us IS NOT NULL
               AND lease_expires_at_us <= ?1",
        )
        .bind(now_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(u32::try_from(result.rows_affected()).unwrap_or(0))
    }

    /// Claims bounded pending or retryable decision evaluation jobs.
    pub async fn claim_decision_evaluation_jobs(
        &self,
        worker_id: &str,
        limits: &SemanticDecisionLimits,
        lease_deadline_us: i64,
        now_us: i64,
    ) -> Result<Vec<DecisionEvaluationJobRecord>, A2aLabError> {
        let batch = i64::from(limits.max_batch_items.max(1));
        let candidates: Vec<(String, String)> = sqlx::query(
            "SELECT j.job_id, j.frame_id
             FROM frame_decision_evaluation_jobs j
             JOIN frame_decision_evaluation_job_runtime r ON r.job_id = j.job_id
             JOIN semantic_frame_revisions f ON f.frame_id = j.frame_id
             WHERE f.search_current = 1
               AND f.retained_state = 'retained'
               AND r.job_state IN ('pending', 'retryable_failed')
               AND r.next_retry_at_us <= ?1
               AND (r.lease_expires_at_us IS NULL OR r.lease_expires_at_us < ?1)
               AND (r.cancelled_at_us IS NULL)
             ORDER BY j.created_at_us ASC, j.job_id ASC
             LIMIT ?2",
        )
        .bind(now_us)
        .bind(batch)
        .fetch_all(self.sqlite_pool())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|row| {
            let job_id: String = row.try_get("job_id").map_err(db_error)?;
            let frame_id: String = row.try_get("frame_id").map_err(db_error)?;
            Ok((job_id, frame_id))
        })
        .collect::<Result<Vec<_>, A2aLabError>>()?;

        let mut claimed = Vec::new();
        for (job_id, frame_id) in candidates {
            if let Some(record) = self
                .try_claim_one_decision_job(
                    &job_id,
                    &frame_id,
                    worker_id,
                    lease_deadline_us,
                    now_us,
                )
                .await?
            {
                claimed.push(record);
            }
        }
        Ok(claimed)
    }

    async fn try_claim_one_decision_job(
        &self,
        job_id: &str,
        frame_id: &str,
        worker_id: &str,
        lease_deadline_us: i64,
        now_us: i64,
    ) -> Result<Option<DecisionEvaluationJobRecord>, A2aLabError> {
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision_seq FROM semantic_frame_revisions WHERE frame_id = ?1",
        )
        .bind(frame_id)
        .fetch_optional(self.sqlite_pool())
        .await
        .map_err(db_error)?
        .unwrap_or(0);
        let result = sqlx::query(
            "UPDATE frame_decision_evaluation_job_runtime
             SET job_state = 'leased',
                 lease_owner = ?2,
                 lease_expires_at_us = ?3,
                 revision_generation = ?4,
                 evaluation_attempts = evaluation_attempts + 1
             WHERE job_id = ?1
               AND job_state IN ('pending', 'retryable_failed')
               AND (lease_expires_at_us IS NULL OR lease_expires_at_us < ?5)
               AND next_retry_at_us <= ?5",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(lease_deadline_us)
        .bind(revision)
        .bind(now_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_decision_evaluation_job(job_id).await.map(Some)
    }

    /// Commits terminal coverage with lease ownership and revision generation checks.
    pub async fn complete_decision_evaluation_job_cas(
        &self,
        job_id: &str,
        worker_id: &str,
        revision_generation: u32,
        coverage: DecisionEvaluationCoverage,
        gap_reason: Option<&str>,
        idempotency_key: Option<&str>,
        decision_id: Option<&str>,
        now_us: i64,
    ) -> Result<bool, A2aLabError> {
        let coverage_s = coverage_label(coverage);
        let result = sqlx::query(
            "UPDATE frame_decision_evaluation_job_runtime
             SET job_state = 'committed',
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 coverage = ?4,
                 gap_reason = ?5,
                 idempotency_key = ?6,
                 decision_id = ?7,
                 completed_at_us = ?8,
                 last_retry_reason = NULL
             WHERE job_id = ?1
               AND job_state = 'leased'
               AND lease_owner = ?2
               AND revision_generation = ?3",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(i64::from(revision_generation))
        .bind(coverage_s)
        .bind(gap_reason)
        .bind(idempotency_key)
        .bind(decision_id)
        .bind(now_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected() > 0)
    }

    /// Closes queued jobs when no production Jev provider is available.
    pub async fn mark_decision_evaluation_jobs_unavailable(
        &self,
        coverage: DecisionEvaluationCoverage,
        reason: &str,
        now_us: i64,
    ) -> Result<u32, A2aLabError> {
        let coverage_s = coverage_label(coverage);
        let result = sqlx::query(
            "UPDATE frame_decision_evaluation_job_runtime
             SET job_state = 'committed',
                 coverage = ?1,
                 gap_reason = ?2,
                 completed_at_us = ?3,
                 last_retry_reason = NULL
             WHERE job_state IN ('pending', 'retryable_failed')",
        )
        .bind(coverage_s)
        .bind(reason)
        .bind(now_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(u32::try_from(result.rows_affected()).unwrap_or(u32::MAX))
    }

    /// Marks a leased job retryable after a transient evaluator outage.
    pub async fn fail_decision_evaluation_job_retryable_cas(
        &self,
        job_id: &str,
        worker_id: &str,
        revision_generation: u32,
        coverage: DecisionEvaluationCoverage,
        gap_reason: &str,
        next_retry_at_us: i64,
        now_us: i64,
        max_attempts: u32,
    ) -> Result<bool, A2aLabError> {
        let coverage_s = coverage_label(coverage);
        let attempts: i64 = sqlx::query_scalar(
            "SELECT evaluation_attempts FROM frame_decision_evaluation_job_runtime WHERE job_id = ?1",
        )
        .bind(job_id)
        .fetch_one(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        let terminal = u32::try_from(attempts.max(0)).unwrap_or(u32::MAX) >= max_attempts;
        let state = if terminal {
            "terminal_failed"
        } else {
            "retryable_failed"
        };
        let result = sqlx::query(
            "UPDATE frame_decision_evaluation_job_runtime
             SET job_state = ?6,
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 coverage = ?4,
                 gap_reason = ?5,
                 last_retry_reason = ?5,
                 last_error_at_us = ?7,
                 next_retry_at_us = ?8
             WHERE job_id = ?1
               AND job_state = 'leased'
               AND lease_owner = ?2
               AND revision_generation = ?3",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(i64::from(revision_generation))
        .bind(coverage_s)
        .bind(gap_reason)
        .bind(state)
        .bind(now_us)
        .bind(next_retry_at_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected() > 0)
    }

    /// Cancels one pending or leased job without invoking the evaluator.
    pub async fn cancel_decision_evaluation_job(
        &self,
        job_id: &str,
        now_us: i64,
    ) -> Result<bool, A2aLabError> {
        let result = sqlx::query(
            "UPDATE frame_decision_evaluation_job_runtime
             SET job_state = 'cancelled',
                 lease_owner = NULL,
                 lease_expires_at_us = NULL,
                 coverage = 'cancelled',
                 gap_reason = 'operator_cancelled',
                 cancelled_at_us = ?2,
                 completed_at_us = ?2
             WHERE job_id = ?1
               AND job_state IN ('pending', 'retryable_failed', 'leased')",
        )
        .bind(job_id)
        .bind(now_us)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected() > 0)
    }
}

/// Per-policy gap accounting for saved frames.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DecisionEvaluationCoverageReport {
    policy: String,
    evaluated: u32,
    not_evaluated: u32,
    failed: u32,
    unsupported: u32,
    cancelled: u32,
    policy_gaps: u32,
    pending_jobs: u32,
}

impl DecisionEvaluationCoverageReport {
    #[must_use]
    pub fn policy(&self) -> &str {
        &self.policy
    }

    #[must_use]
    pub const fn policy_gaps(&self) -> u32 {
        self.policy_gaps
    }

    #[must_use]
    pub const fn not_evaluated(&self) -> u32 {
        self.not_evaluated
    }
}

impl IndexedVideoDvr {
    /// Summarizes committed job coverage and explicit policy skips for one frame set.
    pub async fn summarize_decision_evaluation_coverage(
        &self,
        frame_ids: &[String],
        policy: &str,
    ) -> Result<DecisionEvaluationCoverageReport, A2aLabError> {
        if frame_ids.is_empty() {
            return Ok(DecisionEvaluationCoverageReport {
                policy: policy.to_owned(),
                ..DecisionEvaluationCoverageReport::default()
            });
        }
        let placeholders = frame_ids
            .iter()
            .enumerate()
            .map(|(index, _)| format!("?{}", index + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT COALESCE(r.coverage, 'not_evaluated') AS coverage, r.job_state
             FROM frame_decision_evaluation_jobs j
             JOIN frame_decision_evaluation_job_runtime r ON r.job_id = j.job_id
             WHERE j.coverage_policy = ?1
               AND j.frame_id IN ({placeholders})"
        );
        let mut query_builder = sqlx::query(&query).bind(policy);
        for frame_id in frame_ids {
            query_builder = query_builder.bind(frame_id);
        }
        let rows = query_builder
            .fetch_all(self.sqlite_pool())
            .await
            .map_err(db_error)?;
        let mut report = DecisionEvaluationCoverageReport {
            policy: policy.to_owned(),
            ..DecisionEvaluationCoverageReport::default()
        };
        for row in rows {
            let coverage: String = row.try_get("coverage").map_err(db_error)?;
            let state: String = row.try_get("job_state").map_err(db_error)?;
            match coverage.as_str() {
                "evaluated" => report.evaluated = report.evaluated.saturating_add(1),
                "failed" | "timed_out" => report.failed = report.failed.saturating_add(1),
                "unsupported" => report.unsupported = report.unsupported.saturating_add(1),
                "cancelled" => report.cancelled = report.cancelled.saturating_add(1),
                _ if state == "pending" || state == "leased" || state == "retryable_failed" => {
                    report.pending_jobs = report.pending_jobs.saturating_add(1);
                    report.not_evaluated = report.not_evaluated.saturating_add(1);
                }
                _ => report.not_evaluated = report.not_evaluated.saturating_add(1),
            }
        }
        report.policy_gaps = report
            .not_evaluated
            .saturating_add(report.pending_jobs)
            .saturating_add(report.failed);
        Ok(report)
    }
}
