//! Pairs description and embedding adapters without decision authority.

/// Luna description plus text-embedding clients wired for one worker pass.
#[derive(Debug, Clone)]
pub struct SemanticIndexingProviders<D, E> {
    pub description: D,
    pub embedding: E,
}

impl<D, E> SemanticIndexingProviders<D, E> {
    /// Wires the two retrieval-only provider boundaries.
    #[must_use]
    pub const fn new(description: D, embedding: E) -> Self {
        Self {
            description,
            embedding,
        }
    }
}
