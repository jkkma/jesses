//! Opt-in native source format gate: `cargo test -p media-runtime --test source_format_jobs -- --include-ignored`.
//! FFmpeg creates every input in an owned temporary directory.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_runtime::{
    EncodeBackend, EncodeRequest, EncodeSettings, JobManager, JobState, RemuxRequest, VideoEncoder,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::Value;
use sha2::{Digest, Sha256};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "jesses-source-format-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, av1an, mkvmerge and standalone x264"]
async fn av1an_converts_verified_444_source_to_explicit_422_output() {
    let ffmpeg = tool("ffmpeg").await;
    let ffprobe = tool("ffprobe").await;
    for tool_name in ["av1an", "mkvmerge", "x264"] {
        let _ = tool(tool_name).await;
    }
    let fixture = Fixture::new();
    let input = fixture.0.join("source-444.mkv");
    let destination = fixture.0.join("encoded-422.mkv");
    let mut command = Command::new(&ffmpeg);
    command.args(["-v", "error", "-nostdin", "-n", "-f", "lavfi", "-i", "testsrc2=s=128x96:r=24", "-frames:v", "8", "-vf",
        "format=yuv444p10le,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v", "ffv1", "-level", "3", "-chroma_sample_location", "left", "-color_range", "tv", "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"])
        .arg(&input);
    output(&command).await;
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: destination.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                backend: EncodeBackend::Av1an,
                encoder: VideoEncoder::X264,
                crf: 24,
                preset: 0,
                workers: 1,
                av1an_options: Some(media_core::Av1anOptions {
                    pixel_format: Some(media_core::Av1anPixelFormat::Yuv422p10le),
                    ..Default::default()
                }),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == started.id)
                .unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
    let source = scan(&ffprobe, &input).await;
    let encoded = scan(&ffprobe, &destination).await;
    assert_eq!(source["streams"][0]["pix_fmt"], "yuv444p10le");
    assert_eq!(encoded["streams"][0]["pix_fmt"], "yuv422p10le");
    for field in [
        "color_range",
        "color_space",
        "color_primaries",
        "color_transfer",
        "sample_aspect_ratio",
    ] {
        assert_eq!(
            encoded["streams"][0][field], source["streams"][0][field],
            "{field}"
        );
    }
    assert_eq!(encoded["frames"].as_array().unwrap().len(), 8);
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Source format fixture retained at {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

async fn tool(name: &str) -> PathBuf {
    media_runtime::get_capabilities()
        .await
        .into_iter()
        .find(|tool| tool.id == name && tool.available)
        .and_then(|tool| tool.path.map(PathBuf::from))
        .unwrap_or_else(|| panic!("{name} is required for the native source format gate"))
}

async fn output(command: &Command) -> Vec<u8> {
    let spec = CommandSpec {
        executable: PathBuf::from(command.get_program()),
        args: command.get_args().map(Into::into).collect(),
        cwd: command.get_current_dir().map(Path::to_path_buf),
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = run_capture(&spec, cancel, 8 * 1024 * 1024, Duration::from_secs(45))
        .await
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

async fn scan(ffprobe: &Path, path: &Path) -> Value {
    let mut command = Command::new(ffprobe);
    command.args(["-v", "error", "-select_streams", "v:0", "-show_streams", "-show_frames", "-show_entries",
        "stream=codec_name,pix_fmt,width,height,sample_aspect_ratio,color_range,color_space,color_primaries,color_transfer:stream_tags=alpha_mode:frame=best_effort_timestamp_time,pix_fmt,color_range,color_space,color_primaries,color_transfer", "-of", "json", "-i"])
        .arg(path);
    serde_json::from_slice(&output(&command).await).unwrap()
}

#[tokio::test]
#[ignore = "requires FFmpeg with libvpx-vp9 and FFprobe"]
async fn vp9_preserves_real_alpha_plane_and_container_flag() {
    let ffmpeg = tool("ffmpeg").await;
    let ffprobe = tool("ffprobe").await;
    let fixture = Fixture::new();
    let raw = fixture.0.join("alpha.yuva");
    let input = fixture.0.join("alpha-source.mkv");
    let (width, height, frames) = (128usize, 96usize, 4usize);
    let mut source_pixels = Vec::new();
    for frame in 0..frames {
        source_pixels.extend((0..width * height).map(|i| ((i + frame * 11) % 180 + 40) as u8));
        source_pixels.extend(std::iter::repeat_n(128u8, width * height / 4));
        source_pixels.extend(std::iter::repeat_n(128u8, width * height / 4));
        source_pixels.extend((0..width * height).map(|i| match (i + frame) % 5 {
            0 => 0,
            1 => 64,
            2 => 128,
            3 => 192,
            _ => 255,
        }));
    }
    std::fs::write(&raw, &source_pixels).unwrap();
    let expected_alpha = source_pixels
        .chunks_exact(width * height * 5 / 2)
        .flat_map(|frame| frame[width * height * 3 / 2..].iter().copied())
        .collect::<Vec<_>>();
    let mut create = Command::new(&ffmpeg);
    create.args(["-v", "error", "-nostdin", "-n", "-f", "rawvideo", "-pix_fmt", "yuva420p", "-video_size", "128x96", "-framerate", "24/1", "-color_range", "tv", "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-chroma_sample_location", "left", "-i"])
        .arg(&raw)
        .args(["-frames:v", "4", "-vf", "setsar=1,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709", "-c:v", "ffv1", "-pix_fmt", "+yuva420p", "-level", "3", "-chroma_sample_location", "left", "-color_range", "tv", "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"])
        .arg(&input);
    output(&create).await;
    let source = scan(&ffprobe, &input).await;
    assert_eq!(source["streams"][0]["pix_fmt"], "yuva420p");
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let manager = JobManager::new(fixture.0.join("logs"));
    for lossless in [false, true] {
        let destination = fixture.0.join(if lossless {
            "alpha-vp9-lossless.mkv"
        } else {
            "alpha-vp9.mkv"
        });
        let started = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: destination.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    encoder: VideoEncoder::Vp9,
                    output_pixel_format: Some(media_core::Av1anPixelFormat::Yuva420p),
                    crf: 18,
                    preset: 4,
                    lossless,
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let snapshot = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|job| job.id == started.id)
                    .unwrap();
                if snapshot.state.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
        assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
        let encoded = scan(&ffprobe, &destination).await;
        assert_eq!(encoded["streams"][0]["pix_fmt"], "yuv420p");
        assert!(
            encoded["streams"][0]["tags"]
                .as_object()
                .unwrap()
                .iter()
                .any(|(key, value)| key.eq_ignore_ascii_case("alpha_mode") && value == "1")
        );
        assert_eq!(encoded["frames"].as_array().unwrap().len(), frames);
        let mut decode = Command::new(&ffmpeg);
        decode
            .args(["-v", "error", "-c:v", "libvpx-vp9", "-i"])
            .arg(&destination)
            .args([
                "-map",
                "0:v:0",
                "-vf",
                "extractplanes=a",
                "-pix_fmt",
                "gray",
                "-f",
                "rawvideo",
                "-",
            ]);
        let alpha = output(&decode).await;
        assert_eq!(alpha.len(), width * height * frames);
        let maximum_delta = alpha
            .iter()
            .zip(&expected_alpha)
            .map(|(actual, expected)| actual.abs_diff(*expected))
            .max()
            .unwrap();
        let mean_delta = alpha
            .iter()
            .zip(&expected_alpha)
            .map(|(actual, expected)| f64::from(actual.abs_diff(*expected)))
            .sum::<f64>()
            / alpha.len() as f64;
        assert!(
            maximum_delta <= if lossless { 0 } else { 16 }
                && mean_delta <= if lossless { 0.0 } else { 2.0 },
            "VP9 alpha drift: max={maximum_delta}, mean={mean_delta:.3}, lossless={lossless}"
        );
        assert!(alpha.contains(&0));
        assert!(alpha.iter().any(|&value| value > 0 && value < 255));
        assert!(alpha.contains(&255));
    }
    let transparent_vp9 = fixture.0.join("alpha-vp9-lossless.mkv");
    let roundtrip = fixture.0.join("alpha-vp9-roundtrip.webm");
    let saved_source = Sha256::digest(std::fs::read(&transparent_vp9).unwrap());
    let started = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: transparent_vp9.to_string_lossy().into_owned(),
                output_path: roundtrip.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::Vp9,
                output_pixel_format: Some(media_core::Av1anPixelFormat::Yuva420p),
                lossless: true,
                preset: 4,
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == started.id)
                .unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    assert_eq!(
        Sha256::digest(std::fs::read(&transparent_vp9).unwrap()),
        saved_source
    );
    let webm = scan(&ffprobe, &roundtrip).await;
    assert_eq!(webm["frames"].as_array().unwrap().len(), frames);
    assert!(
        webm["streams"][0]["tags"]
            .as_object()
            .unwrap()
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("alpha_mode") && value == "1")
    );
    let mut decode = Command::new(&ffmpeg);
    decode
        .args(["-v", "error", "-c:v", "libvpx-vp9", "-i"])
        .arg(&roundtrip)
        .args([
            "-map",
            "0:v:0",
            "-vf",
            "extractplanes=a",
            "-pix_fmt",
            "gray",
            "-f",
            "rawvideo",
            "-",
        ]);
    assert_eq!(output(&decode).await, expected_alpha);
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn opaque_16bit_gbr_source_encodes_to_verified_10bit_yuv_without_source_changes() {
    let ffmpeg = tool("ffmpeg").await;
    let ffprobe = tool("ffprobe").await;
    let _x264 = tool("x264").await;
    let fixture = Fixture::new();
    let raw = fixture.0.join("rgb48.raw");
    let input = fixture.0.join("gbr16-source.mkv");
    let destination = fixture.0.join("gbr16-encoded.mkv");
    let mut samples = Vec::new();
    for frame in 0..4u16 {
        for index in 0..128u16 * 96 {
            for base in [0x1234u16, 0x4567, 0x789a] {
                samples.extend(
                    base.wrapping_add(index.wrapping_mul(7))
                        .wrapping_add(frame * 13)
                        .to_le_bytes(),
                );
            }
        }
    }
    std::fs::write(&raw, samples).unwrap();
    let mut create = Command::new(&ffmpeg);
    create.args(["-v", "error", "-nostdin", "-n", "-f", "rawvideo", "-pix_fmt", "rgb48le", "-video_size", "128x96", "-framerate", "24/1", "-color_range", "pc", "-colorspace", "rgb", "-color_primaries", "bt709", "-color_trc", "bt709", "-i"])
        .arg(&raw)
        .args(["-frames:v", "4", "-vf", "format=gbrp16le,setsar=1,setparams=field_mode=prog:range=pc:color_primaries=bt709:color_trc=bt709:colorspace=gbr", "-c:v", "ffv1", "-pix_fmt", "gbrp16le", "-level", "3", "-color_range", "pc", "-colorspace", "rgb", "-color_primaries", "bt709", "-color_trc", "bt709"])
        .arg(&input);
    output(&create).await;
    let source = scan(&ffprobe, &input).await;
    assert_eq!(source["streams"][0]["pix_fmt"], "gbrp16le");
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: destination.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::X264,
                output_pixel_format: Some(media_core::Av1anPixelFormat::Yuv420p10le),
                lossless: true,
                preset: 0,
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == started.id)
                .unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
    let encoded = scan(&ffprobe, &destination).await;
    assert_eq!(encoded["streams"][0]["pix_fmt"], "yuv420p10le");
    assert_eq!(encoded["streams"][0]["color_space"], "bt709");
    assert_eq!(encoded["streams"][0]["color_range"], "pc");
    assert_eq!(encoded["frames"].as_array().unwrap().len(), 4);
    assert!(
        completed
            .logs
            .iter()
            .any(|line| line.contains("Verified lossless decoded pixel SHA-256"))
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn planar_422_444_and_12bit_sources_encode_with_verified_420_output() {
    let ffmpeg = tool("ffmpeg").await;
    let ffprobe = tool("ffprobe").await;
    let _x264 = tool("x264").await;
    let fixture = Fixture::new();
    for (name, source_format, expected_format) in [
        ("422-10", "yuv422p10le", "yuv420p10le"),
        ("444-8", "yuv444p", "yuv420p"),
        ("444-12", "yuv444p12le", "yuv420p10le"),
    ] {
        let input = fixture.0.join(format!("{name}-source.mkv"));
        let destination = fixture.0.join(format!("{name}-encoded.mkv"));
        let filter = format!(
            "format={source_format},setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709"
        );
        let mut command = Command::new(&ffmpeg);
        command
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=128x96:r=24",
                "-frames:v",
                "4",
                "-vf",
                &filter,
                "-c:v",
                "ffv1",
                "-level",
                "3",
                "-chroma_sample_location",
                "left",
                "-color_range",
                "tv",
                "-colorspace",
                "bt709",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
            ])
            .arg(&input);
        output(&command).await;
        let before = Sha256::digest(std::fs::read(&input).unwrap());
        let source = scan(&ffprobe, &input).await;
        assert_eq!(
            source["streams"][0]["pix_fmt"], source_format,
            "{name}: {source}"
        );
        let manager = JobManager::new(fixture.0.join(format!("{name}-logs")));
        let started = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: destination.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    encoder: VideoEncoder::X264,
                    crf: 18,
                    preset: 0,
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let snapshot = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|job| job.id == started.id)
                    .unwrap();
                if snapshot.state.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            completed.state,
            JobState::Succeeded,
            "{name}: {completed:?}"
        );
        assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
        let encoded = scan(&ffprobe, &destination).await;
        assert_eq!(
            encoded["streams"][0]["pix_fmt"], expected_format,
            "{name}: {encoded}"
        );
        for field in [
            "color_range",
            "color_space",
            "color_primaries",
            "color_transfer",
            "sample_aspect_ratio",
        ] {
            assert_eq!(
                encoded["streams"][0][field], source["streams"][0][field],
                "{name}: {field}"
            );
        }
        let frames = encoded["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 4, "{name}: {encoded}");
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(frame["pix_fmt"], expected_format);
            let time = frame["best_effort_timestamp_time"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap();
            assert!(
                (time - index as f64 / 24.0).abs() <= 0.001_002,
                "{name} frame {index}: {time}"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn packed_yuv_and_opaque_rgb_sources_encode_with_verified_color_and_filter_preview() {
    let ffmpeg = tool("ffmpeg").await;
    let ffprobe = tool("ffprobe").await;
    let _x264 = tool("x264").await;
    let fixture = Fixture::new();
    for (name, pix_fmt, codec, range, matrix, expected_matrix) in [
        ("yuyv", "yuyv422", "rawvideo", "tv", "bt709", "bt709"),
        ("uyvy", "uyvy422", "rawvideo", "tv", "bt709", "bt709"),
        ("rgb", "rgb24", "png", "pc", "gbr", "bt709"),
        ("bgr", "bgr24", "rawvideo", "pc", "gbr", "bt709"),
    ] {
        let input = fixture.0.join(format!("{name}-source.mkv"));
        let destination = fixture.0.join(format!("{name}-encoded.mkv"));
        let mut command = Command::new(&ffmpeg);
        let filter = format!(
            "format={pix_fmt},setparams=field_mode=prog:range={range}:color_primaries=bt709:color_trc=bt709:colorspace={matrix}"
        );
        command.args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=128x96:r=24",
            "-frames:v",
            "4",
            "-vf",
            &filter,
            "-c:v",
            codec,
            "-pix_fmt",
            pix_fmt,
            "-color_range",
            range,
            "-colorspace",
            if matrix == "gbr" { "rgb" } else { matrix },
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
        ]);
        if matrix != "gbr" {
            command.args(["-chroma_sample_location", "left"]);
        }
        if codec == "rawvideo" {
            command.args(["-allow_raw_vfw", "1"]);
        }
        command.arg(&input);
        output(&command).await;
        let source = scan(&ffprobe, &input).await;
        assert_eq!(source["streams"][0]["pix_fmt"], pix_fmt, "{name}: {source}");
        assert_eq!(source["streams"][0]["color_space"], matrix);
        let before = Sha256::digest(std::fs::read(&input).unwrap());
        let request = EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: destination.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::X264,
                crf: 18,
                preset: 0,
                av1an_filters: vec!["eq=contrast=1.10".into()],
                ..Default::default()
            },
        };
        let (_owner, cancel) = tokio::sync::watch::channel(false);
        let preview = media_runtime::preview_encode_plan(request.clone(), cancel)
            .await
            .unwrap();
        let producer = preview
            .stages
            .iter()
            .find(|stage| stage.label == "Pass 1: source decoder")
            .unwrap();
        assert!(
            producer
                .arguments
                .iter()
                .any(|arg| arg.contains("eq=contrast=1.10")),
            "{name}: {producer:#?}"
        );
        assert!(
            producer
                .arguments
                .iter()
                .any(|arg| arg.contains("format=yuv420p")),
            "{name}: {producer:#?}"
        );
        assert!(!destination.exists());
        let manager = JobManager::new(fixture.0.join(format!("{name}-logs")));
        let started = manager.start_encode(request).await.unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let snapshot = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|job| job.id == started.id)
                    .unwrap();
                if snapshot.state.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            completed.state,
            JobState::Succeeded,
            "{name}: {completed:#?}"
        );
        assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
        let encoded = scan(&ffprobe, &destination).await;
        assert_eq!(
            encoded["streams"][0]["pix_fmt"],
            if range == "pc" { "yuvj420p" } else { "yuv420p" },
            "{name}: {encoded}"
        );
        assert_eq!(
            encoded["streams"][0]["color_space"], expected_matrix,
            "{name}: {encoded}"
        );
        assert_eq!(
            encoded["streams"][0]["color_range"], range,
            "{name}: {encoded}"
        );
        assert_eq!(encoded["streams"][0]["color_primaries"], "bt709");
        assert_eq!(encoded["streams"][0]["color_transfer"], "bt709");
        assert_eq!(encoded["frames"].as_array().unwrap().len(), 4);
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and SVT-AV1"]
async fn full_range_rgb_source_converts_to_limited_svt_output() {
    let ffmpeg = tool("ffmpeg").await;
    let ffprobe = tool("ffprobe").await;
    let _svt = tool("svt-av1").await;
    let fixture = Fixture::new();
    let input = fixture.0.join("rgb-source.mkv");
    let destination = fixture.0.join("svt-limited.mkv");
    let mut command = Command::new(&ffmpeg);
    command.args(["-v", "error", "-nostdin", "-n", "-f", "lavfi", "-i", "testsrc2=s=128x96:r=24", "-frames:v", "4", "-vf", "format=rgb24,setparams=field_mode=prog:range=pc:color_primaries=bt709:color_trc=bt709:colorspace=gbr", "-c:v", "png", "-pix_fmt", "rgb24", "-color_range", "pc", "-colorspace", "rgb", "-color_primaries", "bt709", "-color_trc", "bt709"]).arg(&input);
    output(&command).await;
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let request = EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: destination.to_string_lossy().into_owned(),
            stream_indices: vec![0],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::SvtAv1,
            crf: 35,
            preset: 8,
            ..Default::default()
        },
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let preview = media_runtime::preview_encode_plan(request.clone(), cancel)
        .await
        .unwrap();
    let producer = preview
        .stages
        .iter()
        .find(|stage| stage.label == "Pass 1: source decoder")
        .unwrap();
    assert!(
        producer
            .arguments
            .iter()
            .any(|arg| arg.contains("in_range=full:out_range=limited")),
        "{producer:#?}"
    );
    assert!(
        producer
            .arguments
            .iter()
            .any(|arg| arg.contains("format=yuv420p10le")),
        "{producer:#?}"
    );
    let manager = JobManager::new(fixture.0.join("svt-rgb-logs"));
    let started = manager.start_encode(request).await.unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == started.id)
                .unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    let encoded = scan(&ffprobe, &destination).await;
    assert_eq!(encoded["streams"][0]["pix_fmt"], "yuv420p10le", "{encoded}");
    assert_eq!(encoded["streams"][0]["color_range"], "tv", "{encoded}");
    assert_eq!(encoded["streams"][0]["color_space"], "bt709", "{encoded}");
    assert_eq!(encoded["frames"].as_array().unwrap().len(), 4);
    assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, x264 and x265"]
