use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::AppError;

/// A job owns this selection; later changes to the inspector cannot alter it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RemuxRequest {
    pub input_path: String,
    pub output_path: String,
    pub stream_indices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EncodeSettings {
    /// Per-source framing, applied before standalone video encoding.
    #[serde(default)]
    pub framing: VideoFraming,
    /// Per-source-track overrides; omitted selected audio streams are copied.
    #[serde(default)]
    pub audio: Vec<AudioTrackSettings>,
    #[serde(default)]
    pub backend: EncodeBackend,
    #[serde(default)]
    pub encoder: VideoEncoder,
    #[serde(default = "default_workers")]
    pub workers: u8,
    pub video_stream_index: u32,
    pub crf: u8,
    pub preset: u8,
    /// AV1 grain synthesis strength; zero leaves synthesis disabled.
    #[serde(default)]
    pub film_grain: u8,
    /// 5fish's paired anime controls; never passed to another SVT build.
    #[serde(default)]
    pub lineart_psy_bias: u8,
    #[serde(default)]
    pub texture_psy_bias: u8,
    #[serde(default)]
    pub hdr_tune: HdrTune,
    /// Explicit consent to discard dynamic HDR metadata for an HDR10 base layer.
    #[serde(default)]
    pub hdr10_fallback: bool,
}

