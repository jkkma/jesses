//! Opt-in actual-tool checks for subtitles owned by a second input file.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_core::SubtitleMode;
use media_runtime::{
    EncodeRequest, EncodeSettings, ExternalTrack, JobManager, JobSnapshot, JobState, RemuxRequest,
    VideoEncoder,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::Value;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let base = std::env::var_os("JESSES_TEST_EXTERNAL_SUBTITLE_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&base).unwrap();
        let root = base.join(format!(
            "jesses-external-subtitles-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking()
            || std::env::var_os("JESSES_TEST_EXTERNAL_SUBTITLE_EVIDENCE").is_some()
        {
            eprintln!("External subtitle evidence retained: {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

async fn tool(name: &str, args: Vec<OsString>) -> Vec<u8> {
    if let Some(resources) = std::env::var_os("JESSES_TEST_TOOL_RESOURCES") {
        media_runtime::configure_bundled_tools(PathBuf::from(resources)).unwrap();
    }
    let executable = media_runtime::get_capabilities()
        .await
        .into_iter()
        .find(|tool| tool.id == name)
        .unwrap()
        .path
        .expect("required native tool");
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(
        &CommandSpec {
            executable: executable.into(),
            args,
            cwd: None,
        },
        cancel,
        16 * 1024 * 1024,
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

async fn sources(root: &Path, ass: bool) -> (PathBuf, PathBuf) {
    let video = root.join("picture $'主.mkv");
    let subtitle = root.join(if ass { "external.ass" } else { "external.srt" });
    let external = root.join("captions and font.mkv");
    let font = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/subtitle-font.ttf");
    let text = if ass {
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 64\nPlayResY: 64\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Jesses Fixture,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,2,1,1,1,1\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.20,0:00:01.20,Default,,0,0,0,,External burn\n"
    } else {
        "1\n00:00:00,200 --> 00:00:01,300\nExternal conversion\n\n"
    };
    std::fs::write(&subtitle, text).unwrap();
    let mut args = strings(&[
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "color=black:s=64x64:r=24:d=2,format=yuv420p,setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-map",
        "0:v",
        "-c:v",
        "ffv1",
        "-pix_fmt",
        "yuv420p",
        "-chroma_sample_location",
        "left",
    ]);
    args.push(video.clone().into_os_string());
    tool("ffmpeg", args).await;
    let mut args = strings(&["-v", "error", "-nostdin", "-i"]);
    args.push(subtitle.into_os_string());
    args.extend(strings(&["-map", "0:s", "-c:s", "copy", "-attach"]));
    args.push(font.into_os_string());
    args.extend(strings(&[
        "-metadata:s:t:0",
        "mimetype=application/x-truetype-font",
        "-metadata:s:t:0",
        "filename=external.ttf",
        "-metadata:s:s:0",
        "language=jpn",
    ]));
    args.push(external.clone().into_os_string());
    tool("ffmpeg", args).await;
    (video, external)
}

fn request(video: &Path, external: &Path, output: &Path, mode: SubtitleMode) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: video.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::X264,
            preset: 0,
            external_tracks: vec![ExternalTrack {
                input_path: external.to_string_lossy().into_owned(),
                stream_index: 0,
                audio: None,
                subtitle_mode: Some(mode),
                title: None,
                language: None,
                default: None,
                forced: None,
                offset_milliseconds: 500,
            }],
            ..Default::default()
        },
    }
}

async fn run(root: &Path, request: EncodeRequest) -> JobSnapshot {
    let manager = JobManager::new(root.join("logs"));
    let job = manager.start_encode(request).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|candidate| candidate.id == job.id)
                .unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("subtitle encode completed");
    manager.shutdown().await;
    result
}

async fn probe(path: &Path) -> Value {
    let mut args = strings(&["-v", "error", "-of", "json", "-show_streams"]);
    args.push(path.as_os_str().to_owned());
    serde_json::from_slice(&tool("ffprobe", args).await).unwrap()
}

// A generated PGS display set: one opaque white 16x8 rectangle at (24,48),
// visible from 0.25 to 1.25 seconds. No copyrighted subtitle media is needed.
fn bitmap_fixture(path: &Path) {
    fn segment(bytes: &mut Vec<u8>, pts: u32, kind: u8, payload: &[u8]) {
        bytes.extend(b"PG");
        bytes.extend(pts.to_be_bytes());
        bytes.extend(0_u32.to_be_bytes());
        bytes.push(kind);
        bytes.extend((payload.len() as u16).to_be_bytes());
        bytes.extend(payload);
    }
    let mut bytes = Vec::new();
    segment(
        &mut bytes,
        22_500,
        0x16,
        &[
            0, 64, 0, 64, 0x10, 0, 0, 0x80, 0, 0, 1, 0, 0, 0, 0, 0, 24, 0, 48,
        ],
    );
    segment(&mut bytes, 22_500, 0x17, &[1, 0, 0, 0, 0, 0, 0, 64, 0, 64]);
    segment(
        &mut bytes,
        22_500,
        0x14,
        &[0, 0, 0, 16, 128, 128, 0, 1, 235, 128, 128, 255],
    );
    let rle: Vec<u8> = (0..8).flat_map(|_| [0, 0x90, 1, 0, 0]).collect();
    let mut object = vec![0, 0, 0, 0xc0, 0, 0, (rle.len() + 4) as u8, 0, 16, 0, 8];
    object.extend(rle);
    segment(&mut bytes, 22_500, 0x15, &object);
    segment(&mut bytes, 22_500, 0x80, &[]);
    segment(
        &mut bytes,
        112_500,
        0x16,
        &[0, 64, 0, 64, 0x10, 0, 1, 0, 0, 0, 0],
    );
    segment(&mut bytes, 112_500, 0x80, &[]);
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
#[ignore = "requires FFmpeg, x264 and PGS subtitle decoding"]
async fn external_bitmap_burn_tracks_offset_and_trim_at_decoded_frame_boundaries() {
    let fixture = Fixture::new();
    let (video, _) = sources(&fixture.0, false).await;
    let external = fixture.0.join("bitmap captions.sup");
    bitmap_fixture(&external);
    let original = std::fs::read(&external).unwrap();
    for (trimmed, offset) in [(false, 500), (true, 500), (false, -500), (true, -500)] {
        let output = fixture.0.join(format!("bitmap-{trimmed}-{offset}.mkv"));
        let mut request = request(&video, &external, &output, SubtitleMode::BurnIn);
        request.settings.external_tracks[0].offset_milliseconds = offset;
        request.settings.lossless = true;
        if trimmed {
            request.settings.trim =
                serde_json::from_value(serde_json::json!({"startFrame":12,"endFrameExclusive":36}))
                    .unwrap();
        }
        let job = run(&fixture.0, request).await;
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        let document = probe(&output).await;
        assert_eq!(document["streams"].as_array().unwrap().len(), 1);
        let mut args = strings(&["-v", "error", "-i"]);
        args.push(output.into_os_string());
        args.extend(strings(&[
            "-map", "0:v:0", "-pix_fmt", "gray", "-f", "rawvideo", "-",
        ]));
        let frames = tool("ffmpeg", args).await;
        assert_eq!(frames.len(), 64 * 64 * if trimmed { 24 } else { 48 });
        for (index, frame) in frames.as_chunks::<{ 64 * 64 }>().0.iter().enumerate() {
            let source_time = index as f64 / 24.0 + if trimmed { 0.5 } else { 0.0 };
            let lit = frame[51 * 64 + 30] > 200;
            let shift = f64::from(offset) / 1000.0;
            assert_eq!(
                lit,
                (0.25 + shift..1.25 + shift).contains(&source_time),
                "offset {offset}, frame {index}, source time {source_time}, center {}",
                frame[51 * 64 + 30]
            );
            assert!(frame[8 * 64 + 8] < 10, "background must remain black");
        }
    }
    assert_eq!(std::fs::read(external).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires managed QTGMC, FFmpeg, x264, libass and PGS decoding"]
async fn qtgmc_burns_external_text_and_bitmap_into_the_verified_intermediate() {
    let fixture = Fixture::new();
    let (video, text) = sources(&fixture.0, true).await;
    let interlaced = fixture.0.join("interlaced.mkv");
    let mut args = strings(&["-v", "error", "-i"]);
    args.push(video.into_os_string());
    args.extend(strings(&[
        "-vf",
        "setfield=tff",
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
    ]));
    args.push(interlaced.clone().into_os_string());
    tool("ffmpeg", args).await;
    let bitmap = fixture.0.join("captions.sup");
    bitmap_fixture(&bitmap);
    for (name, external) in [("text", text), ("bitmap", bitmap)] {
        let output = fixture.0.join(format!("qtgmc-{name}.mkv"));
        let mut request = request(&interlaced, &external, &output, SubtitleMode::BurnIn);
        request.settings.lossless = true;
        request.settings.temporal = Some(media_core::TemporalSettings {
            qtgmc: Some(media_core::QtgmcSettings {
                mode: media_core::DeinterlaceMode::Bob,
                field_order: media_core::FieldOrder::TopFirst,
                preset: media_core::QtgmcPreset::Fast,
            }),
            ..Default::default()
        });
        request.settings.trim =
            serde_json::from_value(serde_json::json!({"startFrame":12,"endFrameExclusive":36}))
                .unwrap();
        let before = std::fs::read(&external).unwrap();
        let job = run(&fixture.0, request).await;
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        assert!(
            job.logs
                .iter()
                .any(|line| line.contains("QTGMC uses one owned FFV1 intermediate"))
        );
        let mut args = strings(&["-v", "error", "-i"]);
        args.push(output.into_os_string());
        args.extend(strings(&[
            "-map", "0:v:0", "-pix_fmt", "gray", "-f", "rawvideo", "-",
        ]));
        let frames = tool("ffmpeg", args).await;
        assert_eq!(frames.len(), 64 * 64 * 48);
        for (index, frame) in frames.as_chunks::<{ 64 * 64 }>().0.iter().enumerate() {
            let lit = frame.iter().any(|pixel| *pixel > 200);
            let expected_start = if name == "bitmap" { 12 } else { 10 };
            assert_eq!(lit, index >= expected_start, "{name} frame {index}");
        }
        assert_eq!(std::fs::read(external).unwrap(), before);
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, x264 and subtitle codecs"]
async fn external_text_conversion_uses_source_local_timing_and_trim() {
    let fixture = Fixture::new();
    let (video, external) = sources(&fixture.0, false).await;
    let original = std::fs::read(&external).unwrap();
    let output = fixture.0.join("converted.mkv");
    let mut request = request(&video, &external, &output, SubtitleMode::Ass);
    request.settings.external_tracks[0].title = Some("Converted external captions".into());
    request.settings.external_tracks[0].language = Some("spa".into());
    request.settings.external_tracks[0].default = Some(true);
    request.settings.trim = serde_json::from_value(serde_json::json!({
        "startFrame": 12, "endFrameExclusive": 36
    }))
    .unwrap();
    let job = run(&fixture.0, request).await;
    assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
    let document = probe(&output).await;
    assert_eq!(document["streams"][1]["codec_name"], "ass");
    assert_eq!(document["streams"][1]["tags"]["language"], "spa");
    assert_eq!(
        document["streams"][1]["tags"]["title"],
        "Converted external captions"
    );
    assert_eq!(document["streams"][1]["disposition"]["default"], 1);
    let mut args = strings(&["-v", "error", "-i"]);
    args.push(output.into_os_string());
    args.extend(strings(&["-map", "0:1", "-c:s", "srt", "-f", "srt", "-"]));
    let cues = String::from_utf8(tool("ffmpeg", args).await).unwrap();
    assert!(
        cues.contains("00:00:00,200 --> 00:00:01,000"),
        "shifted and trimmed cue: {cues}"
    );
    assert_eq!(std::fs::read(external).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires FFmpeg, x264, libass and subtitle fonts"]
async fn external_ass_burn_uses_subtitle_only_source_and_its_fonts() {
    let fixture = Fixture::new();
    let (video, external) = sources(&fixture.0, true).await;
    let output = fixture.0.join("burned.mkv");
    let job = run(
        &fixture.0,
        request(&video, &external, &output, SubtitleMode::BurnIn),
    )
    .await;
    assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
    let document = probe(&output).await;
    assert_eq!(document["streams"].as_array().unwrap().len(), 1);
    let frame = |time: &str| {
        let mut args = strings(&["-v", "error", "-ss", time, "-i"]);
        args.push(output.as_os_str().to_owned());
        args.extend(strings(&["-frames:v", "1", "-f", "framemd5", "-"]));
        args
    };
    let hash = |output: Vec<u8>| {
        String::from_utf8(output)
            .unwrap()
            .lines()
            .find(|line| !line.starts_with('#') && line.contains(','))
            .unwrap()
            .rsplit(',')
            .next()
            .unwrap()
            .trim()
            .to_owned()
    };
    assert_ne!(
        hash(tool("ffmpeg", frame("0.08")).await),
        hash(tool("ffmpeg", frame("0.75")).await),
        "subtitle rendering must alter the video frame"
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg, x264 and subtitle codecs"]
async fn negative_external_text_offset_clips_only_the_invisible_lead() {
    let fixture = Fixture::new();
    let (video, external) = sources(&fixture.0, false).await;
    let output = fixture.0.join("negative-offset.mkv");
    let mut request = request(&video, &external, &output, SubtitleMode::Ass);
    request.settings.external_tracks[0].offset_milliseconds = -500;
    let job = run(&fixture.0, request).await;
    assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
    let mut args = strings(&["-v", "error", "-i"]);
    args.push(output.into_os_string());
    args.extend(strings(&["-map", "0:1", "-c:s", "srt", "-f", "srt", "-"]));
    let cues = String::from_utf8(tool("ffmpeg", args).await).unwrap();
    assert!(
        cues.contains("00:00:00,000 --> 00:00:00,800"),
        "negative shifted cue should be clipped at the visible boundary: {cues}"
    );
}