async fn standalone_output_chroma_selection_matches_preview_and_decoded_video() {
    let ffmpeg = tool("ffmpeg").await;
    let ffprobe = tool("ffprobe").await;
    let _x264 = tool("x264").await;
    let _x265 = tool("x265").await;
    let fixture = Fixture::new();
    let input = fixture.0.join("source-444.mkv");
    let mut command = Command::new(&ffmpeg);
    command.args(["-v", "error", "-nostdin", "-n", "-f", "lavfi", "-i", "testsrc2=s=128x96:r=24", "-frames:v", "4", "-vf", "format=yuv444p10le,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709", "-c:v", "ffv1", "-level", "3", "-color_range", "tv", "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-chroma_sample_location", "left"]).arg(&input);
    output(&command).await;
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    for (encoder, selected, name, expected_codec) in [
        (
            VideoEncoder::X264,
            media_core::Av1anPixelFormat::Yuv444p10le,
            "x264-444",
            "h264",
        ),
        (
            VideoEncoder::X265Standalone,
            media_core::Av1anPixelFormat::Yuv422p10le,
            "x265-422",
            "hevc",
        ),
        (
            VideoEncoder::X265,
            media_core::Av1anPixelFormat::Yuv444p10le,
            "ffmpeg-x265-444",
            "hevc",
        ),
        (
            VideoEncoder::Vp9,
            media_core::Av1anPixelFormat::Yuv444p10le,
            "ffmpeg-vp9-444",
            "vp9",
        ),
    ] {
        let destination = fixture.0.join(format!("{name}.mkv"));
        let request = EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: destination.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder,
                crf: 22,
                preset: 0,
                output_pixel_format: Some(selected),
                ..Default::default()
            },
        };
        let (_owner, cancel) = tokio::sync::watch::channel(false);
        let preview = media_runtime::preview_encode_plan(request.clone(), cancel)
            .await
            .unwrap();
        let producer = preview
            .stages
            .iter()
            .find(|stage| stage.label == "Pass 1: source decoder")
            .unwrap();
        assert!(
            producer
                .arguments
                .windows(2)
                .any(|pair| pair[0] == "-pix_fmt" && pair[1] == selected.ffmpeg()),
            "{name}: {producer:#?}"
        );
        assert!(!destination.exists());
        let manager = JobManager::new(fixture.0.join(format!("{name}-logs")));
        let started = manager.start_encode(request).await.unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let snapshot = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|job| job.id == started.id)
                    .unwrap();
                if snapshot.state.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            completed.state,
            JobState::Succeeded,
            "{name}: {completed:#?}"
        );
        let encoded = scan(&ffprobe, &destination).await;
        assert_eq!(encoded["streams"][0]["codec_name"], expected_codec);
        assert_eq!(encoded["streams"][0]["pix_fmt"], selected.ffmpeg());
        assert_eq!(encoded["streams"][0]["color_space"], "bt709");
        assert_eq!(encoded["streams"][0]["color_primaries"], "bt709");
        assert_eq!(encoded["streams"][0]["color_transfer"], "bt709");
        assert_eq!(encoded["frames"].as_array().unwrap().len(), 4);
        assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
    }
}
