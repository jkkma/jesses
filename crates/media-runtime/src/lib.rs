//! Local tool discovery, media inspection, and durable single-file encoding.
//!
//! PATH discovery is the initial development provider. Installed tool manifests,
//! and additional encoding providers remain future adapters.

mod discovery;
mod jobs;
mod probe;
mod process;
mod supervisor;

pub use discovery::get_capabilities;
pub use jobs::JobManager;
pub use media_core::{AppError, MediaFile, MediaStream, ToolInfo};
pub use probe::probe_media;
