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
    pub video_stream_index: u32,
    pub crf: u8,
    pub preset: u8,
}

impl Default for EncodeSettings {
    fn default() -> Self {
        Self {
            video_stream_index: 0,
            crf: 30,
            preset: 4,
        }
    }
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
