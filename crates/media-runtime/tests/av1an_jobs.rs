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
        if std::env::var_os("JESSES_KEEP_TEST_FIXTURES").is_some() {
            return;
        }
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

async fn synthesize_hdr10(path: &Path, frames: u32) {
    let filter = "testsrc2=size=128x96:rate=24000/1001,format=yuv420p10le,setparams=range=limited:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc";
    let generated = output(
        command("ffmpeg")
            .await
            .args([
                "-v", "error", "-nostdin", "-n", "-f", "lavfi", "-i", filter,
                "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
                "-map", "0:v", "-map", "1:a", "-frames:v", &frames.to_string(),
                "-t", &(f64::from(frames) * 1001.0 / 24000.0).to_string(),
                "-c:v", "libx265", "-preset", "ultrafast",
                "-x265-params", "log-level=error:pools=none:frame-threads=1:bframes=0:hdr10=1:chromaloc=0:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=200,142",
                "-c:a", "pcm_s16le", "-color_primaries", "bt2020",
                "-color_trc", "smpte2084", "-colorspace", "bt2020nc",
                "-color_range", "tv", "-chroma_sample_location", "left",
            ])
            .arg(path),
    )
    .await;
    assert!(
        generated.status.success(),
        "HDR fixture: {}",
        String::from_utf8_lossy(&generated.stderr)
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

async fn probe_first_video_frame(path: &Path) -> Value {
    let captured = output(
        command("ffprobe")
            .await
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_frames",
                "-read_intervals",
                "%+#1",
                "-of",
                "json",
            ])
            .arg(path),
    )
    .await;
    assert!(
        captured.status.success(),
        "Frame probe: {}",
        String::from_utf8_lossy(&captured.stderr)
    );
    serde_json::from_slice(&captured.stdout).unwrap()
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
#[ignore = "requires FFmpeg, FFprobe, av1an, aomenc, vpxenc, x265, mkvmerge, VapourSynth and L-SMASH on PATH"]
async fn av1an_aom_vpx_x265_publish_complete_timed_video() {
    let fixture = Fixture::new();
    let input = fixture.0.join("additional encoders source.mkv");
    synthesize(&input, "128x96", 16).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    for (encoder, codec, preset) in [
        (media_core::VideoEncoder::AomAv1, "av1", 8),
        (media_core::VideoEncoder::VpxStandalone, "vp9", 5),
        (media_core::VideoEncoder::X265Standalone, "hevc", 0),
    ] {
        let output = fixture.0.join(format!("{codec}-av1an.mkv"));
        let mut request = request(&input, &output, preset);
        request.settings.encoder = encoder;
        request.settings.crf = 32;
        request.settings.parameters = match encoder {
            media_core::VideoEncoder::AomAv1 => vec![
                media_core::EncoderParameter {
                    name: "sharpness".into(),
                    value: "2".into(),
                },
                media_core::EncoderParameter {
                    name: "tune".into(),
                    value: "psnr".into(),
                },
            ],
            media_core::VideoEncoder::VpxStandalone => vec![media_core::EncoderParameter {
                name: "tune".into(),
                value: "ssim".into(),
            }],
            media_core::VideoEncoder::X265Standalone => vec![
                media_core::EncoderParameter {
                    name: "deblock".into(),
                    value: "-1:1".into(),
                },
                media_core::EncoderParameter {
                    name: "me".into(),
                    value: "hex".into(),
                },
                media_core::EncoderParameter {
                    name: "sao".into(),
                    value: "0".into(),
                },
            ],
            _ => unreachable!(),
        };
        request.settings.av1an_options = Some(media_core::Av1anOptions {
            split_method: media_core::Av1anSplitMethod::FixedChunks,
            maximum_chunk_frames: 8,
            minimum_scene_frames: 1,
            ..Default::default()
        });
        let submitted = manager.start_encode(request).await.unwrap();
        let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
        assert_eq!(
            finished.state,
            JobState::Succeeded,
            "{encoder:?}: {finished:#?}"
        );
        let inspected = probe(&output).await;
        let video = inspected["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap();
        assert_eq!(video["codec_name"], codec);
        assert_eq!(video["nb_read_frames"], "16");
        assert_eq!(video["r_frame_rate"], "24000/1001");
        assert_eq!(video["pix_fmt"], "yuv420p10le");
        assert_eq!(std::fs::read(&input).unwrap(), original);
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires AOM/VPX, FFmpeg, FFprobe, av1an/VapourSynth and mkvmerge"]
async fn aom_vpx_non_420_outputs_preserve_frame_count_and_timing() {
    use media_core::{Av1anPixelFormat as Pixel, VideoEncoder};
    let fixture = Fixture::new();
    let input = fixture.0.join("non420 source.mkv");
    synthesize(&input, "128x96", 16).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    for (encoder, backend, format, codec, pixel) in [
        (
            VideoEncoder::AomAv1,
            EncodeBackend::Standalone,
            Pixel::Yuv422p10le,
            "av1",
            "yuv422p10le",
        ),
        (
            VideoEncoder::AomAv1,
            EncodeBackend::Av1an,
            Pixel::Yuv422p10le,
            "av1",
            "yuv422p10le",
        ),
        (
            VideoEncoder::AomAv1,
            EncodeBackend::Standalone,
            Pixel::Yuv444p10le,
            "av1",
            "yuv444p10le",
        ),
        (
            VideoEncoder::AomAv1,
            EncodeBackend::Av1an,
            Pixel::Yuv444p10le,
            "av1",
            "yuv444p10le",
        ),
        (
            VideoEncoder::VpxStandalone,
            EncodeBackend::Standalone,
            Pixel::Yuv444p10le,
            "vp9",
            "yuv444p10le",
        ),
        (
            VideoEncoder::VpxStandalone,
            EncodeBackend::Av1an,
            Pixel::Yuv444p10le,
            "vp9",
            "yuv444p10le",
        ),
    ] {
        let output = fixture
            .0
            .join(format!("{encoder:?}-{backend:?}-{pixel}.mkv"));
        let mut selected = request(
            &input,
            &output,
            if encoder == VideoEncoder::AomAv1 {
                8
            } else {
                5
            },
        );
        selected.settings.backend = backend;
        selected.settings.encoder = encoder;
        selected.settings.crf = 32;
        if backend == EncodeBackend::Av1an {
            selected.settings.av1an_options = Some(media_core::Av1anOptions {
                pixel_format: Some(format),
                split_method: media_core::Av1anSplitMethod::FixedChunks,
                maximum_chunk_frames: 8,
                minimum_scene_frames: 1,
                ..Default::default()
            });
        } else {
            selected.settings.output_pixel_format = Some(format);
        }
        let submitted = manager.start_encode(selected).await.unwrap();
        let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
        assert_eq!(
            finished.state,
            JobState::Succeeded,
            "{encoder:?} {backend:?} {pixel}: {finished:#?}"
        );
        let inspected = probe(&output).await;
        let video = inspected["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap();
        assert_eq!(video["codec_name"], codec);
        assert_eq!(video["pix_fmt"], pixel);
        assert_eq!(video["nb_read_frames"], "16");
        assert_eq!(video["r_frame_rate"], "24000/1001");
        assert_eq!(std::fs::read(&input).unwrap(), original);
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, x265, mkvmerge, and av1an/VapourSynth for the parallel route"]
async fn x265_standalone_and_av1an_preserve_hdr10_static_metadata() {
    let fixture = Fixture::new();
    let input = fixture.0.join("hdr10 source.mkv");
    synthesize_hdr10(&input, 16).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    for backend in [EncodeBackend::Standalone, EncodeBackend::Av1an] {
        let output = fixture.0.join(format!("x265-{backend:?}.mkv"));
        let mut selected = request(&input, &output, 0);
        selected.settings.backend = backend;
        selected.settings.encoder = media_core::VideoEncoder::X265Standalone;
        selected.settings.crf = 28;
        if backend == EncodeBackend::Av1an {
            selected.settings.av1an_options = Some(media_core::Av1anOptions {
                split_method: media_core::Av1anSplitMethod::FixedChunks,
                maximum_chunk_frames: 8,
                minimum_scene_frames: 1,
                ..Default::default()
            });
        }
        let submitted = manager.start_encode(selected).await.unwrap();
        let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
        assert_eq!(
            finished.state,
            JobState::Succeeded,
            "{backend:?}: {finished:#?}"
        );
        let inspected = probe(&output).await;
        let video = inspected["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap();
        assert_eq!(video["codec_name"], "hevc");
        assert_eq!(video["nb_read_frames"], "16");
        assert_eq!(video["r_frame_rate"], "24000/1001");
        assert_eq!(video["pix_fmt"], "yuv420p10le");
        assert_eq!(video["color_primaries"], "bt2020");
        assert_eq!(video["color_transfer"], "smpte2084");
        assert_eq!(video["color_space"], "bt2020nc");
        let first = probe_first_video_frame(&output).await;
        let side = first["frames"][0]["side_data_list"].as_array().unwrap();
        assert!(
            side.iter()
                .any(|entry| entry["side_data_type"] == "Mastering display metadata")
        );
        assert!(
            side.iter()
                .any(|entry| entry["side_data_type"] == "Content light level metadata")
        );
        assert_eq!(std::fs::read(&input).unwrap(), original);
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires a compatible AOM build, FFmpeg, FFprobe, mkvmerge, and av1an/VapourSynth for the parallel route"]
async fn aom_standalone_and_av1an_preserve_hdr10_static_metadata() {
    let fixture = Fixture::new();
    let input = fixture.0.join("hdr10 aom source.mkv");
    synthesize_hdr10(&input, 16).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    for backend in [EncodeBackend::Standalone, EncodeBackend::Av1an] {
        let output = fixture.0.join(format!("aom-{backend:?}.mkv"));
        let mut selected = request(&input, &output, 8);
        selected.settings.backend = backend;
        selected.settings.encoder = media_core::VideoEncoder::AomAv1;
        selected.settings.crf = 32;
        if backend == EncodeBackend::Av1an {
            selected.settings.av1an_options = Some(media_core::Av1anOptions {
                split_method: media_core::Av1anSplitMethod::FixedChunks,
                maximum_chunk_frames: 8,
                minimum_scene_frames: 1,
                ..Default::default()
            });
        }
        let submitted = manager.start_encode(selected).await.unwrap();
        let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
        assert_eq!(
            finished.state,
            JobState::Succeeded,
            "{backend:?}: {finished:#?}"
        );
        let inspected = probe(&output).await;
        let video = inspected["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap();
        assert_eq!(video["codec_name"], "av1");
        assert_eq!(video["nb_read_frames"], "16");
        assert_eq!(video["r_frame_rate"], "24000/1001");
        assert_eq!(video["pix_fmt"], "yuv420p10le");
        assert_eq!(video["color_primaries"], "bt2020");
        assert_eq!(video["color_transfer"], "smpte2084");
        assert_eq!(video["color_space"], "bt2020nc");
        let first = probe_first_video_frame(&output).await;
        let side = first["frames"][0]["side_data_list"].as_array().unwrap();
        assert!(
            side.iter()
                .any(|entry| entry["side_data_type"] == "Mastering display metadata")
        );
        assert!(
            side.iter()
                .any(|entry| entry["side_data_type"] == "Content light level metadata")
        );
        assert_eq!(std::fs::read(&input).unwrap(), original);
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires standalone aomenc and SVT-AV1, FFmpeg, FFprobe and JESSES_GRAV1SYNTH"]
async fn standalone_aom_and_svt_use_requested_grain_sources() {
    use media_core::{Av1anGrainSettings, VideoEncoder};
    const TABLE: &str = "filmgrn1\nE 0 18446744073709551615 1 10956 1\n\tp 0 6 0 8 1 1 0 0 0 0 0 0\n\tsY 14  0 5 20 4 39 4 59 4 78 4 98 4 118 4 137 4 157 4 177 4 196 4 216 5 235 5 255 5\n\tsCb 0\n\tsCr 0\n\tcY\n\tcCb 0\n\tcCr 0\n";
    let fixture = Fixture::new();
    let input = fixture.0.join("grain source.mkv");
    synthesize(&input, "128x96", 16).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    for (name, encoder, strength, table) in [
        ("aom-numeric", VideoEncoder::AomAv1, 8, None),
        ("aom-table", VideoEncoder::AomAv1, 0, Some(TABLE)),
        ("svt-table", VideoEncoder::SvtAv1, 0, Some(TABLE)),
    ] {
        let output = fixture.0.join(format!("{name}.mkv"));
        let mut selected = request(
            &input,
            &output,
            if encoder == VideoEncoder::AomAv1 {
                8
            } else {
                10
            },
        );
        selected.settings.backend = EncodeBackend::Standalone;
        selected.settings.encoder = encoder;
        selected.settings.film_grain = strength;
        if let Some(table) = table {
            selected.settings.av1an_grain = Some(Av1anGrainSettings {
                table: Some(table.into()),
                denoise: false,
                denoise_strength: 4,
            });
        }
        let submitted = manager.start_encode(selected).await.unwrap();
        let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
        assert_eq!(finished.state, JobState::Succeeded, "{name}: {finished:#?}");
        let inspected = probe(&output).await;
        let video = inspected["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap();
        assert_eq!(video["codec_name"], "av1", "{name}");
        assert_eq!(video["nb_read_frames"], "16", "{name}");
        assert_eq!(video["r_frame_rate"], "24000/1001", "{name}");
        let inspected_table = fixture.0.join(format!("{name}-headers.tbl"));
        let inspector = std::env::var_os("JESSES_GRAV1SYNTH")
            .expect("JESSES_GRAV1SYNTH points to the installed grav1synth executable");
        let headers = self::output(
            std::process::Command::new(inspector)
                .arg("inspect")
                .arg(&output)
                .arg("--output")
                .arg(&inspected_table),
        )
        .await;
        assert!(
            headers.status.success(),
            "{name} grain-header inspection: {}",
            String::from_utf8_lossy(&headers.stderr)
        );
        let table_text = std::fs::read_to_string(&inspected_table).unwrap();
        assert!(
            table_text.starts_with("filmgrn1\n") && table_text.contains("\nE "),
            "{name} has no readable film-grain headers"
        );
        assert_eq!(std::fs::read(&input).unwrap(), original);
        assert_no_partial_output(&fixture.0);
    }
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
