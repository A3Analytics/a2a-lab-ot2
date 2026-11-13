//! Amazon Bedrock Cohere embed models (`cohere.embed-english-v3`).

use a2a_lab_dev_kit::videos::SemanticIndexingFailure;
use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::primitives::Blob;
use serde::{Deserialize, Serialize};

/// Default retrieval embedding model for OT-2 semantic indexing.
pub const DEFAULT_COHERE_EMBED_MODEL: &str = "cohere.embed-english-v3";

/// Required model-space dimension count for [`DEFAULT_COHERE_EMBED_MODEL`].
pub const DEFAULT_COHERE_EMBED_DIMENSIONS: u32 = 1024;

#[derive(Serialize)]
struct CohereEmbedRequest<'a> {
    texts: &'a [&'a str],
    input_type: &'a str,
    embedding_types: &'static [&'static str],
}

#[derive(Deserialize)]
struct CohereEmbedResponse {
    embeddings: Vec<CohereEmbeddingBlock>,
}

#[derive(Deserialize)]
struct CohereEmbeddingBlock {
    #[serde(default)]
    float: Vec<Vec<f32>>,
}

/// Embeds one batch of texts through Bedrock Cohere embed.
pub async fn cohere_embed_texts(
    client: &Client,
    model_id: &str,
    expected_dims: usize,
    texts: &[&str],
    input_type: &str,
) -> Result<Vec<Vec<f32>>, SemanticIndexingFailure> {
    if texts.is_empty() {
        return Err(SemanticIndexingFailure::PartialResponse {
            message: "empty Cohere embed batch".to_owned(),
        });
    }
    let body = serde_json::to_vec(&CohereEmbedRequest {
        texts,
        input_type,
        embedding_types: &["float"],
    })
    .map_err(|error| SemanticIndexingFailure::ProviderUnavailable {
        message: format!("Cohere request encode failed: {error}"),
    })?;
    let response = client
        .invoke_model()
        .model_id(model_id)
        .content_type("application/json")
        .accept("application/json")
        .body(Blob::new(body))
        .send()
        .await
        .map_err(|error| super::bedrock::map_bedrock_invoke_failure(&error.to_string()))?;
    let parsed: CohereEmbedResponse =
        serde_json::from_slice(response.body().as_ref()).map_err(|error| {
            SemanticIndexingFailure::PartialResponse {
                message: format!("Cohere response decode failed: {error}"),
            }
        })?;
    let Some(block) = parsed.embeddings.first() else {
        return Err(SemanticIndexingFailure::PartialResponse {
            message: "Cohere response missing embeddings".to_owned(),
        });
    };
    if block.float.len() != texts.len() {
        return Err(SemanticIndexingFailure::PartialResponse {
            message: format!(
                "Cohere returned {} vectors for {} inputs",
                block.float.len(),
                texts.len()
            ),
        });
    }
    for vector in &block.float {
        if vector.len() != expected_dims {
            return Err(SemanticIndexingFailure::InvalidVector {
                message: format!(
                    "Cohere vector length {} does not match configured space {}",
                    vector.len(),
                    expected_dims
                ),
            });
        }
        if vector.iter().copied().any(|value| !value.is_finite()) {
            return Err(SemanticIndexingFailure::InvalidVector {
                message: "non-finite Cohere embedding component".to_owned(),
            });
        }
    }
    Ok(block.float.clone())
}

#[must_use]
pub fn cohere_bedrock_client(config: &aws_config::SdkConfig) -> Client {
    Client::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_constants_match_v3_english() {
        assert_eq!(DEFAULT_COHERE_EMBED_MODEL, "cohere.embed-english-v3");
        assert_eq!(DEFAULT_COHERE_EMBED_DIMENSIONS, 1024);
    }
}
