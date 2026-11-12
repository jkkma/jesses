//! Local discovery, folder previews, and supervised remux and video encode jobs.
//!
//! PATH discovery is the initial development provider. Installed tool manifests,
//! automatic recovery, and broader codec workflows remain future adapters.

mod analysis;
mod av1an_resources;
pub use av1an_resources::estimate_av1an_resources;
mod batch;
mod output_resolution;
pub use output_resolution::resolve_output_path;
mod bitrate;
mod bundled_tools;
mod discovery;
mod loudness;
pub mod preferences;
mod quality;
pub use loudness::measure_loudness;
pub use quality::analyze_quality;
pub mod jobs;
mod probe;
mod process;
mod utilities;
pub use utilities::read_color_metadata;
pub use utilities::resolve_deinterlace;
pub use utilities::{UtilityOutputReservations, prepare_utility_outputs};
pub use utilities::{
    inspect_utility_capabilities, run_utility, run_utility_batch, with_utility_progress,
};
pub use utilities::{make_av1an_grain_preset, read_av1an_grain_table};
mod saved_jobs;
pub use saved_jobs::{
    export_saved_job, inspect_saved_job, inspect_saved_job_with_encoder,
    inspect_saved_job_with_media,
};
pub mod supervisor;

pub use analysis::{
    detect_crop, detect_crop_with_samples, detect_fields, preview_frame, snap_keyframe,
};
pub use batch::{
    resolve_batch_output_paths, resolve_batch_output_paths_cancellable, scan_media_folder,
};
pub use bitrate::analyze_bitrate;
pub use bundled_tools::configure_bundled_tools;
pub use discovery::get_capabilities;
pub use jobs::{JobManager, get_encoder_parameters, preview_encode_plan};
pub use jobs::{import_image_sequence, release_image_sequence};
pub use media_core::{AppError, MediaFile, MediaStream, ToolInfo};
pub use media_core::{
    AudioChannels, AudioCodec, AudioTrackSettings, BatchEncodeInput, BatchEncodeItem,
    BatchEncodePreview, BatchEncodeRequest, FolderScanRequest, FolderScanResult,
};
pub use media_core::{AutoCropRequest, AutoCropResult, FramePreviewRequest, FramePreviewResult};
pub use media_core::{
    Av1anRecovery, BorderSettings, CropSettings, EncodeBackend, EncodeRequest, EncodeSettings,
    EncodeTrackOverride, EncodeTrackRef, ExternalAudioSettings, ExternalTrack, HdrTune,
    JobSnapshot, JobState, RecoveryPhase, RemuxRequest, VideoEncoder, VideoFraming,
};
pub use media_core::{MuxRequest, MuxSource, MuxTrack};
pub use probe::{probe_media, probe_media_cancellable};
