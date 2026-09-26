use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum QualityMetric {
    Psnr,
    Ssim,
    Vmaf,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum QualityAlignment {
    #[default]
    None,
    CropReference,
    ResizeReference,
    CropAndResizeReference,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum QualityVmafModel {
    #[default]
    Standard,
    Negative,
    FourK,
}

/// Optional controls preserve the exact, unsampled comparison of older requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QualityOptions {
    #[serde(default)]
    pub alignment: QualityAlignment,
    #[serde(default)]
    pub vmaf_model: QualityVmafModel,
    /// One scores every selected frame; N scores frames 0, N, 2N, ... on both inputs.
    #[serde(default = "default_subsample")]
    pub subsample: u32,
    /// Pair selected frames in ordinal order even when container timestamps disagree.
    #[serde(default)]
    pub fix_frame_rate: bool,
}

fn default_subsample() -> u32 {
    1
}

impl Default for QualityOptions {
    fn default() -> Self {
        Self {
            alignment: QualityAlignment::None,
            vmaf_model: QualityVmafModel::Standard,
            subsample: 1,
            fix_frame_rate: false,
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub options: Option<QualityOptions>,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_comparison_json_retains_exact_unsampled_defaults() {
        let request: QualityRequest = serde_json::from_value(serde_json::json!({
            "referencePath":"/reference.mkv", "referenceStreamIndex":0, "referenceStartFrame":0,
            "candidatePath":"/candidate.mkv", "candidateStreamIndex":0, "candidateStartFrame":0,
            "frameCount":24, "metric":"ssim"
        }))
        .unwrap();
        assert_eq!(request.options, None);
        assert_eq!(QualityOptions::default().subsample, 1);
        assert!(!QualityOptions::default().fix_frame_rate);
        let explicit: QualityOptions = serde_json::from_value(serde_json::json!({
            "alignment":"cropAndResizeReference", "vmafModel":"fourK"
        }))
        .unwrap();
        assert_eq!(explicit.subsample, 1);
    }
}
