//! Tauri-independent contracts for media inspection and queued conversion jobs.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

mod analysis;
mod command_edit;
pub use command_edit::{
    CommandEditConfirmation, CommandEditRequest, CommandEditResult, CommandEditReview,
    EditedCommand,
};
mod output_resolution;
mod source;
pub use output_resolution::{OutputPathRequest, OutputPathResolution, OutputTarget};
pub use source::{ImageSequenceImportRequest, ImageSequenceReleaseRequest, MediaSource};
mod hdr_metadata;
mod images;
pub use hdr_metadata::{
    HdrChromaticity, HdrContentLight, HdrMasteringDisplay, HdrMetadataSettings,
};
mod saved_jobs;
pub use saved_jobs::SavedJobInspection;
mod completion;
pub use completion::{CompletionOptions, CompletionStatus, FinishAction};
mod utilities;
pub use images::{
    ImageExportMetadata, ImageImplicitAudio, ImageImplicitAudioTrack, ImageImplicitMetadata,
    ImageImplicitSubtitle, ImageImplicitTrackMetadata, ImageInputTrack, ImageOutput,
    ImagePixelFormat, ImageProcessingSettings, ImageRequest, ImageResult, ImageSubtitleSource,
};
pub use utilities::{
    CadenceRepairExportRequest, ColorMetadataInspectionRequest, ColorMetadataReport,
    ColorMetadataTransferRequest, ConcatRequest, CrfLadderRequest, CrfLadderResult, CrfLadderRung,
    DeinterlaceExportMethod, DeinterlaceExportRequest, DeinterlaceResolution, GrainRequest,
    GrainSource, GrainTableResult, KeyframeCutRequest, LadderEncoder, LadderMetric,
    SubtitleOcrRequest, SubtitleOcrResult, UtilityArtifact, UtilityBatchEntry, UtilityBatchRequest,
    UtilityBatchResult, UtilityCapabilities, UtilityDependency, UtilityProgress, UtilityRequest,
    UtilityResult,
};
mod av1an;
mod av1an_resources;
pub use av1an::{
    Av1anChunkMethod, Av1anChunkOrder, Av1anConcatMethod, Av1anGrainSettings, Av1anOptions,
    Av1anPixelFormat, Av1anSceneDetection, Av1anSplitMethod, Av1anTargetMetric, Av1anTargetQuality,
};
pub use av1an_resources::{Av1anResourceEstimate, Av1anResourceRequest};
mod batch;
mod bitrate;
mod container;
mod encoder_parameters;
mod jobs;
mod recovery_actions;
pub use recovery_actions::{Av1anRecoveryAction, Av1anRecoveryActionResult, Av1anRecoveryEntry};
mod loudness;
mod preferences;
mod quality;
mod reports;
pub use loudness::{AudioNormalization, LoudnessRequest, LoudnessResult, LoudnessTarget};
pub use preferences::{
    ConsoleWindowMode, EncodingPreferences, GeneralPreferences, PreferenceImportPreview,
    ResetOnNewFilePreferences, SavePreferencesRequest, UserPreferences,
};
pub use quality::{
    QualityAlignment, QualityMetric, QualityOptions, QualityPoint, QualityRequest, QualityResult,
    QualityVmafModel,
};
pub use reports::{AnalysisExportFormat, AnalysisExportRequest, AnalysisReport};
mod mux;
mod subtitles;
mod temporal;
mod tone_map;
pub use analysis::{
    AutoCropRequest, AutoCropResult, DetectedFieldOrder, FieldDetectionRequest,
    FieldDetectionResult, FramePreviewRequest, FramePreviewResult, KeyframeSnapRequest,
    KeyframeSnapResult,
};
pub use batch::{
    BatchEncodeInput, BatchEncodeItem, BatchEncodePreview, BatchEncodeRequest, BatchNamingValues,
    BatchOutputPathInput, BatchOutputPathItem, BatchOutputPathRequest, BatchOutputPaths,
    FolderScanRequest, FolderScanResult,
};
pub use bitrate::{BitratePoint, BitrateRequest, BitrateResult};
pub use container::ContainerFormat;
pub use encoder_parameters::{
    EncodeCommandPlan, EncodeCommandStage, EncoderParameter, EncoderParameterCatalog,
    EncoderParameterPreset, EncoderParameterPresetKey, EncoderParameterQuery, EncoderParameterSpec,
};
pub use jobs::{
    AudioChannelLayoutPolicy, AudioChannels, AudioCodec, AudioGain, AudioTrackSettings,
    Av1anRecovery, BorderRatio, BorderSettings, CropSettings, EncodeBackend, EncodeRequest,
    EncodeSettings, EncodeTrackOverride, EncodeTrackRef, ExternalAudioSettings, ExternalTrack,
    HdrTune, JobSnapshot, JobState, OutputMetadata, RecoveryPhase, RemuxRequest, ResizeFill,
    ResizeMode, ResizeSettings, StandaloneRecovery, StandaloneRecoveryPhase, VideoEncoder,
    VideoFraming, VideoMode, VideoRateControl, VideoSeekStrategy, VideoTimeTrim, VideoTrim,
};
pub use mux::{MuxRequest, MuxSource, MuxTrack};
pub use subtitles::{SubtitleMode, SubtitleTrackSettings};
pub use temporal::{
    AspectRatioKind, AspectRatioSettings, CadenceRepairKind, CadenceRepairSettings,
    DeinterlaceEngine, DeinterlaceMode, DeinterlaceSettings, FieldOrder, FrameRate, QtgmcPreset,
    QtgmcSettings, ResizeFilter, TemporalSettings,
};
pub use tone_map::{ToneMapAlgorithm, ToneMapBackend, ToneMapPeakMode, ToneMapSettings};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MediaFile {
    /// Stable for the same canonical input path; this is not a content hash.
    pub id: String,
    pub path: String,
    pub name: String,
    /// The original folder identity; `path` remains the guarded processing source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<MediaSource>,
    /// Decimal text preserves byte counts beyond JavaScript's safe integer range.
    pub size_bytes: String,
    /// File modification time as signed Unix milliseconds. Unknown for sequences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub modified_at_ms: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
    /// Reported bits per second, kept as decimal text without JS rounding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bit_rate: Option<String>,
    pub streams: Vec<MediaStream>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MediaStream {
    /// Original source stream index, independent of presentation order.
    pub index: u32,
    pub kind: String,
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub codec_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub codec_long_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bit_rate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub average_frame_rate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub nominal_frame_rate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub is_default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attachment_filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attachment_mime_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sample_aspect_ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub display_aspect_ratio: Option<String>,
    /// Reported display-matrix rotation, in degrees as finite decimal text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rotation_degrees: Option<String>,
    /// Original rational frame rate, for example `24000/1001`.
    pub frame_rate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub field_order: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub channel_layout: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub pixel_format: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub bit_depth: Option<u32>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub color_primaries: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub color_transfer: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub color_space: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub color_range: Option<String>,
    /// HDR transfer family; this does not establish encode compatibility.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub hdr_format: Option<String>,
    /// Whether stream headers report mastering or content light metadata.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub has_hdr_static_metadata: Option<bool>,
    /// Formats reported in stream headers; absence does not rule out frame metadata.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub dynamic_hdr_formats: Option<Vec<String>>,
    /// Header-declared Dolby Vision profile; runtime validates its full record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dolby_vision_profile: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub path: Option<String>,
}

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, path: Option<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path,
        }
    }
}

