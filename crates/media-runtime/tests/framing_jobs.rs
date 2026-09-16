//! Opt-in native crop/resize/border qualification through the public JobManager.
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
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jesses-framing-{}-{nonce}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Failed framing fixture retained at {}", self.0.display());
            return;
        }
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
    result.expect("native framing job must progress within 90 seconds")
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

fn framing() -> media_runtime::VideoFraming {
    serde_json::from_value(json!({
        "crop": {"top":10,"bottom":10,"left":16,"right":16},
        "resizeWidth":192
    }))
    .unwrap()
}

fn bordered_framing() -> media_runtime::VideoFraming {
    serde_json::from_value(json!({
        "crop": {"top":6,"bottom":14,"left":12,"right":20},
        "resizeWidth":192,
        "borders": {"top":32,"right":48,"bottom":48,"left":32}
    }))
    .unwrap()
}

fn borders_only() -> media_runtime::VideoFraming {
    serde_json::from_value(json!({
        "borders": {"top":48,"right":32,"bottom":32,"left":48}
    }))
    .unwrap()
}

#[derive(Clone, Copy, Debug)]
enum FramingCase {
    CropResize,
    CropResizeBorders,
    BordersOnly,
}

impl FramingCase {
    fn settings(self) -> media_runtime::VideoFraming {
        match self {
            Self::CropResize => framing(),
            Self::CropResizeBorders => bordered_framing(),
            Self::BordersOnly => borders_only(),
        }
    }

    fn content_size(self) -> (usize, usize) {
        match self {
            Self::CropResize | Self::CropResizeBorders => (192, 106),
            Self::BordersOnly => (320, 180),
        }
    }

    // An independent crop/resize reference deliberately contains no pad filter.
    // Expected border pixels are constructed from planar YUV values below.
    fn reference_filter(self, full: bool) -> Option<String> {
        let (left, top) = match self {
            Self::CropResize => (16, 10),
            Self::CropResizeBorders => (12, 6),
            Self::BordersOnly => return None,
        };
        let range = if full { "pc" } else { "tv" };
        Some(format!(
            "crop=288:160:{left}:{top}:exact=1,scale=192:106:flags=lanczos:in_range={range}:out_range={range}:in_color_matrix=bt709:out_color_matrix=bt709:in_h_chr_pos=0:out_h_chr_pos=0:in_v_chr_pos=128:out_v_chr_pos=128,setsar=1"
        ))
    }
}

async fn decoded_pixels(
    path: &Path,
    stream: &str,
    filter: Option<&str>,
    depth: u8,
    full: bool,
    frames: u32,
) -> Vec<u8> {
    let mut cmd = command("ffmpeg");
    cmd.args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-map", stream, "-an", "-sn", "-dn"]);
    if let Some(filter) = filter {
        cmd.args(["-vf", filter]);
    }
    // Preserve the decoded range instead of implicitly converting yuvj420p to
    // limited-range yuv420p during inspection.
    let format = match (depth, full) {
        (10, _) => "yuv420p10le",
        (_, true) => "yuvj420p",
        _ => "yuv420p",
    };
    cmd.args([
        "-frames:v",
        &frames.to_string(),
        "-pix_fmt",
        format,
        "-f",
        "rawvideo",
        "-",
    ]);
    output(&mut cmd).await
}

fn border_edges(framing: media_runtime::VideoFraming) -> [usize; 4] {
    let borders = framing.borders;
    [borders.top, borders.right, borders.bottom, borders.left].map(|edge| edge as usize)
}

