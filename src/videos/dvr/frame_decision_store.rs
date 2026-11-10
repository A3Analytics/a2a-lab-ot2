//! Immutable Jev frame decisions persisted in the DVR SQLite index.

use std::collections::HashSet;

use std::collections::BTreeMap;

use a2a_lab_dev_kit::{
    A2aLabError, AppendFrameDecisionResult, DecisionEvidenceAvailability, DecisionEvidenceItem,
    DecisionEvidenceKind, DecisionHistoryPolicy, DecisionOutcome, DecisionSubject,
    FrameDecisionAppendError, FrameDecisionHistoryEntry, FrameDecisionHistoryResult,
    FrameDecisionId, FrameDecisionReadScope, FrameDecisionRecord, JevProducerIdentity,
    LabPredicate, Page, PageRequest, ReadFrameDecisionHistoryRequest, SavedFrameId, SavedFrameRef,
    Sha256Digest,
};
use sqlx::{Row, Sqlite, Transaction};

use super::frame_manifest::{ManifestRetainedState, MediaPayloadStatus};
use super::store::{IndexedVideoDvr, db_error};

impl IndexedVideoDvr {
    /// Reconciles decision tombstones from manifest retention state (restart-safe).
    pub async fn reconcile_frame_decision_tombstones(&self) -> Result<u64, A2aLabError> {
        let now = crate::time::now()?.as_micros();
        let result = sqlx::query(
            "INSERT INTO frame_decision_tombstones (frame_id, retained_state, tombstoned_at_us)
             SELECT frame_id, retained_state, ?1
             FROM semantic_frame_revisions
             WHERE retained_state IN ('expired', 'superseded')
               AND frame_id NOT IN (SELECT frame_id FROM frame_decision_tombstones)
             ON CONFLICT(frame_id) DO NOTHING",
        )
        .bind(now)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        let marked = sqlx::query(
            "UPDATE frame_decisions
             SET tombstoned = 1
             WHERE frame_id IN (SELECT frame_id FROM frame_decision_tombstones)
               AND tombstoned = 0",
        )
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected() + marked.rows_affected())
    }

    /// Append-only Jev decision persistence independent of description/embedding checkpoints.
    pub async fn append_frame_decision(
        &self,
        record: FrameDecisionRecord,
    ) -> Result<AppendFrameDecisionResult, FrameDecisionAppendError> {
        record
            .validate()
            .map_err(FrameDecisionAppendError::Validation)?;
        self.validate_append_manifest_revision(record.frame())
            .await?;

        let fingerprint = record
            .idempotent_fingerprint()
            .map_err(FrameDecisionAppendError::Validation)?;
        let payload_json = serde_json::to_string(&record).map_err(|error| {
            FrameDecisionAppendError::Validation(A2aLabError::protocol(error.to_string()))
        })?;
        let subject_digest = subject_digest(record.subject())?;

        let mut tx = self
            .sqlite_pool()
            .begin()
            .await
            .map_err(append_validation)?;

        if let Some(existing) = load_by_idempotency_tx(&mut tx, record.idempotency_key().as_str())
            .await
            .map_err(append_validation)?
        {
            if existing.fingerprint == fingerprint.as_str() {
                tx.commit().await.map_err(append_validation)?;
                let stored = existing.record;
                return Ok(AppendFrameDecisionResult::new(stored, true));
            }
            return Err(FrameDecisionAppendError::IdempotencyConflict {
                key: record.idempotency_key().clone(),
                existing: existing.record.decision_id().clone(),
            });
        }

        if let Some(prior_id) = record.supersedes() {
            validate_supersession_tx(&mut tx, &record, prior_id).await?;
        }

        let append_seq: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(append_seq), 0) + 1 FROM frame_decisions")
                .fetch_one(&mut *tx)
                .await
                .map_err(append_validation)?;

        sqlx::query(
            "INSERT INTO frame_decisions (
                decision_id, idempotency_key, frame_id, source_id, captured_at_us,
                predicate_id, predicate_version, subject_digest, producer_id, model_revision,
                evaluated_at_us, payload_json, payload_fingerprint, supersedes, append_seq
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )
        .bind(record.decision_id().as_str())
        .bind(record.idempotency_key().as_str())
        .bind(record.frame().frame_id().as_str())
        .bind(record.frame().source_id().as_str())
        .bind(record.frame().captured_at().as_micros())
        .bind(record.predicate().id())
        .bind(i64::from(record.predicate().version()))
        .bind(subject_digest)
        .bind(record.provenance().producer().producer_id())
        .bind(record.provenance().producer().model_revision())
        .bind(record.provenance().evaluated_at().as_micros())
        .bind(payload_json)
        .bind(fingerprint.as_str())
        .bind(record.supersedes().map(FrameDecisionId::as_str))
        .bind(append_seq)
        .execute(&mut *tx)
        .await
        .map_err(append_validation)?;

        tx.commit().await.map_err(append_validation)?;
        Ok(AppendFrameDecisionResult::new(record, false))
    }

    /// Bounded history read with explicit reduction policies.
    pub async fn read_frame_decision_history(
        &self,
        request: &ReadFrameDecisionHistoryRequest,
    ) -> Result<FrameDecisionHistoryResult, A2aLabError> {
        request.check()?;
        let records = self.load_decisions_in_scope(request.scope()).await?;
        let filtered = filter_scope_records(
            &records,
            request.scope(),
            request.predicate(),
            request.subject(),
            request.producer(),
            None,
        );
        let reduced =
            reduce_history_records(filtered, request.history_policy(), request.producer());
        let total = reduced.len();
        let offset = page_offset(request.page())?;
        let limit = request.page().limit() as usize;
        let page_items = build_history_page(
            self,
            reduced,
            offset,
            limit,
            request.report_unavailable_evidence(),
        )
        .await?;
        let next_offset = offset + page_items.len();
        let cursor = if next_offset < total {
            Some(next_offset.to_string())
        } else {
            None
        };
        Ok(FrameDecisionHistoryResult::new(
            request.scope().clone(),
            request.history_policy(),
            Page::new(page_items, cursor),
        ))
    }

    async fn validate_append_manifest_revision(
        &self,
        frame: &SavedFrameRef,
    ) -> Result<(), FrameDecisionAppendError> {
        let manifest = self
            .get_manifest_frame(frame.frame_id().as_str())
            .await
            .map_err(FrameDecisionAppendError::Validation)?;
        if manifest.frame() != frame {
            return Err(FrameDecisionAppendError::Validation(A2aLabError::invalid(
                "frame",
                "record frame revision does not match the retained manifest row",
            )));
        }
        if !matches!(
            manifest.retained_state(),
            ManifestRetainedState::Superseded | ManifestRetainedState::Expired
        ) && matches!(
            manifest.payload_status(),
            MediaPayloadStatus::Missing | MediaPayloadStatus::Corrupt
        ) {
            return Err(FrameDecisionAppendError::Validation(A2aLabError::invalid(
                "frame",
                "saved frame evidence is not accessible for this revision",
            )));
        }
        Ok(())
    }

    /// Records one human evaluation-corpus label (never a model decision).
    pub async fn insert_human_corpus_label(
        &self,
        label_id: &str,
        frame_id: &str,
        predicate_id: &str,
        label_json: &str,
    ) -> Result<(), A2aLabError> {
        if label_id.trim().is_empty() {
            return Err(A2aLabError::invalid("label_id", "must not be empty"));
        }
        let labeled_at = crate::time::now()?.as_micros();
        sqlx::query(
            "INSERT INTO human_corpus_labels (label_id, frame_id, predicate_id, label_json, labeled_at_us)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(label_id)
        .bind(frame_id)
        .bind(predicate_id)
        .bind(label_json)
        .bind(labeled_at)
        .execute(self.sqlite_pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    pub(crate) async fn tombstone_decisions_for_frame_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        frame_id: &str,
        retained_state: &str,
        tombstoned_at_us: i64,
    ) -> Result<(), A2aLabError> {
        sqlx::query(
            "INSERT INTO frame_decision_tombstones (frame_id, retained_state, tombstoned_at_us)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(frame_id) DO UPDATE SET
                retained_state = excluded.retained_state,
                tombstoned_at_us = excluded.tombstoned_at_us",
        )
        .bind(frame_id)
        .bind(retained_state)
        .bind(tombstoned_at_us)
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
        sqlx::query("UPDATE frame_decisions SET tombstoned = 1 WHERE frame_id = ?1")
            .bind(frame_id)
            .execute(&mut **tx)
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn load_decisions_in_scope(
        &self,
        scope: &FrameDecisionReadScope,
    ) -> Result<Vec<FrameDecisionRecord>, A2aLabError> {
        let start = scope.range().start().as_micros();
        let end = scope.range().end().as_micros();
        let frame_pin = scope
            .frame()
            .map(|frame| frame.frame_id().as_str().to_owned());
        let rows = if let Some(frame_id) = &frame_pin {
            sqlx::query(
                "SELECT payload_json FROM frame_decisions
                 WHERE source_id = ?1
                   AND captured_at_us >= ?2 AND captured_at_us < ?3
                   AND frame_id = ?4
                 ORDER BY append_seq ASC",
            )
            .bind(scope.source_id().as_str())
            .bind(start)
            .bind(end)
            .bind(frame_id)
            .fetch_all(self.sqlite_pool())
            .await
            .map_err(db_error)?
        } else {
            sqlx::query(
                "SELECT payload_json FROM frame_decisions
                 WHERE source_id = ?1
                   AND captured_at_us >= ?2 AND captured_at_us < ?3
                 ORDER BY append_seq ASC",
            )
            .bind(scope.source_id().as_str())
            .bind(start)
            .bind(end)
            .fetch_all(self.sqlite_pool())
            .await
            .map_err(db_error)?
        };
        rows.into_iter()
            .map(|row| decode_record_row(&row))
            .collect()
    }

    async fn frame_is_tombstoned(&self, frame_id: &SavedFrameId) -> Result<bool, A2aLabError> {
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM frame_decision_tombstones WHERE frame_id = ?1")
                .bind(frame_id.as_str())
                .fetch_optional(self.sqlite_pool())
                .await
                .map_err(db_error)?;
        Ok(exists.is_some())
    }
}

struct StoredDecisionRow {
    record: FrameDecisionRecord,
    fingerprint: String,
}

async fn load_by_idempotency_tx(
    tx: &mut Transaction<'_, Sqlite>,
    key: &str,
) -> Result<Option<StoredDecisionRow>, A2aLabError> {
    let row = sqlx::query(
        "SELECT payload_json, payload_fingerprint FROM frame_decisions WHERE idempotency_key = ?1",
    )
    .bind(key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(decode_stored_row(&row)?))
}

async fn load_by_decision_id_tx(
    tx: &mut Transaction<'_, Sqlite>,
    decision_id: &str,
) -> Result<Option<FrameDecisionRecord>, A2aLabError> {
    let row = sqlx::query("SELECT payload_json FROM frame_decisions WHERE decision_id = ?1")
        .bind(decision_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db_error)?;
    row.map(|row| decode_record_row(&row)).transpose()
}

async fn validate_supersession_tx(
    tx: &mut Transaction<'_, Sqlite>,
    record: &FrameDecisionRecord,
    prior_id: &FrameDecisionId,
) -> Result<(), FrameDecisionAppendError> {
    let prior = load_by_decision_id_tx(tx, prior_id.as_str())
        .await
        .map_err(append_validation)?
        .ok_or_else(|| FrameDecisionAppendError::InvalidSupersession {
            prior: prior_id.clone(),
            message: "superseded decision was not found".to_owned(),
        })?;
    if prior.frame().frame_id() != record.frame().frame_id() {
        return Err(FrameDecisionAppendError::InvalidSupersession {
            prior: prior_id.clone(),
            message: "supersession must stay on the same frame revision".to_owned(),
        });
    }
    if prior.predicate() != record.predicate() {
        return Err(FrameDecisionAppendError::InvalidSupersession {
            prior: prior_id.clone(),
            message: "supersession must keep the same predicate".to_owned(),
        });
    }
    if prior.subject() != record.subject() {
        return Err(FrameDecisionAppendError::InvalidSupersession {
            prior: prior_id.clone(),
            message: "supersession must keep the same subject".to_owned(),
        });
    }
    if prior.provenance().producer() != record.provenance().producer() {
        return Err(FrameDecisionAppendError::InvalidSupersession {
            prior: prior_id.clone(),
            message: "supersession must stay within the same Jev producer stream".to_owned(),
        });
    }
    if !acyclic_supersession_tx(tx, prior_id)
        .await
        .map_err(append_validation)?
    {
        return Err(FrameDecisionAppendError::InvalidSupersession {
            prior: prior_id.clone(),
            message: "supersession chain would form a cycle".to_owned(),
        });
    }
    Ok(())
}

async fn acyclic_supersession_tx(
    tx: &mut Transaction<'_, Sqlite>,
    start: &FrameDecisionId,
) -> Result<bool, A2aLabError> {
    let mut seen = HashSet::new();
    let mut current = Some(start.clone());
    while let Some(id) = current {
        if !seen.insert(id.clone()) {
            return Ok(false);
        }
        let next: Option<String> =
            sqlx::query_scalar("SELECT supersedes FROM frame_decisions WHERE decision_id = ?1")
                .bind(id.as_str())
                .fetch_optional(&mut **tx)
                .await
                .map_err(db_error)?;
        current = match next {
            Some(value) if value.trim().is_empty() => None,
            Some(value) => Some(FrameDecisionId::new(value)?),
            None => None,
        };
    }
    Ok(true)
}

async fn build_history_page(
    store: &IndexedVideoDvr,
    records: Vec<&FrameDecisionRecord>,
    offset: usize,
    limit: usize,
    report_unavailable: &[String],
) -> Result<Vec<FrameDecisionHistoryEntry>, A2aLabError> {
    let mut page = Vec::new();
    for record in records.into_iter().skip(offset).take(limit) {
        let tombstoned = store.frame_is_tombstoned(record.frame().frame_id()).await?;
        let unavailable: Vec<String> = if tombstoned {
            record.evidence().iter().map(evidence_report_key).collect()
        } else {
            report_unavailable.to_vec()
        };
        let evidence = record
            .evidence()
            .iter()
            .map(|item| evidence_availability(item, &unavailable))
            .collect();
        page.push(FrameDecisionHistoryEntry::new(record.clone(), evidence));
    }
    Ok(page)
}

fn evidence_report_key(item: &DecisionEvidenceItem) -> String {
    let kind = match item.kind() {
        DecisionEvidenceKind::SavedFrameRevision => "saved_frame_revision",
        DecisionEvidenceKind::ReferenceImage => "reference_image",
    };
    format!("{}:{kind}", item.frame_id().as_str())
}

fn evidence_availability(
    item: &DecisionEvidenceItem,
    unavailable_keys: &[String],
) -> DecisionEvidenceAvailability {
    let key = evidence_report_key(item);
    if unavailable_keys.iter().any(|candidate| candidate == &key) {
        DecisionEvidenceAvailability::Unavailable {
            reason: "retained evidence bytes are not currently resolvable".to_owned(),
        }
    } else {
        DecisionEvidenceAvailability::Ready
    }
}

fn page_offset(page: &PageRequest) -> Result<usize, A2aLabError> {
    page.check()?;
    match page.cursor() {
        None => Ok(0),
        Some(cursor) => cursor
            .parse()
            .map_err(|_| A2aLabError::invalid("cursor", "is not a valid page cursor")),
    }
}

fn decode_stored_row(row: &sqlx::sqlite::SqliteRow) -> Result<StoredDecisionRow, A2aLabError> {
    let payload: String = row.try_get("payload_json").map_err(db_error)?;
    let fingerprint: String = row.try_get("payload_fingerprint").map_err(db_error)?;
    let record: FrameDecisionRecord =
        serde_json::from_str(&payload).map_err(|error| A2aLabError::protocol(error.to_string()))?;
    record.validate()?;
    Ok(StoredDecisionRow {
        record,
        fingerprint,
    })
}

fn decode_record_row(row: &sqlx::sqlite::SqliteRow) -> Result<FrameDecisionRecord, A2aLabError> {
    let payload: String = row.try_get("payload_json").map_err(db_error)?;
    let record: FrameDecisionRecord =
        serde_json::from_str(&payload).map_err(|error| A2aLabError::protocol(error.to_string()))?;
    record.validate()?;
    Ok(record)
}

fn subject_digest(subject: &DecisionSubject) -> Result<String, FrameDecisionAppendError> {
    let json = serde_json::to_string(subject).map_err(|error| {
        FrameDecisionAppendError::Validation(A2aLabError::protocol(error.to_string()))
    })?;
    Ok(Sha256Digest::hash_bytes(json.as_bytes())
        .as_str()
        .to_owned())
}

fn append_validation(error: impl std::fmt::Display) -> FrameDecisionAppendError {
    FrameDecisionAppendError::Validation(db_error(error))
}

fn filter_scope_records<'a>(
    records: &'a [FrameDecisionRecord],
    scope: &FrameDecisionReadScope,
    predicate: Option<&LabPredicate>,
    subject: Option<&DecisionSubject>,
    producer: Option<&JevProducerIdentity>,
    outcome: Option<DecisionOutcome>,
) -> Vec<&'a FrameDecisionRecord> {
    records
        .iter()
        .filter(|record| scope.contains(record))
        .filter(|record| predicate.is_none_or(|expected| record.predicate() == expected))
        .filter(|record| subject.is_none_or(|expected| record.subject() == expected))
        .filter(|record| producer.is_none_or(|expected| record.provenance().producer() == expected))
        .filter(|record| outcome.is_none_or(|expected| record.outcome() == expected))
        .collect()
}

