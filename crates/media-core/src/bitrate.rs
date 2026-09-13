use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BitrateRequest {
    pub input_path: String,
    pub stream_index: u32,
    /// Fixed, presentation-time-aligned windows; packets are assigned by PTS.
    pub window_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BitratePoint {
    pub start_seconds: f64,
    pub megabits_per_second: f64,
    pub packet_bytes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BitrateResult {
    pub stream_index: u32,
    pub window_seconds: f64,
    /// Compressed packet payload only; excludes container overhead.
    pub packet_bytes: String,
    pub packet_count: String,
    pub untimed_packet_bytes: String,
    pub untimed_packet_count: String,
    pub dts_fallback_count: String,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    /// Timed payload divided by measured timestamp span, when known.
    pub average_megabits_per_second: Option<f64>,
    pub peak_window_megabits_per_second: f64,
    pub points: Vec<BitratePoint>,
    pub source_fingerprint: String,
}