fn assert_lossless_reference(actual: &[u8], content: &[u8], case: FramingCase, full: bool) {
    let (content_width, content_height) = case.content_size();
    let [top, right, bottom, left] = border_edges(case.settings());
    let (width, height) = (content_width + left + right, content_height + top + bottom);
    let content_frame_size = content_width * content_height * 3 / 2;
    let frame_size = width * height * 3 / 2;
    assert_eq!(content.len(), content_frame_size * 48);
    assert_eq!(actual.len(), frame_size * 48);
    for (index, (actual, content)) in actual
        .chunks_exact(frame_size)
        .zip(content.chunks_exact(content_frame_size))
        .enumerate()
    {
        let mut actual_offset = 0;
        let mut content_offset = 0;
        for plane in 0..3 {
            let divisor = if plane == 0 { 1 } else { 2 };
            let (plane_width, plane_height) = (width / divisor, height / divisor);
            let (inner_width, inner_height) = (content_width / divisor, content_height / divisor);
            let black = if plane == 0 {
                if full { 0 } else { 16 }
            } else {
                128
            };
            let mut expected = vec![black; plane_width * plane_height];
            for row in 0..inner_height {
                let start = (top / divisor + row) * plane_width + left / divisor;
                let source = content_offset + row * inner_width;
                expected[start..start + inner_width]
                    .copy_from_slice(&content[source..source + inner_width]);
            }
            // This compares every content and border sample of every frame,
            // including exact asymmetric offsets, without using FFmpeg pad.
            if let Some(sample) = actual[actual_offset..actual_offset + expected.len()]
                .iter()
                .zip(&expected)
                .position(|(actual, expected)| actual != expected)
            {
                panic!(
                    "{case:?}, full={full}, lossless frame {index}, plane {plane}, sample ({}, {}): actual {}, expected {}",
                    sample % plane_width,
                    sample / plane_width,
                    actual[actual_offset + sample],
                    expected[sample]
                );
            }
            actual_offset += plane_width * plane_height;
            content_offset += inner_width * inner_height;
        }
    }
}

