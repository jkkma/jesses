//! Optional parallel scene detection for the L-SMASH reader.
//!
//! Slice scripts and reader indexes stay in a unique launch directory. The
//! validated merged scene list is installed at av1an's durable scene path so
//! its normal recovery checkpoint can receipt that same file.
use super::*;
use serde_json::{Map, Value};
use std::{fs, io::Write};
use tokio::task::JoinSet;

const MIN_FRAMES_PER_SLICE: usize = 1_200;
const MAX_SLICES: usize = 16;
// Match the durable recovery receipt limit so a successful prepass cannot
// create a scene file that its own recovery gate refuses later.
const MAX_SCENE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) struct Prepared {
    pub(super) path: PathBuf,
    pub(super) slices: usize,
    pub(super) scenes: usize,
    scene_list: Source,
    index: Source,
    chunks_guard: fs::File,
    split_guard: fs::File,
}

impl Prepared {
    pub(super) fn verify(&self) -> Result<(), AppError> {
        let chunks = self.path.parent().expect("durable scene parent");
        for (path, guard) in [
            (chunks.to_owned(), &self.chunks_guard),
            (chunks.join("split"), &self.split_guard),
        ] {
            recovery::verify_launch_directory(&path, guard)?;
        }
        recovery::verify_launch_file(&self.scene_list)?;
        recovery::verify_launch_file(&self.index)?;
        for name in ["chunks.json", "done.json"] {
            let marker = chunks.join(name);
            match fs::symlink_metadata(&marker) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(failure(&marker, error.to_string())),
                Ok(_) => {
                    return Err(failure(
                        &marker,
                        "Unexpected recovery marker before the fresh encode.",
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn supports_preload(version: &str) -> bool {
    // This revision (or any build advertising the preload capability tag) only
    // clears --temp when --resume is absent. With no queue or completion
    // marker, it then changes back to normal fresh mode.
    version.contains("[preload-scenes-v1]")
        || version.lines().any(|line| {
            line.starts_with("av1an 0.5.2-unstable (rev 805dad69143fa0a81cfe2fb89c0b9e90a828ea72) ")
        })
}

fn failure(path: &Path, detail: impl Into<String>) -> AppError {
    files::error("AV1AN_SCENE_DETECTION_FAILED", detail, path)
}

fn capture_error(error: supervisor::SupervisorError, path: &Path) -> AppError {
    if matches!(error, supervisor::SupervisorError::Cleanup(_)) {
        files::error("AV1AN_SCENE_CLEANUP_FAILED", error.to_string(), path)
    } else {
        process_error(error, path)
    }
}

/// The job's frame count when the prepass starts. A count read from the file
/// headers only sizes the slices; the reader's own index then says where the
/// video ends, exactly as it does when av1an detects scenes itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlannedFrames {
    Counted(usize),
    FromHeaders(usize),
}

impl PlannedFrames {
    fn planned(self) -> usize {
        match self {
            Self::Counted(frames) | Self::FromHeaders(frames) => frames,
        }
    }

    /// The frame count the slices tile, given what the reader indexed.
    fn indexed(self, indexed: Option<usize>, slices: usize) -> Result<usize, String> {
        match (self, indexed) {
            (Self::Counted(expected), Some(frames)) if frames == expected => Ok(frames),
            (Self::Counted(expected), _) => Err(format!(
                "L-SMASH indexed {indexed:?} frames; the validated source has {expected}. Parallel scene detection was not used."
            )),
            // Keep the slice count chosen from the header estimate unless the
            // real video is so much shorter that slicing it is pointless.
            (Self::FromHeaders(_), Some(frames))
                if frames >= slices * (MIN_FRAMES_PER_SLICE / 2) =>
            {
                Ok(frames)
            }
            (Self::FromHeaders(planned), _) => Err(format!(
                "L-SMASH indexed {indexed:?} frames where the file headers suggested {planned}. Parallel scene detection was not used."
            )),
        }
    }
}

fn slice_count(requested: u8, frames: usize) -> usize {
    usize::from(requested)
        .min(MAX_SLICES)
        .min(frames / MIN_FRAMES_PER_SLICE)
}

/// Every slice is its own av1an process decoding the source at full size.
/// Measured with the pinned build: about 0.55 GiB a slice for 1080p and
/// 1.7 GiB for 3832x1600. Keep all slices within half of the memory that is
/// free right now, so a large source cannot push the machine into its page file.
fn memory_slices(picture: (u32, u32), available_mib: Option<u32>) -> usize {
    let Some(available) = available_mib else {
        return MAX_SLICES;
    };
    let megapixels = f64::from(picture.0) * f64::from(picture.1) / 1_000_000.0;
    let per_slice_mib = 128.0 + megapixels * 300.0;
    ((f64::from(available) / 2.0 / per_slice_mib) as usize).min(MAX_SLICES)
}

fn bounds(frames: usize, slices: usize) -> Vec<usize> {
    (0..=slices)
        .map(|index| ((frames as u128 * index as u128) / slices as u128) as usize)
        .collect()
}

fn vspipe(environment: &supervisor::ChildEnvironment) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "vspipe.exe"
    } else {
        "vspipe"
    };
    environment.path.as_ref().and_then(|path| {
        std::env::split_paths(path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

fn script(
    source: &Path,
    cache: &Path,
    start: usize,
    end: Option<usize>,
) -> Result<String, AppError> {
    let source = source.to_str().ok_or_else(|| {
        failure(
            source,
            "The source path cannot be represented in a VapourSynth script.",
        )
    })?;
    let cache_text = cache.to_str().ok_or_else(|| {
        failure(
            cache,
            "The scene index path cannot be represented in a VapourSynth script.",
        )
    })?;
    let source = serde_json::to_string(source).expect("serializable path");
    let cache = serde_json::to_string(cache_text).expect("serializable path");
    let trim = end.map_or_else(
        || format!("clip = clip[{start}:]"),
        |end| format!("clip = clip[{start}:{end}]"),
    );
    Ok(format!(
        "import vapoursynth as vs\ncore = vs.core\nclip = core.lsmas.LWLibavSource(source={source}, cachefile={cache})\n{trim}\nclip.set_output()\n"
    ))
}

fn write_new(path: &Path, content: &[u8]) -> Result<(), AppError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| failure(path, error.to_string()))?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|error| failure(path, error.to_string()))
}

fn parse_frame_count(output: &[u8]) -> Option<usize> {
    String::from_utf8_lossy(output).lines().find_map(|line| {
        line.trim()
            .strip_prefix("Frames:")
            .and_then(|frames| frames.trim().parse::<usize>().ok())
    })
}

/// Returns `None` when the requested optimization is inapplicable. Errors are
/// reported to the caller, which may log them and let av1an detect in-run.
#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare(
    executable: &Path,
    environment: &supervisor::ChildEnvironment,
    input: &Path,
    launch_directory: &Path,
    durable_chunks: &Path,
    frames: PlannedFrames,
    picture: (u32, u32),
    options: media_core::Av1anOptions,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<Prepared>, AppError> {
    use media_core::{Av1anChunkMethod, Av1anSplitMethod};
    if options.scene_detection_slices < 2
        || options.split_method != Av1anSplitMethod::SceneDetection
        || options.chunk_method != Av1anChunkMethod::Lsmash
    {
        return Ok(None);
    }
    let slices = slice_count(options.scene_detection_slices, frames.planned()).min(memory_slices(
        picture,
        crate::av1an_resources::available_memory_mib(),
    ));
    if slices < 2 {
        return Ok(None);
    }
    check_cancel(cancel)?;
    let reader = vspipe(environment).ok_or_else(|| {
        failure(input, "VSPipe is missing from the selected av1an environment; scene detection will run inside av1an.")
    })?;
    let work = launch_directory.join("scene-prepass");
    let cache = work.join("cache.lwi");
    fs::create_dir(&work).map_err(|error| failure(&work, error.to_string()))?;
    let scratch = recovery::SceneScratch::capture(&work)?;
    let result = detect(
        executable,
        environment,
        input,
        &work,
        &cache,
        reader,
        frames,
        options,
        slices,
        cancel,
    )
    .await;
    if result
        .as_ref()
        .is_err_and(|error| error.code == "AV1AN_SCENE_CLEANUP_FAILED")
    {
        // Keep the named scratch contents when a child could still be alive.
        return result.map(|_| None);
    }
    let cleanup_path = work.clone();
    let durable_chunks = durable_chunks.to_owned();
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        let prepared = result.and_then(|(scenes, bytes)| {
            check_cancel(&cancel)?;
            install(&durable_chunks, &cache, slices, scenes, &bytes, &cancel)
                .map(Some)
                .map_err(|error| {
                    if error.code == "JOB_CANCELED" {
                        error
                    } else {
                        files::error("AV1AN_SCENE_CLEANUP_FAILED", error.message, &durable_chunks)
                    }
                })
        });
        scratch.remove(&cleanup_path).map_err(|error| {
            files::error("AV1AN_SCENE_CLEANUP_FAILED", error.message, &cleanup_path)
        })?;
        prepared
    })
    .await
    .map_err(|error| files::error("AV1AN_SCENE_CLEANUP_FAILED", error.to_string(), &work))?
}

#[allow(clippy::too_many_arguments)]
async fn detect(
    executable: &Path,
    environment: &supervisor::ChildEnvironment,
    input: &Path,
    work: &Path,
    cache: &Path,
    reader: PathBuf,
    frames: PlannedFrames,
    options: media_core::Av1anOptions,
    slices: usize,
    cancel: &watch::Receiver<bool>,
) -> Result<(usize, Vec<u8>), AppError> {
    let help = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable: executable.to_owned(),
            args: vec!["--help".into()],
            cwd: Some(work.to_owned()),
        },
        cancel.clone(),
        512 * 1024,
        Duration::from_secs(15),
        Some(environment),
    )
    .await
    .map_err(|error| capture_error(error, executable))?;
    let help_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    if !help.status.success()
        || ["--sc-only", "--scenes"]
            .iter()
            .any(|flag| !help_text.split_whitespace().any(|word| word == *flag))
    {
        return Err(failure(
            executable,
            "This av1an does not advertise --sc-only and --scenes.",
        ));
    }

