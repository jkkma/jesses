use crate::{EncodeSettings, JobSnapshot};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A verified inactive workspace belonging to a durable Jesses job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Av1anRecoveryEntry {
    pub job: JobSnapshot,
    /// Decimal bytes avoid losing precision in JavaScript.
    pub encoded_bytes: String,
    pub chunk_count: u32,
    /// Unix milliseconds; old records with no known timestamp remain unknown.
    #[ts(type = "number | null")]
    pub created_at: Option<u64>,
    #[ts(type = "number | null")]
    pub last_run_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Av1anRecoveryAction {
    DeleteEncode {
        id: String,
    },
    DeleteChunks {
        id: String,
    },
    ResumeSaved {
        id: String,
    },
    ResumeNew {
        id: String,
        settings: Box<EncodeSettings>,
    },
    /// The exact reviewed set; later arrivals are not implicitly authorized.
    ClearAll {
        ids: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Av1anRecoveryActionResult {
    pub jobs: Vec<JobSnapshot>,
}
