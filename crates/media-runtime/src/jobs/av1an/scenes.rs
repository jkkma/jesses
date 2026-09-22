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
    _scratch: Scratch,
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        // This is always launch_directory/scene-prepass, created for one owned
        // attempt. No source or durable chunk path is below it.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn failure(path: &Path, detail: impl Into<String>) -> AppError {
    files::error("AV1AN_SCENE_DETECTION_FAILED", detail, path)
}

fn slice_count(requested: u8, frames: usize) -> usize {
    usize::from(requested)
        .min(MAX_SLICES)
        .min(frames / MIN_FRAMES_PER_SLICE)
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
        "import vapoursynth as vs\ncore = vs.core\nclip = core.lsmas.LWLibavSource(source={source}, cachedir={cache})\n{trim}\nclip.set_output()\n"
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
    expected_frames: usize,
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
    let slices = slice_count(options.scene_detection_slices, expected_frames);
    if slices < 2 {
        return Ok(None);
    }
    check_cancel(cancel)?;
    let reader = vspipe(environment).ok_or_else(|| {
        failure(input, "VSPipe is missing from the selected av1an environment; scene detection will run inside av1an.")
    })?;
    let work = launch_directory.join("scene-prepass");
    let cache = work.join("index");
    fs::create_dir(&work).map_err(|error| failure(&work, error.to_string()))?;
    let scratch = Scratch(work.clone());
    fs::create_dir(&cache).map_err(|error| failure(&cache, error.to_string()))?;

    let help = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable: executable.to_owned(),
            args: vec!["--help".into()],
            cwd: Some(work.clone()),
        },
        cancel.clone(),
        512 * 1024,
        Duration::from_secs(15),
        Some(environment),
    )
    .await
    .map_err(|error| process_error(error, executable))?;
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

    // Build the L-SMASH index once before concurrent readers open it. The
    // cachedir is inside the owned launch workspace; source media stay untouched.
    let info_script = work.join("source-info.vpy");
    write_new(&info_script, script(input, &cache, 0, None)?.as_bytes())?;
    let info = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable: reader,
            args: vec!["--info".into(), info_script.into_os_string(), "-".into()],
            cwd: Some(work.clone()),
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(15 * 60),
        Some(environment),
    )
    .await
    .map_err(|error| process_error(error, input))?;
    let indexed_frames =
        parse_frame_count(&info.stdout).or_else(|| parse_frame_count(&info.stderr));
    if !info.status.success() || indexed_frames != Some(expected_frames) {
        return Err(failure(
            input,
            format!(
                "L-SMASH indexed {indexed_frames:?} frames; the validated source has {expected_frames}. Parallel scene detection was not used."
            ),
        ));
    }

    let limits = bounds(expected_frames, slices);
    let mut tasks = JoinSet::new();
    for index in 0..slices {
        let slice_dir = work.join(format!("slice-{index:02}"));
        fs::create_dir(&slice_dir).map_err(|error| failure(&slice_dir, error.to_string()))?;
        let slice_script = slice_dir.join("input.vpy");
        write_new(
            &slice_script,
            script(input, &cache, limits[index], Some(limits[index + 1]))?.as_bytes(),
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
        let environment = environment.clone();
        let cancel = cancel.clone();
        tasks.spawn(async move {
            let run = supervisor::run_capture_with_environment(
                &spec,
                cancel,
                256 * 1024,
                Duration::from_secs(15 * 60),
                Some(&environment),
            )
            .await
            .map_err(|error| process_error(error, &spec.executable))?;
            if !run.status.success() {
                return Err(failure(
                    &scenes,
                    format!("Scene slice {} exited with {}.", index + 1, run.status),
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
    let mut documents = vec![None; slices];
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok((index, value))) => documents[index] = Some(value),
            Ok(Err(error)) => {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                return Err(error);
            }
            Err(error) => {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                return Err(failure(&work, format!("Scene slice task failed: {error}")));
            }
        }
    }
    check_cancel(cancel)?;
    let documents = documents
        .into_iter()
        .map(|item| item.expect("every task completed"))
        .collect::<Vec<_>>();
    let (merged, scene_count) = merge(&documents, &limits)?;
    let path = durable_chunks.join("scenes.json");
    let bytes = serde_json::to_vec_pretty(&merged).expect("serializable scenes");
    if bytes.len() as u64 > MAX_SCENE_BYTES {
        return Err(failure(
            &path,
            "The merged scene list exceeds the recovery receipt limit.",
        ));
    }
    // Recovery::prepare removed any unreceipted chunks tree before this fresh
    // attempt. A pre-existing tree or scene list here is unexpected, so never
    // replace it. A crash before av1an writes its queue is cleaned on the next
    // fresh recovery attempt; after that, recovery owns and verifies this file.
    fs::create_dir(durable_chunks).map_err(|error| failure(durable_chunks, error.to_string()))?;
    write_new(&path, &bytes)?;
    Ok(Some(Prepared {
        path,
        slices,
        scenes: scene_count,
        _scratch: scratch,
    }))
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
            2400,
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
        drop(prepared);
        assert!(!launch.join("scene-prepass").exists());
        assert!(chunks.join("scenes.json").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
