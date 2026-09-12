//! Admission and persistence checks independent of installed media tools.
use super::*;
use media_core::{Av1anRecovery, EncodeBackend, RecoveryPhase};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-recovery-state-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn request(&self) -> EncodeRequest {
        EncodeRequest {
            source: RemuxRequest {
                input_path: self.0.join("source.mkv").to_string_lossy().into(),
                output_path: self.0.join("output.mkv").to_string_lossy().into(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                backend: EncodeBackend::Av1an,
                ..Default::default()
            },
        }
    }

    fn recovery(&self) -> Av1anRecovery {
        Av1anRecovery {
            workspace: self.0.join("retained-work").to_string_lossy().into(),
            phase: RecoveryPhase::Encoding,
            completed_frames: 120,
            total_frames: 480,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Only this test creates and owns this nonce-qualified temporary directory.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn stop_waits_for_workers_and_does_not_publish_or_claim_unsaved_progress() {
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    let slot = manager.execution.lock().await;
    let job = manager.enqueue_encode(fixture.request()).await.unwrap();
    assert_eq!(
        manager.resume_job(job.id.clone()).await.unwrap_err().code,
        "JOB_RESUME_UNAVAILABLE"
    );
    assert_eq!(
        manager.stop_job(job.id.clone()).await.unwrap().state,
        JobState::Stopping
    );
    // A late phase notification cannot erase the stop request.
    manager
        .phase(&job.id, JobState::Running, "Late phase event")
        .await;
    assert_eq!(manager.list_jobs().await[0].state, JobState::Stopping);
    drop(slot);
    manager.shutdown().await;
    let stopped = manager.list_jobs().await.remove(0);
    assert_eq!(stopped.state, JobState::Stopped);
    assert!(stopped.recovery.is_none());
    assert!(stopped.error.is_none());
    assert_eq!(
        manager.resume_job(job.id).await.unwrap_err().code,
        "APP_CLOSING"
    );
    assert!(!Path::new(&stopped.request.output_path).exists());
}

#[tokio::test]
async fn recovery_checkpoint_is_durable_without_a_phase_transition() {
    let fixture = Fixture::new();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let slot = manager.execution.lock().await;
    let job = manager.enqueue_encode(fixture.request()).await.unwrap();
    let recovery = fixture.recovery();
    manager
        .change(&job.id, |snapshot| {
            snapshot.recovery = Some(recovery.clone())
        })
        .await;
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.0.join("history/jobs.json")).unwrap())
            .unwrap();
    assert_eq!(saved["jobs"][0]["state"], "queued");
    assert_eq!(saved["jobs"][0]["recovery"]["completedFrames"], 120);
    manager.stop_job(job.id).await.unwrap();
    drop(slot);
    manager.shutdown().await;
    drop(manager);
    let reopened = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    reopened.ready().await.unwrap();
    let before = reopened.list_jobs().await.remove(0);
    assert_eq!(before.state, JobState::Stopped);
    assert_eq!(before.recovery, Some(recovery));
    // An unverified history locator must not launch tools or mutate job state.
    assert!(reopened.resume_job(before.id.clone()).await.is_err());
    assert_eq!(reopened.list_jobs().await[0], before);
    assert!(!fixture.0.join("retained-work").exists());
    reopened.shutdown().await;
}

#[tokio::test]
async fn history_capacity_never_prunes_recoverable_work() {
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    let request = fixture.request();
    {
        let mut state = manager.state.lock().await;
        for index in 0..MAX_HISTORY {
            let (cancel, _) = watch::channel(true);
            state.entries.push(Entry {
                snapshot: JobSnapshot {
                    id: format!("saved-{index}"),
                    state: JobState::Stopped,
                    request: request.source.clone(),
                    encode_settings: Some(request.settings.clone()),
                    recovery: Some(fixture.recovery()),
                    progress_seconds: None,
                    duration_seconds: None,
                    logs: vec![],
                    error: None,
                    log_path: None,
                },
                cancel,
                task: None,
            });
        }
    }
    let before = manager.list_jobs().await;
    assert_eq!(
        manager.enqueue_encode(request).await.unwrap_err().code,
        "QUEUE_FULL"
    );
    assert_eq!(manager.list_jobs().await, before);
    manager.shutdown().await;
}

#[tokio::test]
async fn stop_is_not_available_for_remux_or_standalone_encoding() {
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    let slot = manager.execution.lock().await;
    let job = manager.start_remux(fixture.request().source).await.unwrap();
    let before = manager.list_jobs().await;
    assert_eq!(
        manager.stop_job(job.id.clone()).await.unwrap_err().code,
        "JOB_STOP_UNAVAILABLE"
    );
    assert_eq!(manager.list_jobs().await, before);
    manager.cancel_job(job.id).await.unwrap();
    drop(slot);
    manager.shutdown().await;
}

#[tokio::test]
async fn delayed_stop_does_not_override_cancel_or_shutdown() {
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    let slot = manager.execution.lock().await;
    let job = manager.enqueue_encode(fixture.request()).await.unwrap();
    manager.cancel_all_jobs().await.unwrap();
    assert_eq!(
        manager.stop_job(job.id.clone()).await.unwrap().state,
        JobState::Canceling
    );
    drop(slot);
    manager.shutdown().await;
    assert_eq!(manager.list_jobs().await[0].state, JobState::Canceled);
    assert_eq!(
        manager.stop_job(job.id).await.unwrap_err().code,
        "APP_CLOSING"
    );
}
