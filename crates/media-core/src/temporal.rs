use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DeinterlaceMode {
    Frame,
    Bob,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum QtgmcPreset {
    Faster,
    #[default]
    Fast,
    Medium,
    Slow,
    Slower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FieldOrder {
    TopFirst,
    BottomFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeinterlaceSettings {
    pub mode: DeinterlaceMode,
    pub field_order: FieldOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QtgmcSettings {
    pub mode: DeinterlaceMode,
    pub field_order: FieldOrder,
    #[serde(default)]
    pub preset: QtgmcPreset,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CadenceRepairKind {
    #[default]
    InverseTelecine,
    ExactDuplicates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CadenceRepairSettings {
    /// Omission preserves the historical inverse-telecine workflow.
    #[serde(default)]
    pub kind: CadenceRepairKind,
    pub field_order: FieldOrder,
    /// Deinterlace frames which remain combed after field matching before the
    /// fixed 5:4 decimation cycle. Omission keeps pure inverse telecine.
    #[serde(default)]
    pub combed_fallback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FrameRate {
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AspectRatioKind {
    Sample,
    Display,
}

/// Output aspect-ratio metadata. This changes SAR/DAR without resampling the
/// picture; later framing is applied first so the requested ratio is final.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AspectRatioSettings {
    pub kind: AspectRatioKind,
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ResizeFilter {
    Nearest,
    Bilinear,
    Bicubic,
    #[default]
    Lanczos,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TemporalSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub deinterlace: Option<DeinterlaceSettings>,
    /// Motion-compensated deinterlacing is a separate optional workflow so
    /// older BWDIF settings retain their exact serialized representation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub qtgmc: Option<QtgmcSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub frame_rate: Option<FrameRate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cadence_repair: Option<CadenceRepairSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub aspect_ratio: Option<AspectRatioSettings>,
    #[serde(default)]
    pub resize_filter: ResizeFilter,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_temporal_json_keeps_bwdif_and_omits_new_workflows() {
        let settings: TemporalSettings = serde_json::from_str(
            r#"{"deinterlace":{"mode":"bob","fieldOrder":"topFirst"},"resizeFilter":"lanczos"}"#,
        )
        .unwrap();
        assert_eq!(settings.deinterlace.unwrap().mode, DeinterlaceMode::Bob);
        assert!(settings.qtgmc.is_none());
        assert!(settings.cadence_repair.is_none());
        assert!(settings.aspect_ratio.is_none());

        let cadence: CadenceRepairSettings =
            serde_json::from_str(r#"{"fieldOrder":"topFirst","combedFallback":false}"#).unwrap();
        assert_eq!(cadence.kind, CadenceRepairKind::InverseTelecine);
    }

    #[test]
    fn new_processing_fields_round_trip_without_changing_names() {
        let settings = TemporalSettings {
            qtgmc: Some(QtgmcSettings {
                mode: DeinterlaceMode::Frame,
                field_order: FieldOrder::BottomFirst,
                preset: QtgmcPreset::Slow,
            }),
            cadence_repair: None,
            aspect_ratio: Some(AspectRatioSettings {
                kind: AspectRatioKind::Display,
                numerator: 16,
                denominator: 9,
            }),
            ..Default::default()
        };
        let value = serde_json::to_value(settings).unwrap();
        assert_eq!(value["qtgmc"]["preset"], "slow");
        assert_eq!(value["aspectRatio"]["kind"], "display");
        assert_eq!(
            serde_json::from_value::<TemporalSettings>(value).unwrap(),
            settings
        );
    }
}
