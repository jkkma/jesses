//! Shared source BatchNaming/UiData behavior through the public native APIs.
use media_core::*;
use media_runtime::{JobManager, resolve_batch_output_paths};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/source-batch-naming")
            .join(format!(
                "fixture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn source(&self, directory: &str, name: &str) -> PathBuf {
        let directory = self.0.join(directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(name);
        fs::write(&path, b"original source").unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Retained source naming fixture {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn request(paths: &[PathBuf], target: OutputTarget) -> BatchOutputPathRequest {
    BatchOutputPathRequest {
        inputs: paths
            .iter()
            .map(|path| BatchOutputPathInput {
                input_path: path.to_string_lossy().into_owned(),
                width: Some(1920),
                height: Some(1080),
            })
            .collect(),
        target,
        naming: BatchNamingValues {
            codec: "av1".into(),
            quality: "50".into(),
            preset: "4".into(),
        },
        output_name_template: Some("{name}".into()),
        naming_date: Some("2026-11-01".into()),
        default_output_directory: None,
    }
}
fn paths(result: BatchOutputPaths) -> Vec<PathBuf> {
    result
        .items
        .into_iter()
        .map(|item| {
            assert!(item.error.is_none(), "{item:?}");
            PathBuf::from(item.resolution.unwrap().output_path)
        })
        .collect()
}

#[tokio::test]
async fn defaults_fall_back_per_source_and_shared_folder_reserves_files_directories_and_links() {
    let fixture = Fixture::new();
    let inputs = [
        fixture.source("one", "movie.part.one.mkv"),
        fixture.source("two", "movie.part.one.mkv"),
    ];
    let mut naming = request(
        &inputs,
        OutputTarget::Container {
            format: ContainerFormat::Matroska,
        },
    );
    naming.default_output_directory =
        Some(fixture.0.join("missing").to_string_lossy().into_owned());
    let beside = paths(resolve_batch_output_paths(naming.clone()).await.unwrap());
    assert_eq!(
        beside,
        vec![
            fixture.0.join("one/movie.part.one (1).mkv"),
            fixture.0.join("two/movie.part.one (1).mkv")
        ]
    );
    let shared = fixture.0.join("shared");
    fs::create_dir(&shared).unwrap();
    fs::write(shared.join("movie.part.one.mkv"), b"occupied").unwrap();
    fs::create_dir(shared.join("movie.part.one (1).mkv")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(shared.join("gone"), shared.join("movie.part.one (2).mkv")).unwrap();
    naming.default_output_directory = Some(format!(" \"{}\" ", shared.display()));
    let outputs = paths(resolve_batch_output_paths(naming.clone()).await.unwrap());
    let first = if cfg!(unix) { 3 } else { 2 };
    assert_eq!(
        outputs,
        vec![
            shared.join(format!("movie.part.one ({first}).mkv")),
            shared.join(format!("movie.part.one ({}).mkv", first + 1))
        ]
    );
    assert!(outputs.iter().all(|path| !path.exists()));
    assert_eq!(
        fs::read(shared.join("movie.part.one.mkv")).unwrap(),
        b"occupied"
    );
    for input in inputs {
        assert_eq!(fs::read(input).unwrap(), b"original source");
    }
}

#[tokio::test]
async fn formats_tokens_and_partial_failures_share_one_frozen_naming_snapshot() {
    let fixture = Fixture::new();
    let input = fixture.source("one", "Episode.One.MKV");
    let targets = [
        (
            OutputTarget::Container {
                format: ContainerFormat::Mp4,
            },
            Some("mp4"),
        ),
        (
            OutputTarget::Image {
                format: ImageOutput::Gif,
            },
            Some("gif"),
        ),
        (
            OutputTarget::Image {
                format: ImageOutput::PngSequence,
            },
            None,
        ),
        (
            OutputTarget::Image {
                format: ImageOutput::JpegSequence,
            },
            None,
        ),
    ];
    for (target, extension) in targets {
        let mut naming = request(&[input.clone(), input.clone()], target);
        naming.naming = BatchNamingValues {
            codec: "gif".into(),
            quality: "128".into(),
            preset: String::new(),
        };
        naming.output_name_template = Some(" {NaMe}_{EXT}_{CoDeC}_{crf}_{QUALITY}_{PRESET}_{width}x{height}_{DATE}_{unknown}/\\:*?\"<>|. ".into());
        let outputs = paths(resolve_batch_output_paths(naming).await.unwrap());
        let stem = "Episode.One_MKV_gif_128_128__1920x1080_2026-11-01_{unknown}_________";
        let name = |number: Option<u32>| {
            format!(
                "{stem}{}{}",
                number.map_or_else(String::new, |number| format!(" ({number})")),
                extension.map_or_else(String::new, |value| format!(".{value}"))
            )
        };
        assert_eq!(
            outputs,
            vec![
                input.parent().unwrap().join(name(None)),
                input.parent().unwrap().join(name(Some(1)))
            ]
        );
        assert!(outputs.iter().all(|path| !path.exists()));
    }
    let mut naming = request(
        &[fixture.0.join("absent.mkv"), input.clone()],
        OutputTarget::Image {
            format: ImageOutput::Gif,
        },
    );
    naming.output_name_template = Some("{index}_{width}_{height}".into());
    naming.inputs[1].width = None;
    naming.inputs[1].height = None;
    let result = resolve_batch_output_paths(naming).await.unwrap();
    assert_eq!(result.items.len(), 2);
    assert!(result.items[0].error.is_some());
    assert!(result.items[0].resolution.is_none());
    assert!(
        result.items[1]
            .resolution
            .as_ref()
            .unwrap()
            .output_path
            .ends_with("2__.gif")
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe for actual source inspection and folder import"]
async fn encoded_preview_uses_first_video_and_source_defaults_and_queue_rejects_later_collision() {
    let fixture = Fixture::new();
    let first = fixture.0.join("one");
    let second = fixture.0.join("two");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let input = first.join("video.part.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "color=size=32x24:rate=4:duration=1",
            "-f",
            "lavfi",
            "-i",
            "color=size=64x48:rate=4:duration=1",
            "-map",
            "0:v",
            "-map",
            "1:v",
            "-c:v",
            "ffv1",
            "-threads",
            "1",
        ])
        .arg(&input)
        .status()
        .unwrap();
    assert!(status.success());
    let other = second.join("video.part.mkv");
    fs::copy(&input, &other).unwrap();
    let original = fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    manager.ready().await.unwrap();
    let batch: BatchEncodeRequest = serde_json::from_value(serde_json::json!({
        "inputs": [
            {"inputPath":input,"streamIndices":[1],"videoStreamIndex":1},
            {"inputPath":other,"streamIndices":[1],"videoStreamIndex":1}
        ],
        "outputDirectory":"",
        "outputContainer":"matroska",
        "outputNameTemplate":"{name}_{width}x{height}_{codec}_{crf}_{quality}_{preset}",
        "sourceNaming":{"codec":"copy","quality":"50","preset":""},
        "videoMode":"copy", "crf":30, "preset":4
    }))
    .unwrap();
    let preview = manager.preview_encode_batch(batch.clone()).await.unwrap();
    assert!(
        preview.items.iter().all(|item| item.error.is_none()),
        "{preview:?}"
    );
    assert_eq!(
        preview.items[0].output_path.as_deref(),
        first.join("video.part_32x24_copy_50_50_.mkv").to_str()
    );
    assert_eq!(
        preview.items[1].output_path.as_deref(),
        second.join("video.part_32x24_copy_50_50_.mkv").to_str()
    );
    let proposed = preview.items[0].request.clone().unwrap();
    fs::write(&proposed.source.output_path, b"later owner").unwrap();
    let error = manager
        .enqueue_encode_batch(vec![proposed.clone()])
        .await
        .unwrap_err();
    assert_eq!(error.code, "OUTPUT_EXISTS");
    assert_eq!(
        fs::read(&proposed.source.output_path).unwrap(),
        b"later owner"
    );
    assert!(manager.list_jobs().await.is_empty());
    assert_eq!(fs::read(&input).unwrap(), original);

    // A managed image sequence names its original dotted folder, never the
    // private lossless processing file; {ext} is empty for that folder.
    let folder = fixture.0.join("Frames.v1");
    fs::create_dir(&folder).unwrap();
    assert!(
        std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "color=size=32x24",
                "-frames:v",
                "1",
                "-threads",
                "1"
            ])
            .arg(folder.join("frame.png"))
            .status()
            .unwrap()
            .success()
    );
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let imported = media_runtime::jobs::import_image_sequence(
        ImageSequenceImportRequest {
            directory_path: folder.to_string_lossy().into_owned(),
            frame_rate: FrameRate {
                numerator: 30,
                denominator: 1,
            },
        },
        fixture.0.join("sequence-cache"),
        cancel,
    )
    .await
    .unwrap();
    let naming = request(
        &[PathBuf::from(&imported.path), PathBuf::from(&imported.path)],
        OutputTarget::Image {
            format: ImageOutput::PngSequence,
        },
    );
    let outputs = paths(resolve_batch_output_paths(naming).await.unwrap());
    assert_eq!(
        outputs,
        vec![
            fixture.0.join("Frames.v1 (1)"),
            fixture.0.join("Frames.v1 (2)")
        ]
    );
    let Some(MediaSource::ImageSequence {
        sequence_fingerprint,
        ..
    }) = imported.source
    else {
        panic!()
    };
    media_runtime::jobs::release_image_sequence(
        ImageSequenceReleaseRequest {
            input_path: imported.path,
            sequence_fingerprint,
        },
        fixture.0.join("sequence-cache"),
    )
    .unwrap();
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn source_naming_and_actual_preview_reserve_more_than_one_hundred_files() {
    let fixture = Fixture::new();
    let input = fixture.0.join("original.mkv");
    assert!(
        std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=32x24:rate=4:duration=1",
                "-c:v",
                "ffv1",
                "-threads",
                "1"
            ])
            .arg(&input)
            .status()
            .unwrap()
            .success()
    );
    let paths = vec![input.clone(); 101];
    let mut naming = request(
        &paths,
        OutputTarget::Container {
            format: ContainerFormat::Matroska,
        },
    );
    naming.output_name_template = Some("large".into());
    let named = resolve_batch_output_paths(naming).await.unwrap();
    assert_eq!(named.items.len(), 101);
    let names = named
        .items
        .iter()
        .map(|item| item.resolution.as_ref().unwrap().output_path.clone())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(names.len(), 101);
    assert!(
        names.contains(
            &fixture
                .0
                .join("large (100).mkv")
                .to_string_lossy()
                .into_owned()
        )
    );
    let inputs = (0..101)
        .map(|_| serde_json::json!({"inputPath":input,"streamIndices":[0],"videoStreamIndex":0}))
        .collect::<Vec<_>>();
    let request:BatchEncodeRequest=serde_json::from_value(serde_json::json!({"inputs":inputs,"outputDirectory":"","sourceNaming":{"codec":"Copy","quality":"N/A","preset":"N/A"},"outputNameTemplate":"large","videoMode":"copy","crf":30,"preset":4})).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let result = manager.preview_encode_batch(request.clone()).await.unwrap();
    assert_eq!(result.items.len(), 101);
    for item in result.items {
        assert!(item.error.is_none(), "{item:?}");
        assert!(names.contains(item.output_path.as_ref().unwrap()));
        assert_eq!(item.request.unwrap().settings.video_mode, VideoMode::Copy);
    }
    let mut legacy = request;
    legacy.source_naming = None;
    legacy.output_directory = fixture.0.to_string_lossy().into_owned();
    assert_eq!(
        manager.preview_encode_batch(legacy).await.unwrap_err().code,
        "BATCH_SIZE_INVALID"
    );
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and x264"]
async fn mixed_quick_batch_freezes_video_audio_and_subtitle_only_modes_before_inspection() {
    let fixture = Fixture::new();
    let video = fixture.0.join("video.mkv");
    let audio = fixture.0.join("audio.mka");
    let subtitle = fixture.0.join("subtitle.srt");
    assert!(
        std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=64x48:rate=4:duration=1",
                "-c:v",
                "ffv1",
                "-threads",
                "1"
            ])
            .arg(&video)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=sample_rate=48000:duration=1",
                "-c:a",
                "pcm_s16le"
            ])
            .arg(&audio)
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:00,500\nSubtitle only\n",
    )
    .unwrap();
    let request:BatchEncodeRequest=serde_json::from_value(serde_json::json!({"inputs":[
      {"inputPath":video,"streamIndices":[0],"videoStreamIndex":0},
      {"inputPath":audio,"streamIndices":[0],"videoStreamIndex":0,"videoMode":"drop","framing":{"crop":{"top":0,"bottom":0,"left":1000,"right":0}}},
      {"inputPath":subtitle,"streamIndices":[0],"videoStreamIndex":0,"videoMode":"drop","framing":{"crop":{"top":0,"bottom":0,"left":1000,"right":0}}}
    ],"outputDirectory":"","sourceNaming":{"codec":"H264","quality":"30","preset":"ultrafast"},"outputNameTemplate":"{name}-mixed","encoder":"x264","backend":"standalone","videoMode":"encode","crf":30,"preset":0,"outputPixelFormat":"yuv420p","av1anFilters":["eq=contrast=1"]})).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let preview = manager.preview_encode_batch(request.clone()).await.unwrap();
    assert_eq!(preview.items.len(), 3);
    for (index, item) in preview.items.into_iter().enumerate() {
        assert!(item.error.is_none(), "{item:?}");
        let request = item.request.unwrap();
        assert_eq!(
            request.settings.video_mode,
            if index == 0 {
                VideoMode::Encode
            } else {
                VideoMode::Drop
            }
        );
        if index != 0 {
            assert_eq!(request.settings.framing, VideoFraming::default());
            assert!(request.settings.output_pixel_format.is_none());
            assert!(request.settings.av1an_filters.is_empty());
            assert!(request.settings.temporal.is_none());
            assert!(request.settings.parameters.is_empty());
        }
        let output = request.source.output_path.clone();
        let job = manager.enqueue_encode(request).await.unwrap();
        let finished = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let job = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|candidate| candidate.id == job.id)
                    .unwrap();
                if job.state.is_terminal() {
                    break job;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
        let media = media_runtime::probe_media(output).await.unwrap();
        assert_eq!(media.streams.len(), 1);
        assert_eq!(media.streams[0].kind, ["video", "audio", "subtitle"][index]);
        assert_eq!(
            finished.encode_settings.as_ref().unwrap().video_mode,
            if index == 0 {
                VideoMode::Encode
            } else {
                VideoMode::Drop
            }
        );
    }
    let mut av1an = request;
    av1an.backend = EncodeBackend::Av1an;
    av1an.av1an_filters.clear();
    av1an.output_pixel_format = None;
    let result = manager.preview_encode_batch(av1an).await.unwrap();
    assert_eq!(
        result.items[1].error.as_ref().unwrap().code,
        "STREAM_SELECTION_INVALID"
    );
    assert_eq!(
        result.items[2].error.as_ref().unwrap().code,
        "STREAM_SELECTION_INVALID"
    );
    manager.shutdown().await;
}

