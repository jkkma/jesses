use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ToneMapAlgorithm {
    #[default]
    Hable,
    Mobius,
}

/// Explicit HDR-to-SDR rendering. Omission preserves the original color workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToneMapSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub algorithm: Option<ToneMapAlgorithm>,
    /// Signal peak relative to the fixed 100-nit SDR target.
    pub source_peak_nits: u16,
    /// Discard dynamic HDR only for the already-qualified HDR10 base-layer profiles.
    #[serde(default)]
    pub hdr10_base_layer: bool,
}
