//! Durable receipts for an explicitly resumed av1an job. A history locator is
//! never sufficient authority to run saved commands or remove a directory.
use super::*;
use crate::jobs::files::WorkspaceLock;
use crate::jobs::source_queue::{self, ChunkContract, Regions};
use media_core::{Av1anRecovery, RecoveryPhase, VideoEncoder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    sync::Mutex as StdMutex,
};

const LIMIT: usize = 16 * 1024 * 1024;
static WRITE_ID: AtomicU64 = AtomicU64::new(1);
#[cfg(test)]
type DigestReadHook = Box<dyn FnMut(&Path)>;
#[cfg(test)]
thread_local! {
    static REMOVE_ENUMERATED_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static CUT_SLOT_WRITE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static COUNT_SOURCE_HASHES: std::cell::RefCell<Option<(PathBuf, usize)>> = const { std::cell::RefCell::new(None) };
    static DIGEST_FIRST_READ_HOOK: std::cell::RefCell<Option<DigestReadHook>> = const { std::cell::RefCell::new(None) };
}

fn error(path: &Path, message: impl Into<String>) -> AppError {
    files::error("RECOVERY_INVALID", message, path)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Identity(Vec<u64>);

fn file_identity(file: &File) -> std::io::Result<Identity> {
    #[cfg(windows)]
    {
        let (a, b, c) = files::windows_file_id(file)?;
        Ok(Identity(vec![u64::from(a), u64::from(b), u64::from(c)]))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok(Identity(vec![metadata.dev(), metadata.ino()]))
    }
}

fn identity(path: &Path) -> Result<Identity, AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| error(path, e.to_string()))?;
    if metadata.file_type().is_symlink() {
        return Err(error(
            path,
            "Recovery files and directories cannot be links.",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(error(path, "Recovery paths cannot be reparse points."));
        }
        let file = OpenOptions::new()
            // Identity queries must also work for our exclusively held lock.
            // FILE_READ_ATTRIBUTES does not request access to the file data.
            .access_mode(0x80)
            .share_mode(1 | 2 | 4)
            .custom_flags(0x02000000 | 0x00200000)
            .open(path)
            .map_err(|e| error(path, e.to_string()))?;
        file_identity(&file).map_err(|e| error(path, e.to_string()))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Identity(vec![metadata.dev(), metadata.ino()]))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    path: PathBuf,
    identity: Identity,
    length: u64,
    sha256: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LaunchArtifact {
    path: PathBuf,
    identity: Identity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image: Option<Stamp>,
}

fn tool_contents_match(saved: &[Stamp], current: &[Stamp]) -> bool {
    saved.len() == current.len()
        && saved
            .iter()
            .zip(current)
            // Tool roles are positional: FFmpeg, FFprobe, encoder, av1an and
            // the optional preparation producer. Scoop may copy the same
            // package into a new version directory, changing both path and
            // filesystem identity without changing the selected tool bytes.
            .all(|(saved, current)| {
                saved.length == current.length && saved.sha256 == current.sha256
            })
}

fn digest(path: &Path, cancel: Option<&watch::Receiver<bool>>) -> Result<Stamp, AppError> {
    #[cfg(test)]
    COUNT_SOURCE_HASHES.with(|counter| {
        if let Some((counted, reads)) = counter.borrow_mut().as_mut()
            && counted == path
        {
            *reads += 1;
        }
    });
    let before = identity(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1 | 2); // Existing durable IVF handle is writable; source/tool guards still deny writes.
    }
    let mut file = options.open(path).map_err(|e| error(path, e.to_string()))?;
    let metadata = file.metadata().map_err(|e| error(path, e.to_string()))?;
    if !metadata.is_file() {
        return Err(error(path, "Expected a regular recovery file."));
    }
    let mut hasher = Sha256::new();
    let mut bytes = vec![0u8; 1024 * 1024];
    let mut length = 0;
    loop {
        if let Some(cancel) = cancel {
            check_cancel(cancel)?;
        }
        let read = file
            .read(&mut bytes)
            .map_err(|e| error(path, e.to_string()))?;
        if read == 0 {
            break;
        }
        #[cfg(test)]
        if length == 0 {
            DIGEST_FIRST_READ_HOOK.with(|hook| {
                if let Some(hook) = hook.borrow_mut().as_mut() {
                    hook(path);
                }
            });
        }
        length += read as u64;
        hasher.update(&bytes[..read]);
    }
    let after = file.metadata().map_err(|e| error(path, e.to_string()))?;
    if before != identity(path)?
        || metadata.len() != length
        || after.len() != length
        || metadata.modified().ok() != after.modified().ok()
    {
        return Err(error(
            path,
            "The file changed while its recovery fingerprint was calculated.",
        ));
    }
    Ok(Stamp {
        path: path.to_owned(),
        identity: before,
        length,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn copy_prepared(
    source: &Path,
    destination: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<Stamp, AppError> {
    // `Temporary` retains a writable identity guard until this handoff. A
    // second `Source` guard would deny that already-open handle on Windows, so
    // verify the file immediately before and after the bounded copy instead.
    let before = digest(source, Some(cancel))?;
    let mut input = File::open(source).map_err(|e| error(source, e.to_string()))?;
    with_owned_output(destination, |output| {
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            check_cancel(cancel)?;
            let read = input
                .read(&mut buffer)
                .map_err(|e| error(source, e.to_string()))?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .map_err(|e| error(destination, e.to_string()))?;
        }
        output
            .sync_all()
            .map_err(|e| error(destination, e.to_string()))?;
        if digest(source, Some(cancel))? != before {
            return Err(error(
                source,
                "The prepared source changed while it was copied into recovery.",
            ));
        }
        let copied = digest(destination, Some(cancel))?;
        if copied.length != before.length || copied.sha256 != before.sha256 {
            return Err(error(
                destination,
                "The durable prepared source differs from its verified input.",
            ));
        }
        Ok(copied)
    })
}

fn with_owned_output<T>(
    destination: &Path,
    operation: impl FnOnce(&mut File) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|e| error(destination, e.to_string()))?;
    let owned = file_identity(&output).map_err(|e| error(destination, e.to_string()))?;
    let result = operation(&mut output);
    if result.is_err() {
        // Remove only the exact create-new file. If its pathname was replaced,
        // fail closed and preserve the replacement.
        #[cfg(unix)]
        let guard = Some(output);
        #[cfg(not(unix))]
        let guard = {
            drop(output);
            None
        };
        let _ = remove_tree(destination, &owned, guard);
    }
    result
}

fn bytes(path: &Path) -> Result<Vec<u8>, AppError> {
    identity(path)?;
    let mut data = Vec::new();
    File::open(path)
        .and_then(|file| file.take(LIMIT as u64 + 1).read_to_end(&mut data))
        .map_err(|e| error(path, e.to_string()))?;
    if data.len() > LIMIT {
        return Err(error(path, "Recovery record exceeds its size limit."));
    }
    Ok(data)
}

fn atomic(path: &Path, data: &[u8]) -> Result<(), AppError> {
    if data.len() > LIMIT {
        return Err(error(path, "Recovery record exceeds its size limit."));
    }
    let temp = path.with_extension(format!(
        "next-{}-{}",
        std::process::id(),
        WRITE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| error(&temp, e.to_string()))?;
    let created = file_identity(&file).map_err(|e| error(&temp, e.to_string()))?;
    let result = (|| {
        file.write_all(data)
            .and_then(|()| file.sync_all())
            .map_err(|e| error(&temp, e.to_string()))?;
        drop(file);
        fs::rename(&temp, path).map_err(|e| error(path, e.to_string()))?;
        #[cfg(unix)]
        File::open(path.parent().expect("owned directory"))
            .and_then(|f| f.sync_all())
            .map_err(|e| error(path, e.to_string()))?;
        Ok(())
    })();
    if result.is_err() && identity(&temp).is_ok_and(|current| current == created) {
        // A failed write must not strand our partial receipt as a foreign
        // workspace artifact. Never adopt or remove another writer's file.
        let _ = fs::remove_file(&temp);
    }
    result
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Chunk {
    frames: u64,
    file: Stamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decoded_contract: Option<super::super::source_queue::ChunkContract>,
}

struct SourceSealBatch {
    manifest: Manifest,
    tools: [Source; 2],
    pending: usize,
}

impl SourceSealBatch {
    async fn prove<F: Future<Output = ()>>(
        &self,
        cancel: &watch::Receiver<bool>,
        mut report: impl FnMut(usize, usize) -> F,
    ) -> Result<BTreeMap<String, ChunkContract>, AppError> {
        let total = self.manifest.completed.len();
        let mut verified = total - self.pending;
        report(verified, total).await;
        let mut contracts = BTreeMap::new();
        for (name, chunk) in &self.manifest.completed {
            if chunk.decoded_contract.is_some() {
                continue;
            }
            check_cancel(cancel)?;
            let contract = super::super::encode::boxed(|| {
                source_queue::seal(
                    &self.tools[0].path,
                    &self.tools[1].path,
                    &chunk.file.path,
                    chunk.frames,
                    queue_settings(&self.manifest).encoder,
                    queue_count_policy(&self.manifest),
                    cancel,
                )
            })
            .await?;
            verify_chunk_contract(chunk, &contract)?;
            contracts.insert(name.clone(), contract);
            verified += 1;
            report(verified, total).await;
        }
        Ok(contracts)
    }
}

fn verify_chunk_contract(chunk: &Chunk, contract: &ChunkContract) -> Result<(), AppError> {
    if contract.source_frames != chunk.frames
        || contract.bytes != chunk.file.length
        || format!(
            "{:x}",
            sha2::digest::Output::<Sha256>::from(contract.file_sha256)
        ) != chunk.file.sha256
    {
        return Err(error(
            &chunk.file.path,
            "The decoded contract differs from its immutable source chunk receipt.",
        ));
    }
    Ok(())
}

pub(super) struct SourceJoin {
    pub(super) regions: Arc<Regions>,
    pub(super) rate: (u32, u32),
    pub(super) chunks: Vec<super::source_queue_concat::SealedChunk>,
    pub(super) method: media_core::Av1anConcatMethod,
    pub(super) source_rate: (u32, u32),
    pub(super) force_rate: bool,
}

pub(super) enum SourceJoinMode {
    Basic {
        frames: usize,
        rate: (u32, u32),
        method: media_core::Av1anConcatMethod,
    },
    Detailed {
        reason: &'static str,
    },
}

/// Immutable authority for an already sealed engine queue. New argv belongs
/// to Manifest.settings/params/filter; it cannot rewrite these old commands.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct QueueRecipe {
    source: Stamp,
    original_source: Option<Stamp>,
    tools: Vec<Stamp>,
    settings: EncodeSettings,
    params: Vec<String>,
    pixel_format: String,
    fps_num: u32,
    fps_den: u32,
    source_rate: (u32, u32),
    total_frames: u64,
    source_filter: Option<String>,
    /// Frozen per-chunk count policy, distinct from the current global concat flag.
    #[serde(default)]
    ignore_frame_mismatch: bool,
}

fn epoch_milliseconds() -> Option<u64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|value| u64::try_from(value.as_millis()).ok())
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Manifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edited_command: Option<media_core::EditedCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run_at: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    restart_pending: bool,
    /// Allows recovery from a crash between manifest reset and history commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_settings: Option<EncodeSettings>,
    version: u32,
    id: String,
    directory: Identity,
    request: RemuxRequest,
    settings: EncodeSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue_recipe: Option<QueueRecipe>,
    /// A source-created cut uses the same decoder pathname on New Settings,
    /// while immutable variants retain both old and new bytes for rollback.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    source_cut_slot: bool,
    /// Exact next immutable cut admitted before an owned in-place slot write.
    /// A crash can restore the committed variant without trusting partial bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_slot_write: Option<Stamp>,
    /// The source av1an reads. A frame-changing preprocessing workflow copies
    /// its verified lossless output to a stable path inside this workspace.
    source: Stamp,
    /// Selected non-video sources remain outside this owned workspace.
    #[serde(default)]
    external_sources: Vec<Stamp>,
    /// The immutable user source remains the authority for the request and the
    /// final audio/metadata mux when av1an reads a prepared video instead.
    #[serde(default)]
    original_source: Option<Stamp>,
    /// The immutable user file remains authority when source AV1AN reads a cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_cut_original: Option<Stamp>,
    /// Exact authorized New-settings digest; Saved can never authorize a cut replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_cut_rebind_pending: Option<String>,
    /// Recipe/source/tool fields only: completed chunk receipts are never resurrected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_cut_committed_recipe: Option<Box<CommandRecipe>>,
    /// Decoded-pixel identity of a freshly regenerated prepared source. This
    /// prevents old chunks from surviving a plugin/filter output change while
    /// remaining independent of Matroska IDs and path spelling.
    #[serde(default)]
    prepared_identity: Option<String>,
    tools: Vec<Stamp>,
    /// Sealed before any staged host executes. Names alone never own a launch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    launch_artifacts: Vec<LaunchArtifact>,
    params: Vec<String>,
    #[serde(default = "default_pixel_format")]
    pixel_format: String,
    fps_num: u32,
    fps_den: u32,
    total_frames: u64,
    #[serde(default)]
    source_filter: Option<String>,
    phase: RecoveryPhase,
    av1an_version: Option<String>,
    queue: Option<Stamp>,
    scenes: Option<Stamp>,
    script: Option<Stamp>,
    #[serde(default)]
    reader_cache: Option<Stamp>,
    #[serde(default)]
    concat_inputs: Vec<Stamp>,
    /// Quiescent metric/probe files are disposable, never executable receipts.
    /// Existing files must still match their sealed identities before removal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    target_artifacts: Vec<Stamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_scenes: Option<Stamp>,
    completed: BTreeMap<String, Chunk>,
    /// A workspace may retain changed-source or manually edited chunks even
    /// after a later attempt returns to the frozen queue settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_chunk_history_needs_proof: Option<bool>,
    #[serde(default)]
    segments: Vec<Stamp>,
    intermediate: Identity,
    final_video: Option<Stamp>,
}

fn is_source_cut(settings: &EncodeSettings) -> bool {
    settings.backend == media_core::EncodeBackend::Av1an
        && settings.source_trim
        && settings.trim.is_some()
}
fn source_workflow(settings: &EncodeSettings) -> bool {
    settings.backend == media_core::EncodeBackend::Av1an
        && settings
            .av1an_options
            .is_some_and(|options| options.source_workflow)
}
fn queue_settings(manifest: &Manifest) -> &EncodeSettings {
    manifest
        .queue_recipe
        .as_ref()
        .map_or(&manifest.settings, |recipe| &recipe.settings)
}
fn queue_rate(manifest: &Manifest) -> (u32, u32) {
    manifest
        .queue_recipe
        .as_ref()
        .map_or((manifest.fps_num, manifest.fps_den), |recipe| {
            (recipe.fps_num, recipe.fps_den)
        })
}
fn queue_frames(manifest: &Manifest) -> u64 {
    manifest
        .queue_recipe
        .as_ref()
        .map_or(manifest.total_frames, |recipe| recipe.total_frames)
}
fn queue_count_policy(manifest: &Manifest) -> super::super::source_queue::CountPolicy {
    if manifest
        .queue_recipe
        .as_ref()
        .is_some_and(|recipe| recipe.ignore_frame_mismatch)
    {
        super::super::source_queue::CountPolicy::SourceTypedResample
    } else {
        super::super::source_queue::CountPolicy::ExactSourceFrames
    }
}

fn source_recipe_changed(manifest: &Manifest, recipe: &QueueRecipe) -> bool {
    recipe.source != manifest.source
        || recipe.original_source != manifest.original_source
        || recipe.source_filter != manifest.source_filter
        || recipe.settings.encoder != manifest.settings.encoder
        || recipe.pixel_format != manifest.pixel_format
}

fn record_source_chunk_provenance(manifest: &mut Manifest) {
    if manifest.queue_recipe.as_ref().is_some_and(|recipe| {
        manifest.edited_command.is_some() || source_recipe_changed(manifest, recipe)
    }) {
        manifest.source_chunk_history_needs_proof = Some(true);
    }
}

fn source_concat_method(settings: &EncodeSettings) -> media_core::Av1anConcatMethod {
    if super::encoder::uses_mkvmerge(settings.encoder, settings) {
        media_core::Av1anConcatMethod::Mkvmerge
    } else {
        media_core::Av1anConcatMethod::Ffmpeg
    }
}
fn queue_options(manifest: &Manifest) -> media_core::Av1anOptions {
    super::options::effective_chunks(queue_settings(manifest), queue_rate(manifest), 0)
}
fn cut_slot(root: &Path, original: &Path) -> PathBuf {
    root.join(format!(
        "source-cut.{}",
        original
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("mkv")
    ))
}
fn write_cut_slot(
    source: &Stamp,
    path: &Path,
    owner: &Identity,
    cancel: Option<&watch::Receiver<bool>>,
) -> Result<Stamp, AppError> {
    use std::io::{Seek, SeekFrom};
    if identity(path)? != *owner || digest(&source.path, cancel)? != *source {
        return Err(error(
            path,
            "The immutable cut or owned decoder slot changed before its authorized write.",
        ));
    }
    let guard = Source::open(&source.path)?;
    let mut input =
        File::open(&source.path).map_err(|cause| error(&source.path, cause.to_string()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|cause| error(path, cause.to_string()))?;
    output
        .set_len(0)
        .and_then(|()| output.seek(SeekFrom::Start(0)))
        .map_err(|cause| error(path, cause.to_string()))?;
    let mut buffer = [0u8; 65536];
    loop {
        if let Some(cancel) = cancel {
            check_cancel(cancel)?;
        }
        let count = input
            .read(&mut buffer)
            .map_err(|cause| error(&source.path, cause.to_string()))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|cause| error(path, cause.to_string()))?;
        #[cfg(test)]
        if let Some(hook) = CUT_SLOT_WRITE_HOOK.with(|hook| hook.borrow_mut().take()) {
            hook();
        }
    }
    output
        .sync_all()
        .map_err(|cause| error(path, cause.to_string()))?;
    drop(output);
    guard.verify()?;
    let actual = digest(path, cancel)?;
    if actual.identity != *owner || actual.length != source.length || actual.sha256 != source.sha256
    {
        return Err(error(
            path,
            "The source decoder slot differs from its immutable cut receipt.",
        ));
    }
    Ok(actual)
}
fn settings_digest(settings: &EncodeSettings) -> Result<String, AppError> {
    serde_json::to_vec(settings)
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|cause| AppError::new("RECOVERY_INVALID", cause.to_string(), None))
}
fn verify_source_cut_original(
    manifest: &Manifest,
    cancel: Option<&watch::Receiver<bool>>,
) -> Result<(), AppError> {
    if let Some(original) = &manifest.source_cut_original
        && digest(&original.path, cancel)? != *original
    {
        return Err(error(
            &original.path,
            "The immutable original file changed; a copied-input job cannot adopt or resume this source.",
        ));
    }
    Ok(())
}

fn default_pixel_format() -> String {
    "yuv420p10le".into()
}

fn video_path(root: &Path, settings: &EncodeSettings) -> PathBuf {
    root.join(format!(
        "video.{}",
        super::encoder::video_extension(settings.encoder, settings)
    ))
}

fn owned_concat_output(root: &Path, output: &Path) -> Result<bool, AppError> {
    if !output.is_absolute()
        || output.file_name().is_none()
        || output
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Ok(false);
    }
    let Some(parent) = output.parent() else {
        return Ok(false);
    };
    let before = identity(parent)?;
    let canonical = parent
        .canonicalize()
        .map_err(|cause| error(parent, cause.to_string()))?;
    if !canonical.starts_with(root) {
        return Ok(false);
    }
    // AV1AN removes the Windows verbatim prefix for mkvmerge. Compare existing
    // directory identities instead, rejecting every intervening link/reparse.
    let owner = identity(root)?;
    let mut ancestor = parent;
    while identity(ancestor)? != owner {
        let Some(next) = ancestor.parent() else {
            return Ok(false);
        };
        ancestor = next;
    }
    if before != identity(parent)? || before != identity(&canonical)? {
        return Err(error(output, "The concatenation output directory changed."));
    }
    match fs::symlink_metadata(output) {
        Ok(metadata) => {
            identity(output)?;
            Ok(metadata.is_file())
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(cause) => Err(error(output, cause.to_string())),
    }
}

fn root_for(id: &str, request: &RemuxRequest) -> Result<PathBuf, AppError> {
    if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err(error(
            Path::new(&request.output_path),
            "Invalid recovery job identifier.",
        ));
    }
    let parent = Path::new(&request.output_path)
        .parent()
        .ok_or_else(|| error(Path::new(&request.output_path), "Missing output directory."))?;
    Ok(parent
        .canonicalize()
        .map_err(|e| error(parent, e.to_string()))?
        .join(format!(".jesses-{id}.av1an")))
}

fn load(
    id: &str,
    request: &RemuxRequest,
    settings: &EncodeSettings,
    locator: &Av1anRecovery,
) -> Result<(PathBuf, Manifest), AppError> {
    let root = root_for(id, request)?;
    if Path::new(&locator.workspace) != root {
        return Err(error(
            &root,
            "The recovery locator does not belong to this job's output directory.",
        ));
    }
    let owner = identity(&root)?;
    let manifest: Manifest = serde_json::from_slice(&bytes(&root.join("manifest.json"))?)
        .map_err(|e| error(&root, e.to_string()))?;
    if manifest.version != 1
        || manifest.id != id
        || manifest.directory != owner
        || manifest.request != *request
        || (manifest.settings != *settings
            && !(manifest.restart_pending && manifest.previous_settings.as_ref() == Some(settings)))
        || manifest.total_frames == 0
    {
        return Err(error(
            &root,
            "Recovery ownership, source selection, or settings do not match this job.",
        ));
    }
    validate_layout(&root, &manifest)?;
    for stamp in &manifest.concat_inputs {
        if digest(&stamp.path, None)? != *stamp {
            return Err(error(
                &stamp.path,
                "A saved concatenation control file changed.",
            ));
        }
    }
    Ok((root, manifest))
}

fn external_reader_cache_path(
    chunks: &Path,
    reader: super::recovery_receipts::ReaderCache,
) -> Result<PathBuf, AppError> {
    let split = chunks.join("split");
    identity(&split)?;
    let mut found = None;
    for entry in fs::read_dir(&split).map_err(|e| error(&split, e.to_string()))? {
        let path = entry.map_err(|e| error(&split, e.to_string()))?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("cache.ffindex") && !name.starts_with("cache.bsindex") {
            continue;
        }
        let valid = match reader {
            super::recovery_receipts::ReaderCache::Ffms2 => name == "cache.ffindex",
            super::recovery_receipts::ReaderCache::Bestsource => name
                .strip_prefix("cache.bsindex.")
                .and_then(|name| name.strip_suffix(".bsindex"))
                .is_some_and(|track| {
                    !track.is_empty() && track.bytes().all(|byte| byte.is_ascii_digit())
                }),
        };
        if !valid || !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            return Err(error(
                &path,
                "Unexpected external source-reader cache artifact.",
            ));
        }
        identity(&path)?;
        if found.replace(path).is_some() {
            return Err(error(
                &split,
                "Multiple external source-reader caches are present.",
            ));
        }
    }
    found.ok_or_else(|| error(&split, "The external source-reader cache is missing."))
}

