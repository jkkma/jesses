//! Durable receipts for an explicitly resumed av1an job. A history locator is
//! never sufficient authority to run saved commands or remove a directory.
use super::*;
use crate::jobs::files::WorkspaceLock;
use media_core::{Av1anRecovery, RecoveryPhase};
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
            .read(true)
            .custom_flags(0x02000000)
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
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Chunk {
    frames: u64,
    file: Stamp,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    id: String,
    directory: Identity,
    request: RemuxRequest,
    settings: EncodeSettings,
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
    /// Decoded-pixel identity of a freshly regenerated prepared source. This
    /// prevents old chunks from surviving a plugin/filter output change while
    /// remaining independent of Matroska IDs and path spelling.
    #[serde(default)]
    prepared_identity: Option<String>,
    tools: Vec<Stamp>,
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
    completed: BTreeMap<String, Chunk>,
    #[serde(default)]
    segments: Vec<Stamp>,
    intermediate: Identity,
    final_video: Option<Stamp>,
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
        || manifest.settings != *settings
        || manifest.total_frames == 0
    {
        return Err(error(
            &root,
            "Recovery ownership, source selection, or settings do not match this job.",
        ));
    }
    validate_layout(&root, &manifest)?;
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
    } else if manifest.original_source.is_some() {
        return Err(error(
            root,
            "An original-source recovery stamp is present without a prepared source.",
        ));
    }
    let expected = [
        ("chunks/chunks.json", manifest.queue.as_ref()),
        ("chunks/scenes.json", manifest.scenes.as_ref()),
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
    let options = manifest.settings.av1an_options.unwrap_or_default();
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
                    super::encoder::chunk_extension(manifest.settings.encoder)
                ))
            || chunk.frames == 0
            || chunk.frames > manifest.total_frames
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
    if completed > manifest.total_frames {
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

fn source_matches_attempt(
    manifest: &Manifest,
    root: &Path,
    original_source: &Stamp,
    prepared: Option<&PreparedSource>,
    cancel: Option<&watch::Receiver<bool>>,
) -> Result<bool, AppError> {
    if let Some(prepared) = prepared {
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
                self.manifest.total_frames
            } else {
                self.manifest.completed.values().map(|c| c.frames).sum()
            },
            total_frames: self.manifest.total_frames,
        }
    }
    fn receipts(&self) -> Result<super::recovery_receipts::Receipts, AppError> {
        let chunks = self.root.join("chunks");
        identity(&chunks)?;
        identity(&chunks.join("split"))?;
        let options = self.manifest.settings.av1an_options.unwrap_or_default();
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
                source: &self.manifest.source.path,
                chunks_directory: &chunks,
                video_params: &self.manifest.params,
                encoder: self.manifest.settings.encoder,
                pixel_format: &self.manifest.pixel_format,
                total_frames: self.manifest.total_frames,
                fps_num: self.manifest.fps_num,
                fps_den: self.manifest.fps_den,
                source_fps_num: self.source_rate.0,
                source_fps_den: self.source_rate.1,
                script_text,
                options,
                source_filter: self.manifest.source_filter.as_deref(),
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
        if receipts.total_frames != self.manifest.total_frames
            || receipts.completed_frames > receipts.total_frames
            || receipts.queued_chunks == 0
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
                super::encoder::chunk_extension(self.manifest.settings.encoder)
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
            self.manifest.completed.insert(
                chunk.name,
                Chunk {
                    frames: chunk.frames,
                    file: stamp,
                },
            );
            self.chunks.push(guard);
        }
        Ok(())
    }
    fn verify_saved(&self, cancel: Option<&watch::Receiver<bool>>) -> Result<(), AppError> {
        for stamp in self
            .manifest
            .queue
            .iter()
            .chain(self.manifest.scenes.iter())
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
        atomic(&path,&serde_json::to_vec(&serde_json::json!({"frames":self.manifest.total_frames,"done":done,"audio_done":false})).expect("serializable receipts"))
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
                                    super::encoder::chunk_extension(self.manifest.settings.encoder)
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

impl Recovery {
    /// Delete only the workspace bound to this saved job. Discarding progress
    /// intentionally does not read the original media or installed tools: a
    /// missing source must not prevent removal of an owned recovery directory.
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
            remove_tree(&root, &manifest.directory, Some(directory))
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
        let source_filter = prepared.is_none().then(|| plan.decoder_filter()).flatten();
        tokio::task::spawn_blocking(move || {
            let original_source = digest(&source, Some(&cancel))?;
            let external_sources = external_source_stamps(&settings, Some(&cancel))?;
            let tool_guards = tools
                .iter()
                .map(|path| Source::open(path))
                .collect::<Result<Vec<_>, _>>()?;
            let tool_stamps = tool_guards
                .iter()
                .map(|tool| digest(&tool.path, Some(&cancel)))
                .collect::<Result<Vec<_>, _>>()?;
            let (root, manifest, intermediate) = if let Some(locator) = locator {
                let (root, manifest) = load(&id, &request, &settings, &locator)?;
                let source_matches = source_matches_attempt(
                    &manifest,
                    &root,
                    &original_source,
                    prepared.as_ref(),
                    Some(&cancel),
                )?;
                if !source_matches
                    || manifest.external_sources != external_sources
                    || !tool_contents_match(&manifest.tools, &tool_stamps)
                    || manifest.params != params
                    || manifest.pixel_format != pixel_format
                    || manifest.fps_num != fps_num
                    || manifest.fps_den != fps_den
                    || manifest.total_frames != frame_count as u64
                    || manifest.source_filter != source_filter
                {
                    return Err(error(
                        &root,
                        "Source content, prepared decoded pixels, selected tools, encoder parameters, or frame timing changed; this job cannot reuse old chunks.",
                    ));
                }
                if identity(&video_path(&root, &settings))? != manifest.intermediate {
                    return Err(error(&root, "The durable video intermediate was replaced."));
                }
                let intermediate = Temporary::durable(&video_path(&root, &settings), true)?;
                (root, manifest, intermediate)
            } else {
                let root = root_for(&id, &request)?;
                fs::create_dir(&root).map_err(|e| error(&root, e.to_string()))?;
                let intermediate = Temporary::durable(&video_path(&root, &settings), false)?;
                let (source_stamp, saved_original, prepared_identity) =
                    if let Some(prepared) = &prepared {
                        (
                            copy_prepared(&prepared.path, &root.join("prepared.mkv"), &cancel)?,
                            Some(original_source),
                            Some(prepared.decoded_identity.clone()),
                        )
                    } else {
                        (original_source, None, None)
                    };
                let manifest = Manifest {
                    version: 1,
                    id,
                    directory: identity(&root)?,
                    request,
                    settings,
                    source: source_stamp,
                    external_sources,
                    original_source: saved_original,
                    prepared_identity,
                    tools: tool_stamps,
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
                    segments: Vec::new(),
                    completed: BTreeMap::new(),
                    intermediate: identity(&intermediate.path)?,
                    final_video: None,
                };
                (root, manifest, intermediate)
            };
            let input = manifest.source.path.clone();
            let source_guard = Source::open(&input)?;
            let (directory_guard, lock) = lock_workspace(&root)?;
            let chunk_guards = manifest
                .completed
                .values()
                .map(|chunk| &chunk.file.path)
                .chain(manifest.segments.iter().map(|stamp| &stamp.path))
                .chain(manifest.reader_cache.iter().map(|stamp| &stamp.path))
                .map(|path| Source::open(path))
                .collect::<Result<Vec<_>, _>>()?;
            let workspace = Workspace {
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
            let resume_chunks=workspace.manifest.queue.is_some();
            let finalizing=workspace.manifest.phase==RecoveryPhase::Finalizing;
            if !finalizing {
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
    pub(in crate::jobs) async fn reader_cache_path(&self) -> Result<Option<PathBuf>, AppError> {
        self.with(|w| {
            Ok(w.manifest
                .reader_cache
                .as_ref()
                .map(|stamp| stamp.path.clone()))
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
        let segments = self
            .with(|w| {
                for guard in &w.chunks {
                    guard.verify()?;
                }
                Ok(if w.segments_verified {
                    Vec::new()
                } else {
                    w.manifest
                        .segments
                        .iter()
                        .map(|stamp| stamp.path.clone())
                        .collect::<Vec<_>>()
                })
            })
            .await?;
        if segments.is_empty() {
            return Ok(());
        }
        let list = self.root.join("segments.ffconcat");
        let text = format!(
            "ffconcat version 1.0\n{}",
            segments
                .iter()
                .map(|path| format!(
                    "file chunks/split/{}\n",
                    path.file_name()
                        .expect("validated segment name")
                        .to_string_lossy()
                ))
                .collect::<String>()
        );
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
    pub(in crate::jobs) async fn finalizing(&self) -> Result<Av1anRecovery, AppError> {
        self.with(|w| {
            w.transaction(|w| {
                for chunk in &w.chunks {
                    chunk.verify()?;
                }
                w.manifest.final_video =
                    Some(digest(&video_path(&w.root, &w.manifest.settings), None)?);
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
            remove_tree(&w.root, &w.manifest.directory, w.directory_guard.take())
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
    ) -> Result<(), AppError> {
        let metadata = fs::symlink_metadata(path).map_err(|e| error(path, e.to_string()))?;
        let directory = metadata.is_dir();
        let guard = match guard {
            Some(guard) => Some(guard),
            None if directory => Some(directory_guard(path)?),
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
                collect(&child, &identity(&child)?, None, entries)?;
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
    collect(root, expected, existing_guard, &mut entries)?;
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
            drop(entry.guard.take());
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
            let locator = snapshot.recovery.as_ref().ok_or_else(|| {
                AppError::new(
                    "RECOVERY_INVALID",
                    "This job has no saved recovery workspace.",
                    None,
                )
            })?;
            let settings = snapshot.encode_settings.as_ref().ok_or_else(|| {
                error(
                    Path::new(&locator.workspace),
                    "This is not an encoding job.",
                )
            })?;
            let (_, manifest) = load(&snapshot.id, &snapshot.request, settings, locator)?;
            verify_external_sources(&manifest, settings, None)?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::new("RECOVERY_INVALID", e.to_string(), None))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                version: 1,
                id: "owned-job".into(),
                directory: identity(&root).unwrap(),
                request,
                settings,
                source: digest(&source, None).unwrap(),
                external_sources: vec![],
                original_source: None,
                prepared_identity: None,
                tools: vec![],
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
                completed: BTreeMap::new(),
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

    #[tokio::test]
    async fn discard_requires_the_saved_owner_and_preserves_neighbor_without_source() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let external = fixture.0.join("external.mka");
        fs::write(&external, b"external bytes").unwrap();
        workspace.manifest.settings.external_tracks = vec![media_core::ExternalTrack {
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
    fn donor_only_sources_are_fingerprinted_for_av1an_resume() {
        let fixture = Fixture::new();
        let (mut workspace, intermediate) = fixture.workspace();
        let metadata = fixture.0.join("metadata.mka");
        let chapters = fixture.0.join("chapters.mka");
        fs::write(&metadata, b"metadata donor").unwrap();
        fs::write(&chapters, b"chapter donor").unwrap();
        workspace.manifest.settings.metadata_source_path =
            Some(metadata.to_string_lossy().into_owned());
        workspace.manifest.settings.chapters_source_path =
            Some(chapters.to_string_lossy().into_owned());
        workspace.manifest.external_sources =
            external_source_stamps(&workspace.manifest.settings, None).unwrap();
        assert_eq!(workspace.manifest.external_sources.len(), 2);
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
