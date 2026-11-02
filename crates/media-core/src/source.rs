//! A folder import is an ordered still sequence, distinct from recursive media scanning.
use crate::FrameRate;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageSequenceImportRequest {
    pub directory_path: String,
    pub frame_rate: FrameRate,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageSequenceReleaseRequest {
    pub input_path: String,
    pub sequence_fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MediaSource {
    ImageSequence {
        directory_path: String,
        frame_rate: FrameRate,
        extension: String,
        frame_count: u32,
        sequence_fingerprint: String,
    },
}
