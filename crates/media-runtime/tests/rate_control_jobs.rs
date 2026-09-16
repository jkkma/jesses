//! Native rate-control gates use fresh synthetic sources and owned destinations.
use media_core::{StandaloneRecoveryPhase, VideoRateControl};
use media_runtime::{
    EncodeRequest, EncodeSettings, JobManager, JobState, RemuxRequest, VideoEncoder,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-rate-{}-{}",
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

async fn tool(name: &str, args: Vec<String>) -> Vec<u8> {
    static TOOLS: tokio::sync::OnceCell<Vec<media_runtime::ToolInfo>> =
        tokio::sync::OnceCell::const_new();
    let tools = TOOLS
        .get_or_init(|| Box::pin(media_runtime::get_capabilities()))
        .await;
    let found = tools.iter().find(|t| t.id == name).unwrap();
    assert!(found.available, "{found:?}");
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = run_capture(
        &CommandSpec {
            executable: found.path.as_ref().unwrap().into(),
            args: args.into_iter().map(Into::into).collect(),
            cwd: None,
        },
        cancel,
        16 * 1024 * 1024,
        Duration::from_secs(90),
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

async fn source(fixture: &Fixture) -> PathBuf {
    source_seconds(fixture, "12").await
}

async fn source_seconds(fixture: &Fixture, seconds: &str) -> PathBuf {
    let input = fixture.0.join("source's [video]; 漢.mkv");
    let mut args: Vec<String> = ["-v", "error", "-nostdin", "-n", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=24,format=yuv420p10le,setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709", "-f", "lavfi", "-i", "sine=frequency=997:sample_rate=48000", "-t", seconds, "-map", "0:v", "-map", "1:a", "-c:v", "ffv1", "-level", "3", "-chroma_sample_location", "left", "-c:a", "pcm_s16le"].into_iter().map(Into::into).collect();
    args.push(input.to_string_lossy().into_owned());
    tool("ffmpeg", args).await;
    input
}

async fn wait(manager: &JobManager, id: &str) -> media_runtime::JobSnapshot {
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|s| s.id == id)
                .unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap()
}

async fn job(
    fixture: &Fixture,
    input: &Path,
    encoder: VideoEncoder,
    rate: VideoRateControl,
    name: &str,
) -> (PathBuf, media_runtime::JobSnapshot) {
    let destination = fixture.0.join(format!("{name}.mkv"));
    let manager = JobManager::new(fixture.0.join(format!("{name}-logs")));
    let snapshot = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: destination.to_string_lossy().into_owned(),
                stream_indices: vec![0, 1],
            },
            settings: EncodeSettings {
                encoder,
                preset: if encoder.is_svt() { 12 } else { 5 },
                rate_control: Some(rate),
                ..EncodeSettings::default()
            },
        })
        .await
        .unwrap();
    let snapshot = wait(&manager, &snapshot.id).await;
    assert_eq!(
        snapshot.state,
        JobState::Succeeded,
        "{encoder:?}: {:?}\n{:?}",
        snapshot.error,
        snapshot.logs
    );
    (destination, snapshot)
}

async fn inspect(path: &Path) -> Value {
    serde_json::from_slice(
        &tool(
            "ffprobe",
            vec![
                "-v".into(),
                "error".into(),
                "-count_frames".into(),
                "-show_streams".into(),
                "-show_format".into(),
                "-of".into(),
                "json".into(),
                path.to_string_lossy().into_owned(),
            ],
        )
        .await,
    )
    .unwrap()
}

fn verify_recovery_stamp(stamp: &Value) -> PathBuf {
    let path = PathBuf::from(stamp["path"].as_str().expect("recovery stamp path"));
    let data = std::fs::read(&path).unwrap();
    assert_eq!(
        stamp["length"].as_u64(),
        Some(data.len() as u64),
        "recovery stamp length: {}",
        path.display()
    );
    assert_eq!(
        stamp["sha256"].as_str(),
        Some(format!("{:x}", Sha256::digest(&data)).as_str()),
        "recovery stamp hash: {}",
        path.display()
    );
    assert!(
        stamp["identity"]
            .as_array()
            .is_some_and(|identity| !identity.is_empty()),
        "recovery stamp identity: {}",
        path.display()
    );
    path
}

