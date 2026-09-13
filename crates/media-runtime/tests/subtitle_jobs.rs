// Opt-in real subtitle conversion/rendering gates.
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
            "jesses-subtitles-{}-{nonce}-{serial}",
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
        32 * 1024 * 1024,
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

const ASS: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 320\nPlayResY: 180\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Jesses Fixture,30,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,2,10,10,10,1\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.25,0:00:01.75,Default,,0,0,0,,A {\\i1}styled{\\i0} line\\Nsecond & third\n";

async fn source(root: &Path, extension: &str, body: &str, font: bool) -> PathBuf {
    let input = root.join(format!("source {extension} '[]; résumé.mkv"));
    let asset = root.join(format!("captions.{extension}"));
    std::fs::write(&asset, body).unwrap();
    let mut command = command("ffmpeg");
    command.args(["-v", "error", "-n", "-f", "lavfi", "-i", "color=black:size=320x180:rate=24:duration=2,format=yuv420p,setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709", "-f", extension, "-i"])
        .arg(asset).args(["-map", "1:0", "-map", "0:0", "-c", "copy", "-c:v", "ffv1", "-color_range", "tv", "-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-chroma_sample_location", "left", "-metadata:s:s:0", "language=eng", "-metadata:s:s:0", "title=Selected captions", "-metadata:s:s:0", "DURATION-eng=00:10:00.000000000"]);
    if font {
        command
            .arg("-attach")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tests/fixtures/subtitle-font.ttf"),
            )
            .args([
                "-metadata:s:t:0",
                "mimetype=application/octet-stream",
                "-metadata:s:t:0",
                "filename=../../untrusted-font.ttf",
            ]);
    }
    output(command.arg(&input)).await;
    input
}

fn request(input: &Path, destination: &Path, mode: &str) -> EncodeRequest {
    let mut settings = EncodeSettings {
        backend: EncodeBackend::Standalone,
        encoder: VideoEncoder::Vp9,
        video_stream_index: 1,
        crf: 0,
        preset: 5,
        ..Default::default()
    };
    settings.subtitles = serde_json::from_value(json!([{"streamIndex":0,"mode":mode}])).unwrap();
    EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: destination.to_string_lossy().into_owned(),
            stream_indices: vec![0, 1],
        },
        settings,
    }
}

async fn run(root: &Path, request: EncodeRequest) -> JobSnapshot {
    let manager = JobManager::new(root.join("logs"));
    let started = manager.start_encode(request).await.unwrap();
    let result = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    result
}