fn reduce_history_records<'a>(
    records: Vec<&'a FrameDecisionRecord>,
    policy: DecisionHistoryPolicy,
    producer: Option<&JevProducerIdentity>,
) -> Vec<&'a FrameDecisionRecord> {
    match policy {
        DecisionHistoryPolicy::FullHistory => records,
        DecisionHistoryPolicy::LatestPerPredicateSubject => {
            latest_grouped_records(records, |record| group_key_record(record, None))
        }
        DecisionHistoryPolicy::LatestAuthorizedProduction => {
            let mut retained = records;
            if let Some(expected) = producer {
                retained.retain(|record| record.provenance().producer() == expected);
            }
            latest_grouped_records(retained, |record| group_key_record(record, producer))
        }
    }
}

fn latest_grouped_records<'a>(
    records: Vec<&'a FrameDecisionRecord>,
    key: impl Fn(&'a FrameDecisionRecord) -> String,
) -> Vec<&'a FrameDecisionRecord> {
    let mut chosen: BTreeMap<String, &FrameDecisionRecord> = BTreeMap::new();
    for record in records {
        let bucket = key(record);
        let replace = match chosen.get(&bucket) {
            None => true,
            Some(prior) => record.provenance().evaluated_at() >= prior.provenance().evaluated_at(),
        };
        if replace {
            chosen.insert(bucket, record);
        }
    }
    let mut ordered: Vec<_> = chosen.into_values().collect();
    ordered.sort_by_key(|record| record.provenance().evaluated_at());
    ordered
}

