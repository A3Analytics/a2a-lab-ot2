//! Operator-authored pilot predicates, subjects, and criteria for Chupacabra DVR.

use a2a_lab_dev_kit::{
    DecisionSubject, ForeignObjectSubject, FrameDecisionEvaluatorProviderConfig, FrameRoiTransform,
    LabPredicate, OperatorDecisionCriteria, OperatorReferenceImage, PipetteChannelSubject,
    PlateAlignmentReference, PlateSlotSubject, Sha256Digest,
};

/// Pilot evaluator configuration digest pins.
#[must_use]
pub fn pilot_evaluator_config() -> FrameDecisionEvaluatorProviderConfig {
    FrameDecisionEvaluatorProviderConfig::pilot(
        Sha256Digest::hash_bytes(b"ot2-pilot-jev-eval-config"),
        Sha256Digest::hash_bytes(b"ot2-pilot-jev-threshold"),
    )
    .expect("pilot evaluator config")
}

/// Operator criteria bundle for Chupacabra pilot stills (not Luna-derived).
#[must_use]
pub fn pilot_operator_criteria() -> OperatorDecisionCriteria {
    OperatorDecisionCriteria::new(
        Sha256Digest::hash_bytes(b"chupacabra-operator-criteria-v1"),
        Sha256Digest::hash_bytes(b"chupacabra-operator-prompt-v1"),
        vec![
            OperatorReferenceImage::new(
                Sha256Digest::hash_bytes(b"chupacabra-reference-tip-expected"),
                "expected-tip",
                FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
            )
            .unwrap(),
        ],
    )
    .unwrap()
}

/// The three initial lab predicates with pilot subjects.
#[must_use]
pub fn pilot_predicate_targets() -> Vec<(LabPredicate, DecisionSubject)> {
    vec![
        (
            LabPredicate::pipette_tip_attached_v1(),
            DecisionSubject::PipetteChannel(
                PipetteChannelSubject::new(
                    "left-pipetter",
                    "channel-1",
                    Some(FrameRoiTransform::new([0.2, 0.3, 0.5, 0.7]).unwrap()),
                )
                .unwrap(),
            ),
        ),
        (
            LabPredicate::foreign_object_present_v1(),
            DecisionSubject::ForeignObject(
                ForeignObjectSubject::new(
                    "deck-slot-3",
                    FrameRoiTransform::new([0.1, 0.1, 0.9, 0.9]).unwrap(),
                    Some(Sha256Digest::hash_bytes(b"chupacabra-expected-scene-v1")),
                    None,
                )
                .unwrap(),
            ),
        ),
        (
            LabPredicate::plate_misaligned_v1(),
            DecisionSubject::PlateSlot(
                PlateSlotSubject::new(
                    "plate-a",
                    "slot-3",
                    PlateAlignmentReference::ExpectedPoseDigest(Sha256Digest::hash_bytes(
                        b"chupacabra-plate-pose-v1",
                    )),
                    None,
                )
                .unwrap(),
            ),
        ),
    ]
}
