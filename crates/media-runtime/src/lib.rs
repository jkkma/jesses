//! Local tool discovery, bounded media inspection, and supervised copy/remux jobs.
//!
//! PATH discovery is the initial development provider. Installed tool manifests,
//! durable job recovery, and encoding remain future adapters.

mod discovery;
pub mod jobs;
mod probe;
mod process;
pub mod supervisor;

pub use discovery::get_capabilities;
pub use jobs::JobManager;
pub use media_core::{AppError, MediaFile, MediaStream, ToolInfo};
pub use media_core::{EncodeRequest, EncodeSettings, JobSnapshot, JobState, RemuxRequest};
pub use probe::probe_media;
