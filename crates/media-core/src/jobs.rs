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
    #[serde(default)]
    pub backend: EncodeBackend,
    #[serde(default = "default_workers")]
    pub workers: u8,
    pub video_stream_index: u32,
    pub crf: u8,
    pub preset: u8,
    /// AV1 grain synthesis strength; zero leaves synthesis disabled.
    #[serde(default)]
    pub film_grain: u8,
    /// Explicit consent to discard dynamic HDR metadata for an HDR10 base layer.
    #[serde(default)]
    pub hdr10_fallback: bool,
}

impl Default for EncodeSettings {
    fn default() -> Self {
        Self {
            backend: EncodeBackend::default(),
            workers: default_workers(),
            video_stream_index: 0,
            crf: 30,
            preset: 4,
            film_grain: 0,
            hdr10_fallback: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum EncodeBackend {
    #[default]
    SvtAv1,
    Av1an,
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
}
