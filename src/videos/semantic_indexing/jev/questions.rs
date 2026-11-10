//! Maps lab predicates to independent System One questions.

use a2a_lab_dev_kit::{
    FrameDecisionEvaluationFailure, FrameDecisionEvaluationRequest, LabPredicate,
    PREDICATE_FOREIGN_OBJECT_PRESENT, PREDICATE_PLATE_MISALIGNED,
};

use super::wire::WireQuestion;

/// Suffixes for sub-questions tied to one evaluation request.
pub const QNOUL: &str = "noul";
pub const QCHOICE: &str = "choice";
pub const QSCORE: &str = "score";
pub const QBOX: &str = "box";

pub fn question_id(evaluation_id: &str, suffix: &str) -> String {
    format!("{evaluation_id}:{suffix}")
}

/// Builds operator-authored primitive questions for one lab predicate.
pub fn predicate_questions(
    request: &FrameDecisionEvaluationRequest,
    frame_ref: &str,
) -> Result<Vec<(String, WireQuestion)>, FrameDecisionEvaluationFailure> {
    let eval = request.evaluation_id().as_str();
    let predicate = request.predicate();
    let mut out = Vec::new();
    if predicate == &LabPredicate::pipette_tip_attached_v1() {
        out.push((
            question_id(eval, QNOUL),
            WireQuestion {
                kind: "noul",
                instructions: format!(
                    "Is a pipette tip attached on the pipetter channel in `{frame_ref}`?"
                ),
                criteria: None,
            },
        ));
        out.push((
            question_id(eval, QBOX),
            WireQuestion {
                kind: "box",
                instructions: format!("the pipette tip evidence in `{frame_ref}`"),
                criteria: None,
            },
        ));
        return Ok(out);
    }
    if predicate.id() == PREDICATE_FOREIGN_OBJECT_PRESENT {
        out.push((
            question_id(eval, QNOUL),
            WireQuestion {
                kind: "noul",
                instructions: format!(
                    "Is a foreign object visible in the deck region in `{frame_ref}`?"
                ),
                criteria: None,
            },
        ));
        out.push((
            question_id(eval, QCHOICE),
            WireQuestion {
                kind: "choice",
                instructions: format!("Foreign object visibility in `{frame_ref}`"),
                criteria: Some(serde_json::json!({
                    "present": "A foreign object is clearly visible.",
                    "absent": "No foreign object is visible.",
                    "occluded": "The region is occluded or out of view.",
                    "unknown": "Evidence is insufficient to decide."
                })),
            },
        ));
        out.push((
            question_id(eval, QBOX),
            WireQuestion {
                kind: "box",
                instructions: format!("any foreign object in `{frame_ref}`"),
                criteria: None,
            },
        ));
        return Ok(out);
    }
    if predicate.id() == PREDICATE_PLATE_MISALIGNED {
        out.push((
            question_id(eval, QSCORE),
            WireQuestion {
                kind: "score",
                instructions: format!("Plate alignment severity in `{frame_ref}`"),
                criteria: Some(serde_json::json!([
                    "Well aligned with the deck reference",
                    "Slightly misaligned",
                    "Clearly misaligned"
                ])),
            },
        ));
        out.push((
            question_id(eval, QNOUL),
            WireQuestion {
                kind: "noul",
                instructions: format!("Is the plate misaligned in `{frame_ref}`?"),
                criteria: None,
            },
        ));
        return Ok(out);
    }
    Err(FrameDecisionEvaluationFailure::UnsupportedPredicate {
        predicate_id: predicate.id().to_owned(),
        predicate_version: predicate.version(),
    })
}
