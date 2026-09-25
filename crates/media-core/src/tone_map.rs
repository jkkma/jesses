use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ToneMapAlgorithm {
    #[default]
    Hable,
    Mobius,
    Reinhard,
    Spline,
}

/// Omitted in older jobs: their original CPU filter remains authoritative.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ToneMapBackend {
    #[default]
    Cpu,
    Auto,
    Gpu,
}

/// The old explicit peak is retained unless the caller requests sampling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ToneMapPeakMode {
    #[default]
    Manual,
    Measured,
}

fn cpu_backend(value: &ToneMapBackend) -> bool {
    *value == ToneMapBackend::Cpu
}

fn manual_peak(value: &ToneMapPeakMode) -> bool {
    *value == ToneMapPeakMode::Manual
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
    #[serde(default, skip_serializing_if = "cpu_backend")]
    #[ts(optional, as = "Option<_>")]
    pub backend: ToneMapBackend,
    #[serde(default, skip_serializing_if = "manual_peak")]
    #[ts(optional, as = "Option<_>")]
    pub peak_mode: ToneMapPeakMode,
    /// Discard dynamic HDR only for the already-qualified HDR10 base-layer profiles.
    #[serde(default)]
    pub hdr10_base_layer: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_manual_cpu_settings_remain_identical_on_the_wire() {
        let settings: ToneMapSettings = serde_json::from_str(
            r#"{"algorithm":"hable","sourcePeakNits":1000,"hdr10BaseLayer":false}"#,
        )
        .unwrap();
        assert_eq!(settings.backend, ToneMapBackend::Cpu);
        assert_eq!(settings.peak_mode, ToneMapPeakMode::Manual);
        let value = serde_json::to_value(settings).unwrap();
        assert!(value.get("backend").is_none());
        assert!(value.get("peakMode").is_none());
        let new: ToneMapSettings = serde_json::from_str(
            r#"{"algorithm":"spline","sourcePeakNits":1000,"backend":"auto","peakMode":"measured"}"#,
        )
        .unwrap();
        assert_eq!(new.algorithm, Some(ToneMapAlgorithm::Spline));
        assert_eq!(new.backend, ToneMapBackend::Auto);
        assert_eq!(new.peak_mode, ToneMapPeakMode::Measured);
        assert_eq!(
            serde_json::from_value::<ToneMapSettings>(serde_json::to_value(new).unwrap()).unwrap(),
            new
        );
    }
}
