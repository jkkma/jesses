//! Opt-in native gate: `cargo test -p media-runtime --test tone_map_jobs -- --include-ignored`.
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
            "jesses-tone-map-{}-{nonce}-{serial}",
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

async fn hdr_source(fixture: &Fixture, transfer: &str, frames: u32) -> PathBuf {
    let base = fixture.0.join("base.mkv");
    synthesize(
        &base,
        10,
        false,
        true,
        "320x180",
        frames,
        ("bt709", "topleft"),
    )
    .await;
    let source = fixture.0.join("HDR source.mkv");
    let params = if transfer == "smpte2084" {
        "log-level=error:pools=2:frame-threads=2:bframes=0:lossless=1:hdr10=1:chromaloc=2:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400"
    } else {
        "log-level=error:pools=2:frame-threads=2:bframes=0:lossless=1:chromaloc=2:transfer=18:colorprim=9:colormatrix=9"
    };
    output(
        command("ffmpeg")
            .args(["-v", "error", "-n", "-i"])
            .arg(&base)
            .args([
                "-map",
                "0",
                "-c",
                "copy",
                "-c:v:1",
                "libx265",
                "-preset:v:1",
                "ultrafast",
                "-x265-params:v:1",
                params,
                "-filter:v:1",
                &format!("setparams=color_trc={transfer}"),
                "-color_trc:v:1",
                transfer,
                "-color_primaries:v:1",
                "bt2020",
                "-colorspace:v:1",
                "bt2020nc",
                "-color_range:v:1",
                "tv",
            ])
            .arg(&source),
    )
    .await;
    source
}

fn mapped_request(input: &Path, destination: &Path, encoder: VideoEncoder) -> EncodeRequest {
    let mut request = request(input, destination, encoder, 4);
    request.settings.tone_map = Some(
        serde_json::from_value(json!({"sourcePeakNits":1000,"hdr10BaseLayer":false})).unwrap(),
    );
    request.settings.trim =
        Some(serde_json::from_value(json!({"startFrame":12,"endFrameExclusive":36})).unwrap());
    request.settings.audio = serde_json::from_value(
        json!([{"streamIndex":2,"codec":"flac","bitrateKbps":128,"channels":"preserve"}]),
    )
    .unwrap();
    request.settings.framing = serde_json::from_value(
        json!({"resizeWidth":160,"borders":{"left":16,"right":16,"top":16,"bottom":16}}),
    )
    .unwrap();
    request
}

