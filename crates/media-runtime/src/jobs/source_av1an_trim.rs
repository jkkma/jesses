//! Durable source AV1AN stream-copy input. The fork seeks the requested point
//! and preserves FFmpeg's keyframe preroll; it does not pre-snap/recompute -t.
use super::*;
use serde::{Deserialize, Serialize};
#[path = "source_cut_packets.rs"]
mod packets;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Section {
    start_ms: u64,
    end_ms: u64,
    extension: String,
}
#[derive(Debug, Serialize, Deserialize)]
struct CutOwner {
    version: u8,
    id: String,
    request: RemuxRequest,
    settings: EncodeSettings,
    directory: Identity,
    files: BTreeMap<String, Identity>,
    original: Stamp,
    tool: Stamp,
    section: Section,
    arguments: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize)]
struct CutComplete {
    cut: Stamp,
    tracks: Vec<packets::TrackProof>,
    video_origin: f64,
}
struct CutWorkspace {
    root: PathBuf,
    owner: CutOwner,
    lock: Option<files::WorkspaceLock>,
    guard: Option<rate_control::Stats>,
}
fn cut_root(
    id: &str,
    request: &RemuxRequest,
    settings: &EncodeSettings,
) -> Result<PathBuf, AppError> {
    let original = root_for(id, request)?;
    let key = serde_json::to_vec(&(settings.trim, settings.video_stream_index))
        .map_err(|error| invalid(&original, error.to_string()))?;
    let key = format!("{:x}", Sha256::digest(key));
    Ok(original
        .parent()
        .expect("source parent")
        .join(format!(".jesses-{id}.source-cut-{key}")))
}
fn cut_roots(id: &str, request: &RemuxRequest) -> Result<Vec<PathBuf>, AppError> {
    let original = root_for(id, request)?;
    let parent = original.parent().expect("source parent");
    let prefix = format!(".jesses-{id}.source-cut-");
    let mut roots = Vec::new();
    for entry in fs::read_dir(parent).map_err(|error| invalid(parent, error.to_string()))? {
        let path = entry
            .map_err(|error| invalid(parent, error.to_string()))?
            .path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(key) = name.strip_prefix(&prefix) else {
            continue;
        };
        if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid(
                &path,
                "Foreign source-cut workspace name; it was preserved.",
            ));
        }
        identity(&path)?;
        roots.push(path);
    }
    roots.sort();
    Ok(roots)
}
fn cut_name(section: &Section) -> String {
    format!("cut.{}", section.extension)
}
fn section(
    settings: &EncodeSettings,
    document: &Document,
    input: &Path,
) -> Result<Section, AppError> {
    let trim = settings
        .trim
        .ok_or_else(|| invalid(input, "The source AV1AN cut requires a trim."))?;
    let video = document
        .streams
        .iter()
        .find(|stream| {
            stream.index == settings.video_stream_index
                && stream.codec_type.as_deref() == Some("video")
        })
        .ok_or_else(|| invalid(input, "The selected source video no longer exists."))?;
    let frame = video
        .avg_frame_rate
        .as_deref()
        .or(video.r_frame_rate.as_deref())
        .and_then(|rate| rate.split_once('/'))
        .and_then(|(n, d)| Some(d.parse::<f64>().ok()? / n.parse::<f64>().ok()?))
        .filter(|value| value.is_finite() && *value > 0.0);
    let (start_ms, mut end_ms) = if let Some(time) = trim.time {
        (
            u64::from(time.start_milliseconds),
            u64::from(time.end_milliseconds),
        )
    } else {
        let frame = frame
            .ok_or_else(|| invalid(input, "Frame-based source trimming needs a source rate."))?;
        (
            (f64::from(trim.start_frame) * frame * 1000.0).round_ties_even() as u64,
            (f64::from(trim.end_frame_exclusive) * frame * 1000.0).round_ties_even() as u64,
        )
    };
    if let Some(duration) = document
        .duration()
        .filter(|duration| duration.is_finite() && *duration > 0.0)
    {
        end_ms = end_ms.min((duration * 1000.0).round_ties_even() as u64);
    }
    if end_ms <= start_ms {
        return Err(invalid(
            input,
            "The source ends at or before the configured AV1AN trim start.",
        ));
    }
    let extension = input
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 24
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .ok_or_else(|| {
            invalid(
                input,
                "The source-copy intermediate needs the original media filename extension.",
            )
        })?
        .to_owned();
    Ok(Section {
        start_ms,
        end_ms,
        extension,
    })
}
fn arguments(input: &Path, output: &Path, section: &Section) -> Vec<OsString> {
    [
        "-hide_banner",
        "-nostdin",
        "-v",
        "warning",
        "-nostats",
        "-progress",
        "pipe:1",
        "-ss",
    ]
    .into_iter()
    .map(Into::into)
    .chain([
        format!("{}.{:03}", section.start_ms / 1000, section.start_ms % 1000).into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-i".into(),
        input.as_os_str().to_owned(),
        "-t".into(),
        format!(
            "{}.{:03}",
            (section.end_ms - section.start_ms) / 1000,
            (section.end_ms - section.start_ms) % 1000
        )
        .into(),
        "-map".into(),
        "0".into(),
        "-c".into(),
        "copy".into(),
        "-avoid_negative_ts".into(),
        "make_zero".into(),
        "-ignore_unknown".into(),
        "-y".into(),
        output.as_os_str().to_owned(),
    ])
    .collect()
}

