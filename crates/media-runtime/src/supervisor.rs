//! Owned media-tool process trees with cancellation and bounded diagnostics.
//!
//! Windows uses an atomic job-list spawn and a non-inheritable kill-on-close
//! job. Unix tools run in a new process group; descendants must not deliberately
//! detach from that group. Dropping the run future also terminates the tree.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Mutex, mpsc, watch},
};

#[cfg(windows)]
#[path = "supervisor/windows.rs"]
mod platform;
#[cfg(unix)]
#[path = "supervisor/unix.rs"]
mod platform;

#[path = "supervisor/ansi.rs"]
mod ansi;
#[path = "supervisor/av1an_activity.rs"]
mod av1an_activity;
#[path = "supervisor/pause.rs"]
mod pause;
pub(crate) use pause::scoped_pause;
pub use pause::{PauseControl, with_pause};
mod console;
#[cfg(feature = "console-fixture")]
pub use console::console_fixture_presentations;
pub(crate) use console::source_tool_console_profile;
pub use console::{
    ConsoleProfile, configure_console_preferences, console_helper_main, initialize_console_host,
    shutdown_console_presentations, with_console_profile,
};

type CaptureObserver = std::sync::Arc<dyn Fn(bool, &[u8]) + Send + Sync>;
tokio::task_local! { static CAPTURE_OBSERVER: CaptureObserver; }
#[cfg(test)]
tokio::task_local! {
    static BLOCK_PIPELINE_LOG_WRITE: std::sync::Arc<tokio::sync::Notify>;
}
pub(crate) async fn with_capture_observer<T>(
    observer: CaptureObserver,
    future: impl std::future::Future<Output = T>,
) -> T {
    CAPTURE_OBSERVER.scope(observer, future).await
}

const RECORD_BYTES: usize = 8192;
const LOG_BYTES: u64 = 4 * 1024 * 1024;
const PIPE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct CommandSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
}

