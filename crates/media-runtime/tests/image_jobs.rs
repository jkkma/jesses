//! Actual FFmpeg image ordering, export, no-clobber and cancellation gates.
use media_core::{FrameRate, ImageOutput, ImagePixelFormat, ImageRequest};
use media_runtime::{
    jobs::run_image_job,
    supervisor::{CommandSpec, run_capture},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "jesses-image-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Retained fixture {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
async fn tool_output(name: &str, values: Vec<OsString>) -> Vec<u8> {
    let tools = media_runtime::get_capabilities().await;
    let executable = PathBuf::from(
        tools
            .into_iter()
            .find(|t| t.id == name)
            .unwrap()
            .path
            .unwrap(),
    );
    let (_owner, cancel) = watch::channel(false);
    let output = run_capture(
        &CommandSpec {
            executable,
            args: values,
            cwd: None,
        },
        cancel,
        16 * 1024 * 1024,
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
async fn ffmpeg(values: Vec<OsString>) -> Vec<u8> {
    tool_output("ffmpeg", values).await
}
async fn ffprobe(values: Vec<OsString>) -> serde_json::Value {
    serde_json::from_slice(&tool_output("ffprobe", values).await).unwrap()
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
async fn pixels(path: &Path) -> Vec<u8> {
    let mut a = args(&["-v", "error", "-i"]);
    a.push(path.into());
    a.extend(args(&[
        "-map",
        "0:v:0",
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-",
    ]));
    ffmpeg(a).await
}

async fn pixels_as(path: &Path, pixel_format: &str) -> Vec<u8> {
    let mut a = args(&["-v", "error", "-i"]);
    a.push(path.into());
    a.extend(args(&[
        "-map",
        "0:v:0",
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        pixel_format,
        "-",
    ]));
    ffmpeg(a).await
}

async fn all_pixels_as(path: &Path, pixel_format: &str) -> Vec<u8> {
    let mut a = args(&["-v", "error", "-i"]);
    a.push(path.into());
    a.extend(args(&[
        "-map",
        "0:v:0",
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-pix_fmt",
        pixel_format,
        "-",
    ]));
    ffmpeg(a).await
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn explicit_png_depth_and_alpha_preserve_decoded_samples() {
    let fixture = Fixture::new();
    let raw = fixture.0.join("source.rgba64le");
    let source = fixture.0.join("source.png");
    let mut samples = Vec::new();
    for index in 0..16u16 {
        for channel in [
            0x1234 + index,
            0x5678 + index,
            0x9abc + index,
            0x3456 + index,
        ] {
            samples.extend(channel.to_le_bytes());
        }
    }
    std::fs::write(&raw, &samples).unwrap();
    let mut create = args(&[
        "-v", "error", "-nostdin", "-f", "rawvideo", "-pix_fmt", "rgba64le", "-s", "4x4", "-r",
        "1", "-i",
    ]);
    create.push(raw.into());
    create.extend(args(&[
        "-frames:v",
        "1",
        "-c:v",
        "png",
        "-pix_fmt",
        "rgba64be",
    ]));
    create.push(source.clone().into());
    ffmpeg(create).await;
    let source_bytes = std::fs::read(&source).unwrap();
    let decoded = pixels_as(&source, "rgba64be").await;
    assert_eq!(
        &decoded[..8],
        &[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0x34, 0x56]
    );
    let (_owner, cancel) = watch::channel(false);
    for (format, expected_layout) in [
        (ImagePixelFormat::Rgb24, "rgb24"),
        (ImagePixelFormat::Rgba, "rgba"),
        (ImagePixelFormat::Rgb48, "rgb48be"),
        (ImagePixelFormat::Rgba64, "rgba64be"),
    ] {
        let output = fixture.0.join(format!("{:?}.png", format));
        let result = run_image_job(
            ImageRequest::Export {
                input_path: source.to_string_lossy().into_owned(),
                stream_index: 0,
                start_frame: 0,
                frame_count: 1,
                format: ImageOutput::Png,
                output_path: output.to_string_lossy().into_owned(),
                width: None,
                pixel_format: Some(format),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
        assert_eq!(result.frame_count, 1);
        let mut probe_args = args(&[
            "-v",
            "error",
            "-show_entries",
            "stream=pix_fmt",
            "-of",
            "json",
            "-i",
        ]);
        probe_args.push(output.clone().into());
        let probe = ffprobe(probe_args).await;
        assert_eq!(probe["streams"][0]["pix_fmt"], expected_layout);
        let result_bytes = pixels_as(&output, expected_layout).await;
        assert_eq!(result_bytes, pixels_as(&source, expected_layout).await);
        if format == ImagePixelFormat::Rgba64 {
            assert_eq!(
                &result_bytes[..8],
                &[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0x34, 0x56]
            );
        }
        if format == ImagePixelFormat::Rgba {
            assert!(
                result_bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] != 255 && pixel[3] != 0)
            );
        }
    }
    let imported = fixture.0.join("high-depth-sequence.mkv");
    let result = run_image_job(
        ImageRequest::ImportSequence {
            paths: vec![source.to_string_lossy().into_owned(); 2],
            frame_rate: FrameRate {
                numerator: 24,
                denominator: 1,
            },
            output_path: imported.to_string_lossy().into_owned(),
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(result.frame_count, 2);
    let imported_pixels = all_pixels_as(&imported, "rgba64be").await;
    assert_eq!(imported_pixels, [decoded.clone(), decoded.clone()].concat());
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    let invalid = fixture.0.join("not-alpha.jpg");
    let error = run_image_job(
        ImageRequest::Export {
            input_path: source.to_string_lossy().into_owned(),
            stream_index: 0,
            start_frame: 0,
            frame_count: 1,
            format: ImageOutput::Jpeg,
            output_path: invalid.to_string_lossy().into_owned(),
            width: None,
            pixel_format: Some(ImagePixelFormat::Rgba),
        },
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "IMAGE_JOB_FAILED");
    assert!(!invalid.exists());
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn rational_image_rates_preserve_every_source_frame_and_timestamp() {
    let fixture = Fixture::new();
    let source_pattern = fixture.0.join("source-%02d.png");
    let mut create = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=64x48:r=17:d=1",
        "-frames:v",
        "17",
        "-pix_fmt",
        "rgb24",
        "-threads",
        "1",
    ]);
    create.push(source_pattern.into());
    ffmpeg(create).await;

    // Reverse filenames so a filename sort cannot accidentally pass.
    let paths: Vec<PathBuf> = (1..=17)
        .rev()
        .map(|i| fixture.0.join(format!("source-{i:02}.png")))
        .collect();
    let original_bytes: Vec<Vec<u8>> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    let mut expected = Vec::new();
    for path in &paths {
        expected.extend(pixels(path).await);
    }
    assert_eq!(expected.len(), 17 * 64 * 48 * 3);
    let (_sender, cancel) = watch::channel(false);
    for (label, numerator, denominator) in [
        ("half", 1, 2),
        ("twelve", 12, 1),
        ("ntsc24", 24_000, 1001),
        ("ntsc30", 30_000, 1001),
        ("sixty", 60, 1),
        ("one-twenty", 120, 1),
    ] {
        let output = fixture.0.join(format!("{label}.mkv"));
        let result = run_image_job(
            ImageRequest::ImportSequence {
                paths: paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
                frame_rate: FrameRate {
                    numerator,
                    denominator,
                },
                output_path: output.to_string_lossy().into_owned(),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
        assert_eq!(result.frame_count, 17, "{label}");
        assert_eq!(
            pixels(&output).await,
            expected,
            "{label}: decoded frame identities"
        );

        let mut probe_args = args(&[
            "-v",
            "error",
            "-count_frames",
            "-show_streams",
            "-show_frames",
            "-show_format",
            "-of",
            "json",
        ]);
        probe_args.push(output.into());
        let probe = ffprobe(probe_args).await;
        let stream = &probe["streams"][0];
        assert_eq!(stream["codec_name"], "ffv1", "{label}");
        assert_eq!(stream["nb_read_frames"], "17", "{label}");
        let rate = stream["avg_frame_rate"].as_str().unwrap();
        let (actual_num, actual_den) = rate.split_once('/').unwrap();
        let actual_num: u64 = actual_num.parse().unwrap();
        let actual_den: u64 = actual_den.parse().unwrap();
        assert_eq!(
            actual_num * u64::from(denominator),
            actual_den * u64::from(numerator),
            "{label}: stream rate {rate}"
        );

        let frames = probe["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 17, "{label}: metadata frame count");
        for (index, frame) in frames.iter().enumerate() {
            let pts: f64 = frame["best_effort_timestamp_time"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let expected_pts = index as f64 * f64::from(denominator) / f64::from(numerator);
            assert!(
                (pts - expected_pts).abs() <= 0.0015,
                "{label}: frame {index} PTS {pts} expected {expected_pts}"
            );
        }
        let duration: f64 = probe["format"]["duration"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let expected_duration = 17.0 * f64::from(denominator) / f64::from(numerator);
        assert!(
            (duration - expected_duration).abs() <= 0.003,
            "{label}: duration {duration} expected {expected_duration}"
        );
    }
    for (path, original) in paths.iter().zip(original_bytes) {
        assert_eq!(
            std::fs::read(path).unwrap(),
            original,
            "source bytes changed: {}",
            path.display()
        );
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn explicit_image_order_round_trips_through_lossless_import_and_png_export() {
    let fixture = Fixture::new();
    let mut inputs = Vec::new();
    let mut originals = Vec::new();
    for (name, color) in [("z.png", "red"), ("a.png", "blue"), ("middle.png", "green")] {
        let path = fixture.0.join(name);
        let mut a = args(&[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            &format!("color=c={color}:s=64x48:r=12"),
            "-frames:v",
            "1",
            "-threads",
            "1",
        ]);
        a.push(path.clone().into());
        ffmpeg(a).await;
        originals.push(std::fs::read(&path).unwrap());
        inputs.push(path);
    }
    let expected = [
        pixels(&inputs[0]).await,
        pixels(&inputs[1]).await,
        pixels(&inputs[2]).await,
    ]
    .concat();
    let output = fixture.0.join("sequence.mkv");
    let (_sender, cancel) = watch::channel(false);
    let request = ImageRequest::ImportSequence {
        paths: inputs
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        frame_rate: FrameRate {
            numerator: 12,
            denominator: 1,
        },
        output_path: output.to_string_lossy().into_owned(),
    };
    let result = run_image_job(request.clone(), cancel.clone())
        .await
        .unwrap();
    assert_eq!(result.frame_count, 3);
    assert_eq!(pixels(&output).await, expected);
    assert_eq!(
        run_image_job(request, cancel.clone())
            .await
            .unwrap_err()
            .code,
        "OUTPUT_EXISTS"
    );
    let directory = fixture.0.join("frames");
    let exported = run_image_job(
        ImageRequest::Export {
            input_path: output.to_string_lossy().into_owned(),
            stream_index: 0,
            start_frame: 0,
            frame_count: 3,
            format: ImageOutput::PngSequence,
            output_path: directory.to_string_lossy().into_owned(),
            width: None,
            pixel_format: None,
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(exported.frame_count, 3);
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 3);
    for (i, input) in inputs.iter().enumerate() {
        assert_eq!(std::fs::read(input).unwrap(), originals[i]);
        assert_eq!(
            pixels(&directory.join(format!("frame-{:06}.png", i + 1))).await,
            pixels(input).await
        );
    }
    let failed = run_image_job(
        ImageRequest::Export {
            input_path: output.to_string_lossy().into_owned(),
            stream_index: 0,
            start_frame: 0,
            frame_count: 1,
            format: ImageOutput::PngSequence,
            output_path: directory.to_string_lossy().into_owned(),
            width: None,
            pixel_format: None,
        },
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(failed.code, "OUTPUT_EXISTS");
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 3);
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn gif_and_still_exports_decode_and_cancellation_never_publishes() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=10:d=1",
        "-c:v",
        "ffv1",
    ]);
    a.push(input.clone().into());
    ffmpeg(a).await;
    let (_sender, cancel) = watch::channel(false);
    for (format, name, count) in [
        (ImageOutput::Gif, "clip.gif", 10),
        (ImageOutput::Png, "still.png", 1),
        (ImageOutput::Jpeg, "still.jpg", 1),
    ] {
        let output = fixture.0.join(name);
        let result = run_image_job(
            ImageRequest::Export {
                input_path: input.to_string_lossy().into_owned(),
                stream_index: 0,
                start_frame: 0,
                frame_count: count,
                format,
                output_path: output.to_string_lossy().into_owned(),
                width: Some(48),
                pixel_format: None,
            },
            cancel.clone(),
        )
        .await
        .unwrap();
        assert_eq!(result.width, 48);
        assert_eq!(result.frame_count, count);
        assert!(!pixels(&output).await.is_empty());
    }
    let (_sender, cancel) = watch::channel(true);
    let output = fixture.0.join("canceled");
    let error = run_image_job(
        ImageRequest::Export {
            input_path: input.to_string_lossy().into_owned(),
            stream_index: 0,
            start_frame: 0,
            frame_count: 10,
            format: ImageOutput::PngSequence,
            output_path: output.to_string_lossy().into_owned(),
            width: None,
            pixel_format: None,
        },
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "JOB_CANCELED");
    assert!(!output.exists());
    assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jesses-")
    }));
}
