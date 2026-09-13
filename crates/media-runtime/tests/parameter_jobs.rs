//! Installed parameter catalogs, literal command plans and actual override gates.
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
            "jesses-parameters-{}-{}-{}",
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
            eprintln!("Parameter fixture retained at {}", self.0.display());
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

async fn source(directory: &Path) -> PathBuf {
    let output = directory.join("source & $ % ' 空.mkv");
    let mut command = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=2",
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
    command.push(output.as_os_str().to_owned());
    tool("ffmpeg", command).await;
    output
}
fn settings(encoder: media_core::VideoEncoder) -> media_core::EncodeSettings {
    use media_core::VideoEncoder::*;
    let values: &[(&str, &str)] = match encoder {
        X264 => &[
            ("ref", "3"),
            ("bframes", "4"),
            ("b-adapt", "0"),
            ("aq-mode", "2"),
        ],
        X265 => &[
            ("ref", "3"),
            ("bframes", "3"),
            ("b-adapt", "0"),
            ("sao", "0"),
            ("cutree", "0"),
        ],
        Vp9 => &[
            ("aq-mode", "2"),
            ("lag-in-frames", "16"),
            ("auto-alt-ref", "1"),
        ],
        _ => &[
            ("aq-mode", "2"),
            ("enable-tf", "0"),
            ("hierarchical-levels", "3"),
        ],
    };
    media_core::EncodeSettings {
        encoder,
        crf: 30,
        preset: if encoder.is_svt() { 8 } else { 0 },
        parameters: values
            .iter()
            .map(|(name, value)| media_core::EncoderParameter {
                name: (*name).into(),
                value: (*value).into(),
            })
            .collect(),
        ..Default::default()
    }
}
fn request(
    input: &Path,
    output: &Path,
    encoder: media_core::VideoEncoder,
) -> media_runtime::EncodeRequest {
    media_runtime::EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0],
        },
        settings: settings(encoder),
    }
}
#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, x264 and all three selected SVT builds"]
async fn installed_catalogs_and_actual_overrides_keep_their_native_and_library_vocabulary() {
    use media_core::VideoEncoder::*;
    let directory = Fixture::new();
    let input = source(&directory.0).await;
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let manager = JobManager::new(directory.0.join("logs"));
    for encoder in [X264, X265, Vp9, SvtAv1, SvtAv1FiveFish, SvtAv1Hdr] {
        let (_owner, cancel) = tokio::sync::watch::channel(false);
        let catalog = media_runtime::get_encoder_parameters(
            encoder,
            media_core::EncodeBackend::Standalone,
            cancel,
        )
        .await
        .unwrap();
        let output = directory.0.join(format!("{encoder:?}.mkv"));
        let request = request(&input, &output, encoder);
        for parameter in &request.settings.parameters {
            assert!(
                catalog
                    .parameters
                    .iter()
                    .any(|spec| spec.name == parameter.name),
                "missing installed option {encoder:?}: {}",
                parameter.name
            );
        }
        let job = manager.start_encode(request).await.unwrap();
        let job = finish(&manager, &job.id).await;
        assert_eq!(
            job.state,
            JobState::Succeeded,
            "{encoder:?}: {:?} {:?}",
            job.error,
            job.logs
        );
        let mut command = args(&[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=nb_read_frames,has_b_frames",
            "-of",
            "json",
        ]);
        command.push(output.as_os_str().to_owned());
        let probe: serde_json::Value =
            serde_json::from_slice(&tool("ffprobe", command).await).unwrap();
        assert_eq!(probe["streams"][0]["nb_read_frames"], "48");
        if matches!(encoder, X264 | X265) {
            assert!(
                probe["streams"][0]["has_b_frames"].as_u64().unwrap() > 0,
                "explicit B-frames must override ultrafast defaults"
            );
        }
    }
    assert_eq!(before, Sha256::digest(std::fs::read(&input).unwrap()));
}
#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn real_plan_reuses_native_argv_with_literal_paths_and_never_publishes() {
    let directory = Fixture::new();
    let input = source(&directory.0).await;
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let output = directory.0.join("output $ % ' 空.mp4");
    let mut request = request(&input, &output, media_core::VideoEncoder::X264);
    request.settings.trim = Some(media_core::VideoTrim {
        start_frame: 12,
        end_frame_exclusive: 36,
    });
    request.settings.rate_control = Some(media_core::VideoRateControl::Bitrate {
        bitrate_kbps: 250,
        two_pass: true,
    });
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let plan = media_runtime::preview_encode_plan(request.clone(), cancel)
        .await
        .unwrap();
    assert_eq!(plan.request, request);
    assert_eq!(plan.output_frame_count, "24");
    assert_eq!(plan.output_frame_rate, "24/1");
    assert!(!plan.source_fingerprint.is_empty());
    let decoders: Vec<_> = plan
        .stages
        .iter()
        .filter(|stage| stage.label.ends_with("source decoder"))
        .collect();
    assert_eq!(decoders.len(), 2);
    for decoder in decoders {
        assert_eq!(
            decoder
                .arguments
                .iter()
                .filter(|value| *value == &std::fs::canonicalize(&input).unwrap().to_string_lossy())
                .count(),
            1
        );
        assert!(
            decoder
                .arguments
                .iter()
                .any(|value| value.contains("trim=start_frame=12:end_frame=36"))
        );
    }
    for (index, encoder) in plan
        .stages
        .iter()
        .filter(|stage| stage.label.ends_with("video encoder"))
        .enumerate()
    {
        let pair = |name: &str| {
            let n = encoder
                .arguments
                .iter()
                .position(|value| value == name)
                .unwrap();
            encoder.arguments[n + 1].clone()
        };
        assert_eq!(pair("--bframes"), "4");
        assert_eq!(pair("--pass"), (index + 1).to_string());
        assert_eq!(pair("--bitrate"), "250");
        assert!(
            !Path::new(encoder.working_directory.as_ref().unwrap()).exists(),
            "preview stats directory leaked"
        );
    }
    assert!(!output.exists());
    assert_eq!(before, Sha256::digest(std::fs::read(&input).unwrap()));
    assert_eq!(
        std::fs::read_dir(&directory.0).unwrap().count(),
        1,
        "preview left temporary files or directories behind"
    );
    let (owner, cancel) = tokio::sync::watch::channel(false);
    owner.send_replace(true);
    assert!(
        media_runtime::preview_encode_plan(request, cancel)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
}
