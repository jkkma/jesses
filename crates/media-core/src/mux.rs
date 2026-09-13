use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Identity is stable while inputs are reordered or another input is removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MuxSource {
    pub id: String,
    pub input_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MuxTrack {
    pub source_id: String,
    pub stream_index: u32,
    /// None preserves the source value; an empty string clears the tag.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub title: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub language: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub default: Option<bool>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub forced: Option<bool>,
}

/// Ordered selected tracks, one metadata owner and an optional chapter owner.
/// This separate contract leaves historical standalone encode requests intact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MuxRequest {
    pub sources: Vec<MuxSource>,
    pub tracks: Vec<MuxTrack>,
    pub metadata_source_id: String,
    pub chapters_source_id: Option<String>,
    pub output_path: String,
}