fn validate_layout(root: &Path, manifest: &Manifest) -> Result<(), AppError> {
    validate_launch_layout(root, manifest)?;
    if source_workflow(&manifest.settings)
        && manifest.queue.is_some()
        && manifest.queue_recipe.is_none()
    {
        return Err(error(
            root,
            "The source queue has no sealed original recipe; New/Saved settings cannot infer authority from completion progress.",
        ));
    }
    if manifest.queue_recipe.as_ref().is_some_and(|recipe| {
        !source_workflow(&recipe.settings)
            || recipe.total_frames == 0
            || recipe.source.path != manifest.source.path
    }) {
        return Err(error(
            root,
            "The immutable source queue recipe has inconsistent source/ordinal authority.",
        ));
    }
    if manifest.target_artifacts.is_empty() != manifest.target_scenes.is_none() {
        return Err(error(
            root,
            "The metric target cache lacks its sealed scene authority.",
        ));
    }
    if !manifest.target_artifacts.is_empty() {
        let count = target_artifact_count(root, manifest)?;
        let mut seen = std::collections::BTreeSet::new();
        for stamp in &manifest.target_artifacts {
            let relative = stamp
                .path
                .strip_prefix(root)
                .ok()
                .and_then(Path::to_str)
                .map(|value| value.replace('\\', "/"));
            if relative
                .as_deref()
                .is_none_or(|relative| !is_target_artifact(relative, manifest, count))
                || !seen.insert(&stamp.path)
            {
                return Err(error(root, "Unexpected metric target artifact location."));
            }
        }
    }
    if manifest.concat_inputs.iter().any(|stamp| {
        stamp.path != root.join("chunks/options.json") && stamp.path != root.join("chunks/concat")
    }) {
        return Err(error(root, "Unexpected concatenation control location."));
    }
    if manifest.prepared_identity.is_some() {
        if manifest.source.path != root.join("prepared.mkv")
            || manifest.original_source.is_none()
            || manifest.source_filter.is_some()
        {
            return Err(error(
                root,
                "The prepared recovery source is outside its owned stable path or has inconsistent identity fields.",
            ));
        }
    } else if manifest.source_cut_slot {
        if manifest
            .original_source
            .as_ref()
            .is_none_or(|original| manifest.source.path != cut_slot(root, &original.path))
        {
            return Err(error(
                root,
                "The source cut decoder slot is outside its exact owned path.",
            ));
        }
    } else if manifest.original_source.is_some() {
        return Err(error(
            root,
            "An original-source recovery stamp is present without a prepared source.",
        ));
    }
    let expected = [
        ("chunks/chunks.json", manifest.queue.as_ref()),
        ("chunks/scenes.json", manifest.scenes.as_ref()),
        ("chunks/scenes.json", manifest.target_scenes.as_ref()),
        ("chunks/split/loadscript.vpy", manifest.script.as_ref()),
        (
            if super::encoder::video_extension(manifest.settings.encoder, &manifest.settings)
                == "ivf"
            {
                "video.ivf"
            } else {
                "video.mkv"
            },
            manifest.final_video.as_ref(),
        ),
    ];
    for (relative, stamp) in expected {
        if stamp.is_some_and(|stamp| stamp.path != root.join(relative)) {
            return Err(error(
                root,
                "A recovery artifact points outside its planned workspace location.",
            ));
        }
    }
    let options = queue_options(manifest);
    let expected_reader_cache = if manifest.queue.is_some() {
        super::recovery_receipts::reader_cache(options.chunk_method)
            .map(|reader| external_reader_cache_path(&root.join("chunks"), reader))
            .transpose()?
    } else {
        None
    };
    if manifest
        .reader_cache
        .as_ref()
        .zip(expected_reader_cache.as_ref())
        .is_some_and(|(stamp, expected)| stamp.path != *expected)
    {
        return Err(error(
            root,
            "The external source-reader cache points outside its exact owned path.",
        ));
    }
    for (index, segment) in manifest.segments.iter().enumerate() {
        if !matches!(
            options.chunk_method,
            media_core::Av1anChunkMethod::Hybrid | media_core::Av1anChunkMethod::Segment
        ) || (segment.path != root.join("chunks/split").join(format!("{index:05}.mkv"))
            && !(manifest.segments.len() == 1
                && index == 0
                && segment.path == root.join("chunks/split/0.mkv")))
        {
            return Err(error(root, "Unexpected source segment location."));
        }
    }
    if manifest.queue.is_some() != manifest.scenes.is_some()
        || (manifest.queue.is_some() && super::options::plugin(options).is_some())
            != manifest.script.is_some()
        || (manifest.queue.is_some() && expected_reader_cache.is_some())
            != manifest.reader_cache.is_some()
        || (manifest.queue.is_none() && !manifest.segments.is_empty())
        || (manifest.queue.is_none() && !manifest.completed.is_empty())
        || (manifest.phase == RecoveryPhase::Finalizing) != manifest.final_video.is_some()
    {
        return Err(error(
            root,
            "Incomplete or inconsistent recovery checkpoint.",
        ));
    }
    let mut completed = 0u64;
    for (name, chunk) in &manifest.completed {
        if name.len() != 5
            || !name.bytes().all(|b| b.is_ascii_digit())
            || chunk.file.path
                != root.join("chunks/encode").join(format!(
                    "{name}.{}",
                    super::encoder::chunk_extension(queue_settings(manifest).encoder)
                ))
            || chunk.frames == 0
            || chunk.frames > queue_frames(manifest)
            || (options.chunk_method != media_core::Av1anChunkMethod::Segment
                && options.maximum_chunk_frames > 0
                && chunk.frames > u64::from(options.maximum_chunk_frames))
            || chunk.file.length == 0
        {
            return Err(error(root, "Invalid completed chunk recovery record."));
        }
        completed = completed
            .checked_add(chunk.frames)
            .ok_or_else(|| error(root, "Invalid completed frame count."))?;
    }
    if completed > queue_frames(manifest) {
        return Err(error(root, "Completed chunk frames exceed the source."));
    }
    Ok(())
}

fn directory_guard(root: &Path) -> Result<File, AppError> {
    let mut directory = OpenOptions::new();
    directory.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        directory.share_mode(1 | 2).custom_flags(0x02000000);
    }
    let directory = directory
        .open(root)
        .map_err(|e| error(root, e.to_string()))?;
    Ok(directory)
}

pub(super) fn launch_directory_guard(path: &Path) -> Result<File, AppError> {
    let expected = identity(path)?;
    let guard = directory_guard(path)?;
    if !guard
        .metadata()
        .map_err(|e| error(path, e.to_string()))?
        .is_dir()
        || file_identity(&guard).map_err(|e| error(path, e.to_string()))? != expected
        || identity(path)? != expected
    {
        return Err(error(path, "The staged launch directory changed."));
    }
    Ok(guard)
}

fn lock_workspace(root: &Path) -> Result<(File, WorkspaceLock), AppError> {
    let directory = directory_guard(root)?;
    let lock_path = root.join("workspace.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let lock = options
        .open(&lock_path)
        .map_err(|e| error(&lock_path, format!("The recovery workspace is in use: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: lock owns a live file descriptor; WorkspaceLock releases it.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(error(
                &lock_path,
                "The recovery workspace is already in use.",
            ));
        }
    }
    Ok((directory, WorkspaceLock(lock)))
}

struct Workspace {
    root: PathBuf,
    manifest: Manifest,
    // av1an records the decoder's declared rate in its queue. The encoder and
    // durable manifest use the cadence independently proven by every timestamp.
    // Recompute this declaration from the immutable source on every attempt.
    source_rate: (u32, u32),
    segments_verified: bool,
    // Protect tools from modification during the resumed job on Windows.
    _tools: Vec<Source>,
    // Keep the exact source admitted into av1an immutable for the whole run.
    _source: Option<Source>,
    chunks: Vec<Source>,
    directory_guard: Option<File>,
    lock: Option<WorkspaceLock>,
}

#[derive(Clone)]
pub(in crate::jobs) struct Recovery {
    pub(in crate::jobs) root: PathBuf,
    pub(in crate::jobs) finalizing: bool,
    pub(in crate::jobs) resume_chunks: bool,
    inner: Arc<StdMutex<Workspace>>,
}

/// A freshly regenerated, fully validated lossless video source. Recovery
/// copies it under the durable workspace before av1an writes any receipts.
pub(in crate::jobs) struct PreparedSource {
    pub path: PathBuf,
    pub decoded_identity: String,
}

const VSHIP_ACTIVE_PREFIX: &str = "JESSES_VSHIP_VULKAN_ACTIVE_SHA256:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ScorerIdentity {
    Cpu,
    Vship { sha256: String },
}

impl ScorerIdentity {
    fn from_version(version: &str) -> Result<Self, String> {
        let mut markers = version
            .lines()
            .filter_map(|line| line.strip_prefix(VSHIP_ACTIVE_PREFIX));
        let Some(sha256) = markers.next() else {
            return Ok(Self::Cpu);
        };
        if markers.next().is_some()
            || sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("The saved Vship scorer identity is invalid.".into());
        }
        Ok(Self::Vship {
            sha256: sha256.to_owned(),
        })
    }
}

pub(super) fn version_with_vship(base: &str, sha256: &str) -> String {
    format!("{base}\n{VSHIP_ACTIVE_PREFIX}{sha256}")
}

fn source_matches_attempt(
    manifest: &Manifest,
    root: &Path,
    original_source: &Stamp,
    prepared: Option<&PreparedSource>,
    cancel: Option<&watch::Receiver<bool>>,
) -> Result<bool, AppError> {
    if manifest.source_cut_slot {
        Ok(prepared.is_none()
            && manifest.original_source.as_ref() == Some(original_source)
            && manifest.source.path == cut_slot(root, &original_source.path)
            && digest(&manifest.source.path, cancel)? == manifest.source)
    } else if let Some(prepared) = prepared {
        Ok(manifest.original_source.as_ref() == Some(original_source)
            && manifest.prepared_identity.as_deref() == Some(prepared.decoded_identity.as_str())
            && manifest.source.path == root.join("prepared.mkv")
            && digest(&manifest.source.path, cancel)? == manifest.source)
    } else {
        Ok(manifest.source == *original_source
            && manifest.original_source.is_none()
            && manifest.prepared_identity.is_none())
    }
}

fn external_source_stamps(
    settings: &EncodeSettings,
    cancel: Option<&watch::Receiver<bool>>,
) -> Result<Vec<Stamp>, AppError> {
    let mut paths = std::collections::BTreeSet::new();
    let mut guards = Vec::new();
    for path in super::super::external_tracks::additional_source_paths(settings) {
        let guard = Source::open(path)?;
        if paths.insert(crate::batch::path_key(&guard.path)) {
            guards.push(guard);
        }
    }
    guards.sort_by(|left, right| left.path.cmp(&right.path));
    guards
        .iter()
        .map(|guard| {
            let stamp = digest(&guard.path, cancel)?;
            guard.verify()?;
            Ok(stamp)
        })
        .collect()
}

fn verify_external_sources(
    manifest: &Manifest,
    settings: &EncodeSettings,
    cancel: Option<&watch::Receiver<bool>>,
) -> Result<(), AppError> {
    if manifest.external_sources != external_source_stamps(settings, cancel)? {
        return Err(error(
            Path::new(&manifest.request.output_path),
            "A selected external source changed or lacks a recovery fingerprint.",
        ));
    }
    Ok(())
}

impl Workspace {
    fn transaction(
        &mut self,
        update: impl FnOnce(&mut Self) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let saved = self.manifest.clone();
        let saved_guards = self.chunks.len();
        let result = update(self).and_then(|()| {
            if self.manifest == saved {
                Ok(())
            } else {
                self.save()
            }
        });
        if result.is_err() {
            self.manifest = saved;
            self.chunks.truncate(saved_guards);
        }
        result
    }
    fn verify_owner(&self) -> Result<(), AppError> {
        if identity(&self.root)? != self.manifest.directory {
            return Err(error(&self.root, "The recovery directory was replaced."));
        }
        Ok(())
    }
    fn save(&self) -> Result<(), AppError> {
        self.verify_owner()?;
        atomic(
            &self.root.join("manifest.json"),
            &serde_json::to_vec(&self.manifest).map_err(|e| error(&self.root, e.to_string()))?,
        )
    }
    fn summary(&self) -> Av1anRecovery {
        Av1anRecovery {
            workspace: self.root.to_string_lossy().into_owned(),
            phase: self.manifest.phase,
            completed_frames: if self.manifest.phase == RecoveryPhase::Finalizing {
                queue_frames(&self.manifest)
            } else {
                self.manifest.completed.values().map(|c| c.frames).sum()
            },
            total_frames: queue_frames(&self.manifest),
        }
    }
    fn receipts(&self) -> Result<super::recovery_receipts::Receipts, AppError> {
        let chunks = self.root.join("chunks");
        identity(&chunks)?;
        identity(&chunks.join("split"))?;
        let options = queue_options(&self.manifest);
        let script_bytes = if super::options::plugin(options).is_some() {
            bytes(&chunks.join("split/loadscript.vpy"))?
        } else {
            Vec::new()
        };
        let script_text =
            std::str::from_utf8(&script_bytes).map_err(|e| error(&chunks, e.to_string()))?;
        super::recovery_receipts::validate(
            &bytes(&chunks.join("chunks.json"))?,
            &bytes(&chunks.join("scenes.json"))?,
            &bytes(&chunks.join("done.json"))?,
            &super::recovery_receipts::Expected {
                source: self
                    .manifest
                    .queue_recipe
                    .as_ref()
                    .map_or(&self.manifest.source.path, |recipe| &recipe.source.path),
                chunks_directory: &chunks,
                video_params: self
                    .manifest
                    .queue_recipe
                    .as_ref()
                    .map_or(&self.manifest.params, |recipe| &recipe.params),
                encoder: queue_settings(&self.manifest).encoder,
                pixel_format: self
                    .manifest
                    .queue_recipe
                    .as_ref()
                    .map_or(&self.manifest.pixel_format, |recipe| &recipe.pixel_format),
                total_frames: queue_frames(&self.manifest),
                fps_num: queue_rate(&self.manifest).0,
                fps_den: queue_rate(&self.manifest).1,
                source_fps_num: self
                    .manifest
                    .queue_recipe
                    .as_ref()
                    .map_or(self.source_rate, |recipe| recipe.source_rate)
                    .0,
                source_fps_den: self
                    .manifest
                    .queue_recipe
                    .as_ref()
                    .map_or(self.source_rate, |recipe| recipe.source_rate)
                    .1,
                ignore_frame_mismatch: self.manifest.queue_recipe.as_ref().map_or_else(
                    || super::options::source_resamples(&self.manifest.settings, self.source_rate),
                    |recipe| recipe.ignore_frame_mismatch,
                ),
                script_text,
                options,
                source_filter: self
                    .manifest
                    .queue_recipe
                    .as_ref()
                    .map_or(self.manifest.source_filter.as_deref(), |recipe| {
                        recipe.source_filter.as_deref()
                    }),
            },
        )
        .map_err(|e| error(&chunks, e))
    }
    fn checkpoint(&mut self, required: bool) -> Result<(), AppError> {
        self.transaction(|workspace| workspace.checkpoint_candidate(required))
    }
    fn checkpoint_candidate(&mut self, required: bool) -> Result<(), AppError> {
        self.verify_owner()?;
        if self.manifest.phase == RecoveryPhase::Finalizing {
            return Ok(());
        }
        let receipts = match self.receipts() {
            Ok(v) => v,
            Err(_) if !required => return Ok(()),
            Err(e) => return Err(e),
        };
        if receipts.total_frames != queue_frames(&self.manifest)
            || receipts.completed_frames > receipts.total_frames
            || receipts.queued_chunks == 0
            || (required
                && (receipts.completed_frames != receipts.total_frames
                    || receipts.completed.len() != receipts.queued_chunks))
            || receipts
                .script_path
                .as_ref()
                .is_some_and(|path| *path != self.root.join("chunks/split/loadscript.vpy"))
        {
            return Err(error(&self.root, "Inconsistent av1an recovery receipts."));
        }
        for (path, old) in [
            ("chunks/chunks.json", &mut self.manifest.queue),
            ("chunks/scenes.json", &mut self.manifest.scenes),
        ] {
            let stamp = digest(&self.root.join(path), None)?;
            if old.as_ref().is_some_and(|old| old != &stamp) {
                return Err(error(
                    &stamp.path,
                    "Saved av1an commands or scenes changed.",
                ));
            }
            *old = Some(stamp);
        }
        if source_workflow(&self.manifest.settings) && self.manifest.queue_recipe.is_none() {
            self.manifest.queue_recipe = Some(QueueRecipe {
                source: self.manifest.source.clone(),
                original_source: self.manifest.original_source.clone(),
                tools: self.manifest.tools.clone(),
                settings: self.manifest.settings.clone(),
                params: self.manifest.params.clone(),
                pixel_format: self.manifest.pixel_format.clone(),
                fps_num: self.manifest.fps_num,
                fps_den: self.manifest.fps_den,
                source_rate: self.source_rate,
                total_frames: self.manifest.total_frames,
                source_filter: self.manifest.source_filter.clone(),
                ignore_frame_mismatch: super::options::source_resamples(
                    &self.manifest.settings,
                    self.source_rate,
                ),
            });
        }
        self.manifest.concat_inputs.clear();
        for relative in ["chunks/options.json", "chunks/concat"] {
            let path = self.root.join(relative);
            if path.exists() {
                if relative.ends_with("options.json") {
                    let values: Vec<String> = serde_json::from_slice(&bytes(&path)?)
                        .map_err(|cause| error(&path, cause.to_string()))?;
                    if values.first().map(String::as_str) != Some("-o")
                        || !values.get(1).map_or(Ok(false), |value| {
                            owned_concat_output(&self.root, Path::new(value))
                        })?
                        || values.last().map(String::as_str) != Some("]")
                    {
                        return Err(error(
                            &path,
                            "The concatenation options do not name an owned output.",
                        ));
                    }
                }
                self.manifest.concat_inputs.push(digest(&path, None)?);
            }
        }
        if let Some(path) = &receipts.script_path {
            let stamp = digest(path, None)?;
            if self
                .manifest
                .script
                .as_ref()
                .is_some_and(|old| old != &stamp)
            {
                return Err(error(path, "Saved av1an source script changed."));
            }
            self.manifest.script = Some(stamp);
        } else if self.manifest.script.is_some() {
            return Err(error(
                &self.root,
                "Unexpected saved source script for FFmpeg reader.",
            ));
        }
        if let Some(reader) = receipts.reader_cache {
            let path = external_reader_cache_path(&self.root.join("chunks"), reader)?;
            if let Some(saved) = &self.manifest.reader_cache {
                #[cfg(windows)]
                {
                    let canonical =
                        fs::canonicalize(&path).map_err(|e| error(&path, e.to_string()))?;
                    let guard = self
                        .chunks
                        .iter()
                        .find(|guard| guard.path == canonical)
                        .ok_or_else(|| {
                            error(&path, "The external source-reader cache guard is missing.")
                        })?;
                    guard.verify().map_err(|_| {
                        error(&path, "The saved external source-reader cache changed.")
                    })?;
                    let metadata = fs::metadata(&path).map_err(|e| error(&path, e.to_string()))?;
                    if identity(&path)? != saved.identity || metadata.len() != saved.length {
                        return Err(error(
                            &path,
                            "The saved external source-reader cache changed.",
                        ));
                    }
                }
                #[cfg(not(windows))]
                if digest(&path, None)?.ne(saved) {
                    return Err(error(
                        &path,
                        "The saved external source-reader cache changed.",
                    ));
                }
            } else {
                // Seal only a complete, readable index. On Windows this guard
                // prevents writes, deletion and replacement for the remainder
                // of the attempt, so periodic checkpoints need only verify its
                // identity and metadata. Unix retains full-digest verification.
                let guard = Source::open(&path)?;
                let stamp = digest(&path, None)?;
                if stamp.length == 0 {
                    return Err(error(&path, "The external source-reader cache is empty."));
                }
                self.manifest.reader_cache = Some(stamp);
                self.chunks.push(guard);
            }
        } else if self.manifest.reader_cache.is_some() {
            return Err(error(
                &self.root,
                "Unexpected external source-reader cache for this chunk method.",
            ));
        }
        if self.manifest.segments.is_empty() {
            for path in &receipts.segments {
                let guard = Source::open(path)?;
                self.manifest.segments.push(digest(path, None)?);
                self.chunks.push(guard);
            }
        } else if self
            .manifest
            .segments
            .iter()
            .map(|stamp| &stamp.path)
            .ne(receipts.segments.iter())
        {
            return Err(error(
                &self.root,
                "Saved hybrid source segment list changed.",
            ));
        }
        let encode = self.root.join("chunks/encode");
        if !receipts.completed.is_empty() {
            identity(&encode)?;
        }
        for chunk in receipts.completed {
            if self.manifest.completed.contains_key(&chunk.name) {
                continue;
            }
            let path = encode.join(format!(
                "{}.{}",
                chunk.name,
                super::encoder::chunk_extension(queue_settings(&self.manifest).encoder)
            ));
            // Once a completion receipt is observed, keep its file read-only
            // through concatenation. This closes the verification-to-reuse gap.
            let guard = Source::open(&path)?;
            let stamp = digest(&path, None)?;
            if stamp.length != chunk.size_bytes {
                return Err(error(
                    &path,
                    "Completed chunk length differs from its receipt.",
                ));
            }
            record_source_chunk_provenance(&mut self.manifest);
            self.manifest.completed.insert(
                chunk.name,
                Chunk {
                    decoded_contract: None,
                    frames: chunk.frames,
                    file: stamp,
                },
            );
            self.chunks.push(guard);
        }
        Ok(())
    }
    fn verify_saved(&self, cancel: Option<&watch::Receiver<bool>>) -> Result<(), AppError> {
        if let Some(cancel) = cancel {
            check_cancel(cancel)?;
        }
        if let Some(recipe) = &self.manifest.queue_recipe {
            for stamp in recipe.tools.iter().chain(std::iter::once(
                recipe.original_source.as_ref().unwrap_or(&recipe.source),
            )) {
                if digest(&stamp.path, cancel)? != *stamp {
                    return Err(error(
                        &stamp.path,
                        "An immutable source queue resource changed.",
                    ));
                }
            }
        }
        if self.manifest.source_cut_slot
            && self.manifest.source_slot_write.is_none()
            && digest(&self.manifest.source.path, cancel)? != self.manifest.source
        {
            return Err(error(
                &self.manifest.source.path,
                "The sealed source cut decoder slot changed.",
            ));
        }
        for stamp in self
            .manifest
            .queue
            .iter()
            .chain(self.manifest.scenes.iter())
            .chain(self.manifest.target_scenes.iter())
            .chain(self.manifest.script.iter())
            .chain(self.manifest.reader_cache.iter())
            .chain(self.manifest.segments.iter())
            .chain(self.manifest.completed.values().map(|c| &c.file))
            .chain(self.manifest.final_video.iter())
        {
            if digest(&stamp.path, cancel)? != *stamp {
                return Err(error(
                    &stamp.path,
                    "A saved recovery receipt or completed video artifact changed.",
                ));
            }
        }
        for stamp in &self.manifest.target_artifacts {
            match fs::symlink_metadata(&stamp.path) {
                Ok(_) if digest(&stamp.path, cancel)? != *stamp => {
                    return Err(error(&stamp.path, "A sealed metric target cache changed."));
                }
                Ok(_) => {}
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(cause) => return Err(error(&stamp.path, cause.to_string())),
            }
        }
        Ok(())
    }

    fn verify_source_seal(&self, cancel: &watch::Receiver<bool>) -> Result<(), AppError> {
        self.verify_owner()?;
        self.verify_saved(Some(cancel))?;
        if self.manifest.queue_recipe.is_none() {
            for tool in &self.manifest.tools {
                if digest(&tool.path, Some(cancel))? != *tool {
                    return Err(error(&tool.path, "A frozen source queue tool changed."));
                }
            }
        }
        Ok(())
    }

