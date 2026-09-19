//! Durable whole-phase checkpoints for standalone encoders. Recovery never
//! resumes inside an encoder process: an unfinished phase is rerun from the
//! last artifact whose complete bytes and decoded structure were verified.
use super::{
    check_cancel,
    files::{self, Source, Temporary, WorkspaceLock},
    rate_control::Stats,
};
use media_core::{
    AppError, EncodeSettings, RemuxRequest, StandaloneRecovery, StandaloneRecoveryPhase,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::sync::watch;

const LIMIT: usize = 16 * 1024 * 1024;
static WRITE_ID: AtomicU64 = AtomicU64::new(1);

fn error(path: &Path, message: impl Into<String>) -> AppError {
    files::error("RECOVERY_INVALID", message, path)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Identity(Vec<u64>);

fn file_identity(file: &File) -> Result<Identity, AppError> {
    #[cfg(windows)]
    {
        let (a, b, c) = files::windows_file_id(file)
            .map_err(|cause| AppError::new("RECOVERY_INVALID", cause.to_string(), None))?;
        Ok(Identity(vec![u64::from(a), u64::from(b), u64::from(c)]))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file
            .metadata()
            .map_err(|cause| AppError::new("RECOVERY_INVALID", cause.to_string(), None))?;
        Ok(Identity(vec![metadata.dev(), metadata.ino()]))
    }
}

fn identity(path: &Path) -> Result<Identity, AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|cause| error(path, cause.to_string()))?;
    if metadata.file_type().is_symlink() {
        return Err(error(path, "Recovery paths cannot be links."));
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
            .map_err(|cause| error(path, cause.to_string()))?;
        file_identity(&file).map_err(|cause| error(path, cause.message))
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

fn digest(path: &Path, cancel: Option<&watch::Receiver<bool>>) -> Result<Stamp, AppError> {
    let before = identity(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1 | 2);
    }
    let mut file = options
        .open(path)
        .map_err(|cause| error(path, cause.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|cause| error(path, cause.to_string()))?;
    if !metadata.is_file() {
        return Err(error(path, "Expected a regular recovery file."));
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut length = 0u64;
    loop {
        if let Some(cancel) = cancel {
            check_cancel(cancel)?;
        }
        let count = file
            .read(&mut buffer)
            .map_err(|cause| error(path, cause.to_string()))?;
        if count == 0 {
            break;
        }
        length += count as u64;
        hasher.update(&buffer[..count]);
    }
    let after = file
        .metadata()
        .map_err(|cause| error(path, cause.to_string()))?;
    if before != identity(path)?
        || metadata.len() != length
        || after.len() != length
        || metadata.modified().ok() != after.modified().ok()
    {
        return Err(error(
            path,
            "The artifact changed while its recovery fingerprint was calculated.",
        ));
    }
    Ok(Stamp {
        path: path.to_owned(),
        identity: before,
        length,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn copy_verified(
    source: &Path,
    destination: &Path,
    cancel: Option<&watch::Receiver<bool>>,
) -> Result<Stamp, AppError> {
    let before = digest(source, cancel)?;
    let mut input = File::open(source).map_err(|cause| error(source, cause.to_string()))?;
    if file_identity(&input)? != before.identity {
        return Err(error(
            source,
            "The recovery source was replaced before its durable copy began.",
        ));
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|cause| error(destination, cause.to_string()))?;
    let output_identity = file_identity(&output)?;
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut copied = 0u64;
    loop {
        if let Some(cancel) = cancel {
            check_cancel(cancel)?;
        }
        let count = input
            .read(&mut buffer)
            .map_err(|cause| error(source, cause.to_string()))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|cause| error(destination, cause.to_string()))?;
        copied += count as u64;
    }
    output
        .sync_all()
        .map_err(|cause| error(destination, cause.to_string()))?;
    if copied != before.length
        || file_identity(&input)? != before.identity
        || digest(source, cancel)? != before
    {
        return Err(error(
            source,
            "The recovery source changed while its durable copy was written.",
        ));
    }
    let durable = digest(destination, cancel)?;
    if durable.identity != output_identity
        || durable.length != before.length
        || durable.sha256 != before.sha256
    {
        return Err(error(
            destination,
            "The durable recovery copy differs from its verified source.",
        ));
    }
    Ok(durable)
}

fn bytes(path: &Path) -> Result<Vec<u8>, AppError> {
    identity(path)?;
    let mut value = Vec::new();
    File::open(path)
        .and_then(|file| file.take(LIMIT as u64 + 1).read_to_end(&mut value))
        .map_err(|cause| error(path, cause.to_string()))?;
    if value.len() > LIMIT {
        return Err(error(path, "Recovery manifest exceeds its size limit."));
    }
    Ok(value)
}

fn atomic(path: &Path, value: &[u8]) -> Result<(), AppError> {
    if value.len() > LIMIT {
        return Err(error(path, "Recovery manifest exceeds its size limit."));
    }
    let temporary = path.with_extension(format!(
        "next-{}-{}",
        std::process::id(),
        WRITE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|cause| error(&temporary, cause.to_string()))?;
    file.write_all(value)
        .and_then(|()| file.sync_all())
        .map_err(|cause| error(&temporary, cause.to_string()))?;
    drop(file);
    fs::rename(&temporary, path).map_err(|cause| error(path, cause.to_string()))?;
    #[cfg(unix)]
    File::open(path.parent().expect("owned recovery directory"))
        .and_then(|file| file.sync_all())
        .map_err(|cause| error(path, cause.to_string()))?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    id: String,
    directory: Identity,
    request: RemuxRequest,
    settings: EncodeSettings,
    source: Stamp,
    tools: Vec<Stamp>,
    plan: Vec<String>,
    total_frames: u64,
    two_pass: bool,
    video_extension: String,
    phase: StandaloneRecoveryPhase,
    #[serde(default)]
    stats: Vec<Stamp>,
    #[serde(default)]
    stats_directory: Option<Identity>,
    video: Option<Stamp>,
    timed_video: Option<Stamp>,
    final_stage: Option<Stamp>,
}

struct Seed {
    id: String,
    request: RemuxRequest,
    settings: EncodeSettings,
    source: Stamp,
    tools: Vec<Stamp>,
    plan: Vec<String>,
    total_frames: u64,
    two_pass: bool,
    video_extension: String,
}

pub(super) struct Prepared {
    pub recovery: Recovery,
    pub video: Option<Temporary>,
    pub timed_video: Option<Temporary>,
    pub final_stage: Option<Temporary>,
    pub stats: Option<Stats>,
}

pub(super) struct Recovery {
    root: PathBuf,
    seed: Seed,
    manifest: Option<Manifest>,
    _tool_guards: Vec<Source>,
    directory_guard: Option<File>,
    lock: Option<WorkspaceLock>,
}

fn root_for(id: &str, request: &RemuxRequest) -> Result<PathBuf, AppError> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || value == b'-')
    {
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
        .map_err(|cause| error(parent, cause.to_string()))?
        .join(format!(".jesses-{id}.standalone")))
}

fn directory_guard(root: &Path) -> Result<File, AppError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1 | 2).custom_flags(0x02000000);
    }
    options
        .open(root)
        .map_err(|cause| error(root, cause.to_string()))
}

fn lock_workspace(root: &Path) -> Result<(File, WorkspaceLock), AppError> {
    let directory = directory_guard(root)?;
    let path = root.join("workspace.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let lock = options
        .open(&path)
        .map_err(|cause| error(&path, format!("The recovery workspace is in use: {cause}")))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(error(&path, "The recovery workspace is already in use."));
        }
    }
    Ok((directory, WorkspaceLock(lock)))
}

