//! Injectable faults for durable Jev evaluation worker tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Test-only faults on the Jev evaluation worker path.
#[derive(Debug, Default)]
pub struct DecisionEvaluationWorkerFaults {
    crash_before_decision_commit: AtomicBool,
    force_evidence_failure: AtomicBool,
    luna_spy_calls: AtomicU32,
    chat_spy_calls: AtomicU32,
}

impl DecisionEvaluationWorkerFaults {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn set_crash_before_decision_commit(&self, value: bool) {
        self.crash_before_decision_commit
            .store(value, Ordering::Relaxed);
    }

    #[must_use]
    pub fn crash_before_decision_commit(&self) -> bool {
        self.crash_before_decision_commit.load(Ordering::Relaxed)
    }

    pub fn set_force_evidence_failure(&self, value: bool) {
        self.force_evidence_failure.store(value, Ordering::Relaxed);
    }

    #[must_use]
    pub fn force_evidence_failure(&self) -> bool {
        self.force_evidence_failure.load(Ordering::Relaxed)
    }

    /// Reserved for tests proving the decision path never reaches Luna.
    #[must_use]
    pub fn luna_spy_calls(&self) -> u32 {
        self.luna_spy_calls.load(Ordering::Relaxed)
    }

    /// Reserved for tests proving the decision path never reaches chat.
    #[must_use]
    pub fn chat_spy_calls(&self) -> u32 {
        self.chat_spy_calls.load(Ordering::Relaxed)
    }
}
