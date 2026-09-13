//! Sequential media jobs with immutable requests and optional durable history.
//! Interrupted jobs are reported after restart and never automatically resumed.

mod audio;
mod av1an;
#[cfg(test)]
mod batch_tests;
mod container;
mod encode;
pub use encode::preview_encode_plan;
mod encode_plan;
pub(crate) mod files;
mod history;
mod metadata;
mod mux;
pub(crate) mod parameters;
mod rate_control;
#[cfg(test)]
mod real_tests;
#[cfg(test)]
mod recovery_tests;
mod reports;
mod subtitles;
pub use reports::export_analysis;
mod trim;

use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use media_core::{
    AppError, BatchEncodeInput, BatchEncodePreview, BatchEncodeRequest, EncodeRequest,
    EncodeSettings, JobSnapshot, JobState, RemuxRequest,
};
use tokio::sync::{Mutex, Notify, mpsc, watch};

use crate::{
    discovery::find_executable,
    supervisor::{self, CommandSpec, ProcessEvent, SupervisorError},
};
use files::{Source, Temporary};
use metadata::Document;

const MAX_LOG_LINES: usize = 150;
const MAX_HISTORY: usize = 100;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Entry {
    pause: supervisor::PauseControl,
    snapshot: JobSnapshot,
    cancel: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

struct Preflight {
    cancel: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    preflights: Vec<Preflight>,
    shutting_down: bool,
    storage_error: Option<AppError>,
    submission_epoch: u64,
}

#[derive(Clone)]
pub struct JobManager {
    state: Arc<Mutex<State>>,
    log_dir: Arc<PathBuf>,
    history: Option<history::History>,
    execution: Arc<Mutex<()>>,
    queue_changed: Arc<Notify>,
}

fn canceled() -> AppError {
    AppError::new(
        "JOB_CANCELED",
        "The job was canceled. No output was published.",
        None,
    )
}

fn batch_canceled() -> AppError {
    AppError::new(
        "BATCH_CANCELED",
        "Stop Queue canceled this batch while its sources were being checked. No jobs were added.",
        None,
    )
}

fn check_cancel(cancel: &watch::Receiver<bool>) -> Result<(), AppError> {
    if *cancel.borrow() {
        Err(canceled())
    } else {
        Ok(())
    }
}

fn process_error(error: SupervisorError, path: &Path) -> AppError {
    let code = match &error {
        SupervisorError::Cancelled => "JOB_CANCELED",
        SupervisorError::Timeout => "JOB_TIMEOUT",
        SupervisorError::OutputLimit => "PROBE_OUTPUT_LIMIT",
        _ => "TOOL_FAILED",
    };
    files::error(code, error.to_string(), path)
}

impl JobManager {
    pub fn new(log_dir: PathBuf) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            log_dir: Arc::new(log_dir),
            history: None,
            execution: Arc::new(Mutex::new(())),
            queue_changed: Arc::new(Notify::new()),
        }
    }

    pub async fn open(log_dir: PathBuf, history_dir: PathBuf) -> Self {
        let mut manager = Self::new(log_dir);
        match history::History::open(history_dir).await {
            Ok((history, snapshots)) => {
                manager.history = Some(history);
                let mut state = manager.state.lock().await;
                for mut snapshot in snapshots {
                    if !snapshot.state.is_terminal() {
                        snapshot.state = JobState::Interrupted;
                        snapshot.error = Some(AppError::new(
                            "JOB_INTERRUPTED",
                            "The previous session ended before completion was recorded. Saved av1an work can be resumed after verification; no processes were restarted or old files deleted.",
                            Some(snapshot.request.output_path.clone()),
                        ));
                        append_log(
                            &mut snapshot,
                            "Interrupted job restored for review. Resume is available only for verified saved av1an work and must be requested explicitly."
                                .into(),
                        );
                    }
                    let (cancel, _) = watch::channel(true);
                    state.entries.push(Entry {
                        pause: Default::default(),
                        snapshot,
                        cancel,
                        task: None,
                    });
                }
                let _ = manager.persist(&mut state).await;
            }
            Err(error) => manager.state.lock().await.storage_error = Some(error),
        }
        manager
    }

    pub async fn ready(&self) -> Result<(), AppError> {
        self.state
            .lock()
            .await
            .storage_error
            .clone()
            .map_or(Ok(()), Err)
    }

    async fn persist(&self, state: &mut State) -> Result<(), AppError> {
        if let Some(error) = &state.storage_error {
            return Err(error.clone());
        }
        if let Some(history) = &self.history
            && let Err(error) = history
                .save(state.entries.iter().map(|e| e.snapshot.clone()).collect())
                .await
        {
            for entry in &mut state.entries {
                if !entry.snapshot.state.is_terminal() {
                    entry.cancel.send_replace(true);
                }
                append_log(&mut entry.snapshot, error.message.clone());
                entry.snapshot.error = Some(error.clone());
            }
            state.storage_error = Some(error.clone());
            return Err(error);
        }
        Ok(())
    }

    pub async fn start_remux(&self, request: RemuxRequest) -> Result<JobSnapshot, AppError> {
        self.start_job(request, None, false).await
    }

    pub async fn start_encode(&self, request: EncodeRequest) -> Result<JobSnapshot, AppError> {
        encode::validate_settings(&request.settings)?;
        self.start_job(request.source, Some(request.settings), false)
            .await
    }

    pub async fn enqueue_encode(&self, request: EncodeRequest) -> Result<JobSnapshot, AppError> {
        encode::validate_settings(&request.settings)?;
        self.start_job(request.source, Some(request.settings), true)
            .await
    }

    pub async fn preview_encode_batch(
        &self,
        request: BatchEncodeRequest,
    ) -> Result<BatchEncodePreview, AppError> {
        encode::validate_settings(&EncodeSettings {
            parameters: request.parameters.clone(),
            temporal: None,
            av1an_options: request.av1an_options,
            rate_control: request.rate_control,
            tone_map: None,
            trim: None,
            subtitles: Vec::new(),
            framing: Default::default(),
            audio: Vec::new(),
            video_stream_index: 0,
            crf: request.crf,
            preset: request.preset,
            film_grain: request.film_grain,
            lineart_psy_bias: request.lineart_psy_bias,
            texture_psy_bias: request.texture_psy_bias,
            hdr_tune: request.hdr_tune,
            hdr10_fallback: request.hdr10_fallback,
            backend: request.backend,
            encoder: request.encoder,
            workers: request.workers,
        })?;
        let (queued, epoch) = {
            let state = self.state.lock().await;
            let queued: Vec<_> = state
                .entries
                .iter()
                .filter(|entry| !entry.snapshot.state.is_terminal())
                .map(|entry| entry.snapshot.request.output_path.clone())
                .collect();
            (queued, state.submission_epoch)
        };
        let reserved = queued
            .iter()
            .map(|path| {
                crate::batch::destination_key(Path::new(path))
                    .unwrap_or_else(|_| crate::batch::path_key(Path::new(path)))
            })
            .collect();
        crate::batch::preview(self, request, reserved, epoch).await
    }

    pub(crate) async fn inspect_encode_source(
        &self,
        input: &BatchEncodeInput,
        settings: &EncodeSettings,
        epoch: u64,
    ) -> Result<media_core::MediaFile, AppError> {
        let input = input.clone();
        let settings = settings.clone();
        self.run_preflight(epoch, move |cancel| async move {
            inspect_encode_source(&input, &settings, &cancel).await
        })
        .await
    }

    async fn run_preflight<T, F, Fut>(&self, epoch: u64, action: F) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(watch::Receiver<bool>) -> Fut,
        Fut: Future<Output = Result<T, AppError>> + Send + 'static,
    {
        let mut state = self.state.lock().await;
        if state.shutting_down {
            return Err(AppError::new(
                "APP_CLOSING",
                "The application is closing; source inspection cannot start.",
                None,
            ));
        }
        if epoch != state.submission_epoch {
            return Err(batch_canceled());
        }
        if let Some(error) = &state.storage_error {
            return Err(error.clone());
        }
        state.preflights.retain(|entry| !entry.task.is_finished());
        let (cancel, receiver) = watch::channel(false);
        let (sender, result) = tokio::sync::oneshot::channel();
        let work = action(receiver);
        let task = tokio::spawn(async move {
            let result = work.await.map_err(|error| {
                if error.code == "JOB_CANCELED" {
                    batch_canceled()
                } else {
                    error
                }
            });
            let _ = sender.send(result);
        });
        state.preflights.push(Preflight { cancel, task });
        drop(state);
        result
            .await
            .map_err(|e| AppError::new("PREFLIGHT_FAILED", e.to_string(), None))?
    }

    /// Admit one immutable batch with a single durable write. Expensive probes
    /// happen before the lock; admission rechecks destinations/capacity while
    /// Stop Queue and other submissions are excluded by the same state lock.
    pub async fn enqueue_encode_batch(
        &self,
        requests: Vec<EncodeRequest>,
    ) -> Result<Vec<JobSnapshot>, AppError> {
        let epoch = self.state.lock().await.submission_epoch;
        crate::batch::validate_batch_len(requests.len())?;
        let mut prepared = Vec::with_capacity(requests.len());
        for mut request in requests {
            if epoch != self.state.lock().await.submission_epoch {
                return Err(batch_canceled());
            }
            encode::validate_settings(&request.settings)?;
            files::validate_request(&request.source)?;
            let media = crate::batch::inspect_selection(
                self,
                &BatchEncodeInput {
                    temporal: request.settings.temporal,
                    tone_map: request.settings.tone_map,
                    trim: request.settings.trim,
                    subtitles: request.settings.subtitles.clone(),
                    framing: request.settings.framing,
                    audio: request.settings.audio.clone(),
                    input_path: request.source.input_path.clone(),
                    stream_indices: request.source.stream_indices.clone(),
                    video_stream_index: request.settings.video_stream_index,
                },
                &request.settings,
                epoch,
            )
            .await?;
            request.source.input_path = media.path;
            prepared.push(request);
        }
        self.admit_encode_batch_at_epoch(prepared, epoch).await
    }

    #[cfg(test)]
    async fn admit_encode_batch(
        &self,
        requests: Vec<EncodeRequest>,
    ) -> Result<Vec<JobSnapshot>, AppError> {
        let epoch = self.state.lock().await.submission_epoch;
        self.admit_encode_batch_at_epoch(requests, epoch).await
    }

    async fn admit_encode_batch_at_epoch(
        &self,
        requests: Vec<EncodeRequest>,
        epoch: u64,
    ) -> Result<Vec<JobSnapshot>, AppError> {
        crate::batch::validate_batch_len(requests.len())?;
        let mut state = self.state.lock().await;
        if epoch != state.submission_epoch {
            return Err(batch_canceled());
        }
        if let Some(error) = &state.storage_error {
            return Err(error.clone());
        }
        if state.shutting_down {
            return Err(AppError::new(
                "APP_CLOSING",
                "The application is closing; new jobs cannot start.",
                None,
            ));
        }
        let mut reserved: HashSet<_> = state
            .entries
            .iter()
            .filter(|entry| !entry.snapshot.state.is_terminal())
            .map(|entry| {
                crate::batch::destination_key(Path::new(&entry.snapshot.request.output_path))
                    .unwrap_or_else(|_| {
                        crate::batch::path_key(Path::new(&entry.snapshot.request.output_path))
                    })
            })
            .collect();
        let mut normalized = Vec::with_capacity(requests.len());
        for mut request in requests {
            files::validate_request(&request.source)?;
            encode::validate_settings(&request.settings)?;
            let source = Source::open(Path::new(&request.source.input_path))?;
            let output = files::output_path(&request.source, &source)?;
            crate::batch::writable_directory(output.parent().expect("validated output parent"))?;
            if !reserved.insert(crate::batch::path_key(&output)) {
                return Err(files::error(
                    "OUTPUT_QUEUED",
                    "The batch or existing queue already reserves this output filename.",
                    &output,
                ));
            }
            request.source.input_path = source.path.to_string_lossy().into_owned();
            request.source.output_path = output.to_string_lossy().into_owned();
            normalized.push(request);
        }
        let remove_count = (state.entries.len() + normalized.len()).saturating_sub(MAX_HISTORY);
        let prune: HashSet<_> = state
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.snapshot.state.is_terminal()
                    && entry.snapshot.recovery.is_none()
                    && entry.task.as_ref().is_none_or(|task| task.is_finished())
            })
            .take(remove_count)
            .map(|(index, _)| index)
            .collect();
        if prune.len() != remove_count {
            return Err(AppError::new(
                "QUEUE_FULL",
                "The whole batch does not fit in the queue. Wait for jobs to finish or submit fewer files.",
                None,
            ));
        }
        let mut staged = Vec::with_capacity(normalized.len());
        for request in normalized {
            let nonce = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let id = format!(
                "{}-{nonce}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            );
            let log_path = self.log_dir.join(format!("{id}.log"));
            let snapshot = JobSnapshot {
                id,
                state: JobState::Queued,
                request: request.source,
                mux_request: None,
                encode_settings: Some(request.settings),
                recovery: None,
                progress_seconds: None,
                duration_seconds: None,
                logs: vec!["Media job admitted with an atomic batch.".into()],
                error: None,
                log_path: Some(log_path.to_string_lossy().into_owned()),
            };
            let (cancel, receiver) = watch::channel(false);
            staged.push((
                Entry {
                    pause: Default::default(),
                    snapshot,
                    cancel,
                    task: None,
                },
                receiver,
                log_path,
            ));
        }
        let mut persisted: Vec<_> = state
            .entries
            .iter()
            .enumerate()
            .filter(|(index, _)| !prune.contains(index))
            .map(|(_, entry)| entry.snapshot.clone())
            .collect();
        persisted.extend(staged.iter().map(|(entry, _, _)| entry.snapshot.clone()));
        if let Some(history) = &self.history
            && let Err(error) = history.save(persisted).await
        {
            // No staged entry has entered memory and no task has been spawned.
            // Existing jobs receive the same durability failure behavior as a
            // failed normal state transition.
            for entry in &mut state.entries {
                if !entry.snapshot.state.is_terminal() {
                    entry.cancel.send_replace(true);
                }
                append_log(&mut entry.snapshot, error.message.clone());
                entry.snapshot.error = Some(error.clone());
            }
            state.storage_error = Some(error.clone());
            self.queue_changed.notify_waiters();
            return Err(error);
        }
        let mut index = 0;
        state.entries.retain(|_| {
            let retain = !prune.contains(&index);
            index += 1;
            retain
        });
        let snapshots = staged
            .iter()
            .map(|(entry, _, _)| entry.snapshot.clone())
            .collect();
        for (mut entry, receiver, log_path) in staged {
            let worker = self.clone();
            let id = entry.snapshot.id.clone();
            entry.task = Some(tokio::spawn(async move {
                worker.run_queued(id, receiver, log_path).await;
            }));
            state.entries.push(entry);
        }
        self.queue_changed.notify_waiters();
        Ok(snapshots)
    }

    async fn start_job(
        &self,
        request: RemuxRequest,
        encode_settings: Option<EncodeSettings>,
        allow_queue: bool,
    ) -> Result<JobSnapshot, AppError> {
        self.admit_job(request, encode_settings, None, allow_queue)
            .await
    }

    async fn admit_job(
        &self,
        request: RemuxRequest,
        encode_settings: Option<EncodeSettings>,
        mux_request: Option<media_core::MuxRequest>,
        allow_queue: bool,
    ) -> Result<JobSnapshot, AppError> {
        files::validate_request(&request)?;
        let mut state = self.state.lock().await;
        if let Some(error) = &state.storage_error {
            return Err(error.clone());
        }
        if state.shutting_down {
            return Err(AppError::new(
                "APP_CLOSING",
                "The application is closing; new jobs cannot start.",
                None,
            ));
        }
        if !allow_queue
            && state
                .entries
                .iter()
                .any(|e| !e.snapshot.state.is_terminal())
        {
            return Err(AppError::new(
                "JOB_BUSY",
                "A job is already active. Wait for it or cancel it before starting another.",
                None,
            ));
        }
        let destination = crate::batch::destination_key(Path::new(&request.output_path))
            .unwrap_or_else(|_| crate::batch::path_key(Path::new(&request.output_path)));
        if state.entries.iter().any(|e| {
            !e.snapshot.state.is_terminal()
                && crate::batch::destination_key(Path::new(&e.snapshot.request.output_path))
                    .unwrap_or_else(|_| {
                        crate::batch::path_key(Path::new(&e.snapshot.request.output_path))
                    })
                    == destination
        }) {
            return Err(AppError::new(
                "OUTPUT_QUEUED",
                "This destination is already assigned to an unfinished job.",
                Some(request.output_path),
            ));
        }
        while state.entries.len() >= MAX_HISTORY {
            let index = state
                .entries
                .iter()
                .position(|e| {
                    e.snapshot.state.is_terminal()
                        && e.snapshot.recovery.is_none()
                        && e.task.as_ref().is_none_or(|task| task.is_finished())
                })
                .ok_or_else(|| {
                    AppError::new(
                        "QUEUE_FULL",
                        "The queue is full. Wait for jobs to finish before adding more.",
                        None,
                    )
                })?;
            state.entries.remove(index);
        }
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!(
            "{}-{nonce}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let log_path = self.log_dir.join(format!("{id}.log"));
        let snapshot = JobSnapshot {
            id: id.clone(),
            state: JobState::Queued,
            request,
            mux_request,
            encode_settings,
            recovery: None,
            progress_seconds: None,
            duration_seconds: None,
            logs: vec!["Media job queued.".into()],
            error: None,
            log_path: Some(log_path.to_string_lossy().into_owned()),
        };
        let (cancel, receiver) = watch::channel(false);
        let worker = self.clone();
        let task_id = id.clone();
        state.entries.push(Entry {
            pause: Default::default(),
            snapshot: snapshot.clone(),
            cancel,
            task: None,
        });
        if let Err(error) = self.persist(&mut state).await {
            state.entries.pop();
            return Err(error);
        }
        let task = tokio::spawn(async move {
            worker.run_queued(task_id, receiver, log_path).await;
        });
        state.entries.last_mut().expect("registered job").task = Some(task);
        self.queue_changed.notify_waiters();
        Ok(snapshot)
    }

    async fn run_queued(&self, id: String, cancel: watch::Receiver<bool>, log_path: PathBuf) {
        let turn = async {
            loop {
                let changed = self.queue_changed.notified();
                let first = self
                    .state
                    .lock()
                    .await
                    .entries
                    .iter()
                    .find(|e| !e.snapshot.state.is_terminal())
                    .map(|e| e.snapshot.id.clone());
                if first.as_deref() == Some(&id) {
                    return self.execution.lock().await;
                }
                changed.await;
            }
        };
        let slot = tokio::select! {
            biased;
            _ = wait_cancel(cancel.clone()) => None,
            guard = turn => Some(guard),
        };
        if slot.is_some() {
            self.execute(id, cancel, log_path).await;
        } else {
            let storage_error = self.state.lock().await.storage_error.clone();
            self.change(&id, |snapshot| {
                if let Some(error) = storage_error {
                    snapshot.state = JobState::Failed;
                    append_log(snapshot, error.message.clone());
                    snapshot.error = Some(error);
                } else {
                    snapshot.state = if snapshot.state == JobState::Stopping {
                        JobState::Stopped
                    } else {
                        JobState::Canceled
                    };
                    append_log(
                        snapshot,
                        "Queued job stopped before starting; any saved progress was retained."
                            .into(),
                    );
                }
            })
            .await;
        }
        drop(slot);
        self.queue_changed.notify_waiters();
    }

    pub async fn list_jobs(&self) -> Vec<JobSnapshot> {
        self.state
            .lock()
            .await
            .entries
            .iter()
            .rev()
            .map(|e| e.snapshot.clone())
            .collect()
    }

    /// Live pause is separate from stop-and-keep-progress and durable resume.
    pub async fn set_job_paused(&self, id: String, paused: bool) -> Result<JobSnapshot, AppError> {
        let mut state = self.state.lock().await;
        if let Some(error) = &state.storage_error {
            return Err(error.clone());
        }
        if state.shutting_down {
            return Err(AppError::new(
                "APP_CLOSING",
                "The application is closing; pause requests are no longer accepted.",
                None,
            ));
        }
        let entry = state
            .entries
            .iter_mut()
            .find(|entry| entry.snapshot.id == id)
            .ok_or_else(|| AppError::new("JOB_NOT_FOUND", "The job was not found.", None))?;
        if !matches!(entry.snapshot.state, JobState::Running | JobState::Paused)
            || !entry
                .snapshot
                .encode_settings
                .as_ref()
                .is_some_and(|settings| settings.backend == media_core::EncodeBackend::Av1an)
        {
            return Err(AppError::new(
                "JOB_NOT_PAUSABLE",
                "Live pause is available while av1an encodes chunks.",
                None,
            ));
        }
        let control = entry.pause.clone();
        tokio::task::spawn_blocking(move || control.set_paused(paused))
            .await
            .map_err(|e| AppError::new("PAUSE_FAILED", e.to_string(), None))?
            .map_err(|e| AppError::new("PAUSE_FAILED", e.to_string(), None))?;
        entry.snapshot.state = if paused {
            JobState::Paused
        } else {
            JobState::Running
        };
        append_log(
            &mut entry.snapshot,
            if paused {
                "Paused the live av1an process tree. Memory and open files are retained."
            } else {
                "Continued the live av1an process tree."
            }
            .into(),
        );
        let snapshot = entry.snapshot.clone();
        self.persist(&mut state).await?;
        Ok(snapshot)
    }

    pub async fn cancel_job(&self, id: String) -> Result<JobSnapshot, AppError> {
        let mut state = self.state.lock().await;
        let entry = state
            .entries
            .iter_mut()
            .find(|e| e.snapshot.id == id)
            .ok_or_else(|| {
                AppError::new(
                    "JOB_NOT_FOUND",
                    "The job no longer exists in this session.",
                    None,
                )
            })?;
        if !entry.snapshot.state.is_terminal() {
            entry.cancel.send_replace(true);
            entry.snapshot.state = JobState::Canceling;
            append_log(
                &mut entry.snapshot,
                "Cancellation requested; waiting for owned processes and temporary output cleanup."
                    .into(),
            );
        }
        let snapshot = entry.snapshot.clone();
        self.persist(&mut state).await?;
        self.queue_changed.notify_waiters();
        Ok(snapshot)
    }

    /// Stops the owned worker tree while retaining av1an recovery artifacts.
    pub async fn stop_job(&self, id: String) -> Result<JobSnapshot, AppError> {
        let mut state = self.state.lock().await;
        if let Some(error) = &state.storage_error {
            return Err(error.clone());
        }
        if state.shutting_down {
            return Err(AppError::new(
                "APP_CLOSING",
                "The application is closing; stop requests are no longer accepted.",
                None,
            ));
        }
        let entry = state
            .entries
            .iter_mut()
            .find(|entry| entry.snapshot.id == id)
            .ok_or_else(|| AppError::new("JOB_NOT_FOUND", "The saved job was not found.", None))?;
        if entry
            .snapshot
            .encode_settings
            .as_ref()
            .is_none_or(|settings| settings.backend != media_core::EncodeBackend::Av1an)
        {
            return Err(AppError::new(
                "JOB_STOP_UNAVAILABLE",
                "Only av1an jobs can keep encoded progress for resume.",
                None,
            ));
        }
        if !entry.snapshot.state.is_terminal()
            && !matches!(
                entry.snapshot.state,
                JobState::Stopping | JobState::Canceling
            )
        {
            entry.cancel.send_replace(true);
            entry.snapshot.state = JobState::Stopping;
            append_log(
                &mut entry.snapshot,
                "Stop requested; waiting for owned processes to exit and keeping av1an progress."
                    .into(),
            );
        }
        let snapshot = entry.snapshot.clone();
        self.persist(&mut state).await?;
        self.queue_changed.notify_waiters();
        Ok(snapshot)
    }

    /// Resumes the immutable original job after verifying its recovery locator.
    /// Full source, tool and chunk checks happen again before the encoder starts.
    pub async fn resume_job(&self, id: String) -> Result<JobSnapshot, AppError> {
        let (original, epoch) = {
            let state = self.state.lock().await;
            if let Some(error) = &state.storage_error {
                return Err(error.clone());
            }
            if state.shutting_down {
                return Err(AppError::new(
                    "APP_CLOSING",
                    "The application is closing; jobs cannot resume.",
                    None,
                ));
            }
            let entry = state
                .entries
                .iter()
                .find(|entry| entry.snapshot.id == id)
                .ok_or_else(|| {
                    AppError::new("JOB_NOT_FOUND", "The saved job was not found.", None)
                })?;
            if !entry.snapshot.state.is_terminal()
                || entry.snapshot.state == JobState::Succeeded
                || entry.snapshot.recovery.is_none()
                || entry
                    .snapshot
                    .encode_settings
                    .as_ref()
                    .is_none_or(|settings| settings.backend != media_core::EncodeBackend::Av1an)
            {
                return Err(AppError::new(
                    "JOB_RESUME_UNAVAILABLE",
                    "This job has no stopped av1an progress available to resume.",
                    None,
                ));
            }
            if entry.task.as_ref().is_some_and(|task| !task.is_finished()) {
                return Err(AppError::new(
                    "JOB_BUSY",
                    "The previous attempt is still finishing cleanup. Try Resume again when it has finished.",
                    None,
                ));
            }
            (entry.snapshot.clone(), state.submission_epoch)
        };
        self.validate_recovery(&id).await?;
        let mut state = self.state.lock().await;
        if let Some(error) = &state.storage_error {
            return Err(error.clone());
        }
        if state.shutting_down {
            return Err(AppError::new(
                "APP_CLOSING",
                "The application is closing; jobs cannot resume.",
                None,
            ));
        }
        if state.submission_epoch != epoch {
            return Err(batch_canceled());
        }
        let index = state
            .entries
            .iter()
            .position(|entry| entry.snapshot.id == id)
            .ok_or_else(|| AppError::new("JOB_NOT_FOUND", "The saved job was not found.", None))?;
        if state.entries[index].snapshot != original {
            return Err(AppError::new(
                "JOB_BUSY",
                "This job changed while resume was being checked. Review its current state.",
                None,
            ));
        }
        let destination = crate::batch::destination_key(Path::new(&original.request.output_path))?;
        if state.entries.iter().any(|entry| {
            !entry.snapshot.state.is_terminal()
                && crate::batch::destination_key(Path::new(&entry.snapshot.request.output_path))
                    .unwrap_or_else(|_| {
                        crate::batch::path_key(Path::new(&entry.snapshot.request.output_path))
                    })
                    == destination
        }) {
            return Err(AppError::new(
                "OUTPUT_QUEUED",
                "This destination is already assigned to an unfinished job.",
                Some(original.request.output_path),
            ));
        }
        let log_path = self.log_dir.join(format!("{id}.log"));
        let (cancel, receiver) = watch::channel(false);
        let mut entry = state.entries.remove(index);
        entry.snapshot.state = JobState::Queued;
        entry.snapshot.error = None;
        entry.snapshot.progress_seconds = None;
        entry.cancel = cancel;
        entry.task = None;
        append_log(&mut entry.snapshot, "Resume requested with the original saved settings; source, tools and completed chunks will be verified before reuse.".into());
        let snapshot = entry.snapshot.clone();
        state.entries.push(entry);
        if let Err(error) = self.persist(&mut state).await {
            let mut entry = state.entries.pop().expect("reserved resume entry");
            entry.snapshot = original;
            entry.snapshot.error = Some(error.clone());
            state.entries.insert(index, entry);
            return Err(error);
        }
        let worker = self.clone();
        state
            .entries
            .last_mut()
            .expect("reserved resume entry")
            .task = Some(tokio::spawn(async move {
            worker.run_queued(id, receiver, log_path).await;
        }));
        self.queue_changed.notify_waiters();
        Ok(snapshot)
    }

    pub async fn cancel_all_jobs(&self) -> Result<Vec<JobSnapshot>, AppError> {
        let mut state = self.state.lock().await;
        state.submission_epoch = state.submission_epoch.wrapping_add(1);
        for preflight in &state.preflights {
            preflight.cancel.send_replace(true);
        }
        for entry in &mut state.entries {
            if !entry.snapshot.state.is_terminal() {
                entry.cancel.send_replace(true);
                entry.snapshot.state = JobState::Canceling;
                append_log(
                    &mut entry.snapshot,
                    "Queue stopped; cancellation applies to this job and all waiting jobs.".into(),
                );
            }
        }
        self.queue_changed.notify_waiters();
        self.persist(&mut state).await?;
        Ok(state
            .entries
            .iter()
            .rev()
            .map(|e| e.snapshot.clone())
            .collect())
    }

    /// Prevents new jobs and waits for process-tree and owned-output cleanup.
    pub async fn shutdown(&self) {
        let tasks = {
            let mut state = self.state.lock().await;
            state.shutting_down = true;
            let mut tasks = state
                .entries
                .iter_mut()
                .filter_map(|e| {
                    if !e.snapshot.state.is_terminal() {
                        e.cancel.send_replace(true);
                        if e.snapshot.state != JobState::Stopping {
                            e.snapshot.state = JobState::Canceling;
                        }
                    }
                    e.task.take()
                })
                .collect::<Vec<_>>();
            for preflight in state.preflights.drain(..) {
                preflight.cancel.send_replace(true);
                tasks.push(preflight.task);
            }
            let _ = self.persist(&mut state).await;
            self.queue_changed.notify_waiters();
            tasks
        };
        for task in tasks {
            let _ = task.await;
        }
    }

    async fn change(&self, id: &str, update: impl FnOnce(&mut JobSnapshot)) {
        let mut state = self.state.lock().await;
        if let Some(entry) = state.entries.iter_mut().find(|e| e.snapshot.id == id) {
            let before = entry.snapshot.state;
            let recovery_before = entry.snapshot.recovery.clone();
            update(&mut entry.snapshot);
            if entry.snapshot.state != before
                || entry.snapshot.recovery != recovery_before
                || entry.snapshot.state.is_terminal()
            {
                let _ = self.persist(&mut state).await;
                self.queue_changed.notify_waiters();
            }
        }
    }

    async fn phase(&self, id: &str, phase: JobState, message: &str) {
        self.change(id, |s| {
            if !matches!(s.state, JobState::Canceling | JobState::Stopping) {
                s.state = phase;
            }
            append_log(s, message.into());
        })
        .await;
    }

    async fn execute(&self, id: String, cancel: watch::Receiver<bool>, log_path: PathBuf) {
        let snapshot = {
            let state = self.state.lock().await;
            state
                .entries
                .iter()
                .find(|e| e.snapshot.id == id)
                .expect("registered job")
                .snapshot
                .clone()
        };
        let request = snapshot.request;
        let mut temporary = None;
        let mut scratch = Vec::new();
        let result = if let Some(mapping) = snapshot.mux_request {
            self.mux(&id, &mapping, &cancel, &log_path, &mut temporary)
                .await
        } else if let Some(settings) = snapshot.encode_settings {
            self.encode(
                &id,
                &request,
                &settings,
                &cancel,
                &log_path,
                &mut temporary,
                &mut scratch,
            )
            .await
        } else {
            self.remux(&id, &request, &cancel, &log_path, &mut temporary)
                .await
        };
        let cleanup_errors: Vec<_> = temporary
            .iter_mut()
            .chain(scratch.iter_mut())
            .filter_map(|temp| temp.cleanup().err())
            .collect();
        let saved_log = tokio::fs::metadata(&log_path).await.is_ok();
        // Storage-triggered cancellation is a failure, not a successful user
        // stop. Keep its diagnostic visible even when token cancellation wins.
        let result = match result {
            Err(error) if error.code == "JOB_CANCELED" => Err(self
                .state
                .lock()
                .await
                .storage_error
                .clone()
                .unwrap_or(error)),
            other => other,
        };
        self.change(&id, |snapshot| {
            if saved_log {
                snapshot.log_path = Some(log_path.to_string_lossy().into_owned());
            }
            // A successful publication already transitioned under the cancel lock.
            if let Err(error) = result {
                snapshot.state = if error.code == "JOB_CANCELED" {
                    if snapshot.state == JobState::Stopping {
                        JobState::Stopped
                    } else {
                        JobState::Canceled
                    }
                } else {
                    JobState::Failed
                };
                append_log(snapshot, error.message.clone());
                if snapshot.state == JobState::Stopped {
                    append_log(snapshot, if snapshot.recovery.is_some() {
                        "Stopped. Saved av1an progress is available for explicit resume.".into()
                    } else {
                        "Stopped before resumable work was created; start a new job to encode this source.".into()
                    });
                }
                snapshot.error = if matches!(snapshot.state, JobState::Canceled | JobState::Stopped) {
                    None
                } else {
                    Some(error)
                };
            }
            for error in cleanup_errors {
                append_log(snapshot, error.message.clone());
                snapshot.error = Some(error);
                if snapshot.state != JobState::Succeeded {
                    snapshot.state = JobState::Failed;
                }
            }
        })
        .await;
    }

    async fn remux(
        &self,
        id: &str,
        request: &RemuxRequest,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        temporary: &mut Option<Temporary>,
    ) -> Result<(), AppError> {
        check_cancel(cancel)?;
        self.phase(
            id,
            JobState::Preparing,
            "Checking the source, selected tracks, tools, and output destination.",
        )
        .await;
        let input = PathBuf::from(&request.input_path);
        let owned_request = request.clone();
        let (source, output) = tokio::task::spawn_blocking(move || {
            let source = Source::open(&input)?;
            let output = files::output_path(&owned_request, &source)?;
            Ok::<_, AppError>((source, output))
        })
        .await
        .map_err(|e| AppError::new("PREFLIGHT_FAILED", e.to_string(), None))??;
        check_cancel(cancel)?;
        let ffmpeg = discover("ffmpeg", cancel).await?;
        let ffprobe = discover("ffprobe", cancel).await?;
        let document = probe(
            &ffprobe,
            &source.path,
            cancel,
            Some(&request.stream_indices),
        )
        .await?;
        let selected = document.selected(&request.stream_indices)?;
        container::preflight(&output, &document, &selected, None)?;
        let duration = document.selected_duration(&selected);
        source.verify()?;
        check_cancel(cancel)?;
        self.change(id, |snapshot| snapshot.duration_seconds = duration)
            .await;
        tokio::fs::create_dir_all(self.log_dir.as_ref())
            .await
            .map_err(|e| files::error("LOG_CREATE_FAILED", e.to_string(), self.log_dir.as_ref()))?;
        check_cancel(cancel)?;
        *temporary = Some(Temporary::create(&output, id)?);
        let temp = temporary.as_ref().expect("created temporary");
        let spec = CommandSpec {
            executable: ffmpeg,
            args: remux_arguments(&source.path, &temp.path, &selected),
            cwd: None,
        };
        self.phase(
            id,
            JobState::Running,
            "Copying selected tracks into the temporary Matroska output.",
        )
        .await;
        let (sender, mut receiver) = mpsc::channel(256);
        let observer = self.clone();
        let event_id = id.to_owned();
        let event_task = tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                observer
                    .change(&event_id, |snapshot| match event {
                        ProcessEvent::Stdout(line) => {
                            if let Some(seconds) = progress_seconds(&line) {
                                snapshot.progress_seconds = Some(seconds);
                            }
                        }
                        ProcessEvent::Stderr(line) => append_log(snapshot, line),
                    })
                    .await;
            }
        });
        let result = supervisor::run(
            &spec,
            cancel.clone(),
            sender,
            log_path,
            Duration::from_secs(24 * 60 * 60),
        )
        .await;
        let _ = event_task.await;
        if tokio::fs::metadata(log_path).await.is_ok() {
            self.change(id, |snapshot| {
                snapshot.log_path = Some(log_path.to_string_lossy().into_owned());
            })
            .await;
        }
        let result = result.map_err(|e| process_error(e, &source.path))?;
        check_cancel(cancel)?;
        if !result.status.success() {
            return Err(files::error(
                "REMUX_FAILED",
                format!(
                    "FFmpeg failed ({}). The source and existing destinations were preserved; see the job log.",
                    result.status
                ),
                &source.path,
            ));
        }
        self.phase(
            id,
            JobState::Finalizing,
            "Checking output streams, metadata, chapters, and duration before publication.",
        )
        .await;
        temp.flush_nonempty_async().await?;
        let artifact = probe(&ffprobe, &temp.path, cancel, None).await?;
        metadata::verify(&document, &selected, &artifact)?;
        source.verify()?;
        check_cancel(cancel)?;
        self.finalize(id, cancel, &source, temp, &output, None)
            .await
    }

    async fn finalize(
        &self,
        id: &str,
        cancel: &watch::Receiver<bool>,
        source: &Source,
        temporary: &Temporary,
        output: &Path,
        cadence: Option<container::Cadence>,
    ) -> Result<(), AppError> {
        // Cancellation and publication share a single commit lock: a cancel
        // accepted before this point cannot publish an output; after publication
        // the job is succeeded and cancellation is a no-op.
        let converted = container::prepare(temporary, output, id, cancel, cadence).await?;
        let temporary = converted.as_ref().unwrap_or(temporary);
        let mut state = self.state.lock().await;
        check_cancel(cancel)?;
        source.verify()?;
        temporary.publish(output)?;
        if let Some(entry) = state.entries.iter_mut().find(|e| e.snapshot.id == id) {
            entry.snapshot.state = JobState::Succeeded;
            entry.snapshot.progress_seconds = entry.snapshot.duration_seconds;
            append_log(
                &mut entry.snapshot,
                "Verified output published. The source was preserved.".into(),
            );
        }
        // Publication is already committed; a history-write error is reported
        // on the successful job and must not relabel or delete its output.
        let _ = self.persist(&mut state).await;
        Ok(())
    }
}