fn phase_at_least(phase: StandaloneRecoveryPhase, expected: StandaloneRecoveryPhase) -> bool {
    let rank = |value| match value {
        StandaloneRecoveryPhase::PassOneComplete => 0,
        StandaloneRecoveryPhase::VideoComplete => 1,
        StandaloneRecoveryPhase::TimingWrapComplete => 2,
        StandaloneRecoveryPhase::Finalizing => 3,
    };
    rank(phase) >= rank(expected)
}

fn manifest_summary(root: &Path, manifest: &Manifest) -> Option<StandaloneRecovery> {
    let complete = match manifest.phase {
        StandaloneRecoveryPhase::PassOneComplete => !manifest.stats.is_empty(),
        StandaloneRecoveryPhase::VideoComplete => manifest.video.is_some(),
        StandaloneRecoveryPhase::TimingWrapComplete => manifest.timed_video.is_some(),
        StandaloneRecoveryPhase::Finalizing => manifest.final_stage.is_some(),
    };
    complete.then(|| StandaloneRecovery {
        workspace: root.to_string_lossy().into_owned(),
        phase: manifest.phase,
        completed_frames: if matches!(
            manifest.phase,
            StandaloneRecoveryPhase::VideoComplete
                | StandaloneRecoveryPhase::TimingWrapComplete
                | StandaloneRecoveryPhase::Finalizing
        ) {
            manifest.total_frames
        } else {
            0
        },
        total_frames: manifest.total_frames,
    })
}

fn validate_lock_entry(path: &Path) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|cause| error(path, cause.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(
            path,
            "The recovery workspace lock is not a regular file.",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(error(
                path,
                "The recovery workspace lock cannot be a reparse point.",
            ));
        }
    }
    Ok(())
}

fn stats_path(manifest: &Manifest) -> Result<Option<PathBuf>, AppError> {
    if manifest.stats.is_empty() {
        return Ok(None);
    }
    let path = manifest.stats[0]
        .path
        .parent()
        .ok_or_else(|| error(&manifest.stats[0].path, "Invalid pass-statistics path."))?
        .to_owned();
    if manifest
        .stats
        .iter()
        .any(|stamp| stamp.path.parent() != Some(path.as_path()))
    {
        return Err(error(
            &path,
            "Pass-statistics receipts must share one owned directory.",
        ));
    }
    Ok(Some(path))
}

