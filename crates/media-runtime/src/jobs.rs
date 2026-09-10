//! One active, in-memory copy/remux job with immutable selection and no-clobber
//! output publication. This is not a durable queue or encoding pipeline.

mod files;
mod metadata;
#[cfg(test)]
mod real_tests;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use media_core::{AppError, JobSnapshot, JobState, RemuxRequest};
use tokio::sync::{Mutex, mpsc, watch};

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
    snapshot: JobSnapshot,
    cancel: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    shutting_down: bool,
}

#[derive(Clone)]
pub struct JobManager {
    state: Arc<Mutex<State>>,
    log_dir: Arc<PathBuf>,
}

fn canceled() -> AppError {
    AppError::new(
        "JOB_CANCELED",
        "The job was canceled. No output was published.",
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
        }
    }

    pub async fn start_remux(&self, request: RemuxRequest) -> Result<JobSnapshot, AppError> {
        files::validate_request(&request)?;
        let mut state = self.state.lock().await;
        if state.shutting_down {
            return Err(AppError::new(
                "APP_CLOSING",
                "The application is closing; new jobs cannot start.",
                None,
            ));
        }
        if state
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
        while state.entries.len() >= MAX_HISTORY {
            state.entries.remove(0);
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
            progress_seconds: None,
            duration_seconds: None,
            logs: vec!["Copy/remux job queued.".into()],
            error: None,
            log_path: None,
        };
        let (cancel, receiver) = watch::channel(false);
        let worker = self.clone();
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            worker.execute(task_id, receiver, log_path).await;
        });
        state.entries.push(Entry {
            snapshot: snapshot.clone(),
            cancel,
            task: Some(task),
        });
        Ok(snapshot)
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
        Ok(entry.snapshot.clone())
    }

    /// Prevents new jobs and waits for process-tree and owned-output cleanup.
    pub async fn shutdown(&self) {
        let tasks = {
            let mut state = self.state.lock().await;
            state.shutting_down = true;
            state
                .entries
                .iter_mut()
                .filter_map(|e| {
                    if !e.snapshot.state.is_terminal() {
                        e.cancel.send_replace(true);
                        e.snapshot.state = JobState::Canceling;
                    }
                    e.task.take()
                })
                .collect::<Vec<_>>()
        };
        for task in tasks {
            let _ = task.await;
        }
    }

    async fn change(&self, id: &str, update: impl FnOnce(&mut JobSnapshot)) {
        if let Some(entry) = self
            .state
            .lock()
            .await
            .entries
            .iter_mut()
            .find(|e| e.snapshot.id == id)
        {
            update(&mut entry.snapshot);
        }
    }

    async fn phase(&self, id: &str, phase: JobState, message: &str) {
        self.change(id, |s| {
            if s.state != JobState::Canceling {
                s.state = phase;
            }
            append_log(s, message.into());
        })
        .await;
    }

    async fn execute(&self, id: String, cancel: watch::Receiver<bool>, log_path: PathBuf) {
        let request = {
            let state = self.state.lock().await;
            state
                .entries
                .iter()
                .find(|e| e.snapshot.id == id)
                .expect("registered job")
                .snapshot
                .request
                .clone()
        };
        let mut temporary = None;
        let result = self
            .remux(&id, &request, &cancel, &log_path, &mut temporary)
            .await;
        let cleanup_error = temporary.as_mut().and_then(|temp| temp.cleanup().err());
        let saved_log = tokio::fs::metadata(&log_path).await.is_ok();
        self.change(&id, |snapshot| {
            if saved_log {
                snapshot.log_path = Some(log_path.to_string_lossy().into_owned());
            }
            // A successful publication already transitioned under the cancel lock.
            if let Err(error) = result {
                snapshot.state = if error.code == "JOB_CANCELED" {
                    JobState::Canceled
                } else {
                    JobState::Failed
                };
                append_log(snapshot, error.message.clone());
                snapshot.error = if snapshot.state == JobState::Canceled {
                    None
                } else {
                    Some(error)
                };
            }
            if let Some(error) = cleanup_error {
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
        let document = probe(&ffprobe, &source.path, cancel).await?;
        let selected = document.selected(&request.stream_indices)?;
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
        let artifact = probe(&ffprobe, &temp.path, cancel).await?;
        metadata::verify(&document, &selected, &artifact)?;
        source.verify()?;
        check_cancel(cancel)?;
        self.finalize(id, cancel, &source, temp, &output).await
    }

    async fn finalize(
        &self,
        id: &str,
        cancel: &watch::Receiver<bool>,
        source: &Source,
        temporary: &Temporary,
        output: &Path,
    ) -> Result<(), AppError> {
        // Cancellation and publication share a single commit lock: a cancel
        // accepted before this point cannot publish an output; after publication
        // the job is succeeded and cancellation is a no-op.
        let mut state = self.state.lock().await;
        check_cancel(cancel)?;
        source.verify()?;
        temporary.publish(output)?;
        if let Some(entry) = state.entries.iter_mut().find(|e| e.snapshot.id == id) {
            entry.snapshot.state = JobState::Succeeded;
            entry.snapshot.progress_seconds = entry.snapshot.duration_seconds;
            append_log(
                &mut entry.snapshot,
                "Verified Matroska output published. The source was preserved.".into(),
            );
        }
        Ok(())
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

async fn probe(
    executable: &Path,
    path: &Path,
    cancel: &watch::Receiver<bool>,
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
    serde_json::from_slice(&output.stdout).map_err(|_| {
        files::error(
            "PROBE_INVALID_RESPONSE",
            "FFprobe returned unreadable metadata.",
            path,
        )
    })
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

#[cfg(test)]
mod tests {
    use super::*;

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
                .finalize("id", &receiver, &source, &temporary, &output)
                .await
                .unwrap_err()
                .code,
            "JOB_CANCELED"
        );
        assert!(!output.exists());
        temporary.cleanup().unwrap();
        drop(source);
        assert_eq!(std::fs::read(input).unwrap(), b"source");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
