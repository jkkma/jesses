//! Optional authored HDR metadata. Fixed-point units preserve exact resource
//! identity and native HDR signaling precision across saved requests.
use serde::{Deserialize, Serialize};
use ts_rs::TS;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HdrChromaticity {
    /// CIE x in units of 1/50000.
    pub x: u16,
    /// CIE y in units of 1/50000.
    pub y: u16,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HdrMasteringDisplay {
    pub red: HdrChromaticity,
    pub green: HdrChromaticity,
    pub blue: HdrChromaticity,
    pub white_point: HdrChromaticity,
    pub max_luminance_ten_thousandths: u32,
    pub min_luminance_ten_thousandths: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HdrContentLight {
    pub max_content: u16,
    pub max_average: u16,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HdrMetadataSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mastering_display: Option<HdrMasteringDisplay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub content_light: Option<HdrContentLight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dolby_vision_rpu_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hdr10_plus_json_path: Option<String>,
}
