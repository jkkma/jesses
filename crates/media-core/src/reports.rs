use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{BitrateRequest, BitrateResult, QualityRequest, QualityResult};

/// The request and result are captured together when an analysis completes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AnalysisReport {
    Bitrate {
        request: BitrateRequest,
        result: BitrateResult,
    },
    Quality {
        request: QualityRequest,
        result: QualityResult,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AnalysisExportFormat {
    Csv,
    Svg,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisExportRequest {
    pub output_path: String,
    pub format: AnalysisExportFormat,
    pub report: AnalysisReport,
}
