//! Folder sources are immutable ordered stills, not recursive media scans or exports.
use media_core::{
    AutoCropRequest, EncodeRequest, EncodeSettings, FramePreviewRequest, FrameRate,
    ImageSequenceImportRequest, ImageSequenceReleaseRequest, JobState, MediaFile, MediaSource,
    RemuxRequest, VideoEncoder, VideoTrim,
};
use media_runtime::{
    jobs::JobManager,
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
        let path = std::env::temp_dir().join(format!(
            "jesses-sequence-source-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn cache(&self) -> PathBuf {
        self.0.join("cache")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Retained {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(Into::into).collect()
}
async fn run(tool: &str, args: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = watch::channel(false);
    let result = run_capture(
        &CommandSpec {
            executable: tool.into(),
            args,
            cwd: None,
        },
        cancel,
        4 * 1024 * 1024,
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
async fn make(path: &Path, value: u16, alpha: bool, high_depth: bool) {
    let raw = path.with_extension("pam");
    let mut bytes = format!(
        "P7\nWIDTH 64\nHEIGHT 64\nDEPTH {}\nMAXVAL {}\nTUPLTYPE {}\nENDHDR\n",
        if alpha { 4 } else { 3 },
        if high_depth { 65535 } else { 255 },
        if alpha { "RGB_ALPHA" } else { "RGB" }
    )
    .into_bytes();
    for y in 0..64 {
        for x in 0..64 {
            let active = (8..56).contains(&x) && (8..56).contains(&y);
            let scale = if high_depth { 257 } else { 1 };
            let values = [
                if active { value } else { 0 },
                if active { (x * 3) as u16 * scale } else { 0 },
                if active { (y * 3) as u16 * scale } else { 0 },
                if high_depth {
                    (x * 997) as u16
                } else {
                    (x * 4) as u16
                },
            ];
            for channel in &values[..if alpha { 4 } else { 3 }] {
                if high_depth {
                    bytes.extend(channel.to_be_bytes());
                } else {
                    bytes.push(*channel as u8);
                }
            }
        }
    }
    std::fs::write(&raw, bytes).unwrap();
    let mut a = args(&["-v", "error", "-i"]);
    a.push(raw.as_os_str().to_owned());
    if path.extension().unwrap() == "jpg" {
        a.extend(args(&[
            "-c:v", "mjpeg", "-q:v", "3", "-pix_fmt", "yuvj444p",
        ]));
    } else {
        a.extend(args(&[
            "-c:v",
            "png",
            "-pix_fmt",
            if high_depth && alpha {
                "rgba64be"
            } else if alpha {
                "rgba"
            } else if high_depth {
                "rgb48be"
            } else {
                "rgb24"
            },
        ]));
    }
    a.extend(args(&["-frames:v", "1"]));
    a.push(path.as_os_str().to_owned());
    run("ffmpeg", a).await;
    std::fs::remove_file(raw).unwrap();
}
async fn hashes(path: &Path, pixel: &str) -> Vec<String> {
    let mut a = args(&["-v", "error", "-i"]);
    a.push(path.as_os_str().to_owned());
    a.extend(args(&[
        "-map", "0:v:0", "-pix_fmt", pixel, "-f", "framemd5", "-",
    ]));
    String::from_utf8(run("ffmpeg", a).await)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| line.rsplit(',').next().unwrap().trim().to_owned())
        .collect()
}
fn fingerprint(media: &MediaFile) -> String {
    match media.source.as_ref().unwrap() {
        MediaSource::ImageSequence {
            sequence_fingerprint,
            ..
        } => sequence_fingerprint.clone(),
    }
}
async fn import(folder: &Path, cache: PathBuf, rate: FrameRate) -> MediaFile {
    let (_owner, cancel) = watch::channel(false);
    media_runtime::import_image_sequence(
        ImageSequenceImportRequest {
            directory_path: folder.to_string_lossy().into_owned(),
            frame_rate: rate,
        },
        cache,
        cancel,
    )
    .await
    .unwrap()
}
fn release(media: &MediaFile, cache: PathBuf) -> Result<(), media_core::AppError> {
    media_runtime::release_image_sequence(
        ImageSequenceReleaseRequest {
            input_path: media.path.clone(),
            sequence_fingerprint: fingerprint(media),
        },
        cache,
    )
}
async fn finish(manager: &JobManager, id: &str) -> media_core::JobSnapshot {
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe PNG16 alpha and JPEG"]
async fn nonsequential_png_alpha_and_jpeg_names_keep_order_fps_and_normal_previews() {
    for (extension, alpha, high_depth, rate) in [
        (
            "png",
            true,
            true,
            FrameRate {
                numerator: 30000,
                denominator: 1001,
            },
        ),
        (
            "jpg",
            false,
            false,
            FrameRate {
                numerator: 12,
                denominator: 1,
            },
        ),
    ] {
        let fixture = Fixture::new();
        let folder = fixture.0.join("Original pictures");
        std::fs::create_dir(&folder).unwrap();
        std::fs::create_dir(folder.join("nested")).unwrap();
        std::fs::write(folder.join("0-readme.txt"), b"not a frame").unwrap();
        for (name, value) in [("shot-2", 91), ("a-first", 43), ("shot-10", 152)] {
            make(
                &folder.join(format!("{name}.{extension}")),
                if high_depth { value * 257 + 13 } else { value },
                alpha,
                high_depth,
            )
            .await;
        }
        make(&folder.join("nested/ignored.png"), 29, false, false).await;
        let paths =
            ["a-first", "shot-10", "shot-2"].map(|name| folder.join(format!("{name}.{extension}")));
        let original = paths
            .iter()
            .map(std::fs::read)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let pixel = if high_depth { "rgba64le" } else { "yuvj444p" };
        let mut reference = Vec::new();
        for path in &paths {
            reference.extend(hashes(path, pixel).await);
        }
        std::fs::create_dir(fixture.cache()).unwrap();
        std::fs::write(fixture.cache().join("personal-save.txt"), b"unrelated").unwrap();
        let media = import(&folder, fixture.cache(), rate).await;
        assert_eq!(media.name, "Original pictures");
        assert_eq!(media.modified_at_ms, None);
        assert_eq!(media.streams.len(), 1);
        assert_eq!(
            media.streams[0].codec.as_deref(),
            Some(if extension == "png" { "png" } else { "mjpeg" })
        );
        assert_eq!(media.bit_rate, None);
        assert_eq!(media.streams[0].bit_rate, None);
        assert_eq!(
            media.size_bytes,
            original
                .iter()
                .map(|bytes| bytes.len() as u64)
                .sum::<u64>()
                .to_string()
        );
        let (_owner, cancel) = watch::channel(false);
        assert_eq!(
            media_runtime::analyze_bitrate(
                media_core::BitrateRequest {
                    input_path: media.path.clone(),
                    stream_index: 0,
                    window_seconds: 1.0
                },
                cancel
            )
            .await
            .unwrap_err()
            .code,
            "IMAGE_SEQUENCE_UNSUPPORTED"
        );
        assert_eq!(media.streams[0].width, Some(64));
        assert_eq!(media.streams[0].height, Some(64));
        assert_eq!(
            media.streams[0].bit_depth,
            Some(if high_depth { 16 } else { 8 })
        );
        assert!(
            (media.duration_seconds.unwrap()
                - 3.0 * f64::from(rate.denominator) / f64::from(rate.numerator))
            .abs()
                < 1e-9
        );
        assert_eq!(hashes(Path::new(&media.path), pixel).await, reference);
        let MediaSource::ImageSequence {
            directory_path,
            frame_rate,
            extension: chosen,
            frame_count,
            ..
        } = media.source.as_ref().unwrap();
        assert_eq!(Path::new(directory_path), folder);
        assert_eq!(*frame_rate, rate);
        assert_eq!(chosen, extension);
        assert_eq!(*frame_count, 3);
        let restored = media_runtime::probe_media(media.path.clone())
            .await
            .unwrap();
        assert_eq!(restored.source, media.source);
        assert_eq!(restored.id, media.id);
        let (_owner, cancel) = watch::channel(false);
        let preview = media_runtime::preview_frame(
            FramePreviewRequest {
                input_path: media.path.clone(),
                video_stream_index: 0,
                position_seconds: 0.0,
                display_orientation: Some(true),
            },
            cancel,
        )
        .await
        .unwrap();
        assert!(preview.image_data_url.starts_with("data:image/png;base64,"));
        assert_eq!((preview.source_width, preview.source_height), (64, 64));
        let (_owner, cancel) = watch::channel(false);
        let crop = media_runtime::detect_crop(
            AutoCropRequest {
                input_path: media.path.clone(),
                video_stream_index: 0,
            },
            cancel,
        )
        .await
        .unwrap();
        assert_eq!((crop.source_width, crop.source_height), (64, 64));
        if !alpha {
            let manager = JobManager::new(fixture.0.join("logs"));
            let output = fixture.0.join("trimmed.mkv");
            let job = manager
                .start_encode(EncodeRequest {
                    source: RemuxRequest {
                        input_path: media.path.clone(),
                        output_path: output.to_string_lossy().into_owned(),
                        stream_indices: vec![0],
                    },
                    settings: EncodeSettings {
                        encoder: VideoEncoder::X264,
                        preset: 0,
                        lossless: true,
                        output_pixel_format: Some(media_core::Av1anPixelFormat::Yuv444p),
                        trim: Some(VideoTrim {
                            start_frame: 1,
                            end_frame_exclusive: 3,
                            time: None,
                        }),
                        ..Default::default()
                    },
                })
                .await
                .unwrap();
            let done = finish(&manager, &job.id).await;
            assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
            assert_eq!(hashes(&output, pixel).await, reference[1..]);
            manager.shutdown().await;
        }
        release(&media, fixture.cache()).unwrap();
        assert!(!Path::new(&media.path).exists());
        assert_eq!(
            std::fs::read(fixture.cache().join("personal-save.txt")).unwrap(),
            b"unrelated"
        );
        for (path, bytes) in paths.iter().zip(original) {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe MJPEG full-range 420/422/444 and FFV1"]
async fn jpeg_full_range_samples_survive_every_supported_chroma_layout() {
    for layout in ["yuvj420p", "yuvj422p", "yuvj444p"] {
        let fixture = Fixture::new();
        let folder = fixture.0.join("JPEG originals");
        std::fs::create_dir(&folder).unwrap();
        let mut create = args(&[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=4:duration=0.75",
            "-c:v",
            "mjpeg",
            "-q:v",
            "3",
            "-pix_fmt",
            layout,
            "-threads",
            "2",
        ]);
        create.push(folder.join("frame-%d.jpg").into_os_string());
        run("ffmpeg", create).await;
        let images: Vec<_> = (1..=3)
            .map(|index| folder.join(format!("frame-{index}.jpg")))
            .collect();
        let originals: Vec<_> = images
            .iter()
            .map(std::fs::read)
            .collect::<Result<_, _>>()
            .unwrap();
        let mut expected = Vec::new();
        for image in &images {
            expected.extend(hashes(image, layout).await);
        }
        let media = import(
            &folder,
            fixture.cache(),
            FrameRate {
                numerator: 4,
                denominator: 1,
            },
        )
        .await;
        assert_eq!(
            hashes(Path::new(&media.path), layout).await,
            expected,
            "Full-range samples changed for {layout}"
        );
        for (image, original) in images.iter().zip(&originals) {
            assert_eq!(&std::fs::read(image).unwrap(), original);
        }
        release(&media, fixture.cache()).unwrap();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires qualified FFmpeg/FFprobe"]
async fn changed_frames_foreign_cache_entries_and_active_cancellation_preserve_originals() {
    let fixture = Fixture::new();
    let folder = fixture.0.join("frames");
    std::fs::create_dir(&folder).unwrap();
    let first = folder.join("not-a-number.png");
    make(&first, 95, true, false).await;
    let original = std::fs::read(&first).unwrap();
    let media = import(
        &folder,
        fixture.cache(),
        FrameRate {
            numerator: 30,
            denominator: 1,
        },
    )
    .await;
    let error = media_runtime::release_image_sequence(
        ImageSequenceReleaseRequest {
            input_path: media.path.clone(),
            sequence_fingerprint: "forged".into(),
        },
        fixture.cache(),
    )
    .unwrap_err();
    assert_eq!(error.code, "IMAGE_SEQUENCE_CHANGED");
    assert!(Path::new(&media.path).is_file());
    let foreign = Path::new(&media.path)
        .parent()
        .unwrap()
        .join("personal-save.txt");
    std::fs::write(&foreign, b"foreign data").unwrap();
    assert!(release(&media, fixture.cache()).is_err());
    assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign data");
    std::fs::remove_file(foreign).unwrap();
    let mut changed = original.clone();
    let at = changed.len() / 2;
    changed[at] ^= 1;
    std::fs::write(&first, &changed).unwrap();
    let error = media_runtime::probe_media(media.path.clone())
        .await
        .unwrap_err();
    assert_eq!(error.code, "IMAGE_SEQUENCE_CHANGED");
    release(&media, fixture.cache()).unwrap();
    assert_eq!(std::fs::read(&first).unwrap(), changed);
    std::fs::write(&first, &original).unwrap();
    for index in 0..255 {
        std::fs::copy(&first, folder.join(format!("arbitrary-{index:04}.png"))).unwrap();
    }
    let cache = fixture.cache();
    let (owner, cancel) = watch::channel(false);
    let request = ImageSequenceImportRequest {
        directory_path: folder.to_string_lossy().into_owned(),
        frame_rate: FrameRate {
            numerator: 30,
            denominator: 1,
        },
    };
    let running = tokio::spawn(media_runtime::import_image_sequence(
        request,
        cache.clone(),
        cancel,
    ));
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let staged = std::fs::read_dir(&cache)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".jesses-images-")
                        && std::fs::read_dir(entry.path()).is_ok_and(|files| files.count() >= 8)
                });
            if staged {
                owner.send(true).unwrap();
                break;
            }
            assert!(
                !running.is_finished(),
                "import finished before cancellation hook"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(running.await.unwrap().is_err());
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
    assert_eq!(std::fs::read(first).unwrap(), original);
    assert_eq!(std::fs::read_dir(folder).unwrap().count(), 256);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires qualified FFmpeg/FFprobe and standalone x264"]
async fn native_stop_restart_resume_pins_sequence_images_and_blocks_recovery_cache_release() {
    let fixture = Fixture::new();
    let folder = fixture.0.join("Sequence for recovery");
    std::fs::create_dir(&folder).unwrap();
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=640x360:rate=24:duration=4",
        "-c:v",
        "png",
        "-threads",
        "2",
    ]);
    a.push(folder.join("frame-%04d.png").into_os_string());
    run("ffmpeg", a).await;
    let media = import(
        &folder,
        fixture.cache(),
        FrameRate {
            numerator: 24,
            denominator: 1,
        },
    )
    .await;
    let output = fixture.0.join("resumed.mkv");
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: media.path.clone(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::X264,
                preset: 9,
                rate_control: Some(media_core::VideoRateControl::Bitrate {
                    bitrate_kbps: 500,
                    two_pass: true,
                }),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let release_request = ImageSequenceReleaseRequest {
        input_path: media.path.clone(),
        sequence_fingerprint: fingerprint(&media),
    };
    assert_eq!(
        manager
            .release_image_sequence(release_request.clone(), fixture.cache())
            .await
            .unwrap_err()
            .code,
        "IMAGE_SEQUENCE_BUSY"
    );
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|snapshot| snapshot.id == job.id)
                .unwrap();
            assert!(!snapshot.state.is_terminal(), "{snapshot:#?}");
            if snapshot.standalone_recovery.as_ref().is_some_and(|saved| {
                saved.phase == media_core::StandaloneRecoveryPhase::PassOneComplete
            }) {
                manager.stop_job(job.id.clone()).await.unwrap();
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let stopped = finish(&manager, &job.id).await;
    assert_eq!(stopped.state, JobState::Stopped, "{stopped:#?}");
    manager.shutdown().await;
    drop(manager);
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    assert_eq!(
        manager
            .release_image_sequence(release_request.clone(), fixture.cache())
            .await
            .unwrap_err()
            .code,
        "IMAGE_SEQUENCE_BUSY"
    );
    let image = folder.join("frame-0001.png");
    let bytes = std::fs::read(&image).unwrap();
    let modified = std::fs::metadata(&image).unwrap().modified().unwrap();
    let mut changed = bytes.clone();
    let at = changed.len() / 2;
    changed[at] ^= 1;
    std::fs::write(&image, changed).unwrap();
    match manager.resume_job(job.id.clone()).await {
        Ok(_) => {
            let failed = finish(&manager, &job.id).await;
            assert_eq!(failed.state, JobState::Failed);
            assert_eq!(failed.error.unwrap().code, "IMAGE_SEQUENCE_CHANGED");
        }
        Err(error) => assert_eq!(error.code, "IMAGE_SEQUENCE_CHANGED"),
    };
    assert!(!output.exists());
    manager.shutdown().await;
    drop(manager);
    std::fs::write(&image, &bytes).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&image)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.resume_job(job.id.clone()).await.unwrap();
    let done = finish(&manager, &job.id).await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    assert!(
        done.logs
            .iter()
            .any(|line| line.contains("Resuming after verified pass one")),
        "{done:#?}"
    );
    assert_eq!(
        done.input_size_bytes.as_deref(),
        Some(media.size_bytes.as_str())
    );
    assert_eq!(hashes(&output, "yuv420p").await.len(), 96);
    assert_eq!(std::fs::read(&image).unwrap(), bytes);
    manager
        .release_image_sequence(release_request, fixture.cache())
        .await
        .unwrap();
    manager.shutdown().await;
    assert!(!Path::new(&media.path).exists());
    assert_eq!(std::fs::read_dir(folder).unwrap().count(), 96);
}