fn copy_diagnostic(event: ProcessEvent) -> Option<String> {
    match event {
        ProcessEvent::Stdout(line) if supervisor::is_ffmpeg_progress_line(&line) => None,
        ProcessEvent::Stdout(line) | ProcessEvent::Stderr(line) => Some(line),
    }
}

fn publish_source(temporary: &mut Temporary, output: &Path) -> Result<Source, AppError> {
    let expected = file_identity(&temporary.clone_file()?)?;
    temporary.publish(output)?;
    // A Windows immutable Source cannot coexist with our writable reservation.
    // Release only that completed temporary link, then bind the published name
    // to the same file identity before accepting its read-only source guard.
    temporary.cleanup()?;
    let source = Source::open(output)?;
    if identity(&source.path)? != expected {
        return Err(invalid(
            output,
            "The published source-cut file was replaced.",
        ));
    }
    source.verify()?;
    Ok(source)
}
impl CutWorkspace {
    fn open(id: &str, request: &RemuxRequest, settings: &EncodeSettings) -> Result<Self, AppError> {
        Self::open_path(cut_root(id, request, settings)?, id, request)
    }
    fn open_path(root: PathBuf, id: &str, request: &RemuxRequest) -> Result<Self, AppError> {
        let guard = rate_control::Stats::durable(&root, true)?;
        let lock = lock_file(&root.join("workspace.lock"), true)?;
        let owner: CutOwner = read_json(&root.join("owner.json"))?;
        if owner.version != 1
            || owner.id != id
            || owner.request != *request
            || owner.directory != identity(&root)?
            || root != cut_root(id, request, &owner.settings)?
            || !owner.settings.source_trim
            || owner.settings.backend != media_core::EncodeBackend::Av1an
            || owner.settings.trim.is_none()
        {
            return Err(invalid(
                &root,
                "Source-cut identity, request or immutable trim recipe changed.",
            ));
        }
        let expected = [
            "owner.json".to_owned(),
            "workspace.lock".to_owned(),
            "complete.json".to_owned(),
            cut_name(&owner.section),
            format!(
                ".jesses-{id}-source-cut.partial.{}",
                owner.section.extension
            ),
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
        if owner
            .files
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            != expected
        {
            return Err(invalid(
                &root,
                "The source-cut receipt has an invalid owned layout.",
            ));
        }
        let result = Self {
            root,
            owner,
            lock: Some(lock),
            guard: Some(guard),
        };
        result.verify()?;
        Ok(result)
    }
    fn verify(&self) -> Result<(), AppError> {
        self.guard.as_ref().expect("cut directory guard").check()?;
        if identity(&self.root)? != self.owner.directory {
            return Err(invalid(
                &self.root,
                "The source-cut directory was replaced.",
            ));
        }
        for entry in
            fs::read_dir(&self.root).map_err(|error| invalid(&self.root, error.to_string()))?
        {
            let path = entry
                .map_err(|error| invalid(&self.root, error.to_string()))?
                .path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| invalid(&path, "Unknown source-cut filename."))?;
            let expected = self.owner.files.get(name).ok_or_else(|| {
                invalid(
                    &path,
                    "Foreign source-cut file; the workspace was preserved.",
                )
            })?;
            if identity(&path)? != *expected
                || !fs::symlink_metadata(&path)
                    .map_err(|error| invalid(&path, error.to_string()))?
                    .is_file()
            {
                return Err(invalid(&path, "A source-cut file was replaced."));
            }
        }
        for name in ["owner.json", "complete.json", "workspace.lock"] {
            if self.owner.files.get(name).is_none_or(|expected| {
                identity(&self.root.join(name)).ok().as_ref() != Some(expected)
            }) {
                return Err(invalid(
                    &self.root,
                    "A source-cut ownership receipt is missing or changed.",
                ));
            }
        }
        Ok(())
    }
    fn cleanup(mut self) -> Result<(), AppError> {
        self.verify()?;
        for (name, expected) in &self.owner.files {
            if matches!(name.as_str(), "owner.json" | "workspace.lock") {
                continue;
            }
            let path = self.root.join(name);
            if path.symlink_metadata().is_ok() {
                remove_owned(&path, expected)?;
            }
        }
        remove_owned(
            &self.root.join("owner.json"),
            &self.owner.files["owner.json"],
        )?;
        drop(self.lock.take());
        remove_owned(
            &self.root.join("workspace.lock"),
            &self.owner.files["workspace.lock"],
        )?;
        if identity(&self.root)? != self.owner.directory {
            return Err(invalid(
                &self.root,
                "The source-cut directory changed before cleanup.",
            ));
        }
        drop(self.guard.take());
        fs::remove_dir(&self.root).map_err(|error| invalid(&self.root, error.to_string()))
    }
}
pub(in crate::jobs) struct Prepared {
    pub original: Source,
    pub source_path: PathBuf,
    pub request: RemuxRequest,
    pub settings: EncodeSettings,
    pub video_origin: f64,
    workspace: CutWorkspace,
}
impl Prepared {
    pub(in crate::jobs) async fn prepare(
        manager: &JobManager,
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log: &Path,
    ) -> Result<Self, AppError> {
        let original = Source::open(Path::new(&request.input_path))?;
        let root = cut_root(id, request, settings)?;
        let ffprobe = discover("ffprobe", cancel).await?;
        let ffmpeg = discover("ffmpeg", cancel).await?;
        let document = probe(&ffprobe, &original.path, cancel, None).await?;
        let section = section(settings, &document, &original.path)?;
        let original_stamp = stamp(&original, cancel).await?;
        // Other trim variants remain immutable for canceled New-settings review.
        // Their original-source authority must agree before any replacement is made.
        for prior_root in cut_roots(id, request)? {
            if prior_root == root {
                continue;
            }
            let prior = CutWorkspace::open_path(prior_root, id, request)?;
            if prior.owner.original != original_stamp {
                return Err(invalid(
                    &prior.root,
                    "The original source changed since a saved source-cut variant; cached files were preserved.",
                ));
            }
            prior.verify()?;
        }
        if root.symlink_metadata().is_ok() {
            let workspace = CutWorkspace::open(id, request, settings)?;
            if workspace.owner.original != original_stamp || workspace.owner.section != section {
                return Err(invalid(
                    &root,
                    "The original source or source-cut section changed; cached chunks were preserved.",
                ));
            }
            let complete: CutComplete = read_json(&root.join("complete.json"))?;
            let source_path = root.join(cut_name(&section));
            let source = Source::open(&source_path)?;
            if complete.cut.path != source_path
                || workspace.owner.files.get(&cut_name(&section)) != Some(&complete.cut.identity)
                || stamp(&source, cancel).await? != complete.cut
            {
                return Err(invalid(
                    &source_path,
                    "The verified source-copy intermediate changed.",
                ));
            }
            drop(source);
            let (request, settings) = effective(request, settings, &complete.tracks)?;
            manager.change(id,|snapshot|append_log(snapshot,"Reusing the verified source AV1AN copied input; complete original/cut hashes and frozen section were rechecked.".into())).await;
            return Ok(Self {
                original,
                source_path,
                request,
                settings,
                video_origin: complete.video_origin,
                workspace,
            });
        }
        manager.phase_with_fresh_progress(id,JobState::Preparing,"Stream-copying the source AV1AN section with the fork's original keyframe/preroll semantics.").await;
        let guard = rate_control::Stats::durable(&root, false)?;
        let directory = identity(&root)?;
        let mut creating = Creating {
            root: root.clone(),
            directory: directory.clone(),
            files: BTreeMap::new(),
            guard: Some(guard),
            active: true,
        };
        let lock = lock_file(&root.join("workspace.lock"), false)?;
        creating
            .files
            .insert("workspace.lock".into(), file_identity(&lock.0)?);
        let mut complete_file = create_file(&root.join("complete.json"))?;
        creating
            .files
            .insert("complete.json".into(), file_identity(&complete_file)?);
        let source_path = root.join(cut_name(&section));
        let mut temporary = Temporary::create_extension(
            &source_path,
            &format!("{id}-source-cut"),
            &section.extension,
        )?;
        let temporary_id = file_identity(&temporary.clone_file()?)?;
        creating.files.insert(
            temporary
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            temporary_id.clone(),
        );
        creating.files.insert(cut_name(&section), temporary_id);
        let mut owner_file = create_file(&root.join("owner.json"))?;
        creating
            .files
            .insert("owner.json".into(), file_identity(&owner_file)?);
        let tool = stamp(&Source::open(&ffmpeg)?, cancel).await?;
        let args = arguments(&original.path, &temporary.path, &section);
        let owner = CutOwner {
            version: 1,
            id: id.into(),
            request: request.clone(),
            settings: settings.clone(),
            directory,
            files: creating.files.clone(),
            original: original_stamp,
            tool,
            section: section.clone(),
            arguments: args
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
        };
        owner_file
            .write_all(
                &serde_json::to_vec(&owner).map_err(|error| invalid(&root, error.to_string()))?,
            )
            .and_then(|_| owner_file.sync_all())
            .map_err(|error| invalid(&root, error.to_string()))?;
        drop(owner_file);
        let workspace = CutWorkspace {
            root: root.clone(),
            owner,
            lock: Some(lock),
            guard: creating.guard.take(),
        };
        creating.active = false;
        let result = async {
            temporary.close_for_path_writer()?;
            let copy_log = log.with_extension("source-cut.log");
            manager
                .change(id, |snapshot| {
                    snapshot.log_path = Some(copy_log.to_string_lossy().into_owned());
                    snapshot.duration_seconds =
                        Some((section.end_ms - section.start_ms) as f64 / 1000.0);
                })
                .await;
            // The cut's copy clock measures only this subprocess. Keep its
            // progress protocol in the dedicated tool log and report the
            // preparation steps independently of the upcoming encode.
            let (sender, mut events) = tokio::sync::mpsc::channel(256);
            let observer_manager = manager.clone();
            let job_id = id.to_owned();
            let observer = tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    if let Some(line) = copy_diagnostic(event) {
                        observer_manager
                            .change(&job_id, |snapshot| append_log(snapshot, line))
                            .await;
                    }
                }
            });
            let run = supervisor::run(
                &CommandSpec {
                    executable: ffmpeg.clone(),
                    args,
                    cwd: None,
                },
                cancel.clone(),
                sender,
                &copy_log,
                TIME,
            )
            .await;
            let _ = observer.await;
            let rebound = temporary.reopen_after_path_writer();
            let run = run.map_err(|error| process_error(error, &original.path))?;
            rebound?;
            if !run.status.success() {
                return Err(invalid(
                    &original.path,
                    format!(
                        "The source AV1AN copied-input command failed; inspect {}.",
                        copy_log.display()
                    ),
                ));
            }
            manager
                .phase_with_fresh_progress(
                    id,
                    JobState::Preparing,
                    "Verifying the stream-copied AV1AN section.",
                )
                .await;
            temporary.flush_nonempty_async().await?;
            let actual = probe(&ffprobe, &temporary.path, cancel, None).await?;
            let proof = packets::verify(
                &ffmpeg,
                &ffprobe,
                &original.path,
                &document,
                &temporary.path,
                &actual,
                settings.video_stream_index,
                section.start_ms as f64 / 1000.0,
                section.end_ms as f64 / 1000.0,
                cancel,
            )
            .await?;
            original.verify()?;
            workspace.verify()?;
            check_cancel(cancel)?;
            let cut = stamp(&publish_source(&mut temporary, &source_path)?, cancel).await?;
            let complete = CutComplete {
                cut,
                tracks: proof.0,
                video_origin: proof.1,
            };
            complete_file
                .write_all(
                    &serde_json::to_vec(&complete)
                        .map_err(|error| invalid(&root, error.to_string()))?,
                )
                .and_then(|_| complete_file.sync_all())
                .map_err(|error| invalid(&root, error.to_string()))?;
            Ok::<_, AppError>(complete)
        }
        .await;
        drop(complete_file);
        drop(temporary);
        match result {
            Err(error) => {
                let cleanup = workspace.cleanup();
                match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(AppError::new(
                        error.code,
                        format!("{} Cleanup: {}", error.message, cleanup.message),
                        cleanup.path,
                    )),
                }
            }
            Ok(complete) => {
                let (effective_request, effective_settings) =
                    effective(request, settings, &complete.tracks)?;
                manager.change(id,|snapshot|append_log(snapshot,format!("Verified source AV1AN cut: {} original streams, copied bytes SHA256 {}, video origin {:.6}s. Requested {}..{}ms; keyframe preroll remains in the intermediate.",complete.tracks.len(),complete.cut.sha256,complete.video_origin,section.start_ms,section.end_ms))).await;
                Ok(Self {
                    original,
                    source_path,
                    request: effective_request,
                    settings: effective_settings,
                    video_origin: complete.video_origin,
                    workspace,
                })
            }
        }
    }
    pub(in crate::jobs) fn finish(self, keep: bool) -> Result<(), AppError> {
        self.original.verify()?;
        if keep {
            self.workspace.verify()
        } else {
            let id = self.workspace.owner.id.clone();
            let request = self.workspace.owner.request.clone();
            self.workspace.cleanup()?;
            discard(&id, &request, &self.settings)
        }
    }
}
fn effective(
    request: &RemuxRequest,
    settings: &EncodeSettings,
    tracks: &[packets::TrackProof],
) -> Result<(RemuxRequest, EncodeSettings), AppError> {
    let remap = |index: u32| {
        tracks
            .iter()
            .find(|track| track.source_index == index)
            .map(|track| track.cut_index)
            .ok_or_else(|| {
                invalid(
                    Path::new(&request.input_path),
                    "A selected source stream was not copied into the AV1AN input.",
                )
            })
    };
    let mut request = request.clone();
    request.stream_indices = request
        .stream_indices
        .iter()
        .map(|index| remap(*index))
        .collect::<Result<_, _>>()?;
    let mut settings = settings.clone();
    settings.trim = None;
    settings.source_trim = false;
    settings.video_stream_index = remap(settings.video_stream_index)?;
    for track in &mut settings.audio {
        track.stream_index = remap(track.stream_index)?;
        if let Some(gain) = &mut track.gain {
            gain.source_fingerprint = None;
        }
    }
    for track in &mut settings.subtitles {
        track.stream_index = remap(track.stream_index)?;
    }
    for track in &mut settings.track_overrides {
        track.stream_index = remap(track.stream_index)?;
    }
    for track in &mut settings.track_order {
        if track.input_path.is_none() {
            track.stream_index = remap(track.stream_index)?;
        }
    }
    Ok((request, settings))
}
pub(in crate::jobs) struct Cleanup(Vec<CutWorkspace>);
impl Cleanup {
    pub(in crate::jobs) fn verify_source(&self) -> Result<(), AppError> {
        let (_owner, cancel) = watch::channel(false);
        for workspace in &self.0 {
            let original = Source::open(&workspace.owner.original.path)?;
            if digest(&original, &cancel)? != workspace.owner.original {
                return Err(invalid(
                    &original.path,
                    "The original source changed since copied input preparation.",
                ));
            }
            workspace.verify()?;
            let complete: CutComplete = read_json(&workspace.root.join("complete.json"))?;
            let cut = Source::open(&complete.cut.path)?;
            if digest(&cut, &cancel)? != complete.cut {
                return Err(invalid(&cut.path, "A saved source-cut variant changed."));
            }
        }
        Ok(())
    }
    pub(in crate::jobs) fn finish(self) -> Result<(), AppError> {
        for workspace in self.0 {
            workspace.cleanup()?;
        }
        Ok(())
    }
    /// Variants bind only the cut recipe. New settings select/create another
    /// immutable variant; the committed variant remains available for rollback.
    pub(in crate::jobs) fn rebind(self, _settings: &EncodeSettings) -> Result<(), AppError> {
        self.verify_source()
    }
}
pub(in crate::jobs) fn prepare_discard(
    id: &str,
    request: &RemuxRequest,
    _settings: &EncodeSettings,
) -> Result<Option<Cleanup>, AppError> {
    let workspaces = cut_roots(id, request)?
        .into_iter()
        .map(|root| CutWorkspace::open_path(root, id, request))
        .collect::<Result<Vec<_>, _>>()?;
    if workspaces.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Cleanup(workspaces)))
    }
}
pub(in crate::jobs) fn prepare_recovery_action(
    id: &str,
    request: &RemuxRequest,
    saved: &EncodeSettings,
    _pending: &EncodeSettings,
) -> Result<Option<Cleanup>, AppError> {
    prepare_discard(id, request, saved)
}
pub(in crate::jobs) fn discard(
    id: &str,
    request: &RemuxRequest,
    settings: &EncodeSettings,
) -> Result<(), AppError> {
    if let Some(cleanup) = prepare_discard(id, request, settings)? {
        cleanup.finish()?;
    }
    Ok(())
}

