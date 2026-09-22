use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{EncodeBackend, EncodeRequest, VideoEncoder};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EncoderParameterQuery {
    pub encoder: VideoEncoder,
    pub backend: EncodeBackend,
}

/// Validated encoder overrides. Names are catalog identifiers, never raw flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EncoderParameter {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EncoderParameterSpec {
    pub name: String,
    pub label: String,
    pub argument: String,
    pub minimum: u16,
    pub maximum: u16,
    /// Whole, decimal, pairWhole, pairDecimal, choice, or choiceList.
    #[serde(default)]
    pub value_kind: String,
    #[serde(default)]
    pub minimum_value: String,
    #[serde(default)]
    pub maximum_value: String,
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub example: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EncoderParameterCatalog {
    pub encoder: VideoEncoder,
    pub backend: EncodeBackend,
    pub route: String,
    pub tool_path: String,
    pub tool_version: String,
    pub parameters: Vec<EncoderParameterSpec>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EncodeCommandStage {
    pub label: String,
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EncodeCommandPlan {
    pub request: EncodeRequest,
    pub source_fingerprint: String,
    pub output_frame_count: String,
    pub output_frame_rate: String,
    pub stages: Vec<EncodeCommandStage>,
    pub notes: Vec<String>,
}

/// A reusable validated override list, scoped to an encoder and execution route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncoderParameterPreset {
    pub name: String,
    pub encoder: VideoEncoder,
    pub backend: EncodeBackend,
    pub parameters: Vec<EncoderParameter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncoderParameterPresetKey {
    pub name: String,
    pub encoder: VideoEncoder,
    pub backend: EncodeBackend,
}
