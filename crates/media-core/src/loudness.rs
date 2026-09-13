use crate::AudioChannels;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessRequest {
    pub input_path: String,
    pub stream_index: u32,
    pub channels: AudioChannels,
    pub target_lufs: f64,
    pub peak_limit_dbfs: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessResult {
    pub integrated_lufs: Option<f64>,
    pub true_peak_dbfs: Option<f64>,
    pub loudness_range_lu: Option<f64>,
    /// Flat gain only, rounded down to tenths of a decibel to retain headroom.
    pub suggested_gain_tenths_db: Option<i16>,
    pub target_limited_by_peak: bool,
    pub source_fingerprint: String,
    pub message: String,
}
