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
