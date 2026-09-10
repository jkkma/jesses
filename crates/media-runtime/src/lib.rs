//! Read-only local tool discovery and bounded FFprobe inspection.
//!
//! PATH discovery is the initial development provider. Installed tool manifests,
//! job persistence, encoding, and process-tree supervision are future adapters.

mod discovery;
mod probe;
mod process;

pub use discovery::get_capabilities;
pub use media_core::{AppError, MediaFile, MediaStream, ToolInfo};
pub use probe::probe_media;
