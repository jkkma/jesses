use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Av1anResourceRequest {
    pub encoder: crate::VideoEncoder,
    pub source_width: u32,
    pub source_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub workers: u8,
    pub filtered: bool,
    pub float_filter: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Av1anResourceEstimate {
    pub logical_processors: u32,
    pub total_memory_mib: Option<u32>,
    pub available_memory_mib: Option<u32>,
    pub per_worker_mib: u32,
    pub estimated_memory_mib: u32,
    pub suggested_workers: u8,
    pub suggested_threads: u8,
    pub suggested_scene_slices: u8,
    pub warning: Option<String>,
}
