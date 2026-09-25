//! Opt-in av1an gate: `cargo test -p media-runtime --test av1an_jobs -- --ignored`.
//! Fixtures are small, locally generated videos; source media is never required.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_runtime::{
    EncodeBackend, EncodeRequest, EncodeSettings, JobManager, JobSnapshot, JobState, RemuxRequest,
    get_capabilities,
    supervisor::{CapturedOutput, CommandSpec, run_capture},
};
use serde_json::Value;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        if let Some(resources) = std::env::var_os("JESSES_TEST_TOOL_RESOURCES") {
            media_runtime::configure_bundled_tools(PathBuf::from(resources))
                .expect("the package gate uses one absolute verified resource root");
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("jesses-av1an-test-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // The test exclusively owns this fresh directory and every file in it.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn command(tool: &str) -> std::process::Command {
    let executable = get_capabilities()
        .await
        .into_iter()
        .find(|entry| entry.id == tool && entry.available)
        .and_then(|entry| entry.path)
        .unwrap_or_else(|| panic!("{tool} must resolve to an installed executable"));
    // This is only an argument builder. The supervisor owns every child.
    std::process::Command::new(executable)
}

async fn output(command: &mut std::process::Command) -> CapturedOutput {
    let spec = CommandSpec {
        executable: command.get_program().into(),
        args: command.get_args().map(Into::into).collect(),
        cwd: command.get_current_dir().map(Into::into),
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    run_capture(&spec, cancel, 4 * 1024 * 1024, Duration::from_secs(20))
        .await
        .expect("fixture tool completes within its capture and time limits")
}

async fn synthesize(path: &Path, size: &str, frames: u32) {
    let filter = format!(
        "testsrc2=size={size}:rate=24000/1001,negate=enable='between(t,1,2)',format=yuv420p10le,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709"
    );
    let output = output(
        command("ffmpeg")
            .await
            .args([
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
                &frames.to_string(),
                "-t",
                &(f64::from(frames) * 1001.0 / 24000.0).to_string(),
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "0",
                "-bf",
                "0",
                "-c:a",
                "pcm_s16le",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
                "-color_range",
                "tv",
                "-chroma_sample_location",
                "left",
                "-metadata:s:a:0",
                "language=jpn",
                "-metadata:s:a:0",
                "title=Test tone",
            ])
            .arg(path),
    )
    .await;
    assert!(
        output.status.success(),
        "Fixture generation: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn request(input: &Path, output: &Path, preset: u8) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            // Audio comes first to exercise the final mux's stream selection.
            stream_indices: vec![1, 0],
        },
        settings: EncodeSettings {
            backend: EncodeBackend::Av1an,
            workers: 2,
            video_stream_index: 0,
            crf: 32,
            preset,
            ..EncodeSettings::default()
        },
    }
}

async fn wait_for(
    manager: &JobManager,
    id: &str,
    ready: impl Fn(&JobSnapshot) -> bool,
) -> JobSnapshot {
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if ready(&job) || job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if result.is_err() {
        manager.shutdown().await;
    }
    result.expect("av1an job must progress within 90 seconds")
}

async fn probe(path: &Path) -> Value {
    let output = output(
        command("ffprobe")
            .await
            .args([
                "-v",
                "error",
                "-count_frames",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
            ])
            .arg(path),
    )
    .await;
    assert!(
        output.status.success(),
        "Output probe: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_no_partial_output(root: &Path) {
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(
                !name.contains(".partial.") && !name.ends_with(".ivf"),
                "Unreleased output: {name}"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, av1an, SVT-AV1, VapourSynth and L-SMASH on PATH"]
async fn av1an_preserves_frame_rate_color_audio_and_immutable_request() {
    let fixture = Fixture::new();
    let input = fixture.0.join("- scene's & $ % 日本語.mkv");
    let output = fixture.0.join("encoded scenes.mkv");
    synthesize(&input, "320x180", 96).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let request = request(&input, &output, 12);
    let submitted = manager.start_encode(request.clone()).await.unwrap();
    let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    if finished.state != JobState::Succeeded {
        manager.shutdown().await;
        panic!("av1an failed: {finished:#?}");
    }
    assert_eq!(finished.request, request.source);
    assert_eq!(finished.encode_settings.as_ref(), Some(&request.settings));
    assert_eq!(
        std::fs::read(&input).unwrap(),
        original,
        "Encoding changed its source"
    );

    let inspected = probe(&output).await;
    let streams = inspected["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0]["codec_type"], "audio");
    assert_eq!(streams[0]["codec_name"], "pcm_s16le");
    assert_eq!(streams[0]["tags"]["language"], "jpn");
    assert_eq!(streams[0]["tags"]["title"], "Test tone");
    let video = &streams[1];
    assert_eq!(video["codec_name"], "av1");
    assert_eq!(video["width"], 320);
    assert_eq!(video["height"], 180);
    assert_eq!(video["nb_read_frames"], "96");
    assert_eq!(video["r_frame_rate"], "24000/1001");
    assert_eq!(video["pix_fmt"], "yuv420p10le");
    assert_eq!(video["color_primaries"], "bt709");
    assert_eq!(video["color_transfer"], "bt709");
    assert_eq!(video["color_space"], "bt709");
    assert_eq!(video["color_range"], "tv");
    assert_eq!(video["chroma_location"], "left");
    let duration: f64 = inspected["format"]["duration"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (duration - 4.004).abs() < 0.06,
        "Incorrect concatenated duration: {duration}"
    );
    assert!(finished.logs.iter().any(|line| line.contains("av1an")));
    assert_no_partial_output(&fixture.0);

    // A second request may be rejected on submission or by execution preflight.
    let published = std::fs::read(&output).unwrap();
    match manager.start_encode(request).await {
        Ok(job) => {
            let conflict = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
            assert_eq!(conflict.state, JobState::Failed, "{conflict:#?}");
            assert_eq!(conflict.error.unwrap().code, "OUTPUT_EXISTS");
        }
        Err(error) => assert_eq!(error.code, "OUTPUT_EXISTS"),
    }
    manager.shutdown().await;
    assert_eq!(
        std::fs::read(&output).unwrap(),
        published,
        "Existing output was overwritten"
    );
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, av1an, x264, VapourSynth and L-SMASH on PATH"]
async fn av1an_x264_keeps_timing_color_audio_and_source() {
    let fixture = Fixture::new();
    let input = fixture.0.join("x264 source.mkv");
    let output = fixture.0.join("x264 av1an.mkv");
    synthesize(&input, "320x180", 96).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let mut request = request(&input, &output, 5);
    request.settings.encoder = media_core::VideoEncoder::X264;
    request.settings.crf = 23;
    request.settings.av1an_options = Some(media_core::Av1anOptions {
        split_method: media_core::Av1anSplitMethod::FixedChunks,
        maximum_chunk_frames: 48,
        ..Default::default()
    });
    let submitted = manager.start_encode(request).await.unwrap();
    let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    if finished.state != JobState::Succeeded {
        manager.shutdown().await;
        panic!("av1an x264 failed: {finished:#?}");
    }
    let inspected = probe(&output).await;
    let streams = inspected["streams"].as_array().unwrap();
    let video = streams
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    assert_eq!(video["codec_name"], "h264");
    assert_eq!(video["nb_read_frames"], "96");
    assert_eq!(video["r_frame_rate"], "24000/1001");
    assert_eq!(video["pix_fmt"], "yuv420p10le");
    assert_eq!(video["color_primaries"], "bt709");
    assert_eq!(video["color_transfer"], "bt709");
    assert_eq!(video["color_space"], "bt709");
    assert_eq!(streams[0]["codec_name"], "pcm_s16le");
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires a packaged av1an with segment-ffmpeg9-v1, FFmpeg, FFprobe, x264 and mkvmerge"]
async fn av1an_segment_reader_preserves_every_frame_and_its_source() {
    let fixture = Fixture::new();
    let input = fixture.0.join("segment source.mkv");
    let output = fixture.0.join("segment output.mkv");
    synthesize(&input, "320x180", 96).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let mut request = request(&input, &output, 5);
    request.settings.encoder = media_core::VideoEncoder::X264;
    request.settings.crf = 23;
    request.settings.av1an_options = Some(media_core::Av1anOptions {
        chunk_method: media_core::Av1anChunkMethod::Segment,
        split_method: media_core::Av1anSplitMethod::FixedChunks,
        maximum_chunk_frames: 24,
        ..Default::default()
    });

    let submitted = manager.start_encode(request).await.unwrap();
    let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    if finished.state != JobState::Succeeded {
        manager.shutdown().await;
        panic!("av1an Segment reader failed: {finished:#?}");
    }
    let inspected = probe(&output).await;
    let streams = inspected["streams"].as_array().unwrap();
    let video = streams
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    assert_eq!(video["codec_name"], "h264");
    assert_eq!(video["nb_read_frames"], "96");
    assert_eq!(video["r_frame_rate"], "24000/1001");
    assert_eq!(streams[0]["codec_name"], "pcm_s16le");
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, av1an, x264, mkvmerge, VapourSynth and L-SMASH on PATH"]
async fn av1an_x264_stops_with_verified_chunks_and_resumes_original_request() {
    let fixture = Fixture::new();
    let input = fixture.0.join("resume x264 source.mkv");
    let output = fixture.0.join("resume x264 output.mkv");
    synthesize(&input, "640x360", 288).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let mut request = request(&input, &output, 5);
    request.settings.encoder = media_core::VideoEncoder::X264;
    request.settings.crf = 23;
    request.settings.av1an_options = Some(media_core::Av1anOptions {
        split_method: media_core::Av1anSplitMethod::FixedChunks,
        maximum_chunk_frames: 48,
        ..Default::default()
    });
    let submitted = manager.start_encode(request.clone()).await.unwrap();
    let partial = wait_for(&manager, &submitted.id, |job| {
        job.recovery
            .as_ref()
            .is_some_and(|saved| saved.completed_frames >= 48)
    })
    .await;
    assert_eq!(partial.state, JobState::Running, "{partial:#?}");
    assert_eq!(
        manager
            .discard_av1an_recovery(submitted.id.clone())
            .await
            .unwrap_err()
            .code,
        "JOB_DISCARD_UNAVAILABLE"
    );
    manager.stop_job(submitted.id.clone()).await.unwrap();
    let stopped = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    assert_eq!(stopped.state, JobState::Stopped, "{stopped:#?}");
    let saved = stopped
        .recovery
        .as_ref()
        .expect("verified recovery locator");
    assert!(saved.completed_frames >= 48 && saved.completed_frames < 288);
    assert!(!output.exists());
    manager.resume_job(submitted.id.clone()).await.unwrap();
    let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    assert_eq!(finished.state, JobState::Succeeded, "{finished:#?}");
    assert_eq!(finished.encode_settings.as_ref(), Some(&request.settings));
    let inspected = probe(&output).await;
    let streams = inspected["streams"].as_array().unwrap();
    let video = streams
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    assert_eq!(video["codec_name"], "h264");
    assert_eq!(video["nb_read_frames"], "288");
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, av1an, x264, mkvmerge, VapourSynth and L-SMASH on PATH"]
async fn av1an_x264_discard_removes_only_stopped_jobs_saved_workspace() {
    let fixture = Fixture::new();
    let input = fixture.0.join("discard x264 source.mkv");
    let output = fixture.0.join("discard x264 output.mkv");
    synthesize(&input, "640x360", 288).await;
    let original = std::fs::read(&input).unwrap();
    let neighbor = fixture.0.join("neighbor.txt");
    std::fs::write(&neighbor, b"preserve me").unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let mut request = request(&input, &output, 5);
    request.settings.encoder = media_core::VideoEncoder::X264;
    request.settings.crf = 23;
    request.settings.av1an_options = Some(media_core::Av1anOptions {
        split_method: media_core::Av1anSplitMethod::FixedChunks,
        maximum_chunk_frames: 48,
        ..Default::default()
    });
    let submitted = manager.start_encode(request).await.unwrap();
    let partial = wait_for(&manager, &submitted.id, |job| {
        job.recovery
            .as_ref()
            .is_some_and(|saved| saved.completed_frames >= 48)
    })
    .await;
    assert_eq!(partial.state, JobState::Running, "{partial:#?}");
    manager.stop_job(submitted.id.clone()).await.unwrap();
    let stopped = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    assert_eq!(stopped.state, JobState::Stopped, "{stopped:#?}");
    let workspace = PathBuf::from(&stopped.recovery.as_ref().unwrap().workspace);
    assert!(workspace.exists());
    let discarded = manager
        .discard_av1an_recovery(submitted.id.clone())
        .await
        .unwrap();
    assert_eq!(discarded.state, JobState::Stopped);
    assert!(discarded.recovery.is_none());
    assert!(!workspace.exists());
    assert_eq!(
        manager.resume_job(submitted.id).await.unwrap_err().code,
        "JOB_RESUME_UNAVAILABLE"
    );
    assert_eq!(std::fs::read(&neighbor).unwrap(), b"preserve me");
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert!(!output.exists());
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires corrected av1an, libvmaf, FFmpeg, FFprobe, SVT-AV1, VapourSynth and L-SMASH"]
async fn av1an_target_probe_and_final_chunks_share_temporal_and_aspect_processing() {
    let fixture = Fixture::new();
    let input = fixture.0.join("processed target source.mkv");
    let output = fixture.0.join("processed target output.mkv");
    synthesize(&input, "320x180", 96).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let mut request = request(&input, &output, 12);
    request.settings.encoder = media_core::VideoEncoder::SvtAv1Hdr;
    request.settings.temporal = Some(media_core::TemporalSettings {
        frame_rate: Some(media_core::FrameRate {
            numerator: 12,
            denominator: 1,
        }),
        aspect_ratio: Some(media_core::AspectRatioSettings {
            kind: media_core::AspectRatioKind::Sample,
            numerator: 4,
            denominator: 3,
        }),
        ..Default::default()
    });
    request.settings.av1an_options = Some(media_core::Av1anOptions {
        split_method: media_core::Av1anSplitMethod::FixedChunks,
        maximum_chunk_frames: 240,
        scene_downscale_height: None,
        target_quality: Some(media_core::Av1anTargetQuality {
            metric: media_core::Av1anTargetMetric::Vmaf,
            minimum_score_tenths: 0,
            maximum_score_tenths: 1_000,
            minimum_crf: 30,
            maximum_crf: 34,
            probes: 1,
            probing_rate: 1,
            probe_width: 320,
            probe_height: 180,
        }),
        ..Default::default()
    });
    let submitted = manager.start_encode(request).await.unwrap();
    let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    if finished.state != JobState::Succeeded {
        manager.shutdown().await;
        panic!("processed av1an failed: {finished:#?}");
    }
    let inspected = probe(&output).await;
    let video = inspected["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    assert_eq!(video["nb_read_frames"], "48");
    assert_eq!(video["r_frame_rate"], "12/1");
    assert_eq!(video["sample_aspect_ratio"], "4:3");
    assert!(finished.logs.iter().any(|line| {
        line.contains("probe encodes and reference scoring read the same verified lossless")
    }));
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires packaged av1an with CPU scorers, FFmpeg, FFprobe, SVT-AV1, VapourSynth and L-SMASH"]
async fn av1an_filtered_non_vmaf_targets_score_the_verified_processed_source() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/filtered-target-gap-20260925")
        .join(format!("native-{nonce}"));
    std::fs::create_dir_all(&root).unwrap();
    if let Some(resources) = std::env::var_os("JESSES_TEST_TOOL_RESOURCES") {
        media_runtime::configure_bundled_tools(PathBuf::from(resources)).unwrap();
    }
    let input = root.join("source.mkv");
    synthesize(&input, "320x180", 48).await;
    let original = std::fs::read(&input).unwrap();

    for (name, metric, rate) in [
        ("ssimulacra2", media_core::Av1anTargetMetric::Ssimulacra2, 2),
        ("butteraugli", media_core::Av1anTargetMetric::Butteraugli, 2),
        ("xpsnr", media_core::Av1anTargetMetric::Xpsnr, 1),
        (
            "xpsnr-weighted",
            media_core::Av1anTargetMetric::XpsnrWeighted,
            2,
        ),
    ] {
        let output = root.join(format!("{name}.mkv"));
        let manager = JobManager::new(root.join(format!("{name}-logs")));
        let mut request = request(&input, &output, 12);
        request.settings.encoder = media_core::VideoEncoder::SvtAv1Hdr;
        request.settings.workers = 1;
        request.settings.framing = serde_json::from_value(serde_json::json!({
            "crop": {"top": 10, "right": 0, "bottom": 10, "left": 0},
            "resizeWidth": 256
        }))
        .unwrap();
        request.settings.av1an_options = Some(media_core::Av1anOptions {
            split_method: media_core::Av1anSplitMethod::FixedChunks,
            maximum_chunk_frames: 48,
            scene_downscale_height: None,
            target_quality: Some(media_core::Av1anTargetQuality {
                metric,
                minimum_score_tenths: 0,
                maximum_score_tenths: 1_000,
                minimum_crf: 30,
                maximum_crf: 34,
                probes: 1,
                probing_rate: rate,
                probe_width: 256,
                probe_height: 128,
            }),
            ..Default::default()
        });
        let submitted = manager.start_encode(request).await.unwrap();
        let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
        std::fs::write(
            root.join(format!("{name}-snapshot.json")),
            serde_json::to_vec_pretty(&finished).unwrap(),
        )
        .unwrap();
        if finished.state != JobState::Succeeded {
            manager.shutdown().await;
            panic!("filtered {name} target failed: {finished:#?}");
        }
        assert!(finished.logs.iter().any(|line| line.contains(
            "probe encodes and reference scoring read the same verified lossless processed source"
        )));
        let inspected = probe(&output).await;
        let video = inspected["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap();
        assert_eq!(video["width"], 256);
        assert_eq!(video["height"], 128);
        assert_eq!(video["nb_read_frames"], "48");
        assert_eq!(std::fs::read(&input).unwrap(), original);
        manager.shutdown().await;
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, av1an, SVT-AV1, VapourSynth and L-SMASH on PATH"]
async fn canceling_running_av1an_stops_workers_and_releases_output_handles() {
    let fixture = Fixture::new();
    let input = fixture.0.join("cancel source.mkv");
    let output = fixture.0.join("must not publish.mkv");
    synthesize(&input, "640x360", 480).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let request = request(&input, &output, 0);
    let submitted = manager.start_encode(request.clone()).await.unwrap();
    let running = wait_for(&manager, &submitted.id, |job| {
        job.state == JobState::Running
            && job
                .logs
                .iter()
                .any(|line| line.contains("Queue ") && line.contains("Workers"))
    })
    .await;
    if running.state != JobState::Running {
        manager.shutdown().await;
        panic!("av1an must launch its worker queue before cancellation: {running:#?}");
    }
    // Allow the announced worker queue to spawn its decoder/encoder children.
    tokio::time::sleep(Duration::from_millis(200)).await;
    manager.cancel_job(submitted.id.clone()).await.unwrap();
    let canceled = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    tokio::time::timeout(Duration::from_secs(10), manager.shutdown())
        .await
        .expect("Cancellation must reap the worker tree promptly");
    assert_eq!(canceled.state, JobState::Canceled, "{canceled:#?}");
    assert_eq!(canceled.encode_settings.as_ref(), Some(&request.settings));
    assert!(!output.exists(), "Canceled job published an output");
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
    // On Windows this also detects surviving descendants holding their files.
    // Failed-job chunk diagnostics may be retained by the app; the test owns them.
    std::fs::remove_dir_all(&fixture.0).expect("Canceled workers must release all fixture handles");
}