fn validate_layout(root: &Path, manifest: &Manifest) -> Result<(), AppError> {
    let mut known = vec![root.join("manifest.json"), root.join("workspace.lock")];
    if let Some(stats) = stats_path(manifest)? {
        known.push(stats);
    }
    known.extend(manifest.stats.iter().map(|stamp| stamp.path.clone()));
    known.extend(manifest.video.iter().map(|stamp| stamp.path.clone()));
    known.extend(manifest.timed_video.iter().map(|stamp| stamp.path.clone()));
    known.extend(manifest.final_stage.iter().map(|stamp| stamp.path.clone()));
    for entry in fs::read_dir(root).map_err(|cause| error(root, cause.to_string()))? {
        let path = entry
            .map_err(|cause| error(root, cause.to_string()))?
            .path();
        let interrupted_write = path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.starts_with("manifest.next-") || name.starts_with("uncommitted-")
        });
        if !known.contains(&path) && !interrupted_write {
            return Err(error(&path, "Unexpected entry in the recovery workspace."));
        }
        // An active workspace lock intentionally denies every second open on
        // Windows. Its owned handle is the identity guard until cleanup.
        if path == root.join("workspace.lock") {
            validate_lock_entry(&path)?;
        } else {
            identity(&path)?;
        }
    }
    let stats_expected = manifest.two_pass;
    if manifest.stats.is_empty() != !stats_expected
        || manifest.video.is_some()
            != phase_at_least(manifest.phase, StandaloneRecoveryPhase::VideoComplete)
        || manifest.timed_video.is_some()
            != (matches!(
                manifest.settings.encoder,
                media_core::VideoEncoder::X265Standalone | media_core::VideoEncoder::VpxStandalone
            ) && phase_at_least(manifest.phase, StandaloneRecoveryPhase::TimingWrapComplete))
        || manifest.final_stage.is_some()
            != phase_at_least(manifest.phase, StandaloneRecoveryPhase::Finalizing)
    {
        return Err(error(
            root,
            "Recovery phase and artifact receipts disagree.",
        ));
    }
    for stamp in manifest
        .video
        .iter()
        .chain(manifest.timed_video.iter())
        .chain(manifest.final_stage.iter())
    {
        if stamp.path.parent() != Some(root) {
            return Err(error(&stamp.path, "Recovery artifact location changed."));
        }
    }
    if manifest.two_pass {
        let stats = stats_path(manifest)?
            .ok_or_else(|| error(root, "Pass one has no reusable statistics."))?;
        if stats.parent() != Some(root)
            || !stats
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("uncommitted-stats-"))
        {
            return Err(error(&stats, "Invalid pass-statistics directory."));
        }
        if manifest.stats_directory.as_ref() != Some(&identity(&stats)?) {
            return Err(error(&stats, "The pass-statistics directory changed."));
        }
        for stamp in &manifest.stats {
            if stamp.path.parent() != Some(stats.as_path())
                || !stamp
                    .path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("jesses.stats"))
            {
                return Err(error(&stamp.path, "Invalid pass-statistics receipt."));
            }
        }
    }
    Ok(())
}