    // Build one index before concurrent readers open it. The completed index
    // is installed at the exact cachefile used by the fresh encode.
    let info_script = work.join("source-info.vpy");
    write_new(&info_script, script(input, cache, 0, None)?.as_bytes())?;
    let info = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable: reader,
            args: vec!["--info".into(), info_script.into_os_string(), "-".into()],
            cwd: Some(work.to_owned()),
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(15 * 60),
        Some(environment),
    )
    .await
    .map_err(|error| capture_error(error, input))?;
    let indexed_frames = info
        .status
        .success()
        .then(|| parse_frame_count(&info.stdout).or_else(|| parse_frame_count(&info.stderr)))
        .flatten();
    let total = frames
        .indexed(indexed_frames, slices)
        .map_err(|detail| failure(input, detail))?;

    let limits = bounds(total, slices);
    let mut commands = Vec::with_capacity(slices);
    for index in 0..slices {
        let slice_dir = work.join(format!("slice-{index:02}"));
        fs::create_dir(&slice_dir).map_err(|error| failure(&slice_dir, error.to_string()))?;
        let slice_script = slice_dir.join("input.vpy");
        write_new(
            &slice_script,
            script(input, cache, limits[index], Some(limits[index + 1]))?.as_bytes(),
        )?;
        let scenes = slice_dir.join("scenes.json");
        let temp = slice_dir.join("temp");
        let output = slice_dir.join("unused.mkv");
        let mut args: Vec<OsString> = [
            "-y",
            "--sc-only",
            "--split-method",
            "av-scenechange",
            "-m",
            "lsmash",
            "--scenes",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.push(scenes.clone().into_os_string());
        args.extend([
            "--sc-method".into(),
            (if options.scene_detection == media_core::Av1anSceneDetection::Standard {
                "standard"
            } else {
                "fast"
            })
            .into(),
            "--extra-split".into(),
            options.maximum_chunk_frames.to_string().into(),
            "--min-scene-len".into(),
            options.minimum_scene_frames.to_string().into(),
        ]);
        if let Some(height) = options.scene_downscale_height {
            args.extend(["--sc-downscale-height".into(), height.to_string().into()]);
        }
        args.extend([
            "--log-file".into(),
            slice_dir.join("av1an.log").into_os_string(),
            "--temp".into(),
            temp.into_os_string(),
            "-i".into(),
            slice_script.into_os_string(),
            "-o".into(),
            output.into_os_string(),
        ]);
        let spec = CommandSpec {
            executable: executable.to_owned(),
            args,
            cwd: Some(slice_dir),
        };
        commands.push((index, scenes, spec));
    }
    let documents = detect_slices(commands, environment, cancel, work).await?;
    let (merged, scene_count) = merge(&documents, &limits)?;
    let bytes = serde_json::to_vec_pretty(&merged).expect("serializable scenes");
    if bytes.len() as u64 > MAX_SCENE_BYTES {
        return Err(failure(
            work,
            "The merged scene list exceeds the recovery receipt limit.",
        ));
    }
    Ok((scene_count, bytes))
}

