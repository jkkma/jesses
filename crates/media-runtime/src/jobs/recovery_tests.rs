//! Admission and persistence checks independent of installed media tools.
use super::*;
use media_core::{
    Av1anRecovery, EncodeBackend, RecoveryPhase, StandaloneRecovery, StandaloneRecoveryPhase,
};

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

    fn standalone_recovery(&self) -> StandaloneRecovery {
        StandaloneRecovery {
            workspace: self
                .0
                .join("retained-standalone-work")
                .to_string_lossy()
                .into(),
            phase: StandaloneRecoveryPhase::PassOneComplete,
            completed_frames: 0,
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
async fn live_pause_and_continue_restore_av1an_verification_phase() {
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    let slot = manager.execution.lock().await;
    let job = manager.enqueue_encode(fixture.request()).await.unwrap();
    manager
        .phase(&job.id, JobState::Running, "Encoding chunks.")
        .await;
    manager
        .change(&job.id, |snapshot| snapshot.progress_seconds = Some(30.0))
        .await;
    manager
        .phase_with_fresh_progress(&job.id, JobState::Finalizing, "Verifying AV1AN chunks.")
        .await;
    let verifying = manager.list_jobs().await.remove(0);
    assert_eq!(verifying.state, JobState::Finalizing);
    assert_eq!(verifying.progress_seconds, None);
    let pause = manager.state.lock().await.entries[0].pause.clone();
    supervisor::with_pause(pause, async {
        assert_eq!(
            manager
                .set_job_paused(job.id.clone(), true)
                .await
                .unwrap()
                .state,
            JobState::Paused
        );
        manager
            .phase(
                &job.id,
                JobState::Finalizing,
                "Checking the joined AV1AN video.",
            )
            .await;
        assert_eq!(manager.list_jobs().await[0].state, JobState::Paused);
        let continued = manager.set_job_paused(job.id.clone(), false).await.unwrap();
        assert_eq!(continued.state, JobState::Finalizing);
        assert_eq!(continued.progress_seconds, None);
    })
    .await;
    manager.cancel_job(job.id.clone()).await.unwrap();
    drop(slot);
    let task = manager.state.lock().await.entries[0].task.take().unwrap();
    task.await.unwrap();
    assert_eq!(
        manager
            .set_job_paused(job.id, false)
            .await
            .unwrap_err()
            .code,
        "JOB_NOT_PAUSABLE"
    );
    assert_eq!(manager.list_jobs().await[0].state, JobState::Canceled);
    manager.shutdown().await;
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
async fn standalone_checkpoint_is_durable_and_stop_is_admitted() {
    let fixture = Fixture::new();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let slot = manager.execution.lock().await;
    let mut request = fixture.request();
    request.settings.backend = EncodeBackend::Standalone;
    let job = manager.enqueue_encode(request).await.unwrap();
    let recovery = fixture.standalone_recovery();
    manager
        .change(&job.id, |snapshot| {
            snapshot.standalone_recovery = Some(recovery.clone())
        })
        .await;
    assert_eq!(
        manager.stop_job(job.id.clone()).await.unwrap().state,
        JobState::Stopping
    );
    drop(slot);
    manager.shutdown().await;
    drop(manager);

    let reopened = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    reopened.ready().await.unwrap();
    let before = reopened.list_jobs().await.remove(0);
    assert_eq!(before.state, JobState::Stopped);
    assert_eq!(before.standalone_recovery, Some(recovery));
    assert!(reopened.resume_job(before.id.clone()).await.is_err());
    assert_eq!(reopened.list_jobs().await[0], before);
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
            let standalone = index % 2 == 1;
            let mut settings = request.settings.clone();
            settings.backend = if standalone {
                EncodeBackend::Standalone
            } else {
                EncodeBackend::Av1an
            };
            state.entries.push(Entry {
                command_review: None,
                pause: Default::default(),
                phase_before_pause: None,
                snapshot: JobSnapshot {
                    id: format!("saved-{index}"),
                    state: JobState::Stopped,
                    request: request.source.clone(),
                    mux_request: None,
                    encode_settings: Some(settings),
                    edited_command: None,
                    recovery: (!standalone).then(|| fixture.recovery()),
                    standalone_recovery: standalone.then(|| fixture.standalone_recovery()),
                    input_size_bytes: None,
                    output_size_bytes: None,
                    encoder_progress: None,
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
async fn stop_is_not_available_for_remux() {
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
