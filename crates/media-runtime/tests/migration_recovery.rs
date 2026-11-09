//! Real native stop/resume with stable canonical source ownership.
use media_core::{
    EncodeRequest, EncodeSettings, JobState, RemuxRequest, StandaloneRecoveryPhase, VideoEncoder,
    VideoRateControl,
};
use media_runtime::{
    jobs::JobManager,
    supervisor::{self, CommandSpec},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
/// The supervisor runs exactly the path it is given: on Windows it neither
/// searches PATH nor adds ".exe". Resolve tools the way the app does.
async fn tool_path(id: &str) -> PathBuf {
    static TOOLS: tokio::sync::OnceCell<Vec<media_runtime::ToolInfo>> =
        tokio::sync::OnceCell::const_new();
    let tool = TOOLS
        .get_or_init(|| Box::pin(media_runtime::get_capabilities()))
        .await
        .iter()
        .find(|tool| tool.id == id)
        .unwrap();
    assert!(tool.available, "Required tool unavailable: {tool:?}");
    PathBuf::from(tool.path.as_ref().unwrap())
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        // Parallel tests can read the same clock value on Windows.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "jesses-normalized-recovery-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
async fn generate(path: &Path) {
    let (_owner, cancel) = watch::channel(false);
    let mut args:Vec<std::ffi::OsString>=["-v","error","-nostdin","-n","-f","lavfi","-i","testsrc2=size=640x360:rate=24:duration=4","-f","lavfi","-i","sine=duration=4:sample_rate=48000","-map","0:v","-map","1:a","-vf","setpts=PTS+if(gte(N\\,24)\\,0.25/TB\\,0),setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709","-fps_mode","vfr","-c:v","ffv1","-level","3","-c:a","pcm_s16le"].into_iter().map(Into::into).collect();
    args.push(path.as_os_str().to_owned());
    let output = supervisor::run_capture(
        &CommandSpec {
            executable: tool_path("ffmpeg").await,
            args,
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
async fn finish(manager: &JobManager, id: &str) -> media_core::JobSnapshot {
    tokio::time::timeout(Duration::from_secs(90), async {
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
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("job completed")
}
async fn stopped_pass_one(fixture: &Fixture) -> (String, PathBuf, Vec<u8>) {
    let input = fixture.0.join("variable-source.mkv");
    generate(&input).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: fixture.0.join("output.mkv").to_string_lossy().into_owned(),
                stream_indices: vec![0, 1],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::X264,
                preset: 9,
                rate_control: Some(VideoRateControl::Bitrate {
                    bitrate_kbps: 500,
                    two_pass: true,
                }),
                // Only thorough runs keep a resume workspace.
                encoding_preferences: Some(media_core::EncodingPreferences {
                    keyframe_interval_seconds: 10,
                    mp4_faststart: true,
                    full_output_verification: true,
                }),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|entry| entry.id == job.id)
                .unwrap();
            assert!(
                !snapshot.state.is_terminal(),
                "The job finished before the requested stop boundary: {snapshot:#?}"
            );
            if snapshot
                .standalone_recovery
                .as_ref()
                .is_some_and(|recovery| recovery.phase == StandaloneRecoveryPhase::PassOneComplete)
            {
                manager.stop_job(job.id.clone()).await.unwrap();
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("first pass completed");
    let stopped = finish(&manager, &job.id).await;
    assert_eq!(stopped.state, JobState::Stopped, "{stopped:#?}");
    assert!(stopped.standalone_recovery.is_some());
    manager.shutdown().await;
    drop(manager);
    let source = fixture.0.join(format!(".jesses-{}.source", job.id));
    assert!(source.join("source.mkv").is_file());
    (job.id, source, original)
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe and standalone x264"]
async fn normalized_standalone_pass_resume_rechecks_both_sources_and_rejects_foreign_cleanup() {
    let fixture = Fixture::new();
    let (id, source, original) = stopped_pass_one(&fixture).await;
    let canonical = source.join("source.mkv");
    let canonical_bytes = std::fs::read(&canonical).unwrap();
    let foreign = source.join("personal-save.txt");
    std::fs::write(&foreign, b"unrelated work").unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    assert!(manager.discard_av1an_recovery(id.clone()).await.is_err());
    assert_eq!(std::fs::read(&foreign).unwrap(), b"unrelated work");
    assert_eq!(std::fs::read(&canonical).unwrap(), canonical_bytes);
    manager.resume_job(id.clone()).await.unwrap();
    let failed = finish(&manager, &id).await;
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(
        failed.error.as_ref().unwrap().code,
        "SOURCE_NORMALIZATION_INVALID"
    );
    assert!(!fixture.0.join("output.mkv").exists());
    manager.shutdown().await;
    drop(manager);
    std::fs::remove_file(&foreign).unwrap();
    let mut changed = canonical_bytes.clone();
    let at = changed.len() / 2;
    changed[at] ^= 1;
    std::fs::write(&canonical, &changed).unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.resume_job(id.clone()).await.unwrap();
    let failed = finish(&manager, &id).await;
    assert_eq!(failed.state, JobState::Failed);
    assert!(
        failed
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("canonical source changed"),
        "{failed:#?}"
    );
    manager.shutdown().await;
    drop(manager);
    std::fs::write(&canonical, &canonical_bytes).unwrap();
    let input = fixture.0.join("variable-source.mkv");
    let mut changed = original.clone();
    let at = changed.len() / 2;
    changed[at] ^= 1;
    std::fs::write(&input, &changed).unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.resume_job(id.clone()).await.unwrap();
    let failed = finish(&manager, &id).await;
    assert_eq!(failed.state, JobState::Failed);
    assert!(
        failed
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("original source changed"),
        "{failed:#?}"
    );
    manager.shutdown().await;
    drop(manager);
    std::fs::write(&input, &original).unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.resume_job(id.clone()).await.unwrap();
    let completed = finish(&manager, &id).await;
    assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    assert!(
        completed
            .logs
            .iter()
            .any(|line| line.contains("Reusing the verified lossless canonical source"))
    );
    assert!(
        completed
            .logs
            .iter()
            .any(|line| line.contains("Resuming after verified pass one")),
        "{completed:#?}"
    );
    assert_eq!(std::fs::read(input).unwrap(), original);
    manager.shutdown().await;
    drop(manager);
    assert!(!source.exists());
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe and standalone x264"]
async fn normalized_recovery_discard_does_not_require_original_source_and_preserves_neighbors() {
    let fixture = Fixture::new();
    let (id, source, _) = stopped_pass_one(&fixture).await;
    std::fs::remove_file(fixture.0.join("variable-source.mkv")).unwrap();
    let neighbor = fixture.0.join("personal-save.mkv");
    std::fs::write(&neighbor, b"preserved unrelated file").unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let discarded = manager.discard_av1an_recovery(id).await.unwrap();
    assert!(discarded.recovery.is_none());
    assert!(discarded.standalone_recovery.is_none());
    assert!(!source.exists());
    assert_eq!(
        std::fs::read(neighbor).unwrap(),
        b"preserved unrelated file"
    );
    manager.shutdown().await;
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe and standalone x264"]
async fn cancellation_during_canonical_preparation_cleans_only_owned_workspace() {
    let fixture = Fixture::new();
    let input = fixture.0.join("variable-source.mkv");
    generate(&input).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let output = fixture.0.join("canceled.mkv");
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![0, 1],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::X264,
                preset: 9,
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let source = fixture.0.join(format!(".jesses-{}.source", job.id));
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if source.join("owner.json").is_file() {
                break;
            }
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|entry| entry.id == job.id)
                .unwrap();
            assert!(!snapshot.state.is_terminal(), "{snapshot:#?}");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    manager.cancel_job(job.id.clone()).await.unwrap();
    let canceled = finish(&manager, &job.id).await;
    assert_eq!(canceled.state, JobState::Canceled, "{canceled:#?}");
    manager.shutdown().await;
    assert!(!source.exists());
    assert!(!output.exists());
    assert_eq!(std::fs::read(input).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires patched av1an, VS runtime, qualified FFmpeg/FFprobe and x264"]
async fn normalized_av1an_chunks_resume_with_original_and_canonical_identity_checks() {
    normalized_av1an_recovery_case(false).await;
}

#[tokio::test]
#[ignore = "requires patched av1an, VS runtime, qualified FFmpeg/FFprobe and x264"]
async fn normalized_av1an_new_settings_reuses_verified_canonical_source_and_replans() {
    normalized_av1an_recovery_case(true).await;
}

async fn normalized_av1an_recovery_case(new_settings: bool) {
    use media_core::{
        Av1anChunkMethod, Av1anChunkOrder, Av1anOptions, Av1anSplitMethod, EncodeBackend,
        RecoveryPhase,
    };
    let fixture = Fixture::new();
    let input = fixture.0.join("variable-source.mkv");
    generate(&input).await;
    let original = std::fs::read(&input).unwrap();
    let output = fixture.0.join("av1an-output.mkv");
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![0, 1],
            },
            settings: EncodeSettings {
                backend: EncodeBackend::Av1an,
                encoder: VideoEncoder::X264,
                preset: 9,
                crf: 23,
                workers: 1,
                av1an_options: Some(Av1anOptions {
                    chunk_method: Av1anChunkMethod::Select,
                    split_method: Av1anSplitMethod::FixedChunks,
                    maximum_chunk_frames: 12,
                    minimum_scene_frames: 1,
                    chunk_order: Av1anChunkOrder::Sequential,
                    encoder_threads: Some(1),
                    scene_downscale_height: None,
                    ..Default::default()
                }),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|entry| entry.id == job.id)
                .unwrap();
            assert!(
                !snapshot.state.is_terminal(),
                "The AV1AN job finished before a resumable partial boundary: {snapshot:#?}"
            );
            if snapshot.recovery.as_ref().is_some_and(|recovery| {
                recovery.phase == RecoveryPhase::Encoding
                    && recovery.completed_frames > 0
                    && recovery.completed_frames < recovery.total_frames
            }) {
                manager.stop_job(job.id.clone()).await.unwrap();
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("an actual AV1AN chunk completed");
    let stopped = finish(&manager, &job.id).await;
    assert_eq!(stopped.state, JobState::Stopped, "{stopped:#?}");
    assert!(stopped.recovery.as_ref().unwrap().completed_frames > 0);
    manager.shutdown().await;
    drop(manager);
    let source = fixture.0.join(format!(".jesses-{}.source", job.id));
    assert!(source.join("source.mkv").is_file());
    let canonical_before = std::fs::read(source.join("source.mkv")).unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    if new_settings {
        manager.ready().await.unwrap();
        let saved = manager
            .list_jobs()
            .await
            .into_iter()
            .find(|entry| entry.id == job.id)
            .unwrap();
        let mut settings = saved.encode_settings.unwrap();
        settings.crf = 21;
        settings
            .av1an_options
            .as_mut()
            .unwrap()
            .maximum_chunk_frames = 6;
        let resumed = manager
            .av1an_recovery_action(media_core::Av1anRecoveryAction::ResumeNew {
                id: job.id.clone(),
                settings: settings.clone().into(),
            })
            .await
            .unwrap();
        assert_eq!(resumed.jobs[0].encode_settings.as_ref(), Some(&settings));
        assert_eq!(
            std::fs::read(source.join("source.mkv")).unwrap(),
            canonical_before
        );
    } else {
        manager.resume_job(job.id.clone()).await.unwrap();
    }
    let completed = finish(&manager, &job.id).await;
    assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    assert!(
        completed
            .logs
            .iter()
            .any(|line| line.contains("Reusing the verified lossless canonical source"))
    );
    manager.shutdown().await;
    drop(manager);
    assert!(!source.exists());
    assert_eq!(std::fs::read(input).unwrap(), original);
    let media = media_runtime::probe_media(output.to_string_lossy().into_owned())
        .await
        .unwrap();
    assert!(
        (media.duration_seconds.unwrap() - 4.25).abs() < 0.05,
        "{media:#?}"
    );
}
