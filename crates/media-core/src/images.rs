use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ImageOutput {
    Png,
    Jpeg,
    PngSequence,
    JpegSequence,
    Gif,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ImageRequest {
    ImportSequence {
        /// Explicit presentation order, independent of the source filenames.
        paths: Vec<String>,
        frame_rate: crate::FrameRate,
        output_path: String,
    },
    Export {
        input_path: String,
        stream_index: u32,
        start_frame: u32,
        frame_count: u32,
        format: ImageOutput,
        output_path: String,
        #[serde(default)]
        width: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageResult {
    pub output_path: String,
    pub frame_count: u32,
    pub width: u32,
    pub height: u32,
    pub notes: Vec<String>,
}
