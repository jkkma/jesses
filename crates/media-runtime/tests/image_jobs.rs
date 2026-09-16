//! Actual FFmpeg image ordering, export, no-clobber and cancellation gates.
use media_core::{FrameRate, ImageOutput, ImageRequest};
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
async fn ffmpeg(values: Vec<OsString>) -> Vec<u8> {
    let tools = media_runtime::get_capabilities().await;
    let executable = PathBuf::from(
        tools
            .into_iter()
            .find(|t| t.id == "ffmpeg")
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
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
async fn pixels(path: &Path) -> Vec<u8> {
    let mut a = args(&["-v", "error", "-i"]);
    a.push(path.into());
    a.extend(args(&[
        "-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "rgb24", "-",
    ]));
    ffmpeg(a).await
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
