use crate::{AppError, EncodeRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The source UI's displayed values, frozen with the batch settings. These are
/// filename text: quality and CRF are aliases even for bitrate/size targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchNamingValues {
    pub codec: String,
    pub quality: String,
    pub preset: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchOutputPathInput {
    pub input_path: String,
    /// Dimensions of the source's first video track, as loaded by the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub height: Option<u32>,
}

/// Read-only names for a frozen batch. Destinations are reserved within this
/// result; execution still checks filesystem collisions before publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchOutputPathRequest {
    pub inputs: Vec<BatchOutputPathInput>,
    pub target: crate::OutputTarget,
    pub naming: BatchNamingValues,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub output_name_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub naming_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_output_directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BatchOutputPathItem {
    pub input_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resolution: Option<crate::OutputPathResolution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<AppError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BatchOutputPaths {
    pub items: Vec<BatchOutputPathItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FolderScanRequest {
    pub path: String,
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FolderScanResult {
    pub paths: Vec<String>,
    pub errors: Vec<AppError>,
    pub skipped_count: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BatchEncodeInput {
    /// Quick's per-file mapped video presence; omitted keeps the batch mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub video_mode: Option<crate::VideoMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub temporal: Option<crate::TemporalSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tone_map: Option<crate::ToneMapSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub trim: Option<crate::VideoTrim>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<crate::SubtitleTrackSettings>,
    #[serde(default)]
    pub framing: crate::VideoFraming,
    #[serde(default)]
    pub audio: Vec<crate::AudioTrackSettings>,
    pub input_path: String,
    pub stream_indices: Vec<u32>,
    pub video_stream_index: u32,
}

/// Batch previews accept only encode fields; unrelated operation fields must
/// not be silently discarded and interpreted as a different task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchEncodeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub copy_metadata: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub copy_chapters: Option<bool>,
    /// Source-compatible filename values. Omission retains legacy API naming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_naming: Option<BatchNamingValues>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub encoding_preferences: Option<crate::EncodingPreferences>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hdr_metadata: Option<crate::HdrMetadataSettings>,
    #[serde(default)]
    #[ts(optional, as = "Option<_>")]
    pub video_mode: crate::VideoMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<crate::EncoderParameter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub av1an_options: Option<crate::Av1anOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub output_pixel_format: Option<crate::Av1anPixelFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub av1an_grain: Option<crate::Av1anGrainSettings>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(optional, as = "Option<_>")]
    pub av1an_filters: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(optional, as = "Option<_>")]
    pub source_filters: bool,
    #[serde(default)]
    #[ts(optional)]
    pub output_container: Option<crate::ContainerFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rate_control: Option<crate::VideoRateControl>,
    #[serde(default)]
    pub backend: crate::EncodeBackend,
    #[serde(default)]
    pub encoder: crate::VideoEncoder,
    #[serde(default = "crate::jobs::default_workers")]
    pub workers: u8,
    pub inputs: Vec<BatchEncodeInput>,
    /// With sourceNaming this is the configured default folder: empty/missing
    /// directories fall back beside each original source. Legacy calls require
    /// one existing output folder for the whole batch.
    pub output_directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub output_name_template: Option<String>,
    /// Local calendar date captured once when the batch preview is requested.
    /// It is required only when the filename template contains `{date}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub naming_date: Option<String>,
    /// Compatibility-only legacy quantizer field; hardware encoding is unsupported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub constant_quantizer: Option<u8>,
    /// Compatibility-only marker for retired source hardware-encoder jobs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(optional, as = "Option<_>")]
    pub source_nvenc_quality: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(optional, as = "Option<_>")]
    pub source_trim: bool,
    pub crf: u8,
    pub preset: u8,
    #[serde(default)]
    pub lossless: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub svt_crf_quarter_steps: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub svt_preset: Option<i8>,
    #[serde(default)]
    pub film_grain: u8,
    #[serde(default)]
    pub lineart_psy_bias: u8,
    #[serde(default)]
    pub texture_psy_bias: u8,
    #[serde(default)]
    pub hdr_tune: crate::HdrTune,
    #[serde(default)]
    pub hdr10_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BatchEncodeItem {
    pub input_path: String,
    pub output_path: Option<String>,
    pub request: Option<EncodeRequest>,
    pub error: Option<AppError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BatchEncodePreview {
    pub items: Vec<BatchEncodeItem>,
}
