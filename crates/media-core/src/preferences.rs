use serde::{Deserialize, Serialize};
use ts_rs::TS;

fn discard_retired_verification<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<(), D::Error> {
    serde::de::IgnoredAny::deserialize(deserializer).map(|_| ())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ConsoleWindowMode {
    #[default]
    Hidden,
    Visible,
    VisibleStayOpen,
}

/// Source application's eight reset categories. Defaults apply only to absent
/// fields, so explicitly disabled categories survive a save/reload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetOnNewFilePreferences {
    pub trim: bool,
    pub frame_rate: bool,
    pub resize: bool,
    pub crop: bool,
    pub borders: bool,
    pub deinterlace: bool,
    pub tone_map: bool,
    pub custom_filters: bool,
}

impl Default for ResetOnNewFilePreferences {
    fn default() -> Self {
        Self {
            trim: true,
            frame_rate: false,
            resize: false,
            crop: true,
            borders: false,
            deinterlace: true,
            tone_map: true,
            custom_filters: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct GeneralPreferences {
    pub default_output_directory: String,
    pub recursive_import: bool,
    #[ts(optional, as = "Option<_>")]
    pub console_window: ConsoleWindowMode,
    #[ts(optional, as = "Option<_>")]
    pub auto_crop_samples: u8,
    #[ts(optional, as = "Option<_>")]
    pub keyframe_interval_seconds: u16,
    #[ts(optional, as = "Option<_>")]
    pub zero_indexed_streams: bool,
    #[ts(optional, as = "Option<_>")]
    pub mp4_faststart: bool,
    // Accept the retired setting in saved preferences without retaining its value.
    #[doc(hidden)]
    #[serde(
        rename = "fullOutputVerification",
        skip_serializing,
        deserialize_with = "discard_retired_verification"
    )]
    #[ts(skip)]
    pub retired_verification: (),
    #[ts(optional, as = "Option<_>")]
    pub av1an_console_visible: bool,
    #[ts(optional, as = "Option<_>")]
    pub reset_on_new_file: ResetOnNewFilePreferences,
}

impl Default for GeneralPreferences {
    fn default() -> Self {
        Self {
            default_output_directory: String::new(),
            recursive_import: false,
            console_window: ConsoleWindowMode::Hidden,
            auto_crop_samples: 10,
            keyframe_interval_seconds: 10,
            zero_indexed_streams: false,
            mp4_faststart: true,
            retired_verification: (),
            av1an_console_visible: true,
            reset_on_new_file: ResetOnNewFilePreferences::default(),
        }
    }
}

/// Immutable copy of global encoding settings, captured when a plan is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncodingPreferences {
    pub keyframe_interval_seconds: u16,
    pub mp4_faststart: bool,
    // Older saved jobs and recovery manifests include the retired setting.
    #[doc(hidden)]
    #[serde(
        default,
        rename = "fullOutputVerification",
        skip_serializing,
        deserialize_with = "discard_retired_verification"
    )]
    #[ts(skip)]
    pub retired_verification: (),
}

impl From<&GeneralPreferences> for EncodingPreferences {
    fn from(value: &GeneralPreferences) -> Self {
        Self {
            keyframe_interval_seconds: value.keyframe_interval_seconds,
            mp4_faststart: value.mp4_faststart,
            retired_verification: (),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserPreferences {
    pub general: GeneralPreferences,
    pub recent_paths: Vec<String>,
    pub revision: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameter_presets: Vec<crate::EncoderParameterPreset>,
    /// How this session's preferences were recovered from a damaged or newer
    /// file. Reported by reads only and never written to disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub notice: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_retired_verification_from_saved_preferences_and_jobs() {
        let general: GeneralPreferences = serde_json::from_value(serde_json::json!({
            "fullOutputVerification": true,
            "mp4Faststart": false,
        }))
        .unwrap();
        assert!(!general.mp4_faststart);
        let value = serde_json::to_value(&general).unwrap();
        assert!(value.get("fullOutputVerification").is_none());

        let settings: EncodingPreferences = serde_json::from_value(serde_json::json!({
            "keyframeIntervalSeconds": 3,
            "mp4Faststart": false,
            "fullOutputVerification": true,
        }))
        .unwrap();
        assert_eq!(settings.keyframe_interval_seconds, 3);
        assert!(!settings.mp4_faststart);
        let value = serde_json::to_value(settings).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"keyframeIntervalSeconds": 3, "mp4Faststart": false})
        );
    }
}