async fn wait_cancel(mut cancel: watch::Receiver<bool>) {
    loop {
        if *cancel.borrow_and_update() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

fn append_log(snapshot: &mut JobSnapshot, line: String) {
    if line.trim().is_empty() {
        return;
    }
    if snapshot.logs.len() >= MAX_LOG_LINES {
        snapshot.logs.remove(0);
    }
    snapshot.logs.push(line.chars().take(2000).collect());
}

async fn discover(name: &str, cancel: &watch::Receiver<bool>) -> Result<PathBuf, AppError> {
    check_cancel(cancel)?;
    let result = find_executable(&[name])
        .await
        .map_err(|e| AppError::new("TOOL_DISCOVERY_FAILED", e, None))?;
    check_cancel(cancel)?;
    result.ok_or_else(|| {
        AppError::new(
            "TOOL_MISSING",
            format!(
                "{name} was not found on PATH. Install FFmpeg with FFprobe and restart jesses."
            ),
            None,
        )
    })
}

/// Inspect container headers only. Packet counts and decoded-frame validation
/// remain part of execution, where they are cancelable and visibly Preparing.
async fn inspect_encode_source(
    input: &BatchEncodeInput,
    settings: &EncodeSettings,
    cancel: &watch::Receiver<bool>,
) -> Result<media_core::MediaFile, AppError> {
    check_cancel(cancel)?;
    let path = PathBuf::from(&input.input_path);
    let canonical = tokio::fs::canonicalize(&path).await.map_err(|error| {
        files::error(
            if error.kind() == std::io::ErrorKind::NotFound {
                "FILE_NOT_FOUND"
            } else {
                "FILE_UNREADABLE"
            },
            error.to_string(),
            &path,
        )
    })?;
    let source = tokio::task::spawn_blocking(move || Source::open(&canonical))
        .await
        .map_err(|e| files::error("FILE_UNREADABLE", e.to_string(), &path))??;
    let executable = discover("ffprobe", cancel).await?;
    let args = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .chain(std::iter::once(source.path.as_os_str().to_owned()))
    .collect();
    let output = supervisor::run_capture(
        &CommandSpec {
            executable,
            args,
            cwd: None,
        },
        cancel.clone(),
        2 * 1024 * 1024,
        Duration::from_secs(30),
    )
    .await
    .map_err(|e| process_error(e, &source.path))?;
    check_cancel(cancel)?;
    if !output.status.success() {
        return Err(files::error(
            "PROBE_FAILED",
            format!(
                "FFprobe could not inspect this source: {}",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(600)
                    .collect::<String>()
            ),
            &source.path,
        ));
    }
    validate_encode_preview(&output.stdout, input, settings).map_err(|mut error| {
        error.path = Some(source.path.to_string_lossy().into_owned());
        error
    })?;
    source.verify()?;
    let size = tokio::fs::metadata(&source.path)
        .await
        .map_err(|e| files::error("FILE_UNREADABLE", e.to_string(), &source.path))?
        .len();
    let canonical = source
        .path
        .to_str()
        .ok_or_else(|| {
            files::error(
                "INVALID_INPUT",
                "The source path cannot be represented as Unicode.",
                &source.path,
            )
        })?
        .to_owned();
    crate::probe::parse_probe(
        &output.stdout,
        canonical,
        source
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        size,
    )
}

fn validate_encode_preview(
    bytes: &[u8],
    input: &BatchEncodeInput,
    settings: &EncodeSettings,
) -> Result<(), AppError> {
    let document: Document = serde_json::from_slice(bytes).map_err(|_| {
        AppError::new(
            "PROBE_INVALID_RESPONSE",
            "FFprobe returned unreadable media metadata.",
            None,
        )
    })?;
    let selected = document.selected(&input.stream_indices)?;
    let videos: Vec<_> = selected
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .collect();
    if videos.len() != 1 || videos[0].index != input.video_stream_index {
        return Err(AppError::new(
            "STREAM_SELECTION_INVALID",
            "Select exactly one video stream matching the video chosen for encoding.",
            None,
        ));
    }
    let plan = encode_plan::Plan::build(
        &document,
        &selected,
        &EncodeSettings {
            video_stream_index: input.video_stream_index,
            ..settings.clone()
        },
    )?;
    if settings.backend == media_core::EncodeBackend::Av1an {
        av1an::validate_input(&document, &plan)?;
    }
    Ok(())
}

async fn probe(
    executable: &Path,
    path: &Path,
    cancel: &watch::Receiver<bool>,
    selected: Option<&[u32]>,
) -> Result<Document, AppError> {
    check_cancel(cancel)?;
    let args = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-show_chapters",
        "-count_packets",
        "-show_data_hash",
        "sha256",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .chain(std::iter::once(path.as_os_str().to_owned()))
    .collect();
    let output = supervisor::run_capture(
        &CommandSpec {
            executable: executable.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        4 * 1024 * 1024,
        Duration::from_secs(5 * 60),
    )
    .await
    .map_err(|e| process_error(e, path))?;
    check_cancel(cancel)?;
    if !output.status.success() {
        let detail: String = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(600)
            .collect();
        return Err(files::error(
            "PROBE_FAILED",
            format!("FFprobe could not validate the media: {detail}"),
            path,
        ));
    }
    let mut document: Document = serde_json::from_slice(&output.stdout).map_err(|_| {
        files::error(
            "PROBE_INVALID_RESPONSE",
            "FFprobe returned unreadable metadata.",
            path,
        )
    })?;
    // A Matroska subtitle header may report the container start rather than the
    // first cue. Compare actual packet starts so remuxing cannot create a false
    // synchronization failure or conceal a real cue offset.
    for stream in &mut document.streams {
        if stream.codec_type.as_deref() == Some("subtitle")
            && selected.is_none_or(|indices| indices.contains(&stream.index))
            && stream
                .nb_read_packets
                .as_deref()
                .and_then(|n| n.parse::<u64>().ok())
                .is_some_and(|n| n > 0)
        {
            stream.packet_start_time =
                Some(subtitle_packet_start(executable, path, stream.index, cancel).await?);
        }
    }
    Ok(document)
}

async fn subtitle_packet_start(
    executable: &Path,
    path: &Path,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<f64, AppError> {
    check_cancel(cancel)?;
    let args = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-select_streams",
        &index.to_string(),
        "-read_intervals",
        "%+#1",
        "-show_packets",
        "-show_entries",
        "packet=pts_time",
        "-of",
        "json",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .chain(std::iter::once(path.as_os_str().to_owned()))
    .collect();
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: executable.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(5 * 60),
    )
    .await
    .map_err(|e| process_error(e, path))?;
    check_cancel(cancel)?;
    #[derive(serde::Deserialize)]
    struct Packets {
        packets: Vec<Packet>,
    }
    #[derive(serde::Deserialize)]
    struct Packet {
        pts_time: String,
    }
    let start = serde_json::from_slice::<Packets>(&result.stdout)
        .ok()
        .and_then(|packets| {
            if packets.packets.len() == 1 {
                packets.packets[0]
                    .pts_time
                    .parse::<f64>()
                    .ok()
                    .filter(|pts| pts.is_finite())
            } else {
                None
            }
        });
    if !result.status.success() || !result.stderr.is_empty() || start.is_none() {
        return Err(files::error(
            "PROBE_INVALID_RESPONSE",
            "The first subtitle packet timestamp could not be validated.",
            path,
        ));
    }
    Ok(start.expect("validated subtitle timestamp"))
}

fn remux_arguments(input: &Path, output: &Path, selected: &[&metadata::Stream]) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "warning",
        "-nostats",
        "-progress",
        "pipe:1",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(input.as_os_str().to_owned());
    for stream in selected {
        args.extend([
            OsString::from("-map"),
            OsString::from(format!("0:{}", stream.index)),
        ]);
    }
    args.extend(
        ["-map_metadata", "0", "-map_chapters", "0", "-c", "copy"]
            .into_iter()
            .map(OsString::from),
    );
    for (index, stream) in selected.iter().enumerate() {
        let disposition = stream
            .disposition
            .iter()
            .filter(|(_, enabled)| **enabled != 0)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join("+");
        args.extend([
            OsString::from(format!("-disposition:{index}")),
            OsString::from(if disposition.is_empty() {
                "0"
            } else {
                &disposition
            }),
        ]);
    }
    // -y is confined to the uniquely reserved owned temporary sibling. The user
    // destination is never passed to FFmpeg and is published without clobbering.
    args.extend(["-f", "matroska", "-y"].into_iter().map(OsString::from));
    args.push(output.as_os_str().to_owned());
    args
}

fn progress_seconds(line: &str) -> Option<f64> {
    let seconds = line
        .strip_prefix("out_time_us=")?
        .trim()
        .parse::<f64>()
        .ok()?
        / 1_000_000.0;
    (seconds.is_finite() && seconds >= 0.0).then_some(seconds)
}

/// Query the installed build's bounded, application-qualified scalar catalog.
pub async fn get_encoder_parameters(
    encoder: media_core::VideoEncoder,
    backend: media_core::EncodeBackend,
    cancel: watch::Receiver<bool>,
) -> Result<media_core::EncoderParameterCatalog, AppError> {
    parameters::catalog(encoder, backend, cancel).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires installed FFmpeg and FFprobe"]
    async fn subtitle_start_uses_the_first_selected_packet_after_earlier_media_packets() {
        let nonce = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "jesses-subtitle-start-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let input = dir.join("delayed subtitle.mkv");
        let subtitles = dir.join("cues.srt");
        std::fs::write(&subtitles, b"1\n00:00:00,740 --> 00:00:01,900\nLater cue\n").unwrap();
        let (_sender, cancel) = watch::channel(false);
        let ffmpeg = discover("ffmpeg", &cancel).await.unwrap();
        let ffprobe = discover("ffprobe", &cancel).await.unwrap();
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x72:rate=24:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=48000:duration=2",
            "-i",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        args.push(subtitles.as_os_str().to_owned());
        args.extend(
            [
                "-map", "0:v", "-map", "1:a", "-map", "2:s", "-c:v", "ffv1", "-c:a", "flac",
                "-c:s", "ass",
            ]
            .into_iter()
            .map(OsString::from),
        );
        args.push(input.as_os_str().to_owned());
        let output = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg,
                args,
                cwd: None,
            },
            cancel.clone(),
            64 * 1024,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document = probe(&ffprobe, &input, &cancel, Some(&[0, 1, 2]))
            .await
            .unwrap();
        assert_eq!(document.streams[2].packet_start_time, Some(0.740));
        assert_eq!(document.streams[2].nb_read_packets.as_deref(), Some("1"));
        assert!(document.streams[0].packet_start_time.is_none());
        let without_subtitles = probe(&ffprobe, &input, &cancel, Some(&[0, 1]))
            .await
            .unwrap();
        assert!(
            without_subtitles.streams[2].packet_start_time.is_none(),
            "Unselected subtitle tracks must not require supplemental packet inspection"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn progress_rejects_missing_negative_and_nonfinite_values() {
        assert_eq!(progress_seconds("out_time_us=1234567"), Some(1.234567));
        for line in [
            "out_time_us=N/A",
            "out_time_us=NaN",
            "out_time_us=-1",
            "progress=end",
        ] {
            assert_eq!(progress_seconds(line), None);
        }
    }

    #[tokio::test]
    async fn immediate_cancel_and_shutdown_prevent_publication_and_new_jobs() {
        let dir = std::env::temp_dir();
        let manager = JobManager::new(dir.join("jesses-unstarted-logs"));
        let request = RemuxRequest {
            input_path: dir.join("missing-source.mkv").to_string_lossy().into(),
            output_path: dir.join("missing-output.mkv").to_string_lossy().into(),
            stream_indices: vec![0],
        };
        let job = manager.start_remux(request.clone()).await.unwrap();
        manager.cancel_job(job.id).await.unwrap();
        manager.shutdown().await;
        assert_eq!(manager.list_jobs().await[0].state, JobState::Canceled);
        assert_eq!(
            manager.start_remux(request).await.unwrap_err().code,
            "APP_CLOSING"
        );
    }

    #[tokio::test]
    async fn cancellation_before_finalization_cannot_publish() {
        let nonce = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "jesses-cancel-finalize-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let input = dir.join("source.mkv");
        let output = dir.join("output.mkv");
        std::fs::write(&input, b"source").unwrap();
        let source = Source::open(&input).unwrap();
        let mut temporary = Temporary::create(&output, "cancel").unwrap();
        let manager = JobManager::new(dir.join("logs"));
        let (_sender, receiver) = watch::channel(true);
        assert_eq!(
            manager
                .finalize("id", &receiver, &source, &temporary, &output, None)
                .await
                .unwrap_err()
                .code,
            "JOB_CANCELED"
        );
        assert!(!output.exists());
        let mp4 = dir.join("output.mp4");
        assert_eq!(
            manager
                .finalize("id", &receiver, &source, &temporary, &mp4, None)
                .await
                .unwrap_err()
                .code,
            "JOB_CANCELED"
        );
        assert!(!mp4.exists());
        temporary.cleanup().unwrap();
        drop(source);
        assert_eq!(std::fs::read(input).unwrap(), b"source");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn stop_queue_cancels_waiters_without_starting_and_preserves_settings() {
        let dir = std::env::temp_dir();
        let manager = JobManager::new(dir.join("jesses-queue-test-logs"));
        let slot = manager.execution.lock().await;
        for index in 0..3 {
            manager
                .enqueue_encode(EncodeRequest {
                    source: RemuxRequest {
                        input_path: dir
                            .join("missing-queue-source.mkv")
                            .to_string_lossy()
                            .into(),
                        output_path: dir
                            .join(format!("missing-queue-output-{index}.mkv"))
                            .to_string_lossy()
                            .into(),
                        stream_indices: vec![0],
                    },
                    settings: EncodeSettings {
                        preset: index + 4,
                        encoder: if index == 1 {
                            media_core::VideoEncoder::X264
                        } else {
                            media_core::VideoEncoder::SvtAv1
                        },
                        crf: if index == 1 { 23 } else { 30 },
                        ..Default::default()
                    },
                })
                .await
                .unwrap();
        }
        assert!(
            manager
                .list_jobs()
                .await
                .iter()
                .all(|job| job.state == JobState::Queued)
        );
        manager.cancel_all_jobs().await.unwrap();
        // Workers waiting for the execution slot must respond to cancellation
        // even while the current operation still owns the slot.
        tokio::time::timeout(Duration::from_secs(2), manager.shutdown())
            .await
            .unwrap();
        let jobs = manager.list_jobs().await;
        assert!(jobs.iter().all(|job| job.state == JobState::Canceled));
        assert_eq!(
            jobs.iter()
                .map(|job| job.encode_settings.as_ref().unwrap().preset)
                .collect::<Vec<_>>(),
            vec![6, 5, 4]
        );
        assert!(
            jobs.iter()
                .all(|job| !job.logs.iter().any(|line| line.contains("Checking")))
        );
        assert_eq!(
            jobs[1].encode_settings.as_ref().unwrap().encoder,
            media_core::VideoEncoder::X264
        );
        assert_eq!(jobs[1].encode_settings.as_ref().unwrap().crf, 23);
        drop(slot);
    }

    #[tokio::test]
    async fn unfinished_history_reopens_as_interrupted_without_touching_files() {
        for settings in [
            EncodeSettings::default(),
            EncodeSettings {
                encoder: media_core::VideoEncoder::X264,
                crf: 23,
                preset: 5,
                ..Default::default()
            },
        ] {
            let path = std::env::temp_dir().join(format!(
                "jesses-recovery-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            let input = path.join("source.mkv");
            let output = path.join("existing.mkv");
            std::fs::write(&input, b"original source").unwrap();
            std::fs::write(&output, b"existing output").unwrap();
            let (history, _) = history::History::open(path.join("history")).await.unwrap();
            history
                .save(vec![JobSnapshot {
                    id: "previous-session-job".into(),
                    state: JobState::Running,
                    request: RemuxRequest {
                        input_path: input.to_string_lossy().into(),
                        output_path: output.to_string_lossy().into(),
                        stream_indices: vec![0],
                    },
                    encode_settings: Some(settings.clone()),
                    mux_request: None,
                    recovery: None,
                    progress_seconds: Some(1.0),
                    duration_seconds: Some(5.0),
                    logs: vec!["Started with saved settings".into()],
                    error: None,
                    log_path: None,
                }])
                .await
                .unwrap();
            drop(history);
            if settings.encoder == media_core::VideoEncoder::SvtAv1 {
                // Reopen an actual pre-encoder-field record, including the old
                // standalone backend spelling, through the history boundary.
                let record_path = path.join("history").join("jobs.json");
                let mut record: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
                let saved = record["jobs"][0]["encodeSettings"].as_object_mut().unwrap();
                saved.remove("encoder");
                saved.insert("backend".into(), serde_json::json!("svtAv1"));
                std::fs::write(record_path, serde_json::to_vec(&record).unwrap()).unwrap();
            }
            let manager = JobManager::open(path.join("logs"), path.join("history")).await;
            manager.ready().await.unwrap();
            let jobs = manager.list_jobs().await;
            assert_eq!(jobs[0].state, JobState::Interrupted);
            assert_eq!(jobs[0].error.as_ref().unwrap().code, "JOB_INTERRUPTED");
            assert_eq!(jobs[0].encode_settings, Some(settings));
            manager.shutdown().await;
            drop(manager);
            assert_eq!(std::fs::read(input).unwrap(), b"original source");
            assert_eq!(std::fs::read(output).unwrap(), b"existing output");
            assert!(!path.join("logs").exists());
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[tokio::test]
    async fn unreadable_history_blocks_job_admission_without_erasing_it() {
        let path = std::env::temp_dir().join(format!(
            "jesses-invalid-history-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("jobs.json"), b"corrupt history retained").unwrap();
        let manager = JobManager::open(path.join("logs"), path.clone()).await;
        assert_eq!(
            manager.ready().await.unwrap_err().code,
            "JOB_HISTORY_FAILED"
        );
        let error = manager
            .start_remux(RemuxRequest {
                input_path: path.join("source.mkv").to_string_lossy().into(),
                output_path: path.join("output.mkv").to_string_lossy().into(),
                stream_indices: vec![0],
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "JOB_HISTORY_FAILED");
        assert_eq!(
            std::fs::read(path.join("jobs.json")).unwrap(),
            b"corrupt history retained"
        );
        drop(manager);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn storage_failure_is_not_reported_as_user_cancellation() {
        let path = std::env::temp_dir().join(format!(
            "jesses-history-failure-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let manager = JobManager::open(path.join("logs"), path.join("history")).await;
        manager.ready().await.unwrap();
        let slot = manager.execution.lock().await;
        let job = manager
            .enqueue_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: path.join("source.mkv").to_string_lossy().into(),
                    output_path: path.join("output.mkv").to_string_lossy().into(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings::default(),
            })
            .await
            .unwrap();
        assert!(
            job.log_path
                .as_ref()
                .unwrap()
                .ends_with(&format!("{}.log", job.id))
        );
        let record = path.join("history/jobs.json");
        let saved = path.join("history/previous.json");
        std::fs::rename(&record, &saved).unwrap();
        std::fs::create_dir(&record).unwrap();
        manager
            .phase(&job.id, JobState::Preparing, "Preparing a job.")
            .await;
        tokio::time::timeout(Duration::from_secs(2), manager.shutdown())
            .await
            .unwrap();
        let jobs = manager.list_jobs().await;
        assert_eq!(jobs[0].state, JobState::Failed);
        assert_eq!(jobs[0].error.as_ref().unwrap().code, "JOB_HISTORY_FAILED");
        assert!(std::fs::read_to_string(saved).unwrap().contains(&job.id));
        assert!(!path.join("output.mkv").exists());
        drop(slot);
        drop(manager);
        std::fs::remove_dir_all(path).unwrap();
    }
}
