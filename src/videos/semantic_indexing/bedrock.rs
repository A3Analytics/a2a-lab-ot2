//! Live Bedrock opt-in and startup prerequisites for semantic indexing.

use std::time::Duration;

use a2a_lab_dev_kit::videos::{
    FrameDescriptionBackend, FrameDescriptionProviderConfig, SemanticIndexingFailure,
    TextEmbeddingProviderConfig,
};
use aws_credential_types::provider::ProvideCredentials;

const CREDENTIAL_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether live Luna and text-embedding adapters are selected for indexing and search.
#[must_use]
pub fn live_semantic_indexing_enabled() -> bool {
    std::env::var("A2ALAB_SEMANTIC_INDEXING_LIVE")
        .is_ok_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
}

/// AWS profile used for Bedrock (`AWS_PROFILE`, else `default`).
#[must_use]
pub fn bedrock_profile() -> String {
    std::env::var("AWS_PROFILE")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "default".to_owned())
}

/// AWS region override from `AWS_REGION` or `AWS_DEFAULT_REGION`.
#[must_use]
pub fn bedrock_region_override() -> Option<String> {
    std::env::var("AWS_REGION")
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Loads AWS SDK configuration for Bedrock using the standard credential chain.
pub async fn load_bedrock_sdk_config() -> Result<aws_config::SdkConfig, SemanticIndexingFailure> {
    let profile = bedrock_profile();
    let mut loader =
        aws_config::defaults(aws_config::BehaviorVersion::latest()).profile_name(&profile);
    if let Some(region) = bedrock_region_override() {
        loader = loader.region(aws_config::Region::new(region));
    }
    let config = loader.load().await;
    let region = resolved_region(&config);
    if region.is_none() {
        return Err(missing_region_failure(&profile));
    }
    let Some(provider) = config.credentials_provider() else {
        return Err(credential_chain_failure(
            &profile,
            "no credential provider in AWS SDK chain",
        ));
    };
    match tokio::time::timeout(CREDENTIAL_TIMEOUT, provider.provide_credentials()).await {
        Ok(Ok(credentials)) => {
            let _ = credentials;
            Ok(config)
        }
        Ok(Err(error)) => Err(credential_chain_failure(&profile, &error.to_string())),
        Err(_) => Err(credential_chain_failure(
            &profile,
            "credential provider timed out",
        )),
    }
}

/// Validates configured Bedrock models/spaces and AWS prerequisites for live adapters.
pub async fn ensure_live_bedrock_ready(
    description: &FrameDescriptionProviderConfig,
    embedding: &TextEmbeddingProviderConfig,
) -> Result<(), SemanticIndexingFailure> {
    validate_bedrock_configuration(description, embedding)?;
    if uses_bedrock_runtime(description, embedding) {
        load_bedrock_sdk_config().await?;
    }
    Ok(())
}

/// Validates Bedrock description settings and AWS prerequisites when embeddings are disabled.
pub async fn ensure_live_bedrock_description(
    description: &FrameDescriptionProviderConfig,
) -> Result<(), SemanticIndexingFailure> {
    if description.backend() == FrameDescriptionBackend::Bedrock
        && description.model().trim().is_empty()
    {
        return Err(description_model_failure());
    }
    if description.backend() == FrameDescriptionBackend::Bedrock {
        load_bedrock_sdk_config().await?;
    }
    Ok(())
}

fn uses_bedrock_runtime(
    description: &FrameDescriptionProviderConfig,
    embedding: &TextEmbeddingProviderConfig,
) -> bool {
    description.backend() == FrameDescriptionBackend::Bedrock
        || embedding.provider().eq_ignore_ascii_case("bedrock")
}

fn validate_bedrock_configuration(
    description: &FrameDescriptionProviderConfig,
    embedding: &TextEmbeddingProviderConfig,
) -> Result<(), SemanticIndexingFailure> {
    if description.backend() == FrameDescriptionBackend::Bedrock
        && description.model().trim().is_empty()
    {
        return Err(description_model_failure());
    }
    if embedding.provider().eq_ignore_ascii_case("bedrock") {
        if embedding.model().trim().is_empty() {
            return Err(embedding_model_failure());
        }
        if embedding.model_space().dimensions() == 0 {
            return Err(embedding_space_failure());
        }
    }
    Ok(())
}

fn resolved_region(config: &aws_config::SdkConfig) -> Option<String> {
    bedrock_region_override().or_else(|| config.region().map(|region| region.as_ref().to_owned()))
}

/// Maps Bedrock invoke failures into actionable operator guidance without secret values.
#[must_use]
pub fn map_bedrock_invoke_failure(error: &str) -> SemanticIndexingFailure {
    let lowered = error.to_ascii_lowercase();
    if lowered.contains("accessdenied")
        || lowered.contains("access denied")
        || lowered.contains("unauthorized")
        || lowered.contains("not authorized")
    {
        return SemanticIndexingFailure::ProviderUnavailable {
            message: format!(
                "Bedrock model access or authorization failed: {error}. Confirm the configured description and embedding model ids are enabled in this AWS account and region, and that the active IAM principal can invoke bedrock:InvokeModel."
            ),
        };
    }
    SemanticIndexingFailure::ProviderUnavailable {
        message: format!("Bedrock request failed: {error}"),
    }
}

fn missing_region_failure(profile: &str) -> SemanticIndexingFailure {
    SemanticIndexingFailure::ProviderUnavailable {
        message: format!(
            "AWS region is not configured for live Bedrock semantic indexing (profile {profile}). Set AWS_REGION or AWS_DEFAULT_REGION, or add region to the [{profile}] profile in your AWS config file."
        ),
    }
}

fn credential_chain_failure(profile: &str, detail: &str) -> SemanticIndexingFailure {
    SemanticIndexingFailure::ProviderUnavailable {
        message: format!(
            "AWS credentials are unavailable for live Bedrock semantic indexing (profile {profile}): {detail}. Configure AWS_PROFILE and a resolvable credential source (shared config, environment variables, or credential process supported by the AWS SDK); repository configuration does not store secrets."
        ),
    }
}

fn description_model_failure() -> SemanticIndexingFailure {
    SemanticIndexingFailure::ProviderUnavailable {
        message:
            "configured Bedrock description model is missing or empty; set semantic_indexing.description.model to a Bedrock model id that supports image input in this account"
                .to_owned(),
    }
}

fn embedding_model_failure() -> SemanticIndexingFailure {
    SemanticIndexingFailure::ProviderUnavailable {
        message:
            "configured Bedrock text embedding model is missing or empty; set semantic_indexing.embeddings.model to a Bedrock embedding model id enabled in this account"
                .to_owned(),
    }
}

fn embedding_space_failure() -> SemanticIndexingFailure {
    SemanticIndexingFailure::ProviderUnavailable {
        message:
            "configured Bedrock embedding vector space has invalid dimensions; set semantic_indexing.embeddings.space.dimensions to a positive value compatible with the selected embedding model"
                .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use a2a_lab_dev_kit::{
        EmbeddingMetric, ModelSpaceIdentity, ProvenanceGeneration, VectorNormalization,
    };

    use super::*;

    fn bedrock_embedding_config(model: &str, dimensions: u32) -> TextEmbeddingProviderConfig {
        let space = ModelSpaceIdentity::new(
            "bedrock",
            model,
            dimensions,
            VectorNormalization::L2Unit,
            EmbeddingMetric::Cosine,
            ProvenanceGeneration::new(1).unwrap(),
        )
        .unwrap();
        TextEmbeddingProviderConfig::new("bedrock", model, space).unwrap()
    }

    fn bedrock_description_config(model: &str) -> FrameDescriptionProviderConfig {
        FrameDescriptionProviderConfig::new(
            FrameDescriptionBackend::Bedrock,
            Some(model),
            a2a_lab_dev_kit::Sha256Digest::hash_bytes(b"prompt"),
            a2a_lab_dev_kit::Sha256Digest::hash_bytes(b"preprocess"),
        )
        .unwrap()
    }

    #[test]
    fn live_opt_in_accepts_common_truthy_values() {
        for value in ["1", "true", "yes", " yes "] {
            unsafe { std::env::set_var("A2ALAB_SEMANTIC_INDEXING_LIVE", value) };
            assert!(live_semantic_indexing_enabled(), "value={value}");
        }
        unsafe { std::env::remove_var("A2ALAB_SEMANTIC_INDEXING_LIVE") };
        assert!(!live_semantic_indexing_enabled());
        unsafe { std::env::set_var("A2ALAB_SEMANTIC_INDEXING_LIVE", "false") };
        assert!(!live_semantic_indexing_enabled());
        unsafe { std::env::remove_var("A2ALAB_SEMANTIC_INDEXING_LIVE") };
    }

    #[test]
    fn bedrock_profile_and_region_follow_env_precedence() {
        unsafe {
            std::env::set_var("AWS_PROFILE", " semantic ");
            std::env::set_var("AWS_REGION", " us-west-2 ");
        }
        assert_eq!(bedrock_profile(), "semantic");
        assert_eq!(bedrock_region_override(), Some("us-west-2".to_owned()));
        unsafe {
            std::env::remove_var("AWS_REGION");
            std::env::set_var("AWS_DEFAULT_REGION", "eu-central-1");
        }
        assert_eq!(bedrock_region_override(), Some("eu-central-1".to_owned()));
        unsafe {
            std::env::remove_var("AWS_PROFILE");
            std::env::remove_var("AWS_DEFAULT_REGION");
        }
        assert_eq!(bedrock_profile(), "default");
        assert_eq!(bedrock_region_override(), None);
    }

    #[test]
    fn configuration_validation_accepts_resolved_bedrock_pair() {
        let description = bedrock_description_config("global.openai.gpt-5.6-luna");
        let embedding = bedrock_embedding_config("amazon.titan-embed-text-v2:0", 1024);
        validate_bedrock_configuration(&description, &embedding).expect("valid");
    }

    #[test]
    fn embedding_diagnostics_name_remedies() {
        assert!(
            embedding_model_failure()
                .to_string()
                .contains("embedding model")
        );
        assert!(embedding_space_failure().to_string().contains("dimensions"));
        assert!(
            description_model_failure()
                .to_string()
                .contains("description model")
        );
    }

    #[test]
    fn invoke_access_mapping_is_actionable_and_redacts_secrets() {
        let mapped = map_bedrock_invoke_failure(
            "AccessDeniedException: not authorized to invoke arn:aws:bedrock:us-east-1::foundation-model/global.openai.gpt-5.6-luna",
        );
        assert!(matches!(
            mapped,
            SemanticIndexingFailure::ProviderUnavailable { message }
                if message.contains("model access")
                    && message.contains("bedrock:InvokeModel")
                    && !message.contains("secret")
        ));
    }

    #[test]
    fn missing_region_and_credentials_messages_name_remedies() {
        let region = missing_region_failure("lab");
        assert!(region.to_string().contains("AWS_REGION"));
        let creds = credential_chain_failure("lab", "no credentials");
        let text = creds.to_string();
        assert!(text.contains("AWS_PROFILE"));
        assert!(!text.contains("AKIA"));
    }
}
