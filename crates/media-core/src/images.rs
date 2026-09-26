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

/// PNG's explicit RGB layouts. The 16-bit formats are big-endian on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ImagePixelFormat {
    Rgb24,
    Rgba,
    Rgb48,
    Rgba64,
}

impl ImagePixelFormat {
    pub const fn ffmpeg(self) -> &'static str {
        match self {
            Self::Rgb24 => "rgb24",
            Self::Rgba => "rgba",
            Self::Rgb48 => "rgb48be",
            Self::Rgba64 => "rgba64be",
        }
    }
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        pixel_format: Option<ImagePixelFormat>,
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
