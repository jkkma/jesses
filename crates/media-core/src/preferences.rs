use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeneralPreferences {
    pub default_output_directory: String,
    pub recursive_import: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserPreferences {
    pub general: GeneralPreferences,
    pub recent_paths: Vec<String>,
    pub revision: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameter_presets: Vec<crate::EncoderParameterPreset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavePreferencesRequest {
    pub general: GeneralPreferences,
    /// Omitted for ordinary edits, so a recent import cannot be lost.
    pub recent_paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PreferenceImportPreview {
    pub request: SavePreferencesRequest,
    pub accepted_keys: Vec<String>,
    pub ignored_key_count: u32,
    pub warnings: Vec<String>,
}