/// Single-file contract output keeps imports deterministic and drift checks simple.
pub fn typescript_contracts() -> String {
    let config = ts_rs::Config::default();
    let declarations = [
        SavedJobInspection::decl(&config),
        StandaloneRecovery::decl(&config),
        StandaloneRecoveryPhase::decl(&config),
        QtgmcSettings::decl(&config),
        QtgmcPreset::decl(&config),
        CadenceRepairSettings::decl(&config),
        CadenceRepairKind::decl(&config),
        AspectRatioKind::decl(&config),
        AspectRatioSettings::decl(&config),
        FinishAction::decl(&config),
        CompletionOptions::decl(&config),
        CompletionStatus::decl(&config),
        ImageOutput::decl(&config),
        ImagePixelFormat::decl(&config),
        ImageProcessingSettings::decl(&config),
        ImageSubtitleSource::decl(&config),
        ImageInputTrack::decl(&config),
        ImageImplicitAudioTrack::decl(&config),
        ImageImplicitAudio::decl(&config),
        ImageImplicitSubtitle::decl(&config),
        ImageImplicitTrackMetadata::decl(&config),
        ImageImplicitMetadata::decl(&config),
        ImageExportMetadata::decl(&config),
        OutputTarget::decl(&config),
        OutputPathRequest::decl(&config),
        OutputPathResolution::decl(&config),
        ImageRequest::decl(&config),
        ImageResult::decl(&config),
        UtilityRequest::decl(&config),
        UtilityProgress::decl(&config),
        UtilityBatchRequest::decl(&config),
        UtilityBatchResult::decl(&config),
        UtilityBatchEntry::decl(&config),
        UtilityResult::decl(&config),
        UtilityCapabilities::decl(&config),
        UtilityDependency::decl(&config),
        KeyframeCutRequest::decl(&config),
        ConcatRequest::decl(&config),
        ColorMetadataTransferRequest::decl(&config),
        ColorMetadataInspectionRequest::decl(&config),
        ColorMetadataReport::decl(&config),
        SubtitleOcrRequest::decl(&config),
        GrainRequest::decl(&config),
        GrainSource::decl(&config),
        LadderEncoder::decl(&config),
        LadderMetric::decl(&config),
        CrfLadderRequest::decl(&config),
        DeinterlaceExportMethod::decl(&config),
        DeinterlaceExportRequest::decl(&config),
        DeinterlaceResolution::decl(&config),
        CadenceRepairExportRequest::decl(&config),
        UtilityArtifact::decl(&config),
        SubtitleOcrResult::decl(&config),
        GrainTableResult::decl(&config),
        CrfLadderResult::decl(&config),
        CrfLadderRung::decl(&config),
        ToolInfo::decl(&config),
        GeneralPreferences::decl(&config),
        EncodingPreferences::decl(&config),
        ConsoleWindowMode::decl(&config),
        ResetOnNewFilePreferences::decl(&config),
        UserPreferences::decl(&config),
        EncoderParameterPreset::decl(&config),
        EncoderParameterPresetKey::decl(&config),
        SavePreferencesRequest::decl(&config),
        PreferenceImportPreview::decl(&config),
        MediaStream::decl(&config),
        MediaFile::decl(&config),
        MediaSource::decl(&config),
        ImageSequenceImportRequest::decl(&config),
        ImageSequenceReleaseRequest::decl(&config),
        AppError::decl(&config),
        FramePreviewRequest::decl(&config),
        FramePreviewResult::decl(&config),
        AutoCropRequest::decl(&config),
        AutoCropResult::decl(&config),
        KeyframeSnapRequest::decl(&config),
        KeyframeSnapResult::decl(&config),
        FieldDetectionRequest::decl(&config),
        FieldDetectionResult::decl(&config),
        DetectedFieldOrder::decl(&config),
        BitrateRequest::decl(&config),
        BitratePoint::decl(&config),
        BitrateResult::decl(&config),
        LoudnessRequest::decl(&config),
        LoudnessResult::decl(&config),
        QualityMetric::decl(&config),
        QualityAlignment::decl(&config),
        QualityVmafModel::decl(&config),
        QualityOptions::decl(&config),
        QualityPoint::decl(&config),
        QualityRequest::decl(&config),
        QualityResult::decl(&config),
        AnalysisReport::decl(&config),
        AnalysisExportFormat::decl(&config),
        AnalysisExportRequest::decl(&config),
        DeinterlaceMode::decl(&config),
        FieldOrder::decl(&config),
        DeinterlaceSettings::decl(&config),
        DeinterlaceEngine::decl(&config),
        FrameRate::decl(&config),
        ResizeFilter::decl(&config),
        TemporalSettings::decl(&config),
        Av1anChunkMethod::decl(&config),
        Av1anSplitMethod::decl(&config),
        Av1anSceneDetection::decl(&config),
        Av1anChunkOrder::decl(&config),
        Av1anConcatMethod::decl(&config),
        Av1anPixelFormat::decl(&config),
        Av1anResourceRequest::decl(&config),
        Av1anResourceEstimate::decl(&config),
        Av1anTargetMetric::decl(&config),
        Av1anTargetQuality::decl(&config),
        Av1anOptions::decl(&config),
        Av1anGrainSettings::decl(&config),
        ToneMapAlgorithm::decl(&config),
        ToneMapBackend::decl(&config),
        ToneMapPeakMode::decl(&config),
        ToneMapSettings::decl(&config),
        RemuxRequest::decl(&config),
        ExternalTrack::decl(&config),
        EncodeTrackOverride::decl(&config),
        EncodeTrackRef::decl(&config),
        ExternalAudioSettings::decl(&config),
        MuxSource::decl(&config),
        MuxTrack::decl(&config),
        MuxRequest::decl(&config),
        ContainerFormat::decl(&config),
        VideoRateControl::decl(&config),
        SubtitleMode::decl(&config),
        SubtitleTrackSettings::decl(&config),
        AudioCodec::decl(&config),
        AudioChannels::decl(&config),
        AudioChannelLayoutPolicy::decl(&config),
        AudioNormalization::decl(&config),
        LoudnessTarget::decl(&config),
        AudioTrackSettings::decl(&config),
        AudioGain::decl(&config),
        EncodeBackend::decl(&config),
        VideoEncoder::decl(&config),
        HdrTune::decl(&config),
        CropSettings::decl(&config),
        BorderSettings::decl(&config),
        VideoMode::decl(&config),
        ResizeMode::decl(&config),
        ResizeFill::decl(&config),
        ResizeSettings::decl(&config),
        BorderRatio::decl(&config),
        VideoFraming::decl(&config),
        VideoTrim::decl(&config),
        VideoTimeTrim::decl(&config),
        VideoSeekStrategy::decl(&config),
        EncoderParameter::decl(&config),
        EncoderParameterQuery::decl(&config),
        EncoderParameterSpec::decl(&config),
        EncoderParameterCatalog::decl(&config),
        EncodeCommandStage::decl(&config),
        EncodeCommandPlan::decl(&config),
        CommandEditRequest::decl(&config),
        CommandEditReview::decl(&config),
        CommandEditConfirmation::decl(&config),
        CommandEditResult::decl(&config),
        EditedCommand::decl(&config),
        HdrChromaticity::decl(&config),
        HdrMasteringDisplay::decl(&config),
        HdrContentLight::decl(&config),
        HdrMetadataSettings::decl(&config),
        OutputMetadata::decl(&config),
        EncodeSettings::decl(&config),
        EncodeRequest::decl(&config),
        JobState::decl(&config),
        RecoveryPhase::decl(&config),
        Av1anRecovery::decl(&config),
        JobSnapshot::decl(&config),
        Av1anRecoveryAction::decl(&config),
        Av1anRecoveryActionResult::decl(&config),
        Av1anRecoveryEntry::decl(&config),
        FolderScanRequest::decl(&config),
        FolderScanResult::decl(&config),
        BatchEncodeInput::decl(&config),
        BatchNamingValues::decl(&config),
        BatchOutputPathInput::decl(&config),
        BatchOutputPathRequest::decl(&config),
        BatchOutputPathItem::decl(&config),
        BatchOutputPaths::decl(&config),
        BatchEncodeRequest::decl(&config),
        BatchEncodeItem::decl(&config),
        BatchEncodePreview::decl(&config),
    ];
    let mut output =
        String::from("// Generated by media-core. Run pnpm contracts; do not edit.\n\n");
    for declaration in declarations {
        output.push_str("export ");
        output.push_str(&declaration);
        output.push_str("\n\n");
    }
    let normalized = output
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n", normalized.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_preserves_large_byte_counts_and_explicit_missing_metadata() {
        let value = serde_json::to_value(MediaFile {
            id: "media-1".into(),
            path: "example.mkv".into(),
            name: "example.mkv".into(),
            source: None,
            modified_at_ms: None,
            size_bytes: "9007199254740993".into(),
            duration_seconds: None,
            format: None,
            title: None,
            language: None,
            bit_rate: None,
            streams: vec![],
        })
        .unwrap();
        assert_eq!(value["sizeBytes"], "9007199254740993");
        assert!(value["durationSeconds"].is_null());
        assert!(value.get("size_bytes").is_none());
        assert!(typescript_contracts().contains("sizeBytes: string"));
        assert!(typescript_contracts().contains("durationSeconds: number | null"));
    }

    #[test]
    fn older_stream_records_keep_hdr_metadata_unknown() {
        let stream: MediaStream = serde_json::from_str(r#"{"index":0,"kind":"video","codec":"h264","width":1920,"height":1080,"frameRate":"24/1","sampleRate":null,"channels":null,"language":null,"title":null}"#).unwrap();
        assert_eq!(stream.pixel_format, None);
        assert_eq!(stream.has_hdr_static_metadata, None);
        assert_eq!(stream.dynamic_hdr_formats, None);
        assert!(typescript_contracts().contains("pixelFormat?: string | null"));
    }
}