#[tokio::test]
#[ignore = "requires FFmpeg zscale/tonemap, x264, x265 and VP9"]
async fn pq_and_hlg_map_to_sdr_before_framing_without_stale_hdr_metadata() {
    for transfer in ["smpte2084", "arib-std-b67"] {
        let fixture = Fixture::new();
        let input = hdr_source(&fixture, transfer, 48).await;
        let original = std::fs::read(&input).unwrap();
        for encoder in [VideoEncoder::X264, VideoEncoder::X265, VideoEncoder::Vp9] {
            let destination = fixture.0.join(format!("{encoder:?}.mkv"));
            let request = mapped_request(&input, &destination, encoder);
            let manager = JobManager::new(fixture.0.join(format!("logs-{encoder:?}")));
            let job = manager.start_encode(request.clone()).await.unwrap();
            let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
            manager.shutdown().await;
            assert_eq!(
                result.state,
                JobState::Succeeded,
                "{transfer} {encoder:?}: {result:#?}"
            );
            assert_eq!(result.encode_settings.as_ref(), Some(&request.settings));
            let document = probe(
                &destination,
                &[
                    "-show_streams",
                    "-show_chapters",
                    "-count_frames",
                    "-show_data_hash",
                    "sha256",
                ],
            )
            .await;
            let video = &document["streams"][2];
            assert_eq!(video["nb_read_frames"], "24");
            assert_eq!(video["width"], 192);
            assert_eq!(video["height"], 122);
            assert_eq!(video["pix_fmt"], "yuv420p10le");
            for key in ["color_space", "color_transfer", "color_primaries"] {
                assert_eq!(video[key], "bt709");
            }
            assert_eq!(video["color_range"], "tv");
            assert_eq!(video["chroma_location"], "left");
            let frames = probe(
                &destination,
                &[
                    "-select_streams",
                    "v:0",
                    "-show_frames",
                    "-show_entries",
                    "frame=side_data_list",
                ],
            )
            .await;
            let text = format!("{video}{frames}");
            for stale in [
                "Mastering display",
                "Content light",
                "DOVI",
                "Dolby Vision",
                "HDR Dynamic",
                "arib-std-b67",
                "smpte2084",
            ] {
                assert!(!text.contains(stale), "Stale HDR metadata {stale}");
            }
            assert_eq!(document["streams"].as_array().unwrap().len(), 4);
            let source = probe(&input, &["-show_streams", "-show_data_hash", "sha256"]).await;
            assert_eq!(
                source["streams"][4]["extradata_hash"],
                document["streams"][3]["extradata_hash"]
            );
            let pixels = output(
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
            let luma: Vec<_> = pixels
                .as_chunks::<2>()
                .0
                .iter()
                .map(|sample| u16::from_le_bytes(*sample))
                .collect();
            for y in 2..12 {
                for x in 2..12 {
                    assert!(luma[y * 192 + x].abs_diff(64) <= 4);
                }
            }
        }
        assert_eq!(std::fs::read(input).unwrap(), original);
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg and VP9"]
async fn tone_mapping_rejects_sdr_and_invalid_peak_without_output() {
    let fixture = Fixture::new();
    let input = fixture.0.join("SDR source.mkv");
    synthesize(&input, 8, false, false, "320x180", 48, ("bt709", "left")).await;
    for case in ["SDR", "bad-peak"] {
        let destination = fixture.0.join(format!("{case}.mkv"));
        let mut request = mapped_request(&input, &destination, VideoEncoder::Vp9);
        if case == "bad-peak" {
            request.settings.tone_map.as_mut().unwrap().source_peak_nits = 0;
        }
        let manager = JobManager::new(fixture.0.join(format!("logs-{case}")));
        match manager.start_encode(request).await {
            Ok(job) => {
                let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
                assert_eq!(result.state, JobState::Failed, "{result:#?}");
            }
            Err(error) => assert_eq!(error.code, "ENCODE_INPUT_UNSUPPORTED"),
        }
        manager.shutdown().await;
        assert!(!destination.exists());
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg zscale/tonemap, libx265 HDR source and lossless x264"]
async fn neutral_pq_and_hlg_samples_match_independent_transfer_and_hable_arithmetic() {
    for transfer in ["smpte2084", "arib-std-b67"] {
        let fixture = Fixture::new();
        let raw = fixture.0.join("neutral.yuv");
        let levels = [64u16, 256, 512, 768, 940];
        let mut samples = Vec::new();
        for _ in 0..128 {
            for level in levels {
                for _ in 0..64 {
                    samples.extend(level.to_le_bytes());
                }
            }
        }
        for _ in 0..320 * 128 / 2 {
            samples.extend(512u16.to_le_bytes());
        }
        std::fs::write(&raw, samples).unwrap();
        let input = fixture.0.join("neutral HDR.mkv");
        let x265 = format!(
            "log-level=error:pools=2:frame-threads=2:bframes=0:lossless=1:chromaloc=0:colorprim=9:colormatrix=9:transfer={}:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400",
            if transfer == "smpte2084" { 16 } else { 18 }
        );
        output(command("ffmpeg").args(["-v","error","-n","-stream_loop","23","-f","rawvideo","-pixel_format","yuv420p10le","-video_size","320x128","-framerate","24","-i"]).arg(&raw).args(["-frames:v","24","-c:v","libx265","-preset","ultrafast","-x265-params",&x265,"-color_trc",transfer,"-color_primaries","bt2020","-colorspace","bt2020nc","-color_range","tv","-pix_fmt","yuv420p10le","-vf",&format!("setsar=1,setparams=range=tv:color_primaries=bt2020:color_trc={transfer}:colorspace=bt2020nc")]).arg(&input)).await;
        let destination = fixture.0.join("neutral SDR.mkv");
        let request:EncodeRequest=serde_json::from_value(json!({"source":{"inputPath":input.to_string_lossy(),"outputPath":destination.to_string_lossy(),"streamIndices":[0]},"settings":{"videoStreamIndex":0,"encoder":"x264","crf":0,"preset":0,"toneMap":{"sourcePeakNits":1000,"hdr10BaseLayer":false}}})).unwrap();
        let manager = JobManager::new(fixture.0.join("logs"));
        let job = manager.start_encode(request).await.unwrap();
        let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        manager.shutdown().await;
        assert_eq!(result.state, JobState::Succeeded, "{transfer}: {result:#?}");
        let bytes = output(
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
        let hable = |value: f64| {
            ((value * (0.15 * value + 0.05) + 0.004) / (value * (0.15 * value + 0.50) + 0.06))
                - 0.02 / 0.30
        };
        for (index, level) in levels.into_iter().enumerate() {
            let encoded = (f64::from(level) - 64.0) / 876.0;
            let linear = if transfer == "smpte2084" {
                let power = encoded.powf(32.0 / 2523.0);
                ((power - 3424.0 / 4096.0).max(0.0) / (2413.0 / 128.0 - 2392.0 / 128.0 * power))
                    .powf(16384.0 / 2610.0)
                    * 100.0
            } else {
                let scene = if encoded <= 0.5 {
                    encoded * encoded / 3.0
                } else {
                    (((encoded - 0.55991073) / 0.17883277).exp() + 0.28466892) / 12.0
                };
                scene.powf(1.2) * 10.0
            };
            let expected = 64.0
                + 876.0
                    * (hable(linear) / hable(10.0))
                        .clamp(0.0, 1.0)
                        .powf(1.0 / 2.4);
            let offset = (64 * 320 + index * 64 + 32) * 2;
            let actual = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
            assert!(
                (f64::from(actual) - expected).abs() <= 3.0,
                "{transfer} input {level}: output {actual}, independent expected {expected}"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires separate verified mainline/5fish/HDR SVT builds and FFmpeg"]
async fn standalone_svt_builds_receive_sdr_pixels_and_no_hdr_metadata() {
    let fixture = Fixture::new();
    let input = hdr_source(&fixture, "smpte2084", 48).await;
    for encoder in [
        VideoEncoder::SvtAv1,
        VideoEncoder::SvtAv1FiveFish,
        VideoEncoder::SvtAv1Hdr,
    ] {
        let destination = fixture.0.join(format!("{encoder:?}.mkv"));
        let mut request = mapped_request(&input, &destination, encoder);
        request.settings.preset = 13;
        let manager = JobManager::new(fixture.0.join(format!("logs-{encoder:?}")));
        let job = manager.start_encode(request).await.unwrap();
        let result = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        manager.shutdown().await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "{encoder:?}: {result:#?}"
        );
        let actual = probe(
            &destination,
            &["-select_streams", "v:0", "-show_streams", "-count_frames"],
        )
        .await;
        assert_eq!(actual["streams"][0]["codec_name"], "av1");
        assert_eq!(actual["streams"][0]["nb_read_frames"], "24");
        assert_eq!(actual["streams"][0]["color_transfer"], "bt709");
        assert!(!actual.to_string().contains("Mastering display"));
    }
}