impl Default for EncodeSettings {
    fn default() -> Self {
        Self {
            framing: VideoFraming::default(),
            audio: Vec::new(),
            backend: EncodeBackend::default(),
            encoder: VideoEncoder::default(),
            workers: default_workers(),
            video_stream_index: 0,
            crf: 30,
            preset: 4,
            film_grain: 0,
            lineart_psy_bias: 0,
            texture_psy_bias: 0,
            hdr_tune: HdrTune::default(),
            hdr10_fallback: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VideoFraming {
    #[serde(default)]
    pub crop: CropSettings,
    /// Keep the cropped dimensions when omitted; otherwise preserve their aspect ratio.
    #[serde(default)]
    pub resize_width: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CropSettings {
    #[serde(default)]
    pub top: u32,
    #[serde(default)]
    pub right: u32,
    #[serde(default)]
    pub bottom: u32,
    #[serde(default)]
    pub left: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AudioCodec {
    #[default]
    Copy,
    Opus,
    Aac,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AudioChannels {
    #[default]
    Preserve,
    Mono,
    Stereo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrackSettings {
    pub stream_index: u32,
    pub codec: AudioCodec,
    #[serde(default = "default_audio_bitrate")]
    pub bitrate_kbps: u16,
    #[serde(default)]
    pub channels: AudioChannels,
}

fn default_audio_bitrate() -> u16 {
    128
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum EncodeBackend {
    #[default]
    Standalone,
    Av1an,
}

impl<'de> Deserialize<'de> for EncodeBackend {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Keep the old workflow name readable without advertising it in the
        // generated frontend contract. New history uses the canonical name.
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        enum Wire {
            #[serde(alias = "svtAv1")]
            Standalone,
            Av1an,
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Standalone => Self::Standalone,
            Wire::Av1an => Self::Av1an,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum VideoEncoder {
    #[default]
    SvtAv1,
    SvtAv1FiveFish,
    SvtAv1Hdr,
    X264,
}

impl VideoEncoder {
    pub fn is_svt(self) -> bool {
        matches!(self, Self::SvtAv1 | Self::SvtAv1FiveFish | Self::SvtAv1Hdr)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::SvtAv1 => "SVT-AV1",
            Self::SvtAv1FiveFish => "SVT-AV1 5fish",
            Self::SvtAv1Hdr => "SVT-AV1-HDR",
            Self::X264 => "x264",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum HdrTune {
    #[default]
    VisualQuality,
    FilmGrain,
}

pub(crate) fn default_workers() -> u8 {
    2
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EncodeRequest {
    pub source: RemuxRequest,
    pub settings: EncodeSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Queued,
    Preparing,
    Running,
    Finalizing,
    Succeeded,
    Canceling,
    Canceled,
    Failed,
    Interrupted,
}

impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Canceled | Self::Failed | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: String,
    pub state: JobState,
    pub request: RemuxRequest,
    #[serde(default)]
    pub encode_settings: Option<EncodeSettings>,
    pub progress_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub logs: Vec<String>,
    pub error: Option<AppError>,
    pub log_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_defaults_preserve_old_history_and_per_file_wire_contracts() {
        let old_settings: EncodeSettings =
            serde_json::from_str(r#"{"videoStreamIndex":0,"crf":30,"preset":4}"#).unwrap();
        let old_input: crate::BatchEncodeInput = serde_json::from_str(
            r#"{"inputPath":"source.mkv","streamIndices":[0],"videoStreamIndex":0}"#,
        )
        .unwrap();
        assert_eq!(old_settings.framing, VideoFraming::default());
        assert_eq!(old_input.framing, VideoFraming::default());
        assert_eq!(
            serde_json::from_str::<VideoFraming>("{}").unwrap(),
            VideoFraming::default()
        );
        let framing: VideoFraming =
            serde_json::from_str(r#"{"crop":{"left":16,"right":8},"resizeWidth":960}"#).unwrap();
        assert_eq!(framing.crop.top, 0);
        assert_eq!(framing.crop.bottom, 0);
        assert_eq!(framing.resize_width, Some(960));
        let settings = EncodeSettings {
            framing,
            ..old_settings
        };
        let input = crate::BatchEncodeInput {
            framing,
            ..old_input
        };
        assert_eq!(
            serde_json::from_value::<EncodeSettings>(serde_json::to_value(&settings).unwrap())
                .unwrap(),
            settings
        );
        assert_eq!(
            serde_json::from_value::<crate::BatchEncodeInput>(
                serde_json::to_value(&input).unwrap()
            )
            .unwrap(),
            input
        );
        for malformed in [
            r#"{"resizeWidth":-2}"#,
            r#"{"resizeWidth":128.5}"#,
            r#"{"resizeWidth":4294967296}"#,
            r#"{"crop":{"left":-2}}"#,
            r#"{"crop":{"top":2.5}}"#,
        ] {
            assert!(
                serde_json::from_str::<VideoFraming>(malformed).is_err(),
                "{malformed}"
            );
        }
    }
    #[test]
    fn audio_defaults_and_overrides_survive_old_and_new_snapshots() {
        let old: crate::BatchEncodeInput = serde_json::from_str(
            r#"{"inputPath":"source.mkv","streamIndices":[0,1],"videoStreamIndex":0}"#,
        )
        .unwrap();
        assert!(old.audio.is_empty());
        let track: AudioTrackSettings =
            serde_json::from_str(r#"{"streamIndex":4,"codec":"opus"}"#).unwrap();
        assert_eq!(track.bitrate_kbps, 128);
        assert_eq!(track.channels, AudioChannels::Preserve);
        let settings = EncodeSettings {
            audio: vec![track],
            ..Default::default()
        };
        let wire = serde_json::to_value(&settings).unwrap();
        assert_eq!(wire["audio"][0]["codec"], "opus");
        assert_eq!(
            serde_json::from_value::<EncodeSettings>(wire).unwrap(),
            settings
        );
    }
    #[test]
    fn older_history_defaults_to_standalone_without_hdr_metadata_loss_or_added_grain() {
        let old: EncodeSettings =
            serde_json::from_str(r#"{"videoStreamIndex":2,"crf":26,"preset":6}"#).unwrap();
        assert_eq!(
            old,
            EncodeSettings {
                video_stream_index: 2,
                crf: 26,
                preset: 6,
                ..Default::default()
            }
        );
        let new = EncodeSettings {
            backend: EncodeBackend::Av1an,
            workers: 3,
            film_grain: 8,
            hdr10_fallback: true,
            ..old
        };
        let saved = serde_json::to_value(&new).unwrap();
        assert_eq!(saved["backend"], "av1an");
        assert_eq!(saved["hdr10Fallback"], true);
        assert_eq!(
            serde_json::from_value::<EncodeSettings>(saved).unwrap(),
            new
        );
    }

    #[test]
    fn legacy_standalone_backend_and_new_encoder_round_trip_without_ambiguity() {
        let old: EncodeSettings = serde_json::from_str(
            r#"{"backend":"svtAv1","videoStreamIndex":0,"crf":30,"preset":4}"#,
        )
        .unwrap();
        assert_eq!(old.backend, EncodeBackend::Standalone);
        assert_eq!(old.encoder, VideoEncoder::SvtAv1);
        let settings = EncodeSettings {
            encoder: VideoEncoder::X264,
            crf: 23,
            preset: 5,
            ..old
        };
        let value = serde_json::to_value(&settings).unwrap();
        assert_eq!(value["backend"], "standalone");
        assert_eq!(value["encoder"], "x264");
        assert_eq!(
            serde_json::from_value::<EncodeSettings>(value).unwrap(),
            settings
        );
    }

    #[test]
    fn fork_identity_and_tuning_survive_history_round_trip() {
        for (encoder, wire) in [
            (VideoEncoder::SvtAv1FiveFish, "svtAv1FiveFish"),
            (VideoEncoder::SvtAv1Hdr, "svtAv1Hdr"),
        ] {
            let settings = EncodeSettings {
                encoder,
                lineart_psy_bias: if encoder == VideoEncoder::SvtAv1FiveFish {
                    5
                } else {
                    0
                },
                texture_psy_bias: if encoder == VideoEncoder::SvtAv1FiveFish {
                    4
                } else {
                    0
                },
                hdr_tune: if encoder == VideoEncoder::SvtAv1Hdr {
                    HdrTune::FilmGrain
                } else {
                    HdrTune::VisualQuality
                },
                ..Default::default()
            };
            let value = serde_json::to_value(&settings).unwrap();
            assert_eq!(value["encoder"], wire);
            assert_eq!(
                serde_json::from_value::<EncodeSettings>(value).unwrap(),
                settings
            );
        }
    }
}
