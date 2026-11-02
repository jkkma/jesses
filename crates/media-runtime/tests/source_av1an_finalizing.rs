//! Saved finalization must retain the sealed mixed-region source queue contract.
#![cfg(unix)]
use media_core::*;
use media_runtime::JobManager;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};
#[path = "common/source_trim_fixture.rs"]
mod fixture;
use fixture::*;

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}
fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
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
            }),
            av1an_options: Some(Av1anOptions {
                source_workflow: true,
                chunk_method: Av1anChunkMethod::Select,
                split_method: Av1anSplitMethod::FixedChunks,
                chunk_order: Av1anChunkOrder::Sequential,
                encoder_threads: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        },
    }
}
async fn wait_for(
    manager: &JobManager,
    id: &str,
    predicate: impl Fn(&JobSnapshot) -> bool,
) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if predicate(&job) {
                return job;
            }
            assert!(!job.state.is_terminal(), "{job:#?}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
fn decode(path: &Path, seek: Option<f64>, filter: Option<&str>, limit: Option<u64>) -> Vec<u8> {
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
}

#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9, x264 and AV1AN"]
async fn saved_finalizing_source_queue_restores_mixed_regions_without_reencoding() {
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
    let calls = f.0.join("encoded-chunks.log");
    let wrapper = f.0.join("x264");
    std::fs::write(&wrapper, format!("#!/bin/sh\nfor value in \"$@\"; do\n case \"$value\" in */chunks/encode/*.264) printf 'chunk\\n' >> {}; sleep 0.3;; esac\ndone\nexec /usr/bin/x264 \"$@\"\n", quoted(&calls))).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _restore = Restore(std::env::var_os("JESSES_X264"));
    unsafe {
        std::env::set_var("JESSES_X264", &wrapper);
    }
    let input = f.0.join("source.mkv");
    let output = f.0.join("mixed-finalized.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=30:d=4",
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
    let request = source_request(&input, &output);
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let job = manager.start_encode(request.clone()).await.unwrap();
    wait_for(&manager, &job.id, |job| {
        job.recovery
            .as_ref()
            .is_some_and(|state| state.completed_frames > 0 && state.completed_frames < 120)
    })
    .await;
    manager.stop_job(job.id.clone()).await.unwrap();
    let stopped = terminal(&manager, &job.id).await;
    assert_eq!(stopped.state, JobState::Stopped, "{stopped:#?}");
    let workspace = PathBuf::from(&stopped.recovery.as_ref().unwrap().workspace);
    let before = read(&workspace.join("manifest.json"));
    let old_frames = before["completed"]
        .as_object()
        .unwrap()
        .values()
        .map(|chunk| chunk["frames"].as_u64().unwrap())
        .sum::<u64>();
    let queue = std::fs::read(workspace.join("chunks/chunks.json")).unwrap();
    let scenes = std::fs::read(workspace.join("chunks/scenes.json")).unwrap();
    let attempts = stopped
        .logs
        .iter()
        .filter(|line| line.starts_with("av1an detail log:"))
        .count();
    let mut settings = request.settings;
    settings.source_filters = true;
    settings.av1an_filters = vec!["hflip,scale=48:32".into()];
    manager
        .av1an_recovery_action(Av1anRecoveryAction::ResumeNew {
            id: job.id.clone(),
            settings: Box::new(settings),
        })
        .await
        .unwrap();
    wait_for(&manager, &job.id, |job| {
        job.logs
            .iter()
            .filter(|line| line.starts_with("av1an detail log:"))
            .count()
            > attempts
    })
    .await;
    let sentinel = b"late destination must remain intact";
    std::fs::write(&output, sentinel).unwrap();
    let failed = terminal(&manager, &job.id).await;
    assert_eq!(failed.state, JobState::Failed, "{failed:#?}");
    assert_eq!(
        failed.error.as_ref().unwrap().code,
        "OUTPUT_EXISTS",
        "{failed:#?}"
    );
    assert_eq!(
        failed.recovery.as_ref().unwrap().phase,
        RecoveryPhase::Finalizing
    );
    assert_eq!(std::fs::read(&output).unwrap(), sentinel);
    let sealed = read(&workspace.join("manifest.json"));
    let chunks = sealed["completed"]
        .as_object()
        .unwrap()
        .values()
        .map(|chunk| {
            let path = PathBuf::from(chunk["file"]["path"].as_str().unwrap());
            (
                path.clone(),
                std::fs::read(&path).unwrap(),
                std::fs::metadata(&path).unwrap().modified().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let stage = PathBuf::from(sealed["final_video"]["path"].as_str().unwrap());
    let stage_packets = probe(&stage);
    let encoder_calls = std::fs::read(&calls).unwrap();
    manager.shutdown().await;
    drop(manager);
    std::fs::remove_file(&output).unwrap();
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    for (path, bytes, modified) in &chunks {
        assert_eq!(std::fs::read(path).unwrap(), *bytes);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            *modified
        );
    }
    assert_eq!(
        std::fs::read(workspace.join("chunks/chunks.json")).unwrap(),
        queue
    );
    assert_eq!(
        std::fs::read(workspace.join("chunks/scenes.json")).unwrap(),
        scenes
    );
    manager.resume_job(job.id.clone()).await.unwrap();
    let done = terminal(&manager, &job.id).await;
    manager.shutdown().await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    assert_eq!(
        std::fs::read(&calls).unwrap(),
        encoder_calls,
        "Saved finalization must not run another chunk encoder"
    );
    assert_eq!(std::fs::read(&input).unwrap(), original);
    let final_probe = probe(&output);
    let hashes = |document: &Value| {
        packets(document, 0)
            .iter()
            .map(|packet| packet["data_hash"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(hashes(&final_probe), hashes(&stage_packets));
    assert_eq!(packets(&final_probe, 0).len(), 120);
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
    assert_eq!(frames["frames"].as_array().unwrap().len(), 120);
    for (index, frame) in frames["frames"].as_array().unwrap().iter().enumerate() {
        let expected = if (index as u64) < old_frames {
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
    }
    assert!(
        decode(&output, None, None, Some(old_frames))
            == decode(&input, None, None, Some(old_frames)),
        "The preserved decoded region must match the original source pixels."
    );
    let remaining_filter = format!("trim=start_frame={old_frames},hflip,scale=48:32");
    assert!(
        decode(
            &output,
            Some(old_frames as f64 / 30.0),
            Some("scale=48:32"),
            None
        ) == decode(&input, None, Some(&remaining_filter), None),
        "The remaining decoded region must retain the new source filters."
    );
    assert!(!workspace.exists());
    let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/migration-validation/source-av1an-trim-20261101");
    std::fs::write(proof.join("native-source-finalizing-saved.json"), serde_json::to_vec_pretty(&json!({"oldFrames":old_frames,"finalizing":sealed,"final":final_probe,"frames":frames,"encoderCallsUnchanged":true})).unwrap()).unwrap();
}