    fn prepare_source_seal(
        &self,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Option<SourceSealBatch>, AppError> {
        if !source_workflow(&self.manifest.settings) {
            return Ok(None);
        }
        check_cancel(cancel)?;
        let tools = self
            .manifest
            .queue_recipe
            .as_ref()
            .map_or(&self.manifest.tools, |recipe| &recipe.tools);
        let [ffmpeg, ffprobe, ..] = tools.as_slice() else {
            return Err(error(
                &self.root,
                "The saved source queue has no qualified FFmpeg pair.",
            ));
        };
        let pending = self
            .manifest
            .completed
            .values()
            .filter(|chunk| chunk.decoded_contract.is_none())
            .count();
        if pending == 0 {
            // The join and New Settings action verify their own saved inputs.
            return Ok(None);
        }
        // On Windows these handles prevent writes/deletion during proof. A
        // fresh content and identity check still brackets the whole batch.
        let tools = [Source::open(&ffmpeg.path)?, Source::open(&ffprobe.path)?];
        self.verify_source_seal(cancel)?;
        Ok(Some(SourceSealBatch {
            manifest: self.manifest.clone(),
            tools,
            pending,
        }))
    }

    fn commit_source_seal(
        &mut self,
        batch: &SourceSealBatch,
        contracts: BTreeMap<String, ChunkContract>,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        if self.manifest != batch.manifest {
            return Err(error(
                &self.root,
                "The saved source queue changed during decoded chunk verification.",
            ));
        }
        // Do not checkpoint a proof made against changed bytes or a stale
        // recipe. One transaction publishes the batch, leaving existing
        // completed chunks and proofs usable if verification is interrupted.
        self.verify_source_seal(cancel)?;
        let path = self.root.join("manifest.json");
        let saved: Manifest = serde_json::from_slice(&bytes(&path)?)
            .map_err(|cause| error(&path, cause.to_string()))?;
        if saved != batch.manifest {
            return Err(error(
                &path,
                "The durable source queue changed during verification.",
            ));
        }
        check_cancel(cancel)?;
        self.transaction(|workspace| {
            for (name, contract) in contracts {
                workspace
                    .manifest
                    .completed
                    .get_mut(&name)
                    .expect("unchanged batch manifest")
                    .decoded_contract = Some(contract);
            }
            Ok(())
        })
    }

    fn target_artifacts(&self) -> Result<Vec<PathBuf>, AppError> {
        let split = self.root.join("chunks/split");
        if !split
            .try_exists()
            .map_err(|cause| error(&split, cause.to_string()))?
        {
            return Ok(Vec::new());
        }
        identity(&split)?;
        let count = target_artifact_count(&self.root, &self.manifest)?;
        let mut paths = Vec::new();
        for entry in fs::read_dir(&split).map_err(|cause| error(&split, cause.to_string()))? {
            let path = entry
                .map_err(|cause| error(&split, cause.to_string()))?
                .path();
            let relative = format!(
                "chunks/split/{}",
                path.file_name().unwrap().to_string_lossy()
            );
            if is_target_artifact(&relative, &self.manifest, count) {
                identity(&path)?;
                if !fs::symlink_metadata(&path)
                    .map_err(|cause| error(&path, cause.to_string()))?
                    .is_file()
                {
                    return Err(error(
                        &path,
                        "A metric target artifact must be a regular file.",
                    ));
                }
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }

    fn seal_target_artifacts(&mut self) -> Result<(), AppError> {
        self.verify_owner()?;
        self.manifest.target_artifacts = self
            .target_artifacts()?
            .iter()
            .map(|path| digest(path, None))
            .collect::<Result<_, _>>()?;
        if !self.manifest.target_artifacts.is_empty() {
            let scenes = digest(&self.root.join("chunks/scenes.json"), None)?;
            if self
                .manifest
                .target_scenes
                .as_ref()
                .is_some_and(|saved| *saved != scenes)
            {
                return Err(error(&scenes.path, "The target scene map changed."));
            }
            self.manifest.target_scenes = Some(scenes);
        } else {
            self.manifest.target_scenes = None;
        }
        Ok(())
    }

    fn remove_target_artifacts(&mut self) -> Result<(), AppError> {
        self.verify_saved(None)?;
        for path in self.target_artifacts()? {
            self.verify_owner()?;
            remove_tree(&path, &identity(&path)?, None)?;
        }
        self.manifest.target_artifacts.clear();
        self.manifest.target_scenes = None;
        Ok(())
    }
    fn sanitize_done(&self) -> Result<(), AppError> {
        let path = self.root.join("chunks/done.json");
        let done: BTreeMap<_, _> = self
            .manifest
            .completed
            .iter()
            .map(|(name, chunk)| {
                (
                    name,
                    serde_json::json!({"frames":chunk.frames,"size_bytes":chunk.file.length}),
                )
            })
            .collect();
        atomic(&path,&serde_json::to_vec(&serde_json::json!({"frames":queue_frames(&self.manifest),"done":done,"audio_done":false})).expect("serializable receipts"))
    }

    fn prepare_video_only_resume(&self, queued_chunks: usize) -> Result<(), AppError> {
        self.verify_owner()?;
        let chunks = self.root.join("chunks");
        let expected = identity(&chunks)?;
        let _chunks_guard = directory_guard(&chunks)?;
        if identity(&chunks)? != expected {
            return Err(error(&chunks, "The chunk directory changed before resume."));
        }
        // av1an's FFmpeg concat enumerates every file in encode, rather than
        // consulting its queue. Never let an unrelated file enter that list.
        let encode = chunks.join("encode");
        match fs::symlink_metadata(&encode) {
            Ok(metadata) => {
                let expected_encode = identity(&encode)?;
                if !metadata.is_dir() {
                    return Err(error(&encode, "Expected a chunk output directory."));
                }
                let _encode_guard = directory_guard(&encode)?;
                if identity(&encode)? != expected_encode {
                    return Err(error(
                        &encode,
                        "The chunk output directory changed before resume.",
                    ));
                }
                for entry in fs::read_dir(&encode).map_err(|e| error(&encode, e.to_string()))? {
                    let path = entry.map_err(|e| error(&encode, e.to_string()))?.path();
                    identity(&path)?;
                    let valid_name =
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                let extension = format!(
                                    ".{}",
                                    super::encoder::chunk_extension(
                                        queue_settings(&self.manifest).encoder
                                    )
                                );
                                let Some(index) = name.strip_suffix(&extension) else {
                                    return false;
                                };
                                index.len() == 5
                                    && index.bytes().all(|b| b.is_ascii_digit())
                                    && index
                                        .parse::<usize>()
                                        .is_ok_and(|index| index < queued_chunks)
                            });
                    if !valid_name
                        || !fs::symlink_metadata(&path)
                            .map_err(|e| error(&path, e.to_string()))?
                            .is_file()
                    {
                        return Err(error(
                            &path,
                            "An unexpected file is present in the saved chunk output directory.",
                        ));
                    }
                }
            }
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(error(&encode, cause.to_string())),
        }
        // Older app attempts could leave a malformed attachment-only audio.mkv.
        // FFmpeg's no-stream failure leaves existing files untouched, and av1an
        // then blindly adds this stale file to concat. Remove only this exact
        // owned artifact; sources and completed video chunks remain untouched.
        let audio = chunks.join("audio.mkv");
        match fs::symlink_metadata(&audio) {
            Ok(metadata) => {
                let audio_identity = identity(&audio)?;
                if !metadata.is_file() {
                    return Err(error(&audio, "Expected a regular ignored audio artifact."));
                }
                self.verify_owner()?;
                if identity(&chunks)? != expected {
                    return Err(error(
                        &chunks,
                        "The chunk directory changed before audio cleanup.",
                    ));
                }
                remove_tree(&audio, &audio_identity, None)?;
            }
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(error(&audio, cause.to_string())),
        }
        Ok(())
    }
}

pub(in crate::jobs) struct OwnedRecovery {
    workspace: Workspace,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::jobs) struct CommandRecipe {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue_recipe: Option<QueueRecipe>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    source_cut_slot: bool,
    source: Stamp,
    original_source: Option<Stamp>,
    source_cut_original: Option<Stamp>,
    prepared_identity: Option<String>,
    tools: Vec<Stamp>,
    external_sources: Vec<Stamp>,
    settings: EncodeSettings,
    params: Vec<String>,
    pixel_format: String,
    fps_num: u32,
    fps_den: u32,
    total_frames: u64,
    source_filter: Option<String>,
    edited_command: Option<media_core::EditedCommand>,
}

fn target_artifact_count(root: &Path, manifest: &Manifest) -> Result<usize, AppError> {
    let options = queue_options(manifest);
    if options.target_quality.is_none() {
        return Ok(0);
    }
    let scenes = root.join("chunks/scenes.json");
    if !scenes
        .try_exists()
        .map_err(|cause| error(&scenes, cause.to_string()))?
    {
        return Ok(0);
    }
    super::recovery_receipts::target_scene_count(
        &bytes(&scenes)?,
        queue_frames(manifest),
        options.maximum_chunk_frames,
    )
    .map_err(|message| error(&scenes, message))
}

fn is_target_artifact(relative: &str, manifest: &Manifest, count: usize) -> bool {
    use media_core::Av1anTargetMetric;
    let options = queue_options(manifest);
    let Some(target) =
        super::options::effective_target(options, queue_settings(manifest).encoder, (1920, 1080))
    else {
        return false;
    };
    let Some(name) = relative.strip_prefix("chunks/split/") else {
        return false;
    };
    if let Some(index) = name.strip_suffix(".json") {
        return (target.metric == Av1anTargetMetric::Vmaf
            || (matches!(
                target.metric,
                Av1anTargetMetric::Xpsnr | Av1anTargetMetric::XpsnrWeighted
            ) && target.probing_rate == 1))
            && index
                .parse::<usize>()
                .is_ok_and(|value| value < count && value.to_string() == index);
    }
    let extension = match queue_settings(manifest).encoder {
        VideoEncoder::X264 => ".264",
        VideoEncoder::X265Standalone => ".hevc",
        _ => ".ivf",
    };
    let Some((index, quality)) = name
        .strip_prefix("v_")
        .and_then(|name| name.strip_suffix(extension))
        .and_then(|name| name.split_once('_'))
    else {
        return false;
    };
    let index_valid = index.len() == 5
        && index.bytes().all(|byte| byte.is_ascii_digit())
        && index.parse::<usize>().is_ok_and(|value| value < count);
    index_valid
        && quality.parse::<f32>().is_ok_and(|value| {
            let canonical = if value.fract().abs() < 1e-6 {
                format!("{value:.0}")
            } else {
                format!("{value:.2}")
            };
            value.is_finite()
                && value >= f32::from(target.minimum_crf)
                && value <= f32::from(target.maximum_crf)
                && canonical == quality
        })
}

fn segment_list_text(root: &Path, segments: &[Stamp]) -> Result<String, AppError> {
    let split = root.join("chunks/split");
    let mut text = String::from("ffconcat version 1.0\n");
    for (index, segment) in segments.iter().enumerate() {
        if segment.path != split.join(format!("{index:05}.mkv"))
            && !(segments.len() == 1 && index == 0 && segment.path == split.join("0.mkv"))
        {
            return Err(error(&segment.path, "Unexpected source segment location."));
        }
        if identity(&segment.path)? != segment.identity
            || !fs::symlink_metadata(&segment.path)
                .map_err(|cause| error(&segment.path, cause.to_string()))?
                .is_file()
        {
            return Err(error(
                &segment.path,
                "The saved source segment identity changed.",
            ));
        }
        let name = segment
            .path
            .file_name()
            .expect("validated source segment name");
        text.push_str(&format!("file chunks/split/{}\n", name.to_string_lossy()));
    }
    Ok(text)
}

fn validate_launch_layout(root: &Path, manifest: &Manifest) -> Result<(), AppError> {
    let mut seen = std::collections::BTreeSet::new();
    for launch in &manifest.launch_artifacts {
        let valid_name = launch
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("launch-"))
            .and_then(|name| name.split_once('-'))
            .is_some_and(|(pid, id)| {
                !pid.is_empty()
                    && !id.is_empty()
                    && pid.bytes().chain(id.bytes()).all(|b| b.is_ascii_digit())
            });
        if launch.path.parent() != Some(root)
            || !valid_name
            || !seen.insert(&launch.path)
            || launch.identity.0.len() != manifest.directory.0.len()
        {
            return Err(error(
                &launch.path,
                "Invalid sealed launch directory location or identity.",
            ));
        }
        if let Some(image) = &launch.image
            && (image.path != launch.path.join("av1an.exe")
                || image.identity.0.len() != manifest.directory.0.len()
                || manifest
                    .tools
                    .get(3)
                    .is_none_or(|tool| image.length != tool.length || image.sha256 != tool.sha256))
        {
            return Err(error(
                &image.path,
                "The staged launch image lacks its selected tool authority.",
            ));
        }
    }
    Ok(())
}

fn verify_launch_artifacts(root: &Path, manifest: &Manifest) -> Result<(), AppError> {
    validate_launch_layout(root, manifest)?;
    for launch in &manifest.launch_artifacts {
        match fs::symlink_metadata(&launch.path) {
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
            Err(cause) => return Err(error(&launch.path, cause.to_string())),
            Ok(metadata) if !metadata.is_dir() => {
                return Err(error(
                    &launch.path,
                    "The sealed launch directory changed type.",
                ));
            }
            Ok(_) => {}
        }
        let _guard = launch_directory_guard(&launch.path)?;
        if identity(&launch.path)? != launch.identity {
            return Err(error(
                &launch.path,
                "The sealed launch directory was replaced.",
            ));
        }
        if let Some(image) = &launch.image {
            match fs::symlink_metadata(&image.path) {
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(cause) => return Err(error(&image.path, cause.to_string())),
                Ok(_) if digest(&image.path, None)? != *image => {
                    return Err(error(
                        &image.path,
                        "The sealed staged launch image changed.",
                    ));
                }
                Ok(_) => {}
            }
        }
    }
    Ok(())
}

fn remove_empty_launch(path: &Path, expected: &Identity) -> Result<(), AppError> {
    let guard = launch_directory_guard(path)?;
    if identity(path)? != *expected {
        return Err(error(
            path,
            "The owned launch directory was replaced before cleanup.",
        ));
    }
    if fs::read_dir(path)
        .map_err(|e| error(path, e.to_string()))?
        .next()
        .is_some()
    {
        return Err(error(
            path,
            "Unexpected artifacts remain in the owned launch directory.",
        ));
    }
    #[cfg(windows)]
    {
        drop(guard);
        let id = &expected.0;
        files::windows_delete_owned(path, (id[0] as u32, id[1] as u32, id[2] as u32))
            .map_err(|e| error(path, e.to_string()))?;
    }
    #[cfg(unix)]
    {
        fs::remove_dir(path).map_err(|e| error(path, e.to_string()))?;
        drop(guard);
    }
    Ok(())
}

