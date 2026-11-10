//! Redacted credential references for semantic indexing adapters.

use a2a_lab_dev_kit::CredentialReference;

/// Returns a redacted reference suitable for logs and errors (never secret values).
#[must_use]
pub fn redacted_credential_env(reference: &CredentialReference) -> String {
    format!("env:{}", reference.env_var())
}

/// Whether the referenced environment variable is set (values are not read into errors).
#[must_use]
pub fn credential_env_configured(reference: &CredentialReference) -> bool {
    std::env::var(reference.env_var()).is_ok_and(|value| !value.trim().is_empty())
}
