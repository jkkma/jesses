use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::CropSettings;

/// Read-only inspection of one source video stream. Preview seeking does not
/// trim an encode, and the image is the source before crop, resize or borders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FramePreviewRequest {
    pub input_path: String,
    pub video_stream_index: u32,
    pub position_seconds: f64,
    /// Inspector images may honor rotation and pixel aspect ratio. Crop previews
    /// retain coded coordinates when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub display_orientation: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FramePreviewResult {
    /// Bounded PNG returned directly; no source path is exposed as a web asset.
    pub image_data_url: String,
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
    /// Requested seek position, not a claim of frame-accurate trim timing.
    pub position_seconds: f64,
    /// Metadata and sampled-content identity, not a complete-file SHA-256.
    pub source_fingerprint: String,
    /// HDR was converted to SDR only for this display image.
    pub tone_mapped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AutoCropRequest {
    pub input_path: String,
    pub video_stream_index: u32,
}

/// A proposal only. Applying it is an explicit draft edit and must invalidate
/// any batch preview; queued requests are never modified by analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AutoCropResult {
    /// None means that the samples did not establish a usable crop.
    pub crop: Option<CropSettings>,
    pub source_width: u32,
    pub source_height: u32,
    pub sample_count: u32,
    pub sampled_frames: u32,
    pub agreement_percent: u8,
    pub source_fingerprint: String,
    pub message: String,
}

/// Read-only keyframe lookup at or before the requested source position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeyframeSnapRequest {
    pub input_path: String,
    pub video_stream_index: u32,
    pub position_seconds: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeyframeSnapResult {
    pub position_seconds: Option<f64>,
    pub source_fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FieldDetectionRequest {
    pub input_path: String,
    pub video_stream_index: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DetectedFieldOrder {
    Progressive,
    TopFirst,
    BottomFirst,
    Undetermined,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FieldDetectionResult {
    pub order: DetectedFieldOrder,
    pub telecined: bool,
    pub sampled_frames: u32,
    pub top_first_frames: u32,
    pub bottom_first_frames: u32,
    pub progressive_frames: u32,
    pub undetermined_frames: u32,
    pub field_gap_ratio: Option<f64>,
    pub source_fingerprint: String,
    pub evidence: String,
}
