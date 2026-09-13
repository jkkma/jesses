//! Opt-in native gate: `cargo test -p media-runtime --test ffmpeg_video_jobs -- --include-ignored`.
//! All sources are synthesized locally; no user media is read or modified.
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_runtime::{
    EncodeBackend, EncodeRequest, EncodeSettings, JobManager, JobSnapshot, JobState, RemuxRequest,
    VideoEncoder,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::{Value, json};

struct Fixture(PathBuf);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jesses-ffmpeg-video-{}-{nonce}-{serial}",
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

async fn synthesize(
    input: &Path,
    depth: u8,
    full: bool,
    hdr: bool,
    size: &str,
    frames: u32,
    color_chroma: (&str, &str),
) {
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
        (color_chroma.0, color_chroma.0, color_chroma.0)
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
                color_chroma.1,
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

fn request(input: &Path, destination: &Path, encoder: VideoEncoder, preset: u8) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: destination.to_string_lossy().into_owned(),
            // Reorder copied tracks and select the alternate video, not video 0.
            stream_indices: vec![3, 2, 1, 4],
        },
        settings: EncodeSettings {
            backend: EncodeBackend::Standalone,
            encoder,
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
    result.expect("native FFmpeg video job must progress within 90 seconds")
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
#[ignore = "requires FFmpeg libx265/libvpx-vp9 with 8/10-bit support"]
async fn ffmpeg_encoders_preserve_depth_color_fractional_timing_and_copied_tracks() {
    for encoder in [VideoEncoder::X265, VideoEncoder::Vp9] {
        for (depth, full) in [(8, false), (10, false), (8, true), (10, true)] {
            let fixture = Fixture::new();
            let input = fixture.0.join("- movie's & $ % 日本語.mkv");
            let destination = fixture.0.join("encoded café & 東京.mkv");
            synthesize(&input, depth, full, false, "320x180", 48, ("bt709", "left")).await;
            let original = std::fs::read(&input).unwrap();
            let manager = JobManager::new(fixture.0.join("logs"));
            let request = request(
                &input,
                &destination,
                encoder,
                if encoder == VideoEncoder::X265 { 5 } else { 4 },
            );
            let job = manager.start_encode(request.clone()).await.unwrap();
            let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
            manager.shutdown().await;
            assert_eq!(
                result.state,
                JobState::Succeeded,
                "{encoder:?} {depth}-bit full={full}: {result:#?}"
            );
            assert_eq!(result.encode_settings.as_ref(), Some(&request.settings));
            let actual = probe(
                &destination,
                &[
                    "-count_frames",
                    "-show_streams",
                    "-show_chapters",
                    "-show_data_hash",
                    "sha256",
                ],
            )
            .await;
            let streams = actual["streams"].as_array().unwrap();
            assert_eq!(streams.len(), 4);
            let video = &streams[2];
            assert_eq!(
                video["codec_name"],
                if encoder == VideoEncoder::X265 {
                    "hevc"
                } else {
                    "vp9"
                }
            );
            assert_eq!(video["width"], 320);
            assert_eq!(video["height"], 180);
            assert_eq!(video["nb_read_frames"], "48");
            assert_eq!(
                video["pix_fmt"],
                if depth == 10 {
                    "yuv420p10le"
                } else if full && encoder == VideoEncoder::X265 {
                    "yuvj420p"
                } else {
                    "yuv420p"
                }
            );
            assert_eq!(video["color_range"], if full { "pc" } else { "tv" });
            assert_eq!(video["color_primaries"], "bt709");
            assert_eq!(video["color_transfer"], "bt709");
            assert_eq!(video["color_space"], "bt709");
            assert_eq!(video["chroma_location"], "left");
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
            assert_eq!(frames.len(), 48);
            if encoder == VideoEncoder::X265 {
                assert!(
                    frames.iter().any(|frame| frame["pict_type"] == "B"),
                    "x265 must exercise B-frame timing"
                );
            }
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
        }
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg libx265/libvpx-vp9"]
async fn ffmpeg_encoders_preserve_supported_sdr_color_and_chroma_tags() {
    for encoder in [VideoEncoder::X265, VideoEncoder::Vp9] {
        for (color, chroma) in [
            ("bt709", "center"),
            ("bt709", "topleft"),
            ("bt470bg", "left"),
            ("smpte170m", "left"),
        ] {
            let fixture = Fixture::new();
            let input = fixture.0.join("color source.mkv");
            let destination = fixture.0.join("color encoded.mkv");
            synthesize(&input, 10, false, false, "320x180", 24, (color, chroma)).await;
            let original = std::fs::read(&input).unwrap();
            let manager = JobManager::new(fixture.0.join("logs"));
            let job = manager
                .start_encode(request(&input, &destination, encoder, 4))
                .await
                .unwrap();
            let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
            manager.shutdown().await;
            assert_eq!(
                result.state,
                JobState::Succeeded,
                "{encoder:?} {color} {chroma}: {result:#?}"
            );
            let actual = probe(&destination, &["-select_streams", "v", "-show_streams"]).await;
            let video = &actual["streams"][0];
            for key in ["color_primaries", "color_transfer", "color_space"] {
                assert_eq!(video[key], color);
            }
            assert_eq!(video["chroma_location"], chroma);
            assert_eq!(std::fs::read(input).unwrap(), original);
            assert_no_partial_output(&fixture.0);
        }
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg libx265/libvpx-vp9 and native FLAC"]
async fn ffmpeg_encoders_apply_framing_and_audio_with_validated_black_borders() {
    for encoder in [VideoEncoder::X265, VideoEncoder::Vp9] {
        let fixture = Fixture::new();
        let input = fixture.0.join("framing source.mkv");
        let destination = fixture.0.join("framed output.mkv");
        synthesize(&input, 10, false, false, "320x180", 48, ("bt709", "left")).await;
        let original = std::fs::read(&input).unwrap();
        let manager = JobManager::new(fixture.0.join("logs"));
        let mut request = request(&input, &destination, encoder, 4);
        request.settings.framing = serde_json::from_value(json!({"crop":{"top":4,"right":8,"bottom":8,"left":8},"resizeWidth":160,"borders":{"top":8,"right":8,"bottom":8,"left":8}})).unwrap();
        request.settings.audio = serde_json::from_value(
            json!([{"streamIndex":2,"codec":"flac","bitrateKbps":128,"channels":"preserve"}]),
        )
        .unwrap();
        let job = manager.start_encode(request.clone()).await.unwrap();
        let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        manager.shutdown().await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "{encoder:?}: {result:#?}"
        );
        assert_eq!(result.encode_settings.as_ref(), Some(&request.settings));
        let actual = probe(
            &destination,
            &[
                "-count_frames",
                "-show_streams",
                "-show_data_hash",
                "sha256",
            ],
        )
        .await;
        assert_eq!(actual["streams"][2]["width"], 176);
        assert_eq!(actual["streams"][2]["height"], 104);
        assert_eq!(actual["streams"][2]["nb_read_frames"], "48");
        assert_eq!(actual["streams"][1]["codec_name"], "flac");
        assert_eq!(actual["streams"][1]["sample_rate"], "48000");
        assert_eq!(actual["streams"][1]["channels"], 1);
        assert_eq!(packets(&input, "3").await, packets(&destination, "0").await);
        let source = probe(&input, &["-show_streams", "-show_data_hash", "sha256"]).await;
        assert_eq!(
            actual["streams"][3]["extradata_hash"],
            source["streams"][4]["extradata_hash"]
        );
        let raw = output(
            command("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&destination)
                .args([
                    "-map",
                    "0:v:0",
                    "-frames:v",
                    "1",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "yuv420p10le",
                    "-",
                ]),
        )
        .await;
        let pixels: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|sample| u16::from_le_bytes(*sample))
            .collect();
        for y in 0..4 {
            for x in 0..4 {
                assert!(
                    pixels[y * 176 + x].abs_diff(64) <= 4,
                    "Black border luma {}",
                    pixels[y * 176 + x]
                );
            }
        }
        assert_eq!(std::fs::read(&input).unwrap(), original);
        assert_no_partial_output(&fixture.0);
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg libx265/libvpx-vp9"]
async fn ffmpeg_encoders_reject_hdr_and_cancel_active_pipeline_without_publishing() {
    for encoder in [VideoEncoder::X265, VideoEncoder::Vp9] {
        let fixture = Fixture::new();
        let input = fixture.0.join("source.mkv");
        let destination = fixture.0.join("must not publish.mkv");
        synthesize(&input, 10, false, true, "320x180", 24, ("bt709", "left")).await;
        let original = std::fs::read(&input).unwrap();
        let manager = JobManager::new(fixture.0.join("logs"));
        let job = manager
            .start_encode(request(&input, &destination, encoder, 4))
            .await
            .unwrap();
        let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        manager.shutdown().await;
        assert_eq!(result.state, JobState::Failed, "{result:#?}");
        assert_eq!(
            result.error.as_ref().unwrap().code,
            "ENCODE_INPUT_UNSUPPORTED"
        );
        assert!(!destination.exists());
        assert_eq!(std::fs::read(&input).unwrap(), original);
        let input = fixture.0.join("cancel source.mkv");
        synthesize(&input, 8, false, false, "640x360", 480, ("bt709", "left")).await;
        let original = std::fs::read(&input).unwrap();
        let manager = JobManager::new(fixture.0.join("cancel-logs"));
        let job = manager
            .start_encode(request(
                &input,
                &destination,
                encoder,
                if encoder == VideoEncoder::X265 { 9 } else { 0 },
            ))
            .await
            .unwrap();
        let running = wait_for(&manager, &job.id, |job| {
            job.state == JobState::Running
                && job
                    .logs
                    .iter()
                    .any(|line| line.contains("[consumer] frame="))
        })
        .await;
        if running.state != JobState::Running {
            manager.shutdown().await;
            panic!("Expected active encoder: {running:#?}");
        }
        manager.cancel_job(job.id.clone()).await.unwrap();
        let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        tokio::time::timeout(Duration::from_secs(10), manager.shutdown())
            .await
            .expect("Owned pipeline must be reaped");
        assert_eq!(result.state, JobState::Canceled, "{result:#?}");
        assert!(!destination.exists());
        assert_eq!(std::fs::read(&input).unwrap(), original);
        assert_no_partial_output(&fixture.0);
    }
}
