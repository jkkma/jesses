use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Av1anChunkMethod {
    #[default]
    Lsmash,
    Ffms2,
    Bestsource,
    Select,
    Hybrid,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Av1anSplitMethod {
    #[default]
    SceneDetection,
    FixedChunks,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Av1anSceneDetection {
    #[default]
    Standard,
    Fast,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Av1anChunkOrder {
    #[default]
    LongToShort,
    ShortToLong,
    Sequential,
    Random,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Av1anTargetMetric {
    #[default]
    Vmaf,
    Ssimulacra2,
    Butteraugli,
    Xpsnr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Av1anTargetQuality {
    #[serde(default)]
    pub metric: Av1anTargetMetric,
    pub minimum_score_tenths: u16,
    pub maximum_score_tenths: u16,
    pub minimum_crf: u8,
    pub maximum_crf: u8,
    pub probes: u8,
    pub probing_rate: u8,
    pub probe_width: u16,
    pub probe_height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct Av1anOptions {
    pub chunk_method: Av1anChunkMethod,
    pub split_method: Av1anSplitMethod,
    pub scene_detection: Av1anSceneDetection,
    pub maximum_chunk_frames: u32,
    pub minimum_scene_frames: u32,
    pub scene_downscale_height: Option<u16>,
    pub chunk_order: Av1anChunkOrder,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target_quality: Option<Av1anTargetQuality>,
}

impl Default for Av1anOptions {
    fn default() -> Self {
        Self {
            chunk_method: Av1anChunkMethod::Lsmash,
            split_method: Av1anSplitMethod::SceneDetection,
            scene_detection: Av1anSceneDetection::Standard,
            maximum_chunk_frames: 240,
            minimum_scene_frames: 24,
            scene_downscale_height: Some(360),
            chunk_order: Av1anChunkOrder::LongToShort,
            target_quality: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_target_without_metric_preserves_vmaf() {
        let target: Av1anTargetQuality = serde_json::from_str(r#"{"minimumScoreTenths":940,"maximumScoreTenths":960,"minimumCrf":15,"maximumCrf":50,"probes":4,"probingRate":1,"probeWidth":1920,"probeHeight":1080}"#).unwrap();
        assert_eq!(target.metric, Av1anTargetMetric::Vmaf);
    }
}
