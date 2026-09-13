use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum QualityMetric {
    Psnr,
    Ssim,
    Vmaf,
}

/// Compare explicit corresponding decoded frame intervals; never infer alignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QualityRequest {
    pub reference_path: String,
    pub reference_stream_index: u32,
    pub reference_start_frame: u32,
    pub candidate_path: String,
    pub candidate_stream_index: u32,
    pub candidate_start_frame: u32,
    pub frame_count: u32,
    pub metric: QualityMetric,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QualityPoint {
    pub frame: u32,
    /// None represents infinite PSNR for identical decoded pixels.
    pub score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QualityResult {
    pub metric: QualityMetric,
    pub frame_count: u32,
    pub score: Option<f64>,
    pub points: Vec<QualityPoint>,
    pub reference_fingerprint: String,
    pub candidate_fingerprint: String,
    pub model: Option<String>,
    pub message: String,
}
