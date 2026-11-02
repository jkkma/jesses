use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CommandEditRequest {
    Encode {
        request: Box<crate::EncodeRequest>,
    },
    Image {
        request: Box<crate::ImageRequest>,
    },
    ResumeSaved {
        id: String,
    },
    ResumeNew {
        id: String,
        settings: Box<crate::EncodeSettings>,
    },
}

/// Native preparation remains alive while this single-use review is open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CommandEditReview {
    pub id: String,
    pub executable_label: String,
    pub arguments: String,
    pub output_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub job: Option<crate::JobSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandEditConfirmation {
    pub id: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CommandEditResult {
    Encode { job: Box<crate::JobSnapshot> },
    Image { result: crate::ImageResult },
}

/// Confirmed user program. Preparation paths are accompanied by their generated
/// reference so an explicit recovery can rebind owned locations deterministically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditedCommand {
    pub generated: String,
    pub arguments: String,
}
