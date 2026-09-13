use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A selected source subtitle can remain a track, change text format, or become
/// visible video pixels. Burn-in removes that subtitle from the output track list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SubtitleMode {
    #[default]
    Copy,
    SubRip,
    Ass,
    WebVtt,
    BurnIn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTrackSettings {
    /// Original subtitle stream index in the selected source file.
    pub stream_index: u32,
    #[serde(default)]
    pub mode: SubtitleMode,
}
