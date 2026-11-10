//! SQLite-backed [`FrameDecisionLedgerProvider`] over the indexed DVR store.

use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, AppendFrameDecisionRequest, AppendFrameDecisionResult, FrameDecisionCapabilities,
    FrameDecisionClientRole, FrameDecisionEvaluatorLimits, FrameDecisionEvaluatorProviderConfig,
    FrameDecisionHistoryResult, FrameDecisionLedgerProvider, FrameDecisionOperation,
    GetFrameDecisionCapabilitiesRequest, ReadFrameDecisionHistoryRequest,
    RegisteredFrameDecisionEvaluator, Sha256Digest, frame_decision_unavailable, map_append_error,
};
use async_trait::async_trait;

use super::store::IndexedVideoDvr;

/// Persistent Jev decision ledger stored beside DVR SQLite metadata.
#[derive(Clone)]
pub struct DvrFrameDecisionLedger {
    role: FrameDecisionClientRole,
    registered: Option<RegisteredFrameDecisionEvaluator>,
    store: Arc<IndexedVideoDvr>,
}

impl DvrFrameDecisionLedger {
    /// Authorized SQLite ledger for OT-2 serve with pilot Jev predicates pinned.
    pub fn pilot_authorized_producer(
        store: Arc<IndexedVideoDvr>,
    ) -> Result<Arc<Self>, A2aLabError> {
        let config = FrameDecisionEvaluatorProviderConfig::pilot(
            Sha256Digest::hash_bytes(b"ot2-frame-decision-eval-config"),
            Sha256Digest::hash_bytes(b"ot2-frame-decision-threshold-policy"),
        )?;
        Ok(Arc::new(Self::authorized_producer(
            store,
            &config,
            FrameDecisionEvaluatorLimits::pilot(),
        )?))
    }

    /// Read-only consumer bound to one DVR store.
    pub fn read_only_consumer(
        store: Arc<IndexedVideoDvr>,
        config: &FrameDecisionEvaluatorProviderConfig,
        limits: FrameDecisionEvaluatorLimits,
    ) -> Result<Self, A2aLabError> {
        Ok(Self {
            role: FrameDecisionClientRole::ReadOnlyConsumer,
            registered: Some(RegisteredFrameDecisionEvaluator::register(config, limits)?),
            store,
        })
    }

    /// Authorized producer that may append through the DVR store.
    pub fn authorized_producer(
        store: Arc<IndexedVideoDvr>,
        config: &FrameDecisionEvaluatorProviderConfig,
        limits: FrameDecisionEvaluatorLimits,
    ) -> Result<Self, A2aLabError> {
        Ok(Self {
            role: FrameDecisionClientRole::AuthorizedProducer,
            registered: Some(RegisteredFrameDecisionEvaluator::register(config, limits)?),
            store,
        })
    }

    fn registered_capabilities(&self) -> FrameDecisionCapabilities {
        let Some(registered) = &self.registered else {
            return FrameDecisionCapabilities::unregistered(self.role);
        };
        FrameDecisionCapabilities::registered(self.role, registered.capabilities().clone())
    }
}

#[async_trait]
impl FrameDecisionLedgerProvider for DvrFrameDecisionLedger {
    fn client_role(&self) -> FrameDecisionClientRole {
        self.role
    }

    async fn get_frame_decision_capabilities(
        &self,
        _request: GetFrameDecisionCapabilitiesRequest,
    ) -> Result<FrameDecisionCapabilities, A2aLabError> {
        Ok(self.registered_capabilities())
    }

    async fn append_frame_decision(
        &self,
        request: AppendFrameDecisionRequest,
    ) -> Result<AppendFrameDecisionResult, A2aLabError> {
        if self.registered.is_none() {
            return Err(frame_decision_unavailable());
        }
        if self.role != FrameDecisionClientRole::AuthorizedProducer {
            return Err(A2aLabError::invalid(
                "client_role",
                "append requires an authorized Jev producer",
            ));
        }
        if !self
            .registered_capabilities()
            .operations()
            .contains(&FrameDecisionOperation::AppendFrameDecision)
        {
            return Err(A2aLabError::invalid(
                "operation",
                "append is not advertised for this caller",
            ));
        }
        request.check()?;
        let prior = self
            .store
            .append_frame_decision(request.record().clone())
            .await
            .map_err(map_append_error)?;
        Ok(prior)
    }

    async fn read_frame_decision_history(
        &self,
        request: ReadFrameDecisionHistoryRequest,
    ) -> Result<FrameDecisionHistoryResult, A2aLabError> {
        if self.registered.is_none() {
            return Err(frame_decision_unavailable());
        }
        self.store.read_frame_decision_history(&request).await
    }
}
