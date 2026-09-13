use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Explicit HDR-to-SDR rendering. Omission preserves the original color workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToneMapSettings {
    /// Signal peak used by Hable, relative to the fixed 100-nit SDR target.
    pub source_peak_nits: u16,
    /// Discard dynamic HDR only for the already-qualified HDR10 base-layer profiles.
    #[serde(default)]
    pub hdr10_base_layer: bool,
}
