use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SavedJobInspection {
    pub path: String,
    pub compatible: bool,
    pub message: String,
    pub request: Option<crate::EncodeRequest>,
}
