//! Opt-in native gate: `cargo test -p media-runtime --test trim_jobs -- --include-ignored`.
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
            "jesses-trim-{}-{nonce}-{serial}",
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
#[ignore = "requires FFmpeg libx265/libvpx-vp9 and FLAC"]
async fn trim_preserves_exact_video_interval_audio_samples_subtitle_overlap_and_chapters() {
    for encoder in [VideoEncoder::X264, VideoEncoder::X265, VideoEncoder::Vp9] {
        let fixture = Fixture::new();
        let input = fixture.0.join("trim source.mkv");
        let destination = fixture.0.join("trim result.mkv");
        synthesize(&input, 8, false, false, "320x180", 48, ("bt709", "left")).await;
        let original = std::fs::read(&input).unwrap();
        let mut request = request(&input, &destination, encoder, 4);
        if encoder == VideoEncoder::X264 {
            request.settings.crf = 0;
        }
        request.settings.trim =
            Some(serde_json::from_value(json!({"startFrame":12,"endFrameExclusive":36})).unwrap());
        request.settings.audio = serde_json::from_value(
            json!([{"streamIndex":2,"codec":"flac","bitrateKbps":128,"channels":"preserve"}]),
        )
        .unwrap();
        let manager = JobManager::new(fixture.0.join("logs"));
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
                "-show_chapters",
                "-show_data_hash",
                "sha256",
            ],
        )
        .await;
        assert_eq!(actual["streams"][2]["nb_read_frames"], "24");
        let source = probe(&input, &["-show_streams", "-show_data_hash", "sha256"]).await;
        assert_eq!(
            actual["streams"][3]["extradata_hash"],
            source["streams"][4]["extradata_hash"]
        );
        let cues = packets(&destination, "0").await;
        assert_eq!(cues.as_array().unwrap().len(), 1);
        assert_eq!(cues[0]["pts_time"], "0.000000");
        let subtitle = output(
            command("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&destination)
                .args(["-map", "0:s:0", "-c:s", "copy", "-f", "srt", "-"]),
        )
        .await;
        assert!(String::from_utf8_lossy(&subtitle).contains("Copied subtitle"));
        let reference = output(
            command("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&input)
                .args([
                    "-map",
                    "0:2",
                    "-af",
                    "atrim=start_sample=24024:end_sample=72072,asetpts=PTS-STARTPTS",
                    "-f",
                    "s24le",
                    "-",
                ]),
        )
        .await;
        let decoded = output(
            command("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&destination)
                .args(["-map", "0:a:0", "-f", "s24le", "-"]),
        )
        .await;
        assert_eq!(decoded.len(), 48048 * 3);
        assert_eq!(decoded, reference);
        if encoder == VideoEncoder::X264 {
            let reference = output(
                command("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(&input)
                    .args([
                        "-map",
                        "0:1",
                        "-vf",
                        "trim=start_frame=12:end_frame=36,setpts=PTS-STARTPTS",
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        "yuv420p",
                        "-",
                    ]),
            )
            .await;
            let actual = output(
                command("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(&destination)
                    .args([
                        "-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-",
                    ]),
            )
            .await;
            assert_eq!(
                actual, reference,
                "Lossless x264 must select the exact source pictures, not only the frame count"
            );
        }

        assert_eq!(actual["chapters"][0]["start_time"], "0.000000");
        assert!(
            (actual["chapters"][0]["end_time"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap()
                - 0.9995)
                .abs()
                < 0.002
        );
        assert_eq!(std::fs::read(input).unwrap(), original);
        assert_no_partial_output(&fixture.0);
    }
}

async fn replace_subtitle(input: &Path, base: &Path, extension: &str, content: &str) {
    let subtitle = input.with_extension(extension);
    std::fs::write(&subtitle, content).unwrap();
    output(
        command("ffmpeg")
            .args(["-v", "error", "-n", "-i"])
            .arg(base)
            .args(["-f", extension, "-i"])
            .arg(&subtitle)
            .args([
                "-map",
                "0:0",
                "-map",
                "0:1",
                "-map",
                "0:2",
                "-map",
                "1:0",
                "-map",
                "0:4",
                "-map_metadata",
                "0",
                "-map_chapters",
                "0",
                "-c",
                "copy",
            ])
            .arg(input),
    )
    .await;
}
const ASS: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 320\nPlayResY: 180\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1,0,2,10,10,10,1\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.00,0:00:01.50,Default,,0,0,0,,Styled {\\i1}subtitle{\\i0}\n";

#[tokio::test]
#[ignore = "requires FFmpeg libvpx-vp9"]
async fn trim_clips_text_formats_and_retains_selected_zero_cue_tracks() {
    for extension in ["srt", "ass", "webvtt"] {
        for empty in [false, true] {
            let fixture = Fixture::new();
            let base = fixture.0.join("base.mkv");
            synthesize(&base, 8, false, false, "320x180", 48, ("bt709", "left")).await;
            let input = fixture.0.join("source.mkv");
            let text = match extension {
                "ass" => ASS,
                "webvtt" => "WEBVTT\n\n00:00:00.000 --> 00:00:01.500\nStyled <i>subtitle</i>\n",
                _ => "1\n00:00:00,000 --> 00:00:01,500\nStyled <i>subtitle</i>\n",
            };
            replace_subtitle(&input, &base, extension, text).await;
            let destination = fixture.0.join("output.mkv");
            let mut request = request(&input, &destination, VideoEncoder::Vp9, 4);
            request.settings.trim = Some(
                serde_json::from_value(if empty {
                    json!({"startFrame":36,"endFrameExclusive":48})
                } else {
                    json!({"startFrame":12,"endFrameExclusive":36})
                })
                .unwrap(),
            );
            request.settings.audio = serde_json::from_value(
                json!([{"streamIndex":2,"codec":"flac","bitrateKbps":128,"channels":"preserve"}]),
            )
            .unwrap();
            let manager = JobManager::new(fixture.0.join("logs"));
            let job = manager.start_encode(request).await.unwrap();
            let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
            manager.shutdown().await;
            assert_eq!(
                result.state,
                JobState::Succeeded,
                "{extension} empty={empty}: {result:#?}"
            );
            let actual = probe(&destination, &["-show_streams", "-show_chapters"]).await;
            assert_eq!(actual["streams"].as_array().unwrap().len(), 4);
            assert_eq!(
                actual["streams"][0]["codec_name"],
                if extension == "srt" {
                    "subrip"
                } else {
                    extension
                }
            );
            let cues = packets(&destination, "0").await;
            assert_eq!(cues.as_array().map_or(0, Vec::len), usize::from(!empty));
            if empty {
                assert!(actual["chapters"].as_array().unwrap().is_empty());
            }
            assert_no_partial_output(&fixture.0);
        }
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg libvpx-vp9"]
async fn trim_rejects_unsafe_boundaries_copy_audio_and_out_of_range_without_output() {
    for case in ["copy-audio", "out-of-range", "timed-ass"] {
        let fixture = Fixture::new();
        let base = fixture.0.join("base.mkv");
        synthesize(&base, 8, false, false, "320x180", 48, ("bt709", "left")).await;
        let input = if case == "timed-ass" {
            let input = fixture.0.join("animated.mkv");
            replace_subtitle(
                &input,
                &base,
                "ass",
                &ASS.replace("Styled ", "{\\t(0,1500,\\fs50)}Styled "),
            )
            .await;
            input
        } else {
            base
        };
        let original = std::fs::read(&input).unwrap();
        let destination = fixture.0.join("must not publish.mkv");
        let mut request = request(&input, &destination, VideoEncoder::Vp9, 4);
        request.settings.trim = Some(
            serde_json::from_value(
                json!({"startFrame":12,"endFrameExclusive":if case=="out-of-range" {49} else {36}}),
            )
            .unwrap(),
        );
        if case != "copy-audio" {
            request.settings.audio = serde_json::from_value(
                json!([{"streamIndex":2,"codec":"flac","bitrateKbps":128,"channels":"preserve"}]),
            )
            .unwrap();
        }
        let manager = JobManager::new(fixture.0.join("logs"));
        let job = manager.start_encode(request).await.unwrap();
        let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        manager.shutdown().await;
        assert_eq!(result.state, JobState::Failed, "{case}: {result:#?}");
        assert_eq!(result.error.unwrap().code, "TRIM_UNSUPPORTED");
        assert!(!destination.exists());
        assert_eq!(std::fs::read(input).unwrap(), original);
        assert_no_partial_output(&fixture.0);
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg libvpx-vp9"]
async fn trim_batch_history_and_cancel_preserve_interval_and_cleanup_assets() {
    let fixture = Fixture::new();
    let input = fixture.0.join("batch source.mkv");
    synthesize(&input, 8, false, false, "320x180", 48, ("bt709", "left")).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let preview=manager.preview_encode_batch(serde_json::from_value(json!({
        "inputs":[{"inputPath":input.to_string_lossy(),"videoStreamIndex":1,"streamIndices":[3,2,1,4],"trim":{"startFrame":12,"endFrameExclusive":36},"audio":[{"streamIndex":2,"codec":"flac","bitrateKbps":128,"channels":"preserve"}]}],
        "outputDirectory":fixture.0.to_string_lossy(),"backend":"standalone","encoder":"vp9","crf":32,"preset":4
    })).unwrap()).await.unwrap();
    let reviewed = preview.items[0]
        .request
        .clone()
        .expect("reviewed trim request");
    assert!(reviewed.settings.trim.is_some());
    let jobs = manager
        .enqueue_encode_batch(vec![reviewed.clone()])
        .await
        .unwrap();
    let result = wait_for(&manager, &jobs[0].id, |job| job.state.is_terminal()).await;
    assert_eq!(result.state, JobState::Succeeded, "{result:#?}");
    manager.shutdown().await;
    drop(manager);
    let restored = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    restored.ready().await.unwrap();
    assert_eq!(
        restored.list_jobs().await[0].encode_settings.as_ref(),
        Some(&reviewed.settings)
    );
    restored.shutdown().await;
    assert_eq!(std::fs::read(&input).unwrap(), original);
    let input = fixture.0.join("cancel source.mkv");
    synthesize(&input, 8, false, false, "640x360", 480, ("bt709", "left")).await;
    let original = std::fs::read(&input).unwrap();
    let destination = fixture.0.join("never published.mkv");
    let mut request = request(&input, &destination, VideoEncoder::Vp9, 0);
    request.settings.trim =
        Some(serde_json::from_value(json!({"startFrame":12,"endFrameExclusive":468})).unwrap());
    request.settings.audio = serde_json::from_value(
        json!([{"streamIndex":2,"codec":"flac","bitrateKbps":128,"channels":"preserve"}]),
    )
    .unwrap();
    let manager = JobManager::new(fixture.0.join("cancel-logs"));
    let job = manager.start_encode(request).await.unwrap();
    let active = wait_for(&manager, &job.id, |job| {
        job.state == JobState::Running
            && job
                .logs
                .iter()
                .any(|line| line.contains("[consumer] frame="))
    })
    .await;
    assert_eq!(active.state, JobState::Running, "{active:#?}");
    manager.cancel_job(job.id.clone()).await.unwrap();
    let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    assert_eq!(result.state, JobState::Canceled, "{result:#?}");
    assert!(!destination.exists());
    assert_eq!(std::fs::read(input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
}

#[tokio::test]
#[ignore = "requires matching FFmpeg/FFprobe with AAC/Opus/MP3 and x264"]
async fn trim_codec_delay_and_positive_audio_offset_preserve_sample_boundaries() {
    let fixture = Fixture::new();
    let base = fixture.0.join("base.mkv");
    synthesize(&base, 8, false, false, "320x180", 48, ("bt709", "left")).await;
    let input = fixture.0.join("audio offset.mkv");
    output(
        command("ffmpeg")
            .args(["-v", "error", "-n", "-i"])
            .arg(&base)
            .args([
                "-map",
                "0",
                "-c",
                "copy",
                "-c:a",
                "pcm_s16le",
                "-af",
                "asetpts=PTS+0.012/TB",
                "-avoid_negative_ts",
                "disabled",
            ])
            .arg(&input),
    )
    .await;
    let original = std::fs::read(&input).unwrap();
    for codec in ["aac", "opus", "mp3"] {
        for start in [0, 12] {
            let destination = fixture.0.join(format!("{codec}-{start}.mkv"));
            let mut request = request(&input, &destination, VideoEncoder::X264, 0);
            request.settings.trim = Some(
                serde_json::from_value(json!({"startFrame":start,"endFrameExclusive":start+24}))
                    .unwrap(),
            );
            request.settings.audio = serde_json::from_value(
                json!([{"streamIndex":2,"codec":codec,"bitrateKbps":128,"channels":"preserve"}]),
            )
            .unwrap();
            let manager = JobManager::new(fixture.0.join(format!("logs-{codec}-{start}")));
            let job = manager.start_encode(request).await.unwrap();
            let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
            manager.shutdown().await;
            assert_eq!(
                result.state,
                JobState::Succeeded,
                "{codec} start={start}: {result:#?}"
            );
            let frames = probe(
                &destination,
                &[
                    "-select_streams",
                    "a:0",
                    "-show_frames",
                    "-show_entries",
                    "frame=best_effort_timestamp_time,nb_samples",
                ],
            )
            .await;
            let frames = frames["frames"].as_array().unwrap();
            let audible_start = frames[0]["best_effort_timestamp_time"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap();
            assert!((audible_start - if start == 0 { 0.012 } else { 0.0 }).abs() < 0.002);
            let sample_count: u64 = frames
                .iter()
                .map(|frame| frame["nb_samples"].as_u64().unwrap())
                .sum();
            let expected_samples = if start == 0 { 47_472 } else { 48_048 };
            let padding = sample_count as i64 - expected_samples;
            assert!(
                (-2..=if codec == "aac" { 1025 } else { 2 }).contains(&padding),
                "{codec}: padding {padding}"
            );
            let pcm = output(
                command("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(&destination)
                    .args(["-map", "0:a:0", "-f", "s16le", "-"]),
            )
            .await;
            assert_eq!(
                pcm.len() as u64 / 2,
                sample_count,
                "FFprobe and actual decoder must agree after priming/discard padding"
            );
            assert_eq!(
                probe(
                    &destination,
                    &[
                        "-select_streams",
                        "v:0",
                        "-count_frames",
                        "-show_entries",
                        "stream=nb_read_frames"
                    ]
                )
                .await["streams"][0]["nb_read_frames"],
                "24"
            );
        }
    }
    assert_eq!(std::fs::read(input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
}
