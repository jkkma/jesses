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
    DeinterlaceExport(DeinterlaceExportRequest),
    CadenceRepairExport(CadenceRepairExportRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DeinterlaceExportMethod {
    Qtgmc,
    Bwdif,
    Yadif,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeinterlaceExportRequest {
    pub input_path: String,
    pub output_path: String,
    pub video_stream_index: u32,
    pub method: DeinterlaceExportMethod,
    pub mode: crate::DeinterlaceMode,
    pub field_order: crate::FieldOrder,
    pub qtgmc_preset: crate::QtgmcPreset,
}

/// A source-style automatic/explicit decision; no request means automatic
/// mode should leave progressive or undetermined input alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeinterlaceResolution {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub request: Option<DeinterlaceExportRequest>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CadenceRepairExportRequest {
    pub input_path: String,
    pub output_path: String,
    pub video_stream_index: u32,
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
    #[serde(default)]
    #[ts(optional, as = "Option<_>")]
    pub output_path: String,
    #[serde(default = "enabled")]
    #[ts(optional, as = "Option<_>")]
    pub copy_color: bool,
    #[serde(default = "enabled")]
    #[ts(optional, as = "Option<_>")]
    pub copy_hdr: bool,
    #[serde(default)]
    #[ts(optional, as = "Option<_>")]
    pub in_place: bool,
}
fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ColorMetadataInspectionRequest {
    pub input_path: String,
    pub video_stream_index: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ColorMetadataReport {
    pub input_path: String,
    pub video_stream_index: u32,
    pub color: std::collections::BTreeMap<String, String>,
    pub hdr: std::collections::BTreeMap<String, String>,
    pub dolby_vision_profile: Option<u32>,
    pub dolby_vision_compatibility: Option<u32>,
    pub source_fingerprint: String,
    pub summary: String,
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
    /// Denoise once into a verified lossless derivative, then measure source grain.
    MeasureSource {
        source_path: String,
        output_table_path: String,
        denoise_strength: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        denoised_output_path: Option<String>,
    },
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
    AomAv1,
    H264Nvenc,
    HevcNvenc,
}
impl LadderEncoder {
    pub fn is_hardware(self) -> bool {
        matches!(self, Self::H264Nvenc | Self::HevcNvenc)
    }
    pub fn require_cpu_encoding(self) -> Result<(), crate::AppError> {
        if self.is_hardware() {
            Err(crate::AppError::new(
                "HARDWARE_ENCODING_UNSUPPORTED",
                "Jesses supports CPU video encoding only. This historical Sample Encodes hardware encoder is unsupported.",
                None,
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LadderMetric {
    None,
    Psnr,
    Ssim,
    Vmaf,
    Ssimulacra2,
    ButteraugliInf,
    XpsnrWeighted,
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
    /// Human-readable content-preset identity, retained with the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub content_preset: Option<String>,
    /// Encoder-native content controls; unsupported fork controls are reported.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(optional, as = "Option<_>")]
    pub parameters: Vec<crate::EncoderParameter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub vmaf_model: Option<crate::QualityVmafModel>,
    /// A new directory for validated references and encoded samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub keep_samples_directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UtilityBatchRequest {
    pub requests: Vec<UtilityRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UtilityBatchEntry {
    pub index: u32,
    pub request: UtilityRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub result: Option<UtilityResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<crate::AppError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UtilityBatchResult {
    pub entries: Vec<UtilityBatchEntry>,
    pub completed: u32,
    pub canceled: bool,
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
    /// Measured table bytes only; a retained denoised video is separate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub size_bytes: Option<String>,
    pub segment_count: u32,
    pub source_fingerprints: Vec<String>,
    pub message: String,
    pub diagnostics: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub denoised_output_path: Option<String>,
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
    pub recommendation_threshold: Option<f64>,
    pub recommended_crf: Option<u8>,
    pub source_fingerprint: String,
    pub message: String,
    pub diagnostics: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub content_preset: Option<String>,
    #[serde(default)]
    pub parameters: Vec<crate::EncoderParameter>,
    #[serde(default)]
    pub vmaf_model: crate::QualityVmafModel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub samples_directory: Option<String>,
    /// Complete recursively measured retained directory, only on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub samples_size_bytes: Option<String>,
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
    fn legacy_color_transfer_defaults_to_copying_both_categories_to_a_new_file() {
        let request:ColorMetadataTransferRequest=serde_json::from_value(serde_json::json!({"metadataSourcePath":"/donor.mkv","metadataSourceVideoStreamIndex":0,"inputPath":"/target.mkv","inputVideoStreamIndex":0,"outputPath":"/new.mkv"})).unwrap();
        assert!(request.copy_color && request.copy_hdr);
        assert!(!request.in_place);
        let in_place:ColorMetadataTransferRequest=serde_json::from_value(serde_json::json!({"metadataSourcePath":"/donor.mkv","metadataSourceVideoStreamIndex":0,"inputPath":"/target.mkv","inputVideoStreamIndex":0,"inPlace":true,"copyColor":false,"copyHdr":true})).unwrap();
        assert!(
            in_place.output_path.is_empty()
                && in_place.in_place
                && !in_place.copy_color
                && in_place.copy_hdr
        );
    }
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

/// Live status for one utility ticket. Terminal results remain authoritative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UtilityProgress {
    pub row_index: Option<u32>,
    pub row_count: u32,
    pub completed_rows: u32,
    /// running, succeeded, failed or skipped.
    pub status: String,
    pub phase: String,
    pub message: String,
    pub elapsed_seconds: f64,
    pub progress_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub estimated_remaining_seconds: Option<f64>,
}
