//! Owned media-tool process trees with cancellation and bounded diagnostics.
//!
//! Windows uses an atomic job-list spawn and a non-inheritable kill-on-close
//! job. Unix tools run in a new process group; descendants must not deliberately
//! detach from that group. Dropping the run future also terminates the tree.

use std::{
    ffi::OsString,
    io,
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

const RECORD_BYTES: usize = 8192;
const LOG_BYTES: u64 = 4 * 1024 * 1024;
const PIPE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct CommandSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub enum ProcessEvent {
    Stdout(String),
    Stderr(String),
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
pub struct PipelineResult {
    pub producer_status: ExitStatus,
    pub consumer_status: ExitStatus,
    pub bytes_transferred: u64,
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
    #[error("The job was cancelled.")]
    Cancelled,
    #[error("The tool exceeded its time limit.")]
    Timeout,
    #[error("The tool produced more output than the capture limit allows.")]
    OutputLimit,
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

async fn cancelled(mut cancel: watch::Receiver<bool>) {
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
/// use `run_capture` instead.
pub async fn run(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
) -> Result<ProcessResult, SupervisorError> {
    if *cancel.borrow() {
        return Err(SupervisorError::Cancelled);
    }
    let log = Mutex::new(RotatingLog::open(log_path).await?);
    let mut child = platform::OwnedChild::spawn(spec)?;
    let (stdout, stderr) = child.take_pipes();
    let execution = async {
        let (_, _, status) = tokio::try_join!(
            drain(stdout, false, &events, &log),
            drain(stderr, true, &events, &log),
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
        _ = tokio::time::sleep(time_limit) => Err(SupervisorError::Timeout),
        result = execution => result,
    };
    // Explicit cleanup also reaps the leader. The guard handles future abortion.
    child
        .terminate_and_wait()
        .await
        .map_err(SupervisorError::Cleanup)?;
    log.lock().await.flush().await?;
    result
}

/// Lossless, bounded capture for short machine-readable commands (e.g. FFprobe).
/// `max_bytes` applies independently to stdout and stderr.
pub async fn run_capture(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    max_bytes: usize,
    time_limit: Duration,
) -> Result<CapturedOutput, SupervisorError> {
    if *cancel.borrow() {
        return Err(SupervisorError::Cancelled);
    }
    let mut child = platform::OwnedChild::spawn(spec)?;
    let (stdout, stderr) = child.take_pipes();
    let execution = async {
        let (stdout, stderr, status) = tokio::try_join!(
            capture(stdout, max_bytes),
            capture(stderr, max_bytes),
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
        _ = tokio::time::sleep(time_limit) => Err(SupervisorError::Timeout),
        result = execution => result,
    };
    child
        .terminate_and_wait()
        .await
        .map_err(SupervisorError::Cleanup)?;
    result
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
    if *cancel.borrow() {
        return Err(SupervisorError::Cancelled);
    }
    let log = Mutex::new(RotatingLog::open(log_path).await?);
    // Start the reader first. Each stage has its own owned tree; failure to
    // launch either stage tears down everything already started.
    let mut consumer = platform::OwnedChild::spawn_with_stdin(consumer_spec)?;
    let mut producer = match platform::OwnedChild::spawn(producer_spec) {
        Ok(child) => child,
        Err(error) => {
            consumer
                .terminate_and_wait()
                .await
                .map_err(SupervisorError::Cleanup)?;
            return Err(SupervisorError::Io(error));
        }
    };
    let (producer_stdout, producer_stderr) = producer.take_pipes();
    let consumer_stdin = consumer.take_stdin();
    let (consumer_stdout, consumer_stderr) = consumer.take_pipes();
    let delivered = AtomicBool::new(false);
    let execution = async {
        let producer_wait = producer.wait_and_terminate_descendants();
        let consumer_wait = consumer.wait_and_terminate_descendants();
        let transfer = transfer_binary(producer_stdout, consumer_stdin, &delivered);
        let diagnostics = async {
            tokio::try_join!(
                drain_stage(producer_stderr, true, Some("producer"), &events, &log),
                drain_stage(consumer_stdout, false, Some("consumer"), &events, &log),
                drain_stage(consumer_stderr, true, Some("consumer"), &events, &log),
            )?;
            Ok::<_, SupervisorError>(())
        };
        tokio::pin!(producer_wait, consumer_wait, transfer, diagnostics);
        let (mut producer_status, mut consumer_status, mut bytes_transferred) = (None, None, None);
        let mut diagnostics_done = false;
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
                result = &mut transfer, if bytes_transferred.is_none() => {
                    bytes_transferred = Some(result?);
                }
                result = &mut diagnostics, if !diagnostics_done => {
                    result?;
                    diagnostics_done = true;
                }
            }
            if let (Some(producer_status), Some(consumer_status), Some(bytes_transferred), true) = (
                producer_status,
                consumer_status,
                bytes_transferred,
                diagnostics_done,
            ) {
                return Ok(PipelineResult {
                    producer_status,
                    consumer_status,
                    bytes_transferred,
                });
            }
        }
    };
    let result = tokio::select! {
        biased;
        _ = cancelled(cancel) => Err(SupervisorError::Cancelled),
        _ = tokio::time::sleep(time_limit) => Err(SupervisorError::Timeout),
        result = execution => result,
    };
    // Await both cleanups even if one reports failure. Killing both trees also
    // releases any blocking anonymous-pipe IO still finishing on Windows.
    let (producer_cleanup, consumer_cleanup) =
        tokio::join!(producer.terminate_and_wait(), consumer.terminate_and_wait(),);
    producer_cleanup.map_err(SupervisorError::Cleanup)?;
    consumer_cleanup.map_err(SupervisorError::Cleanup)?;
    log.lock().await.flush().await?;
    result
}

async fn transfer_binary(
    mut producer: impl AsyncRead + Unpin,
    mut consumer: impl AsyncWrite + Unpin,
    delivered: &AtomicBool,
) -> io::Result<u64> {
    let mut buffer = [0_u8; PIPE_BYTES];
    let mut total = 0_u64;
    loop {
        let count = read_pipe(&mut producer, &mut buffer).await?;
        if count == 0 {
            break;
        }
        consumer.write_all(&buffer[..count]).await?;
        total += count as u64;
    }
    consumer.flush().await?;
    consumer.shutdown().await?;
    drop(consumer); // EOF must reach the encoder before its success is accepted.
    delivered.store(true, Ordering::Release);
    Ok(total)
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
) -> Result<Vec<u8>, SupervisorError> {
    let mut result = Vec::with_capacity(max_bytes.min(RECORD_BYTES));
    let mut buffer = [0; RECORD_BYTES];
    loop {
        let count = read_pipe(&mut stream, &mut buffer).await?;
        if count == 0 {
            return Ok(result);
        }
        if result.len().saturating_add(count) > max_bytes {
            return Err(SupervisorError::OutputLimit);
        }
        result.extend_from_slice(&buffer[..count]);
    }
}

async fn drain(
    stream: impl AsyncRead + Unpin,
    stderr: bool,
    events: &mpsc::Sender<ProcessEvent>,
    log: &Mutex<RotatingLog>,
) -> Result<(), SupervisorError> {
    drain_stage(stream, stderr, None, events, log).await
}

async fn drain_stage(
    mut stream: impl AsyncRead + Unpin,
    stderr: bool,
    stage: Option<&str>,
    events: &mpsc::Sender<ProcessEvent>,
    log: &Mutex<RotatingLog>,
) -> Result<(), SupervisorError> {
    let mut buffer = [0; RECORD_BYTES];
    let mut pending = Vec::with_capacity(RECORD_BYTES);
    loop {
        let count = read_pipe(&mut stream, &mut buffer).await?;
        if count == 0 {
            if !pending.is_empty() {
                record_stage(&pending, stderr, stage, events, log).await?;
            }
            return Ok(());
        }
        for &byte in &buffer[..count] {
            if byte == b'\r' || byte == b'\n' {
                if !pending.is_empty() {
                    record_stage(&pending, stderr, stage, events, log).await?;
                    pending.clear();
                }
            } else {
                pending.push(byte);
                if pending.len() == RECORD_BYTES {
                    record_stage(&pending, stderr, stage, events, log).await?;
                    pending.clear();
                }
            }
        }
    }
}

async fn record_stage(
    bytes: &[u8],
    stderr: bool,
    stage: Option<&str>,
    events: &mpsc::Sender<ProcessEvent>,
    log: &Mutex<RotatingLog>,
) -> io::Result<()> {
    let staged = stage.map(|stage| {
        let mut staged = format!("[{stage}] ").into_bytes();
        staged.extend_from_slice(bytes);
        staged
    });
    log.lock()
        .await
        .write_record(staged.as_deref().unwrap_or(bytes), stderr)
        .await?;
    // Consumer stdout stays parseable; stderr events identify their stage.
    let event_bytes = if stderr {
        staged.as_deref().unwrap_or(bytes)
    } else {
        bytes
    };
    let text = String::from_utf8_lossy(event_bytes).into_owned();
    let _ = events.try_send(if stderr {
        ProcessEvent::Stderr(text)
    } else {
        ProcessEvent::Stdout(text)
    });
    Ok(())
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
