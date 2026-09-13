use crate::{AppError, EncodeRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BatchEncodeRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<crate::EncoderParameter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub av1an_options: Option<crate::Av1anOptions>,
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
    pub output_directory: String,
    pub crf: u8,
    pub preset: u8,
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