/// Recovery independently binds its freshly hashed original/cut inputs to the
/// verified variant already held by the encode worker. Do not reacquire its
/// workspace lock while that worker holds it.
#[allow(clippy::too_many_arguments)]
pub(in crate::jobs) fn verify_variant(
    id: &str,
    request: &RemuxRequest,
    settings: &EncodeSettings,
    source: &Path,
    original_sha: &str,
    original_length: u64,
    cut_sha: &str,
    cut_length: u64,
) -> Result<(), AppError> {
    let root = cut_root(id, request, settings)?;
    let owner: CutOwner = read_json(&root.join("owner.json"))?;
    let complete: CutComplete = read_json(&root.join("complete.json"))?;
    if owner.version != 1
        || owner.id != id
        || owner.request != *request
        || owner.directory != identity(&root)?
        || root != cut_root(id, request, &owner.settings)?
        || source != root.join(cut_name(&owner.section))
        || complete.cut.path != source
        || owner.files.get(&cut_name(&owner.section)) != Some(&complete.cut.identity)
        || identity(source)? != complete.cut.identity
        || owner.original.sha256 != original_sha
        || owner.original.length != original_length
        || complete.cut.sha256 != cut_sha
        || complete.cut.length != cut_length
        || identity(&owner.original.path)? != owner.original.identity
    {
        return Err(invalid(
            &root,
            "The proposed AV1AN input is not the exact verified source-cut variant for these authorized settings.",
        ));
    }
    Ok(())
}

