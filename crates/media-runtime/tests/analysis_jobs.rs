//! Opt-in source analysis gate using synthetic media and real FFmpeg/FFprobe.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use media_runtime::{
    AutoCropRequest, CropSettings, FramePreviewRequest, detect_crop, preview_frame,
    supervisor::{CommandSpec, run_capture},
};
use sha2::{Digest, Sha256};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-analysis-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Analysis fixture retained at {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

async fn ffmpeg(args: Vec<OsString>) -> Vec<u8> {
    static FFMPEG: tokio::sync::OnceCell<PathBuf> = tokio::sync::OnceCell::const_new();
    let executable = FFMPEG
        .get_or_init(|| async {
            let tools = media_runtime::get_capabilities().await;
            PathBuf::from(
                tools
                    .into_iter()
                    .find(|tool| tool.id == "ffmpeg")
                    .unwrap()
                    .path
                    .expect("FFmpeg is required"),
            )
        })
        .await
        .clone();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(
        &CommandSpec {
            executable,
            args,
            cwd: None,
        },
        cancel,
        8 * 1024 * 1024,
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

async fn fixture(path: &Path, second_video: bool, hdr: bool, black: bool) {
    let first = if black {
        "color=c=black:s=192x112:r=24:d=1"
    } else {
        "color=c=red:s=128x80:r=24:d=1,pad=192:112:32:16:black"
    };
    let mut command = args(&[
        "-hide_banner",
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        first,
    ]);
    if second_video {
        command.extend(args(&[
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=160x80:r=24:d=1,pad=192:112:16:16:black",
            "-map",
            "0:v",
            "-map",
            "1:v",
        ]));
    }
    let parameters = if hdr {
        "setparams=range=tv:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc"
    } else {
        "setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709"
    };
    command.extend(args(&[
        "-vf",
        parameters,
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-pix_fmt",
        if hdr { "yuv420p10le" } else { "yuv420p" },
        "-color_primaries",
        if hdr { "bt2020" } else { "bt709" },
        "-color_trc",
        if hdr { "smpte2084" } else { "bt709" },
        "-colorspace",
        if hdr { "bt2020nc" } else { "bt709" },
        "-color_range",
        "tv",
    ]));
    command.push(path.as_os_str().to_owned());
    ffmpeg(command).await;
}

async fn preview_pixels(path: &Path, image: &str) -> Vec<u8> {
    let image = STANDARD
        .decode(image.strip_prefix("data:image/png;base64,").unwrap())
        .unwrap();
    std::fs::write(path, image).unwrap();
    let mut command = args(&["-hide_banner", "-v", "error", "-nostdin", "-i"]);
    command.push(path.as_os_str().to_owned());
    command.extend(args(&[
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-",
    ]));
    ffmpeg(command).await
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn preview_and_crop_use_the_selected_stream_and_preserve_the_source() {
    let directory = Fixture::new();
    let source = directory.0.join("source's 视频 $.mkv");
    fixture(&source, true, false, false).await;
    let before = Sha256::digest(std::fs::read(&source).unwrap());
    let before_metadata = std::fs::metadata(&source).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let request = FramePreviewRequest {
        display_orientation: None,
        input_path: source.to_string_lossy().into_owned(),
        video_stream_index: 1,
        position_seconds: 0.2,
    };
    let preview = preview_frame(request.clone(), cancel.clone())
        .await
        .unwrap();
    assert_eq!(
        (
            preview.width,
            preview.height,
            preview.source_width,
            preview.source_height
        ),
        (192, 112, 192, 112)
    );
    assert!(!preview.tone_mapped);
    let pixels = preview_pixels(&directory.0.join("preview.png"), &preview.image_data_url).await;
    assert_eq!(pixels.len(), 192 * 112 * 3);
    assert!(pixels[..3].iter().all(|channel| *channel <= 2));
    let middle = &pixels[(56 * 192 + 96) * 3..][..3];
    assert!(
        middle[2] > 200 && middle[0] < 40,
        "selected second video must be blue: {middle:?}"
    );
    let crop = detect_crop(
        AutoCropRequest {
            input_path: request.input_path.clone(),
            video_stream_index: 1,
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        crop.crop,
        Some(CropSettings {
            left: 16,
            right: 16,
            top: 16,
            bottom: 16
        })
    );
    assert_eq!(crop.agreement_percent, 100);
    assert!(crop.sampled_frames > 0 && crop.sampled_frames <= 60);
    assert_eq!(crop.source_fingerprint, preview.source_fingerprint);
    let first = detect_crop(
        AutoCropRequest {
            input_path: request.input_path.clone(),
            video_stream_index: 0,
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        first.crop,
        Some(CropSettings {
            left: 32,
            right: 32,
            top: 16,
            bottom: 16
        })
    );
    let missing = preview_frame(
        FramePreviewRequest {
            video_stream_index: 99,
            ..request
        },
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code, "STREAM_SELECTION_INVALID");
    assert_eq!(Sha256::digest(std::fs::read(&source).unwrap()), before);
    let after = std::fs::metadata(&source).unwrap();
    assert_eq!(after.len(), before_metadata.len());
    assert_eq!(
        after.modified().unwrap(),
        before_metadata.modified().unwrap()
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg with zscale/tonemap and FFprobe"]
async fn hdr_display_preview_is_tone_mapped_and_black_samples_do_not_propose_a_crop() {
    let directory = Fixture::new();
    let hdr = directory.0.join("hdr.mkv");
    fixture(&hdr, false, true, false).await;
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let preview = preview_frame(
        FramePreviewRequest {
            input_path: hdr.to_string_lossy().into_owned(),
            display_orientation: None,
            video_stream_index: 0,
            position_seconds: 0.2,
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(preview.tone_mapped);
    let pixels = preview_pixels(
        &directory.0.join("hdr-preview.png"),
        &preview.image_data_url,
    )
    .await;
    assert!(
        pixels[..3].iter().all(|channel| *channel <= 2),
        "HDR borders remain black"
    );
    assert!(
        pixels[(56 * 192 + 96) * 3..][..3]
            .iter()
            .any(|channel| *channel > 20),
        "HDR picture remains visible"
    );
    let black = directory.0.join("black.mkv");
    fixture(&black, false, false, true).await;
    let crop = detect_crop(
        AutoCropRequest {
            input_path: black.to_string_lossy().into_owned(),
            video_stream_index: 0,
        },
        cancel,
    )
    .await
    .unwrap();
    assert!(
        crop.crop.is_none(),
        "all-black frames must not remove the picture: {crop:?}"
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn active_analysis_cancellation_returns_promptly_and_releases_source_handles() {
    let directory = Fixture::new();
    let source = directory.0.join("cancel.mkv");
    fixture(&source, false, false, false).await;
    let request = AutoCropRequest {
        input_path: source.to_string_lossy().into_owned(),
        video_stream_index: 0,
    };
    let (owner, cancel) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(detect_crop(request, cancel));
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert!(
        !task.is_finished(),
        "fixture should still be analyzing before cancellation"
    );
    let start = std::time::Instant::now();
    owner.send(true).unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.code, "ANALYSIS_CANCELLED");
    assert!(start.elapsed() < Duration::from_secs(3));
    // Windows will refuse this while the read-only source guard or owned tool
    // still has the source open. The fixture file is safe to rename on all OSes.
    std::fs::rename(&source, directory.0.join("released.mkv")).unwrap();
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn inspector_thumbnail_honors_rotation_and_sar_without_changing_coded_crop_coordinates() {
    let directory = Fixture::new();
    let original = directory.0.join("anamorphic.mp4");
    let rotated = directory.0.join("rotated.mp4");
    let mut command = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=1",
        "-vf",
        "setsar=2/1",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "0",
    ]);
    command.push(original.as_os_str().to_owned());
    ffmpeg(command).await;
    let mut command = args(&["-v", "error", "-display_rotation:v:0", "90", "-i"]);
    command.push(original.as_os_str().to_owned());
    command.extend(args(&["-c", "copy"]));
    command.push(rotated.as_os_str().to_owned());
    ffmpeg(command).await;
    let before = Sha256::digest(std::fs::read(&rotated).unwrap());
    let source = media_runtime::probe_media(rotated.to_string_lossy().into_owned())
        .await
        .unwrap();
    assert_eq!(
        source.streams[0].sample_aspect_ratio.as_deref(),
        Some("2:1")
    );
    assert_eq!(source.streams[0].rotation_degrees.as_deref(), Some("90"));
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let request = FramePreviewRequest {
        input_path: rotated.to_string_lossy().into_owned(),
        video_stream_index: 0,
        position_seconds: 0.0,
        display_orientation: Some(true),
    };
    let preview = preview_frame(request.clone(), cancel.clone())
        .await
        .unwrap();
    assert_eq!((preview.width, preview.height), (112, 384));
    assert_eq!((preview.source_width, preview.source_height), (192, 112));
    let actual = preview_pixels(&directory.0.join("display.png"), &preview.image_data_url).await;
    // The reference lets FFmpeg read the display matrix itself; the app builds
    // an explicit transform while keeping autorotation disabled for crop work.
    let mut reference = args(&["-v", "error", "-i"]);
    reference.push(rotated.as_os_str().to_owned());
    reference.extend(args(&[
        "-vf",
        "scale=112:384:flags=lanczos,format=rgb24,setsar=1",
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-",
    ]));
    assert_eq!(
        Sha256::digest(actual),
        Sha256::digest(ffmpeg(reference).await)
    );
    let coded = preview_frame(
        FramePreviewRequest {
            display_orientation: None,
            ..request
        },
        cancel,
    )
    .await
    .unwrap();
    assert_eq!((coded.width, coded.height), (192, 112));
    assert_eq!(Sha256::digest(std::fs::read(&rotated).unwrap()), before);
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn inspector_thumbnail_matches_autorotation_for_reflected_display_matrices() {
    let directory = Fixture::new();
    let original = directory.0.join("asymmetric.mp4");
    let mut command = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=1",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "0",
    ]);
    command.push(original.as_os_str().to_owned());
    ffmpeg(command).await;
    let (_owner, cancel) = tokio::sync::watch::channel(false);

    for (name, rotation, flip) in [
        ("horizontal", "0", "-display_hflip:v:0"),
        ("vertical", "0", "-display_vflip:v:0"),
        ("reflected-quarter", "90", "-display_hflip:v:0"),
        ("reflected-three-quarter", "270", "-display_hflip:v:0"),
    ] {
        let source = directory.0.join(format!("{name}.mp4"));
        let mut command = args(&["-v", "error", "-display_rotation:v:0", rotation, flip, "-i"]);
        command.push(original.as_os_str().to_owned());
        command.extend(args(&["-c", "copy"]));
        command.push(source.as_os_str().to_owned());
        ffmpeg(command).await;
        let before = Sha256::digest(std::fs::read(&source).unwrap());
        let request = FramePreviewRequest {
            input_path: source.to_string_lossy().into_owned(),
            video_stream_index: 0,
            position_seconds: 0.0,
            display_orientation: Some(true),
        };
        let preview = preview_frame(request.clone(), cancel.clone())
            .await
            .unwrap();
        let actual = preview_pixels(
            &directory.0.join(format!("{name}-preview.png")),
            &preview.image_data_url,
        )
        .await;
        let mut reference = args(&["-v", "error", "-i"]);
        reference.push(source.as_os_str().to_owned());
        reference.extend(args(&[
            "-vf",
            &format!(
                "scale={}:{}:flags=lanczos,format=rgb24,setsar=1",
                preview.width, preview.height
            ),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ]));
        assert_eq!(
            Sha256::digest(actual),
            Sha256::digest(ffmpeg(reference).await),
            "{name} display pixels differ from FFmpeg autorotation"
        );
        let coded = preview_frame(
            FramePreviewRequest {
                display_orientation: None,
                ..request
            },
            cancel.clone(),
        )
        .await
        .unwrap();
        assert_eq!((coded.width, coded.height), (192, 112));
        assert_eq!(Sha256::digest(std::fs::read(&source).unwrap()), before);
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn translated_mp4_display_matrix_keeps_the_same_oriented_pixels() {
    let directory = Fixture::new();
    let original = directory.0.join("original.mp4");
    let rotated = directory.0.join("rotated.mp4");
    let translated = directory.0.join("translated.mp4");
    let mut command = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=1",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "0",
    ]);
    command.push(original.as_os_str().to_owned());
    ffmpeg(command).await;
    let mut command = args(&["-v", "error", "-display_rotation:v:0", "90", "-i"]);
    command.push(original.as_os_str().to_owned());
    command.extend(args(&["-c", "copy"]));
    command.push(rotated.as_os_str().to_owned());
    ffmpeg(command).await;
    std::fs::copy(&rotated, &translated).unwrap();

    // The MP4 tkhd box stores this unique 3x3 display matrix as big-endian
    // fixed-point words. Change only its origin (the last row's first two words).
    let matrix = [0_i32, -65_536, 0, 65_536, 0, 0, 0, 0, 1_073_741_824];
    let pattern = matrix
        .iter()
        .flat_map(|entry| entry.to_be_bytes())
        .collect::<Vec<_>>();
    let mut bytes = std::fs::read(&translated).unwrap();
    let positions = bytes
        .windows(pattern.len())
        .enumerate()
        .filter_map(|(index, window)| (window == pattern).then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(positions.len(), 1, "expected one MP4 track display matrix");
    let offset = positions[0];
    bytes[offset + 24..offset + 28].copy_from_slice(&(160_i32 << 16).to_be_bytes());
    bytes[offset + 28..offset + 32].copy_from_slice(&(-96_i32 << 16).to_be_bytes());
    std::fs::write(&translated, bytes).unwrap();
    let before = Sha256::digest(std::fs::read(&translated).unwrap());

    let reference_pixels = |path: &Path| {
        let mut command = args(&["-v", "error", "-i"]);
        command.push(path.as_os_str().to_owned());
        command.extend(args(&[
            "-vf",
            "scale=112:192:flags=lanczos,format=rgb24,setsar=1",
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ]));
        command
    };
    let unshifted = ffmpeg(reference_pixels(&rotated)).await;
    let expected = ffmpeg(reference_pixels(&translated)).await;
    assert_eq!(Sha256::digest(unshifted), Sha256::digest(&expected));

    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let preview = preview_frame(
        FramePreviewRequest {
            input_path: translated.to_string_lossy().into_owned(),
            video_stream_index: 0,
            position_seconds: 0.0,
            display_orientation: Some(true),
        },
        cancel,
    )
    .await
    .unwrap();
    assert_eq!((preview.width, preview.height), (112, 192));
    let actual = preview_pixels(
        &directory.0.join("translated-preview.png"),
        &preview.image_data_url,
    )
    .await;
    assert_eq!(Sha256::digest(actual), Sha256::digest(expected));
    assert_eq!(Sha256::digest(std::fs::read(&translated).unwrap()), before);
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn selected_matroska_stream_duration_rejects_seeks_before_container_end() {
    let directory = Fixture::new();
    let source = directory.0.join("different-stream-durations.mkv");
    let mut command = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=3",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=128x96:r=24:d=1",
        "-map",
        "0:v",
        "-map",
        "1:v",
        "-c:v",
        "ffv1",
        "-level",
        "3",
    ]);
    command.push(source.as_os_str().to_owned());
    ffmpeg(command).await;
    let before = Sha256::digest(std::fs::read(&source).unwrap());
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let request = FramePreviewRequest {
        input_path: source.to_string_lossy().into_owned(),
        video_stream_index: 1,
        position_seconds: 0.5,
        display_orientation: None,
    };
    preview_frame(request.clone(), cancel.clone())
        .await
        .unwrap();
    let error = preview_frame(
        FramePreviewRequest {
            position_seconds: 1.5,
            ..request
        },
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "ANALYSIS_SETTINGS_INVALID");
    assert_eq!(Sha256::digest(std::fs::read(&source).unwrap()), before);
}
