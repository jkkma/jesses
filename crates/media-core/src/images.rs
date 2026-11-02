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

/// PNG RGB layouts and JPEG YUV layouts. PNG 16-bit formats are big-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ImagePixelFormat {
    Rgb24,
    Rgba,
    Rgb48,
    Rgba64,
    Yuv420p,
    Yuv422p,
    Yuv444p,
}

impl ImagePixelFormat {
    pub const fn ffmpeg(self) -> &'static str {
        match self {
            Self::Rgb24 => "rgb24",
            Self::Rgba => "rgba",
            Self::Rgb48 => "rgb48be",
            Self::Rgba64 => "rgba64be",
            Self::Yuv420p => "yuvj420p",
            Self::Yuv422p => "yuvj422p",
            Self::Yuv444p => "yuvj444p",
        }
    }
}

/// Exactly one burned track, scoped to its original source. Image outputs have
/// no retained subtitle stream, so track copy/conversion flags do not apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "source",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ImageSubtitleSource {
    Primary {
        stream_index: u32,
    },
    External {
        input_path: String,
        stream_index: u32,
        #[serde(default)]
        #[ts(optional, as = "Option<_>")]
        offset_milliseconds: i32,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageProcessingSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub metadata: Option<Box<ImageExportMetadata>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub subtitle_burn_in: Option<ImageSubtitleSource>,
    #[serde(default)]
    pub framing: crate::VideoFraming,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub temporal: Option<crate::TemporalSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub trim: Option<crate::VideoTrim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tone_map: Option<crate::ToneMapSettings>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(optional, as = "Option<_>")]
    pub filters: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(optional, as = "Option<_>")]
    pub source_filters: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}

/// Original loaded-input identity. This is not an FFmpeg output map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageInputTrack {
    pub input_index: u32,
    pub stream_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageImplicitAudioTrack {
    pub source: ImageInputTrack,
    /// Source UI's resolved per-track channel count and channel-scaled bitrate.
    pub channels: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bitrate_kbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub normalization: Option<crate::AudioNormalization>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ImageImplicitAudio {
    Strip,
    Copy,
    Encode {
        codec: crate::AudioCodec,
        tracks: Vec<ImageImplicitAudioTrack>,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ImageImplicitSubtitle {
    #[default]
    Copy,
    Strip,
    MovText,
    SubRip,
    WebVtt,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageImplicitTrackMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
}

/// Source metadata options address output ordinals, even when automatic stream
/// selection chooses streams different from the checked rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageImplicitMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_input: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub chapters_input: Option<u32>,
    #[serde(default)]
    pub attachment_inputs: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
    #[serde(default)]
    pub tracks: Vec<ImageImplicitTrackMetadata>,
    #[serde(default)]
    pub audio_dispositions: Vec<bool>,
    #[serde(default)]
    pub subtitle_dispositions: Vec<bool>,
}

/// Loaded-source metadata inputs are separate from synthetic picture, caption
/// and palette inputs; native execution remaps their donor indices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageExportMetadata {
    pub inputs: Vec<String>,
    pub options: ImageImplicitMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ImageRequest {
    /// Nmkoder's fixed-format path with checked nonvideo tracks and no checked
    /// video. Every loaded source participates in FFmpeg automatic selection.
    ImplicitExport {
        inputs: Vec<String>,
        checked: Vec<ImageInputTrack>,
        current_input: u32,
        format: ImageOutput,
        output_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        audio: Option<ImageImplicitAudio>,
        #[serde(default)]
        #[ts(optional, as = "Option<_>")]
        subtitle_codec: ImageImplicitSubtitle,
        #[serde(default)]
        #[ts(optional, as = "Option<_>")]
        metadata: ImageImplicitMetadata,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        trim: Option<crate::VideoTrim>,
        /// Source ChainKeepsFrameCount; it controls only frame-mode trim.
        #[serde(default = "keep_frames")]
        #[ts(optional, as = "Option<_>")]
        keep_frame_count: bool,
    },
    ImportSequence {
        /// Explicit presentation order, independent of the source filenames.
        paths: Vec<String>,
        frame_rate: crate::FrameRate,
        output_path: String,
    },
    Export {
        input_path: String,
        stream_index: u32,
        #[serde(default)]
        #[ts(optional, as = "Option<_>")]
        start_frame: u32,
        #[serde(default)]
        #[ts(optional, as = "Option<_>")]
        frame_count: u32,
        /// Export every frame after the optional shared picture processing.
        #[serde(default, skip_serializing_if = "is_false")]
        #[ts(optional, as = "Option<_>")]
        full_source: bool,
        format: ImageOutput,
        output_path: String,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        pixel_format: Option<ImagePixelFormat>,
        /// GIF palette 3–256 (default 128); JPEG quality 1–31 (default 3).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        quality: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        processing: Option<ImageProcessingSettings>,
    },
}

fn keep_frames() -> bool {
    true
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
