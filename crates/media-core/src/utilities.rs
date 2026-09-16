//! Stable contracts for supervised media utilities.
//!
//! Every write request names a new destination. Implementations must hold each
//! source open, verify its identity after tool execution, and publish only a
//! semantically validated temporary artifact.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "request", rename_all = "camelCase")]
pub enum UtilityRequest {
    KeyframeCut(KeyframeCutRequest),
    Concat(ConcatRequest),
    ColorMetadataTransfer(ColorMetadataTransferRequest),
    SubtitleOcr(SubtitleOcrRequest),
    Grain(GrainRequest),
    CrfLadder(CrfLadderRequest),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeyframeCutRequest {
    pub input_path: String,
    pub output_path: String,
    /// Requested presentation timestamp in seconds. Stream copy starts at the
    /// nearest usable keyframe at or before this point.
    pub start_seconds: f64,
    pub end_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConcatRequest {
    pub input_paths: Vec<String>,
    pub output_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ColorMetadataTransferRequest {
    /// Supplies the color declaration. Encoded packets are taken from input_path.
    pub metadata_source_path: String,
    pub metadata_source_video_stream_index: u32,
    pub input_path: String,
    pub input_video_stream_index: u32,
    pub output_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleOcrRequest {
    pub input_path: String,
    pub subtitle_stream_index: u32,
    /// Tesseract language expression, for example `eng` or `eng+spa`.
    pub language: String,
    pub output_path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GrainRequest {
    Measure {
        source_path: String,
        denoised_path: String,
        output_table_path: String,
    },
    Extract {
        input_path: String,
        output_table_path: String,
    },
    Apply {
        input_path: String,
        output_path: String,
        source: GrainSource,
    },
    /// Deliberately replaces grain headers already present in the AV1 stream.
    RewriteHeaders {
        input_path: String,
        output_path: String,
        source: GrainSource,
    },
    Remove {
        input_path: String,
        output_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GrainSource {
    Table { table_path: String },
    Preset { preset: String },
    PhotonNoise { iso: u32, chroma: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LadderEncoder {
    H264,
    Hevc,
    Av1,
    Vp9,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LadderMetric {
    None,
    Psnr,
    Ssim,
    Vmaf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrfLadderRequest {
    pub input_path: String,
    pub video_stream_index: u32,
    pub encoder: LadderEncoder,
    /// Passed only to the selected, capability-checked FFmpeg encoder wrapper.
    pub preset: String,
    pub pixel_format: String,
    pub crfs: Vec<u8>,
    pub sample_count: u8,
    pub sample_seconds: f64,
    pub metric: LadderMetric,
    /// When absent, a documented metric default is used. Ignored for `none`.
    pub recommendation_threshold: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UtilityDependency {
    pub id: String,
    pub available: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UtilityCapabilities {
    pub dependencies: Vec<UtilityDependency>,
    pub ocr_languages: Vec<String>,
    pub grain_presets: Vec<String>,
    pub ffmpeg_encoders: Vec<LadderEncoder>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "result", rename_all = "camelCase")]
pub enum UtilityResult {
    Artifact(UtilityArtifact),
    SubtitleOcr(SubtitleOcrResult),
    GrainTable(GrainTableResult),
    CrfLadder(CrfLadderResult),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UtilityArtifact {
    pub operation: String,
    pub output_path: String,
    /// Decimal text preserves byte counts beyond JavaScript's safe integer range.
    pub size_bytes: String,
    pub duration_seconds: Option<f64>,
    pub source_fingerprints: Vec<String>,
    pub message: String,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleOcrResult {
    pub output_path: String,
    pub cue_count: u32,
    pub language: String,
    pub source_fingerprint: String,
    pub message: String,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GrainTableResult {
    pub output_path: String,
    pub segment_count: u32,
    pub source_fingerprints: Vec<String>,
    pub message: String,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrfLadderResult {
    pub encoder: LadderEncoder,
    pub preset: String,
    pub metric: LadderMetric,
    pub source_duration_seconds: f64,
    pub source_size_bytes: String,
    pub sampled_seconds: f64,
    pub sampled_fraction: f64,
    pub rungs: Vec<CrfLadderRung>,
    pub recommended_crf: Option<u8>,
    pub source_fingerprint: String,
    pub message: String,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrfLadderRung {
    pub crf: u8,
    pub encoded_bytes: String,
    pub encoded_seconds: f64,
    pub bitrate_kbps: f64,
    pub bytes_per_minute: String,
    pub projected_size_bytes: String,
    pub score: Option<f64>,
    pub encode_seconds: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn externally_tagged_payloads_are_explicit_and_forward_readable() {
        let value = serde_json::to_value(UtilityRequest::Grain(GrainRequest::Apply {
            input_path: "in.mkv".into(),
            output_path: "out.mkv".into(),
            source: GrainSource::PhotonNoise {
                iso: 400,
                chroma: true,
            },
        }))
        .unwrap();
        assert_eq!(value["kind"], "grain");
        assert_eq!(value["request"]["operation"], "apply");
        assert_eq!(value["request"]["source"]["kind"], "photonNoise");
    }
}