impl Recovery {
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare(
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        source: &Path,
        tools: Vec<PathBuf>,
        plan: Vec<String>,
        total_frames: usize,
        two_pass: bool,
        video_extension: &str,
        locator: Option<StandaloneRecovery>,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Prepared, AppError> {
        let root = root_for(id, request)?;
        let source_path = source.to_owned();
        let cancel_copy = cancel.clone();
        let (source_stamp, tool_guards, tool_stamps) = tokio::task::spawn_blocking(move || {
            let source_stamp = digest(&source_path, Some(&cancel_copy))?;
            let guards = tools
                .iter()
                .map(|path| Source::open(path))
                .collect::<Result<Vec<_>, _>>()?;
            let stamps = guards
                .iter()
                .map(|guard| digest(&guard.path, Some(&cancel_copy)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, AppError>((source_stamp, guards, stamps))
        })
        .await
        .map_err(|cause| AppError::new("RECOVERY_INVALID", cause.to_string(), None))??;
        let seed = Seed {
            id: id.to_owned(),
            request: request.clone(),
            settings: settings.clone(),
            source: source_stamp,
            tools: tool_stamps,
            plan,
            total_frames: total_frames as u64,
            two_pass,
            video_extension: video_extension.to_owned(),
        };
        if let Some(locator) = locator {
            if Path::new(&locator.workspace) != root {
                return Err(error(
                    &root,
                    "The recovery locator does not belong to this output directory.",
                ));
            }
            let (directory_guard, lock) = lock_workspace(&root)?;
            let manifest: Manifest = serde_json::from_slice(&bytes(&root.join("manifest.json"))?)
                .map_err(|cause| error(&root, cause.to_string()))?;
            let mut changed = Vec::new();
            if manifest.version != 1 {
                changed.push("version");
            }
            if manifest.id != seed.id {
                changed.push("job identifier");
            }
            if manifest.directory != identity(&root)? {
                changed.push("workspace identity");
            }
            if manifest.request != seed.request {
                changed.push("request");
            }
            if manifest.settings != seed.settings {
                changed.push("settings");
            }
            if manifest.source != seed.source {
                changed.push("source content");
            }
            if manifest.tools != seed.tools {
                changed.push("tools");
            }
            if manifest.plan != seed.plan {
                changed.push("processing plan");
            }
            if manifest.total_frames != seed.total_frames {
                changed.push("frame count");
            }
            if manifest.two_pass != seed.two_pass {
                changed.push("pass mode");
            }
            if manifest.video_extension != seed.video_extension {
                changed.push("video format");
            }
            if !phase_at_least(manifest.phase, locator.phase) {
                changed.push("checkpoint phase");
            }
            if locator.total_frames != seed.total_frames {
                changed.push("locator frame count");
            }
            if !changed.is_empty() {
                return Err(error(
                    &root,
                    format!(
                        "Standalone recovery bindings changed: {}.",
                        changed.join(", ")
                    ),
                ));
            }
            validate_layout(&root, &manifest)?;
            let stamps = manifest
                .stats
                .iter()
                .chain(manifest.video.iter())
                .chain(manifest.timed_video.iter())
                .chain(manifest.final_stage.iter());
            for stamp in stamps {
                if digest(&stamp.path, Some(cancel))? != *stamp {
                    return Err(error(
                        &stamp.path,
                        "A verified standalone recovery artifact changed.",
                    ));
                }
            }
            let video = manifest
                .video
                .as_ref()
                .map(|stamp| Temporary::durable(&stamp.path, true))
                .transpose()?;
            let timed_video = manifest
                .timed_video
                .as_ref()
                .map(|stamp| Temporary::durable(&stamp.path, true))
                .transpose()?;
            let final_stage = manifest
                .final_stage
                .as_ref()
                .map(|stamp| Temporary::durable(&stamp.path, true))
                .transpose()?;
            let stats = stats_path(&manifest)?
                .map(|path| Stats::durable(&path, true))
                .transpose()?;
            if let Some(stats) = &stats {
                stats.verify_stats()?;
            }
            return Ok(Prepared {
                recovery: Self {
                    root,
                    seed,
                    manifest: Some(manifest),
                    _tool_guards: tool_guards,
                    directory_guard: Some(directory_guard),
                    lock: Some(lock),
                },
                video,
                timed_video,
                final_stage,
                stats,
            });
        }
        Ok(Prepared {
            recovery: Self {
                root,
                seed,
                manifest: None,
                _tool_guards: tool_guards,
                directory_guard: None,
                lock: None,
            },
            video: None,
            timed_video: None,
            final_stage: None,
            stats: None,
        })
    }

    pub fn phase(&self) -> Option<StandaloneRecoveryPhase> {
        self.manifest.as_ref().map(|manifest| manifest.phase)
    }

    fn ensure_workspace(&mut self, initial: StandaloneRecoveryPhase) -> Result<(), AppError> {
        if self.manifest.is_some() {
            return Ok(());
        }
        fs::create_dir(&self.root).map_err(|cause| error(&self.root, cause.to_string()))?;
        let (directory_guard, lock) = lock_workspace(&self.root)?;
        let manifest = Manifest {
            version: 1,
            id: self.seed.id.clone(),
            directory: identity(&self.root)?,
            request: self.seed.request.clone(),
            settings: self.seed.settings.clone(),
            source: self.seed.source.clone(),
            tools: self.seed.tools.clone(),
            plan: self.seed.plan.clone(),
            total_frames: self.seed.total_frames,
            two_pass: self.seed.two_pass,
            video_extension: self.seed.video_extension.clone(),
            phase: initial,
            stats: Vec::new(),
            stats_directory: None,
            video: None,
            timed_video: None,
            final_stage: None,
        };
        self.directory_guard = Some(directory_guard);
        self.lock = Some(lock);
        self.manifest = Some(manifest);
        Ok(())
    }

    fn save(&self) -> Result<(), AppError> {
        let manifest = self.manifest.as_ref().expect("recovery workspace");
        if identity(&self.root)? != manifest.directory {
            return Err(error(&self.root, "The recovery directory was replaced."));
        }
        atomic(
            &self.root.join("manifest.json"),
            &serde_json::to_vec(manifest).map_err(|cause| error(&self.root, cause.to_string()))?,
        )
    }

    pub fn checkpoint_pass_one(&mut self, stats: &Stats) -> Result<StandaloneRecovery, AppError> {
        stats.verify_stats()?;
        self.ensure_workspace(StandaloneRecoveryPhase::PassOneComplete)?;
        let target = self.root.join(format!(
            "uncommitted-stats-{}-{}",
            std::process::id(),
            WRITE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&target).map_err(|cause| error(&target, cause.to_string()))?;
        let stats_directory = identity(&target)?;
        let mut stamps = Vec::new();
        for entry in
            fs::read_dir(&stats.path).map_err(|cause| error(&stats.path, cause.to_string()))?
        {
            let entry = entry.map_err(|cause| error(&stats.path, cause.to_string()))?;
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("jesses.stats") {
                return Err(error(&entry.path(), "Unexpected pass-statistics artifact."));
            }
            let destination = target.join(name);
            stamps.push(copy_verified(&entry.path(), &destination, None)?);
        }
        if stamps.is_empty() {
            return Err(error(&target, "Pass one produced no reusable statistics."));
        }
        let manifest = self.manifest.as_mut().expect("recovery workspace");
        manifest.stats = stamps;
        manifest.stats_directory = Some(stats_directory);
        manifest.phase = StandaloneRecoveryPhase::PassOneComplete;
        self.save()?;
        Ok(self.summary().expect("checkpoint summary"))
    }

    pub async fn checkpoint_video(
        &mut self,
        source: &Temporary,
        phase: StandaloneRecoveryPhase,
        cancel: &watch::Receiver<bool>,
    ) -> Result<StandaloneRecovery, AppError> {
        self.ensure_workspace(phase)?;
        let (label, extension) = match phase {
            StandaloneRecoveryPhase::VideoComplete => ("video", self.seed.video_extension.as_str()),
            StandaloneRecoveryPhase::TimingWrapComplete => ("timed", "mkv"),
            StandaloneRecoveryPhase::Finalizing => ("final", "mkv"),
            StandaloneRecoveryPhase::PassOneComplete => {
                return Err(error(&self.root, "Invalid video checkpoint phase."));
            }
        };
        let destination = self.root.join(format!(
            "uncommitted-{label}-{}-{}.{}",
            std::process::id(),
            WRITE_ID.fetch_add(1, Ordering::Relaxed),
            extension
        ));
        // The producer's Temporary handle is intentionally writable. Bind the
        // source pathname and exact open handle before copying so a same-size
        // edit or close-and-replace cannot become a committed checkpoint.
        let source_before = digest(&source.path, Some(cancel))?;
        let mut input = source.clone_file()?;
        let input_identity = file_identity(&input)?;
        if input_identity != source_before.identity {
            return Err(error(
                &source.path,
                "The checkpoint source was replaced before its durable copy began.",
            ));
        }
        input
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|cause| error(&source.path, cause.to_string()))?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|cause| error(&destination, cause.to_string()))?;
        let output_identity = file_identity(&output)?;
        let path = destination.clone();
        let source_path = source.path.clone();
        let cancel_copy = cancel.clone();
        let stamp = tokio::task::spawn_blocking(move || {
            let mut copied = 0u64;
            let mut buffer = vec![0u8; 1024 * 1024];
            loop {
                check_cancel(&cancel_copy)?;
                let count = input
                    .read(&mut buffer)
                    .map_err(|cause| error(&path, cause.to_string()))?;
                if count == 0 {
                    break;
                }
                output
                    .write_all(&buffer[..count])
                    .map_err(|cause| error(&path, cause.to_string()))?;
                copied += count as u64;
            }
            output
                .sync_all()
                .map_err(|cause| error(&path, cause.to_string()))?;
            if copied != source_before.length {
                return Err(error(
                    &path,
                    "The checkpoint source changed while its durable copy was written.",
                ));
            }
            if file_identity(&input)? != input_identity
                || digest(&source_path, Some(&cancel_copy))? != source_before
            {
                return Err(error(
                    &source_path,
                    "The checkpoint source changed while its durable copy was written.",
                ));
            }
            let copied = digest(&path, Some(&cancel_copy))?;
            if copied.identity != output_identity
                || copied.length != source_before.length
                || copied.sha256 != source_before.sha256
            {
                return Err(error(
                    &path,
                    "The durable checkpoint differs from its verified source.",
                ));
            }
            Ok(copied)
        })
        .await
        .map_err(|cause| AppError::new("RECOVERY_INVALID", cause.to_string(), None))??;
        let manifest = self.manifest.as_mut().expect("recovery workspace");
        match phase {
            StandaloneRecoveryPhase::VideoComplete => manifest.video = Some(stamp),
            StandaloneRecoveryPhase::TimingWrapComplete => manifest.timed_video = Some(stamp),
            StandaloneRecoveryPhase::Finalizing => manifest.final_stage = Some(stamp),
            StandaloneRecoveryPhase::PassOneComplete => unreachable!(),
        }
        manifest.phase = phase;
        self.save()?;
        Ok(self.summary().expect("checkpoint summary"))
    }

    pub fn summary(&self) -> Option<StandaloneRecovery> {
        self.manifest
            .as_ref()
            .and_then(|manifest| manifest_summary(&self.root, manifest))
    }

    pub fn cleanup(mut self) -> Result<(), AppError> {
        let Some(manifest) = self.manifest.take() else {
            return Ok(());
        };
        if identity(&self.root)? != manifest.directory {
            return Err(error(&self.root, "The recovery directory was replaced."));
        }
        remove_owned_contents(&self.root, &manifest, &mut self.lock)?;
        drop(self.directory_guard.take());
        remove_exact(&self.root, &manifest.directory, true)
    }
}

fn remove_owned_contents(
    root: &Path,
    manifest: &Manifest,
    lock_guard: &mut Option<WorkspaceLock>,
) -> Result<(), AppError> {
    use std::collections::BTreeMap;
    let manifest_path = root.join("manifest.json");
    let lock_path = root.join("workspace.lock");
    let lock_identity = file_identity(
        &lock_guard
            .as_ref()
            .ok_or_else(|| error(&lock_path, "Missing recovery workspace lock guard."))?
            .0,
    )?;
    let mut expected_files = BTreeMap::from([
        (manifest_path.clone(), identity(&manifest_path)?),
        (lock_path.clone(), lock_identity),
    ]);
    for stamp in manifest
        .stats
        .iter()
        .chain(manifest.video.iter())
        .chain(manifest.timed_video.iter())
        .chain(manifest.final_stage.iter())
    {
        if digest(&stamp.path, None)? != *stamp {
            return Err(error(
                &stamp.path,
                "A recovery artifact changed before cleanup; the workspace was preserved.",
            ));
        }
        expected_files.insert(stamp.path.clone(), stamp.identity.clone());
    }
    let mut expected_directories = BTreeMap::new();
    if let Some(identity) = &manifest.stats_directory {
        let stats = stats_path(manifest)?
            .ok_or_else(|| error(root, "Missing pass-statistics receipts."))?;
        expected_directories.insert(stats, identity.clone());
    }
    let mut actual_files = BTreeMap::new();
    let mut actual_directories = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in
            fs::read_dir(&directory).map_err(|cause| error(&directory, cause.to_string()))?
        {
            let path = entry
                .map_err(|cause| error(&directory, cause.to_string()))?
                .path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|cause| error(&path, cause.to_string()))?;
            if metadata.file_type().is_symlink() {
                return Err(error(
                    &path,
                    "Recovery cleanup found a link; the workspace was preserved.",
                ));
            }
            let found = if path == lock_path {
                validate_lock_entry(&path)?;
                file_identity(
                    &lock_guard
                        .as_ref()
                        .ok_or_else(|| error(&lock_path, "Missing recovery workspace lock guard."))?
                        .0,
                )?
            } else {
                identity(&path)?
            };
            if metadata.is_dir() {
                pending.push(path.clone());
                actual_directories.insert(path, found);
            } else if metadata.is_file() {
                actual_files.insert(path, found);
            } else {
                return Err(error(
                    &path,
                    "Recovery cleanup found an unexpected filesystem entry; the workspace was preserved.",
                ));
            }
        }
    }
    if actual_files != expected_files || actual_directories != expected_directories {
        return Err(error(
            root,
            "Recovery workspace contents or identities changed; cleanup preserved every entry for review.",
        ));
    }
    // Preserve the receipt and exclusive lock until every data artifact is
    // gone. A failed artifact removal therefore leaves an inspectable,
    // mutually-exclusive workspace rather than a manifest-less partial tree.
    for (path, expected) in expected_files
        .iter()
        .filter(|(path, _)| **path != manifest_path && **path != lock_path)
    {
        remove_exact(path, expected, false)?;
    }
    let mut directories = expected_directories.into_iter().collect::<Vec<_>>();
    directories.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (path, expected) in directories {
        remove_exact(&path, &expected, true)?;
    }
    remove_exact(
        &manifest_path,
        expected_files
            .get(&manifest_path)
            .expect("manifest inventory"),
        false,
    )?;
    drop(lock_guard.take());
    remove_exact(
        &lock_path,
        expected_files.get(&lock_path).expect("lock inventory"),
        false,
    )?;
    Ok(())
}