async fn detect_slices(
    commands: Vec<(usize, PathBuf, CommandSpec)>,
    environment: &supervisor::ChildEnvironment,
    cancel: &watch::Receiver<bool>,
    work: &Path,
) -> Result<Vec<Value>, AppError> {
    let mut documents = vec![None; commands.len()];
    let (stop, stopped) = watch::channel(false);
    let mut tasks = JoinSet::new();
    for (index, scenes, spec) in commands {
        let environment = environment.clone();
        let cancel = stopped.clone();
        let pause = supervisor::scoped_pause();
        tasks.spawn(async move {
            let execution = supervisor::run_capture_with_environment(
                &spec,
                cancel,
                256 * 1024,
                Duration::from_secs(15 * 60),
                Some(&environment),
            );
            let run = if let Some(control) = pause {
                supervisor::with_pause(control, execution).await
            } else {
                execution.await
            }
            .map_err(|error| capture_error(error, &spec.executable))?;
            if !run.status.success() {
                return Err(failure(
                    &scenes,
                    format!(
                        "Scene slice {} exited with {}. {}",
                        index + 1,
                        run.status,
                        String::from_utf8_lossy(&run.stderr)
                            .lines()
                            .last()
                            .unwrap_or_default()
                    ),
                ));
            }
            let size = fs::metadata(&scenes)
                .map_err(|error| failure(&scenes, error.to_string()))?
                .len();
            if size == 0 || size > MAX_SCENE_BYTES {
                return Err(failure(
                    &scenes,
                    "The scene list is empty or exceeds the recovery receipt limit.",
                ));
            }
            let bytes = fs::read(&scenes).map_err(|error| failure(&scenes, error.to_string()))?;
            let value = serde_json::from_slice::<Value>(&bytes)
                .map_err(|error| failure(&scenes, format!("Invalid scene JSON: {error}")))?;
            Ok::<_, AppError>((index, value))
        });
    }
    let mut owner = cancel.clone();
    let mut first_error = None;
    while !tasks.is_empty() {
        tokio::select! {
            biased;
            _ = owner.wait_for(|value| *value), if first_error.is_none() => {
                first_error = Some(process_error(supervisor::SupervisorError::Cancelled, work));
                stop.send_replace(true);
            }
            result = tasks.join_next() => {
                let error = match result.expect("a task is present") {
                    Ok(Ok((index, document))) => {
                        documents[index] = Some(document);
                        continue;
                    }
                    Ok(Err(error)) => error,
                    Err(error) => failure(work, format!("Scene slice task failed: {error}")),
                };
                if error.code == "AV1AN_SCENE_CLEANUP_FAILED" || first_error.is_none() {
                    first_error = Some(error);
                }
                stop.send_replace(true);
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    check_cancel(cancel)?;
    Ok(documents
        .into_iter()
        .map(|item| item.expect("every task completed"))
        .collect())
}

fn install(
    durable_chunks: &Path,
    cache: &Path,
    slices: usize,
    scenes: usize,
    bytes: &[u8],
    cancel: &watch::Receiver<bool>,
) -> Result<Prepared, AppError> {
    let path = durable_chunks.join("scenes.json");
    let split = durable_chunks.join("split");
    let index = split.join("cache.lwi");
    let failed = |error: std::io::Error| failure(durable_chunks, error.to_string());
    let scratch_index = recovery::open_launch_file(cache)?;
    // Only install into a fresh directory, without replacing even a partially
    // initialized recovery workspace or following a pre-existing link.
    fs::create_dir(durable_chunks).map_err(failed)?;
    let chunks_guard = recovery::launch_directory_guard(durable_chunks)?;
    fs::create_dir(&split).map_err(failed)?;
    let split_guard = recovery::launch_directory_guard(&split)?;
    let mut new_index = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&index)
        .map_err(failed)?;
    use std::io::Read;
    let mut reader = scratch_index.file();
    let length = reader.metadata().map_err(failed)?.len();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = reader.read(&mut buffer).map_err(failed)?;
        if count == 0 {
            break;
        }
        new_index.write_all(&buffer[..count]).map_err(failed)?;
    }
    new_index.sync_all().map_err(failed)?;
    drop(new_index);
    scratch_index.verify()?;
    if length == 0 {
        return Err(failure(&index, "The prebuilt L-SMASH index is empty."));
    }
    write_new(&path, bytes)?;
    let prepared = Prepared {
        path,
        slices,
        scenes,
        scene_list: recovery::open_launch_file(&durable_chunks.join("scenes.json"))?,
        index: recovery::open_launch_file(&index)?,
        chunks_guard,
        split_guard,
    };
    prepared.verify()?;
    Ok(prepared)
}

fn offset_list(
    value: &Value,
    key: &str,
    offset: usize,
    expected_frames: usize,
    target: &mut Vec<Value>,
) -> Result<(), AppError> {
    let scenes = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| failure(Path::new(key), format!("Missing {key} scene list.")))?;
    if scenes.is_empty() {
        return Err(failure(Path::new(key), "A scene slice has no scenes."));
    }
    let mut position = 0usize;
    for scene in scenes {
        let start = scene
            .get("start_frame")
            .and_then(Value::as_u64)
            .and_then(|number| usize::try_from(number).ok())
            .ok_or_else(|| failure(Path::new(key), "Scene start_frame is missing or invalid."))?;
        let end = scene
            .get("end_frame")
            .and_then(Value::as_u64)
            .and_then(|number| usize::try_from(number).ok())
            .ok_or_else(|| failure(Path::new(key), "Scene end_frame is missing or invalid."))?;
        if start != position || end <= start || end > expected_frames {
            return Err(failure(
                Path::new(key),
                format!("{key} does not tile its slice at frame {position}."),
            ));
        }
        let mut copy = scene.clone();
        let object = copy
            .as_object_mut()
            .ok_or_else(|| failure(Path::new(key), "A scene is not an object."))?;
        object.insert("start_frame".into(), Value::from(start + offset));
        object.insert("end_frame".into(), Value::from(end + offset));
        target.push(copy);
        position = end;
    }
    if position != expected_frames {
        return Err(failure(
            Path::new(key),
            format!("{key} ends at {position} of {expected_frames} slice frames."),
        ));
    }
    Ok(())
}