pub(in crate::jobs) fn discard_variants_except(
    id: &str,
    request: &RemuxRequest,
    settings: &[EncodeSettings],
) -> Result<(), AppError> {
    let keep = settings
        .iter()
        .filter(|settings| {
            settings.source_trim
                && settings.trim.is_some()
                && settings.backend == media_core::EncodeBackend::Av1an
        })
        .map(|settings| cut_root(id, request, settings))
        .collect::<Result<Vec<_>, _>>()?;
    let owned = cut_roots(id, request)?
        .into_iter()
        .filter(|root| !keep.contains(root))
        .map(|root| CutWorkspace::open_path(root, id, request))
        .collect::<Result<Vec<_>, _>>()?;
    for workspace in owned {
        workspace.cleanup()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_activity_keeps_diagnostics_from_both_streams() {
        for line in [
            "frame=1200",
            "fps=0.00",
            "stream_0_0_q=-1.0",
            "bitrate=9299.4kbits/s",
            "total_size=23288052",
            "out_time_us=20034000",
            "out_time_ms=20034000",
            "out_time=00:00:20.034000",
            "dup_frames=0",
            "drop_frames=0",
            "speed=618x",
            "progress=end",
        ] {
            assert_eq!(copy_diagnostic(ProcessEvent::Stdout(line.into())), None);
            assert_eq!(
                copy_diagnostic(ProcessEvent::Stderr(line.into())).as_deref(),
                Some(line)
            );
        }
        let diagnostic = "diagnostic=unexpected stdout";
        assert_eq!(
            copy_diagnostic(ProcessEvent::Stdout(diagnostic.into())).as_deref(),
            Some(diagnostic)
        );
    }

    #[test]
    fn published_cut_adopts_immutable_source_without_writable_alias() {
        let root = std::env::temp_dir().join(format!(
            "jesses-source-cut-publish-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let output = root.join("cut.mkv");
        let mut temporary = Temporary::create(&output, "cut").unwrap();
        let partial = temporary.path.clone();
        let mut writer = temporary.clone_file().unwrap();
        writer.write_all(b"verified source-cut bytes").unwrap();
        writer.sync_all().unwrap();
        drop(writer);
        temporary.flush_nonempty().unwrap();
        let original_identity = file_identity(&temporary.clone_file().unwrap()).unwrap();
        let result = publish_source(&mut temporary, &output);
        let adopted = result
            .as_ref()
            .map(|source| {
                source.verify().unwrap();
                assert_eq!(identity(&source.path).unwrap(), original_identity);
                assert_eq!(
                    fs::read(&source.path).unwrap(),
                    b"verified source-cut bytes"
                );
                assert!(!partial.exists());
            })
            .map_err(|error| format!("{}: {}", error.code, error.message));
        drop(result);
        drop(temporary);
        let _ = fs::remove_file(&partial);
        fs::remove_file(&output).unwrap();
        fs::remove_dir(&root).unwrap();
        adopted.unwrap();
    }
}