#[tokio::test]
#[ignore = "requires installed native FFmpeg, x264 and all three SVT builds"]
async fn native_bitrate_and_fresh_two_passes_preserve_frames_and_copied_audio() {
    let fixture = Fixture::new();
    let input = source(&fixture).await;
    let original = std::fs::read(&input).unwrap();
    let mut evidence = Vec::new();
    for encoder in [
        VideoEncoder::X264,
        VideoEncoder::X265,
        VideoEncoder::Vp9,
        VideoEncoder::SvtAv1,
        VideoEncoder::SvtAv1FiveFish,
        VideoEncoder::SvtAv1Hdr,
    ] {
        for two_pass in [false, true] {
            let (path, snapshot) = job(
                &fixture,
                &input,
                encoder,
                VideoRateControl::Bitrate {
                    bitrate_kbps: 300,
                    two_pass,
                },
                &format!("{encoder:?}-{two_pass}"),
            )
            .await;
            let info = inspect(&path).await;
            assert_eq!(info["streams"][0]["nb_read_frames"], "288");
            assert_eq!(info["streams"][1]["codec_name"], "pcm_s16le");
            let raw = tool(
                "ffmpeg",
                vec![
                    "-v".into(),
                    "error".into(),
                    "-i".into(),
                    path.to_string_lossy().into_owned(),
                    "-map".into(),
                    "0:a".into(),
                    "-f".into(),
                    "s16le".into(),
                    "-".into(),
                ],
            )
            .await;
            let source_raw = tool(
                "ffmpeg",
                vec![
                    "-v".into(),
                    "error".into(),
                    "-i".into(),
                    input.to_string_lossy().into_owned(),
                    "-map".into(),
                    "0:a".into(),
                    "-f".into(),
                    "s16le".into(),
                    "-".into(),
                ],
            )
            .await;
            assert_eq!(raw, source_raw);
            let log = snapshot.log_path.as_ref().map(PathBuf::from).unwrap();
            if two_pass {
                assert!(log.with_extension("pass1.log").exists());
                assert!(
                    std::fs::metadata(log.with_extension("pass1.log"))
                        .unwrap()
                        .len()
                        > 0
                );
                let video_kbps =
                    (std::fs::metadata(&path).unwrap().len() - raw.len() as u64) as f64 * 8.0
                        / 12.0
                        / 1000.0;
                assert!(
                    (270.0..=330.0).contains(&video_kbps),
                    "two-pass 300 kb/s output including small mux overhead: {video_kbps}"
                );
            }
            evidence.push(json!({"encoder":encoder,"twoPass":two_pass,"bytes":std::fs::metadata(path).unwrap().len(),"frames":288,"audioSamples":raw.len()/2}));
            assert!(
                !std::fs::read_dir(&fixture.0)
                    .unwrap()
                    .flatten()
                    .any(|entry| entry.file_name().to_string_lossy().contains(".jesses-")),
                "owned pass assets must be cleaned"
            );
        }
    }
    assert_eq!(std::fs::read(input).unwrap(), original);
    eprintln!("{}", serde_json::to_string_pretty(&evidence).unwrap());
}