fn merge(documents: &[Value], limits: &[usize]) -> Result<(Value, usize), AppError> {
    let total = *limits
        .last()
        .ok_or_else(|| failure(Path::new("scenes"), "Missing slice bounds."))?;
    if documents.is_empty() || limits.len() != documents.len() + 1 {
        return Err(failure(
            Path::new("scenes"),
            "Scene slice count does not match bounds.",
        ));
    }
    let mut template: Map<String, Value> = documents[0]
        .as_object()
        .cloned()
        .ok_or_else(|| failure(Path::new("scenes"), "Scene JSON root is not an object."))?;
    let has_splits = documents[0]
        .get("split_scenes")
        .is_some_and(Value::is_array);
    let mut scenes = Vec::new();
    let mut splits = Vec::new();
    for (index, document) in documents.iter().enumerate() {
        let expected = limits[index + 1] - limits[index];
        if document.get("frames").and_then(Value::as_u64) != Some(expected as u64) {
            return Err(failure(
                Path::new("scenes"),
                format!("Slice {} reports the wrong frame count.", index + 1),
            ));
        }
        if document.get("split_scenes").is_some_and(Value::is_array) != has_splits {
            return Err(failure(
                Path::new("scenes"),
                "Scene slices disagree on split_scenes schema.",
            ));
        }
        offset_list(document, "scenes", limits[index], expected, &mut scenes)?;
        if has_splits {
            offset_list(
                document,
                "split_scenes",
                limits[index],
                expected,
                &mut splits,
            )?;
        }
    }
    let scene_count = scenes.len();
    template.insert("frames".into(), Value::from(total));
    template.insert("scenes".into(), Value::Array(scenes));
    if has_splits {
        template.insert("split_scenes".into(), Value::Array(splits));
    } else {
        template.remove("split_scenes");
    }
    Ok((Value::Object(template), scene_count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn limits_slices_and_tiles_boundaries() {
        assert_eq!(slice_count(16, 2_399), 1);
        assert_eq!(slice_count(16, 2_400), 2);
        assert_eq!(slice_count(16, 19_200), 16);
        assert_eq!(bounds(5_001, 4), vec![0, 1250, 2500, 3750, 5001]);
    }

    #[test]
    fn free_memory_bounds_the_slices_of_a_large_picture() {
        // Unknown memory leaves the requested count alone.
        assert_eq!(memory_slices((3840, 2160), None), MAX_SLICES);
        // 12 GiB free: half of it holds two 4K slices but every 1080p slice.
        assert_eq!(memory_slices((3840, 2160), Some(12 * 1024)), 2);
        assert_eq!(memory_slices((1920, 1080), Some(12 * 1024)), 8);
        assert_eq!(memory_slices((1920, 1080), Some(64 * 1024)), MAX_SLICES);
        // Too little for even two slices: the caller skips the prepass.
        assert!(memory_slices((3840, 2160), Some(8 * 1024)) < 2);
    }

    #[test]
    fn a_counted_plan_must_match_the_index_and_a_header_estimate_follows_it() {
        let counted = PlannedFrames::Counted(4_800);
        assert_eq!(counted.indexed(Some(4_800), 4), Ok(4_800));
        assert!(
            counted
                .indexed(Some(4_799), 4)
                .unwrap_err()
                .contains("validated source has 4800")
        );
        assert!(counted.indexed(None, 4).is_err());

        // Header counts can be a few frames off; the reader's index decides.
        let estimated = PlannedFrames::FromHeaders(4_800);
        assert_eq!(estimated.planned(), 4_800);
        assert_eq!(estimated.indexed(Some(4_793), 4), Ok(4_793));
        assert_eq!(estimated.indexed(Some(4_900), 4), Ok(4_900));
        assert_eq!(estimated.indexed(Some(2_400), 4), Ok(2_400));
        assert!(
            estimated
                .indexed(Some(2_399), 4)
                .unwrap_err()
                .contains("file headers suggested 4800")
        );
        assert!(estimated.indexed(None, 4).is_err());
    }

    #[test]
    fn merges_validated_scene_documents_without_losing_metadata() {
        let documents = [
            json!({"frames": 1200, "engine_marker": "kept", "scenes": [
                {"start_frame": 0, "end_frame": 500, "zone_overrides": null},
                {"start_frame": 500, "end_frame": 1200, "zone_overrides": null}
            ], "split_scenes": [{"start_frame":0,"end_frame":1200,"zone_overrides":null}]}),
            json!({"frames": 1200, "scenes": [{"start_frame":0,"end_frame":1200,"zone_overrides":null}],
                "split_scenes": [{"start_frame":0,"end_frame":1200,"zone_overrides":null}]}),
        ];
        let (merged, count) = merge(&documents, &[0, 1200, 2400]).unwrap();
        assert_eq!(count, 3);
        assert_eq!(merged["frames"], 2400);
        assert_eq!(merged["engine_marker"], "kept");
        assert_eq!(merged["scenes"][2]["start_frame"], 1200);
        assert_eq!(merged["split_scenes"][1]["end_frame"], 2400);
    }

    #[test]
    fn rejects_frame_mismatch_gap_overlap_and_inconsistent_split_schema() {
        let good = json!({"frames":1200,"scenes":[{"start_frame":0,"end_frame":1200}],
            "split_scenes":[{"start_frame":0,"end_frame":1200}]});
        let wrong_count = json!({"frames":1199,"scenes":[{"start_frame":0,"end_frame":1200}],
            "split_scenes":[{"start_frame":0,"end_frame":1200}]});
        assert!(merge(&[good.clone(), wrong_count], &[0, 1200, 2400]).is_err());
        let gap = json!({"frames":1200,"scenes":[{"start_frame":0,"end_frame":600},
            {"start_frame":601,"end_frame":1200}], "split_scenes":[{"start_frame":0,"end_frame":1200}]});
        assert!(merge(&[good.clone(), gap], &[0, 1200, 2400]).is_err());
        let overlap = json!({"frames":1200,"scenes":[{"start_frame":0,"end_frame":700},
            {"start_frame":699,"end_frame":1200}], "split_scenes":[{"start_frame":0,"end_frame":1200}]});
        assert!(merge(&[good.clone(), overlap], &[0, 1200, 2400]).is_err());
        let missing = json!({"frames":1200,"scenes":[{"start_frame":0,"end_frame":1200}]});
        assert!(merge(&[good, missing], &[0, 1200, 2400]).is_err());
    }

    #[test]
    fn escapes_source_paths_in_owned_scripts() {
        let content = script(
            Path::new("C:\\Media\\a\"b.mkv"),
            Path::new("C:\\Cache\\index"),
            5,
            Some(100),
        )
        .unwrap();
        assert!(content.contains("C:\\\\Media\\\\a\\\"b.mkv"));
        assert!(content.contains("clip = clip[5:100]"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn failed_slice_joins_running_sibling_before_checked_cleanup() {
        use std::time::UNIX_EPOCH;

        let root = std::env::temp_dir().join(format!(
            "jesses-scene-cancel-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let work = root.join("scene-prepass");
        let slice = work.join("slice-01");
        fs::create_dir_all(&slice).unwrap();
        let scratch = recovery::SceneScratch::capture(&work).unwrap();
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let command = |line: &str| CommandSpec {
            executable: powershell.clone(),
            args: ["-NoProfile", "-NonInteractive", "-Command", line]
                .into_iter()
                .map(Into::into)
                .collect(),
            cwd: Some(slice.clone()),
        };
        let scripts = [
            "$ErrorActionPreference = 'Stop'; while (!(Test-Path -LiteralPath 'scenes.json')) { Start-Sleep -Milliseconds 20 }; exit 7",
            "$ErrorActionPreference = 'Stop'; $held = [IO.File]::Open((Join-Path $PWD 'input.vpy'), 'Create', 'ReadWrite', 'None'); Set-Content -LiteralPath 'scenes.json' -Value 'running'; Start-Sleep -Seconds 60",
        ];
        let commands = scripts
            .into_iter()
            .enumerate()
            .map(|(index, script)| (index, slice.join("scenes.json"), command(script)))
            .collect();
        let (_owner, cancel) = watch::channel(false);
        let error = tokio::time::timeout(
            Duration::from_secs(15),
            detect_slices(commands, &Default::default(), &cancel, &work),
        )
        .await
        .expect("the sibling must stop promptly")
        .unwrap_err();
        assert!(
            error
                .message
                .contains("Scene slice 1 exited with exit code: 7"),
            "{error:?}"
        );
        assert!(slice.join("input.vpy").exists());
        scratch.remove(&work).unwrap();
        assert!(!work.exists());
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn enables_preload_only_for_the_pinned_av1an_revision() {
        assert!(supports_preload(
            "\nav1an 0.5.2-unstable (rev 805dad69143fa0a81cfe2fb89c0b9e90a828ea72) (Release)\n"
        ));
        assert!(!supports_preload(
            "av1an 0.5.2-unstable (rev 0000000000000000000000000000000000000000) (Release)"
        ));
        assert!(!supports_preload("av1an 0.5.2"));
    }

    #[tokio::test]
    #[ignore = "requires av1an, FFmpeg, VSPipe and the L-SMASH VapourSynth plugin on PATH"]
    async fn native_parallel_prepass_covers_all_source_frames() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "jesses-scene-prepass-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let input = root.join("source.mkv");
        let encoded = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=128x96:rate=24",
                "-frames:v",
                "2400",
                "-c:v",
                "ffv1",
            ])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            encoded.status.success(),
            "{}",
            String::from_utf8_lossy(&encoded.stderr)
        );
        let original = fs::read(&input).unwrap();
        let launch = root.join("launch");
        let chunks = root.join("chunks");
        fs::create_dir(&launch).unwrap();
        let path = std::env::var_os("PATH").expect("native tool PATH");
        let environment = supervisor::ChildEnvironment::with_path(&path);
        let av1an = std::env::var_os("JESSES_TEST_AV1AN")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::split_paths(&path)
                    .map(|directory| {
                        directory.join(if cfg!(windows) { "av1an.exe" } else { "av1an" })
                    })
                    .find(|candidate| candidate.is_file())
            })
            .expect("native av1an executable on PATH or JESSES_TEST_AV1AN");
        let (_owner, cancel) = watch::channel(false);
        let prepared = prepare(
            &av1an,
            &environment,
            &input,
            &launch,
            &chunks,
            PlannedFrames::Counted(2400),
            (128, 96),
            media_core::Av1anOptions {
                scene_detection_slices: 2,
                scene_downscale_height: Some(96),
                ..Default::default()
            },
            &cancel,
        )
        .await
        .unwrap()
        .expect("parallel prepass applies to 2400 frames");
        assert_eq!(prepared.slices, 2);
        assert_eq!(prepared.path, chunks.join("scenes.json"));
        let scenes: Value = serde_json::from_slice(&fs::read(&prepared.path).unwrap()).unwrap();
        assert_eq!(scenes["frames"], 2400);
        assert_eq!(scenes["scenes"][0]["start_frame"], 0);
        assert_eq!(
            scenes["scenes"].as_array().unwrap().last().unwrap()["end_frame"],
            2400
        );
        assert!(!original.is_empty());
        assert_eq!(fs::read(&input).unwrap(), original);
        assert!(!launch.join("scene-prepass").exists());
        assert!(chunks.join("scenes.json").exists());
        prepared.verify().unwrap();
        fs::write(chunks.join("done.json"), b"old recovery marker").unwrap();
        assert!(prepared.verify().is_err());
        drop(prepared);

        let (stop, cancelled) = watch::channel(false);
        let marker = launch.join("scene-prepass/slice-00/temp/done.json");
        let abort_chunks = root.join("aborted-chunks");
        let scene_job = prepare(
            &av1an,
            &environment,
            &input,
            &launch,
            &abort_chunks,
            PlannedFrames::Counted(2400),
            (128, 96),
            media_core::Av1anOptions {
                scene_detection_slices: 2,
                scene_downscale_height: Some(96),
                ..Default::default()
            },
            &cancelled,
        );
        let mut scene_job = std::pin::pin!(scene_job);
        tokio::select! {
            result = &mut scene_job => panic!("scene slices finished before the cancellation boundary: {:?}", result.err()),
            observed = tokio::time::timeout(Duration::from_secs(15), async {
                while !marker.is_file() {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            }) => observed.expect("a real slice must initialize its progress marker"),
        }
        stop.send_replace(true);
        let error = scene_job.await.err().expect("the prepass must stop");
        assert_eq!(error.code, "JOB_CANCELED", "{error:?}");
        assert!(!launch.join("scene-prepass").exists());
        assert!(!abort_chunks.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn supports_preload_accepts_capability_tag() {
        assert!(supports_preload(
            "av1an 0.6.0 (Release) [preload-scenes-v1]"
        ));
        assert!(!supports_preload("av1an 0.5.0 (Release)"));
    }
}
