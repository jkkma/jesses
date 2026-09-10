//! Local discovery, folder previews, and supervised remux and AV1 encode jobs.
//!
//! PATH discovery is the initial development provider. Installed tool manifests,
//! automatic recovery, and broader codec workflows remain future adapters.

mod batch;
mod discovery;
pub mod jobs;
mod probe;
mod process;
pub mod supervisor;

pub use batch::scan_media_folder;
pub use discovery::get_capabilities;
pub use jobs::JobManager;
pub use media_core::{AppError, MediaFile, MediaStream, ToolInfo};
pub use media_core::{
    BatchEncodeInput, BatchEncodeItem, BatchEncodePreview, BatchEncodeRequest, FolderScanRequest,
    FolderScanResult,
};
pub use media_core::{EncodeRequest, EncodeSettings, JobSnapshot, JobState, RemuxRequest};
pub use probe::probe_media;
