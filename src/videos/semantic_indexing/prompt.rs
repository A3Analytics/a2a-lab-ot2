//! Versioned Luna description prompt digests (independent from chat).

use a2a_lab_dev_kit::videos::Sha256Digest;

/// Pilot prompt: bounded lab-scene description for retrieval only (not Jev input).
pub const DESCRIPTION_PROMPT: &str = "\
Describe the visible lab scene in one or two concise sentences for search retrieval. \
Name durable objects, spatial relations, and visible state. \
Do not infer robot commands, safety outcomes, or predicate truth.";

/// Preprocessing policy label hashed for provenance.
pub const DESCRIPTION_PREPROCESSING_LABEL: &str =
    "semantic-indexing-preprocess-v1-roi-or-full-jpeg";

/// Stable digest for [`DESCRIPTION_PROMPT`].
#[must_use]
pub fn description_prompt_digest() -> Sha256Digest {
    Sha256Digest::hash_bytes(DESCRIPTION_PROMPT.as_bytes())
}

/// Preprocessing policy: oriented full frame or configured ROI JPEG passed to Luna.
#[must_use]
pub fn description_preprocessing_digest() -> Sha256Digest {
    Sha256Digest::hash_bytes(DESCRIPTION_PREPROCESSING_LABEL.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_digests_track_prompt_text() {
        assert_eq!(
            description_prompt_digest(),
            Sha256Digest::hash_bytes(DESCRIPTION_PROMPT.as_bytes())
        );
        assert_eq!(
            description_preprocessing_digest(),
            Sha256Digest::hash_bytes(DESCRIPTION_PREPROCESSING_LABEL.as_bytes())
        );
    }
}