fn verify_action_tree(root: &Path, manifest: &Manifest) -> Result<(), AppError> {
    verify_launch_artifacts(root, manifest)?;
    let count = manifest
        .queue
        .as_ref()
        .map(|stamp| {
            bytes(&stamp.path)
                .and_then(|b| {
                    serde_json::from_slice::<serde_json::Value>(&b)
                        .map_err(|e| error(&stamp.path, e.to_string()))
                })
                .and_then(|v| {
                    v.as_array()
                        .map(Vec::len)
                        .ok_or_else(|| error(&stamp.path, "Invalid saved queue."))
                })
        })
        .transpose()?
        .unwrap_or(0);
    let target_count = target_artifact_count(root, manifest)?;
    fn visit(
        root: &Path,
        path: &Path,
        manifest: &Manifest,
        count: usize,
        target_count: usize,
    ) -> Result<(), AppError> {
        identity(path)?;
        for entry in fs::read_dir(path).map_err(|e| error(path, e.to_string()))? {
            let path = entry.map_err(|e| error(path, e.to_string()))?.path();
            let _ = identity(&path)?;
            let metadata = fs::symlink_metadata(&path).map_err(|e| error(&path, e.to_string()))?;
            let rel = path
                .strip_prefix(root)
                .map_err(|e| error(&path, e.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            let dirs = matches!(rel.as_str(), "chunks" | "chunks/encode" | "chunks/split");
            let owned_launch = manifest
                .launch_artifacts
                .iter()
                .any(|launch| launch.path == path);
            let launch_image = manifest
                .launch_artifacts
                .iter()
                .filter_map(|launch| launch.image.as_ref())
                .any(|image| image.path == path);
            let stamped = manifest
                .segments
                .iter()
                .chain(manifest.reader_cache.iter())
                .chain(manifest.script.iter())
                .chain(manifest.concat_inputs.iter())
                .any(|s| s.path == path);
            let segment_list = rel == "segments.ffconcat" && !manifest.segments.is_empty();
            if segment_list {
                let guard = Source::open(&path)?;
                let expected = segment_list_text(root, &manifest.segments)?;
                if bytes(&path)? != expected.as_bytes() {
                    return Err(error(&path, "Source segment verification list changed."));
                }
                guard.verify()?;
            }
            let control = segment_list
                || matches!(
                    rel.as_str(),
                    "manifest.json"
                        | "workspace.lock"
                        | "chunks/chunks.json"
                        | "chunks/scenes.json"
                        | "chunks/done.json"
                        | "chunks/audio.mkv"
                        | "chunks/log.log"
                        | "chunks/split/cache.lwi"
                );
            let video = path == video_path(root, queue_settings(manifest));
            let prepared = (manifest.prepared_identity.is_some()
                && path == root.join("prepared.mkv"))
                || (manifest.source_cut_slot && path == manifest.source.path);
            let attempt = rel.starts_with(&format!("attempt-{}-attempt-", manifest.id))
                && rel.ends_with(".partial.mkv")
                && !rel.contains('/');
            let chunk = rel.strip_prefix("chunks/encode/").is_some_and(|name| {
                name.strip_suffix(&format!(
                    ".{}",
                    super::encoder::chunk_extension(queue_settings(manifest).encoder)
                ))
                .is_some_and(|n| {
                    n.len() == 5
                        && n.bytes().all(|b| b.is_ascii_digit())
                        && n.parse::<usize>().is_ok_and(|n| n < count)
                })
            });
            let stats = rel.strip_prefix("chunks/split/").is_some_and(|n| {
                n.split_once("_fpf").is_some_and(|(index, suffix)| {
                    index.len() == 5
                        && index.bytes().all(|b| b.is_ascii_digit())
                        && index.parse::<usize>().is_ok_and(|n| n < count)
                        && matches!(suffix, "" | ".log" | ".mbtree" | ".cutree" | ".temp")
                })
            });
            if metadata.is_dir() {
                if !dirs && !owned_launch {
                    return Err(error(&path, "Foreign directory in recovery workspace."));
                }
                visit(root, &path, manifest, count, target_count)?;
            } else if !metadata.is_file()
                || !(launch_image
                    || stamped
                    || control
                    || video
                    || prepared
                    || attempt
                    || chunk
                    || stats
                    || is_target_artifact(&rel, manifest, target_count))
            {
                return Err(error(
                    &path,
                    "Foreign artifact in recovery workspace; no files were deleted.",
                ));
            }
        }
        Ok(())
    }
    visit(root, root, manifest, count, target_count)
}

impl OwnedRecovery {
    pub(in crate::jobs) fn open(
        snapshot: &media_core::JobSnapshot,
        require_source: bool,
    ) -> Result<Self, AppError> {
        let settings = snapshot
            .encode_settings
            .as_ref()
            .ok_or_else(|| error(Path::new(""), "Missing encode settings."))?;
        let locator = snapshot
            .recovery
            .as_ref()
            .ok_or_else(|| error(Path::new(""), "Missing AV1AN recovery."))?;
        let (root, manifest) = load(&snapshot.id, &snapshot.request, settings, locator)?;
        let (directory, lock) = lock_workspace(&root)?;
        let (_, current) = load(&snapshot.id, &snapshot.request, settings, locator)?;
        if current != manifest {
            return Err(error(&root, "Recovery changed while acquiring ownership."));
        }
        verify_action_tree(&root, &manifest)?;
        let workspace = Workspace {
            root,
            source_rate: (manifest.fps_num, manifest.fps_den),
            manifest,
            segments_verified: false,
            _tools: vec![],
            _source: None,
            chunks: vec![],
            directory_guard: Some(directory),
            lock: Some(lock),
        };
        workspace.verify_saved(None)?;
        let video = video_path(&workspace.root, queue_settings(&workspace.manifest));
        if identity(&video)? != workspace.manifest.intermediate {
            return Err(error(&video, "The owned video intermediate changed."));
        }
        if (workspace.manifest.prepared_identity.is_some() || workspace.manifest.source_cut_slot)
            && workspace.manifest.source_slot_write.is_none()
            && digest(&workspace.manifest.source.path, None)? != workspace.manifest.source
        {
            return Err(error(&workspace.root, "The owned prepared source changed."));
        }
        if require_source {
            verify_source_cut_original(&workspace.manifest, None)?;
            let original = workspace
                .manifest
                .original_source
                .as_ref()
                .unwrap_or(&workspace.manifest.source);
            if digest(&original.path, None)? != *original {
                return Err(error(
                    &original.path,
                    "The original source changed; resume is unavailable.",
                ));
            }
            if (workspace.manifest.prepared_identity.is_some()
                || workspace.manifest.source_cut_slot)
                && workspace.manifest.source_slot_write.is_none()
                && digest(&workspace.manifest.source.path, None)? != workspace.manifest.source
            {
                return Err(error(&workspace.root, "The prepared source changed."));
            }
            verify_external_sources(&workspace.manifest, settings, None)?;
        }
        Ok(Self { workspace })
    }
    pub(in crate::jobs) fn encoded_stats(&self) -> Result<(u32, u64), AppError> {
        let path = self.workspace.root.join("chunks/encode");
        let mut total = 0u64;
        let mut count = 0u32;
        if path.exists() {
            for entry in fs::read_dir(&path).map_err(|e| error(&path, e.to_string()))? {
                let path = entry.map_err(|e| error(&path, e.to_string()))?.path();
                identity(&path)?;
                let length = fs::metadata(&path)
                    .map_err(|e| error(&path, e.to_string()))?
                    .len();
                count = count
                    .checked_add(1)
                    .ok_or_else(|| error(&path, "Chunk count overflow."))?;
                total = total
                    .checked_add(length)
                    .ok_or_else(|| error(&path, "Encoded byte count overflow."))?;
            }
        }
        Ok((count, total))
    }
    pub(in crate::jobs) fn times(&self) -> (Option<u64>, Option<u64>) {
        (
            self.workspace.manifest.created_at,
            self.workspace.manifest.last_run_at,
        )
    }
    pub(in crate::jobs) fn summary(&self) -> Av1anRecovery {
        self.workspace.summary()
    }
    pub(in crate::jobs) fn settings(&self) -> &EncodeSettings {
        &self.workspace.manifest.settings
    }
    pub(in crate::jobs) fn source_variant_settings(&self) -> Vec<EncodeSettings> {
        let mut settings = vec![self.workspace.manifest.settings.clone()];
        if let Some(recipe) = &self.workspace.manifest.queue_recipe {
            settings.push(recipe.settings.clone());
        }
        settings
    }
    /// Only sealed file receipts enter this scan; done.json cannot introduce a
    /// file or frame count here. New settings never adopt an unproved chunk.
    pub(in crate::jobs) async fn seal_source_chunks(
        &mut self,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        if let Some(batch) = self.workspace.prepare_source_seal(cancel)? {
            let contracts = batch.prove(cancel, |_, _| std::future::ready(())).await?;
            self.workspace
                .commit_source_seal(&batch, contracts, cancel)?;
        }
        Ok(())
    }
    pub(in crate::jobs) fn command_recipe(&self) -> CommandRecipe {
        let manifest = &self.workspace.manifest;
        CommandRecipe {
            queue_recipe: manifest.queue_recipe.clone(),
            source_cut_slot: manifest.source_cut_slot,
            source: manifest.source.clone(),
            original_source: manifest.original_source.clone(),
            source_cut_original: manifest.source_cut_original.clone(),
            prepared_identity: manifest.prepared_identity.clone(),
            tools: manifest.tools.clone(),
            external_sources: manifest.external_sources.clone(),
            settings: manifest.settings.clone(),
            params: manifest.params.clone(),
            pixel_format: manifest.pixel_format.clone(),
            fps_num: manifest.fps_num,
            fps_den: manifest.fps_den,
            total_frames: manifest.total_frames,
            source_filter: manifest.source_filter.clone(),
            edited_command: manifest.edited_command.clone(),
        }
    }
    pub(in crate::jobs) fn restore_command_recipe(
        &mut self,
        recipe: CommandRecipe,
    ) -> Result<Av1anRecovery, AppError> {
        if self.workspace.manifest.source_cut_slot && recipe.source_cut_slot {
            let current = &self.workspace.manifest;
            if identity(&current.source.path)? != recipe.source.identity {
                return Err(error(
                    &current.source.path,
                    "The source decoder slot was replaced during review.",
                ));
            }
            if current.source_slot_write.is_none()
                && digest(&current.source.path, None)? != current.source
            {
                return Err(error(
                    &current.source.path,
                    "The sealed source decoder slot changed during review.",
                ));
            }
            let original = recipe
                .original_source
                .as_ref()
                .ok_or_else(|| {
                    error(
                        &current.source.path,
                        "The committed source decoder slot has no immutable variant.",
                    )
                })?
                .clone();
            if digest(&current.source.path, None).ok().as_ref() != Some(&recipe.source) {
                let path = current.source.path.clone();
                self.workspace.manifest.source_slot_write = Some(original.clone());
                self.workspace.save()?;
                let restored = write_cut_slot(&original, &path, &recipe.source.identity, None)?;
                self.workspace.manifest.source = restored;
                self.workspace.manifest.source_slot_write = None;
                self.workspace.save()?;
            }
        }
        self.new_settings(&recipe.settings)?;
        let manifest = &mut self.workspace.manifest;
        manifest.queue_recipe = recipe.queue_recipe;
        manifest.source_cut_slot = recipe.source_cut_slot;
        manifest.source_slot_write = None;
        manifest.source = recipe.source;
        manifest.original_source = recipe.original_source;
        manifest.source_cut_original = recipe.source_cut_original;
        manifest.prepared_identity = recipe.prepared_identity;
        manifest.tools = recipe.tools;
        manifest.external_sources = recipe.external_sources;
        if manifest.prepared_identity.is_some()
            && digest(&manifest.source.path, None).ok().as_ref() != Some(&manifest.source)
        {
            manifest.source = manifest.original_source.take().ok_or_else(|| {
                error(
                    &self.workspace.root,
                    "The prior prepared source has no immutable original authority.",
                )
            })?;
            manifest.prepared_identity = None;
        }
        manifest.source_cut_rebind_pending = None;
        manifest.source_cut_committed_recipe = None;
        manifest.previous_settings = None;
        manifest.restart_pending =
            !(source_workflow(&manifest.settings) && manifest.queue.is_some());
        manifest.params = recipe.params;
        manifest.pixel_format = recipe.pixel_format;
        manifest.fps_num = recipe.fps_num;
        manifest.fps_den = recipe.fps_den;
        manifest.total_frames = recipe.total_frames;
        manifest.source_filter = recipe.source_filter;
        manifest.edited_command = recipe.edited_command;
        self.workspace.save()?;
        Ok(self.workspace.summary())
    }
    pub(in crate::jobs) fn discard(mut self) -> Result<(), AppError> {
        let w = &mut self.workspace;
        w.verify_owner()?;
        verify_action_tree(&w.root, &w.manifest)?;
        drop(w.lock.take());
        remove_workspace_tree(&w.root, &w.manifest, w.directory_guard.take())
    }
    pub(in crate::jobs) fn reset_chunks(&mut self) -> Result<Av1anRecovery, AppError> {
        let w = &mut self.workspace;
        w.verify_owner()?;
        verify_action_tree(&w.root, &w.manifest)?;
        let encode = w.root.join("chunks/encode");
        if encode.exists() {
            remove_tree(&encode, &identity(&encode)?, None)?;
            fs::create_dir(&encode).map_err(|e| error(&encode, e.to_string()))?;
        }
        w.manifest.completed.clear();
        w.manifest.source_chunk_history_needs_proof = Some(false);
        w.manifest.phase = RecoveryPhase::Encoding;
        w.manifest.final_video = None;
        let video = video_path(&w.root, queue_settings(&w.manifest));
        if identity(&video)? != w.manifest.intermediate {
            return Err(error(&video, "Video intermediate changed."));
        }
        OpenOptions::new()
            .write(true)
            .open(&video)
            .and_then(|f| f.set_len(0))
            .map_err(|e| error(&video, e.to_string()))?;
        if w.manifest.queue.is_some() {
            w.sanitize_done()?;
        }
        w.save()?;
        Ok(w.summary())
    }
    pub(in crate::jobs) fn new_settings(
        &mut self,
        settings: &EncodeSettings,
    ) -> Result<Av1anRecovery, AppError> {
        let committed = self
            .workspace
            .manifest
            .source_cut_committed_recipe
            .as_deref()
            .cloned()
            .unwrap_or_else(|| self.command_recipe());
        if source_workflow(&self.workspace.manifest.settings)
            && source_workflow(settings)
            && self.workspace.manifest.queue.is_some()
        {
            let w = &mut self.workspace;
            w.verify_owner()?;
            w.verify_saved(None)?;
            verify_action_tree(&w.root, &w.manifest)?;
            if w.manifest.queue_recipe.is_none()
                || w.manifest
                    .completed
                    .values()
                    .any(|chunk| chunk.decoded_contract.is_none())
            {
                return Err(error(
                    &w.root,
                    "Source New Settings requires its sealed original queue and decoded completed-chunk contracts.",
                ));
            }
            w.manifest.source_cut_committed_recipe = Some(Box::new(committed));
            w.manifest.source_cut_rebind_pending = Some(settings_digest(settings)?);
            if w.manifest.previous_settings.is_none() {
                w.manifest.previous_settings = Some(w.manifest.settings.clone());
            }
            w.manifest.settings = settings.clone();
            w.manifest.edited_command = None;
            w.manifest.phase = RecoveryPhase::Encoding;
            w.manifest.final_video = None;
            w.manifest.restart_pending = true;
            w.save()?;
            return Ok(w.summary());
        }
        let source_cut_transaction = self.workspace.manifest.source_cut_original.is_some()
            || is_source_cut(&self.workspace.manifest.settings)
            || is_source_cut(settings);
        let cut_changed = (is_source_cut(&self.workspace.manifest.settings)
            || is_source_cut(settings))
            && (self.workspace.manifest.settings.trim != settings.trim
                || self.workspace.manifest.settings.source_trim != settings.source_trim
                || self.workspace.manifest.settings.video_stream_index
                    != settings.video_stream_index);
        self.reset_chunks()?;
        let w = &mut self.workspace;
        if source_cut_transaction {
            w.manifest.source_cut_committed_recipe = Some(Box::new(committed));
        }
        if cut_changed || w.manifest.source_cut_rebind_pending.is_some() {
            w.manifest.source_cut_rebind_pending = Some(settings_digest(settings)?);
        }
        let mut old_recipe = w.manifest.settings.clone();
        let mut new_recipe = settings.clone();
        for recipe in [&mut old_recipe, &mut new_recipe] {
            recipe.crf = 0;
            recipe.preset = 0;
            recipe.svt_crf_quarter_steps = None;
            recipe.svt_preset = None;
            recipe.parameters.clear();
            recipe.workers = 1;
        }
        let compatible = old_recipe == new_recipe;
        let chunks = w.root.join("chunks");
        if chunks.exists() {
            remove_tree(&chunks, &identity(&chunks)?, None)?;
        }
        let old_video = video_path(&w.root, queue_settings(&w.manifest));
        let next_video = video_path(&w.root, settings);
        if old_video != next_video {
            remove_tree(&old_video, &w.manifest.intermediate, None)?;
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&next_video)
                .map_err(|e| error(&next_video, e.to_string()))?;
            drop(file);
            w.manifest.intermediate = identity(&next_video)?;
        }
        if !compatible && w.manifest.original_source.is_some() {
            let prepared = w.root.join("prepared.mkv");
            remove_tree(&prepared, &identity(&prepared)?, None)?;
            w.manifest.source = w.manifest.original_source.take().expect("saved original");
            w.manifest.prepared_identity = None;
        }
        if w.manifest.previous_settings.is_none() {
            w.manifest.previous_settings = Some(w.manifest.settings.clone());
        }
        w.manifest.settings = settings.clone();
        w.manifest.edited_command = None;
        w.manifest.queue = None;
        w.manifest.scenes = None;
        w.manifest.script = None;
        w.manifest.reader_cache = None;
        w.manifest.concat_inputs.clear();
        w.manifest.target_artifacts.clear();
        w.manifest.target_scenes = None;
        w.manifest.segments.clear();
        w.manifest.av1an_version = None;
        w.manifest.restart_pending = true;
        w.save()?;
        Ok(w.summary())
    }
}

impl Recovery {
    pub(super) async fn seal_source_chunks<F: Future<Output = ()> + Send>(
        &self,
        cancel: &watch::Receiver<bool>,
        report: impl FnMut(usize, usize) -> F + Send,
    ) -> Result<(), AppError> {
        let preparing = cancel.clone();
        let batch = self
            .with(move |workspace| workspace.prepare_source_seal(&preparing))
            .await?;
        if let Some(batch) = batch {
            let contracts = batch.prove(cancel, report).await?;
            let cancel = cancel.clone();
            self.with(move |workspace| workspace.commit_source_seal(&batch, contracts, &cancel))
                .await?;
        }
        Ok(())
    }
    pub(in crate::jobs) async fn queue_frame_count(&self) -> Result<usize, AppError> {
        self.with(|workspace| {
            usize::try_from(queue_frames(&workspace.manifest)).map_err(|_| {
                error(
                    &workspace.root,
                    "The source queue frame count exceeds this platform.",
                )
            })
        })
        .await
    }

    pub(super) async fn source_join_mode(
        &self,
        requires_clock: bool,
    ) -> Result<Option<SourceJoinMode>, AppError> {
        self.with(move |workspace| {
            let manifest = &workspace.manifest;
            if !source_workflow(&manifest.settings) {
                return Ok(None);
            }
            workspace.verify_owner()?;
            let recipe = manifest.queue_recipe.as_ref().ok_or_else(|| {
                error(&workspace.root, "The joined source video has no frozen queue.")
            })?;
            if manifest
                .completed
                .values()
                .try_fold(0u64, |total, chunk| total.checked_add(chunk.frames))
                != Some(recipe.total_frames)
                || manifest.completed.is_empty()
            {
                return Err(error(
                    &workspace.root,
                    "The completed chunk receipts do not cover the frozen source queue.",
                ));
            }
            let reason = if super::super::encode::thorough_verification(&manifest.settings) {
                Some("Thorough verification is enabled.")
            } else if requires_clock {
                Some("The original Dolby Vision base-layer clock needs decoded timing verification.")
            } else if recipe.ignore_frame_mismatch
                || super::options::source_resamples(&manifest.settings, workspace.source_rate)
            {
                Some("The source queue resamples frames and needs its actual decoded count and clock.")
            } else if source_recipe_changed(manifest, recipe)
                || manifest.source_chunk_history_needs_proof.unwrap_or_else(|| {
                    // Old workspaces with contracts do not record whether an
                    // earlier New Settings attempt changed the retained video.
                    manifest.completed.values().any(|chunk| chunk.decoded_contract.is_some())
                })
            {
                Some("The saved queue may contain chunks from a changed source recipe or an edited command.")
            } else {
                None
            };
            if let Some(reason) = reason {
                return Ok(Some(SourceJoinMode::Detailed { reason }));
            }
            let frames = usize::try_from(recipe.total_frames).map_err(|_| {
                error(&workspace.root, "The source queue frame count exceeds this platform.")
            })?;
            Ok(Some(SourceJoinMode::Basic { frames, rate: queue_rate(manifest), method: source_concat_method(&manifest.settings) }))
        })
        .await
    }
    pub(super) async fn source_join_inputs(
        &self,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Option<SourceJoin>, AppError> {
        let cancel = cancel.clone();
        self.with(move |workspace| {
            if !source_workflow(&workspace.manifest.settings) {
                return Ok(None);
            }
            workspace.verify_source_seal(&cancel)?;
            let mut chunks = Vec::with_capacity(workspace.manifest.completed.len());
            let mut contracts = Vec::with_capacity(chunks.capacity());
            for chunk in workspace.manifest.completed.values() {
                let contract = chunk.decoded_contract.as_ref().ok_or_else(|| {
                    error(
                        &chunk.file.path,
                        "The source join needs a decoded contract for every sealed chunk.",
                    )
                })?;
                verify_chunk_contract(chunk, contract)?;
                chunks.push(super::source_queue_concat::SealedChunk {
                    path: chunk.file.path.clone(),
                    bytes: contract.bytes,
                    sha256: contract.file_sha256,
                    source_frames: contract.source_frames,
                    decoded_frames: contract.frames,
                });
                contracts.push(contract.clone());
            }
            let regions = Regions::new(contracts)?;
            if regions.source_frames() != queue_frames(&workspace.manifest) {
                return Err(error(
                    &workspace.root,
                    "Decoded source chunks do not cover the complete frozen source ordinals.",
                ));
            }
            let settings = &workspace.manifest.settings;
            let method = source_concat_method(settings);
            Ok(Some(SourceJoin {
                regions,
                rate: queue_rate(&workspace.manifest),
                chunks,
                method,
                source_rate: workspace.source_rate,
                force_rate: !super::options::source_resamples(settings, workspace.source_rate),
            }))
        })
        .await
    }
    pub(in crate::jobs) async fn edited_command(
        &self,
    ) -> Result<Option<media_core::EditedCommand>, AppError> {
        self.with(|workspace| Ok(workspace.manifest.edited_command.clone()))
            .await
    }

    pub(in crate::jobs) async fn commit_source_cut_recipe(&self) -> Result<(), AppError> {
        self.with(|workspace| {
            workspace.transaction(|workspace| {
                if workspace
                    .manifest
                    .source_cut_rebind_pending
                    .as_ref()
                    .is_some_and(|authorized| {
                        settings_digest(&workspace.manifest.settings).as_ref().ok()
                            != Some(authorized)
                    })
                {
                    return Err(error(
                        &workspace.root,
                        "The pending cut no longer matches the authorized new settings.",
                    ));
                }
                workspace.manifest.source_cut_rebind_pending = None;
                workspace.manifest.source_cut_committed_recipe = None;
                Ok(())
            })
        })
        .await
    }
    pub(in crate::jobs) async fn confirm_edited_command(
        &self,
        command: Option<media_core::EditedCommand>,
    ) -> Result<(), AppError> {
        self.with(move |workspace| {
            workspace.transaction(|workspace| {
                if let Some(command) = &command {
                    if let Some(params) = crate::jobs::command_edit::rebind::argument(
                        &command.arguments,
                        "av1an",
                        "--video-params",
                    )? {
                        workspace.manifest.params =
                            crate::jobs::command_edit::rebind::words(&params)?;
                    }
                    if let Some(format) = crate::jobs::command_edit::rebind::argument(
                        &command.arguments,
                        "av1an",
                        "--pix-format",
                    )? {
                        workspace.manifest.pixel_format = format;
                    }
                    workspace.manifest.source_filter = crate::jobs::command_edit::rebind::argument(
                        &command.arguments,
                        "av1an",
                        "--ffmpeg",
                    )?
                    .and_then(|value| value.strip_prefix("-vf ").map(str::to_owned));
                }
                workspace.manifest.edited_command = command;
                workspace.manifest.source_cut_rebind_pending = None;
                workspace.manifest.source_cut_committed_recipe = None;
                Ok(())
            })
        })
        .await
    }
    /// Delete only the workspace bound to this saved job. Discarding progress
    /// intentionally does not read the original media or installed tools: a
    /// missing source must not prevent removal of an owned recovery directory.
    #[cfg(test)]
    pub(in crate::jobs) async fn discard(
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        locator: &Av1anRecovery,
    ) -> Result<(), AppError> {
        let (id, request, settings, locator) = (
            id.to_owned(),
            request.clone(),
            settings.clone(),
            locator.clone(),
        );
        let locator_path = PathBuf::from(&locator.workspace);
        tokio::task::spawn_blocking(move || {
            let (root, manifest) = load(&id, &request, &settings, &locator)?;
            let (directory, lock) = lock_workspace(&root)?;
            // The lock acquisition may have raced a workspace replacement.
            let (_, current) = load(&id, &request, &settings, &locator)?;
            if current.directory != manifest.directory || identity(&root)? != manifest.directory {
                return Err(error(
                    &root,
                    "The recovery directory changed before discard.",
                ));
            }
            drop(lock);
            remove_workspace_tree(&root, &manifest, Some(directory))
        })
        .await
        .map_err(|cause| error(&locator_path, cause.to_string()))?
    }

    pub(in crate::jobs) fn source_filter(&self) -> Result<Option<String>, AppError> {
        self.inner
            .lock()
            .map(|workspace| workspace.manifest.source_filter.clone())
            .map_err(|_| AppError::new("RECOVERY_INVALID", "Recovery state lock failed.", None))
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::jobs) async fn prepare(
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        source: &Path,
        prepared: Option<PreparedSource>,
        tools: Vec<PathBuf>,
        plan: &Plan,
        source_rate: (u32, u32),
        frame_count: usize,
        locator: Option<Av1anRecovery>,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(Self, Temporary, PathBuf), AppError> {
        let (id, request, settings, source, cancel) = (
            id.to_owned(),
            request.clone(),
            settings.clone(),
            source.to_owned(),
            cancel.clone(),
        );
        let params = super::encoder::parameters(plan, &settings)
            .into_iter()
            .map(|v| v.into_string().expect("encoder ASCII params"))
            .collect::<Vec<_>>();
        let (fps_num, fps_den) = (plan.fps_num, plan.fps_den);
        let pixel_format = plan.output_pixel_format.to_owned();
        let source_filter =
            if prepared.is_some() && settings.source_filters && plan.requires_qtgmc() {
                plan.source_filter_rows_only()
            } else {
                prepared
                    .is_none()
                    .then(|| plan.av1an_decoder_filter())
                    .flatten()
            };
        tokio::task::spawn_blocking(move || {
            let original_source = digest(&source, Some(&cancel))?;
            let source_cut_original=if is_source_cut(&settings){Some(digest(Path::new(&request.input_path),Some(&cancel))?)}else{None};
            if let Some(original)=&source_cut_original{super::super::source_normalization::av1an_trim::verify_variant(&id,&request,&settings,&source,&original.sha256,original.length,&original_source.sha256,original_source.length)?;}
            let external_sources = external_source_stamps(&settings, Some(&cancel))?;
            let mut tool_guards = tools
                .iter()
                .map(|path| Source::open(path))
                .collect::<Result<Vec<_>, _>>()?;
            let tool_stamps = tool_guards
                .iter()
                .map(|tool| digest(&tool.path, Some(&cancel)))
                .collect::<Result<Vec<_>, _>>()?;
            let mut preparation_lock=None;
            let (root, mut manifest, intermediate) = if let Some(locator) = locator {
                let (root, mut manifest) = load(&id, &request, &settings, &locator)?;
                if source_workflow(&settings)&&manifest.queue_recipe.is_some() {
                    let owned=lock_workspace(&root)?;let(_,current)=load(&id,&request,&settings,&locator)?;
                    if current!=manifest{return Err(error(&root,"The source queue changed while acquiring its preparation lock."));}preparation_lock=Some(owned);
                    if let Some(recipe)=&manifest.queue_recipe {
                        for tool in &recipe.tools {if digest(&tool.path,Some(&cancel))?!=*tool{return Err(error(&tool.path,"A frozen source queue tool changed."));}tool_guards.push(Source::open(&tool.path)?);}
                    }
                }
                if manifest.restart_pending&&source_workflow(&settings)&&manifest.queue_recipe.is_some() {
                    if manifest.source_cut_rebind_pending.as_ref()!=Some(&settings_digest(&settings)?) {
                        return Err(error(&root,"The retained source queue is not authorized for these exact new settings."));
                    }
                    verify_source_cut_original(&manifest,Some(&cancel))?;
                    if manifest.source_cut_slot&&is_source_cut(&settings) {
                        if source_cut_original.as_ref().is_none_or(|original|manifest.source_cut_original.as_ref()!=Some(original)) {return Err(error(&root,"The original source changed before the authorized cut-slot replacement."));}
                        if manifest.original_source.as_ref()!=Some(&original_source) {
                            if digest(&manifest.source.path,Some(&cancel))?!=manifest.source {return Err(error(&manifest.source.path,"The owned cut decoder slot changed before New Settings."));}
                            manifest.source_slot_write=Some(original_source.clone());
                            atomic(&root.join("manifest.json"),&serde_json::to_vec(&manifest).map_err(|cause|error(&root,cause.to_string()))?)?;
                            manifest.source=write_cut_slot(&original_source,&manifest.source.path,&manifest.source.identity,Some(&cancel))?;
                            manifest.original_source=Some(original_source.clone());manifest.source_slot_write=None;
                        }
                    }
                    manifest.settings=settings.clone();manifest.previous_settings=None;
                    manifest.external_sources=external_sources.clone();manifest.tools=tool_stamps.clone();
                    manifest.params=params.clone();manifest.pixel_format=pixel_format.clone();
                    manifest.fps_num=fps_num;manifest.fps_den=fps_den;manifest.total_frames=frame_count as u64;
                    manifest.source_filter=source_filter.clone();manifest.restart_pending=false;
                    atomic(&root.join("manifest.json"),&serde_json::to_vec(&manifest).map_err(|cause|error(&root,cause.to_string()))?)?;
                } else if manifest.restart_pending {
                    let previous_video=video_path(&root,&manifest.settings);let requested_video=video_path(&root,&settings);
                    if previous_video!=requested_video {
                        remove_tree(&previous_video,&manifest.intermediate,None)?;
                        let file=OpenOptions::new().write(true).create_new(true).open(&requested_video).map_err(|e|error(&requested_video,e.to_string()))?;
                        drop(file);manifest.intermediate=identity(&requested_video)?;
                    }
                    manifest.settings=settings.clone();manifest.previous_settings=None;
                    verify_source_cut_original(&manifest,Some(&cancel))?;
                    let authorized_cut=if let Some(authorized)=&manifest.source_cut_rebind_pending {if *authorized!=settings_digest(&settings)?{return Err(error(&root,"The pending source-cut replacement is not authorized for these exact new settings."));}true}else{false};
                    let saved_original = manifest.original_source.as_ref().unwrap_or(&manifest.source);
                    if saved_original != &original_source && !authorized_cut { return Err(error(&root, "The original source changed before new-settings resume.")); }
                    if authorized_cut&&is_source_cut(&settings)&&source_cut_original.is_none(){return Err(error(&root,"The new source-cut reader lacks immutable original authority."));}
                    if let Some(original)=&source_cut_original {if manifest.source_cut_original.as_ref().is_some_and(|saved|saved!=original){return Err(error(&root,"The original user file changed before source-cut adoption."));}manifest.source_cut_original=Some(original.clone());}
                    let (next_source, original, decoded) = if let Some(prepared) = &prepared {
                        let stable = root.join("prepared.mkv");
                        let reusable = manifest.prepared_identity.as_deref() == Some(prepared.decoded_identity.as_str())
                            && manifest.source.path == stable && digest(&stable, Some(&cancel))? == manifest.source;
                        let stamp = if reusable { manifest.source.clone() } else {
                            if stable.exists() { remove_tree(&stable, &identity(&stable)?, None)?; }
                            copy_prepared(&prepared.path, &stable, &cancel)?
                        };
                        (stamp, Some(original_source.clone()), Some(prepared.decoded_identity.clone()))
                    } else {
                        let stable = root.join("prepared.mkv");
                        if stable.exists() { remove_tree(&stable, &identity(&stable)?, None)?; }
                        (original_source.clone(), None, None)
                    };
                    manifest.source = next_source; manifest.original_source = original; manifest.prepared_identity = decoded;
                    manifest.external_sources = external_sources.clone(); manifest.tools = tool_stamps.clone();
                    manifest.params = params.clone(); manifest.pixel_format = pixel_format.clone();
                    manifest.fps_num = fps_num; manifest.fps_den = fps_den; manifest.total_frames = frame_count as u64;
                    manifest.source_filter = source_filter.clone(); manifest.restart_pending = false;
                }
                verify_source_cut_original(&manifest,Some(&cancel))?;
                let source_matches = if source_workflow(&settings)&&manifest.queue_recipe.is_some(){
                    if manifest.source_cut_slot&&is_source_cut(&settings)&&manifest.original_source.as_ref()!=Some(&original_source) {return Err(error(&root,"Saved Settings cannot adopt an arbitrary replacement cut."));}
                    digest(&manifest.source.path,Some(&cancel))?==manifest.source
                        &&manifest.original_source.as_ref().is_none_or(|source|digest(&source.path,Some(&cancel)).ok().as_ref()==Some(source))
                }else{source_matches_attempt(
                    &manifest,
                    &root,
                    &original_source,
                    prepared.as_ref(),
                    Some(&cancel),
                )?};
                if !source_matches
                    || manifest.external_sources != external_sources
                    || !tool_contents_match(&manifest.tools, &tool_stamps)
                    || (manifest.edited_command.is_none() && manifest.params != params)
                    || (manifest.edited_command.is_none() && manifest.pixel_format != pixel_format)
                    || manifest.fps_num != fps_num
                    || manifest.fps_den != fps_den
                    || manifest.total_frames != frame_count as u64
                    || (manifest.edited_command.is_none() && manifest.source_filter != source_filter)
                {
                    return Err(error(
                        &root,
                        "Source content, prepared decoded pixels, selected tools, encoder parameters, or frame timing changed; this job cannot reuse old chunks.",
                    ));
                }
                if identity(&video_path(&root, queue_settings(&manifest)))? != manifest.intermediate {
                    return Err(error(&root, "The durable video intermediate was replaced."));
                }
                let intermediate = Temporary::durable(&video_path(&root, queue_settings(&manifest)), true)?;
                (root, manifest, intermediate)
            } else {
                let root = root_for(&id, &request)?;
                fs::create_dir(&root).map_err(|e| error(&root, e.to_string()))?;
                let intermediate = Temporary::durable(&video_path(&root, &settings), false)?;
                let source_cut_slot=source_workflow(&settings)&&is_source_cut(&settings)&&prepared.is_none();
                let (source_stamp, saved_original, prepared_identity) =
                    if let Some(prepared) = &prepared {
                        (
                            copy_prepared(&prepared.path, &root.join("prepared.mkv"), &cancel)?,
                            Some(original_source),
                            Some(prepared.decoded_identity.clone()),
                        )
                    } else if source_cut_slot {
                        (copy_prepared(&source,&cut_slot(&root,&source),&cancel)?,Some(original_source),None)
                    } else {
                        (original_source, None, None)
                    };
                let manifest = Manifest {
                    edited_command: None,
                    created_at: epoch_milliseconds(),
                    last_run_at: None,
                    restart_pending: false,
                    previous_settings: None,
                    version: 1,
                    id,
                    directory: identity(&root)?,
                    request,
                    settings,
                    queue_recipe:None,
                    source_cut_slot,
                    source_slot_write:None,
                    source: source_stamp,
                    external_sources,
                    original_source: saved_original,
                    source_cut_original,
                    source_cut_rebind_pending:None,
                    source_cut_committed_recipe:None,
                    prepared_identity,
                    tools: tool_stamps,
                    launch_artifacts: Vec::new(),
                    params,
                    pixel_format,
                    fps_num,
                    fps_den,
                    total_frames: frame_count as u64,
                    source_filter,
                    phase: RecoveryPhase::Encoding,
                    av1an_version: None,
                    queue: None,
                    scenes: None,
                    script: None,
                    reader_cache: None,
                    concat_inputs: Vec::new(),
                    target_artifacts: Vec::new(),
                    target_scenes: None,
                    segments: Vec::new(),
                    completed: BTreeMap::new(),
                    source_chunk_history_needs_proof: Some(false),
                    intermediate: identity(&intermediate.path)?,
                    final_video: None,
                };
                (root, manifest, intermediate)
            };
            manifest.last_run_at=epoch_milliseconds();
            let input = manifest.source.path.clone();
            let source_guard = Source::open(&input)?;
            let (directory_guard, lock) = match preparation_lock {Some(owned)=>owned,None=>lock_workspace(&root)?};
            let chunk_guards = manifest
                .completed
                .values()
                .map(|chunk| &chunk.file.path)
                .chain(manifest.segments.iter().map(|stamp| &stamp.path))
                .chain(manifest.reader_cache.iter().map(|stamp| &stamp.path))
                .map(|path| Source::open(path))
                .collect::<Result<Vec<_>, _>>()?;
            let mut workspace = Workspace {
                root: root.clone(),
                manifest,
                source_rate: if prepared.is_some() {
                    (fps_num, fps_den)
                } else {
                    source_rate
                },
                segments_verified: false,
                _tools: tool_guards,
                _source: Some(source_guard),
                chunks: chunk_guards,
                directory_guard: Some(directory_guard),
                lock: Some(lock),
            };
            workspace.verify_owner()?;
            workspace.verify_saved(Some(&cancel))?;
            verify_action_tree(&workspace.root, &workspace.manifest)?;
            let resume_chunks=workspace.manifest.queue.is_some();
            let finalizing=workspace.manifest.phase==RecoveryPhase::Finalizing;
            if !finalizing {
                workspace.remove_target_artifacts()?;
                if resume_chunks {
                    workspace.sanitize_done()?;
                    let receipts = workspace.receipts()?;
                    workspace.prepare_video_only_resume(receipts.queued_chunks)?;
                } else {
                    match fs::symlink_metadata(root.join("chunks")) {
                        Ok(_) => remove_tree(&root.join("chunks"), &identity(&root.join("chunks"))?, None)?,
                        Err(error) if error.kind()==std::io::ErrorKind::NotFound => {},
                        Err(cause) => return Err(error(&root,cause.to_string())),
                    }
                    // Select/Hybrid can produce target probes before the queue
                    // is saved. Their scene receipt is sealed for safe actions,
                    // then regenerated with the discarded incomplete attempt.
                    workspace.manifest.scenes = None;
                }
                intermediate.clone_file()?.set_len(0).map_err(|e|error(&intermediate.path,e.to_string()))?;
            }
            workspace.save()?;
            Ok((Self {root,finalizing,resume_chunks,inner:Arc::new(StdMutex::new(workspace))},intermediate,input))
        }).await.map_err(|e|AppError::new("RECOVERY_INVALID",e.to_string(),None))?
    }

    async fn with<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Workspace) -> Result<T, AppError> + Send + 'static,
    ) -> Result<T, AppError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut workspace = inner.lock().map_err(|_| {
                AppError::new("RECOVERY_INVALID", "Recovery state lock failed.", None)
            })?;
            action(&mut workspace)
        })
        .await
        .map_err(|e| AppError::new("RECOVERY_INVALID", e.to_string(), None))?
    }
    pub(in crate::jobs) async fn summary(&self) -> Result<Av1anRecovery, AppError> {
        self.with(|w| Ok(w.summary())).await
    }
    pub(super) async fn reserve_launch(&self, path: &Path) -> Result<File, AppError> {
        let path = path.to_owned();
        self.with(move |w| {
            w.verify_owner()?;
            verify_action_tree(&w.root, &w.manifest)?;
            let mut candidate = w.manifest.clone();
            candidate.launch_artifacts.push(LaunchArtifact {
                path: path.clone(),
                identity: candidate.directory.clone(),
                image: None,
            });
            validate_launch_layout(&w.root, &candidate)?;
            fs::create_dir(&path).map_err(|e| error(&path, e.to_string()))?;
            let guard = launch_directory_guard(&path)?;
            let owned = file_identity(&guard).map_err(|e| error(&path, e.to_string()))?;
            let result = w.transaction(|w| {
                w.manifest.launch_artifacts.push(LaunchArtifact {
                    path: path.clone(),
                    identity: owned.clone(),
                    image: None,
                });
                verify_action_tree(&w.root, &w.manifest)
            });
            if let Err(cause) = result {
                drop(guard);
                // Only our create-new, still-empty directory may be removed
                // if persisting the reservation failed. Preserve replacements.
                let _ = remove_empty_launch(&path, &owned);
                return Err(cause);
            }
            Ok(guard)
        })
        .await
    }
    pub(super) async fn seal_launch(
        &self,
        path: &Path,
        image: Option<&Path>,
    ) -> Result<(), AppError> {
        let path = path.to_owned();
        let image = image.map(Path::to_owned);
        self.with(move |w| {
            w.transaction(|w| {
                w.verify_owner()?;
                let _directory = launch_directory_guard(&path)?;
                let current = identity(&path)?;
                let artifact = w
                    .manifest
                    .launch_artifacts
                    .iter_mut()
                    .find(|launch| launch.path == path)
                    .ok_or_else(|| {
                        error(&path, "The launch directory has no durable reservation.")
                    })?;
                if artifact.identity != current || artifact.image.is_some() {
                    return Err(error(
                        &path,
                        "The launch reservation changed before sealing.",
                    ));
                }
                artifact.image = image
                    .as_deref()
                    .map(|image| digest(image, None))
                    .transpose()?;
                validate_launch_layout(&w.root, &w.manifest)?;
                verify_action_tree(&w.root, &w.manifest)
            })
        })
        .await
    }
    pub(super) async fn retire_launch(&self, path: &Path) -> Result<(), AppError> {
        let path = path.to_owned();
        self.with(move |w| {
            w.verify_owner()?;
            validate_launch_layout(&w.root, &w.manifest)?;
            let owned = w
                .manifest
                .launch_artifacts
                .iter()
                .find(|launch| launch.path == path)
                .ok_or_else(|| error(&path, "The launch directory has no durable reservation."))?
                .identity
                .clone();
            match fs::symlink_metadata(&path) {
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(cause) => return Err(error(&path, cause.to_string())),
                Ok(_) => remove_empty_launch(&path, &owned)?,
            }
            w.transaction(|w| {
                w.manifest
                    .launch_artifacts
                    .retain(|launch| launch.path != path);
                Ok(())
            })
        })
        .await
    }
    pub(in crate::jobs) async fn reader_cache_path(&self) -> Result<Option<PathBuf>, AppError> {
        self.with(|w| {
            Ok(w.manifest
                .reader_cache
                .as_ref()
                .map(|stamp| stamp.path.clone()))
        })
        .await
    }
    pub(super) async fn scorer_identity(&self) -> Result<Option<ScorerIdentity>, AppError> {
        self.with(|w| {
            w.manifest
                .av1an_version
                .as_deref()
                .map(ScorerIdentity::from_version)
                .transpose()
                .map_err(|message| error(&w.root, message))
        })
        .await
    }
    pub(in crate::jobs) async fn checkpoint(
        &self,
        required: bool,
    ) -> Result<Av1anRecovery, AppError> {
        self.with(move |w| {
            w.checkpoint(required)?;
            Ok(w.summary())
        })
        .await
    }
    /// Called only after the entire owned engine process tree has exited.
    pub(super) async fn seal_target_artifacts(&self) -> Result<(), AppError> {
        self.with(|workspace| workspace.transaction(|workspace| workspace.seal_target_artifacts()))
            .await
    }
    /// Hybrid and Segment create copied source segments. Fingerprint/lock each file at its
    /// first checkpoint, and independently compare their complete decoded pixel
    /// sequence to the source before reuse or publication. No size/count-only
    /// receipt is accepted as evidence that a segment contains the right frames.
    pub(in crate::jobs) async fn verify_segments(
        &self,
        ffmpeg: &Path,
        input: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        let queue_source = self
            .with(|workspace| {
                Ok(workspace.manifest.queue_recipe.as_ref().map(|recipe| {
                    recipe
                        .original_source
                        .as_ref()
                        .unwrap_or(&recipe.source)
                        .path
                        .clone()
                }))
            })
            .await?;
        let input = queue_source.as_deref().unwrap_or(input);
        let segments = self
            .with(|w| {
                for guard in &w.chunks {
                    guard.verify()?;
                }
                Ok(if w.segments_verified {
                    Vec::new()
                } else {
                    w.manifest.segments.clone()
                })
            })
            .await?;
        if segments.is_empty() {
            return Ok(());
        }
        let list = self.root.join("segments.ffconcat");
        let text = segment_list_text(&self.root, &segments)?;
        match OpenOptions::new().write(true).create_new(true).open(&list) {
            Ok(mut file) => {
                file.write_all(text.as_bytes())
                    .map_err(|e| error(&list, e.to_string()))?;
                file.sync_all().map_err(|e| error(&list, e.to_string()))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if bytes(&list)? != text.as_bytes() {
                    return Err(error(&list, "Source segment verification list changed."));
                }
            }
            Err(e) => return Err(error(&list, e.to_string())),
        }
        let list_guard = Source::open(&list)?;
        let mut hashes = Vec::new();
        for concatenated in [false, true] {
            let mut args: Vec<OsString> = ["-v", "error", "-nostdin", "-threads", "2"]
                .into_iter()
                .map(Into::into)
                .collect();
            if concatenated {
                args.extend(["-f".into(), "concat".into(), "-safe".into(), "1".into()]);
            }
            args.extend([
                "-i".into(),
                if concatenated {
                    OsString::from("segments.ffconcat")
                } else {
                    input.as_os_str().to_owned()
                },
            ]);
            args.extend(
                [
                    "-map",
                    "0:V:0",
                    "-an",
                    "-sn",
                    "-dn",
                    "-fps_mode",
                    "passthrough",
                    "-pix_fmt",
                    "yuv420p10le",
                    "-f",
                    "hash",
                    "-hash",
                    "sha256",
                    "-",
                ]
                .into_iter()
                .map(Into::into),
            );
            let result = supervisor::run_capture(
                &CommandSpec {
                    executable: ffmpeg.to_owned(),
                    args,
                    cwd: Some(self.root.clone()),
                },
                cancel.clone(),
                64 * 1024,
                Duration::from_secs(24 * 60 * 60),
            )
            .await
            .map_err(|e| process_error(e, input))?;
            let hash = String::from_utf8_lossy(&result.stdout).trim().to_owned();
            if !result.status.success()
                || !hash.strip_prefix("SHA256=").is_some_and(|value| {
                    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
                })
            {
                return Err(error(
                    input,
                    format!(
                        "Source segment identity decode failed: {}",
                        String::from_utf8_lossy(&result.stderr)
                    ),
                ));
            }
            hashes.push(hash);
        }
        list_guard.verify()?;
        if hashes[0] != hashes[1] {
            return Err(error(
                input,
                "Source segments changed, reordered, omitted, or duplicated decoded source frames.",
            ));
        }
        self.with(|w| {
            for guard in &w.chunks {
                guard.verify()?;
            }
            w.segments_verified = true;
            Ok(())
        })
        .await
    }

    pub(in crate::jobs) async fn version(&self, version: String) -> Result<(), AppError> {
        self.with(move |w| {
            w.transaction(|w| {
                if w.manifest
                    .av1an_version
                    .as_ref()
                    .is_some_and(|old| *old != version)
                {
                    return Err(error(&w.root, "av1an or its dependency versions changed."));
                }
                w.manifest.av1an_version = Some(version);
                Ok(())
            })
        })
        .await
    }
    pub(in crate::jobs) async fn finalizing(
        &self,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Av1anRecovery, AppError> {
        let cancel = cancel.clone();
        self.with(move |w| {
            w.transaction(|w| {
                for chunk in &w.chunks {
                    check_cancel(&cancel)?;
                    chunk.verify()?;
                }
                w.manifest.final_video = Some(digest(
                    &video_path(&w.root, queue_settings(&w.manifest)),
                    Some(&cancel),
                )?);
                check_cancel(&cancel)?;
                w.manifest.phase = RecoveryPhase::Finalizing;
                Ok(())
            })?;
            Ok(w.summary())
        })
        .await
    }
    pub(in crate::jobs) async fn cleanup(&self) -> Result<(), AppError> {
        self.with(|w| {
            w.verify_owner()?;
            w.chunks.clear();
            drop(w._source.take());
            drop(w.lock.take());
            remove_workspace_tree(&w.root, &w.manifest, w.directory_guard.take())
        })
        .await
    }
}