fn clean(root: &Path) {
    assert!(
        std::fs::read_dir(root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jesses-")),
        "Subtitle assets and partial outputs must be removed"
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg libvpx-vp9 and text subtitle codecs"]
async fn converts_selected_text_tracks_and_trim_assets_without_losing_readable_text() {
    for (extension, text) in [
        ("ass", ASS),
        (
            "srt",
            "1\n00:00:00,250 --> 00:00:01,750\nA <i>styled</i> line\nsecond & third\n\n",
        ),
        (
            "webvtt",
            "WEBVTT\n\n00:00:00.250 --> 00:00:01.750\nA <i>styled</i> line\nsecond & third\n\n",
        ),
    ] {
        for (mode, codec) in [("subRip", "subrip"), ("ass", "ass"), ("webVtt", "webvtt")] {
            let fixture = Fixture::new();
            let input = source(&fixture.0, extension, text, false).await;
            let original = std::fs::read(&input).unwrap();
            let original_time = std::fs::metadata(&input).unwrap().modified().unwrap();
            let destination = fixture.0.join("converted.mkv");
            let mut request = request(&input, &destination, mode);
            request.settings.trim =
                serde_json::from_value(json!({"startFrame":12,"endFrameExclusive":36})).unwrap();
            let result = run(&fixture.0, request).await;
            assert_eq!(
                result.state,
                JobState::Succeeded,
                "{extension} -> {mode}: {result:#?}"
            );
            let actual = probe(&destination, &["-show_streams", "-count_frames"]).await;
            assert_eq!(actual["streams"][0]["codec_name"], codec);
            assert_eq!(actual["streams"][0]["tags"]["language"], "eng");
            assert_eq!(actual["streams"][0]["tags"]["title"], "Selected captions");
            assert!(actual["streams"][0]["tags"].get("DURATION-eng").is_none());
            assert_eq!(actual["streams"][1]["nb_read_frames"], "24");
            let cues = output(
                command("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(&destination)
                    .args(["-map", "0:0", "-c:s", "srt", "-f", "srt", "-"]),
            )
            .await;
            let cues = String::from_utf8(cues).unwrap();
            assert!(cues.contains("00:00:00,000 --> 00:00:01,000"), "{cues}");
            assert!(
                cues.contains("styled") && cues.contains("second") && cues.contains("third"),
                "{cues}"
            );
            assert_eq!(std::fs::read(&input).unwrap(), original);
            assert_eq!(
                std::fs::metadata(input).unwrap().modified().unwrap(),
                original_time
            );
            clean(&fixture.0);
        }
    }
}

async fn gray(path: &Path, frame: usize) -> Vec<u8> {
    output(
        command("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(path)
            .args([
                "-map",
                "0:v:0",
                "-vf",
                &format!("select=eq(n\\,{frame})"),
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "gray",
                "-",
            ]),
    )
    .await
}

#[tokio::test]
#[ignore = "requires FFmpeg libvpx-vp9 and libass"]
async fn burns_embedded_font_after_resize_before_borders_and_rebases_trimmed_cues() {
    let fixture = Fixture::new();
    let input = source(&fixture.0, "ass", ASS, true).await;
    let original = std::fs::read(&input).unwrap();
    let destination = fixture.0.join("burned '[]; 日本語.mkv");
    let mut request = request(&input, &destination, "burnIn");
    request.settings.trim =
        serde_json::from_value(json!({"startFrame":12,"endFrameExclusive":48})).unwrap();
    request.settings.framing = serde_json::from_value(json!({"crop":{"top":0,"right":0,"bottom":0,"left":32},"resizeWidth":160,"borders":{"top":8,"right":16,"bottom":24,"left":16}})).unwrap();
    let result = run(&fixture.0, request).await;
    assert_eq!(result.state, JobState::Succeeded, "{result:#?}");
    let actual = probe(&destination, &["-show_streams", "-count_frames"]).await;
    assert_eq!(
        actual["streams"].as_array().unwrap().len(),
        1,
        "Burned track and unselected fonts must not remain as output tracks"
    );
    assert_eq!(actual["streams"][0]["width"], 192);
    assert_eq!(actual["streams"][0]["height"], 132);
    assert_eq!(actual["streams"][0]["nb_read_frames"], "36");
    let pixels = gray(&destination, 0).await;
    assert_eq!(pixels.len(), 192 * 132);
    let lit: Vec<_> = pixels
        .iter()
        .enumerate()
        .filter(|(_, value)| **value > 220)
        .map(|(index, _)| (index % 192, index / 192))
        .collect();
    assert!(
        lit.len() > 350,
        "Generated embedded block font should render substantial white glyph area: {}",
        lit.len()
    );
    assert!(
        lit.iter()
            .all(|(x, y)| (16..176).contains(x) && (8..108).contains(y)),
        "Text must stay inside resized content, before black borders"
    );
    assert!(
        gray(&destination, 35).await.iter().all(|value| *value < 3),
        "After rebased cue ends the picture must return to black"
    );
    assert_eq!(std::fs::read(input).unwrap(), original);
    clean(&fixture.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg libvpx-vp9"]
async fn subtitle_drawings_reject_text_conversion_and_cancel_removes_owned_assets() {
    let fixture = Fixture::new();
    let input = source(
        &fixture.0,
        "ass",
        &ASS.replace(
            "A {\\i1}styled{\\i0} line\\Nsecond & third",
            "{\\p1}m 0 0 l 20 0 20 20 0 20{\\p0}",
        ),
        false,
    )
    .await;
    let destination = fixture.0.join("unsupported.mkv");
    let rejected = run(&fixture.0, request(&input, &destination, "subRip")).await;
    assert_eq!(rejected.state, JobState::Failed);
    assert!(!destination.exists());
    clean(&fixture.0);
    let manager = JobManager::new(fixture.0.join("cancel-logs"));
    let mut request = request(&input, &destination, "burnIn");
    request.settings.encoder = VideoEncoder::Vp9;
    request.settings.preset = 0;
    let job = manager.start_encode(request).await.unwrap();
    let running = wait_for(&manager, &job.id, |job| job.state == JobState::Running).await;
    assert_eq!(running.state, JobState::Running);
    manager.cancel_job(job.id.clone()).await.unwrap();
    assert_eq!(
        wait_for(&manager, &job.id, |job| job.state.is_terminal())
            .await
            .state,
        JobState::Canceled
    );
    manager.shutdown().await;
    assert!(!destination.exists());
    clean(&fixture.0);
}

// Original PGS fixture: a white 40x20 rectangle at source (20,80), displayed
// from 250ms to 1500ms. Segment layout follows FFmpeg's pgssubdec.c parser.
fn bitmap_fixture(path: &Path) {
    fn segment(bytes: &mut Vec<u8>, pts: u32, kind: u8, payload: &[u8]) {
        bytes.extend(b"PG");
        bytes.extend(pts.to_be_bytes());
        bytes.extend(pts.to_be_bytes());
        bytes.push(kind);
        bytes.extend((payload.len() as u16).to_be_bytes());
        bytes.extend(payload);
    }
    let mut bytes = Vec::new();
    segment(
        &mut bytes,
        22500,
        0x16,
        &[
            1, 64, 0, 180, 0x10, 0, 0, 0x80, 0, 0, 1, 0, 0, 0, 0, 0, 20, 0, 80,
        ],
    );
    segment(&mut bytes, 22500, 0x17, &[1, 0, 0, 0, 0, 0, 1, 64, 0, 180]);
    segment(
        &mut bytes,
        22500,
        0x14,
        &[0, 0, 0, 16, 128, 128, 0, 1, 235, 128, 128, 255],
    );
    let mut rle = Vec::new();
    for _ in 0..20 {
        rle.extend([1; 40]);
        rle.extend([0, 0]);
    }
    let mut object = vec![0, 0, 0, 0xc0];
    object.extend(&((rle.len() + 4) as u32).to_be_bytes()[1..]);
    object.extend([0, 40, 0, 20]);
    object.extend(rle);
    segment(&mut bytes, 22500, 0x15, &object);
    segment(&mut bytes, 22500, 0x80, &[]);
    segment(
        &mut bytes,
        135000,
        0x16,
        &[1, 64, 0, 180, 0x10, 0, 1, 0, 0, 0, 0],
    );
    segment(&mut bytes, 135000, 0x80, &[]);
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
#[ignore = "requires FFmpeg libvpx-vp9 and PGS bitmap decoding"]
async fn bitmap_burn_happens_in_source_coordinates_before_crop_and_resize() {
    for hdr in [false, true] {
        let fixture = Fixture::new();
        let bitmap = fixture.0.join("rectangle.sup");
        bitmap_fixture(&bitmap);
        let input = fixture.0.join("bitmap.mkv");
        let (primaries, transfer, matrix, pixels) = if hdr {
            ("bt2020", "smpte2084", "bt2020nc", "yuv420p10le")
        } else {
            ("bt709", "bt709", "bt709", "yuv420p")
        };
        let pattern = format!(
            "color=black:size=320x180:rate=24:duration=2,format={pixels},setparams=range=tv:color_primaries={primaries}:color_trc={transfer}:colorspace={matrix}"
        );
        output(
            command("ffmpeg")
                .args(["-v", "error", "-n", "-f", "lavfi", "-i", &pattern, "-i"])
                .arg(&bitmap)
                .args([
                    "-map",
                    "1:0",
                    "-map",
                    "0:0",
                    "-c",
                    "copy",
                    "-c:v",
                    "ffv1",
                    "-color_range",
                    "tv",
                    "-color_primaries",
                    primaries,
                    "-color_trc",
                    transfer,
                    "-colorspace",
                    matrix,
                    "-chroma_sample_location",
                    "left",
                ])
                .arg(&input),
        )
        .await;
        let original = std::fs::read(&input).unwrap();
        let destination = fixture.0.join("bitmap burned.mkv");
        let mut request = request(&input, &destination, "burnIn");
        if hdr {
            request.settings.tone_map =
                serde_json::from_value(json!({"sourcePeakNits":1000,"hdr10BaseLayer":false}))
                    .unwrap();
        }
        request.settings.framing = serde_json::from_value(json!({"crop":{"top":0,"right":0,"bottom":0,"left":32},"resizeWidth":144,"borders":{"top":8,"right":16,"bottom":24,"left":16}})).unwrap();
        let result = run(&fixture.0, request).await;
        assert_eq!(result.state, JobState::Succeeded, "{result:#?}");
        let info = probe(&destination, &["-show_streams", "-count_frames"]).await;
        assert_eq!(info["streams"].as_array().unwrap().len(), 1);
        assert_eq!(info["streams"][0]["nb_read_frames"], "48");
        assert!(gray(&destination, 0).await.iter().all(|value| *value < 3));
        let pixels = gray(&destination, 24).await;
        assert_eq!(pixels.len(), 176 * 122);
        let lit: Vec<_> = pixels
            .iter()
            .enumerate()
            .filter(|(_, value)| **value > 220)
            .map(|(index, _)| (index % 176, index / 176))
            .collect();
        assert!(
            (100..=160).contains(&lit.len()),
            "{} white pixels",
            lit.len()
        );
        assert!(
            lit.iter()
                .all(|(x, y)| (16..30).contains(x) && (48..58).contains(y)),
            "Wrong bitmap placement: {lit:?}"
        );
        assert!(gray(&destination, 47).await.iter().all(|value| *value < 3));
        assert_eq!(std::fs::read(input).unwrap(), original);
        clean(&fixture.0);
    }
}