#[tokio::test]
#[ignore = "requires installed native FFmpeg, x264 and all three SVT builds"]
async fn target_size_measures_flac_and_trimmed_duration_and_rejects_impossible_copy_budget() {
    let fixture = Fixture::new();
    let input = source(&fixture).await;
    let original = std::fs::read(&input).unwrap();
    for encoder in [
        VideoEncoder::X264,
        VideoEncoder::X265,
        VideoEncoder::Vp9,
        VideoEncoder::SvtAv1,
        VideoEncoder::SvtAv1FiveFish,
        VideoEncoder::SvtAv1Hdr,
    ] {
        let manager = JobManager::new(fixture.0.join(format!("target-{encoder:?}-logs")));
        let path = fixture.0.join(format!("target-{encoder:?}.mkv"));
        let request = EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: path.to_string_lossy().into_owned(),
                stream_indices: vec![0, 1],
            },
            settings: EncodeSettings {
                encoder,
                preset: if encoder.is_svt() { 12 } else { 5 },
                rate_control: Some(VideoRateControl::TargetSize { target_size_mib: 1 }),
                trim: Some(media_core::VideoTrim {
                    start_frame: 24,
                    end_frame_exclusive: 264,
                    time: None,
                }),
                audio: vec![media_core::AudioTrackSettings {
                    stream_index: 1,
                    codec: media_core::AudioCodec::Flac,
                    bitrate_kbps: 128,
                    channels: media_core::AudioChannels::Preserve,
                    gain: None,
                }],
                ..EncodeSettings::default()
            },
        };
        let job = manager.start_encode(request).await.unwrap();
        let result = wait(&manager, &job.id).await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "{encoder:?}: {:?}\n{:?}",
            result.error,
            result.logs
        );
        let info = inspect(&path).await;
        assert_eq!(info["streams"][0]["nb_read_frames"], "240");
        assert_eq!(info["streams"][1]["codec_name"], "flac");
        let raw = tool(
            "ffmpeg",
            vec![
                "-v".into(),
                "error".into(),
                "-i".into(),
                path.to_string_lossy().into_owned(),
                "-map".into(),
                "0:a".into(),
                "-f".into(),
                "s16le".into(),
                "-".into(),
            ],
        )
        .await;
        let expected = tool(
            "ffmpeg",
            vec![
                "-v".into(),
                "error".into(),
                "-i".into(),
                input.to_string_lossy().into_owned(),
                "-map".into(),
                "0:a".into(),
                "-af".into(),
                "atrim=start_sample=48000:end_sample=528000".into(),
                "-f".into(),
                "s16le".into(),
                "-".into(),
            ],
        )
        .await;
        assert_eq!(raw.len(), 480000 * 2);
        assert_eq!(raw, expected);
        let bytes = std::fs::metadata(&path).unwrap().len();
        assert!(
            (850_000..=1_150_000).contains(&bytes),
            "{encoder:?}: target 1 MiB produced {bytes}"
        );
        assert!(
            result
                .logs
                .iter()
                .any(|line| line.contains("Target-size result") && line.contains("1048576"))
        );
        assert!(
            Path::new(result.log_path.as_ref().unwrap())
                .with_extension("size.log")
                .exists()
        );
        eprintln!(
            "Target 1 MiB: {encoder:?}, {bytes} bytes, 240 frames, 480000 exact FLAC samples"
        );
    }
    let manager = JobManager::new(fixture.0.join("impossible-logs"));
    let path = fixture.0.join("impossible.mkv");
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: path.to_string_lossy().into_owned(),
                stream_indices: vec![0, 1],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::X264,
                rate_control: Some(VideoRateControl::TargetSize { target_size_mib: 1 }),
                ..EncodeSettings::default()
            },
        })
        .await
        .unwrap();
    let result = wait(&manager, &job.id).await;
    assert_eq!(result.state, JobState::Failed);
    assert_eq!(result.error.unwrap().code, "ENCODE_SETTINGS_INVALID");
    assert!(!path.exists());
    assert!(
        !std::fs::read_dir(&fixture.0)
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains(".jesses-"))
    );
    assert_eq!(std::fs::read(input).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires installed native FFmpeg libvpx"]
async fn cancel_each_pass_reaps_tools_and_retains_only_verified_phase_recovery() {
    let fixture = Fixture::new();
    let input = source_seconds(&fixture, "60").await;
    let original = std::fs::read(&input).unwrap();
    for pass in [1, 2] {
        let manager = JobManager::new(fixture.0.join(format!("cancel-{pass}-logs")));
        let path = fixture.0.join(format!("cancel-{pass}.mkv"));
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: path.to_string_lossy().into_owned(),
                    stream_indices: vec![0, 1],
                },
                settings: EncodeSettings {
                    encoder: VideoEncoder::Vp9,
                    preset: 0,
                    rate_control: Some(VideoRateControl::Bitrate {
                        bitrate_kbps: 300,
                        two_pass: true,
                    }),
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let snapshot = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|s| s.id == job.id)
                    .unwrap();
                assert!(
                    !snapshot.state.is_terminal(),
                    "job ended before requested pass: {:?}",
                    snapshot.error
                );
                let log = fixture
                    .0
                    .join(format!("cancel-{pass}-logs"))
                    .join(format!("{}.log", job.id));
                let log = if pass == 1 {
                    log.with_extension("pass1.log")
                } else {
                    log
                };
                let text = std::fs::read_to_string(log).unwrap_or_default();
                if snapshot.state == JobState::Running
                    && text.contains("[producer]")
                    && text.contains("[consumer] frame=")
                    && !text.contains("progress=end")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        manager.cancel_job(job.id.clone()).await.unwrap();
        let canceled = wait(&manager, &job.id).await;
        assert_eq!(canceled.state, JobState::Canceled, "{canceled:#?}");
        assert!(!path.exists());
        let mut leftovers = std::fs::read_dir(&fixture.0)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".jesses-"))
            .map(|entry| entry.path().canonicalize().unwrap())
            .collect::<Vec<_>>();
        leftovers.sort();
        let transient_stats = fixture.0.join(format!(".jesses-{}-passes", job.id));
        assert!(
            !transient_stats.exists(),
            "Canceling pass {pass} retained transient statistics: {canceled:#?}"
        );
        if pass == 1 {
            assert!(canceled.standalone_recovery.is_none(), "{canceled:#?}");
            assert!(
                leftovers.is_empty(),
                "Canceling pass one retained unverified work: {leftovers:?}: {canceled:#?}"
            );
            continue;
        }

        // Pass one is a complete, hash-bound recovery phase. Canceling pass two
        // keeps that resumable checkpoint while deleting its transient encoder
        // statistics and every partial pass-two output.
        let recovery = canceled
            .standalone_recovery
            .as_ref()
            .expect("pass two cancellation retains verified pass one");
        assert_eq!(recovery.phase, StandaloneRecoveryPhase::PassOneComplete);
        assert_eq!(recovery.completed_frames, 0);
        assert_eq!(recovery.total_frames, 60 * 24);
        let workspace = fixture
            .0
            .canonicalize()
            .unwrap()
            .join(format!(".jesses-{}.standalone", job.id));
        assert_eq!(Path::new(&recovery.workspace), workspace);
        assert_eq!(leftovers, vec![workspace.clone()]);

        let manifest: Value =
            serde_json::from_slice(&std::fs::read(workspace.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["version"], json!(1));
        assert_eq!(manifest["id"], json!(job.id));
        assert_eq!(manifest["phase"], json!("passOneComplete"));
        assert_eq!(manifest["two_pass"], json!(true));
        assert_eq!(manifest["total_frames"], json!(recovery.total_frames));
        assert_eq!(
            manifest["request"],
            serde_json::to_value(&canceled.request).unwrap()
        );
        assert_eq!(
            manifest["settings"],
            serde_json::to_value(canceled.encode_settings.as_ref().unwrap()).unwrap()
        );
        assert!(manifest["directory"].as_array().is_some());
        assert!(
            manifest["plan"]
                .as_array()
                .is_some_and(|plan| !plan.is_empty())
        );
        assert!(manifest["video"].is_null());
        assert!(manifest["timed_video"].is_null());
        assert!(manifest["final_stage"].is_null());

        let source_stamp = verify_recovery_stamp(&manifest["source"]);
        assert_eq!(source_stamp, input.canonicalize().unwrap());
        for tool_stamp in manifest["tools"].as_array().expect("tool receipts") {
            verify_recovery_stamp(tool_stamp);
        }
        let stats = manifest["stats"].as_array().expect("pass one receipts");
        assert!(!stats.is_empty());
        assert!(manifest["stats_directory"].as_array().is_some());
        let stats_paths = stats.iter().map(verify_recovery_stamp).collect::<Vec<_>>();
        let stats_directory = stats_paths[0].parent().unwrap().to_owned();
        assert_eq!(stats_directory.parent(), Some(workspace.as_path()));
        assert!(
            stats_directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("uncommitted-stats-")
        );
        assert!(stats_paths.iter().all(|path| {
            path.parent() == Some(stats_directory.as_path())
                && path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("jesses.stats")
        }));
        let recorded_stats = stats_paths
            .iter()
            .map(|path| path.file_name().unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        let actual_stats = std::fs::read_dir(&stats_directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual_stats, recorded_stats);
        let root_entries = std::fs::read_dir(&workspace)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            root_entries,
            BTreeSet::from([
                "manifest.json".into(),
                "workspace.lock".into(),
                stats_directory.file_name().unwrap().to_owned(),
            ])
        );
    }
    assert_eq!(std::fs::read(input).unwrap(), original);
}