fn group_key_record(
    record: &FrameDecisionRecord,
    producer: Option<&JevProducerIdentity>,
) -> String {
    let producer = if let Some(identity) = producer.or(Some(record.provenance().producer())) {
        format!("{}@{}", identity.producer_id(), identity.model_revision())
    } else {
        "any".to_owned()
    };
    format!(
        "{}|v{}|{producer}|{}|{}|{}",
        record.predicate().id(),
        record.predicate().version(),
        subject_token(record.subject()),
        record.frame().frame_id().as_str(),
        record.provenance().evaluation_id().as_str(),
    )
}

fn subject_token(subject: &DecisionSubject) -> String {
    serde_json::to_string(subject).unwrap_or_else(|_| "subject".to_owned())
}

#[cfg(test)]
mod unit {
    use super::subject_digest;
    use a2a_lab_dev_kit::{DecisionSubject, FrameRoiTransform, PipetteChannelSubject};

    #[test]
    fn subject_digest_is_stable() {
        let subject = DecisionSubject::PipetteChannel(
            PipetteChannelSubject::new(
                "pip",
                "ch1",
                Some(FrameRoiTransform::new([0.1, 0.2, 0.3, 0.4]).unwrap()),
            )
            .unwrap(),
        );
        let first = subject_digest(&subject).unwrap();
        let second = subject_digest(&subject).unwrap();
        assert_eq!(first, second);
    }
}
