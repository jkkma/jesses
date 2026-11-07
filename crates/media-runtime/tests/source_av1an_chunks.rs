//! Fork chunk defaults retain -x even when the visible split method is None.
use media_core::*;
use media_runtime::JobManager;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
#[path = "common/source_trim_fixture.rs"]
mod fixture;
use fixture::*;
#[cfg(unix)]
static SOURCE_ENCODER_ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9, x264 and AV1AN"]
async fn source_raw_fps_alone_retains_unflagged_frame_mismatch_failure() {
    let _env = SOURCE_ENCODER_ENV.lock().await;
    let f = Fixture::new();
    let input = f.0.join("raw-input.mkv");
    let output = f.0.join("must-not-publish.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=24:d=2",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "0",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(input.clone().into());
    run("ffmpeg", a);
    let mut req = source_request(&input, &output);
    req.settings.source_filters = true;
    req.settings.av1an_filters = vec!["fps=12".into()];
    let manager = JobManager::new(f.0.join("logs"));
    let held = review(
        &manager,
        CommandEditRequest::Encode {
            request: Box::new(req),
        },
    )
    .await;
    assert!(!held.arguments.contains("--ignore-frame-mismatch"));
    let id = held.job.as_ref().unwrap().id.clone();
    confirm(&manager, held, None).await;
    let done = terminal(&manager, &id).await;
    manager.shutdown().await;
    assert_eq!(done.state, JobState::Failed, "{done:#?}");
    assert!(
        done.logs.iter().any(|line| line.contains("FRAME MISMATCH")),
        "{done:#?}"
    );
    assert!(!output.exists());
    let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/migration-validation/source-av1an-trim-20261101");
    std::fs::write(
        proof.join("source-raw-fps-failure.json"),
        serde_json::to_vec_pretty(&serde_json::json!({"logs":done.logs,"error":done.error}))
            .unwrap(),
    )
    .unwrap();
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9, x264 and AV1AN"]
async fn source_typed_fps_keeps_original_chunk_ordinals_and_counts_decoded_output() {
    let _env = SOURCE_ENCODER_ENV.lock().await;
    let f = Fixture::new();
    let input = f.0.join("twenty-four.mkv");
    let output = f.0.join("thirty.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=24:d=4",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "0",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(input.clone().into());
    run("ffmpeg", a);
    let mut req = source_request(&input, &output);
    req.settings.av1an_options.as_mut().unwrap().concat_method = Av1anConcatMethod::Mkvmerge;
    req.settings.temporal = Some(TemporalSettings {
        frame_rate: Some(FrameRate {
            numerator: 30,
            denominator: 1,
        }),
        ..Default::default()
    });
    let manager = JobManager::new(f.0.join("logs"));
    let held = review(
        &manager,
        CommandEditRequest::Encode {
            request: Box::new(req),
        },
    )
    .await;
    assert!(held.arguments.contains("--ignore-frame-mismatch"));
    assert!(
        held.arguments
            .replace('\'', "")
            .contains("--extra-split 30")
    );
    let id = held.job.as_ref().unwrap().id.clone();
    confirm(&manager, held, None).await;
    let done = terminal(&manager, &id).await;
    manager.shutdown().await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    let reference = f.0.join("source-equivalent.mkv");
    let work = f.0.join("source-engine");
    let mut a = args(&[
        "-y",
        "--keep",
        "--encoder",
        "x264",
        "--passes",
        "1",
        "--no-defaults",
        "--concat",
        "mkvmerge",
        "--chunk-method",
        "select",
        "--split-method",
        "none",
        "-x",
        "30",
        "--workers",
        "1",
        "--pix-format",
        "yuv420p",
        "--video-params",
        "--preset ultrafast --crf 0 --threads 1",
        "--audio-params",
        "-an -sn -dn",
        "--ffmpeg",
        "-vf fps=fps=30",
        "--ignore-frame-mismatch",
        "--temp",
    ]);
    a.push(work.clone().into());
    a.push("-i".into());
    a.push(input.clone().into());
    a.push("-o".into());
    a.push(reference.clone().into());
    run("av1an", a.clone());
    let scenes = read(&work.join("scenes.json"));
    assert_eq!(scenes["frames"], 96);
    assert_eq!(
        split_ranges(&scenes),
        vec![(0, 24), (24, 48), (48, 72), (72, 96)]
    );
    let queue = read(&work.join("chunks.json"));
    assert!(
        queue
            .as_array()
            .unwrap()
            .iter()
            .all(|chunk| chunk["ignore_frame_mismatch"] == true)
    );
    let actual = probe(&output);
    let expected = probe(&reference);
    assert_eq!(packets(&actual, 0).len(), 120);
    assert_eq!(packets(&expected, 0).len(), 120);
    let pixels = |path: &Path| {
        let mut a = args(&["-v", "error", "-i"]);
        a.push(path.into());
        a.extend(args(&[
            "-pix_fmt",
            "yuv420p",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-",
        ]));
        run("ffmpeg", a)
    };
    let actual_pixels = pixels(&output);
    let expected_pixels = pixels(&reference);
    assert!(
        actual_pixels == expected_pixels,
        "source typed FPS must run once per original24-frame chunk, not globally prerender120frames"
    );
    for (actual, expected) in packets(&actual, 0).iter().zip(packets(&expected, 0)) {
        assert!((number(actual, "pts_time") - number(expected, "pts_time")).abs() < 0.0021);
    }
    let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/migration-validation/source-av1an-trim-20261101");
    std::fs::write(proof.join("source-typed-fps.json"),serde_json::to_vec_pretty(&json!({"sourceArguments":a,"sourceScenes":scenes,"sourceQueue":queue,"actual":actual,"reference":expected})).unwrap()).unwrap();
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}
fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
async fn review(manager: &JobManager, request: CommandEditRequest) -> CommandEditReview {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    manager.prepare_command_edit(request, cancel).await.unwrap()
}
async fn confirm(manager: &JobManager, review: CommandEditReview, arguments: Option<String>) {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id,
                arguments: arguments.unwrap_or(review.arguments),
            },
            cancel,
        )
        .await
        .unwrap();
}
fn split_ranges(scenes: &Value) -> Vec<(u64, u64)> {
    scenes["split_scenes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|scene| {
            (
                scene["start_frame"].as_u64().unwrap(),
                scene["end_frame"].as_u64().unwrap(),
            )
        })
        .collect()
}
fn source_request(input: &Path, output: &Path) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into(),
            output_path: output.to_string_lossy().into(),
            stream_indices: vec![0],
        },
        settings: EncodeSettings {
            backend: EncodeBackend::Av1an,
            encoder: VideoEncoder::X264,
            workers: 1,
            preset: 0,
            crf: 0,
            encoding_preferences: Some(EncodingPreferences {
                keyframe_interval_seconds: 1,
                mp4_faststart: true,
                full_output_verification: true,
            }),
            av1an_options: Some(Av1anOptions {
                source_workflow: true,
                chunk_method: Av1anChunkMethod::Select,
                split_method: Av1anSplitMethod::FixedChunks,
                maximum_chunk_frames: 0,
                minimum_scene_frames: 999,
                chunk_order: Av1anChunkOrder::Sequential,
                scene_detection: Av1anSceneDetection::Fast,
                scene_downscale_height: Some(1),
                concat_method: Av1anConcatMethod::Ffmpeg,
                encoder_threads: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        },
    }
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9, x264 and AV1AN"]
async fn source_new_retains_completed_bytes_and_applies_new_filters_only_to_remaining_chunks() {
    let _env = SOURCE_ENCODER_ENV.lock().await;
    use std::{ffi::OsString, os::unix::fs::PermissionsExt};
    struct Restore(Option<OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                if let Some(value) = &self.0 {
                    std::env::set_var("JESSES_X264", value);
                } else {
                    std::env::remove_var("JESSES_X264");
                }
            }
        }
    }
    let f = Fixture::new();
    let encoder = f.0.join("encoder");
    std::fs::create_dir(&encoder).unwrap();
    let wrapper = encoder.join("x264");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\nsleep 0.3\nexec /usr/bin/x264 \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _restore = Restore(std::env::var_os("JESSES_X264"));
    unsafe {
        std::env::set_var("JESSES_X264", &wrapper);
    }
    let input = f.0.join("native-source.mkv");
    let output = f.0.join("native-new.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=30:d=4",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "0",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(input.clone().into());
    run("ffmpeg", a);
    let req = source_request(&input, &output);
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let job = manager.start_encode(req.clone()).await.unwrap();
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let current = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|current| current.id == job.id)
                .unwrap();
            let job = current;
            assert!(!job.state.is_terminal(), "{job:#?}");
            if job.recovery.as_ref().is_some_and(|progress| {
                progress.completed_frames > 0 && progress.completed_frames < 120
            }) {
                break job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    manager.stop_job(job.id.clone()).await.unwrap();
    assert_eq!(terminal(&manager, &job.id).await.state, JobState::Stopped);
    let workspace = PathBuf::from(&stopped.recovery.as_ref().unwrap().workspace);
    let before = read(&workspace.join("manifest.json"));
    let queue = std::fs::read(workspace.join("chunks/chunks.json")).unwrap();
    let scenes = std::fs::read(workspace.join("chunks/scenes.json")).unwrap();
    let old = before["completed"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, chunk)| {
            let path = workspace.join("chunks/encode").join(format!("{name}.264"));
            (
                path.clone(),
                std::fs::read(&path).unwrap(),
                std::fs::metadata(&path).unwrap().modified().unwrap(),
                chunk["frames"].as_u64().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let old_frames = old.iter().map(|chunk| chunk.3).sum::<u64>();
    assert!(old_frames > 0 && old_frames < 120);
    let mut settings = req.settings.clone();
    settings.crf = 39;
    settings
        .encoding_preferences
        .as_mut()
        .unwrap()
        .keyframe_interval_seconds = 2;
    settings.source_filters = true;
    settings.av1an_filters = vec!["hflip,scale=48:32".into()];
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while manager.list_av1an_recoveries().await.unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let attempt = || Av1anRecoveryAction::ResumeNew {
        id: job.id.clone(),
        settings: Box::new(settings.clone()),
    };
    let foreign = workspace.join("foreign.bin");
    std::fs::write(&foreign, b"foreign bytes").unwrap();
    let error = manager.av1an_recovery_action(attempt()).await.unwrap_err();
    assert_eq!(error.code, "RECOVERY_INVALID");
    assert!(foreign.exists());
    std::fs::remove_file(foreign).unwrap();
    let manifest_path = workspace.join("manifest.json");
    let sealed_manifest = std::fs::read(&manifest_path).unwrap();
    let mut unsealed = read(&manifest_path);
    unsealed.as_object_mut().unwrap().remove("queue_recipe");
    std::fs::write(&manifest_path, serde_json::to_vec(&unsealed).unwrap()).unwrap();
    let error = manager.av1an_recovery_action(attempt()).await.unwrap_err();
    assert!(
        error.message.contains("sealed original recipe"),
        "{error:#?}"
    );
    std::fs::write(&manifest_path, &sealed_manifest).unwrap();
    let (path, bytes, modified, _) = &old[0];
    let mut changed = bytes.clone();
    changed[0] ^= 1;
    std::fs::write(path, &changed).unwrap();
    let error = manager.av1an_recovery_action(attempt()).await.unwrap_err();
    assert_eq!(error.code, "RECOVERY_INVALID");
    std::fs::write(path, bytes).unwrap();
    std::fs::File::open(path)
        .unwrap()
        .set_modified(*modified)
        .unwrap();
    let queue_path = workspace.join("chunks/chunks.json");
    let mut changed = queue.clone();
    changed[0] = b' ';
    std::fs::write(&queue_path, changed).unwrap();
    let error = manager.av1an_recovery_action(attempt()).await.unwrap_err();
    assert_eq!(error.code, "RECOVERY_INVALID");
    std::fs::write(queue_path, &queue).unwrap();
    for (path, bytes, modified, _) in &old {
        assert_eq!(std::fs::read(path).unwrap(), *bytes);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            *modified
        );
    }
    assert!(!output.exists());
    let held = review(
        &manager,
        CommandEditRequest::ResumeNew {
            id: job.id.clone(),
            settings: Box::new(settings.clone()),
        },
    )
    .await;
    assert!(
        held.arguments
            .replace('\'', "")
            .contains("--extra-split 60")
    );
    assert_eq!(
        std::fs::read(workspace.join("chunks/chunks.json")).unwrap(),
        queue
    );
    assert_eq!(
        std::fs::read(workspace.join("chunks/scenes.json")).unwrap(),
        scenes
    );
    for (path, bytes, modified, _) in &old {
        assert_eq!(std::fs::read(path).unwrap(), *bytes);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            *modified
        );
    }
    manager.cancel_command_edit(&held.id).await.unwrap();
    let canceled = terminal(&manager, &job.id).await;
    assert_eq!(
        canceled.encode_settings,
        Some(req.settings.clone()),
        "{canceled:#?}"
    );
    let restored = read(&workspace.join("manifest.json"));
    assert_eq!(restored["settings"], before["settings"]);
    assert_eq!(restored["queue_recipe"], before["queue_recipe"]);
    for (path, bytes, modified, _) in &old {
        assert_eq!(std::fs::read(path).unwrap(), *bytes);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            *modified
        );
    }
    let held = review(
        &manager,
        CommandEditRequest::ResumeNew {
            id: job.id.clone(),
            settings: Box::new(settings),
        },
    )
    .await;
    confirm(&manager, held, None).await;
    let done = terminal(&manager, &job.id).await;
    manager.shutdown().await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    let mut a = args(&[
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_frames",
        "-of",
        "json",
    ]);
    a.push(output.clone().into());
    let frames: Value = serde_json::from_slice(&run("ffprobe", a)).unwrap();
    let records = frames["frames"].as_array().unwrap();
    assert_eq!(records.len(), 120);
    for (index, frame) in records.iter().enumerate() {
        let shape = if (index as u64) < old_frames {
            (96, 64)
        } else {
            (48, 32)
        };
        assert_eq!(
            (
                frame["width"].as_u64().unwrap(),
                frame["height"].as_u64().unwrap()
            ),
            shape
        );
        let pts = frame["best_effort_timestamp_time"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap();
        assert!((pts - index as f64 / 30.0).abs() < 0.0011);
    }
    let decode = |path: &Path, seek: Option<f64>, filter: Option<String>, limit: Option<u64>| {
        let mut a = args(&["-v", "error"]);
        if let Some(seek) = seek {
            a.extend(["-ss".into(), format!("{seek:.9}").into()]);
        }
        a.push("-i".into());
        a.push(path.into());
        if let Some(filter) = filter {
            a.extend(["-vf".into(), filter.into()]);
        }
        if let Some(limit) = limit {
            a.extend(["-frames:v".into(), limit.to_string().into()]);
        }
        a.extend(args(&[
            "-pix_fmt",
            "yuv420p",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-",
        ]));
        run("ffmpeg", a)
    };
    assert!(
        decode(&output, None, None, Some(old_frames))
            == decode(&input, None, None, Some(old_frames))
    );
    assert!(
        decode(
            &output,
            Some(old_frames as f64 / 30.0),
            Some("scale=48:32".into()),
            None
        ) == decode(
            &input,
            None,
            Some(format!("trim=start_frame={old_frames},hflip,scale=48:32")),
            None
        )
    );
    let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/migration-validation/source-av1an-trim-20261101");
    std::fs::write(proof.join("native-source-new.json"),serde_json::to_vec_pretty(&json!({"before":before,"heldRestored":restored,"oldFrames":old_frames,"final":probe(&output),"finalFrames":frames})).unwrap()).unwrap();
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9, x264 and AV1AN"]
async fn source_new_cut_slot_retains_queue_on_shorter_failure_and_restores_review_bytes() {
    use std::{ffi::OsString, os::unix::fs::PermissionsExt};
    if let Some(root) = std::env::var_os("JESSES_SOURCE_QUEUE_REVIEW_CHILD") {
        let root = PathBuf::from(root);
        let manager = JobManager::open(root.join("logs"), root.join("history")).await;
        let saved = manager
            .list_jobs()
            .await
            .into_iter()
            .find(|job| job.recovery.is_some())
            .unwrap();
        let settings: EncodeSettings =
            serde_json::from_slice(&std::fs::read(root.join("child-settings.json")).unwrap())
                .unwrap();
        let _held = review(
            &manager,
            CommandEditRequest::ResumeNew {
                id: saved.id,
                settings: Box::new(settings),
            },
        )
        .await;
        std::fs::write(root.join("child-review-ready"), b"ready").unwrap();
        std::future::pending::<()>().await;
        return;
    }
    let _env = SOURCE_ENCODER_ENV.lock().await;
    struct Restore(Option<OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                if let Some(value) = &self.0 {
                    std::env::set_var("JESSES_X264", value);
                } else {
                    std::env::remove_var("JESSES_X264");
                }
            }
        }
    }
    let f = Fixture::new();
    let encoder = f.0.join("encoder");
    std::fs::create_dir(&encoder).unwrap();
    let wrapper = encoder.join("x264");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\nsleep 0.3\nexec /usr/bin/x264 \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _restore = Restore(std::env::var_os("JESSES_X264"));
    unsafe {
        std::env::set_var("JESSES_X264", &wrapper);
    }
    let input = f.0.join("original-eight-seconds.mkv");
    let output = f.0.join("changed-cut.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=30:d=8",
        "-c:v",
        "ffv1",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(input.clone().into());
    run("ffmpeg", a);
    let original = std::fs::read(&input).unwrap();
    let mut req = source_request(&input, &output);
    req.settings.source_trim = true;
    req.settings.trim =
        Some(serde_json::from_value(json!({"startFrame":0,"endFrameExclusive":120})).unwrap());
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let job = manager.start_encode(req.clone()).await.unwrap();
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let current = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|current| current.id == job.id)
                .unwrap();
            assert!(!current.state.is_terminal(), "{current:#?}");
            if current
                .recovery
                .as_ref()
                .is_some_and(|state| state.completed_frames > 0 && state.completed_frames < 120)
            {
                break current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    manager.stop_job(job.id.clone()).await.unwrap();
    assert_eq!(terminal(&manager, &job.id).await.state, JobState::Stopped);
    let workspace = PathBuf::from(&stopped.recovery.as_ref().unwrap().workspace);
    let before = read(&workspace.join("manifest.json"));
    let slot = PathBuf::from(before["source"]["path"].as_str().unwrap());
    assert!(before["source_cut_slot"].as_bool().unwrap());
    assert!(slot.starts_with(&workspace));
    let queue = std::fs::read(workspace.join("chunks/chunks.json")).unwrap();
    let old = before["completed"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, chunk)| {
            let path = workspace.join("chunks/encode").join(format!("{name}.264"));
            (
                path.clone(),
                std::fs::read(&path).unwrap(),
                std::fs::metadata(&path).unwrap().modified().unwrap(),
                chunk["frames"].as_u64().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let old_frames = old.iter().map(|chunk| chunk.3).sum::<u64>();
    let mut shorter = req.settings.clone();
    shorter.trim =
        Some(serde_json::from_value(json!({"startFrame":60,"endFrameExclusive":120})).unwrap());
    shorter.crf = 39;
    shorter
        .encoding_preferences
        .as_mut()
        .unwrap()
        .keyframe_interval_seconds = 2;
    let held = review(
        &manager,
        CommandEditRequest::ResumeNew {
            id: job.id.clone(),
            settings: Box::new(shorter.clone()),
        },
    )
    .await;
    assert_eq!(
        std::fs::read(workspace.join("chunks/chunks.json")).unwrap(),
        queue
    );
    assert_eq!(packets(&probe(&slot), 0).len(), 60);
    confirm(&manager, held, None).await;
    let failed = terminal(&manager, &job.id).await;
    assert_eq!(failed.state, JobState::Failed, "{failed:#?}");
    assert!(!output.exists());
    let source_failure = if old_frames >= 60 {
        "Output chunk file"
    } else {
        "FRAME MISMATCH"
    };
    assert!(
        failed.logs.iter().any(|line| line.contains(source_failure)),
        "saved next ordinal {old_frames}: {failed:#?}"
    );
    assert_eq!(
        std::fs::read(workspace.join("chunks/chunks.json")).unwrap(),
        queue
    );
    for (path, bytes, mtime, _) in &old {
        assert_eq!(std::fs::read(path).unwrap(), *bytes);
        assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), *mtime);
    }
    let failed_manifest = read(&workspace.join("manifest.json"));
    let short_bytes = std::fs::read(&slot).unwrap();
    let mut longer = shorter.clone();
    longer.trim =
        Some(serde_json::from_value(json!({"startFrame":60,"endFrameExclusive":180})).unwrap());
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while manager.list_av1an_recoveries().await.unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let saved_slot = f.0.join("owned-slot-backup.mkv");
    std::fs::rename(&slot, &saved_slot).unwrap();
    std::fs::write(&slot, &short_bytes).unwrap();
    let error = manager
        .av1an_recovery_action(Av1anRecoveryAction::ResumeNew {
            id: job.id.clone(),
            settings: Box::new(longer.clone()),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "RECOVERY_INVALID");
    assert!(!output.exists());
    std::fs::remove_file(&slot).unwrap();
    std::fs::rename(saved_slot, &slot).unwrap();
    let held = review(
        &manager,
        CommandEditRequest::ResumeNew {
            id: job.id.clone(),
            settings: Box::new(longer.clone()),
        },
    )
    .await;
    assert_eq!(packets(&probe(&slot), 0).len(), 120);
    manager.cancel_command_edit(&held.id).await.unwrap();
    let restored_job = terminal(&manager, &job.id).await;
    assert_eq!(restored_job.encode_settings, Some(shorter.clone()));
    assert_eq!(std::fs::read(&slot).unwrap(), short_bytes);
    let restored = read(&workspace.join("manifest.json"));
    assert_eq!(restored["source"], failed_manifest["source"]);
    assert_eq!(restored["queue_recipe"], before["queue_recipe"]);
    assert!(!restored["restart_pending"].as_bool().unwrap_or(false));
    manager.shutdown().await;
    drop(manager);
    std::fs::write(
        f.0.join("child-settings.json"),
        serde_json::to_vec(&longer).unwrap(),
    )
    .unwrap();
    let child_log = std::fs::File::create(f.0.join("child-review.log")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "source_new_cut_slot_retains_queue_on_shorter_failure_and_restores_review_bytes",
            "--ignored",
            "--nocapture",
        ])
        .env("JESSES_SOURCE_QUEUE_REVIEW_CHILD", &f.0)
        .stdout(child_log.try_clone().unwrap())
        .stderr(child_log)
        .spawn()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while !f.0.join("child-review-ready").exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "{}",
                std::fs::read_to_string(f.0.join("child-review.log")).unwrap()
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(packets(&probe(&slot), 0).len(), 120);
    child.kill().unwrap();
    child.wait().unwrap();
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let reopened = manager
        .list_jobs()
        .await
        .into_iter()
        .find(|current| current.id == job.id)
        .unwrap();
    assert_eq!(reopened.encode_settings, Some(shorter));
    assert_eq!(std::fs::read(&slot).unwrap(), short_bytes);
    let interrupted = read(&workspace.join("manifest.json"));
    assert_eq!(interrupted["queue_recipe"], before["queue_recipe"]);
    assert!(!interrupted["restart_pending"].as_bool().unwrap_or(false));
    let held = review(
        &manager,
        CommandEditRequest::ResumeNew {
            id: job.id.clone(),
            settings: Box::new(longer),
        },
    )
    .await;
    confirm(&manager, held, None).await;
    let done = terminal(&manager, &job.id).await;
    manager.shutdown().await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    assert_eq!(std::fs::read(&input).unwrap(), original);
    let decode = |file: &Path, seek: Option<f64>, filter: Option<String>, limit: Option<u64>| {
        let mut a = args(&["-v", "error"]);
        if let Some(seek) = seek {
            a.extend(["-ss".into(), format!("{seek:.9}").into()]);
        }
        a.push("-i".into());
        a.push(file.into());
        if let Some(filter) = filter {
            a.extend(["-vf".into(), filter.into()]);
        }
        if let Some(limit) = limit {
            a.extend(["-frames:v".into(), limit.to_string().into()]);
        }
        a.extend(args(&[
            "-pix_fmt",
            "yuv420p",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-",
        ]));
        run("ffmpeg", a)
    };
    assert!(
        decode(&output, None, None, Some(old_frames))
            == decode(&input, None, None, Some(old_frames))
    );
    let actual = decode(&output, Some(old_frames as f64 / 30.0), None, None);
    let expected = decode(
        &input,
        None,
        Some(format!(
            "trim=start_frame={}:end_frame=180",
            60 + old_frames
        )),
        None,
    );
    assert!(
        actual == expected,
        "unfinished saved ordinals must read the new cut; lengths {}/{}",
        actual.len(),
        expected.len()
    );
    assert_eq!(packets(&probe(&output), 0).len(), 120);
    assert!(!workspace.exists());
    assert!(!std::fs::read_dir(&f.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".source-cut-")
    }));
    let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/migration-validation/source-av1an-trim-20261101");
    std::fs::write(proof.join("native-source-new-cut.json"),serde_json::to_vec_pretty(&json!({"before":before,"shorterFailure":failed_manifest,"restored":restored,"interruptedRestored":interrupted,"oldFrames":old_frames,"final":probe(&output)})).unwrap()).unwrap();
}