#[cfg(unix)]
#[test]
#[ignore = "requires FFmpeg/FFprobe; launches isolated canceled-preflight process"]
fn cancel_source_batch_preview_during_actual_probe_isolated_process() {
    let fixture = Fixture::new();
    let wrapper = fixture.0.join("slow-ffprobe");
    let marker = fixture.0.join("probe-pid");
    let quote = |value: &Path| format!("'{}'", value.to_string_lossy().replace('\'', "'\"'\"'"));
    fs::write(&wrapper,format!("#!/bin/sh\nif [ \"$1\" != '-version' ] && mkdir {} 2>/dev/null; then\n printf '%s' \"$$\" > {}\n sleep 30\nfi\nexec /usr/bin/ffprobe \"$@\"\n",quote(&fixture.0.join("probe-once")),quote(&marker))).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cancel_source_batch_preview_actual_probe_child",
            "--ignored",
            "--nocapture",
        ])
        .env("JESSES_FFPROBE", wrapper)
        .env("JESSES_BATCH_PROBE_MARKER", marker)
        .status()
        .unwrap();
    assert!(result.success());
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "isolated child of actual canceled batch probe test"]
async fn cancel_source_batch_preview_actual_probe_child() {
    let Some(marker) = std::env::var_os("JESSES_BATCH_PROBE_MARKER").map(PathBuf::from) else {
        return;
    };
    let fixture = Fixture::new();
    let input = fixture.0.join("video.mkv");
    assert!(
        std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=32x24:rate=4:duration=1",
                "-c:v",
                "ffv1",
                "-threads",
                "1"
            ])
            .arg(&input)
            .status()
            .unwrap()
            .success()
    );
    let request:BatchEncodeRequest=serde_json::from_value(serde_json::json!({"inputs":[{"inputPath":input,"streamIndices":[0],"videoStreamIndex":0}],"outputDirectory":"","sourceNaming":{"codec":"Copy","quality":"N/A","preset":"N/A"},"outputNameTemplate":"canceled","videoMode":"copy","crf":30,"preset":4})).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let (_owner, already_canceled) = tokio::sync::watch::channel(true);
    assert_eq!(
        manager
            .preview_encode_batch_cancellable(request.clone(), already_canceled)
            .await
            .unwrap_err()
            .code,
        "BATCH_CANCELED"
    );
    assert!(!marker.exists());
    let (_owner, naming_canceled) = tokio::sync::watch::channel(true);
    assert_eq!(
        media_runtime::resolve_batch_output_paths_cancellable(
            self::request(
                std::slice::from_ref(&input),
                OutputTarget::Container {
                    format: ContainerFormat::Matroska
                }
            ),
            naming_canceled
        )
        .await
        .unwrap_err()
        .code,
        "BATCH_CANCELED"
    );
    assert!(!marker.exists());
    let (owner, cancel) = tokio::sync::watch::channel(false);
    let worker = manager.clone();
    let proposed = request.clone();
    let pending = tokio::spawn(async move {
        worker
            .preview_encode_batch_cancellable(proposed, cancel)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !marker.exists() {
            assert!(!pending.is_finished());
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    owner.send_replace(true);
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(result.code, "BATCH_CANCELED");
    let pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "Canceled native preview left FFprobe alive"
    );
    assert!(manager.list_jobs().await.is_empty());
    assert!(!fixture.0.join("canceled.mkv").exists());
    let next = manager.preview_encode_batch(request).await.unwrap();
    assert_eq!(next.items.len(), 1);
    assert!(next.items[0].error.is_none());
    assert!(manager.list_jobs().await.is_empty());
    assert!(!fixture.0.join("canceled.mkv").exists());
    manager.shutdown().await;
}
