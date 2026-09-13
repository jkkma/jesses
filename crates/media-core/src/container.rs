use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The destination extension selects the container; the video/audio encoders
/// remain explicit independent choices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ContainerFormat {
    #[default]
    Matroska,
    Mp4,
    Mov,
    Webm,
}

impl ContainerFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Matroska => "mkv",
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Webm => "webm",
        }
    }

    pub fn ffmpeg_format(self) -> &'static str {
        match self {
            Self::Matroska => "matroska",
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Webm => "webm",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "mkv" => Some(Self::Matroska),
            "mp4" => Some(Self::Mp4),
            "mov" => Some(Self::Mov),
            "webm" => Some(Self::Webm),
            _ => None,
        }
    }
}