/// Environment changes apply only to an owned child and its descendants.
#[derive(Clone, Debug, Default)]
pub struct ChildEnvironment {
    pub path: Option<OsString>,
    pub variables: Vec<(&'static str, Option<OsString>)>,
    pub dynamic_variables: Vec<(String, Option<OsString>)>,
}

impl ChildEnvironment {
    pub fn with_path(path: &OsStr) -> Self {
        Self {
            path: Some(path.to_owned()),
            variables: Vec::new(),
            dynamic_variables: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ProcessEvent {
    Stdout(String),
    Stderr(String),
}

pub(crate) fn is_ffmpeg_progress_line(line: &str) -> bool {
    line.split_once('=').is_some_and(|(key, _)| {
        matches!(
            key,
            "frame"
                | "fps"
                | "bitrate"
                | "total_size"
                | "out_time_us"
                | "out_time_ms"
                | "out_time"
                | "dup_frames"
                | "drop_frames"
                | "speed"
                | "progress"
        ) || (key.starts_with("stream_") && key.ends_with("_q"))
    })
}

#[derive(Debug)]
pub struct ProcessResult {
    pub status: ExitStatus,
}

#[derive(Debug)]
pub struct CapturedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub struct StreamedOutput<T> {
    pub status: ExitStatus,
    pub value: T,
    /// The newest stderr bytes, truncated to the requested diagnostic limit.
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub struct PipelineResult {
    pub producer_status: ExitStatus,
    pub consumer_status: ExitStatus,
    pub bytes_transferred: u64,
    /// Bounded stream prefix for callers that need the binary producer header.
    pub(crate) producer_prefix: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineStage {
    Producer,
    Consumer,
}

impl std::fmt::Display for PipelineStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Producer => "producer",
            Self::Consumer => "consumer",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    // Both Stop (which keeps verified progress) and Abort end the tools this
    // way, so the wording must not claim the work was thrown away.
    #[error("The job's tools were stopped.")]
    Cancelled,
    #[error("The tool exceeded its time limit.")]
    Timeout,
    #[error("The tool exceeded its inactivity limit without encoding progress or log updates.")]
    InactivityTimeout,
    #[error("The tool produced more output than the capture limit allows.")]
    OutputLimit,
    #[error("The tool output could not be parsed: {0}")]
    OutputParse(String),
    #[error("The {stage} stage failed ({status}).")]
    StageFailed {
        stage: PipelineStage,
        status: ExitStatus,
    },
    #[error("The consumer exited before all producer bytes were delivered.")]
    EarlyConsumerExit,
    #[error("The process tree could not be confirmed stopped: {0}")]
    Cleanup(io::Error),
    #[error("The tool could not be started, monitored, or logged: {0}")]
    Io(#[from] io::Error),
}

impl SupervisorError {
    pub(crate) fn is_pipeline_stage_failure(&self) -> bool {
        matches!(self, Self::StageFailed { .. } | Self::EarlyConsumerExit)
            || matches!(self, Self::Io(error) if error.kind() == io::ErrorKind::BrokenPipe)
    }
}

pub(crate) async fn cancelled(mut cancel: watch::Receiver<bool>) {
    loop {
        if *cancel.borrow_and_update() {
            return;
        }
        // Losing the job owner is cancellation, not permission to run forever.
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

/// UI events are bounded, lossy diagnostics. Disk logs retain the newest two
/// 4 MiB segments. Never reconstruct machine-readable tool output from events;
/// use `run_capture` or `run_streaming_stdout` instead.
pub async fn run(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
) -> Result<ProcessResult, SupervisorError> {
    run_with_path(spec, cancel, events, log_path, time_limit, None).await
}

/// Override PATH for this owned child and its descendants only. Every other
/// environment entry is inherited unchanged, including Windows drive state.
pub async fn run_with_path(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
    path: Option<&OsStr>,
) -> Result<ProcessResult, SupervisorError> {
    run_with_pause(spec, cancel, events, log_path, time_limit, path, None).await
}

/// As run_with_path, with live process-tree pause and a clock excluding pauses.
pub async fn run_with_pause(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
    path: Option<&OsStr>,
    pause: Option<&PauseControl>,
) -> Result<ProcessResult, SupervisorError> {
    let environment = path.map(ChildEnvironment::with_path);
    run_with_environment(
        spec,
        cancel,
        events,
        log_path,
        time_limit,
        environment.as_ref(),
        pause,
    )
    .await
}

/// Run with scoped environment changes and optional process-tree pause.
pub async fn run_with_environment(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
    environment: Option<&ChildEnvironment>,
    pause: Option<&PauseControl>,
) -> Result<ProcessResult, SupervisorError> {
    run_with_deadline(
        spec,
        cancel,
        events,
        log_path,
        RunDeadline::Runtime(time_limit),
        environment,
        pause,
    )
    .await
}

/// Run AV1AN with a pause-aware inactivity limit. Advancing frame counters or
/// changes to either progress file or growing chunk outputs restart the clock.
/// `chunk_restarts` is how often AV1AN may start one chunk's output again.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_with_environment_until_inactive(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    inactivity_limit: Duration,
    progress_paths: [&Path; 2],
    chunk_directory: &Path,
    chunk_restarts: u32,
    environment: Option<&ChildEnvironment>,
    pause: Option<&PauseControl>,
) -> Result<ProcessResult, SupervisorError> {
    run_with_deadline(
        spec,
        cancel,
        events,
        log_path,
        RunDeadline::Inactivity {
            limit: inactivity_limit,
            progress_paths,
            chunk_directory,
            chunk_restarts,
        },
        environment,
        pause,
    )
    .await
}

#[derive(Clone, Copy)]
enum RunDeadline<'a> {
    Runtime(Duration),
    Inactivity {
        limit: Duration,
        progress_paths: [&'a Path; 2],
        chunk_directory: &'a Path,
        chunk_restarts: u32,
    },
}

async fn run_with_deadline(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    deadline: RunDeadline<'_>,
    environment: Option<&ChildEnvironment>,
    pause: Option<&PauseControl>,
) -> Result<ProcessResult, SupervisorError> {
    let profile = console::logged_profile(&[spec]);
    #[cfg(windows)]
    if console::av1an_terminal_enabled(profile) {
        return console::run_av1an_terminal(
            spec,
            cancel,
            events,
            log_path,
            deadline,
            environment,
            pause,
        )
        .await;
    }
    console::scope(profile, &[spec], cancel, |cancel| {
        run_with_environment_inner(spec, cancel, events, log_path, deadline, environment, pause)
    })
    .await
}
#[allow(clippy::too_many_arguments)]
async fn run_with_environment_inner(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    deadline: RunDeadline<'_>,
    environment: Option<&ChildEnvironment>,
    pause: Option<&PauseControl>,
) -> Result<ProcessResult, SupervisorError> {
    if *cancel.borrow() {
        return Err(SupervisorError::Cancelled);
    }
    let log = Mutex::new(RotatingLog::open(log_path).await?);
    let child = platform::OwnedChild::spawn_with_environment(spec, environment)?;
    run_logged_child(child, cancel, events, &log, deadline, pause, None).await
}

async fn run_logged_child(
    mut child: platform::OwnedChild,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log: &Mutex<RotatingLog>,
    deadline: RunDeadline<'_>,
    pause: Option<&PauseControl>,
    terminal: Option<&mpsc::Sender<Vec<u8>>>,
) -> Result<ProcessResult, SupervisorError> {
    let pause = pause.cloned().or_else(pause::scoped_pause);
    let attached = match pause
        .as_ref()
        .map(|control| control.attach(&child))
        .transpose()
    {
        Ok(attached) => attached,
        Err(error) => {
            child
                .terminate_and_wait()
                .await
                .map_err(SupervisorError::Cleanup)?;
            return Err(error.into());
        }
    };
    let (stdout, stderr) = child.take_output_pipes();
    let activity = matches!(deadline, RunDeadline::Inactivity { .. }).then(|| watch::channel(()).0);
    let execution = async {
        let (_, _, status) = tokio::try_join!(
            async {
                if let Some(stdout) = stdout {
                    drain_stage_every(
                        stdout,
                        false,
                        None,
                        &events,
                        log,
                        REDRAW_LOG_INTERVAL,
                        DrainExtras {
                            terminal: None,
                            activity: activity.as_ref(),
                        },
                    )
                    .await?;
                }
                Ok(())
            },
            drain_stage_every(
                stderr,
                true,
                None,
                &events,
                log,
                REDRAW_LOG_INTERVAL,
                DrainExtras {
                    terminal,
                    activity: activity.as_ref()
                },
            ),
            async {
                child
                    .wait_and_terminate_descendants()
                    .await
                    .map_err(SupervisorError::Io)
            },
        )?;
        Ok(ProcessResult { status })
    };
    let result = tokio::select! {
        biased;
        _ = cancelled(cancel) => Err(SupervisorError::Cancelled),
        _ = logged_timeout(deadline, pause.as_ref(), activity.as_ref()) => Err(match deadline {
            RunDeadline::Runtime(_) => SupervisorError::Timeout,
            RunDeadline::Inactivity { .. } => SupervisorError::InactivityTimeout,
        }),
        result = execution => result,
    };
    // Explicit cleanup also reaps the leader. The guard handles future abortion.
    child
        .terminate_and_wait()
        .await
        .map_err(SupervisorError::Cleanup)?;
    log.lock().await.flush().await?;
    drop(attached);
    result
}

async fn logged_timeout(
    deadline: RunDeadline<'_>,
    pause: Option<&PauseControl>,
    activity: Option<&watch::Sender<()>>,
) {
    async fn active_timeout(limit: Duration, pause: Option<&PauseControl>) {
        if let Some(control) = pause {
            control.timeout(limit).await;
        } else {
            tokio::time::sleep(limit).await;
        }
    }

    match deadline {
        RunDeadline::Runtime(limit) => active_timeout(limit, pause).await,
        RunDeadline::Inactivity {
            limit,
            progress_paths,
            chunk_directory,
            chunk_restarts,
        } => {
            let activity = activity.expect("inactivity watchdog tracks progress");
            let mut changes = activity.subscribe();
            let deadline = async {
                loop {
                    tokio::select! {
                        biased;
                        _ = changes.changed() => {},
                        _ = active_timeout(limit, pause) => break,
                    }
                }
            };
            tokio::select! {
                _ = watch_progress_files(progress_paths, limit, activity) => {},
                _ = watch_chunk_growth(chunk_directory, chunk_restarts, limit, activity) => {},
                _ = deadline => {},
            }
        }
    }
}

async fn watch_progress_files(paths: [&Path; 2], limit: Duration, activity: &watch::Sender<()>) {
    async fn stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
        let metadata = tokio::fs::symlink_metadata(path).await.ok()?;
        if !metadata.is_file() {
            return None;
        }
        Some((metadata.len(), metadata.modified().ok()?))
    }

    let (mut first, mut second) = tokio::join!(stamp(paths[0]), stamp(paths[1]));
    let period = (limit / 4).clamp(Duration::from_millis(1), Duration::from_secs(5));
    let mut poll = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        poll.tick().await;
        let (next_first, next_second) = tokio::join!(stamp(paths[0]), stamp(paths[1]));
        // Missing files or transient read errors are not progress. Remember
        // the last successful observation so they cannot reset the watchdog.
        let changed = (next_first.is_some() && next_first != first)
            || (next_second.is_some() && next_second != second);
        first = next_first.or(first);
        second = next_second.or(second);
        if changed {
            activity.send_replace(());
        }
    }
}

async fn chunk_output_sizes(directory: &Path) -> Vec<(OsString, u64)> {
    fn ordinary(metadata: &fs::Metadata) -> bool {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
        }
        #[cfg(not(windows))]
        {
            !metadata.file_type().is_symlink()
        }
    }

    let directory = directory.to_owned();
    tokio::task::spawn_blocking(move || {
        if !fs::symlink_metadata(&directory).is_ok_and(|m| m.is_dir() && ordinary(&m))
            || !directory.parent().is_some_and(|parent| {
                fs::symlink_metadata(parent).is_ok_and(|m| m.is_dir() && ordinary(&m))
            })
        {
            return Vec::new();
        }
        let Ok(entries) = fs::read_dir(directory) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let (index, _) = name.to_str()?.split_once('.')?;
                if index.len() < 5 || !index.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let metadata = entry.metadata().ok()?;
                (metadata.is_file() && ordinary(&metadata)).then_some((name, metadata.len()))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

async fn watch_chunk_growth(
    directory: &Path,
    restarts: u32,
    limit: Duration,
    activity: &watch::Sender<()>,
) {
    let mut updates = activity.subscribe();
    // Each file carries its high-water mark and how often that mark may still
    // be rebased. A file left by an earlier run gets one more rebase, because
    // a resumed encoder may truncate it before its first retry.
    let mut sizes: HashMap<_, _> = chunk_output_sizes(directory)
        .await
        .into_iter()
        .map(|(name, bytes)| (name, (bytes, restarts.saturating_add(1))))
        .collect();
    let period = (limit / 4).clamp(Duration::from_millis(1), Duration::from_secs(60));
    loop {
        // Walk the output directory only after a quiet interval. Most jobs
        // report frame or checkpoint progress more often than this.
        tokio::select! {
            biased;
            _ = updates.changed() => continue,
            _ = tokio::time::sleep(period) => {}
        }
        let mut grew = false;
        for (name, bytes) in chunk_output_sizes(directory).await {
            let (peak, rebases) = sizes.entry(name).or_insert((0, restarts));
            if bytes > *peak {
                *peak = bytes;
                grew = true;
            } else if bytes < *peak && *rebases > 0 {
                // AV1AN starts a failed chunk again from an empty file, and a
                // resumed encoder may truncate an old incomplete one. Follow
                // the new file from its current size, or a long chunk would
                // look idle until it passed its earlier size. The rebase
                // itself is not progress and the count is bounded, so a file
                // that keeps being truncated or recreated cannot mask a stall.
                *peak = bytes;
                *rebases -= 1;
            }
        }
        if grew {
            activity.send_replace(());
            updates.borrow_and_update();
        }
    }
}

/// Lossless, bounded capture for short machine-readable commands (e.g. FFprobe).
/// `max_bytes` applies independently to stdout and stderr.
pub async fn run_capture(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    max_bytes: usize,
    time_limit: Duration,
) -> Result<CapturedOutput, SupervisorError> {
    run_capture_with_path(spec, cancel, max_bytes, time_limit, None).await
}

/// Bounded capture with the same child-only PATH override as `run_with_path`.
pub async fn run_capture_with_path(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    max_bytes: usize,
    time_limit: Duration,
    path: Option<&OsStr>,
) -> Result<CapturedOutput, SupervisorError> {
    let environment = path.map(ChildEnvironment::with_path);
    run_capture_with_environment(spec, cancel, max_bytes, time_limit, environment.as_ref()).await
}

/// Capture output with child-only environment changes.
pub async fn run_capture_with_environment(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    max_bytes: usize,
    time_limit: Duration,
    environment: Option<&ChildEnvironment>,
) -> Result<CapturedOutput, SupervisorError> {
    console::scope(console::explicit_profile(), &[spec], cancel, |cancel| {
        run_capture_inner(spec, cancel, max_bytes, time_limit, environment)
    })
    .await
}
async fn run_capture_inner(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    max_bytes: usize,
    time_limit: Duration,
    environment: Option<&ChildEnvironment>,
) -> Result<CapturedOutput, SupervisorError> {
    if *cancel.borrow() {
        return Err(SupervisorError::Cancelled);
    }
    let mut child = platform::OwnedChild::spawn_with_environment(spec, environment)?;
    let pause = pause::scoped_pause();
    let _attached = pause
        .as_ref()
        .map(|control| control.attach(&child))
        .transpose()?;
    let (stdout, stderr) = child.take_pipes();
    let execution = async {
        let (stdout, stderr, status) = tokio::try_join!(
            capture(stdout, max_bytes, false),
            capture(stderr, max_bytes, true),
            async {
                child
                    .wait_and_terminate_descendants()
                    .await
                    .map_err(SupervisorError::Io)
            },
        )?;
        Ok(CapturedOutput {
            status,
            stdout,
            stderr,
        })
    };
    let result = tokio::select! {
        biased;
        _ = cancelled(cancel) => Err(SupervisorError::Cancelled),
        _ = async { if let Some(control) = pause.as_ref() { control.timeout(time_limit).await; } else { tokio::time::sleep(time_limit).await; } } => Err(SupervisorError::Timeout),
        result = execution => result,
    };
    child
        .terminate_and_wait()
        .await
        .map_err(SupervisorError::Cleanup)?;
    result
}

/// Lossless stdout streaming for large machine-readable commands. The parser
/// runs on a blocking worker and receives backpressure through a two-chunk
/// channel; stdout is never retained in full or sent to diagnostic events.
/// Stderr is drained concurrently, retaining only its newest `stderr_max_bytes`.
///
/// The callback must consume and validate the complete stream, including any
/// trailing syntax, and perform bounded work between reads. Returning an error
/// stops the process tree. Cancellation and timeout close the reader, stop and
/// await the tree, and await the parser worker before returning. An aborted
/// future also closes the reader and terminates the tree through its guard.
/// A successful parse still requires checking the returned process exit status.
pub async fn run_streaming_stdout<T, F>(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    stderr_max_bytes: usize,
    time_limit: Duration,
    parse: F,
) -> Result<StreamedOutput<T>, SupervisorError>
where
    T: Send + 'static,
    F: FnOnce(&mut dyn io::Read) -> Result<T, String> + Send + 'static,
{
    console::scope(console::explicit_profile(), &[spec], cancel, |cancel| {
        run_streaming_stdout_inner(spec, cancel, stderr_max_bytes, time_limit, None, parse)
    })
    .await
}

pub async fn run_streaming_stdout_with_stderr_events<T, F>(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    stderr_max_bytes: usize,
    time_limit: Duration,
    events: mpsc::Sender<ProcessEvent>,
    parse: F,
) -> Result<StreamedOutput<T>, SupervisorError>
where
    T: Send + 'static,
    F: FnOnce(&mut dyn io::Read) -> Result<T, String> + Send + 'static,
{
    console::scope(console::explicit_profile(), &[spec], cancel, |cancel| {
        run_streaming_stdout_inner(
            spec,
            cancel,
            stderr_max_bytes,
            time_limit,
            Some(events),
            parse,
        )
    })
    .await
}

async fn run_streaming_stdout_inner<T, F>(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    stderr_max_bytes: usize,
    time_limit: Duration,
    stderr_events: Option<mpsc::Sender<ProcessEvent>>,
    parse: F,
) -> Result<StreamedOutput<T>, SupervisorError>
where
    T: Send + 'static,
    F: FnOnce(&mut dyn io::Read) -> Result<T, String> + Send + 'static,
{
    if *cancel.borrow() {
        return Err(SupervisorError::Cancelled);
    }
    let mut child = platform::OwnedChild::spawn(spec)?;
    let pause = pause::scoped_pause();
    let _attached = pause
        .as_ref()
        .map(|control| control.attach(&child))
        .transpose()?;
    let (stdout, stderr) = child.take_pipes();
    let (sender, receiver) = mpsc::channel(2);
    let mut parser = tokio::task::spawn_blocking(move || {
        let mut reader = StreamingReader {
            receiver,
            current: io::Cursor::new(Vec::new()),
        };
        let value = parse(&mut reader).map_err(SupervisorError::OutputParse)?;
        // A callback must not silently accept an unread suffix. This also waits
        // for genuine pipe EOF when a parser returns immediately after a value.
        if io::Read::read(&mut reader, &mut [0])? != 0 {
            return Err(SupervisorError::OutputParse(
                "The parser returned before consuming all stdout bytes.".into(),
            ));
        }
        Ok(value)
    });
    let mut parser_done = false;
    // Own the sender inside this future so cancellation immediately closes the
    // blocking reader. No blocking worker owns the process or its pipe handles.
    let execution = async {
        let ((), stderr, status, value) = tokio::try_join!(
            forward_stdout(stdout, sender),
            capture_tail_with_events(stderr, stderr_max_bytes, stderr_events),
            async {
                child
                    .wait_and_terminate_descendants()
                    .await
                    .map_err(SupervisorError::Io)
            },
            async {
                let value = (&mut parser).await;
                parser_done = true;
                value.map_err(|error| SupervisorError::Io(io::Error::other(error)))?
            },
        )?;
        Ok(StreamedOutput {
            status,
            value,
            stderr,
        })
    };
    let result = tokio::select! {
        biased;
        _ = cancelled(cancel) => Err(SupervisorError::Cancelled),
        _ = async { if let Some(control) = pause.as_ref() { control.timeout(time_limit).await; } else { tokio::time::sleep(time_limit).await; } } => Err(SupervisorError::Timeout),
        result = execution => result,
    };
    // Even a failed tree cleanup must not leave the parser worker blocked. The
    // sender has already closed, so it can finish without waiting on pipe EOF.
    let cleanup = child.terminate_and_wait().await;
    if !parser_done {
        let _ = parser.await;
    }
    cleanup.map_err(SupervisorError::Cleanup)?;
    result
}

struct StreamingReader {
    receiver: mpsc::Receiver<Vec<u8>>,
    current: io::Cursor<Vec<u8>>,
}

impl io::Read for StreamingReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let count = io::Read::read(&mut self.current, output)?;
            if count != 0 {
                return Ok(count);
            }
            match self.receiver.blocking_recv() {
                Some(bytes) => self.current = io::Cursor::new(bytes),
                None => return Ok(0),
            }
        }
    }
}

async fn forward_stdout(
    mut stdout: impl AsyncRead + Unpin,
    sender: mpsc::Sender<Vec<u8>>,
) -> Result<(), SupervisorError> {
    loop {
        let mut bytes = vec![0; PIPE_BYTES];
        let count = read_pipe(&mut stdout, &mut bytes).await?;
        if count == 0 {
            return Ok(());
        }
        bytes.truncate(count);
        console::emit(0, &bytes);
        if sender.send(bytes).await.is_err() {
            // The parser finished or panicked; its task carries the actual error.
            return Ok(());
        }
    }
}

#[cfg(test)]
async fn capture_tail(
    stream: impl AsyncRead + Unpin,
    max_bytes: usize,
) -> Result<Vec<u8>, SupervisorError> {
    capture_tail_with_events(stream, max_bytes, None).await
}

async fn capture_tail_with_events(
    mut stream: impl AsyncRead + Unpin,
    max_bytes: usize,
    events: Option<mpsc::Sender<ProcessEvent>>,
) -> Result<Vec<u8>, SupervisorError> {
    let mut result = Vec::with_capacity(max_bytes.min(RECORD_BYTES));
    let mut buffer = [0; RECORD_BYTES];
    let mut pending = Vec::new();
    loop {
        let count = read_pipe(&mut stream, &mut buffer).await?;
        if count == 0 {
            if let Some(tx) = &events
                && !pending.is_empty()
            {
                let line = String::from_utf8_lossy(&pending).trim().to_owned();
                if !line.is_empty() {
                    let _ = tx.try_send(ProcessEvent::Stderr(line));
                }
            }
            return Ok(result);
        }
        console::emit(1, &buffer[..count]);
        if let Some(tx) = &events {
            for &byte in &buffer[..count] {
                if byte == b'\n' || byte == b'\r' {
                    if !pending.is_empty() {
                        let line = String::from_utf8_lossy(&pending).trim().to_owned();
                        pending.clear();
                        if !line.is_empty() {
                            let _ = tx.try_send(ProcessEvent::Stderr(line));
                        }
                    }
                } else if pending.len() < 4096 {
                    pending.push(byte);
                }
            }
        }
        if count >= max_bytes {
            result.clear();
            result.extend_from_slice(&buffer[count - max_bytes..count]);
        } else {
            let remove = result.len().saturating_add(count).saturating_sub(max_bytes);
            result.drain(..remove);
            result.extend_from_slice(&buffer[..count]);
        }
    }
}

/// Streams producer stdout directly into consumer stdin through a fixed-size
/// buffer. Raw bytes never enter diagnostic logs or UI events. Both complete
/// stage statuses are mandatory; output validation remains the caller's job.
pub async fn run_pipeline(
    producer_spec: &CommandSpec,
    consumer_spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
) -> Result<PipelineResult, SupervisorError> {
    run_pipeline_inner(
        producer_spec,
        consumer_spec,
        cancel,
        events,
        log_path,
        time_limit,
        None,
        None,
    )
    .await
}

/// A source frameserver pipeline whose final consumer writes an owned path.
#[allow(clippy::too_many_arguments)]
pub async fn run_pipeline_with_producer_environment(
    producer_spec: &CommandSpec,
    consumer_spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
    producer_environment: &ChildEnvironment,
) -> Result<PipelineResult, SupervisorError> {
    run_pipeline_inner(
        producer_spec,
        consumer_spec,
        cancel,
        events,
        log_path,
        time_limit,
        None,
        Some(producer_environment),
    )
    .await
}

/// As `run_pipeline`, with consumer stdout inheriting an already-owned seekable
/// file. Encoders can rewrite container headers through stdout. Neither raw
/// stream is logged. The child trees close their file handles before normal
/// completion/cancellation returns; the caller must flush and validate the file.
/// A `Cleanup` error means process termination could not be confirmed; callers
/// must preserve the temporary artifact rather than assume all handles closed.
pub async fn run_pipeline_to_file(
    producer_spec: &CommandSpec,
    consumer_spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
    output: std::fs::File,
) -> Result<PipelineResult, SupervisorError> {
    run_pipeline_inner(
        producer_spec,
        consumer_spec,
        cancel,
        events,
        log_path,
        time_limit,
        Some(output),
        None,
    )
    .await
}

/// File-output pipeline with environment changes scoped to the producer only.
/// This is used by frameservers whose DLL/plugin lookup must not leak into the
/// encoder or the parent process.
#[allow(clippy::too_many_arguments)]
pub async fn run_pipeline_to_file_with_producer_environment(
    producer_spec: &CommandSpec,
    consumer_spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
    output: std::fs::File,
    producer_environment: &ChildEnvironment,
) -> Result<PipelineResult, SupervisorError> {
    run_pipeline_inner(
        producer_spec,
        consumer_spec,
        cancel,
        events,
        log_path,
        time_limit,
        Some(output),
        Some(producer_environment),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_pipeline_inner(
    producer_spec: &CommandSpec,
    consumer_spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
    output: Option<std::fs::File>,
    producer_environment: Option<&ChildEnvironment>,
) -> Result<PipelineResult, SupervisorError> {
    console::scope(
        console::logged_profile(&[producer_spec, consumer_spec]),
        &[producer_spec, consumer_spec],
        cancel,
        |cancel| {
            run_pipeline_execute(
                producer_spec,
                consumer_spec,
                cancel,
                events,
                log_path,
                time_limit,
                output,
                producer_environment,
            )
        },
    )
    .await
}
#[allow(clippy::too_many_arguments)]
async fn run_pipeline_execute(
    producer_spec: &CommandSpec,
    consumer_spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
    output: Option<std::fs::File>,
    producer_environment: Option<&ChildEnvironment>,
) -> Result<PipelineResult, SupervisorError> {
    if *cancel.borrow() {
        return Err(SupervisorError::Cancelled);
    }
    let log = Mutex::new(RotatingLog::open(log_path).await?);
    // Start the reader first. Each stage has its own owned tree; failure to
    // launch either stage tears down everything already started.
    let mut consumer = if let Some(file) = output {
        platform::OwnedChild::spawn_with_stdin_to_file(consumer_spec, file)?
    } else {
        platform::OwnedChild::spawn_with_stdin(consumer_spec)?
    };
    let mut producer =
        match platform::OwnedChild::spawn_with_environment(producer_spec, producer_environment) {
            Ok(child) => child,
            Err(error) => {
                consumer
                    .terminate_and_wait()
                    .await
                    .map_err(SupervisorError::Cleanup)?;
                return Err(SupervisorError::Io(error));
            }
        };
    let pause = pause::scoped_pause();
    let _producer_pause = pause
        .as_ref()
        .map(|control| control.attach(&producer))
        .transpose()?;
    let _consumer_pause = pause
        .as_ref()
        .map(|control| control.attach(&consumer))
        .transpose()?;
    let (producer_stdout, producer_stderr) = producer.take_pipes();
    let consumer_stdin = consumer.take_stdin();
    let (consumer_stdout, consumer_stderr) = consumer.take_output_pipes();
    let delivered = AtomicBool::new(false);
    // File-output mode has no stdout pipe: the child owns a seekable duplicate,
    // and process-tree cleanup closes every inherited copy before returning.
    let consumer_output = async {
        if let Some(consumer_stdout) = consumer_stdout {
            drain_stage(consumer_stdout, false, Some("consumer"), &events, &log).await
        } else {
            Ok::<_, SupervisorError>(())
        }
    };
    tokio::pin!(consumer_output);
    let mut consumer_output_done = false;
    let mut diagnostics = Box::pin(async {
        tokio::try_join!(
            drain_stage(producer_stderr, true, Some("producer"), &events, &log),
            drain_stage(consumer_stderr, true, Some("consumer"), &events, &log),
        )?;
        Ok::<_, SupervisorError>(())
    });
    let mut diagnostics_done = false;
    let execution = async {
        let producer_wait = producer.wait_and_terminate_descendants();
        let consumer_wait = consumer.wait_and_terminate_descendants();
        let transfer = transfer_binary(producer_stdout, consumer_stdin, &delivered);
        tokio::pin!(producer_wait, consumer_wait, transfer);
        let (mut producer_status, mut consumer_status, mut transferred) = (None, None, None);
        loop {
            tokio::select! {
                biased;
                status = &mut producer_wait, if producer_status.is_none() => {
                    let status = status?;
                    if !status.success() {
                        return Err(SupervisorError::StageFailed { stage: PipelineStage::Producer, status });
                    }
                    producer_status = Some(status);
                }
                status = &mut consumer_wait, if consumer_status.is_none() => {
                    let status = status?;
                    if !status.success() {
                        return Err(SupervisorError::StageFailed { stage: PipelineStage::Consumer, status });
                    }
                    if !delivered.load(Ordering::Acquire) { return Err(SupervisorError::EarlyConsumerExit); }
                    consumer_status = Some(status);
                }
                result = &mut transfer, if transferred.is_none() => {
                    transferred = Some(result?);
                }
                result = &mut diagnostics, if !diagnostics_done => {
                    diagnostics_done = true;
                    result?;
                }
                result = &mut consumer_output, if !consumer_output_done => {
                    consumer_output_done = true;
                    result?;
                }
            }
            if let (
                Some(producer_status),
                Some(consumer_status),
                Some((bytes_transferred, producer_prefix)),
                true,
                true,
            ) = (
                producer_status,
                consumer_status,
                transferred.as_mut(),
                diagnostics_done,
                consumer_output_done,
            ) {
                return Ok(PipelineResult {
                    producer_status,
                    consumer_status,
                    bytes_transferred: *bytes_transferred,
                    producer_prefix: std::mem::take(producer_prefix),
                });
            }
        }
    };
    let result = tokio::select! {
        biased;
        _ = cancelled(cancel) => Err(SupervisorError::Cancelled),
        _ = async { if let Some(control) = pause.as_ref() { control.timeout(time_limit).await; } else { tokio::time::sleep(time_limit).await; } } => Err(SupervisorError::Timeout),
        result = execution => result,
    };
    // Await both cleanups even if one reports failure. Killing both trees also
    // releases any blocking anonymous-pipe IO still finishing on Windows.
    let (producer_cleanup, consumer_cleanup) =
        tokio::join!(producer.terminate_and_wait(), consumer.terminate_and_wait(),);
    producer_cleanup.map_err(SupervisorError::Cleanup)?;
    consumer_cleanup.map_err(SupervisorError::Cleanup)?;
    if !diagnostics_done
        && result
            .as_ref()
            .is_err_and(|error| error.is_pipeline_stage_failure())
    {
        // A failed stage can exit before its stderr was polled. Read its
        // remaining diagnostics after both process trees have closed the pipes.
        let _ = tokio::time::timeout(Duration::from_secs(1), diagnostics.as_mut()).await;
    }
    // Cancellation can interrupt an async log write while the drain still
    // holds the log mutex. Release it before draining stdout or flushing.
    drop(diagnostics);
    let output_result = if consumer_output_done {
        Ok(())
    } else {
        consumer_output.await
    };
    output_result?;
    log.lock().await.flush().await?;
    result
}

async fn transfer_binary(
    mut producer: impl AsyncRead + Unpin,
    mut consumer: impl AsyncWrite + Unpin,
    delivered: &AtomicBool,
) -> io::Result<(u64, Vec<u8>)> {
    // Keep large buffers off the async state-machine stack. Nested job/pipeline
    // futures otherwise overflow Windows' default thread stack when moved.
    let mut buffer = vec![0_u8; PIPE_BYTES];
    let mut total = 0_u64;
    let mut prefix = Vec::new();
    const PREFIX_LIMIT: usize = 512;
    loop {
        let count = read_pipe(&mut producer, &mut buffer).await?;
        if count == 0 {
            break;
        }
        let capture = count.min(PREFIX_LIMIT - prefix.len());
        prefix.extend_from_slice(&buffer[..capture]);
        consumer.write_all(&buffer[..count]).await?;
        total += count as u64;
    }
    consumer.flush().await?;
    consumer.shutdown().await?;
    drop(consumer); // EOF must reach the encoder before its success is accepted.
    delivered.store(true, Ordering::Release);
    Ok((total, prefix))
}

async fn read_pipe(stream: &mut (impl AsyncRead + Unpin), buffer: &mut [u8]) -> io::Result<usize> {
    match stream.read(buffer).await {
        // Anonymous Windows pipes report a broken pipe when the writer closes.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(0),
        other => other,
    }
}

async fn capture(
    mut stream: impl AsyncRead + Unpin,
    max_bytes: usize,
    stderr: bool,
) -> Result<Vec<u8>, SupervisorError> {
    let mut result = Vec::with_capacity(max_bytes.min(RECORD_BYTES));
    // Capability discovery nests several capture futures in one join. Keep the
    // bounded read buffers off Windows' small async worker/test thread stacks.
    let mut buffer = vec![0; RECORD_BYTES];
    loop {
        let count = read_pipe(&mut stream, &mut buffer).await?;
        if count == 0 {
            return Ok(result);
        }
        console::emit(u8::from(stderr), &buffer[..count]);
        if result.len().saturating_add(count) > max_bytes {
            return Err(SupervisorError::OutputLimit);
        }
        let _ = CAPTURE_OBSERVER.try_with(|observer| observer(stderr, &buffer[..count]));
        result.extend_from_slice(&buffer[..count]);
    }
}

/// Progress meters (SVT-AV1, x264, FFmpeg stats) redraw one line with a bare
/// carriage return several times a second. Every redraw still reaches the
/// job's events, but the job log keeps at most one per interval plus the
/// latest redraw before other output, so a long encode cannot fill the log
/// and rotate its command line and warnings away.
const REDRAW_LOG_INTERVAL: Duration = Duration::from_secs(10);

async fn drain_stage(
    stream: impl AsyncRead + Unpin,
    stderr: bool,
    stage: Option<&str>,
    events: &mpsc::Sender<ProcessEvent>,
    log: &Mutex<RotatingLog>,
) -> Result<(), SupervisorError> {
    drain_stage_every(
        stream,
        stderr,
        stage,
        events,
        log,
        REDRAW_LOG_INTERVAL,
        DrainExtras::default(),
    )
    .await
}

#[derive(Default)]
struct DrainExtras<'a> {
    terminal: Option<&'a mpsc::Sender<Vec<u8>>>,
    activity: Option<&'a watch::Sender<()>>,
}

async fn drain_stage_every(
    mut stream: impl AsyncRead + Unpin,
    stderr: bool,
    stage: Option<&str>,
    events: &mpsc::Sender<ProcessEvent>,
    log: &Mutex<RotatingLog>,
    interval: Duration,
    extras: DrainExtras<'_>,
) -> Result<(), SupervisorError> {
    let output = StageOutput {
        stderr,
        stage,
        events,
        log,
    };
    let mut redraws = Redraws {
        interval,
        logged: None,
        held: None,
    };
    let mut buffer = vec![0; RECORD_BYTES];
    let mut pending = Vec::with_capacity(RECORD_BYTES);
    // A record ended by '\r' is a redraw unless '\n' follows (a CRLF line).
    // Its event is sent at once; only the log waits for the next byte.
    let mut carriage: Option<(Vec<u8>, bool)> = None;
    let mut terminal = ansi::Filter::default();
    let mut progress = av1an_activity::Progress::default();
    let mut observe_progress = |record: &[u8]| {
        if let Some(activity) = extras.activity
            && progress.advances(record)
        {
            activity.send_replace(());
        }
    };
    loop {
        let count = read_pipe(&mut stream, &mut buffer).await?;
        if count == 0 {
            if let Some((record, logged)) = carriage.take()
                && !logged
            {
                redraws.held = Some(record);
            }
            redraws.release(None, &output).await?;
            if !pending.is_empty() {
                output.event(&pending);
                output.log(&pending).await?;
            }
            return Ok(());
        }
        if let Some(terminal) = extras.terminal
            && let Ok(slot) = terminal.try_reserve()
        {
            slot.send(buffer[..count].to_vec());
        }
        console::emit(
            match stage {
                Some("producer") => 2,
                Some("consumer") => 3,
                _ => u8::from(stderr),
            },
            &buffer[..count],
        );
        for &raw in &buffer[..count] {
            let byte = if extras.terminal.is_some() {
                terminal.push_console(raw)
            } else {
                terminal.push(raw)
            };
            let Some(byte) = byte else {
                continue;
            };
            if byte == b'\n' {
                match carriage.take() {
                    Some((record, false)) => {
                        redraws.release(Some(&record), &output).await?;
                        output.log(&record).await?;
                    }
                    Some((_, true)) => {}
                    None if !pending.is_empty() => {
                        observe_progress(&pending);
                        output.event(&pending);
                        redraws.release(Some(&pending), &output).await?;
                        output.log(&pending).await?;
                        pending.clear();
                    }
                    None => {}
                }
                continue;
            }
            if let Some((record, logged)) = carriage.take()
                && !logged
            {
                redraws.held = Some(record);
            }
            if byte == b'\r' {
                let cursor_redraw = raw != b'\r';
                if cursor_redraw {
                    pending.truncate(pending.trim_ascii_end().len());
                }
                if !pending.is_empty() {
                    observe_progress(&pending);
                    let logged = redraws.due();
                    if !cursor_redraw || logged {
                        output.event(&pending);
                    }
                    if logged {
                        redraws.release(Some(&pending), &output).await?;
                        output.log(&pending).await?;
                        redraws.logged = Some(tokio::time::Instant::now());
                    }
                    carriage = Some((std::mem::take(&mut pending), logged));
                }
            } else {
                pending.push(byte);
                if pending.len() == RECORD_BYTES {
                    observe_progress(&pending);
                    output.event(&pending);
                    redraws.release(Some(&pending), &output).await?;
                    output.log(&pending).await?;
                    pending.clear();
                }
            }
        }
    }
}

struct StageOutput<'a> {
    stderr: bool,
    stage: Option<&'a str>,
    events: &'a mpsc::Sender<ProcessEvent>,
    log: &'a Mutex<RotatingLog>,
}

impl StageOutput<'_> {
    fn staged(&self, bytes: &[u8]) -> Vec<u8> {
        match self.stage {
            Some(stage) => {
                let mut staged = format!("[{stage}] ").into_bytes();
                staged.extend_from_slice(bytes);
                staged
            }
            None => bytes.to_vec(),
        }
    }

    async fn log(&self, bytes: &[u8]) -> io::Result<()> {
        let mut log = self.log.lock().await;
        #[cfg(test)]
        if self.stderr
            && self.stage.is_some()
            && BLOCK_PIPELINE_LOG_WRITE
                .try_with(|started| started.notify_one())
                .is_ok()
        {
            std::future::pending::<()>().await;
        }
        log.write_record(&self.staged(bytes), self.stderr).await
    }

    fn event(&self, bytes: &[u8]) {
        // Consumer stdout stays parseable; stderr events identify their stage.
        let text = if self.stderr {
            String::from_utf8_lossy(&self.staged(bytes)).into_owned()
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        };
        let _ = self.events.try_send(if self.stderr {
            ProcessEvent::Stderr(text)
        } else {
            ProcessEvent::Stdout(text)
        });
    }
}

struct Redraws {
    interval: Duration,
    logged: Option<tokio::time::Instant>,
    /// The latest redraw not yet written to the log.
    held: Option<Vec<u8>>,
}

impl Redraws {
    fn due(&self) -> bool {
        self.logged
            .is_none_or(|logged| logged.elapsed() >= self.interval)
    }

    /// Writes the held redraw before `next` so the log keeps the last progress
    /// state, unless `next` repeats it.
    async fn release(&mut self, next: Option<&[u8]>, output: &StageOutput<'_>) -> io::Result<()> {
        if let Some(held) = self.held.take()
            && next != Some(held.as_slice())
        {
            output.log(&held).await?;
        }
        Ok(())
    }
}

struct RotatingLog {
    path: PathBuf,
    file: Option<tokio::fs::File>,
    bytes: u64,
}

impl RotatingLog {
    async fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let bytes = file.metadata().await?.len();
        Ok(Self {
            path: path.to_owned(),
            file: Some(file),
            bytes,
        })
    }

    async fn write_record(&mut self, bytes: &[u8], stderr: bool) -> io::Result<()> {
        let prefix: &[u8] = if stderr { b"[stderr] " } else { b"[stdout] " };
        let length = (prefix.len() + bytes.len() + 1) as u64;
        if self.bytes + length > LOG_BYTES {
            self.flush().await?;
            self.file.take();
            let mut archived = self.path.as_os_str().to_owned();
            archived.push(".1");
            let archived = PathBuf::from(archived);
            match tokio::fs::remove_file(&archived).await {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            tokio::fs::rename(&self.path, archived).await?;
            self.file = Some(tokio::fs::File::create(&self.path).await?);
            self.bytes = 0;
        }
        let file = self.file.as_mut().expect("log is open outside rotation");
        file.write_all(prefix).await?;
        file.write_all(bytes).await?;
        file.write_all(b"\n").await?;
        self.bytes += length;
        Ok(())
    }

    async fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "supervisor/tests.rs"]
mod tests;
