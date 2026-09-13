//! Actual deinterlacing, rational frame-rate and source field integrity gates.
use media_runtime::{
    JobManager, JobSnapshot, JobState, RemuxRequest,
    supervisor::{CommandSpec, run_capture},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-temporal-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Temporal fixture retained at {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
async fn tool(name: &str, args: Vec<OsString>) -> Vec<u8> {
    let tools = media_runtime::get_capabilities().await;
    let executable = PathBuf::from(
        tools
            .into_iter()
            .find(|tool| tool.id == name)
            .unwrap()
            .path
            .expect("fixture tool required"),
    );
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(
        &CommandSpec {
            executable,
            args,
            cwd: None,
        },
        cancel,
        8 * 1024 * 1024,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
async fn finish(manager: &JobManager, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(40), async {
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
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("temporal job must finish")
}

async fn source(directory: &Path, bottom: bool) -> PathBuf {
    let output = directory.join(if bottom { "bottom.mkv" } else { "top.mkv" });
    let filter = format!(
        "tinterlace=mode={},setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        if bottom {
            "interleave_bottom"
        } else {
            "interleave_top"
        }
    );
    let mut cmd = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=48:d=2",
        "-vf",
        &filter,
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
        "-level",
        "3",
        "-threads:v",
        "2",
    ]);
    cmd.push(output.as_os_str().to_owned());
    tool("ffmpeg", cmd).await;
    output
}
async fn raw(path: &Path) -> Vec<u8> {
    let mut cmd = args(&["-v", "error", "-i"]);
    cmd.push(path.as_os_str().to_owned());
    cmd.extend(args(&[
        "-map", "0:v:0", "-pix_fmt", "yuv420p", "-f", "rawvideo", "pipe:1",
    ]));
    tool("ffmpeg", cmd).await
}
fn request(
    source: &Path,
    output: &Path,
    temporal: media_core::TemporalSettings,
) -> media_runtime::EncodeRequest {
    media_runtime::EncodeRequest {
        source: RemuxRequest {
            input_path: source.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0],
        },
        settings: media_core::EncodeSettings {
            encoder: media_core::VideoEncoder::X264,
            crf: 0,
            preset: 0,
            temporal: Some(temporal),
            ..Default::default()
        },
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn bwdif_top_and_bottom_field_order_preserve_source_fields_and_bob_count() {
    let directory = Fixture::new();
    let manager = JobManager::new(directory.0.join("logs"));
    let stride = 192 * 112 * 3 / 2;
    for bottom in [false, true] {
        let input = source(&directory.0, bottom).await;
        let before = Sha256::digest(std::fs::read(&input).unwrap());
        let original = raw(&input).await;
        assert_eq!(original.len(), 48 * stride);
        for mode in [
            media_core::DeinterlaceMode::Frame,
            media_core::DeinterlaceMode::Bob,
        ] {
            let output = directory.0.join(format!("{bottom}-{mode:?}.mkv"));
            let settings = media_core::TemporalSettings {
                deinterlace: Some(media_core::DeinterlaceSettings {
                    mode,
                    field_order: if bottom {
                        media_core::FieldOrder::BottomFirst
                    } else {
                        media_core::FieldOrder::TopFirst
                    },
                }),
                ..Default::default()
            };
            let job = manager
                .start_encode(request(&input, &output, settings))
                .await
                .unwrap();
            let job = finish(&manager, &job.id).await;
            assert_eq!(
                job.state,
                JobState::Succeeded,
                "{bottom} {mode:?}: {:?} {:?}",
                job.error,
                job.logs
            );
            let decoded = raw(&output).await;
            let factor = if mode == media_core::DeinterlaceMode::Bob {
                2
            } else {
                1
            };
            assert_eq!(decoded.len(), 48 * factor * stride);
            for n in 0..48 {
                for field in 0..factor {
                    let parity = (usize::from(bottom) + field) % 2;
                    for row in (parity..112).step_by(2) {
                        let a = n * stride + row * 192;
                        let b = (n * factor + field) * stride + row * 192;
                        assert_eq!(
                            &original[a..a + 192],
                            &decoded[b..b + 192],
                            "source field changed {bottom} {mode:?} frame{n} row{row}"
                        );
                    }
                }
            }
        }
        let wrong = directory.0.join(format!("wrong-{bottom}.mkv"));
        let settings = media_core::TemporalSettings {
            deinterlace: Some(media_core::DeinterlaceSettings {
                mode: media_core::DeinterlaceMode::Bob,
                field_order: if bottom {
                    media_core::FieldOrder::TopFirst
                } else {
                    media_core::FieldOrder::BottomFirst
                },
            }),
            ..Default::default()
        };
        let job = manager
            .start_encode(request(&input, &wrong, settings))
            .await
            .unwrap();
        let job = finish(&manager, &job.id).await;
        assert_eq!(job.state, JobState::Failed);
        assert!(!wrong.exists());
        assert_eq!(before, Sha256::digest(std::fs::read(input).unwrap()));
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn rational_fps_duplicates_and_drops_frames_without_speeding_audio() {
    let directory = Fixture::new();
    let input = directory.0.join("progressive.mkv");
    let mut cmd = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=2",
        "-f",
        "lavfi",
        "-i",
        "sine=sample_rate=48000:duration=2",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
        "-level",
        "3",
        "-c:a",
        "flac",
        "-threads:v",
        "2",
    ]);
    cmd.push(input.as_os_str().to_owned());
    tool("ffmpeg", cmd).await;
    let original = raw(&input).await;
    let stride = 192 * 112 * 3 / 2;
    let frame_hashes: Vec<_> = original.chunks_exact(stride).map(Sha256::digest).collect();
    let mut audio_cmd = args(&["-v", "error", "-i"]);
    audio_cmd.push(input.as_os_str().to_owned());
    audio_cmd.extend(args(&[
        "-map",
        "0:a:0",
        "-f",
        "s16le",
        "-acodec",
        "pcm_s16le",
        "pipe:1",
    ]));
    let original_audio = tool("ffmpeg", audio_cmd).await;
    let manager = JobManager::new(directory.0.join("logs"));
    for (num, den, count) in [(12, 1, 24), (30, 1, 60), (30000, 1001, 60)] {
        let output = directory.0.join(format!("fps-{num}-{den}.mp4"));
        let mut request = request(
            &input,
            &output,
            media_core::TemporalSettings {
                frame_rate: Some(media_core::FrameRate {
                    numerator: num,
                    denominator: den,
                }),
                ..Default::default()
            },
        );
        request.source.stream_indices.push(1);
        let job = manager.start_encode(request).await.unwrap();
        let job = finish(&manager, &job.id).await;
        assert_eq!(
            job.state,
            JobState::Succeeded,
            "{num}/{den}: {:?} {:?}",
            job.error,
            job.logs
        );
        let decoded = raw(&output).await;
        assert_eq!(decoded.len(), count * stride);
        let mut previous = 0;
        for (n, frame) in decoded.chunks_exact(stride).enumerate() {
            let input_frame = frame_hashes
                .iter()
                .position(|hash| *hash == Sha256::digest(frame))
                .expect("each output frame must be an unchanged source frame");
            assert!(
                input_frame >= previous,
                "frame-rate conversion reordered source images"
            );
            assert!(
                (input_frame as f64 / 24.0 - n as f64 * den as f64 / num as f64).abs()
                    <= 1.0 / 24.0 + 0.000001,
                "frame moved by more than one source interval"
            );
            previous = input_frame;
        }
        let mut cmd = args(&["-v", "error", "-i"]);
        cmd.push(output.as_os_str().to_owned());
        cmd.extend(args(&[
            "-map",
            "0:a:0",
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "pipe:1",
        ]));
        let audio = tool("ffmpeg", cmd).await;
        assert_eq!(audio.len(), 96000 * 2);
        assert_eq!(
            audio, original_audio,
            "audio samples changed during frame-rate conversion"
        );
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn all_resize_kernels_run_and_nearest_preserves_mathematical_sample_positions() {
    let directory = Fixture::new();
    let input = directory.0.join("impulses.mkv");
    let mut cmd = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "nullsrc=s=256x192:r=24:d=0.5,geq=lum='if(eq(mod(X,7),0),235,16)':cb=128:cr=128",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
        "-level",
        "3",
        "-threads:v",
        "2",
    ]);
    cmd.push(input.as_os_str().to_owned());
    tool("ffmpeg", cmd).await;
    let manager = JobManager::new(directory.0.join("logs"));
    let mut hashes = std::collections::HashSet::new();
    for kernel in [
        media_core::ResizeFilter::Nearest,
        media_core::ResizeFilter::Bilinear,
        media_core::ResizeFilter::Bicubic,
        media_core::ResizeFilter::Lanczos,
    ] {
        let output = directory.0.join(format!("{kernel:?}.mkv"));
        let mut req = request(
            &input,
            &output,
            media_core::TemporalSettings {
                resize_filter: kernel,
                ..Default::default()
            },
        );
        req.settings.framing.resize_width = Some(128);
        let job = manager.start_encode(req).await.unwrap();
        let job = finish(&manager, &job.id).await;
        assert_eq!(
            job.state,
            JobState::Succeeded,
            "{kernel:?}: {:?} {:?}",
            job.error,
            job.logs
        );
        let bytes = raw(&output).await;
        assert_eq!(bytes.len(), 12 * 128 * 96 * 3 / 2);
        hashes.insert(Sha256::digest(&bytes));
        if kernel == media_core::ResizeFilter::Nearest {
            for y in 0..96 {
                for x in 0..128 {
                    assert_eq!(
                        bytes[y * 128 + x],
                        if (2 * x + 1) % 7 == 0 { 235 } else { 16 },
                        "nearest source sample x{x} y{y}"
                    );
                }
            }
        }
    }
    assert_eq!(
        hashes.len(),
        4,
        "resize kernels must produce distinct impulse responses"
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn source_frame_trim_precedes_bob_and_keeps_the_selected_fields() {
    let directory = Fixture::new();
    let input = source(&directory.0, false).await;
    let original = raw(&input).await;
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let output = directory.0.join("trimmed-bob.mkv");
    let mut req = request(
        &input,
        &output,
        media_core::TemporalSettings {
            deinterlace: Some(media_core::DeinterlaceSettings {
                mode: media_core::DeinterlaceMode::Bob,
                field_order: media_core::FieldOrder::TopFirst,
            }),
            ..Default::default()
        },
    );
    req.settings.trim = Some(media_core::VideoTrim {
        start_frame: 12,
        end_frame_exclusive: 36,
    });
    let manager = JobManager::new(directory.0.join("logs"));
    let job = manager.start_encode(req).await.unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?} {:?}",
        job.error,
        job.logs
    );
    let decoded = raw(&output).await;
    let stride = 192 * 112 * 3 / 2;
    assert_eq!(decoded.len(), 48 * stride);
    for n in 0..24 {
        for field in 0..2 {
            for row in (field..112).step_by(2) {
                let a = (n + 12) * stride + row * 192;
                let b = (n * 2 + field) * stride + row * 192;
                assert_eq!(
                    &original[a..a + 192],
                    &decoded[b..b + 192],
                    "trim selected the wrong source field"
                );
            }
        }
    }
    assert_eq!(before, Sha256::digest(std::fs::read(&input).unwrap()));
}
