//! Opt-in native gate: `cargo test -p media-runtime --test x264_jobs -- --include-ignored`.
//! All sources are synthesized locally; no user media is read or modified.
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_runtime::{
    BatchEncodeInput, BatchEncodeRequest, EncodeBackend, EncodeRequest, EncodeSettings, JobManager,
    JobSnapshot, JobState, RemuxRequest, VideoEncoder,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::{Value, json};

struct Fixture(PathBuf);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn lossless_ultrafast_preserves_pixels_when_the_video_start_header_is_absent() {
    let fixture = Fixture::new();
    let input = fixture.0.join("lossless-source.mkv");
    let destination = fixture.0.join("lossless-output.mkv");
    output(command("ffmpeg").args([
        "-v", "error", "-n", "-f", "lavfi", "-i", "testsrc2=s=128x72:r=24:d=0.167", "-frames:v", "4", "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-pix_fmt", "yuv420p10le", "-chroma_sample_location", "left", "-c:v", "ffv1", "-level", "3",
    ]).arg(&input)).await;
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
                preset: 0,
                lossless: true,
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(60), async {
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
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    assert!(
        completed
            .logs
            .iter()
            .any(|line| line.contains("Verified lossless decoded pixel"))
    );
    let mut hashes = Vec::new();
    for path in [&input, &destination] {
        hashes.push(
            output(
                command("ffmpeg")
                    .args(["-v", "error", "-xerror", "-i"])
                    .arg(path)
                    .args([
                        "-map",
                        "0:v:0",
                        "-pix_fmt",
                        "yuv420p10le",
                        "-c:v",
                        "rawvideo",
                        "-f",
                        "hash",
                        "-hash",
                        "sha256",
                        "-",
                    ]),
            )
            .await,
        );
    }
    assert_eq!(hashes[0], hashes[1]);
}
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jesses-x264-{}-{nonce}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Every file is owned exclusively by this newly created test fixture.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn command(executable: impl AsRef<std::ffi::OsStr>) -> Command {
    // This builder only collects arguments. All native spawns below use the
    // owned supervisor and its explicit Windows inherited-handle list.
    Command::new(executable)
}

async fn output(command: &mut Command) -> Vec<u8> {
    static TOOLS: tokio::sync::OnceCell<Vec<media_runtime::ToolInfo>> =
        tokio::sync::OnceCell::const_new();
    let program = Path::new(command.get_program());
    let executable = if program.is_absolute() {
        program.to_path_buf()
    } else {
        let tools = TOOLS
            .get_or_init(|| Box::pin(media_runtime::get_capabilities()))
            .await;
        let tool = tools
            .iter()
            .find(|tool| tool.id == program.to_string_lossy())
            .unwrap();
        assert!(
            tool.available,
            "Required fixture tool is unavailable: {tool:?}"
        );
        PathBuf::from(tool.path.as_ref().unwrap())
    };
    let spec = CommandSpec {
        executable,
        args: command.get_args().map(std::ffi::OsString::from).collect(),
        cwd: command.get_current_dir().map(Path::to_path_buf),
    };
    assert_eq!(command.get_envs().count(), 0);
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = Box::pin(run_capture(
        &spec,
        cancel,
        8 * 1024 * 1024,
        Duration::from_secs(45),
    ))
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "Native tool failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

async fn synthesize(input: &Path, depth: u8, full: bool, hdr: bool, size: &str, frames: u32) {
    let root = input.parent().unwrap();
    let subtitle = root.join("captions.srt");
    let attachment = root.join("font.ttf");
    let chapters = root.join("chapters.txt");
    std::fs::write(
        &subtitle,
        b"1\n00:00:00,000 --> 00:00:01,500\nCopied subtitle\n",
    )
    .unwrap();
    std::fs::write(&attachment, b"owned font attachment fixture").unwrap();
    std::fs::write(&chapters, b";FFMETADATA1\ntitle=Owned fixture\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1500\ntitle=Opening\n").unwrap();
    let format = if depth == 10 {
        "yuv420p10le"
    } else {
        "yuv420p"
    };
    let range = if full { "pc" } else { "tv" };
    let (primaries, transfer, matrix) = if hdr {
        ("bt2020", "smpte2084", "bt2020nc")
    } else {
        ("bt709", "bt709", "bt709")
    };
    let filter = format!(
        "testsrc2=size={size}:rate=24000/1001,format={format},setparams=range={range}:color_primaries={primaries}:color_trc={transfer}:colorspace={matrix}"
    );
    output(
        command("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=96x64:r=24000/1001",
                "-f",
                "lavfi",
                "-i",
                &filter,
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-i",
            ])
            .arg(&subtitle)
            .args(["-f", "ffmetadata", "-i"])
            .arg(&chapters)
            .args([
                "-map",
                "0:v",
                "-map",
                "1:v",
                "-map",
                "2:a",
                "-map",
                "3:s",
                "-map_metadata",
                "4",
                "-map_chapters",
                "4",
                "-c:v",
                "ffv1",
                "-level",
                "3",
                "-pix_fmt:v:0",
                "yuv420p",
                "-pix_fmt:v:1",
                format,
                "-frames:v:0",
                &frames.to_string(),
                "-frames:v:1",
                &frames.to_string(),
                "-t",
                &(f64::from(frames) * 1001.0 / 24000.0).to_string(),
                "-c:a",
                "pcm_s16le",
                "-c:s",
                "srt",
                "-color_primaries:v:1",
                primaries,
                "-color_trc:v:1",
                transfer,
                "-colorspace:v:1",
                matrix,
                "-color_range:v:1",
                range,
                "-chroma_sample_location:v:1",
                "left",
                "-metadata:s:a:0",
                "language=jpn",
                "-metadata:s:a:0",
                "title=Original tone",
                "-metadata:s:s:0",
                "language=eng",
                "-attach",
            ])
            .arg(&attachment)
            .args(["-metadata:s:t:0", "mimetype=application/x-truetype-font"])
            .arg(input),
    )
    .await;
}

fn request(input: &Path, destination: &Path, preset: u8) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: destination.to_string_lossy().into_owned(),
            // Reorder copied tracks and select the alternate video, not video 0.
            stream_indices: vec![3, 2, 1, 4],
        },
        settings: EncodeSettings {
            backend: EncodeBackend::Standalone,
            encoder: VideoEncoder::X264,
            video_stream_index: 1,
            crf: 23,
            preset,
            ..Default::default()
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
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if result.is_err() {
        manager.shutdown().await;
    }
    result.expect("native x264 job must progress within 90 seconds")
}

async fn probe(path: &Path, args: &[&str]) -> Value {
    serde_json::from_slice(
        &output(
            command("ffprobe")
                .args(["-v", "error", "-of", "json"])
                .args(args)
                .arg(path),
        )
        .await,
    )
    .unwrap()
}

async fn packets(path: &Path, stream: &str) -> Value {
    probe(
        path,
        &[
            "-select_streams",
            stream,
            "-show_packets",
            "-show_data_hash",
            "sha256",
            "-show_entries",
            "packet=pts_time,dts_time,duration_time,data_hash",
        ],
    )
    .await["packets"]
        .clone()
}

fn assert_no_partial_output(root: &Path) {
    assert!(std::fs::read_dir(root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial.")
    }));
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264 with 8/10-bit support on PATH"]
async fn x264_preserves_depth_range_bframes_fractional_timing_and_copied_tracks() {
    for (depth, full) in [(8, false), (10, false), (8, true), (10, true)] {
        let fixture = Fixture::new();
        let input = fixture.0.join("- movie's & $ % 日本語.mkv");
        let destination = fixture.0.join("encoded café & 東京.mkv");
        synthesize(&input, depth, full, false, "320x180", 96).await;
        let original = std::fs::read(&input).unwrap();
        let manager = JobManager::new(fixture.0.join("logs"));
        let request = request(&input, &destination, 5);
        let job = manager.start_encode(request.clone()).await.unwrap();
        let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        manager.shutdown().await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "{depth}-bit full={full}: {result:#?}"
        );
        assert_eq!(result.request, request.source);
        assert_eq!(result.encode_settings.as_ref(), Some(&request.settings));
        let actual = probe(
            &destination,
            &[
                "-count_frames",
                "-show_streams",
                "-show_chapters",
                "-show_format",
                "-show_data_hash",
                "sha256",
            ],
        )
        .await;
        let streams = actual["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 4);
        assert_eq!(streams[0]["codec_name"], "subrip");
        assert_eq!(streams[1]["codec_name"], "pcm_s16le");
        assert_eq!(streams[1]["tags"]["language"], "jpn");
        assert_eq!(streams[1]["tags"]["title"], "Original tone");
        let video = &streams[2];
        assert_eq!(video["codec_name"], "h264");
        assert_eq!(video["width"], 320);
        assert_eq!(video["height"], 180);
        assert_eq!(video["nb_read_frames"], "96");
        assert_eq!(video["r_frame_rate"], "24000/1001");
        assert_eq!(
            video["pix_fmt"],
            if depth == 10 {
                "yuv420p10le"
            } else if full {
                "yuvj420p"
            } else {
                "yuv420p"
            }
        );
        assert_eq!(video["color_range"], if full { "pc" } else { "tv" });
        assert_eq!(video["color_primaries"], "bt709");
        assert_eq!(video["color_transfer"], "bt709");
        assert_eq!(video["color_space"], "bt709");
        let frames = probe(
            &destination,
            &[
                "-select_streams",
                "2",
                "-show_frames",
                "-show_entries",
                "frame=pict_type,best_effort_timestamp_time",
            ],
        )
        .await;
        let frames = frames["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 96);
        assert!(
            frames.iter().any(|frame| frame["pict_type"] == "B"),
            "medium preset must actually exercise B-frame timing"
        );
        for (index, frame) in frames.iter().enumerate() {
            let pts: f64 = frame["best_effort_timestamp_time"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            assert!(
                (pts - index as f64 * 1001.0 / 24000.0).abs() < 0.0011,
                "frame {index} PTS={pts}"
            );
        }
        assert_eq!(packets(&input, "2").await, packets(&destination, "1").await);
        assert_eq!(packets(&input, "3").await, packets(&destination, "0").await);
        let source = probe(
            &input,
            &[
                "-show_streams",
                "-show_chapters",
                "-show_data_hash",
                "sha256",
            ],
        )
        .await;
        assert_eq!(
            streams[3]["extradata_hash"],
            source["streams"][4]["extradata_hash"]
        );
        assert!(streams[3]["extradata_hash"].as_str().is_some());
        assert_eq!(actual["chapters"], source["chapters"]);
        assert_eq!(std::fs::read(&input).unwrap(), original);
        assert_no_partial_output(&fixture.0);

        if depth == 8 && full {
            // FFV1 reports yuv420p + pc; actual full-range H.264 reports
            // yuvj420p. Re-encode it to exercise that input format and the
            // source frame scan with B-frames, not only the output validator.
            let original = std::fs::read(&destination).unwrap();
            let roundtrip = fixture.0.join("H.264 full-range source re-encoded.mkv");
            let manager = JobManager::new(fixture.0.join("roundtrip-logs"));
            let job = manager
                .start_encode(EncodeRequest {
                    source: RemuxRequest {
                        input_path: destination.to_string_lossy().into_owned(),
                        output_path: roundtrip.to_string_lossy().into_owned(),
                        stream_indices: vec![0, 1, 2, 3],
                    },
                    settings: EncodeSettings {
                        video_stream_index: 2,
                        ..request.settings
                    },
                })
                .await
                .unwrap();
            let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
            manager.shutdown().await;
            assert_eq!(result.state, JobState::Succeeded, "{result:#?}");
            let output = probe(&roundtrip, &["-count_frames", "-show_streams"]).await;
            assert_eq!(output["streams"][2]["pix_fmt"], "yuvj420p");
            assert_eq!(output["streams"][2]["color_range"], "pc");
            assert_eq!(output["streams"][2]["nb_read_frames"], "96");
            assert_eq!(output["streams"][2]["r_frame_rate"], "24000/1001");
            assert_eq!(
                packets(&destination, "1").await,
                packets(&roundtrip, "1").await
            );
            assert_eq!(
                packets(&destination, "0").await,
                packets(&roundtrip, "0").await
            );
            assert_eq!(std::fs::read(&destination).unwrap(), original);
            assert_no_partial_output(&fixture.0);
        }
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264 on PATH"]
async fn x264_batch_preview_queue_and_history_retain_the_selected_encoder() {
    let fixture = Fixture::new();
    let input = fixture.0.join("batch source.mkv");
    synthesize(&input, 8, false, false, "320x180", 48).await;
    let original = std::fs::read(&input).unwrap();
    let existing = fixture.0.join("batch source_x264.mkv");
    std::fs::write(&existing, b"existing destination").unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let input = BatchEncodeInput {
        temporal: None,
        tone_map: None,
        trim: None,
        subtitles: Vec::new(),
        framing: Default::default(),
        audio: Vec::new(),
        input_path: input.to_string_lossy().into_owned(),
        stream_indices: vec![3, 2, 1, 4],
        video_stream_index: 1,
    };
    let preview = manager
        .preview_encode_batch(BatchEncodeRequest {
            parameters: Vec::new(),
            av1an_options: None,
            av1an_grain: None,
            av1an_filters: Vec::new(),
            output_container: None,
            rate_control: None,
            lossless: false,
            svt_crf_quarter_steps: None,
            svt_preset: None,
            inputs: vec![input.clone(), input.clone()],
            output_directory: fixture.0.to_string_lossy().into_owned(),
            output_name_template: None,
            naming_date: None,
            backend: EncodeBackend::Standalone,
            encoder: VideoEncoder::X264,
            workers: 2,
            crf: 23,
            preset: 5,
            film_grain: 0,
            lineart_psy_bias: 0,
            texture_psy_bias: 0,
            hdr_tune: Default::default(),
            hdr10_fallback: false,
        })
        .await
        .unwrap();
    let requests: Vec<_> = preview
        .items
        .into_iter()
        .map(|item| {
            assert!(item.error.is_none(), "{item:#?}");
            item.request.unwrap()
        })
        .collect();
    assert!(
        requests[0]
            .source
            .output_path
            .ends_with("batch source_x264_2.mkv")
    );
    assert!(
        requests[1]
            .source
            .output_path
            .ends_with("batch source_x264_3.mkv")
    );
    assert!(
        requests
            .iter()
            .all(|request| request.settings.encoder == VideoEncoder::X264)
    );
    let jobs = manager
        .enqueue_encode_batch(requests.clone())
        .await
        .unwrap();
    for job in jobs {
        let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        assert_eq!(result.state, JobState::Succeeded, "{result:#?}");
    }
    manager.shutdown().await;
    drop(manager);
    let restored = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    restored.ready().await.unwrap();
    let history = restored.list_jobs().await;
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|job| job.state == JobState::Succeeded
        && job.encode_settings.as_ref() == Some(&requests[0].settings)));
    restored.shutdown().await;
    assert_eq!(std::fs::read(existing).unwrap(), b"existing destination");
    assert_eq!(std::fs::read(input.input_path).unwrap(), original);
    assert_no_partial_output(&fixture.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264 on PATH"]
async fn x264_rejects_hdr_before_creating_output_in_preview_and_execution() {
    let fixture = Fixture::new();
    let input = fixture.0.join("HDR source.mkv");
    let destination = fixture.0.join("rejected.mkv");
    synthesize(&input, 10, false, true, "320x180", 48).await;
    let source = probe(&input, &["-select_streams", "v:1", "-show_streams"]).await;
    assert_eq!(source["streams"][0]["pix_fmt"], "yuv420p10le");
    assert_eq!(source["streams"][0]["color_primaries"], "bt2020");
    assert_eq!(source["streams"][0]["color_transfer"], "smpte2084");
    assert_eq!(source["streams"][0]["color_space"], "bt2020nc");
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let request = request(&input, &destination, 5);
    let preview = manager
        .preview_encode_batch(BatchEncodeRequest {
            parameters: Vec::new(),
            av1an_options: None,
            av1an_grain: None,
            av1an_filters: Vec::new(),
            output_container: None,
            rate_control: None,
            lossless: false,
            svt_crf_quarter_steps: None,
            svt_preset: None,
            inputs: vec![BatchEncodeInput {
                temporal: None,
                tone_map: None,
                trim: None,
                subtitles: Vec::new(),
                framing: Default::default(),
                audio: Vec::new(),
                input_path: request.source.input_path.clone(),
                stream_indices: request.source.stream_indices.clone(),
                video_stream_index: 1,
            }],
            output_directory: fixture.0.to_string_lossy().into_owned(),
            output_name_template: None,
            naming_date: None,
            backend: EncodeBackend::Standalone,
            encoder: VideoEncoder::X264,
            workers: 2,
            crf: 23,
            preset: 5,
            film_grain: 0,
            lineart_psy_bias: 0,
            texture_psy_bias: 0,
            hdr_tune: Default::default(),
            hdr10_fallback: false,
        })
        .await
        .unwrap();
    assert_eq!(
        preview.items[0].error.as_ref().unwrap().code,
        "ENCODE_INPUT_UNSUPPORTED"
    );
    assert!(preview.items[0].request.is_none());
    let job = manager.start_encode(request).await.unwrap();
    let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    assert_eq!(result.state, JobState::Failed, "{result:#?}");
    assert_eq!(
        result.error.as_ref().unwrap().code,
        "ENCODE_INPUT_UNSUPPORTED"
    );
    assert!(!destination.exists());
    assert_eq!(std::fs::read(input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264 on PATH"]
async fn canceling_active_x264_reaps_encoder_and_preserves_source_without_publishing() {
    let fixture = Fixture::new();
    let input = fixture.0.join("cancel source.mkv");
    let destination = fixture.0.join("must not publish.mkv");
    synthesize(&input, 8, false, false, "640x360", 480).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let request = request(&input, &destination, 9);
    let job = manager.start_encode(request.clone()).await.unwrap();
    let running = wait_for(&manager, &job.id, |job| {
        job.state == JobState::Running
            && job
                .logs
                .iter()
                .any(|line| line.contains("x264 [info]: profile"))
    })
    .await;
    if running.state != JobState::Running {
        manager.shutdown().await;
        panic!("Expected an active x264 encoder: {running:#?}");
    }
    manager.cancel_job(job.id.clone()).await.unwrap();
    let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
    tokio::time::timeout(Duration::from_secs(10), manager.shutdown())
        .await
        .expect("Cancellation must stop the owned process tree promptly");
    assert_eq!(result.state, JobState::Canceled, "{result:#?}");
    assert_eq!(result.encode_settings.as_ref(), Some(&request.settings));
    assert!(!destination.exists());
    assert_eq!(std::fs::read(input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
    std::fs::remove_dir_all(&fixture.0)
        .expect("The canceled encoder must release its file handles");
}

#[tokio::test]
async fn missing_x264_reports_its_tool_without_starting_the_pipeline() {
    let fixture = Fixture::new();
    let bin = fixture.0.join("bin");
    std::fs::create_dir(&bin).unwrap();
    for name in ["ffmpeg", "ffprobe"] {
        let name = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        };
        // Discovery finds these owned executable placeholders. Neither should
        // run: x264 is absent and the admission fails before version checks.
        let executable = std::env::current_exe().unwrap();
        let placeholder = bin.join(name);
        if std::fs::hard_link(&executable, &placeholder).is_err() {
            std::fs::copy(executable, placeholder).unwrap();
        }
    }
    std::fs::write(fixture.0.join("source.mkv"), b"source preserved").unwrap();
    std::fs::write(fixture.0.join("missing-x264-helper"), b"owned fixture").unwrap();
    output(
        command(std::env::current_exe().unwrap())
            .args(["--exact", "missing_x264_child", "--ignored", "--nocapture"])
            .current_dir(&fixture.0),
    )
    .await;
}

#[test]
#[ignore = "helper subprocess for missing-x264 isolation"]
fn missing_x264_child() {
    let root = std::env::current_dir().unwrap();
    if std::fs::read(root.join("missing-x264-helper"))
        .ok()
        .as_deref()
        != Some(b"owned fixture")
    {
        return;
    }
    // This exact helper runs alone in a separate process, before any runtime
    // or discovery workers exist. The parent test process keeps its PATH.
    unsafe {
        std::env::set_var("PATH", root.join("bin"));
        for variable in ["JESSES_FFMPEG", "JESSES_FFPROBE", "JESSES_X264"] {
            std::env::remove_var(variable);
        }
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(missing_x264_job(root));
}

async fn missing_x264_job(root: PathBuf) {
    let input = root.join("source.mkv");
    let destination = root.join("output.mkv");
    let manager = JobManager::new(root.join("logs"));
    let request = request(&input, &destination, 5);
    let job = manager.start_encode(request).await.unwrap();
    let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    assert_eq!(result.state, JobState::Failed, "{result:#?}");
    assert_eq!(result.error.as_ref().unwrap().code, "TOOL_MISSING");
    assert!(result.error.as_ref().unwrap().message.contains("x264"));
    assert!(!destination.exists());
    assert_eq!(std::fs::read(input).unwrap(), b"source preserved");
    assert_no_partial_output(&root);
    assert_eq!(
        serde_json::to_value(EncodeSettings::default()).unwrap()["encoder"],
        json!("svtAv1")
    );
}
