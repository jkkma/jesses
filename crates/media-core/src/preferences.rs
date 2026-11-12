use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
    /// Thorough verification. Off (default), an encode checks the finished
    /// file's frame and stream counts. AV1AN can resume saved chunks and still
    /// checks timing or retained chunks where needed. On, the source and tools
    /// are fingerprinted for resume, every output frame is decoded and every
    /// track is re-checked.
    #[ts(optional, as = "Option<_>")]
    pub full_output_verification: bool,
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
            full_output_verification: false,
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
    /// Absent in plans saved before this option: those run without thorough
    /// verification.
    #[serde(default)]
    #[ts(optional, as = "Option<_>")]
    pub full_output_verification: bool,
}

impl From<&GeneralPreferences> for EncodingPreferences {
    fn from(value: &GeneralPreferences) -> Self {
        Self {
            keyframe_interval_seconds: value.keyframe_interval_seconds,
            mp4_faststart: value.mp4_faststart,
            full_output_verification: value.full_output_verification,
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
