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

/// The source encode dropdown targets; Off is represented by omission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LoudnessTarget {
    Lufs14,
    Lufs16,
    Lufs23,
}

impl LoudnessTarget {
    pub fn lufs(self) -> i16 {
        match self {
            Self::Lufs14 => -14,
            Self::Lufs16 => -16,
            Self::Lufs23 => -23,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioNormalization {
    pub target: LoudnessTarget,
}