// Enumerate and validate the whole owned tree before removing anything. Links,
// junctions and unexpected replacements fail closed; remove_dir never follows
// a directory introduced while cleanup is in progress.
fn remove_tree(
    root: &Path,
    expected: &Identity,
    existing_guard: Option<File>,
) -> Result<(), AppError> {
    remove_tree_checked(root, expected, existing_guard, None)
}

fn remove_workspace_tree(
    root: &Path,
    manifest: &Manifest,
    guard: Option<File>,
) -> Result<(), AppError> {
    remove_tree_checked(root, &manifest.directory, guard, Some(manifest))
}

fn remove_tree_checked(
    root: &Path,
    expected: &Identity,
    existing_guard: Option<File>,
    manifest: Option<&Manifest>,
) -> Result<(), AppError> {
    struct Entry {
        path: PathBuf,
        identity: Identity,
        directory: bool,
        guard: Option<File>,
    }
    fn collect(
        path: &Path,
        expected: &Identity,
        guard: Option<File>,
        entries: &mut Vec<Entry>,
        manifest: Option<&Manifest>,
    ) -> Result<(), AppError> {
        let metadata = fs::symlink_metadata(path).map_err(|e| error(path, e.to_string()))?;
        let directory = metadata.is_dir();
        let guard = match guard {
            Some(guard) => Some(guard),
            None if directory => Some(directory_guard(path)?),
            None if manifest.is_some_and(|manifest| {
                manifest
                    .launch_artifacts
                    .iter()
                    .filter_map(|launch| launch.image.as_ref())
                    .any(|image| image.path == path)
            }) =>
            {
                let mut options = OpenOptions::new();
                options.read(true);
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    // Deny writes through validation and exact-handle deletion.
                    // FILE_SHARE_DELETE permits our DELETE handle while an ID
                    // check still rejects any replacement of this pathname.
                    options.share_mode(1 | 4).custom_flags(0x00200000);
                }
                Some(options.open(path).map_err(|e| error(path, e.to_string()))?)
            }
            None => None,
        };
        if identity(path)? != *expected {
            return Err(error(
                path,
                "Recovery directory or file was replaced before cleanup.",
            ));
        }
        if directory {
            for child in fs::read_dir(path).map_err(|e| error(path, e.to_string()))? {
                let child = child.map_err(|e| error(path, e.to_string()))?.path();
                collect(&child, &identity(&child)?, None, entries, manifest)?;
            }
        } else if !metadata.is_file() {
            return Err(error(path, "Unsupported recovery filesystem entry."));
        }
        entries.push(Entry {
            path: path.to_owned(),
            identity: expected.clone(),
            directory,
            guard,
        });
        Ok(())
    }
    let mut entries = Vec::new();
    collect(root, expected, existing_guard, &mut entries, manifest)?;
    #[cfg(test)]
    REMOVE_ENUMERATED_HOOK.with(|hook| {
        if let Some(mutate) = hook.borrow_mut().take() {
            mutate();
        }
    });
    if let Some(manifest) = manifest {
        // Validate after enumeration, while recorded launch images and all
        // ancestor directories remain guarded. Never delete an extra entry
        // introduced between preflight and cleanup enumeration.
        verify_action_tree(root, manifest)?;
    }
    let root_entry = entries.pop().expect("owned root entry");
    // Preserve the durable locator/receipts if deleting any owned child fails.
    let mut receipts = Vec::new();
    let mut children = Vec::new();
    for entry in entries {
        if entry.path == root.join("manifest.json") || entry.path == root.join("workspace.lock") {
            receipts.push(entry)
        } else {
            children.push(entry)
        }
    }
    children.extend(receipts);
    children.push(root_entry);
    for entry in children {
        if identity(root)? != *expected || identity(&entry.path)? != entry.identity {
            return Err(error(&entry.path, "Recovery entry changed during cleanup."));
        }
        #[cfg(windows)]
        {
            let mut entry = entry;
            // Windows requires closing a directory handle before deleting its
            // exact ID. Ancestor directory guards remain live.
            if entry.directory {
                drop(entry.guard.take());
            }
            let _ = entry.directory;
            let id = &entry.identity.0;
            files::windows_delete_owned(&entry.path, (id[0] as u32, id[1] as u32, id[2] as u32))
                .map_err(|e| error(&entry.path, e.to_string()))?;
        }
        #[cfg(not(windows))]
        {
            // Keep any supplied identity guard live through the unlink.
            if entry.directory {
                fs::remove_dir(&entry.path)
            } else {
                fs::remove_file(&entry.path)
            }
            .map_err(|e| error(&entry.path, e.to_string()))?;
            drop(entry.guard);
        }
    }
    Ok(())
}