fn remove_exact(path: &Path, expected: &Identity, _directory: bool) -> Result<(), AppError> {
    if identity(path)? != *expected {
        return Err(error(
            path,
            "Recovery entry changed immediately before cleanup; it was preserved.",
        ));
    }
    #[cfg(windows)]
    {
        let [volume, high, low] = expected.0.as_slice() else {
            return Err(error(path, "Invalid Windows recovery identity."));
        };
        files::windows_delete_owned(path, (*volume as u32, *high as u32, *low as u32))
            .map_err(|cause| error(path, cause.to_string()))
    }
    #[cfg(not(windows))]
    {
        let result = if _directory {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        };
        result.map_err(|cause| error(path, cause.to_string()))
    }
}

pub(super) fn discover_locator(
    id: &str,
    request: &RemuxRequest,
    settings: &EncodeSettings,
    locator: Option<&StandaloneRecovery>,
) -> Result<Option<StandaloneRecovery>, AppError> {
    let root = root_for(id, request)?;
    if let Some(locator) = locator
        && Path::new(&locator.workspace) != root
    {
        return Err(error(
            &root,
            "Recovery locator is outside its job directory.",
        ));
    }
    if !root.exists() {
        return if locator.is_some() {
            Err(error(
                &root,
                "The standalone recovery workspace is missing.",
            ))
        } else {
            Ok(None)
        };
    }
    // Acquiring the exact job-bound lock proves that this is an inactive
    // workspace before exposing Resume. The guard remains alive through every
    // structural and identity check, then is released without modifying data.
    let (_directory_guard, _lock) = lock_workspace(&root)?;
    let manifest: Manifest = serde_json::from_slice(&bytes(&root.join("manifest.json"))?)
        .map_err(|cause| error(&root, cause.to_string()))?;
    if manifest.version != 1
        || manifest.id != id
        || manifest.directory != identity(&root)?
        || manifest.request != *request
        || manifest.settings != *settings
        || locator.is_some_and(|locator| {
            locator.total_frames != manifest.total_frames
                || !phase_at_least(manifest.phase, locator.phase)
        })
    {
        return Err(error(
            &root,
            "Recovery locator and durable manifest disagree.",
        ));
    }
    validate_layout(&root, &manifest)?;
    manifest_summary(&root, &manifest).map(Some).ok_or_else(|| {
        error(
            &root,
            "The recovery manifest has no complete reusable phase.",
        )
    })
}

