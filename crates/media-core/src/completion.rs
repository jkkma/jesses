use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FinishAction {
    #[default]
    None,
    CloseApp,
    Shutdown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CompletionOptions {
    pub notify: bool,
    pub finish_action: FinishAction,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CompletionStatus {
    pub options: CompletionOptions,
    pub armed_jobs: u32,
    pub seconds_remaining: Option<u32>,
    pub error: Option<String>,
}