impl JobManager {
    pub(crate) async fn validate_recovery(&self, id: &str) -> Result<(), AppError> {
        let snapshot = self
            .state
            .lock()
            .await
            .entries
            .iter()
            .find(|entry| entry.snapshot.id == id)
            .map(|entry| entry.snapshot.clone())
            .ok_or_else(|| AppError::new("JOB_NOT_FOUND", "The job no longer exists.", None))?;
        tokio::task::spawn_blocking(move || {
            let owned = OwnedRecovery::open(&snapshot, true)?;
            let settings = snapshot
                .encode_settings
                .as_ref()
                .expect("verified saved settings");
            if let Some(canonical) = super::super::source_normalization::prepare_recovery_action(
                &snapshot.id,
                &snapshot.request,
                settings,
                owned.settings(),
            )? {
                canonical.rebind(settings)?;
            }
            if let Some(cut) =
                super::super::source_normalization::av1an_trim::prepare_recovery_action(
                    &snapshot.id,
                    &snapshot.request,
                    settings,
                    owned.settings(),
                )?
            {
                cut.rebind(settings)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| AppError::new("RECOVERY_INVALID", e.to_string(), None))?
    }
}

/// Restore only the last committed recipe/source/tool fields after interruption
/// during a source-cut New Settings review. Existing chunk files/receipts remain
/// exactly as they are after the authorized reset; deleted chunks are not revived.
pub(in crate::jobs) fn restore_interrupted_source_cut(
    snapshot: &media_core::JobSnapshot,
) -> Result<
    Option<(
        EncodeSettings,
        Option<media_core::EditedCommand>,
        Av1anRecovery,
    )>,
    AppError,
> {
    if snapshot.recovery.is_none() {
        return Ok(None);
    }
    let root = root_for(&snapshot.id, &snapshot.request)?;
    if !root
        .join("manifest.json")
        .try_exists()
        .map_err(|cause| error(&root, cause.to_string()))?
    {
        return Ok(None);
    }
    let manifest: Manifest = serde_json::from_slice(&bytes(&root.join("manifest.json"))?)
        .map_err(|cause| error(&root, cause.to_string()))?;
    let Some(recipe) = manifest.source_cut_committed_recipe.clone() else {
        return Ok(None);
    };
    if manifest.id != snapshot.id
        || manifest.request != snapshot.request
        || manifest.directory != identity(&root)?
        || snapshot
            .encode_settings
            .as_ref()
            .is_none_or(|settings| *settings != manifest.settings && *settings != recipe.settings)
    {
        return Err(error(
            &root,
            "Interrupted source-cut ownership does not match the saved job.",
        ));
    }
    let mut current = snapshot.clone();
    current.encode_settings = Some(manifest.settings.clone());
    let mut owned = OwnedRecovery::open(&current, false)?;
    let settings = recipe.settings.clone();
    let command = recipe.edited_command.clone();
    let summary = owned.restore_command_recipe(*recipe)?;
    let keep = owned.source_variant_settings();
    drop(owned);
    super::super::source_normalization::av1an_trim::discard_variants_except(
        &snapshot.id,
        &snapshot.request,
        &keep,
    )?;
    Ok(Some((settings, command, summary)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concat_output_must_be_below_owned_root_without_parent_traversal() {
        let fixture = Fixture::new();
        let root = fixture.0.canonicalize().unwrap();
        let launch = root.join("launch-1-1");
        fs::create_dir(&launch).unwrap();
        assert!(owned_concat_output(&root, &launch.join("encoded.mkv")).unwrap());
        assert!(!owned_concat_output(&root, Path::new("launch-1-1/encoded.mkv")).unwrap());
        let mut traversal = root.as_os_str().to_owned();
        traversal.push(format!(
            "{}..{}foreign.mkv",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        ));
        assert!(!owned_concat_output(&root, Path::new(&traversal)).unwrap());
        let mut traversal = launch.as_os_str().to_owned();
        traversal.push(format!(
            "{}..{}..{}foreign.mkv",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        ));
        assert!(!owned_concat_output(&root, Path::new(&traversal)).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn concat_output_accepts_pinned_av1an_mkvmerge_path_without_verbatim_prefix() {
        let fixture = Fixture::new();
        let root = fixture.0.canonicalize().unwrap();
        let launch = root.join("launch-1-1");
        fs::create_dir(&launch).unwrap();
        let output = launch.join("encoded.mkv");
        let normal = output.to_str().unwrap().strip_prefix(r"\\?\").unwrap();
        assert!(owned_concat_output(&root, Path::new(normal)).unwrap());
    }

    #[test]
    fn concat_output_rejects_directory_links_even_when_they_resolve_inside_owned_root() {
        let fixture = Fixture::new();
        let root = fixture.0.canonicalize().unwrap();
        let launch = root.join("launch-1-1");
        let alias = root.join("alias");
        fs::create_dir(&launch).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&launch, &alias).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let result = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&alias)
                .arg(&launch)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        assert!(owned_concat_output(&root, &alias.join("encoded.mkv")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_failure_removes_only_its_uncommitted_receipt() {
        const CHILD: &str = "JESSES_ATOMIC_RECEIPT_FAILURE_CHILD";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = PathBuf::from(root);
            let target = root.join("done.json");
            // Inject a real partial kernel write followed by EFBIG, confined
            // to this subprocess so other tests retain their resource limits.
            unsafe {
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                let limit = libc::rlimit {
                    rlim_cur: 1,
                    rlim_max: 1,
                };
                assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
            }
            assert!(atomic(&target, b"new receipt").is_err());
            assert_eq!(fs::read(&target).unwrap(), b"committed receipt");
            assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
            return;
        }
        let fixture = Fixture::new();
        fs::write(fixture.0.join("done.json"), b"committed receipt").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "jobs::av1an::recovery::tests::atomic_write_failure_removes_only_its_uncommitted_receipt", "--nocapture"])
            .env(CHILD, &fixture.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read(fixture.0.join("done.json")).unwrap(),
            b"committed receipt"
        );
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn scorer_identity_reads_legacy_cpu_and_hash_bound_gpu_versions() {
        let base = "av1an 0.5.2\ncom.lumen.vship : Not found";
        assert_eq!(
            ScorerIdentity::from_version(base).unwrap(),
            ScorerIdentity::Cpu
        );
        let sha256 = "a".repeat(64);
        let gpu = version_with_vship(base, &sha256);
        assert_eq!(
            ScorerIdentity::from_version(&gpu).unwrap(),
            ScorerIdentity::Vship {
                sha256: sha256.clone()
            }
        );
        assert!(ScorerIdentity::from_version(&version_with_vship(base, "bad")).is_err());
        assert!(
            ScorerIdentity::from_version(&format!("{gpu}\n{VSHIP_ACTIVE_PREFIX}{sha256}")).is_err()
        );
    }

    #[tokio::test]
    async fn saved_cpu_scorer_receipt_cannot_be_rebound_to_vship() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let base = "av1an 0.5.2\ncom.lumen.vship : Not found";
        workspace.manifest.av1an_version = Some(base.into());
        workspace.save().unwrap();
        let recovery = Recovery {
            root: workspace.root.clone(),
            finalizing: false,
            resume_chunks: true,
            inner: Arc::new(StdMutex::new(workspace)),
        };
        assert_eq!(
            recovery.scorer_identity().await.unwrap(),
            Some(ScorerIdentity::Cpu)
        );
        assert!(
            recovery
                .version(version_with_vship(base, &"a".repeat(64)))
                .await
                .is_err()
        );
        assert_eq!(
            recovery.scorer_identity().await.unwrap(),
            Some(ScorerIdentity::Cpu)
        );
        drop(recovery);
        drop(intermediate);
    }

    // Concurrent Windows fixtures can observe the same clock timestamp.
    static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jesses-recovery-receipts-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn workspace(&self) -> (Workspace, Temporary) {
            let source = self.0.join("source.mkv");
            fs::write(&source, b"source bytes").unwrap();
            let request = RemuxRequest {
                input_path: source.to_string_lossy().into_owned(),
                output_path: self.0.join("output.mkv").to_string_lossy().into_owned(),
                stream_indices: vec![0],
            };
            let root = root_for("owned-job", &request).unwrap();
            fs::create_dir(&root).unwrap();
            let intermediate = Temporary::durable(&root.join("video.ivf"), false).unwrap();
            let settings = EncodeSettings {
                backend: media_core::EncodeBackend::Av1an,
                ..Default::default()
            };
            let manifest = Manifest {
                edited_command: None,
                created_at: None,
                last_run_at: None,
                restart_pending: false,
                previous_settings: None,
                version: 1,
                id: "owned-job".into(),
                directory: identity(&root).unwrap(),
                request,
                settings,
                queue_recipe: None,
                source_cut_slot: false,
                source_slot_write: None,
                source: digest(&source, None).unwrap(),
                external_sources: vec![],
                original_source: None,
                source_cut_original: None,
                source_cut_rebind_pending: None,
                source_cut_committed_recipe: None,
                prepared_identity: None,
                tools: vec![],
                launch_artifacts: Vec::new(),
                params: vec![],
                pixel_format: default_pixel_format(),
                fps_num: 24,
                fps_den: 1,
                total_frames: 48,
                source_filter: None,
                phase: RecoveryPhase::Encoding,
                av1an_version: None,
                queue: None,
                scenes: None,
                script: None,
                reader_cache: None,
                concat_inputs: Vec::new(),
                target_artifacts: Vec::new(),
                target_scenes: None,
                completed: BTreeMap::new(),
                source_chunk_history_needs_proof: Some(false),
                segments: Vec::new(),
                intermediate: identity(&intermediate.path).unwrap(),
                final_video: None,
            };
            let (directory, lock) = lock_workspace(&root).unwrap();
            let workspace = Workspace {
                root,
                manifest,
                source_rate: (24, 1),
                segments_verified: false,
                _tools: vec![],
                _source: Some(Source::open(&source).unwrap()),
                chunks: vec![],
                directory_guard: Some(directory),
                lock: Some(lock),
            };
            workspace.save().unwrap();
            (workspace, intermediate)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn frozen_source_recipe(workspace: &Workspace, ignore_frame_mismatch: bool) -> QueueRecipe {
        let manifest = &workspace.manifest;
        QueueRecipe {
            source: manifest.source.clone(),
            original_source: manifest.original_source.clone(),
            tools: manifest.tools.clone(),
            settings: manifest.settings.clone(),
            params: manifest.params.clone(),
            pixel_format: manifest.pixel_format.clone(),
            fps_num: manifest.fps_num,
            fps_den: manifest.fps_den,
            source_rate: workspace.source_rate,
            total_frames: manifest.total_frames,
            source_filter: manifest.source_filter.clone(),
            ignore_frame_mismatch,
        }
    }

    fn source_seal_fixture(fixture: &Fixture, count: u64, decode: bool) -> (Workspace, Temporary) {
        let (mut workspace, video) = fixture.workspace();
        let (ffmpeg, ffprobe) = if decode {
            let ffmpeg =
                PathBuf::from(std::env::var_os("JESSES_FFMPEG").expect("set JESSES_FFMPEG"));
            let ffprobe = std::env::var_os("JESSES_FFPROBE")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    ffmpeg.with_file_name(if cfg!(windows) {
                        "ffprobe.exe"
                    } else {
                        "ffprobe"
                    })
                });
            (ffmpeg, ffprobe)
        } else {
            let ffmpeg = fixture.0.join("ffmpeg");
            let ffprobe = fixture.0.join("ffprobe");
            fs::write(&ffmpeg, b"test ffmpeg bytes").unwrap();
            fs::write(&ffprobe, b"test ffprobe bytes").unwrap();
            (ffmpeg, ffprobe)
        };
        workspace.manifest.total_frames = 2 * count;
        workspace.manifest.settings.encoder = VideoEncoder::X264;
        workspace.manifest.settings.av1an_options = Some(media_core::Av1anOptions {
            source_workflow: true,
            ..Default::default()
        });
        workspace.manifest.tools = vec![
            digest(&ffmpeg, None).unwrap(),
            digest(&ffprobe, None).unwrap(),
        ];
        workspace.manifest.queue_recipe = Some(frozen_source_recipe(&workspace, false));
        let directory = workspace.root.join("chunks/encode");
        fs::create_dir_all(&directory).unwrap();
        let encoded = directory.join("00000.h264");
        if decode {
            let output = std::process::Command::new(&ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=64x64:rate=24",
                    "-frames:v",
                    "2",
                    "-c:v",
                    "libx264",
                    "-f",
                    "h264",
                ])
                .arg(&encoded)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            fs::write(&encoded, b"test chunk bytes").unwrap();
        }
        for index in 0..count {
            let name = format!("{index:05}");
            let path = directory.join(format!("{name}.h264"));
            if index != 0 {
                fs::copy(&encoded, &path).unwrap();
            }
            workspace.manifest.completed.insert(
                name,
                Chunk {
                    frames: 2,
                    file: digest(&path, None).unwrap(),
                    decoded_contract: None,
                },
            );
        }
        workspace.save().unwrap();
        (workspace, video)
    }

    #[tokio::test]
    async fn source_join_mode_uses_recipe_and_count_provenance_not_unrelated_settings() {
        let fixture = Fixture::new();
        let (workspace, _video) = source_seal_fixture(&fixture, 2, false);
        let recovery = Recovery {
            root: workspace.root.clone(),
            finalizing: true,
            resume_chunks: true,
            inner: Arc::new(StdMutex::new(workspace)),
        };
        recovery
            .with(|workspace| {
                workspace.manifest.settings.workers = 4;
                workspace.manifest.settings.crf = 20;
                workspace
                    .manifest
                    .settings
                    .av1an_options
                    .as_mut()
                    .unwrap()
                    .concat_method = media_core::Av1anConcatMethod::Ffmpeg;
                Ok(())
            })
            .await
            .unwrap();
        assert!(matches!(
            recovery.source_join_mode(false).await.unwrap(),
            Some(SourceJoinMode::Basic {
                frames: 4,
                rate: (24, 1),
                ..
            })
        ));
        recovery
            .with(|workspace| {
                workspace.manifest.source_filter = Some("hflip".into());
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            matches!(recovery.source_join_mode(false).await.unwrap(), Some(SourceJoinMode::Detailed { reason }) if reason.contains("changed source recipe"))
        );
        recovery
            .with(|workspace| {
                record_source_chunk_provenance(&mut workspace.manifest);
                workspace.manifest.source_filter = None;
                workspace.save()?;
                let path = workspace.root.join("manifest.json");
                workspace.manifest = serde_json::from_slice(&bytes(&path)?)
                    .map_err(|cause| error(&path, cause.to_string()))?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            matches!(recovery.source_join_mode(false).await.unwrap(), Some(SourceJoinMode::Detailed { reason }) if reason.contains("changed source recipe"))
        );
        recovery
            .with(|workspace| {
                workspace.manifest.source_chunk_history_needs_proof = Some(false);
                workspace.manifest.edited_command = Some(media_core::EditedCommand {
                    generated: "generated".into(),
                    arguments: "edited".into(),
                });
                record_source_chunk_provenance(&mut workspace.manifest);
                workspace.manifest.edited_command = None;
                workspace.save()?;
                let path = workspace.root.join("manifest.json");
                workspace.manifest = serde_json::from_slice(&bytes(&path)?)
                    .map_err(|cause| error(&path, cause.to_string()))?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            matches!(recovery.source_join_mode(false).await.unwrap(), Some(SourceJoinMode::Detailed { reason }) if reason.contains("edited command"))
        );
        recovery
            .with(|workspace| {
                workspace.manifest.source_chunk_history_needs_proof = Some(false);
                workspace
                    .manifest
                    .queue_recipe
                    .as_mut()
                    .unwrap()
                    .ignore_frame_mismatch = true;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            matches!(recovery.source_join_mode(false).await.unwrap(), Some(SourceJoinMode::Detailed { reason }) if reason.contains("resamples frames"))
        );
        recovery
            .with(|workspace| {
                workspace.manifest.completed.remove("00000");
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            recovery.source_join_mode(false).await.err().unwrap().code,
            "RECOVERY_INVALID"
        );
    }

    #[tokio::test]
    #[ignore = "requires a qualified FFmpeg/FFprobe pair with libx264"]
    async fn sealing_multiple_decoded_chunks_hashes_the_source_only_twice() {
        let fixture = Fixture::new();
        let (workspace, _video) = source_seal_fixture(&fixture, 4, true);
        COUNT_SOURCE_HASHES.with(|counter| {
            *counter.borrow_mut() = Some((workspace.manifest.source.path.clone(), 0))
        });
        let mut recovery = OwnedRecovery { workspace };
        let (_sender, cancel) = watch::channel(false);
        let result = recovery.seal_source_chunks(&cancel).await;
        let (_, reads) = COUNT_SOURCE_HASHES.with(|counter| counter.borrow_mut().take().unwrap());
        result.unwrap();
        assert!(
            recovery
                .workspace
                .manifest
                .completed
                .values()
                .all(|chunk| chunk.decoded_contract.is_some())
        );
        assert_eq!(
            reads, 2,
            "all pending chunks must share the before/after source hashes"
        );
    }

    #[tokio::test]
    #[ignore = "requires a qualified FFmpeg/FFprobe pair with libx264"]
    async fn active_source_seal_rejects_same_length_tamper_after_the_chunk_decodes() {
        let fixture = Fixture::new();
        let (workspace, _video) = source_seal_fixture(&fixture, 2, true);
        let chunk = workspace.manifest.completed["00000"].file.path.clone();
        let original = fs::read(&chunk).unwrap();
        let mut tampered = original.clone();
        tampered[0] ^= 1;
        let manifest_path = workspace.root.join("manifest.json");
        let saved = fs::read(&manifest_path).unwrap();
        let recovery = Recovery {
            root: workspace.root.clone(),
            finalizing: false,
            resume_chunks: true,
            inner: Arc::new(StdMutex::new(workspace)),
        };
        let (_sender, cancel) = watch::channel(false);
        let result = recovery
            .seal_source_chunks(&cancel, |verified, _| {
                if verified == 1 {
                    fs::write(&chunk, &tampered).unwrap();
                }
                std::future::ready(())
            })
            .await;
        assert_eq!(result.unwrap_err().code, "RECOVERY_INVALID");
        assert_eq!(fs::read(&manifest_path).unwrap(), saved);
        fs::write(&chunk, original).unwrap();
        recovery
            .seal_source_chunks(&cancel, |_, _| std::future::ready(()))
            .await
            .unwrap();
        assert_eq!(
            recovery
                .source_join_inputs(&cancel)
                .await
                .unwrap()
                .unwrap()
                .chunks
                .len(),
            2
        );
    }

    #[test]
    fn source_seal_cancellation_during_full_hash_keeps_saved_receipts() {
        let fixture = Fixture::new();
        let (mut workspace, _video) = source_seal_fixture(&fixture, 1, false);
        let source = workspace.manifest.source.path.clone();
        let (sender, cancel) = watch::channel(false);
        let batch = workspace.prepare_source_seal(&cancel).unwrap().unwrap();
        let manifest_path = workspace.root.join("manifest.json");
        let saved = fs::read(&manifest_path).unwrap();
        DIGEST_FIRST_READ_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |path| {
                if path == source {
                    sender.send_replace(true);
                }
            }))
        });
        let result = workspace.commit_source_seal(&batch, BTreeMap::new(), &cancel);
        DIGEST_FIRST_READ_HOOK.with(|hook| hook.borrow_mut().take());
        assert_eq!(result.unwrap_err().code, "JOB_CANCELED");
        assert_eq!(fs::read(&manifest_path).unwrap(), saved);
        workspace.verify_saved(None).unwrap();
    }

    #[test]
    fn source_seal_rejects_replaced_source_and_concurrent_manifest_changes() {
        let fixture = Fixture::new();
        let (mut workspace, _video) = source_seal_fixture(&fixture, 1, false);
        let (_sender, cancel) = watch::channel(false);
        let batch = workspace.prepare_source_seal(&cancel).unwrap().unwrap();
        let manifest_path = workspace.root.join("manifest.json");
        let saved = fs::read(&manifest_path).unwrap();
        let source = workspace.manifest.source.path.clone();
        let original = fixture.0.join("original-source.mkv");
        drop(workspace._source.take());
        fs::rename(&source, &original).unwrap();
        fs::copy(&original, &source).unwrap();
        let result = workspace.commit_source_seal(&batch, BTreeMap::new(), &cancel);
        assert_eq!(result.unwrap_err().code, "RECOVERY_INVALID");
        assert_eq!(fs::read(&manifest_path).unwrap(), saved);
        fs::remove_file(&source).unwrap();
        fs::rename(original, &source).unwrap();
        workspace.manifest.last_run_at = Some(1);
        workspace.save().unwrap();
        let updated = fs::read(&manifest_path).unwrap();
        let result = workspace.commit_source_seal(&batch, BTreeMap::new(), &cancel);
        assert!(
            result
                .unwrap_err()
                .message
                .contains("saved source queue changed")
        );
        assert_eq!(fs::read(&manifest_path).unwrap(), updated);
        workspace.manifest.last_run_at = batch.manifest.last_run_at;
        let result = workspace.commit_source_seal(&batch, BTreeMap::new(), &cancel);
        assert!(
            result
                .unwrap_err()
                .message
                .contains("durable source queue changed")
        );
        assert_eq!(fs::read(&manifest_path).unwrap(), updated);
    }

    #[test]
    fn source_queue_count_policy_is_frozen_independently_of_new_global_fps() {
        let fixture = Fixture::new();
        let (mut workspace, _video) = fixture.workspace();
        workspace.manifest.settings.av1an_options = Some(media_core::Av1anOptions {
            source_workflow: true,
            ..Default::default()
        });
        workspace.manifest.queue_recipe = Some(frozen_source_recipe(&workspace, false));
        workspace.manifest.settings.temporal = Some(media_core::TemporalSettings {
            frame_rate: Some(media_core::FrameRate {
                numerator: 30,
                denominator: 1,
            }),
            ..Default::default()
        });
        assert!(super::super::options::source_resamples(
            &workspace.manifest.settings,
            (24, 1)
        ));
        assert!(matches!(
            queue_count_policy(&workspace.manifest),
            crate::jobs::source_queue::CountPolicy::ExactSourceFrames
        ));
        workspace
            .manifest
            .queue_recipe
            .as_mut()
            .unwrap()
            .ignore_frame_mismatch = true;
        workspace.manifest.settings.temporal = None;
        assert!(!super::super::options::source_resamples(
            &workspace.manifest.settings,
            (24, 1)
        ));
        assert!(matches!(
            queue_count_policy(&workspace.manifest),
            crate::jobs::source_queue::CountPolicy::SourceTypedResample
        ));
    }

    #[test]
    fn canceled_partial_source_slot_restores_committed_bytes_for_same_process_retry() {
        let fixture = Fixture::new();
        let (mut workspace, _video) = fixture.workspace();
        let original = workspace.manifest.source.clone();
        workspace.manifest.settings.av1an_options = Some(media_core::Av1anOptions {
            source_workflow: true,
            chunk_method: media_core::Av1anChunkMethod::Select,
            ..Default::default()
        });
        workspace.manifest.settings.source_trim = true;
        workspace.manifest.settings.trim = Some(
            serde_json::from_value(serde_json::json!({"startFrame":0,"endFrameExclusive":48}))
                .unwrap(),
        );
        let old = fixture.0.join("old-cut.mkv");
        let next = fixture.0.join("next-cut.mkv");
        for (path, byte) in [(&old, 0x11u8), (&next, 0x22u8)] {
            let mut file = File::create(path).unwrap();
            for _ in 0..256 {
                file.write_all(&[byte; 65536]).unwrap();
            }
            file.sync_all().unwrap();
        }
        let (_sender, cancel) = watch::channel(false);
        let slot = cut_slot(&workspace.root, &old);
        workspace.manifest.source = copy_prepared(&old, &slot, &cancel).unwrap();
        workspace.manifest.original_source = Some(digest(&old, None).unwrap());
        workspace.manifest.source_cut_original = Some(original);
        workspace.manifest.source_cut_slot = true;
        let chunks = workspace.root.join("chunks");
        fs::create_dir_all(chunks.join("split")).unwrap();
        for name in ["chunks.json", "scenes.json"] {
            fs::write(chunks.join(name), b"[]").unwrap();
        }
        workspace.manifest.queue = Some(digest(&chunks.join("chunks.json"), None).unwrap());
        workspace.manifest.scenes = Some(digest(&chunks.join("scenes.json"), None).unwrap());
        workspace.manifest.queue_recipe = Some(frozen_source_recipe(&workspace, false));
        workspace.save().unwrap();
        let old_settings = workspace.manifest.settings.clone();
        let old_stamp = workspace.manifest.source.clone();
        let mut owned = OwnedRecovery { workspace };
        let mut next_settings = old_settings.clone();
        next_settings.trim = Some(
            serde_json::from_value(serde_json::json!({"startFrame":48,"endFrameExclusive":96}))
                .unwrap(),
        );
        owned.new_settings(&next_settings).unwrap();
        let next_stamp = digest(&next, None).unwrap();
        owned.workspace.manifest.source_slot_write = Some(next_stamp.clone());
        owned.workspace.save().unwrap();
        let (sender, cancel) = watch::channel(false);
        let observed = slot.clone();
        // Cancel after one actual write rather than racing a monitor thread
        // against a fast filesystem copy. The remaining recovery path is real.
        CUT_SLOT_WRITE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                assert_eq!(fs::metadata(&observed).unwrap().len(), 65536);
                sender.send_replace(true);
            }));
        });
        let error =
            write_cut_slot(&next_stamp, &slot, &old_stamp.identity, Some(&cancel)).unwrap_err();
        assert_eq!(error.code, "JOB_CANCELED");
        assert_eq!(identity(&slot).unwrap(), old_stamp.identity);
        assert_eq!(fs::read(&slot).unwrap(), vec![0x22; 65536]);
        assert_ne!(digest(&slot, None).unwrap(), old_stamp);
        let snapshot:media_core::JobSnapshot=serde_json::from_value(serde_json::json!({"id":"owned-job","state":"canceled","request":owned.workspace.manifest.request,"encodeSettings":next_settings,"recovery":owned.summary(),"progressSeconds":null,"durationSeconds":null,"logs":[],"error":null,"logPath":null})).unwrap();
        drop(owned);
        let restored = restore_interrupted_source_cut(&snapshot).unwrap().unwrap();
        assert_eq!(restored.0, old_settings);
        assert_eq!(digest(&slot, None).unwrap(), old_stamp);
        let mut saved = snapshot;
        saved.encode_settings = Some(restored.0);
        saved.recovery = Some(restored.2);
        let owned = OwnedRecovery::open(&saved, true).unwrap();
        assert!(!owned.workspace.manifest.restart_pending);
        assert!(owned.workspace.manifest.source_slot_write.is_none());
        assert!(
            owned
                .workspace
                .manifest
                .source_cut_committed_recipe
                .is_none()
        );
        assert_eq!(fs::read(&old).unwrap(), fs::read(&slot).unwrap());
    }

    #[test]
    fn new_settings_journal_keeps_old_and_new_history_coherent_and_preserves_compatible_preparation()
     {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let original = workspace.manifest.source.clone();
        let prepared = workspace.root.join("prepared.mkv");
        fs::write(&prepared, b"verified decoded frames").unwrap();
        workspace.manifest.source = digest(&prepared, None).unwrap();
        workspace.manifest.original_source = Some(original);
        workspace.manifest.prepared_identity = Some("verified decoded identity".into());
        workspace.save().unwrap();
        let request = workspace.manifest.request.clone();
        let settings = workspace.manifest.settings.clone();
        let locator = workspace.summary();
        let root = workspace.root.clone();
        let prepared_before = digest(&prepared, None).unwrap();
        let snapshot:media_core::JobSnapshot=serde_json::from_value(serde_json::json!({"id":"owned-job","state":"stopped","request":request,"encodeSettings":settings,"recovery":locator,"progressSeconds":null,"durationSeconds":null,"logs":[],"error":null,"logPath":null})).unwrap();
        drop(workspace);
        drop(intermediate);
        let mut owned = OwnedRecovery::open(&snapshot, true).unwrap();
        let mut next = settings.clone();
        next.crf = 31;
        let updated = owned.new_settings(&next).unwrap();
        assert_eq!(updated.completed_frames, 0);
        drop(owned);
        assert_eq!(digest(&prepared, None).unwrap(), prepared_before);
        let (_, pending) = load("owned-job", &request, &next, &updated).unwrap();
        assert!(pending.restart_pending);
        assert_eq!(pending.previous_settings, Some(settings.clone()));
        assert!(pending.completed.is_empty());
        // A process can stop between manifest reset and durable history commit.
        // Either coherent history side remains authorized, but no old chunks do.
        load("owned-job", &request, &settings, &locator).unwrap();
        let mut foreign = next.clone();
        foreign.crf = 32;
        assert!(load("owned-job", &request, &foreign, &updated).is_err());
        let mut owned = OwnedRecovery::open(&snapshot, false).unwrap();
        let mut trimmed = next.clone();
        trimmed.trim = Some(media_core::VideoTrim {
            start_frame: 1,
            end_frame_exclusive: 24,
            time: None,
        });
        owned.new_settings(&trimmed).unwrap();
        assert!(!prepared.exists());
        assert!(owned.workspace.manifest.prepared_identity.is_none());
        assert!(owned.workspace.manifest.queue.is_none());
        assert!(!root.join("chunks").exists());
        drop(owned);
        let owned = OwnedRecovery::open(&snapshot, false).unwrap();
        let foreign = root.join("introduced-after-review.txt");
        fs::write(&foreign, b"foreign").unwrap();
        assert!(owned.discard().is_err());
        assert_eq!(fs::read(&foreign).unwrap(), b"foreign");
    }

    #[tokio::test]
    async fn discard_requires_the_saved_owner_and_preserves_neighbor_without_source() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let external = fixture.0.join("external.mka");
        fs::write(&external, b"external bytes").unwrap();
        workspace.manifest.settings.external_tracks = vec![media_core::ExternalTrack {
            subtitle_burn_in: false,
            offset_milliseconds: 0,
            subtitle_mode: None,
            title: None,
            language: None,
            default: None,
            forced: None,
            audio: None,
            input_path: external.to_string_lossy().into_owned(),
            stream_index: 0,
        }];
        workspace.manifest.external_sources =
            external_source_stamps(&workspace.manifest.settings, None).unwrap();
        workspace.save().unwrap();
        let root = workspace.root.clone();
        let locator = workspace.summary();
        let request = workspace.manifest.request.clone();
        let settings = workspace.manifest.settings.clone();
        let neighbor = fixture.0.join("neighbor.txt");
        fs::write(&neighbor, b"not workspace content").unwrap();

        // An active job's exclusive workspace lock bars the discard API.
        assert!(
            Recovery::discard("owned-job", &request, &settings, &locator)
                .await
                .is_err()
        );
        assert!(root.exists());
        drop(workspace);
        drop(intermediate);
        fs::remove_file(&request.input_path).unwrap();
        fs::remove_file(&external).unwrap();
        Recovery::discard("owned-job", &request, &settings, &locator)
            .await
            .unwrap();
        assert!(!root.exists());
        assert!(!external.exists());
        assert_eq!(fs::read(&neighbor).unwrap(), b"not workspace content");
    }

    #[tokio::test]
    async fn discard_rejects_replaced_root_and_symlink_without_deleting_outside_data() {
        let fixture = Fixture::new();
        let (workspace, intermediate) = fixture.workspace();
        let root = workspace.root.clone();
        let locator = workspace.summary();
        let request = workspace.manifest.request.clone();
        let settings = workspace.manifest.settings.clone();
        drop(workspace);
        drop(intermediate);

        let moved = fixture.0.join("original-workspace");
        fs::rename(&root, &moved).unwrap();
        fs::create_dir(&root).unwrap();
        fs::copy(moved.join("manifest.json"), root.join("manifest.json")).unwrap();
        let foreign = root.join("foreign.txt");
        fs::write(&foreign, b"foreign").unwrap();
        assert!(
            Recovery::discard("owned-job", &request, &settings, &locator)
                .await
                .is_err()
        );
        assert_eq!(fs::read(&foreign).unwrap(), b"foreign");
        assert!(moved.join("manifest.json").exists());

        // Reopen the genuine root and place a link to an outside sentinel. A
        // complete pre-delete walk must reject it before removing any entry.
        fs::remove_dir_all(&root).unwrap();
        fs::rename(&moved, &root).unwrap();
        let sentinel = fixture.0.join("outside-sentinel.txt");
        fs::write(&sentinel, b"outside").unwrap();
        let link = root.join("outside-link");
        #[cfg(unix)]
        let link_created = std::os::unix::fs::symlink(&sentinel, &link).is_ok();
        #[cfg(windows)]
        let link_created = std::os::windows::fs::symlink_file(&sentinel, &link).is_ok();
        if link_created {
            assert!(
                Recovery::discard("owned-job", &request, &settings, &locator)
                    .await
                    .is_err()
            );
            assert!(root.join("manifest.json").exists());
        }
        assert_eq!(fs::read(&sentinel).unwrap(), b"outside");
    }

    #[test]
    fn tool_relocation_requires_ordered_byte_identical_roles() {
        let fixture = Fixture::new();
        let original = fixture.0.join("original-tools");
        let relocated = fixture.0.join("relocated-tools");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&relocated).unwrap();
        let tools = [
            ("ffmpeg", b"ffmpeg".as_slice()),
            ("ffprobe", b"ffprobe".as_slice()),
            ("encoder", b"encoder".as_slice()),
            ("av1an", b"av1an".as_slice()),
        ];
        for (name, contents) in tools {
            fs::write(original.join(name), contents).unwrap();
            fs::copy(original.join(name), relocated.join(name)).unwrap();
        }
        let stamps = |root: &Path| {
            tools
                .iter()
                .map(|(name, _)| digest(&root.join(name), None).unwrap())
                .collect::<Vec<_>>()
        };
        let saved = stamps(&original);
        let mut current = stamps(&relocated);
        assert!(saved.iter().zip(&current).all(
            |(saved, current)| saved.path != current.path && saved.identity != current.identity
        ));
        assert!(tool_contents_match(&saved, &current));
        assert!(saved != current, "ordinary recovery stamps remain strict");

        fs::write(relocated.join("ffmpeg"), b"ffmpeh").unwrap();
        current[0] = digest(&relocated.join("ffmpeg"), None).unwrap();
        assert_eq!(saved[0].length, current[0].length);
        assert!(!tool_contents_match(&saved, &current));

        fs::copy(original.join("ffmpeg"), relocated.join("ffmpeg")).unwrap();
        current = stamps(&relocated);
        current.swap(0, 1);
        assert!(!tool_contents_match(&saved, &current));
        current.swap(0, 1);
        current.pop();
        assert!(!tool_contents_match(&saved, &current));

        // Canonical source-provider selection and engine compatibility updates
        // must never silently reinterpret an existing saved job. The encoder
        // and engine occupy separately stamped roles even if their names match.
        for (role, replacement) in [
            ("encoder", b"new HDR".as_slice()),
            ("av1an", b"new engine".as_slice()),
        ] {
            for (name, _) in tools {
                fs::copy(original.join(name), relocated.join(name)).unwrap();
            }
            fs::write(relocated.join(role), replacement).unwrap();
            assert!(
                !tool_contents_match(&saved, &stamps(&relocated)),
                "changed {role} must fail saved-settings reuse"
            );
        }
    }

    #[test]
    fn target_artifacts_require_metric_scene_bounds_and_sealed_identity() {
        let fixture = Fixture::new();
        let (mut workspace, _intermediate) = fixture.workspace();
        workspace.manifest.settings.av1an_options = Some(media_core::Av1anOptions {
            source_target_defaults: true,
            maximum_chunk_frames: 0,
            target_quality: Some(media_core::Av1anTargetQuality {
                metric: media_core::Av1anTargetMetric::XpsnrWeighted,
                minimum_score_tenths: 350,
                maximum_score_tenths: 350,
                minimum_crf: 15,
                maximum_crf: 50,
                probes: 4,
                probing_rate: 1,
                probe_width: 1920,
                probe_height: 1080,
            }),
            ..Default::default()
        });
        let split = workspace.root.join("chunks/split");
        fs::create_dir_all(&split).unwrap();
        fs::write(workspace.root.join("chunks/scenes.json"), br#"{"frames":48,"scenes":[{"start_frame":0,"end_frame":48,"zone_overrides":null}],"split_scenes":[{"start_frame":0,"end_frame":48,"zone_overrides":null}]}"#).unwrap();
        let stats = split.join("0.json");
        let probe = split.join("v_00000_32.50.ivf");
        fs::write(&stats, b"partial disposable XPSNR stats").unwrap();
        fs::write(&probe, b"partial disposable probe").unwrap();
        verify_action_tree(&workspace.root, &workspace.manifest).unwrap();
        workspace
            .transaction(|workspace| workspace.seal_target_artifacts())
            .unwrap();
        assert_eq!(workspace.manifest.target_artifacts.len(), 2);
        assert!(workspace.manifest.target_scenes.is_some());
        assert!(workspace.manifest.queue.is_none());
        assert!(workspace.manifest.scenes.is_none());
        validate_layout(&workspace.root, &workspace.manifest).unwrap();
        workspace.verify_saved(None).unwrap();
        for foreign in [
            "1.json",
            "00.json",
            "v_00001_32.50.ivf",
            "v_00000_14.ivf",
            "v_00000_51.ivf",
            "v_00000_032.50.ivf",
            "v_00000_32.5.ivf",
            "v_00000_32.50.hevc",
            "arbitrary.json",
        ] {
            let path = split.join(foreign);
            fs::write(&path, b"foreign").unwrap();
            assert!(
                verify_action_tree(&workspace.root, &workspace.manifest).is_err(),
                "adopted {foreign}"
            );
            fs::remove_file(path).unwrap();
        }
        fs::write(&stats, b"replaced stats bytes").unwrap();
        assert!(
            workspace
                .verify_saved(None)
                .unwrap_err()
                .message
                .contains("metric target cache changed")
        );
        assert!(workspace.remove_target_artifacts().is_err());
        assert!(probe.exists(), "cache mismatch must precede any deletion");
        fs::remove_file(&stats).unwrap();
        workspace.verify_saved(None).unwrap(); // Missing disposable files can be regenerated.
        workspace.remove_target_artifacts().unwrap();
        assert!(!probe.exists());
        assert!(workspace.manifest.target_artifacts.is_empty());
        fs::write(&stats, b"XPSNR file").unwrap();
        workspace
            .manifest
            .settings
            .av1an_options
            .as_mut()
            .unwrap()
            .target_quality
            .as_mut()
            .unwrap()
            .metric = media_core::Av1anTargetMetric::Ssimulacra2;
        assert!(
            verify_action_tree(&workspace.root, &workspace.manifest).is_err(),
            "VS-only metric must not adopt FFmpeg stats"
        );
        workspace
            .manifest
            .settings
            .av1an_options
            .as_mut()
            .unwrap()
            .target_quality = None;
        assert!(
            verify_action_tree(&workspace.root, &workspace.manifest).is_err(),
            "CRF-only job must not adopt target stats"
        );
    }

    #[test]
    #[cfg(unix)]
    fn target_probe_symlinks_never_authorize_outside_cache_cleanup() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let (mut workspace, _intermediate) = fixture.workspace();
        workspace.manifest.settings.av1an_options = Some(media_core::Av1anOptions {
            source_target_defaults: true,
            maximum_chunk_frames: 0,
            target_quality: Some(media_core::Av1anTargetQuality {
                metric: media_core::Av1anTargetMetric::Vmaf,
                minimum_score_tenths: 950,
                maximum_score_tenths: 950,
                minimum_crf: 15,
                maximum_crf: 50,
                probes: 4,
                probing_rate: 1,
                probe_width: 1920,
                probe_height: 1080,
            }),
            ..Default::default()
        });
        let split = workspace.root.join("chunks/split");
        fs::create_dir_all(&split).unwrap();
        fs::write(workspace.root.join("chunks/scenes.json"), br#"{"frames":48,"scenes":[{"start_frame":0,"end_frame":48,"zone_overrides":null}],"split_scenes":[{"start_frame":0,"end_frame":48,"zone_overrides":null}]}"#).unwrap();
        let outside = fixture.0.join("outside.json");
        fs::write(&outside, b"outside must survive").unwrap();
        symlink(&outside, split.join("0.json")).unwrap();
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        assert!(workspace.seal_target_artifacts().is_err());
        assert!(workspace.remove_target_artifacts().is_err());
        assert_eq!(fs::read(outside).unwrap(), b"outside must survive");
    }

    #[test]
    #[cfg(unix)]
    fn last_recovery_owner_unlocks_even_with_an_inherited_descriptor() {
        let fixture = Fixture::new();
        let (workspace, intermediate) = fixture.workspace();
        let root = workspace.root.clone();
        let inherited = workspace.lock.as_ref().unwrap().0.try_clone().unwrap();
        let recovery = Recovery {
            root: root.clone(),
            finalizing: false,
            resume_chunks: false,
            inner: Arc::new(StdMutex::new(workspace)),
        };
        let writer = recovery.clone();
        drop(recovery);
        assert!(lock_workspace(&root).is_err());
        drop(writer);
        let (_directory, reopened) = lock_workspace(&root).unwrap();
        drop(inherited);
        assert!(lock_workspace(&root).is_err());
        drop(reopened);
        assert!(lock_workspace(&root).is_ok());
        drop(intermediate);
    }

    #[test]
    fn external_sources_bind_resume_and_missing_stamps_reject_mixed_jobs() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let external = fixture.0.join("external.mka");
        fs::write(&external, b"external-a").unwrap();
        workspace.manifest.settings.external_tracks = vec![
            media_core::ExternalTrack {
                subtitle_burn_in: false,
                offset_milliseconds: 0,
                subtitle_mode: None,
                title: None,
                language: None,
                default: None,
                forced: None,
                audio: None,
                input_path: external.to_string_lossy().into_owned(),
                stream_index: 0,
            },
            media_core::ExternalTrack {
                subtitle_burn_in: false,
                offset_milliseconds: 0,
                subtitle_mode: None,
                title: None,
                language: None,
                default: None,
                forced: None,
                audio: None,
                input_path: external.to_string_lossy().into_owned(),
                stream_index: 1,
            },
        ];
        workspace.manifest.external_sources =
            external_source_stamps(&workspace.manifest.settings, None).unwrap();
        assert_eq!(workspace.manifest.external_sources.len(), 1);
        workspace.save().unwrap();
        let locator = workspace.summary();
        let request = workspace.manifest.request.clone();
        let settings = workspace.manifest.settings.clone();
        let manifest_path = workspace.root.join("manifest.json");

        let (_, loaded) = load("owned-job", &request, &settings, &locator).unwrap();
        verify_external_sources(&loaded, &settings, None).unwrap();
        let saved = fs::read(&manifest_path).unwrap();
        let mut missing: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        missing.as_object_mut().unwrap().remove("external_sources");
        fs::write(&manifest_path, serde_json::to_vec(&missing).unwrap()).unwrap();
        let (_, loaded) = load("owned-job", &request, &settings, &locator).unwrap();
        assert!(verify_external_sources(&loaded, &settings, None).is_err());
        fs::write(&manifest_path, saved).unwrap();

        fs::write(&external, b"external-b").unwrap();
        let (_, loaded) = load("owned-job", &request, &settings, &locator).unwrap();
        assert!(verify_external_sources(&loaded, &settings, None).is_err());
        let moved = fixture.0.join("moved-external.mka");
        fs::rename(&external, &moved).unwrap();
        fs::write(&external, b"external-a").unwrap();
        assert!(verify_external_sources(&loaded, &settings, None).is_err());
        drop(workspace);
        drop(intermediate);
    }

    #[test]
    fn donor_and_hdr_resource_sources_are_fingerprinted_for_av1an_resume() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let metadata = fixture.0.join("metadata.mka");
        let chapters = fixture.0.join("chapters.mka");
        fs::write(&metadata, b"metadata donor").unwrap();
        fs::write(&chapters, b"chapter donor").unwrap();
        let rpu = fixture.0.join("metadata.rpu");
        let hdr_json = fixture.0.join("metadata.json");
        fs::write(&rpu, b"rpu bytes").unwrap();
        fs::write(&hdr_json, b"json bytes").unwrap();
        workspace.manifest.settings.hdr_metadata = Some(media_core::HdrMetadataSettings {
            dolby_vision_rpu_path: Some(rpu.to_string_lossy().into_owned()),
            hdr10_plus_json_path: Some(hdr_json.to_string_lossy().into_owned()),
            ..Default::default()
        });
        workspace.manifest.settings.metadata_source_path =
            Some(metadata.to_string_lossy().into_owned());
        workspace.manifest.settings.chapters_source_path =
            Some(chapters.to_string_lossy().into_owned());
        workspace.manifest.external_sources =
            external_source_stamps(&workspace.manifest.settings, None).unwrap();
        assert_eq!(workspace.manifest.external_sources.len(), 4);
        workspace.save().unwrap();
        let locator = workspace.summary();
        let request = workspace.manifest.request.clone();
        let settings = workspace.manifest.settings.clone();
        let (_, loaded) = load("owned-job", &request, &settings, &locator).unwrap();
        verify_external_sources(&loaded, &settings, None).unwrap();
        drop(workspace);
        drop(intermediate);
        fs::write(&metadata, b"changed donor").unwrap();
        let (_, loaded) = load("owned-job", &request, &settings, &locator).unwrap();
        assert!(verify_external_sources(&loaded, &settings, None).is_err());
        fs::write(&metadata, b"metadata donor").unwrap();
        verify_external_sources(&loaded, &settings, None).unwrap();
        fs::write(&rpu, b"changed rpu bytes").unwrap();
        assert!(verify_external_sources(&loaded, &settings, None).is_err());
        fs::write(&rpu, b"rpu bytes").unwrap();
        fs::write(&hdr_json, b"changed json bytes").unwrap();
        assert!(verify_external_sources(&loaded, &settings, None).is_err());
    }

    #[test]
    fn legacy_manifest_without_borders_matches_default_framing_without_rewriting_receipts() {
        let fixture = Fixture::new();
        let (workspace, intermediate) = fixture.workspace();
        let manifest_path = workspace.root.join("manifest.json");
        let mut legacy = serde_json::to_value(&workspace.manifest).unwrap();
        legacy["settings"]["framing"]
            .as_object_mut()
            .unwrap()
            .remove("borders");
        legacy.as_object_mut().unwrap().remove("external_sources");
        let legacy_bytes = serde_json::to_vec(&legacy).unwrap();
        fs::write(&manifest_path, &legacy_bytes).unwrap();
        let (_, loaded) = load(
            &workspace.manifest.id,
            &workspace.manifest.request,
            &workspace.manifest.settings,
            &workspace.summary(),
        )
        .unwrap();
        assert!(loaded == workspace.manifest);
        verify_external_sources(&loaded, &workspace.manifest.settings, None).unwrap();
        assert_eq!(fs::read(&manifest_path).unwrap(), legacy_bytes);
        let mut changed = workspace.manifest.settings.clone();
        changed.framing.borders.left = 2;
        assert!(
            load(
                &workspace.manifest.id,
                &workspace.manifest.request,
                &changed,
                &workspace.summary(),
            )
            .is_err()
        );
        drop(workspace);
        drop(intermediate);
    }

    #[test]
    fn prepared_source_reuse_requires_owned_file_original_and_decoded_identity() {
        let fixture = Fixture::new();
        let (mut workspace, _intermediate) = fixture.workspace();
        let original = workspace.manifest.source.clone();
        let prepared_path = workspace.root.join("prepared.mkv");
        fs::write(&prepared_path, b"verified decoded source").unwrap();
        workspace.manifest.source = digest(&prepared_path, None).unwrap();
        workspace.manifest.original_source = Some(original.clone());
        workspace.manifest.prepared_identity = Some("sha256:pixels;frames:48".into());
        workspace.manifest.source_filter = None;
        let prepared = PreparedSource {
            path: fixture.0.join("fresh-attempt.mkv"),
            decoded_identity: "sha256:pixels;frames:48".into(),
        };
        validate_layout(&workspace.root, &workspace.manifest).unwrap();
        assert!(
            source_matches_attempt(
                &workspace.manifest,
                &workspace.root,
                &original,
                Some(&prepared),
                None,
            )
            .unwrap()
        );

        let changed = PreparedSource {
            path: prepared.path.clone(),
            decoded_identity: "sha256:changed;frames:48".into(),
        };
        assert!(
            !source_matches_attempt(
                &workspace.manifest,
                &workspace.root,
                &original,
                Some(&changed),
                None,
            )
            .unwrap()
        );
        let saved = workspace.manifest.source.clone();
        fs::write(&prepared_path, b"tampered decoded source").unwrap();
        assert!(
            !source_matches_attempt(
                &workspace.manifest,
                &workspace.root,
                &original,
                Some(&prepared),
                None,
            )
            .unwrap()
        );
        workspace.manifest.source = saved;
        workspace.manifest.source.path = fixture.0.join("foreign-prepared.mkv");
        assert!(validate_layout(&workspace.root, &workspace.manifest).is_err());
    }

    #[test]
    fn manifests_reject_foreign_locations_owners_settings_and_truncation_without_mutation() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let summary = workspace.summary();
        let manifest_path = workspace.root.join("manifest.json");
        load(
            &workspace.manifest.id,
            &workspace.manifest.request,
            &workspace.manifest.settings,
            &summary,
        )
        .unwrap();
        let mut changed = workspace.manifest.settings.clone();
        changed.crf += 1;
        assert!(
            load(
                &workspace.manifest.id,
                &workspace.manifest.request,
                &changed,
                &summary
            )
            .is_err()
        );
        let foreign = Av1anRecovery {
            workspace: fixture.0.to_string_lossy().into_owned(),
            ..summary.clone()
        };
        assert!(
            load(
                &workspace.manifest.id,
                &workspace.manifest.request,
                &workspace.manifest.settings,
                &foreign
            )
            .is_err()
        );
        workspace.manifest.directory.0[0] += 1;
        assert!(workspace.save().is_err());
        workspace.manifest.directory = identity(&workspace.root).unwrap();
        fs::write(&manifest_path, b"{\"version\":1,").unwrap();
        assert!(
            load(
                &workspace.manifest.id,
                &workspace.manifest.request,
                &workspace.manifest.settings,
                &summary
            )
            .is_err()
        );
        assert_eq!(fs::read(&manifest_path).unwrap(), b"{\"version\":1,");
        assert_eq!(
            fs::read(&workspace.manifest.request.input_path).unwrap(),
            b"source bytes"
        );
        drop(intermediate);
        drop(workspace);
    }

    #[test]
    fn finalizing_artifact_hashes_while_owned_writable_handle_is_open_and_survives_drop() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let video = intermediate.path.clone();
        intermediate
            .clone_file()
            .unwrap()
            .write_all(b"completed encoded video")
            .unwrap();
        workspace.manifest.final_video = Some(digest(&video, None).unwrap());
        workspace.manifest.phase = RecoveryPhase::Finalizing;
        workspace.save().unwrap();
        workspace.verify_saved(None).unwrap();
        assert_eq!(workspace.summary().completed_frames, 48);
        assert!(lock_workspace(&workspace.root).is_err());
        drop(intermediate);
        drop(workspace);
        assert_eq!(fs::read(video).unwrap(), b"completed encoded video");
    }

    #[test]
    fn saved_chunk_hashes_reject_same_length_changes_and_unchecked_done_entries_are_removed() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        fs::create_dir_all(workspace.root.join("chunks/encode")).unwrap();
        let chunk = workspace.root.join("chunks/encode/00000.ivf");
        fs::write(&chunk, b"verified chunk").unwrap();
        let other = workspace.root.join("chunks/encode/00001.ivf");
        fs::write(&other, b"uncheckpointed").unwrap();
        workspace.manifest.completed.insert(
            "00000".into(),
            Chunk {
                frames: 24,
                decoded_contract: None,
                file: digest(&chunk, None).unwrap(),
            },
        );
        let done = workspace.root.join("chunks/done.json");
        fs::write(&done,br#"{"frames":48,"done":{"00000":{"frames":24,"size_bytes":14},"00001":{"frames":24,"size_bytes":14}},"audio_done":true}"#).unwrap();
        workspace.sanitize_done().unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&done).unwrap()).unwrap();
        assert_eq!(value["done"].as_object().unwrap().len(), 1);
        assert!(value["done"].get("00001").is_none());
        assert_eq!(fs::read(&other).unwrap(), b"uncheckpointed");
        workspace.verify_saved(None).unwrap();
        fs::write(&chunk, b"tampered chunk").unwrap();
        assert!(workspace.verify_saved(None).is_err());
        let mut escaped = workspace.manifest.completed["00000"].clone();
        escaped.file.path = fixture.0.join("source.mkv");
        workspace.manifest.completed.insert("00000".into(), escaped);
        assert!(validate_layout(&workspace.root, &workspace.manifest).is_err());
        drop(intermediate);
        drop(workspace);
    }

    #[test]
    fn video_only_resume_removes_stale_audio_and_preserves_video_source_and_receipts() {
        let fixture = Fixture::new();
        let (workspace, intermediate) = fixture.workspace();
        fs::create_dir_all(workspace.root.join("chunks/encode")).unwrap();
        let chunk = workspace.root.join("chunks/encode/00000.ivf");
        fs::write(&chunk, b"completed video chunk").unwrap();
        let before = digest(&chunk, None).unwrap();
        let manifest = fs::read(workspace.root.join("manifest.json")).unwrap();
        let audio = workspace.root.join("chunks/audio.mkv");
        fs::write(&audio, [0xa5; 2048]).unwrap();
        let neighbor = fixture.0.join("audio.mkv");
        fs::write(&neighbor, b"unrelated audio").unwrap();

        workspace.prepare_video_only_resume(1).unwrap();
        workspace.prepare_video_only_resume(1).unwrap();

        assert!(!audio.exists());
        assert_eq!(digest(&chunk, None).unwrap(), before);
        assert_eq!(
            fs::read(workspace.root.join("manifest.json")).unwrap(),
            manifest
        );
        assert_eq!(fs::read(neighbor).unwrap(), b"unrelated audio");
        assert_eq!(
            fs::read(&workspace.manifest.source.path).unwrap(),
            b"source bytes"
        );
        drop(intermediate);
        drop(workspace);
    }

    #[test]
    fn video_only_resume_rejects_foreign_chunk_entries_audio_directories_and_wrong_owner() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        fs::create_dir_all(workspace.root.join("chunks/encode")).unwrap();
        let audio = workspace.root.join("chunks/audio.mkv");
        fs::write(&audio, b"retained on rejected recovery").unwrap();
        for name in ["00001.ivf", "00000.mkv", "foreign.txt", "0.ivf"] {
            let unexpected = workspace.root.join("chunks/encode").join(name);
            fs::write(&unexpected, b"unrelated").unwrap();
            assert!(workspace.prepare_video_only_resume(1).is_err());
            assert_eq!(fs::read(&unexpected).unwrap(), b"unrelated");
            assert_eq!(fs::read(&audio).unwrap(), b"retained on rejected recovery");
            fs::remove_file(unexpected).unwrap();
        }
        fs::remove_file(&audio).unwrap();
        fs::create_dir(&audio).unwrap();
        fs::write(audio.join("foreign.txt"), b"preserve this directory").unwrap();
        assert!(workspace.prepare_video_only_resume(1).is_err());
        assert_eq!(
            fs::read(audio.join("foreign.txt")).unwrap(),
            b"preserve this directory"
        );
        workspace.manifest.directory.0[0] += 1;
        assert!(workspace.prepare_video_only_resume(1).is_err());
        assert!(audio.is_dir());
        drop(intermediate);
        drop(workspace);
    }

    #[test]
    #[cfg(unix)]
    fn video_only_resume_rejects_audio_links_without_touching_the_target() {
        let fixture = Fixture::new();
        let (workspace, intermediate) = fixture.workspace();
        fs::create_dir_all(workspace.root.join("chunks/encode")).unwrap();
        let audio = workspace.root.join("chunks/audio.mkv");
        std::os::unix::fs::symlink(&workspace.manifest.source.path, &audio).unwrap();
        assert!(workspace.prepare_video_only_resume(1).is_err());
        assert!(
            fs::symlink_metadata(&audio)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(&workspace.manifest.source.path).unwrap(),
            b"source bytes"
        );
        drop(intermediate);
        drop(workspace);
    }

    #[test]
    #[cfg(windows)]
    fn checkpointed_chunk_guards_prevent_replacement_until_cleanup() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        fs::create_dir_all(workspace.root.join("chunks/encode")).unwrap();
        let chunk = workspace.root.join("chunks/encode/00000.ivf");
        fs::write(&chunk, b"verified chunk").unwrap();
        workspace.chunks.push(Source::open(&chunk).unwrap());
        assert!(fs::write(&chunk, b"changed bytes!").is_err());
        assert!(fs::remove_file(&chunk).is_err());
        workspace.chunks[0].verify().unwrap();
        assert_eq!(fs::read(&chunk).unwrap(), b"verified chunk");
        workspace.chunks.clear();
        fs::write(&chunk, b"released for cleanup").unwrap();
        drop(intermediate);
        drop(workspace);
    }

    #[test]
    fn external_reader_cache_is_owned_and_fingerprinted_for_resume() {
        for method in [
            media_core::Av1anChunkMethod::Ffms2,
            media_core::Av1anChunkMethod::Bestsource,
        ] {
            let fixture = Fixture::new();
            let (mut workspace, intermediate) = fixture.workspace();
            workspace.manifest.settings.av1an_options = Some(media_core::Av1anOptions {
                chunk_method: method,
                ..Default::default()
            });
            let chunks = workspace.root.join("chunks");
            fs::create_dir_all(chunks.join("split")).unwrap();
            for (relative, contents) in [
                ("chunks.json", b"queue".as_slice()),
                ("scenes.json", b"scenes".as_slice()),
                ("split/loadscript.vpy", b"script".as_slice()),
            ] {
                fs::write(chunks.join(relative), contents).unwrap();
            }
            workspace.manifest.queue = Some(digest(&chunks.join("chunks.json"), None).unwrap());
            workspace.manifest.scenes = Some(digest(&chunks.join("scenes.json"), None).unwrap());
            workspace.manifest.script =
                Some(digest(&chunks.join("split/loadscript.vpy"), None).unwrap());
            let reader = super::super::recovery_receipts::reader_cache(method)
                .expect("external reader cache kind");
            let cache =
                chunks
                    .join("split")
                    .join(if method == media_core::Av1anChunkMethod::Ffms2 {
                        "cache.ffindex"
                    } else {
                        "cache.bsindex.7.bsindex"
                    });
            fs::write(&cache, b"reader index").unwrap();
            assert_eq!(external_reader_cache_path(&chunks, reader).unwrap(), cache);
            let cache_stamp = digest(&cache, None).unwrap();
            workspace.manifest.reader_cache = Some(cache_stamp.clone());
            validate_layout(&workspace.root, &workspace.manifest).unwrap();
            workspace.verify_saved(None).unwrap();

            workspace.manifest.reader_cache = None;
            assert!(validate_layout(&workspace.root, &workspace.manifest).is_err());
            workspace.manifest.reader_cache = Some(cache_stamp.clone());

            let extra =
                chunks
                    .join("split")
                    .join(if method == media_core::Av1anChunkMethod::Ffms2 {
                        "cache.bsindex.0.bsindex"
                    } else {
                        "cache.ffindex"
                    });
            fs::write(&extra, b"unreported reader index").unwrap();
            assert!(external_reader_cache_path(&chunks, reader).is_err());
            fs::remove_file(extra).unwrap();

            let foreign = chunks.join("split/foreign.index");
            fs::write(&foreign, b"foreign").unwrap();
            workspace.manifest.reader_cache = Some(digest(&foreign, None).unwrap());
            assert!(validate_layout(&workspace.root, &workspace.manifest).is_err());
            workspace.manifest.reader_cache = Some(cache_stamp);

            fs::write(&cache, b"changed reader index").unwrap();
            assert!(workspace.verify_saved(None).is_err());
            drop(intermediate);
            drop(workspace);
        }
    }

    #[test]
    fn segment_checkpoint_paths_are_owned_and_require_a_queue() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        workspace.manifest.settings.av1an_options = Some(media_core::Av1anOptions {
            chunk_method: media_core::Av1anChunkMethod::Segment,
            ..Default::default()
        });
        let split = workspace.root.join("chunks/split");
        fs::create_dir_all(&split).unwrap();
        let segment = split.join("0.mkv");
        fs::write(&segment, b"owned segment").unwrap();
        workspace
            .manifest
            .segments
            .push(digest(&segment, None).unwrap());
        assert!(validate_layout(&workspace.root, &workspace.manifest).is_err());
        let queue = workspace.root.join("chunks/chunks.json");
        let scenes = workspace.root.join("chunks/scenes.json");
        fs::write(&queue, b"queue").unwrap();
        fs::write(&scenes, b"scenes").unwrap();
        workspace.manifest.queue = Some(digest(&queue, None).unwrap());
        workspace.manifest.scenes = Some(digest(&scenes, None).unwrap());
        validate_layout(&workspace.root, &workspace.manifest).unwrap();
        workspace.manifest.segments[0].path = split.join("foreign.mkv");
        assert!(validate_layout(&workspace.root, &workspace.manifest).is_err());
        drop(intermediate);
        drop(workspace);
    }

    #[test]
    fn segment_verification_list_is_owned_only_by_its_exact_sealed_recipe() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let split = workspace.root.join("chunks/split");
        fs::create_dir_all(&split).unwrap();
        for name in ["00000.mkv", "00001.mkv"] {
            let path = split.join(name);
            fs::write(&path, b"sealed source segment").unwrap();
            workspace
                .manifest
                .segments
                .push(digest(&path, None).unwrap());
        }
        let queue = workspace.root.join("chunks/chunks.json");
        fs::write(&queue, b"[{},{}]").unwrap();
        workspace.manifest.queue = Some(digest(&queue, None).unwrap());
        let list = workspace.root.join("segments.ffconcat");
        let expected =
            b"ffconcat version 1.0\nfile chunks/split/00000.mkv\nfile chunks/split/00001.mkv\n";
        fs::write(&list, expected).unwrap();
        verify_action_tree(&workspace.root, &workspace.manifest).unwrap();

        let foreign = b"ffconcat version 1.0\nfile ../../foreign.mkv\n";
        fs::write(&list, foreign).unwrap();
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        assert_eq!(fs::read(&list).unwrap(), foreign);
        assert_eq!(workspace.manifest.segments.len(), 2);
        fs::write(&list, expected).unwrap();
        let saved = workspace.manifest.segments[0].clone();
        workspace.manifest.segments[0].identity.0[0] ^= 1;
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        workspace.manifest.segments[0] = saved.clone();
        workspace.manifest.segments[0].path = workspace.root.join("../foreign.mkv");
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        workspace.manifest.segments[0] = saved;
        let extra = workspace.root.join("foreign.ffconcat");
        fs::write(&extra, expected).unwrap();
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        assert_eq!(fs::read(&extra).unwrap(), expected);
        fs::remove_file(&extra).unwrap();
        verify_action_tree(&workspace.root, &workspace.manifest).unwrap();
        fs::remove_file(&list).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&split, &list).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let result = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&list)
                .arg(&split)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        drop(intermediate);
        drop(workspace);

        let fixture = Fixture::new();
        let (workspace, intermediate) = fixture.workspace();
        let list = workspace.root.join("segments.ffconcat");
        fs::write(&list, b"ffconcat version 1.0\n").unwrap();
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        assert_eq!(fs::read(&list).unwrap(), b"ffconcat version 1.0\n");
        drop(intermediate);
        drop(workspace);
    }

    fn stage_receipted_launch(workspace: &mut Workspace, fixture: &Fixture) -> PathBuf {
        let engine = fixture.0.join("selected-av1an.exe");
        fs::write(&engine, b"verified selected engine").unwrap();
        workspace.manifest.tools = vec![digest(&engine, None).unwrap(); 4];
        let launch = workspace.root.join("launch-123-456");
        fs::create_dir(&launch).unwrap();
        let image = launch.join("av1an.exe");
        fs::copy(&engine, &image).unwrap();
        let mut value = serde_json::to_value(&workspace.manifest).unwrap();
        value["launch_artifacts"] = serde_json::json!([{
            "path": launch,
            "identity": identity(&launch).unwrap(),
            "image": digest(&image, None).unwrap(),
        }]);
        workspace.manifest = serde_json::from_value(value).unwrap();
        workspace.save().unwrap();
        launch
    }

    #[tokio::test]
    async fn persisted_launch_receipt_allows_crash_cleanup_without_changing_source_or_neighbor() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let launch = stage_receipted_launch(&mut workspace, &fixture);
        let root = workspace.root.clone();
        let request = workspace.manifest.request.clone();
        let settings = workspace.manifest.settings.clone();
        let locator = workspace.summary();
        verify_action_tree(&root, &workspace.manifest).unwrap();
        let neighbor = fixture.0.join("neighbor.txt");
        fs::write(&neighbor, b"preserve neighboring bytes").unwrap();
        drop(workspace);
        drop(intermediate);
        let (_, saved) = load("owned-job", &request, &settings, &locator).unwrap();
        verify_action_tree(&root, &saved).unwrap();
        assert_eq!(
            fs::read(launch.join("av1an.exe")).unwrap(),
            b"verified selected engine"
        );
        Recovery::discard("owned-job", &request, &settings, &locator)
            .await
            .unwrap();
        assert!(!root.exists());
        assert_eq!(fs::read(request.input_path).unwrap(), b"source bytes");
        assert_eq!(fs::read(neighbor).unwrap(), b"preserve neighboring bytes");
    }

    #[test]
    fn unrecorded_empty_launch_directory_cannot_authorize_recovery_cleanup() {
        let fixture = Fixture::new();
        let (workspace, _intermediate) = fixture.workspace();
        // Legacy manifests with no staged artifacts remain valid.
        validate_layout(&workspace.root, &workspace.manifest).unwrap();
        verify_action_tree(&workspace.root, &workspace.manifest).unwrap();
        let foreign = workspace.root.join("launch-123-456");
        fs::create_dir(&foreign).unwrap();
        assert_eq!(
            verify_action_tree(&workspace.root, &workspace.manifest)
                .unwrap_err()
                .code,
            "RECOVERY_INVALID"
        );
        assert!(foreign.is_dir());
    }

    #[test]
    fn launch_reservation_before_staging_failure_owns_only_the_exact_empty_directory() {
        let fixture = Fixture::new();
        let (mut workspace, _intermediate) = fixture.workspace();
        let launch = stage_receipted_launch(&mut workspace, &fixture);
        fs::remove_file(launch.join("av1an.exe")).unwrap();
        workspace.manifest.launch_artifacts[0].image = None;
        workspace.save().unwrap();
        verify_action_tree(&workspace.root, &workspace.manifest).unwrap();
        let partial = launch.join("av1an.exe");
        fs::write(&partial, b"unsealed partial image").unwrap();
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        assert!(remove_workspace_tree(&workspace.root, &workspace.manifest, None).is_err());
        assert_eq!(fs::read(partial).unwrap(), b"unsealed partial image");
    }

    #[tokio::test]
    async fn reserved_launch_remains_recoverable_after_prepare_fails_before_sealing() {
        let fixture = Fixture::new();
        let (workspace, intermediate) = fixture.workspace();
        let request = workspace.manifest.request.clone();
        let settings = workspace.manifest.settings.clone();
        let locator = workspace.summary();
        let root = workspace.root.clone();
        let recovery = Recovery {
            root: root.clone(),
            finalizing: false,
            resume_chunks: false,
            inner: Arc::new(StdMutex::new(workspace)),
        };
        let launch = root.join("launch-123-456");
        let reservation = recovery.reserve_launch(&launch).await.unwrap();
        let cause = super::super::launcher::Launch::prepare(
            &fixture.0.join("av1an.exe"),
            &fixture.0.join("wrong-encoder-name"),
            VideoEncoder::SvtAv1,
            &fixture.0.join("ffmpeg.exe"),
            &fixture.0.join("ffprobe.exe"),
            None,
            &launch,
        )
        .err()
        .expect("the invalid canonical encoder fails before staging");
        assert_eq!(cause.code, "AV1AN_ENCODER_PATH_UNSUPPORTED");
        drop(reservation);
        drop(recovery);
        drop(intermediate);
        let (_, saved) = load("owned-job", &request, &settings, &locator).unwrap();
        assert_eq!(saved.launch_artifacts.len(), 1);
        assert!(saved.launch_artifacts[0].image.is_none());
        verify_action_tree(&root, &saved).unwrap();
        Recovery::discard("owned-job", &request, &settings, &locator)
            .await
            .unwrap();
        assert!(!root.exists());
        assert_eq!(fs::read(request.input_path).unwrap(), b"source bytes");
    }

    #[tokio::test]
    async fn launch_retirement_removes_only_the_owned_empty_directory_and_retires_its_authority() {
        for mutation in ["empty", "replacement", "foreign"] {
            let fixture = Fixture::new();
            let (workspace, _intermediate) = fixture.workspace();
            let root = workspace.root.clone();
            let recovery = Recovery {
                root: root.clone(),
                finalizing: false,
                resume_chunks: false,
                inner: Arc::new(StdMutex::new(workspace)),
            };
            let launch = root.join("launch-123-456");
            drop(recovery.reserve_launch(&launch).await.unwrap());
            if mutation == "replacement" {
                fs::rename(&launch, fixture.0.join("original-launch")).unwrap();
                fs::create_dir(&launch).unwrap();
            }
            if mutation != "empty" {
                fs::write(launch.join("sentinel"), b"preserve foreign bytes").unwrap();
            }
            let result = recovery.retire_launch(&launch).await;
            if mutation == "empty" {
                result.unwrap();
                assert!(!launch.exists());
                fs::create_dir(&launch).unwrap();
                recovery
                    .with(|w| {
                        assert!(w.manifest.launch_artifacts.is_empty());
                        assert!(verify_action_tree(&w.root, &w.manifest).is_err());
                        Ok(())
                    })
                    .await
                    .unwrap();
            } else {
                assert_eq!(result.unwrap_err().code, "RECOVERY_INVALID");
                assert_eq!(
                    fs::read(launch.join("sentinel")).unwrap(),
                    b"preserve foreign bytes"
                );
            }
        }
    }

    #[test]
    fn launch_receipts_reject_changed_bytes_replacements_foreign_entries_and_unbound_paths() {
        for mutation in [
            "bytes",
            "image",
            "directory",
            "file",
            "subdirectory",
            "path",
            "duplicate",
            "tool",
        ] {
            let fixture = Fixture::new();
            let (mut workspace, _intermediate) = fixture.workspace();
            let launch = stage_receipted_launch(&mut workspace, &fixture);
            let image = launch.join("av1an.exe");
            match mutation {
                "bytes" => fs::write(&image, b"modified selected engine").unwrap(),
                "image" => {
                    fs::rename(&image, fixture.0.join("original-image")).unwrap();
                    fs::write(&image, b"verified selected engine").unwrap();
                }
                "directory" => {
                    fs::rename(&launch, fixture.0.join("original-launch")).unwrap();
                    fs::create_dir(&launch).unwrap();
                    fs::write(&image, b"verified selected engine").unwrap();
                }
                "file" => fs::write(launch.join("foreign.txt"), b"preserve foreign bytes").unwrap(),
                "subdirectory" => fs::create_dir(launch.join("foreign")).unwrap(),
                "path" => {
                    // PathBuf::push normalizes parent components for Windows
                    // verbatim paths. Preserve the literal untrusted receipt.
                    let mut traversal = workspace.root.as_os_str().to_owned();
                    traversal.push(if cfg!(windows) {
                        r"\nested\..\launch-123-456"
                    } else {
                        "/nested/../launch-123-456"
                    });
                    workspace.manifest.launch_artifacts[0].path = PathBuf::from(traversal);
                }
                "duplicate" => workspace
                    .manifest
                    .launch_artifacts
                    .push(workspace.manifest.launch_artifacts[0].clone()),
                "tool" => workspace.manifest.tools[3].sha256 = "0".repeat(64),
                _ => unreachable!(),
            }
            let before = fs::read(&image).unwrap();
            let cause = verify_action_tree(&workspace.root, &workspace.manifest)
                .err()
                .unwrap_or_else(|| panic!("launch mutation was accepted: {mutation}"));
            assert_eq!(cause.code, "RECOVERY_INVALID", "{mutation}: {cause:?}");
            assert!(
                remove_workspace_tree(&workspace.root, &workspace.manifest, None).is_err(),
                "{mutation}"
            );
            assert_eq!(fs::read(&image).unwrap(), before, "{mutation}");
            assert!(workspace.root.join("manifest.json").exists(), "{mutation}");
            if mutation == "file" {
                assert_eq!(
                    fs::read(launch.join("foreign.txt")).unwrap(),
                    b"preserve foreign bytes"
                );
            }
        }
    }

    #[test]
    fn launch_cleanup_rechecks_same_byte_replacement_after_enumeration_before_any_deletion() {
        let fixture = Fixture::new();
        let (mut workspace, _intermediate) = fixture.workspace();
        let launch = stage_receipted_launch(&mut workspace, &fixture);
        let image = launch.join("av1an.exe");
        let original = fixture.0.join("original-image");
        let replace = image.clone();
        REMOVE_ENUMERATED_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(&replace, original).unwrap();
                fs::write(replace, b"verified selected engine").unwrap();
            }))
        });
        let cause = remove_workspace_tree(&workspace.root, &workspace.manifest, None).unwrap_err();
        assert_eq!(cause.code, "RECOVERY_INVALID");
        assert_eq!(fs::read(&image).unwrap(), b"verified selected engine");
        assert!(workspace.root.join("manifest.json").exists());
        assert!(workspace.root.join("video.ivf").exists());
    }

    #[test]
    fn launch_cleanup_preserves_a_foreign_entry_inserted_after_enumeration() {
        let fixture = Fixture::new();
        let (mut workspace, _intermediate) = fixture.workspace();
        let launch = stage_receipted_launch(&mut workspace, &fixture);
        let foreign = launch.join("injected.txt");
        let injected = foreign.clone();
        REMOVE_ENUMERATED_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::write(injected, b"preserve injected bytes").unwrap();
            }))
        });
        assert!(remove_workspace_tree(&workspace.root, &workspace.manifest, None).is_err());
        assert_eq!(fs::read(foreign).unwrap(), b"preserve injected bytes");
        assert_eq!(
            fs::read(launch.join("av1an.exe")).unwrap(),
            b"verified selected engine"
        );
        assert!(workspace.root.join("manifest.json").exists());
    }

    #[test]
    fn launch_receipt_allows_missing_cleanup_entries_but_rejects_their_recreation() {
        let fixture = Fixture::new();
        let (mut workspace, _intermediate) = fixture.workspace();
        let launch = stage_receipted_launch(&mut workspace, &fixture);
        let image = launch.join("av1an.exe");
        fs::rename(&image, fixture.0.join("original-image")).unwrap();
        verify_action_tree(&workspace.root, &workspace.manifest).unwrap();
        fs::write(&image, b"verified selected engine").unwrap();
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        fs::remove_file(&image).unwrap();
        fs::rename(&launch, fixture.0.join("original-launch")).unwrap();
        verify_action_tree(&workspace.root, &workspace.manifest).unwrap();
        fs::create_dir(&launch).unwrap();
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        assert!(launch.is_dir());
    }

    #[test]
    fn launch_receipt_rejects_directory_links_without_touching_their_target() {
        let fixture = Fixture::new();
        let (mut workspace, _intermediate) = fixture.workspace();
        let launch = stage_receipted_launch(&mut workspace, &fixture);
        fs::rename(&launch, fixture.0.join("original-launch")).unwrap();
        let outside = fixture.0.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"preserve outside bytes").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &launch).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let result = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&launch)
                .arg(&outside)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        assert!(verify_action_tree(&workspace.root, &workspace.manifest).is_err());
        assert!(remove_workspace_tree(&workspace.root, &workspace.manifest, None).is_err());
        assert_eq!(
            fs::read(outside.join("sentinel")).unwrap(),
            b"preserve outside bytes"
        );
    }

    #[test]
    fn failed_checkpoint_write_never_advances_in_memory_saved_progress() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let before = workspace.summary();
        let manifest = workspace.root.join("manifest.json");
        let saved = workspace.root.join("last-good.json");
        fs::rename(&manifest, &saved).unwrap();
        fs::create_dir(&manifest).unwrap();
        let old_bytes = fs::read(&saved).unwrap();
        let result = workspace.transaction(|w| {
            w.manifest.final_video = Some(digest(&w.root.join("video.ivf"), None)?);
            w.manifest.phase = RecoveryPhase::Finalizing;
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(workspace.summary(), before);
        assert!(workspace.manifest.final_video.is_none());
        assert_eq!(fs::read(saved).unwrap(), old_bytes);
        drop(intermediate);
        drop(workspace);
    }

    #[test]
    fn cleanup_refuses_a_replaced_root_and_preserves_foreign_contents() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let expected = workspace.manifest.directory.clone();
        drop(intermediate);
        drop(workspace.lock.take());
        drop(workspace.directory_guard.take());
        let retained = workspace.root.with_extension("retained");
        fs::rename(&workspace.root, &retained).unwrap();
        fs::create_dir(&workspace.root).unwrap();
        let foreign = workspace.root.join("not-owned.txt");
        fs::write(&foreign, b"retain").unwrap();
        assert!(remove_tree(&workspace.root, &expected, None).is_err());
        assert_eq!(fs::read(foreign).unwrap(), b"retain");
        assert!(retained.join("video.ivf").exists());
    }

    #[test]
    fn prepared_copy_cancellation_removes_only_its_owned_destination() {
        let fixture = Fixture::new();
        let destination = fixture.0.join("prepared.mkv");
        let (_, cancel) = watch::channel(true);
        let result: Result<(), AppError> = with_owned_output(&destination, |output| {
            output.write_all(b"partial").unwrap();
            check_cancel(&cancel)?;
            Ok(())
        });
        assert_eq!(result.unwrap_err().code, "JOB_CANCELED");
        assert!(!destination.exists());

        #[cfg(unix)]
        {
            let result: Result<(), AppError> = with_owned_output(&destination, |output| {
                output.write_all(b"owned").unwrap();
                fs::remove_file(&destination).unwrap();
                fs::write(&destination, b"replacement").unwrap();
                Err(error(&destination, "forced prepared-copy failure"))
            });
            assert!(result.is_err());
            assert_eq!(fs::read(destination).unwrap(), b"replacement");
        }
        #[cfg(windows)]
        {
            let owned_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .unwrap();
            let owned = file_identity(&owned_file).unwrap();
            drop(owned_file);
            fs::remove_file(&destination).unwrap();
            fs::write(&destination, b"replacement").unwrap();
            assert!(remove_tree(&destination, &owned, None).is_err());
            assert_eq!(fs::read(destination).unwrap(), b"replacement");
        }
    }

    #[test]
    fn fingerprint_cancellation_and_owned_tree_cleanup_preserve_source_and_neighbors() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let (_, cancel) = watch::channel(true);
        assert_eq!(
            digest(&workspace.manifest.source.path, Some(&cancel))
                .unwrap_err()
                .code,
            "JOB_CANCELED"
        );
        let neighbor = fixture.0.join("unrelated.txt");
        fs::write(&neighbor, b"retain").unwrap();
        fs::create_dir(workspace.root.join("chunks")).unwrap();
        fs::write(workspace.root.join("chunks/leftover"), b"owned").unwrap();
        drop(intermediate);
        drop(workspace.lock.take());
        drop(workspace.directory_guard.take());
        remove_tree(&workspace.root, &workspace.manifest.directory, None).unwrap();
        assert_eq!(fs::read(neighbor).unwrap(), b"retain");
        assert_eq!(
            fs::read(&workspace.manifest.source.path).unwrap(),
            b"source bytes"
        );
        assert!(!workspace.root.exists());
    }
}
