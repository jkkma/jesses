//! Real source-control workflows; opt in with the qualified FFmpeg pair/x264.
use media_core::{
    EncodeRequest, EncodeSettings, EncodingPreferences, JobState, RemuxRequest, ResizeFilter,
    TemporalSettings, VideoEncoder, VideoFraming, VideoSeekStrategy, VideoTimeTrim, VideoTrim,
};
use media_runtime::{
    jobs::JobManager,
    supervisor::{self, CommandSpec},
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
            "jesses-source-controls-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tool(name: &str) -> PathBuf {
    std::env::var_os(format!("JESSES_{}", name.to_uppercase()))
        .map_or_else(|| PathBuf::from(name), PathBuf::from)
}
async fn command(name: &str, args: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = watch::channel(false);
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: tool(name),
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
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
async fn pixels(path: &Path) -> Vec<String> {
    let mut values = args(&["-v", "error", "-i"]);
    values.push(path.as_os_str().to_owned());
    values.extend(args(&["-map", "0:v:0", "-f", "framemd5", "-"]));
    String::from_utf8(command("ffmpeg", values).await)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.rsplit(',').next().unwrap().trim().to_owned())
        .collect()
}
fn atoms(bytes: &[u8]) -> Vec<[u8; 4]> {
    let mut found = Vec::new();
    let mut offset = 0;
    while offset + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        if size < 8 || offset + size > bytes.len() {
            break;
        }
        found.push(bytes[offset + 4..offset + 8].try_into().unwrap());
        offset += size;
    }
    found
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe and standalone x264"]
async fn exact_time_seek_resize_and_global_keyframe_faststart_controls_execute() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source.mkv");
    let mut values = args(&[
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=128x96:rate=24:duration=3",
        "-c:v",
        "ffv1",
        "-pix_fmt",
        "yuv420p",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
    ]);
    values.push(source.as_os_str().to_owned());
    command("ffmpeg", values).await;
    let original = std::fs::read(&source).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let mut outputs = Vec::new();
    let (_crop_owner, crop_cancel) = watch::channel(false);
    let crop = media_runtime::detect_crop_with_samples(
        media_core::AutoCropRequest {
            input_path: source.to_string_lossy().into_owned(),
            video_stream_index: 0,
        },
        crop_cancel,
        3,
    )
    .await
    .unwrap();
    assert_eq!(
        crop.sample_count, 3,
        "the configured sample count must drive actual crop decoding"
    );
    let (_resolve_owner, resolve_cancel) = watch::channel(false);
    let deinterlace = media_core::DeinterlaceExportRequest {
        input_path: source.to_string_lossy().into_owned(),
        output_path: String::new(),
        video_stream_index: 0,
        method: media_core::DeinterlaceExportMethod::Qtgmc,
        mode: media_core::DeinterlaceMode::Frame,
        field_order: media_core::FieldOrder::BottomFirst,
        qtgmc_preset: media_core::QtgmcPreset::VeryFast,
    };
    let automatic =
        media_runtime::resolve_deinterlace(deinterlace.clone(), true, resolve_cancel.clone())
            .await
            .unwrap();
    assert!(
        automatic.request.is_none(),
        "automatic mode must leave this progressive source alone"
    );
    let explicit = media_runtime::resolve_deinterlace(deinterlace, false, resolve_cancel.clone())
        .await
        .unwrap();
    let resolved = explicit.request.unwrap();
    assert_eq!(resolved.field_order, media_core::FieldOrder::TopFirst);
    if resolved.method == media_core::DeinterlaceExportMethod::Bwdif {
        assert!(explicit.message.contains("BWDIF rather than QTGMC"));
    } else {
        assert_eq!(resolved.method, media_core::DeinterlaceExportMethod::Qtgmc);
    }
    let mut tasks = [media_core::UtilityRequest::DeinterlaceExport(
        media_core::DeinterlaceExportRequest {
            method: media_core::DeinterlaceExportMethod::Bwdif,
            ..resolved
        },
    )];
    let _names = media_runtime::prepare_utility_outputs(&mut tasks, "").unwrap();
    let result = media_runtime::run_utility(tasks[0].clone(), resolve_cancel)
        .await
        .unwrap();
    let media_core::UtilityResult::Artifact(artifact) = result else {
        panic!("expected deinterlace artifact")
    };
    assert!(artifact.message.contains("BWDIF"));
    assert!(artifact.output_path.ends_with("source_deinterlaced.mkv"));
    assert_eq!(pixels(Path::new(&artifact.output_path)).await.len(), 72);
    for (index, strategy) in [
        VideoSeekStrategy::InputSeek,
        VideoSeekStrategy::DecodeFromStart,
    ]
    .into_iter()
    .enumerate()
    {
        let output = fixture.0.join(format!("seek-{index}.mp4"));
        let request = EncodeRequest {
            source: RemuxRequest {
                input_path: source.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::X264,
                preset: 0,
                lossless: true,
                encoding_preferences: Some(EncodingPreferences {
                    keyframe_interval_seconds: 1,
                    mp4_faststart: index == 0,
                }),
                trim: Some(VideoTrim {
                    start_frame: 0,
                    end_frame_exclusive: 0,
                    time: Some(VideoTimeTrim {
                        start_milliseconds: 55,
                        end_milliseconds: 1650,
                        seek_strategy: strategy,
                    }),
                }),
                ..Default::default()
            },
        };
        let started = manager.start_encode(request).await.unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let job = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|job| job.id == started.id)
                    .unwrap();
                if matches!(
                    job.state,
                    JobState::Succeeded | JobState::Failed | JobState::Canceled
                ) {
                    break job;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
        let atom = atoms(&std::fs::read(&output).unwrap());
        let moov = atom.iter().position(|atom| atom == b"moov").unwrap();
        let mdat = atom.iter().position(|atom| atom == b"mdat").unwrap();
        assert_eq!(moov < mdat, index == 0);
        let mut values = args(&[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_frames",
            "-show_entries",
            "frame=key_frame",
            "-of",
            "json",
        ]);
        values.push(output.as_os_str().to_owned());
        let frames: serde_json::Value =
            serde_json::from_slice(&command("ffprobe", values).await).unwrap();
        let frames = frames["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 38);
        let keys = frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame["key_frame"] == 1)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(keys[0], 0);
        assert!(keys.len() >= 2);
        assert!(keys.windows(2).all(|pair| pair[1] - pair[0] <= 24));
        outputs.push(output);
    }
    assert_eq!(
        pixels(&outputs[0]).await,
        pixels(&outputs[1]).await,
        "accurate input seek must select exactly the same decoded source frames"
    );
    for (index, filter) in [ResizeFilter::Spline, ResizeFilter::Area]
        .into_iter()
        .enumerate()
    {
        let output = fixture.0.join(format!("resize-{index}.mkv"));
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: source.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    encoder: VideoEncoder::X264,
                    preset: 0,
                    framing: VideoFraming {
                        resize_width: Some(64),
                        ..Default::default()
                    },
                    temporal: Some(TemporalSettings {
                        resize_filter: filter,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let value = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|value| value.id == job.id)
                    .unwrap();
                if matches!(value.state, JobState::Succeeded | JobState::Failed) {
                    break value;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
        let mut values = args(&[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "json",
        ]);
        values.push(output.as_os_str().to_owned());
        let document: serde_json::Value =
            serde_json::from_slice(&command("ffprobe", values).await).unwrap();
        assert_eq!(document["streams"][0]["width"], 64);
        assert_eq!(document["streams"][0]["height"], 48);
    }
    assert_eq!(std::fs::read(&source).unwrap(), original);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe attachment support"]
async fn selected_attachment_export_checks_bytes_scope_and_existing_destinations() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source.mkv");
    let attachment = fixture.0.join("payload.bin");
    let payload = b"complete selected attachment\0\xff";
    std::fs::write(&attachment, payload).unwrap();
    let mut values = args(&[
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "color=size=64x64:rate=1:duration=1",
        "-c:v",
        "ffv1",
        "-attach",
    ]);
    values.push(attachment.as_os_str().to_owned());
    values.extend(args(&[
        "-metadata:s:t",
        "mimetype=application/octet-stream",
        "-metadata:s:t",
        "filename=../../unsafe.bin",
    ]));
    values.push(source.as_os_str().to_owned());
    command("ffmpeg", values).await;
    let original = std::fs::read(&source).unwrap();
    let (_owner, cancel) = watch::channel(false);
    assert_eq!(
        media_runtime::jobs::extract_attachment(
            source.to_string_lossy().into_owned(),
            0,
            cancel.clone()
        )
        .await
        .unwrap_err()
        .code,
        "ATTACHMENT_INVALID"
    );
    let folder = media_runtime::jobs::extract_attachment(
        source.to_string_lossy().into_owned(),
        1,
        cancel.clone(),
    )
    .await
    .unwrap();
    let output = Path::new(&folder).join("unsafe.bin");
    assert_eq!(std::fs::read(&output).unwrap(), payload);
    assert!(!fixture.0.join("unsafe.bin").exists());
    assert_eq!(
        media_runtime::jobs::extract_attachment(source.to_string_lossy().into_owned(), 1, cancel)
            .await
            .unwrap_err()
            .code,
        "OUTPUT_EXISTS"
    );
    assert_eq!(std::fs::read(output).unwrap(), payload);
    assert_eq!(std::fs::read(source).unwrap(), original);
}