#[cfg(unix)]
#[test]
#[ignore = "requires qualified CPU FFmpeg9, x264 and AV1AN"]
fn fork_engine_new_argv_retains_saved_queue_params_and_completed_bytes() {
    use std::{
        os::unix::{fs::PermissionsExt, process::CommandExt},
        process::Command,
        time::{Duration, Instant},
    };
    for case in ["new-filters", "trim-same-frames", "trim-shorter"] {
        let f = Fixture::new();
        let input = f.0.join("engine-source.mkv");
        let full = f.0.join("uncut-source.mkv");
        let mut a = args(&[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=96x64:r=30:d=8",
            "-c:v",
            "ffv1",
        ]);
        a.push(full.clone().into());
        run("ffmpeg", a);
        let mut a = args(&["-v", "error", "-i"]);
        a.push(full.clone().into());
        a.extend(args(&["-t", "4", "-map", "0", "-c", "copy"]));
        a.push(input.clone().into());
        run("ffmpeg", a);
        let wrapper = f.0.join("encoder");
        std::fs::create_dir(&wrapper).unwrap();
        let x264 = wrapper.join("x264");
        // Delay invocation only, keeping the real encoder bytes/arguments/pixels.
        // One worker then leaves a deterministic cancellation window after chunk0.
        std::fs::write(&x264, "#!/bin/sh\nsleep 0.3\nexec /usr/bin/x264 \"$@\"\n").unwrap();
        std::fs::set_permissions(&x264, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![wrapper];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let path = std::env::join_paths(paths).unwrap();
        let work = f.0.join("engine-work");
        let output = f.0.join("engine-output.mkv");
        let mut original = args(&[
            "-y",
            "--keep",
            "--verbose",
            "--encoder",
            "x264",
            "--passes",
            "1",
            "--no-defaults",
            "--concat",
            "ffmpeg",
            "--chunk-method",
            "select",
            "--split-method",
            "none",
            "-x",
            "30",
            "--workers",
            "1",
            "--pix-format",
            "yuv420p",
            "--video-params",
            "--preset ultrafast --crf 0 --threads 1 --fps 30",
            "--audio-params",
            "-an -sn -dn",
            "--temp",
        ]);
        original.push(work.clone().into());
        original.push("-i".into());
        original.push(input.clone().into());
        original.push("-o".into());
        original.push(output.clone().into());
        let log = std::fs::File::create(f.0.join("engine-first.log")).unwrap();
        let mut child = Command::new(tool("av1an"))
            .args(&original)
            .env("PATH", &path)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .process_group(0)
            .spawn()
            .unwrap();
        let begun = Instant::now();
        let done_path = work.join("done.json");
        loop {
            let completed = std::fs::read(&done_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|done| done["done"].as_object().map(|done| done.len()))
                .unwrap_or(0);
            if completed > 0 {
                assert!(completed < 4, "cancellation must leave unfinished chunks");
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "engine exited before first chunk; {}",
                std::fs::read_to_string(f.0.join("engine-first.log")).unwrap()
            );
            assert!(begun.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(2));
        }
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGTERM);
        }
        child.wait().unwrap();
        let scene_bytes = std::fs::read(work.join("scenes.json")).unwrap();
        let queue_bytes = std::fs::read(work.join("chunks.json")).unwrap();
        let before = read(&done_path);
        let finished = before["done"]
            .as_object()
            .unwrap()
            .keys()
            .map(|name| {
                let path = work.join("encode").join(format!("{name}.264"));
                (
                    path.clone(),
                    std::fs::read(&path).unwrap(),
                    std::fs::metadata(path).unwrap().modified().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let mut new = original.clone();
        new.push("-r".into());
        for (flag, value) in [
            ("-x", "60"),
            (
                "--video-params",
                "--preset ultrafast --crf 39 --threads 1 --fps 30",
            ),
        ] {
            let index = new.iter().position(|value| value == flag).unwrap();
            new[index + 1] = value.into();
        }
        if case == "new-filters" {
            new.extend(args(&["--ffmpeg", "-vf hflip,scale=48:32"]));
        } else {
            let cut = f.0.join("replacement-cut.mkv");
            let mut a = args(&["-v", "error", "-ss", "2", "-i"]);
            a.push(full.clone().into());
            a.extend(args(&[
                "-t",
                if case == "trim-shorter" { "2" } else { "4" },
                "-map",
                "0",
                "-c",
                "copy",
                "-avoid_negative_ts",
                "make_zero",
            ]));
            a.push(cut.clone().into());
            run("ffmpeg", a);
            std::fs::rename(cut, &input).unwrap();
        }
        let log = std::fs::File::create(f.0.join("engine-resumed.log")).unwrap();
        let status = Command::new(tool("av1an"))
            .args(&new)
            .env("PATH", &path)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .status()
            .unwrap();
        let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/migration-validation/source-av1an-trim-20261101");
        let resumed_log = std::fs::read_to_string(f.0.join("engine-resumed.log")).unwrap();
        std::fs::copy(
            f.0.join("engine-first.log"),
            proof.join(format!("source-engine-{case}-first.log")),
        )
        .unwrap();
        std::fs::copy(
            f.0.join("engine-resumed.log"),
            proof.join(format!("source-engine-{case}-resumed.log")),
        )
        .unwrap();
        if case == "trim-shorter" {
            assert!(
                !status.success(),
                "shorter replacement must fail the saved queue's frame count"
            );
            assert!(resumed_log.contains("FRAME MISMATCH"), "{resumed_log}");
            assert!(!output.exists());
            assert_eq!(
                std::fs::read(work.join("scenes.json")).unwrap(),
                scene_bytes
            );
            assert_eq!(
                std::fs::read(work.join("chunks.json")).unwrap(),
                queue_bytes
            );
            for (path, bytes, modified) in &finished {
                assert_eq!(std::fs::read(path).unwrap(), *bytes);
                assert_eq!(
                    std::fs::metadata(path).unwrap().modified().unwrap(),
                    *modified
                );
            }
            std::fs::write(proof.join("source-engine-trim-shorter.json"),serde_json::to_vec_pretty(&json!({"originalArguments":original,"newArguments":new,"before":before,"after":read(&done_path),"savedScenes":serde_json::from_slice::<Value>(&scene_bytes).unwrap(),"savedQueue":serde_json::from_slice::<Value>(&queue_bytes).unwrap(),"failure":resumed_log})).unwrap()).unwrap();
            continue;
        }
        assert!(status.success(), "{resumed_log}");
        assert_eq!(
            std::fs::read(work.join("scenes.json")).unwrap(),
            scene_bytes
        );
        assert_eq!(
            std::fs::read(work.join("chunks.json")).unwrap(),
            queue_bytes
        );
        for (path, bytes, modified) in finished {
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(
                std::fs::metadata(path).unwrap().modified().unwrap(),
                modified
            );
        }
        let mut frame_args = args(&[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_frames",
            "-of",
            "json",
        ]);
        frame_args.push(output.clone().into());
        let frames: Value = serde_json::from_slice(&run("ffprobe", frame_args)).unwrap();
        let completed_frames = before["done"]
            .as_object()
            .unwrap()
            .values()
            .map(|chunk| chunk["frames"].as_u64().unwrap())
            .sum::<u64>();
        let actual_frames = frames["frames"].as_array().unwrap();
        assert_eq!(actual_frames.len(), 120);
        for (index, frame) in actual_frames.iter().enumerate() {
            let expected = if case != "new-filters" || (index as u64) < completed_frames {
                (96, 64)
            } else {
                (48, 32)
            };
            assert_eq!(
                (
                    frame["width"].as_u64().unwrap(),
                    frame["height"].as_u64().unwrap()
                ),
                expected
            );
            let pts = frame["best_effort_timestamp_time"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap();
            assert!(
                pts.is_finite() && (pts - index as f64 / 30.0).abs() < 0.0011,
                "source final frame clock {case} frame{index}: {pts}"
            );
        }
        let decode =
            |file: &Path, seek: Option<f64>, filter: Option<String>, limit: Option<u64>| {
                let mut a = args(&["-v", "error"]);
                if let Some(seek) = seek {
                    a.extend(["-ss".into(), format!("{seek:.9}").into()]);
                }
                a.push("-i".into());
                a.push(file.into());
                if let Some(filter) = filter {
                    a.extend(["-vf".into(), filter.into()]);
                }
                if let Some(limit) = limit {
                    a.extend(["-frames:v".into(), limit.to_string().into()]);
                }
                a.extend(args(&[
                    "-pix_fmt",
                    "yuv420p",
                    "-fps_mode",
                    "passthrough",
                    "-f",
                    "rawvideo",
                    "-",
                ]));
                run("ffmpeg", a)
            };
        assert!(
            decode(&output, None, None, Some(completed_frames))
                == decode(&full, None, None, Some(completed_frames)),
            "completed frames must retain original pixels"
        );
        let actual_remaining = decode(
            &output,
            Some(completed_frames as f64 / 30.0),
            (case == "new-filters").then(|| "scale=48:32".into()),
            None,
        );
        let expected_filter = if case == "new-filters" {
            format!("trim=start_frame={completed_frames},hflip,scale=48:32")
        } else {
            format!("trim=start_frame={completed_frames}")
        };
        let expected_remaining = decode(&input, None, Some(expected_filter), None);
        assert!(
            actual_remaining == expected_remaining,
            "new filters apply only to remaining chunks, with old lossless CRF0 parameters; byte lengths {}/{}",
            actual_remaining.len(),
            expected_remaining.len()
        );
        std::fs::write(proof.join(format!("source-engine-{case}.json")),serde_json::to_vec_pretty(&json!({"originalArguments":original,"newArguments":new,"before":before,"after":read(&done_path),"savedScenes":serde_json::from_slice::<Value>(&scene_bytes).unwrap(),"savedQueue":serde_json::from_slice::<Value>(&queue_bytes).unwrap(),"final":probe(&output),"finalFrames":frames})).unwrap()).unwrap();
    }
}

#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9, x264 and AV1AN"]
async fn source_none_retains_keyframe_interval_chunks_and_saved_queue() {
    let f = Fixture::new();
    let input = f.0.join("scene-change-30fps.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=30:d=4",
        "-vf",
        "drawbox=enable='gte(t,1.5)':x=0:y=0:w=iw:h=ih:c=blue:t=fill",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "0",
        "-g",
        "60",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(input.clone().into());
    run("ffmpeg", a);
    let output = f.0.join("none.mkv");
    let req = source_request(&input, &output);
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let held = review(
        &manager,
        CommandEditRequest::Encode {
            request: Box::new(req.clone()),
        },
    )
    .await;
    let argv_text = held.arguments.replace('\'', "");
    assert!(
        argv_text.contains("--split-method none"),
        "{}",
        held.arguments
    );
    assert!(argv_text.contains("--extra-split 30"));
    for flag in ["--sc-method", "--min-scene-len", "--sc-downscale-height"] {
        assert!(!held.arguments.contains(flag));
    }
    assert!(
        held.job
            .as_ref()
            .unwrap()
            .encode_settings
            .as_ref()
            .unwrap()
            .av1an_options
            .unwrap()
            .source_workflow
    );
    let id = held.job.as_ref().unwrap().id.clone();
    let workspace = PathBuf::from(
        &held
            .job
            .as_ref()
            .unwrap()
            .recovery
            .as_ref()
            .unwrap()
            .workspace,
    );
    let marker = f.0.join("engine-complete");
    let hold = f.0.join("hold");
    std::fs::write(&hold, b"hold").unwrap();
    let (engine, mux) = held.arguments.rsplit_once(" &&\n").unwrap();
    let edited = format!(
        "{engine} && touch {} && while [ -e {} ]; do sleep 0.02; done && {mux}",
        quoted(&marker),
        quoted(&hold)
    );
    confirm(&manager, held, Some(edited)).await;
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            assert!(!job.state.is_terminal(), "{job:#?}");
            if marker.exists()
                && job
                    .recovery
                    .as_ref()
                    .is_some_and(|state| state.completed_frames == 120)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    manager.stop_job(id.clone()).await.unwrap();
    assert_eq!(terminal(&manager, &id).await.state, JobState::Stopped);
    let scenes = read(&workspace.join("chunks/scenes.json"));
    let queue = std::fs::read(workspace.join("chunks/chunks.json")).unwrap();
    assert_eq!(
        split_ranges(&scenes),
        vec![(0, 24), (24, 48), (48, 72), (72, 96), (96, 120)]
    );
    let queue_json: Value = serde_json::from_slice(&queue).unwrap();
    assert_eq!(queue_json.as_array().unwrap().len(), 5);
    let reference = f.0.join("source-scenes.json");
    let mut source_args = args(&[
        "-y",
        "--sc-only",
        "--split-method",
        "none",
        "-x",
        "30",
        "-m",
        "select",
        "--encoder",
        "x264",
        "--scenes",
    ]);
    source_args.push(reference.clone().into());
    source_args.extend(args(&["--temp"]));
    source_args.push(f.0.join("reference-temp").into());
    source_args.push("-i".into());
    source_args.push(input.clone().into());
    source_args.push("-o".into());
    source_args.push(f.0.join("unused.mkv").into());
    run("av1an", source_args.clone());
    assert_eq!(split_ranges(&scenes), split_ranges(&read(&reference)));
    let saved = read(&workspace.join("manifest.json"));
    let saved_review = review(&manager, CommandEditRequest::ResumeSaved { id: id.clone() }).await;
    assert!(saved_review.arguments.contains("--resume"));
    assert!(
        saved_review
            .arguments
            .replace('\'', "")
            .contains("--extra-split 30")
    );
    assert_eq!(
        std::fs::read(workspace.join("chunks/chunks.json")).unwrap(),
        queue
    );
    std::fs::remove_file(hold).unwrap();
    confirm(&manager, saved_review, None).await;
    let done = terminal(&manager, &id).await;
    manager.shutdown().await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    let actual = probe(&output);
    assert_eq!(packets(&actual, 0).len(), 120);
    let mut decode = args(&["-v", "error", "-i"]);
    decode.push(input.clone().into());
    decode.extend(args(&["-pix_fmt", "yuv420p", "-f", "rawvideo", "-"]));
    let expected = run("ffmpeg", decode);
    let mut decode = args(&["-v", "error", "-i"]);
    decode.push(output.into());
    decode.extend(args(&["-pix_fmt", "yuv420p", "-f", "rawvideo", "-"]));
    assert!(
        run("ffmpeg", decode) == expected,
        "source None output must retain every original frame pixel"
    );
    let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/migration-validation/source-av1an-trim-20261101");
    std::fs::write(proof.join("source-none-chunks.json"),serde_json::to_vec_pretty(&json!({"sourceEquivalentArguments":source_args,"scenes":scenes,"referenceScenes":read(&reference),"queue":queue_json,"savedManifest":saved,"finalOutput":actual})).unwrap()).unwrap();
}