pub(super) fn validate_locator(
    id: &str,
    request: &RemuxRequest,
    settings: &EncodeSettings,
    locator: &StandaloneRecovery,
) -> Result<(), AppError> {
    discover_locator(id, request, settings, Some(locator)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        directory: PathBuf,
        output: PathBuf,
        source: PathBuf,
        tool: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "jesses-standalone-recovery-{}-{}",
                std::process::id(),
                WRITE_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            let output = directory.join("output.mkv");
            let source = directory.join("source.mkv");
            let tool = directory.join("encoder.exe");
            fs::write(&source, b"immutable source").unwrap();
            fs::write(&tool, b"immutable tool").unwrap();
            Self {
                directory,
                output,
                source,
                tool,
            }
        }

        fn request(&self) -> RemuxRequest {
            RemuxRequest {
                input_path: self.source.to_string_lossy().into_owned(),
                output_path: self.output.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            }
        }

        fn recovery(&self, two_pass: bool, extension: &str) -> Recovery {
            let request = self.request();
            let root = root_for("job-1", &request).unwrap();
            let tool_guard = Source::open(&self.tool).unwrap();
            let tool_stamp = digest(&tool_guard.path, None).unwrap();
            Recovery {
                root,
                seed: Seed {
                    id: "job-1".into(),
                    request,
                    settings: EncodeSettings::default(),
                    source: digest(&self.source, None).unwrap(),
                    tools: vec![tool_stamp],
                    plan: vec!["verified plan".into()],
                    total_frames: 48,
                    two_pass,
                    video_extension: extension.into(),
                },
                manifest: None,
                _tool_guards: vec![tool_guard],
                directory_guard: None,
                lock: None,
            }
        }

        fn video(&self, name: &str) -> Temporary {
            let temporary = Temporary::create(&self.output, name).unwrap();
            let mut file = temporary.clone_file().unwrap();
            file.write_all(b"complete encoded artifact").unwrap();
            file.sync_all().unwrap();
            temporary
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // The nonce-qualified directory and every file in it belong to this test.
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn phase_order_is_whole_stage_only() {
        assert!(phase_at_least(
            StandaloneRecoveryPhase::Finalizing,
            StandaloneRecoveryPhase::PassOneComplete
        ));
        assert!(!phase_at_least(
            StandaloneRecoveryPhase::PassOneComplete,
            StandaloneRecoveryPhase::VideoComplete
        ));
    }

    #[test]
    fn pass_one_and_non_x265_finalizing_claim_only_complete_boundaries() {
        let fixture = Fixture::new();
        let mut recovery = fixture.recovery(true, "ivf");
        assert!(recovery.summary().is_none());
        let stats = Stats::create(&fixture.output, "test-stats").unwrap();
        fs::write(stats.path.join("jesses.stats"), b"complete pass one").unwrap();
        let pass = recovery.checkpoint_pass_one(&stats).unwrap();
        assert_eq!(pass.phase, StandaloneRecoveryPhase::PassOneComplete);
        assert_eq!(pass.completed_frames, 0);
        let video = fixture.video("video");
        let (_owner, cancel) = watch::channel(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let video_summary = runtime
            .block_on(recovery.checkpoint_video(
                &video,
                StandaloneRecoveryPhase::VideoComplete,
                &cancel,
            ))
            .unwrap();
        assert_eq!(video_summary.completed_frames, 48);
        let final_stage = fixture.video("final");
        let final_summary = runtime
            .block_on(recovery.checkpoint_video(
                &final_stage,
                StandaloneRecoveryPhase::Finalizing,
                &cancel,
            ))
            .unwrap();
        assert_eq!(final_summary.phase, StandaloneRecoveryPhase::Finalizing);
        validate_layout(&recovery.root, recovery.manifest.as_ref().unwrap()).unwrap();
        recovery.cleanup().unwrap();
    }

    #[tokio::test]
    async fn canceled_checkpoint_before_copy_does_not_poison_the_next_checkpoint() {
        let fixture = Fixture::new();
        let mut recovery = fixture.recovery(false, "ivf");
        let video = fixture.video("video");
        let (_, canceled) = watch::channel(true);
        assert!(
            recovery
                .checkpoint_video(&video, StandaloneRecoveryPhase::VideoComplete, &canceled,)
                .await
                .is_err()
        );
        assert!(recovery.summary().is_none());
        let (_, active) = watch::channel(false);
        recovery
            .checkpoint_video(&video, StandaloneRecoveryPhase::VideoComplete, &active)
            .await
            .unwrap();
        validate_layout(&recovery.root, recovery.manifest.as_ref().unwrap()).unwrap();
        assert_eq!(
            fs::read_dir(&recovery.root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("uncommitted-video-"))
                .count(),
            1
        );
        let root = recovery.root.clone();
        recovery.cleanup().unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn interrupted_pass_one_directory_does_not_block_a_fresh_checkpoint() {
        let fixture = Fixture::new();
        let mut recovery = fixture.recovery(true, "ivf");
        recovery
            .ensure_workspace(StandaloneRecoveryPhase::PassOneComplete)
            .unwrap();
        let orphan = recovery.root.join("uncommitted-stats-crashed-attempt");
        fs::create_dir(&orphan).unwrap();
        fs::write(orphan.join("jesses.stats"), b"incomplete pass one").unwrap();

        let stats = Stats::create(&fixture.output, "fresh-stats").unwrap();
        fs::write(stats.path.join("jesses.stats"), b"complete pass one").unwrap();
        let summary = recovery.checkpoint_pass_one(&stats).unwrap();
        assert_eq!(summary.phase, StandaloneRecoveryPhase::PassOneComplete);
        let manifest = recovery.manifest.as_ref().unwrap();
        let committed = stats_path(manifest).unwrap().unwrap();
        assert_ne!(committed, orphan);
        assert_eq!(
            manifest.stats_directory.as_ref(),
            Some(&identity(&committed).unwrap())
        );
        validate_layout(&recovery.root, manifest).unwrap();

        let before = fs::read_dir(&recovery.root).unwrap().count();
        assert!(recovery.cleanup().is_err());
        assert_eq!(
            fs::read_dir(root_for("job-1", &fixture.request()).unwrap())
                .unwrap()
                .count(),
            before
        );
    }

    #[tokio::test]
    async fn cleanup_refuses_foreign_and_modified_entries_before_removing_anything() {
        let fixture = Fixture::new();
        let mut recovery = fixture.recovery(false, "mkv");
        let video = fixture.video("video");
        let (_, cancel) = watch::channel(false);
        recovery
            .checkpoint_video(&video, StandaloneRecoveryPhase::VideoComplete, &cancel)
            .await
            .unwrap();
        let foreign = recovery.root.join("foreign.txt");
        fs::write(&foreign, b"not owned by the manifest").unwrap();
        let manifest = recovery.manifest.as_ref().unwrap().clone();
        assert!(remove_owned_contents(&recovery.root, &manifest, &mut recovery.lock).is_err());
        assert!(foreign.is_file());
        assert!(recovery.root.join("manifest.json").is_file());
        fs::remove_file(&foreign).unwrap();
        let artifact = manifest.video.as_ref().unwrap().path.clone();
        fs::write(&artifact, b"modified after checkpoint").unwrap();
        assert!(remove_owned_contents(&recovery.root, &manifest, &mut recovery.lock).is_err());
        assert!(artifact.is_file());
        assert!(recovery.root.join("manifest.json").is_file());
    }

    #[tokio::test]
    async fn vpx_timing_checkpoint_is_discovered_and_adopted_past_a_stale_locator() {
        let fixture = Fixture::new();
        let mut recovery = fixture.recovery(false, "ivf");
        recovery.seed.settings.encoder = media_core::VideoEncoder::VpxStandalone;
        let request = recovery.seed.request.clone();
        let settings = recovery.seed.settings.clone();
        let plan = recovery.seed.plan.clone();
        let (_, cancel) = watch::channel(false);

        let video = fixture.video("video");
        let stale = recovery
            .checkpoint_video(&video, StandaloneRecoveryPhase::VideoComplete, &cancel)
            .await
            .unwrap();
        let timed = fixture.video("timed");
        let latest = recovery
            .checkpoint_video(&timed, StandaloneRecoveryPhase::TimingWrapComplete, &cancel)
            .await
            .unwrap();
        assert_eq!(latest.phase, StandaloneRecoveryPhase::TimingWrapComplete);
        // dup and fork share an open file description. Keep that description
        // alive to deterministically exercise discovery during a child spawn.
        #[cfg(unix)]
        let inherited = recovery.lock.as_ref().unwrap().0.try_clone().unwrap();
        assert!(lock_workspace(&recovery.root).is_err());
        drop(recovery);

        let discovered = discover_locator("job-1", &request, &settings, None)
            .unwrap()
            .unwrap();
        assert_eq!(discovered, latest);
        assert_eq!(
            discover_locator("job-1", &request, &settings, Some(&stale))
                .unwrap()
                .unwrap(),
            latest
        );

        let prepared = Recovery::prepare(
            "job-1",
            &request,
            &settings,
            &fixture.source,
            vec![fixture.tool.clone()],
            plan,
            48,
            false,
            "ivf",
            Some(stale),
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(
            prepared.recovery.phase(),
            Some(StandaloneRecoveryPhase::TimingWrapComplete)
        );
        #[cfg(unix)]
        drop(inherited);
        assert!(lock_workspace(&prepared.recovery.root).is_err());
        drop(prepared.video);
        drop(prepared.timed_video);
        drop(prepared.final_stage);
        drop(prepared.stats);
        prepared.recovery.cleanup().unwrap();
    }
}