fn assert_border_levels(
    pixels: &[u8],
    content_size: (usize, usize),
    framing: media_runtime::VideoFraming,
    depth: u8,
    full: bool,
) {
    let [top, right, bottom, left] = border_edges(framing);
    assert!([top, right, bottom, left].iter().all(|edge| *edge >= 32));
    let (content_width, content_height) = content_size;
    let (width, height) = (content_width + left + right, content_height + top + bottom);
    let sample_bytes = if depth == 10 { 2 } else { 1 };
    let frame_bytes = width * height * 3 / 2 * sample_bytes;
    assert_eq!(pixels.len() % frame_bytes, 0);
    assert!(!pixels.is_empty());
    for (index, frame) in pixels.chunks_exact(frame_bytes).enumerate() {
        let mut offset = 0;
        for plane in 0..3 {
            let divisor = if plane == 0 { 1 } else { 2 };
            let (plane_width, plane_height) = (width / divisor, height / divisor);
            let (inner_width, inner_height) = (content_width / divisor, content_height / divisor);
            let (x0, y0) = (left / divisor, top / divisor);
            let margin = 16 / divisor;
            let mut samples = Vec::new();
            for y in 0..plane_height {
                for x in 0..plane_width {
                    // Inspect all four border interiors, excluding the 16-pixel
                    // neighborhood of content where lossy codecs may ring.
                    if x + margin < x0
                        || x >= x0 + inner_width + margin
                        || y + margin < y0
                        || y >= y0 + inner_height + margin
                    {
                        let sample = offset + (y * plane_width + x) * sample_bytes;
                        samples.push(if depth == 10 {
                            u16::from_le_bytes([frame[sample], frame[sample + 1]])
                        } else {
                            u16::from(frame[sample])
                        });
                    }
                }
            }
            samples.sort_unstable();
            let expected = match (plane, depth, full) {
                (0, _, true) => 0,
                (0, 10, false) => 64,
                (0, _, false) => 16,
                (_, 10, _) => 512,
                _ => 128,
            };
            let minimum = samples[0];
            let maximum = samples[samples.len() - 1];
            let median = samples[samples.len() / 2];
            // An exact median catches systematic wrong range or chroma offsets
            // (including limited 10-bit pad producing 514 instead of 512).
            assert_eq!(
                median, expected,
                "frame {index}, plane {plane}, {depth}-bit full={full}: border [{minimum}, {maximum}], median {median}"
            );
            let tolerance = if depth == 10 { 8 } else { 2 };
            assert!(
                minimum.abs_diff(expected) <= tolerance && maximum.abs_diff(expected) <= tolerance,
                "frame {index}, plane {plane}: border must stay near {expected}, got [{minimum}, {maximum}]"
            );
            offset += plane_width * plane_height * sample_bytes;
        }
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, x264 and all three SVT-AV1 builds"]
async fn framing_preserves_timing_tracks_depth_and_applies_the_requested_pixels() {
    use FramingCase::{BordersOnly, CropResize, CropResizeBorders};
    for (encoder, depth, full, case) in [
        (VideoEncoder::X264, 8, false, CropResize),
        (VideoEncoder::X264, 8, false, CropResizeBorders),
        (VideoEncoder::X264, 8, true, CropResizeBorders),
        (VideoEncoder::X264, 10, false, CropResizeBorders),
        (VideoEncoder::X264, 10, true, CropResizeBorders),
        (VideoEncoder::SvtAv1, 8, false, CropResizeBorders),
        (VideoEncoder::SvtAv1, 10, true, CropResizeBorders),
        (VideoEncoder::SvtAv1FiveFish, 8, false, CropResizeBorders),
        (VideoEncoder::SvtAv1FiveFish, 10, true, CropResizeBorders),
        (VideoEncoder::SvtAv1Hdr, 8, false, CropResizeBorders),
        (VideoEncoder::SvtAv1Hdr, 10, false, CropResizeBorders),
        (VideoEncoder::X264, 8, false, BordersOnly),
        (VideoEncoder::X264, 8, true, BordersOnly),
    ] {
        let fixture = Fixture::new();
        let input = fixture.0.join("- crop's & $ % 日本語.mkv");
        let destination = fixture.0.join("resized café & 東京.mkv");
        synthesize(&input, depth, full, false, "320x180", 48).await;
        let original = std::fs::read(&input).unwrap();
        let modified = std::fs::metadata(&input).unwrap().modified().unwrap();
        let mut request = request(&input, &destination, if encoder.is_svt() { 12 } else { 5 });
        request.settings.encoder = encoder;
        if encoder == VideoEncoder::SvtAv1FiveFish {
            request.settings.lineart_psy_bias = 5;
            request.settings.texture_psy_bias = 4;
        }
        if encoder == VideoEncoder::SvtAv1Hdr {
            request.settings.hdr_tune = media_runtime::HdrTune::FilmGrain;
        }
        request.settings.framing = case.settings();
        // Lossless 8-bit cases check all decoded pixels against a separately
        // cropped/resized source and independently constructed YUV borders.
        request.settings.crf = if encoder == VideoEncoder::X264 && depth == 8 {
            0
        } else {
            23
        };
        let manager = JobManager::new(fixture.0.join("logs"));
        let job = manager.start_encode(request.clone()).await.unwrap();
        let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        manager.shutdown().await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "{encoder:?}/{depth}/{full}/{case:?}: {result:#?}"
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
        let streams = actual["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 4);
        let video = &streams[2];
        let (content_width, content_height) = case.content_size();
        let [top, right, bottom, left] = border_edges(request.settings.framing);
        assert_eq!(
            (video["width"].as_u64(), video["height"].as_u64()),
            (
                Some((content_width + left + right) as u64),
                Some((content_height + top + bottom) as u64)
            )
        );
        assert_eq!(video["nb_read_frames"], "48");
        assert_eq!(video["sample_aspect_ratio"], "1:1");
        assert_eq!(video["color_range"], if full { "pc" } else { "tv" });
        for field in ["color_space", "color_transfer", "color_primaries"] {
            assert_eq!(video[field], "bt709");
        }
        assert_eq!(
            video["pix_fmt"],
            if encoder.is_svt() || depth == 10 {
                "yuv420p10le"
            } else if full {
                "yuvj420p"
            } else {
                "yuv420p"
            }
        );
        let frames = probe(
            &destination,
            &[
                "-select_streams",
                "2",
                "-show_frames",
                "-show_entries",
                "frame=best_effort_timestamp_time",
            ],
        )
        .await;
        for (index, frame) in frames["frames"].as_array().unwrap().iter().enumerate() {
            let pts: f64 = frame["best_effort_timestamp_time"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            assert!((pts - index as f64 * 1001.0 / 24000.0).abs() < 0.0011);
        }
        assert_eq!(packets(&input, "2").await, packets(&destination, "1").await);
        assert_eq!(packets(&input, "3").await, packets(&destination, "0").await);
        assert_eq!(
            streams[3]["extradata_hash"],
            source["streams"][4]["extradata_hash"]
        );
        assert_eq!(actual["chapters"], source["chapters"]);
        if request.settings.crf == 0 {
            let filter = case.reference_filter(full);
            let expected = decoded_pixels(&input, "0:1", filter.as_deref(), 8, full, 48).await;
            let actual = decoded_pixels(&destination, "0:2", None, 8, full, 48).await;
            assert_lossless_reference(&actual, &expected, case, full);
        } else {
            let output_depth = if encoder.is_svt() { 10 } else { depth };
            let pixels = decoded_pixels(&destination, "0:2", None, output_depth, full, 3).await;
            assert_border_levels(
                &pixels,
                case.content_size(),
                request.settings.framing,
                output_depth,
                full,
            );
        }
        assert_eq!(std::fs::read(&input).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&input).unwrap().modified().unwrap(),
            modified
        );
        assert_no_partial_output(&fixture.0);
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and SVT-AV1-HDR"]
async fn framing_batch_keeps_per_file_geometry_and_saved_history() {
    let fixture = Fixture::new();
    let input = fixture.0.join("batch source.mkv");
    synthesize(&input, 8, false, false, "320x180", 24).await;
    let original = std::fs::read(&input).unwrap();
    let existing = fixture.0.join("batch source_av1_hdr.mkv");
    std::fs::write(&existing, b"existing bordered batch destination").unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let base = BatchEncodeInput {
        temporal: None,
        tone_map: None,
        trim: None,
        subtitles: Vec::new(),
        input_path: input.to_string_lossy().into_owned(),
        stream_indices: vec![3, 2, 1, 4],
        video_stream_index: 1,
        audio: Vec::new(),
        framing: bordered_framing(),
    };
    let mut invalid = base.clone();
    invalid.framing.crop.left = 300;
    let mut invalid_borders = base.clone();
    invalid_borders.framing.borders.left = 1;
    let preview = manager
        .preview_encode_batch(BatchEncodeRequest {
            parameters: Vec::new(),
            av1an_options: None,
            output_container: None,
            rate_control: None,
            lossless: false,
            svt_crf_quarter_steps: None,
            svt_preset: None,
            inputs: vec![
                base.clone(),
                BatchEncodeInput {
                    framing: borders_only(),
                    ..base
                },
                invalid,
                invalid_borders,
            ],
            output_directory: fixture.0.to_string_lossy().into_owned(),
            backend: EncodeBackend::Standalone,
            encoder: VideoEncoder::SvtAv1Hdr,
            workers: 2,
            crf: 23,
            preset: 5,
            film_grain: 0,
            lineart_psy_bias: 0,
            texture_psy_bias: 0,
            hdr_tune: media_runtime::HdrTune::FilmGrain,
            hdr10_fallback: false,
        })
        .await
        .unwrap();
    assert_eq!(preview.items.len(), 4);
    for item in &preview.items[2..] {
        assert!(item.error.is_some());
        assert!(item.request.is_none());
    }
    let requests: Vec<_> = preview
        .items
        .into_iter()
        .filter_map(|item| item.request)
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].settings.framing, bordered_framing());
    assert_eq!(requests[1].settings.framing, borders_only());
    assert_ne!(
        requests[0].source.output_path,
        requests[1].source.output_path
    );
    assert!(
        requests
            .iter()
            .all(|request| Path::new(&request.source.output_path) != existing)
    );
    let jobs = manager
        .enqueue_encode_batch(requests.clone())
        .await
        .unwrap();
    for (job, (width, height)) in jobs.iter().zip([(272, 186), (400, 260)]) {
        let terminal = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        assert_eq!(terminal.state, JobState::Succeeded, "{terminal:#?}");
        let actual = probe(Path::new(&terminal.request.output_path), &["-show_streams"]).await;
        assert_eq!(actual["streams"][2]["width"], width);
        assert_eq!(actual["streams"][2]["height"], height);
    }
    manager.shutdown().await;
    drop(manager);
    let reopened = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    reopened.ready().await.unwrap();
    for (job, request) in jobs.iter().zip(requests) {
        let saved = reopened
            .list_jobs()
            .await
            .into_iter()
            .find(|saved| saved.id == job.id)
            .unwrap();
        assert_eq!(saved.encode_settings, Some(request.settings));
        assert_eq!(saved.state, JobState::Succeeded);
    }
    reopened.shutdown().await;
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_eq!(
        std::fs::read(&existing).unwrap(),
        b"existing bordered batch destination"
    );
    assert_no_partial_output(&fixture.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg with libx265, FFprobe and SVT-AV1-HDR"]
async fn framing_hdr10_retains_static_metadata_after_crop_resize_and_borders() {
    let fixture = Fixture::new();
    let input = fixture.0.join("HDR source.mkv");
    output(command("ffmpeg").args([
        "-v","error","-nostdin","-n","-f","lavfi","-i","testsrc2=s=320x180:r=24","-frames:v","24",
        "-vf","setparams=field_mode=prog:range=tv:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc",
        "-c:v","libx265","-preset","ultrafast","-pix_fmt","yuv420p10le","-color_range","tv","-colorspace","bt2020nc","-color_trc","smpte2084","-color_primaries","bt2020",
        "-x265-params","log-level=error:pools=2:frame-threads=2:bframes=0:hdr10=1:chromaloc=2:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=200,142"
    ]).arg(&input)).await;
    let original = std::fs::read(&input).unwrap();
    let modified = std::fs::metadata(&input).unwrap().modified().unwrap();
    let destination = fixture.0.join("HDR resized and bordered.mkv");
    let manager = JobManager::new(fixture.0.join("logs"));
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: destination.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::SvtAv1Hdr,
                hdr_tune: media_runtime::HdrTune::FilmGrain,
                framing: bordered_framing(),
                preset: 12,
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    assert_eq!(result.state, JobState::Succeeded, "{result:#?}");
    let actual = probe(&destination, &["-show_streams", "-show_frames"]).await;
    let video = &actual["streams"][0];
    assert_eq!(video["width"], 272);
    assert_eq!(video["height"], 186);
    assert_eq!(video["pix_fmt"], "yuv420p10le");
    assert_eq!(video["color_range"], "tv");
    assert_eq!(video["color_space"], "bt2020nc");
    assert_eq!(video["color_primaries"], "bt2020");
    assert_eq!(video["color_transfer"], "smpte2084");
    let metadata = actual["frames"][0]["side_data_list"].as_array().unwrap();
    assert!(
        metadata
            .iter()
            .any(|side| side["side_data_type"] == "Mastering display metadata")
    );
    assert!(metadata.iter().any(
        |side| side["side_data_type"] == "Content light level metadata"
            && side["max_content"] == 200
            && side["max_average"] == 142
    ));
    assert_eq!(actual["frames"].as_array().unwrap().len(), 24);
    let pixels = decoded_pixels(&destination, "0:0", None, 10, false, 3).await;
    assert_border_levels(&pixels, (192, 106), bordered_framing(), 10, false);
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_eq!(
        std::fs::metadata(&input).unwrap().modified().unwrap(),
        modified
    );
    assert_no_partial_output(&fixture.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and SVT-AV1-HDR"]
async fn framing_cancel_cleans_owned_outputs_and_preserves_source_and_existing_destination() {
    let fixture = Fixture::new();
    let input = fixture.0.join("cancel source.mkv");
    let destination = fixture.0.join("canceled borders.mkv");
    synthesize(&input, 8, false, false, "1280x720", 120).await;
    let original = std::fs::read(&input).unwrap();
    let mut request = request(&input, &destination, 9);
    request.settings.encoder = VideoEncoder::SvtAv1Hdr;
    request.settings.hdr_tune = media_runtime::HdrTune::FilmGrain;
    request.settings.preset = 2;
    request.settings.framing = serde_json::from_value(json!({
        "crop":{"left":16,"right":16,"top":8,"bottom":8},"resizeWidth":960,
        "borders":{"left":32,"right":48,"top":32,"bottom":48}
    }))
    .unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let job = manager.start_encode(request.clone()).await.unwrap();
    let running = wait_for(&manager, &job.id, |job| job.state == JobState::Running).await;
    assert_eq!(running.state, JobState::Running, "{running:#?}");
    manager.cancel_job(job.id.clone()).await.unwrap();
    let terminal = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
    assert_eq!(terminal.state, JobState::Canceled, "{terminal:#?}");
    assert!(!destination.exists());
    let existing = fixture.0.join("existing bordered output.mkv");
    std::fs::write(&existing, b"existing output must not be overwritten").unwrap();
    request.source.output_path = existing.to_string_lossy().into_owned();
    match manager.start_encode(request).await {
        Ok(job) => {
            let failed = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
            assert_eq!(failed.state, JobState::Failed, "{failed:#?}");
            assert_eq!(failed.error.as_ref().unwrap().code, "OUTPUT_EXISTS");
        }
        Err(error) => assert_eq!(error.code, "OUTPUT_EXISTS"),
    }
    manager.shutdown().await;
    assert_eq!(
        std::fs::read(&existing).unwrap(),
        b"existing output must not be overwritten"
    );
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_no_partial_output(&fixture.0);
}
