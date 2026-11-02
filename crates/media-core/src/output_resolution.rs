use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OutputTarget {
    Container { format: crate::ContainerFormat },
    Image { format: crate::ImageOutput },
}

/// A read-only naming suggestion. Execution still reserves/publishes atomically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputPathRequest {
    pub input_path: String,
    /// Literal extensionless output-box value; an embedded extension is retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub output_stem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_output_directory: Option<String>,
    pub target: OutputTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OutputPathResolution {
    pub output_stem: String,
    pub output_path: String,
    pub is_directory: bool,
    pub numbered: bool,
}
