//! Serde shapes for `POST /v1/systemone` (subset used by the lab adapter).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use a2a_lab_dev_kit::DEFAULT_JEV_MODEL_REVISION;

/// Pinned production model revision.
pub const PINNED_JEV_MODEL: &str = DEFAULT_JEV_MODEL_REVISION;

#[derive(Debug, Clone, Serialize)]
pub struct SystemOneRequest {
    pub model: &'static str,
    pub state: Map<String, Value>,
    pub assets: BTreeMap<String, WireAsset>,
    pub questions: BTreeMap<String, WireQuestion>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WireAsset {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub format: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WireQuestion {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criteria: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SystemOneResponse {
    pub model: String,
    pub answers: BTreeMap<String, WireAnswer>,
    pub usage: Option<WireUsage>,
    #[serde(default)]
    pub quotas: Option<WireQuotas>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireQuotas {
    #[serde(default)]
    pub requests_remaining: Option<u64>,
    #[serde(default)]
    pub tokens_remaining: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireAnswer {
    Noul {
        noul: f32,
    },
    Choice {
        choice: String,
        confidence: f32,
        probabilities: BTreeMap<String, f32>,
    },
    Score {
        score: f32,
        confidence: f32,
        probabilities: BTreeMap<String, f32>,
        legend: BTreeMap<String, String>,
    },
    Box {
        confidence: f32,
        #[serde(rename = "box")]
        bounds: WireBox,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct WireBox {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
}

impl WireBox {
    #[must_use]
    pub const fn to_roi(self) -> [f32; 4] {
        [self.left, self.top, self.right, self.bottom]
    }
}
