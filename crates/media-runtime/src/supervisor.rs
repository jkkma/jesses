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
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
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

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("The job was cancelled.")]
    Cancelled,
    #[error("The tool exceeded its time limit.")]
    Timeout,
    #[error("The tool produced more output than the capture limit allows.")]
    OutputLimit,
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
    mut stream: impl AsyncRead + Unpin,
    stderr: bool,
    events: &mpsc::Sender<ProcessEvent>,
    log: &Mutex<RotatingLog>,
) -> Result<(), SupervisorError> {
    let mut buffer = [0; RECORD_BYTES];
    let mut pending = Vec::with_capacity(RECORD_BYTES);
    loop {
        let count = read_pipe(&mut stream, &mut buffer).await?;
        if count == 0 {
            if !pending.is_empty() {
                record(&pending, stderr, events, log).await?;
            }
            return Ok(());
        }
        for &byte in &buffer[..count] {
            if byte == b'\r' || byte == b'\n' {
                if !pending.is_empty() {
                    record(&pending, stderr, events, log).await?;
                    pending.clear();
                }
            } else {
                pending.push(byte);
                if pending.len() == RECORD_BYTES {
                    record(&pending, stderr, events, log).await?;
                    pending.clear();
                }
            }
        }
    }
}

async fn record(
    bytes: &[u8],
    stderr: bool,
    events: &mpsc::Sender<ProcessEvent>,
    log: &Mutex<RotatingLog>,
) -> io::Result<()> {
    log.lock().await.write_record(bytes, stderr).await?;
    let text = String::from_utf8_lossy(bytes).into_owned();
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
