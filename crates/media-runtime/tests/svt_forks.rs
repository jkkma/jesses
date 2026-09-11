//! Opt-in native qualification using scripts/svt-forks.json's pinned builds.
use media_runtime::{
    EncodeBackend, EncodeRequest, EncodeSettings, HdrTune, JobManager, JobSnapshot, JobState,
    RemuxRequest, VideoEncoder, get_capabilities,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("jesses-forks-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Failed fork fixture retained at {}", self.0.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn executable(id: &str) -> PathBuf {
    let tool = get_capabilities()
        .await
        .into_iter()
        .find(|tool| tool.id == id)
        .unwrap();
    assert!(tool.available, "{tool:?}");
    tool.path.unwrap().into()
}
async fn tool(id: &str, args: Vec<std::ffi::OsString>) -> Vec<u8> {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(
        &CommandSpec {
            executable: executable(id).await,
            args,
            cwd: None,
        },
        cancel,
        4 * 1024 * 1024,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
async fn synthesize_video(path: &Path, hdr: bool, size: &str, frames: u32) {
    let filter = format!("testsrc2=size={size}:rate=24000/1001");
    let frame_count = frames.to_string();
    let duration = (f64::from(frames) * 1001.0 / 24000.0).to_string();
    let mut args: Vec<std::ffi::OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        &filter,
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000",
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-frames:v",
        &frame_count,
        "-t",
        &duration,
        "-c:a",
        "pcm_s16le",
        "-metadata:s:a:0",
        "language=jpn",
        "-metadata:s:a:0",
        "title=Preserved audio",
        "-pix_fmt",
        "yuv420p10le",
        "-color_range",
        "tv",
        "-chroma_sample_location",
        "left",
        "-preset",
        "ultrafast",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let color = if hdr {
        ["bt2020", "smpte2084", "bt2020nc"]
    } else {
        ["bt709", "bt709", "bt709"]
    };
    args.extend([
        "-vf".into(),
        format!("format=yuv420p10le,setparams=field_mode=prog:range=limited:color_primaries={}:color_trc={}:colorspace={}", color[0], color[1], color[2]).into(),
    ]);
    args.extend(
        [
            "-color_primaries",
            color[0],
            "-color_trc",
            color[1],
            "-colorspace",
            color[2],
        ]
        .into_iter()
        .map(Into::into),
    );
    if hdr {
        args.extend(["-c:v", "libx265", "-x265-params", "log-level=error:pools=none:frame-threads=1:bframes=0:hdr10=1:chromaloc=0:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=200,142"].into_iter().map(Into::into));
    } else {
        args.extend(
            ["-c:v", "libx264", "-crf", "0", "-bf", "0"]
                .into_iter()
                .map(Into::into),
        );
    }
    args.push(path.into());
    tool("ffmpeg", args).await;
}
async fn finished(manager: &JobManager, id: &str) -> JobSnapshot {
    wait_for(manager, id, |job| job.state.is_terminal()).await
}
async fn wait_for(
    manager: &JobManager,
    id: &str,
    mut ready: impl FnMut(&JobSnapshot) -> bool,
) -> JobSnapshot {
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if ready(&snapshot) || snapshot.state.is_terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    if result.is_err() {
        manager.shutdown().await;
    }
    result.expect("fork job completes in two minutes")
}
async fn qualify(backend: EncodeBackend) {
    let fixture = Fixture::new();
    // Exceed av1an's 240-frame extra split to exercise actual concatenation.
    let frames = if backend == EncodeBackend::Av1an {
        264
    } else {
        24
    };
    let history = fixture.0.join("history");
    let manager = JobManager::open(fixture.0.join("logs"), history.clone()).await;
    let mut expected = Vec::new();
    for (encoder, id, fingerprint, hdr) in [
        (
            VideoEncoder::SvtAv1FiveFish,
            "svt-av1-5fish",
            "[5fish]",
            false,
        ),
        (VideoEncoder::SvtAv1Hdr, "svt-av1-hdr", "SVT-AV1-HDR", true),
    ] {
        executable(id).await;
        let input = fixture.0.join(format!("{id} source 日本語.mkv"));
        let destination = fixture.0.join(format!("{id} output 日本語.mkv"));
        synthesize_video(&input, hdr, "128x96", frames).await;
        let source_bytes = std::fs::read(&input).unwrap();
        let settings = EncodeSettings {
            backend,
            encoder,
            workers: if backend == EncodeBackend::Av1an {
                2
            } else {
                1
            },
            crf: if hdr { 30 } else { 18 },
            preset: 6,
            lineart_psy_bias: if hdr { 0 } else { 5 },
            texture_psy_bias: if hdr { 0 } else { 4 },
            hdr_tune: if hdr {
                HdrTune::FilmGrain
            } else {
                HdrTune::VisualQuality
            },
            ..Default::default()
        };
        let request = EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: destination.to_string_lossy().into_owned(),
                stream_indices: vec![1, 0],
            },
            settings: settings.clone(),
        };
        let started = manager.start_encode(request.clone()).await.unwrap();
        // The UI snapshot deliberately retains only a bounded log tail. Observe
        // early version/configuration lines before indexing progress evicts them.
        let mut observed_logs = std::collections::BTreeSet::new();
        let job = wait_for(&manager, &started.id, |job| {
            observed_logs.extend(job.logs.iter().cloned());
            job.state.is_terminal()
        })
        .await;
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        assert_eq!(job.encode_settings.as_ref(), Some(&settings));
        assert!(
            observed_logs.iter().any(|line| line.contains(fingerprint)),
            "Missing exact fork identity: {job:#?}"
        );
        assert!(
            observed_logs.iter().all(|line| !line.contains('\u{1b}')),
            "Raw ANSI escaped into history"
        );
        if backend == EncodeBackend::Av1an {
            assert!(
                observed_logs.iter().any(|line| line
                    .split_once("Queue ")
                    .and_then(|(_, rest)| rest.split_whitespace().next())
                    .and_then(|count| count.parse::<usize>().ok())
                    .is_some_and(|count| count >= 2)),
                "Qualification must concatenate multiple encoded chunks: {job:#?}"
            );
        }
        assert_eq!(std::fs::read(&input).unwrap(), source_bytes);
        let mut args: Vec<std::ffi::OsString> = [
            "-v",
            "error",
            "-count_frames",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.push(destination.into());
        let output: Value = serde_json::from_slice(&tool("ffprobe", args).await).unwrap();
        let streams = output["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0]["codec_name"], "pcm_s16le");
        assert_eq!(streams[0]["tags"]["language"], "jpn");
        assert_eq!(streams[0]["tags"]["title"], "Preserved audio");
        let video = &streams[1];
        assert_eq!(video["codec_name"], "av1");
        assert_eq!(video["pix_fmt"], "yuv420p10le");
        assert_eq!(video["nb_read_frames"], frames.to_string());
        assert_eq!(video["r_frame_rate"], "24000/1001");
        assert_eq!(video["color_range"], "tv");
        assert_eq!(
            video["color_transfer"],
            if hdr { "smpte2084" } else { "bt709" }
        );
        assert_eq!(
            video["color_primaries"],
            if hdr { "bt2020" } else { "bt709" }
        );
        if hdr {
            assert!(
                observed_logs
                    .iter()
                    .any(|line| line.contains("film grain retention (5)"))
            );
        }
        expected.push((started.id, settings));
    }
    manager.shutdown().await;
    drop(manager);
    let restored = JobManager::open(fixture.0.join("logs"), history).await;
    let saved = restored.list_jobs().await;
    for (id, settings) in expected {
        assert_eq!(
            saved
                .iter()
                .find(|job| job.id == id)
                .unwrap()
                .encode_settings
                .as_ref(),
            Some(&settings)
        );
    }
    restored.shutdown().await;
}
#[tokio::test]
#[ignore = "requires pinned 5fish and HDR executables, FFmpeg and FFprobe"]
async fn standalone_forks_preserve_frames_tracks_hdr_and_history() {
    qualify(EncodeBackend::Standalone).await;
}
#[tokio::test]
#[ignore = "requires pinned forks, FFmpeg, FFprobe, av1an, VapourSynth and L-SMASH"]
async fn av1an_uses_each_selected_fork_and_preserves_frames_tracks_hdr_and_history() {
    qualify(EncodeBackend::Av1an).await;
}

#[tokio::test]
#[ignore = "requires pinned 5fish, FFmpeg, FFprobe, av1an, VapourSynth and L-SMASH"]
async fn canceling_fork_av1an_reaps_workers_and_releases_the_staged_host() {
    executable("svt-av1-5fish").await;
    let fixture = Fixture::new();
    let input = fixture.0.join("cancel fork source 日本語.mkv");
    let destination = fixture.0.join("must not publish.mkv");
    synthesize_video(&input, false, "640x360", 480).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let settings = EncodeSettings {
        backend: EncodeBackend::Av1an,
        encoder: VideoEncoder::SvtAv1FiveFish,
        workers: 2,
        crf: 18,
        preset: 0,
        lineart_psy_bias: 5,
        texture_psy_bias: 4,
        ..Default::default()
    };
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: destination.to_string_lossy().into_owned(),
                stream_indices: vec![1, 0],
            },
            settings: settings.clone(),
        })
        .await
        .unwrap();
    let running = wait_for(&manager, &job.id, |job| {
        job.state == JobState::Running
            && job
                .logs
                .iter()
                .any(|line| line.contains("Queue ") && line.contains("Workers"))
    })
    .await;
    if running.state != JobState::Running {
        manager.shutdown().await;
        panic!("The fork worker queue must start before cancellation: {running:#?}");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    manager.cancel_job(job.id.clone()).await.unwrap();
    let canceled = finished(&manager, &job.id).await;
    tokio::time::timeout(Duration::from_secs(10), manager.shutdown())
        .await
        .expect("Cancellation must reap all av1an workers promptly");
    assert_eq!(canceled.state, JobState::Canceled, "{canceled:#?}");
    assert_eq!(canceled.encode_settings.as_ref(), Some(&settings));
    assert!(!destination.exists());
    assert_eq!(std::fs::read(&input).unwrap(), original);
    for entry in std::fs::read_dir(&fixture.0).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(
                !name.contains(".partial.") && !name.ends_with(".ivf"),
                "Unreleased owned output: {name}"
            );
        } else if entry.file_name().to_string_lossy().ends_with(".av1an") {
            assert!(
                !entry.path().join("av1an.exe").exists(),
                "Staged host was retained after cancellation"
            );
        }
    }
    // This also proves that Windows descendants released every staged image,
    // chunk and source handle; interrupted diagnostics belong to this fixture.
    std::fs::remove_dir_all(&fixture.0).expect("Canceled workers must release all fixture handles");
}
