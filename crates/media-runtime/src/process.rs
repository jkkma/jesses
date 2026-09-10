use std::{
    ffi::OsString,
    io,
    path::Path,
    process::{ExitStatus, Stdio},
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessError {
    #[error("The tool could not be started or read: {0}")]
    Io(#[from] io::Error),
    #[error("The tool exceeded its time limit.")]
    Timeout,
    #[error("The tool produced more output than the inspection limit allows.")]
    OutputLimit,
}

pub(crate) struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

async fn read_bounded(
    mut stream: impl AsyncRead + Unpin,
    max_bytes: usize,
) -> Result<Vec<u8>, ProcessError> {
    let mut output = Vec::with_capacity(max_bytes.min(8192));
    let mut buffer = [0_u8; 8192];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(count) > max_bytes {
            return Err(ProcessError::OutputLimit);
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

/// Launch a known executable directly. Both pipes are drained concurrently and
/// bounded independently. Cancellation drops/kills the direct child; this
/// read-only adapter is deliberately not an encoding/job process supervisor.
pub(crate) async fn run_tool(
    executable: &Path,
    args: &[OsString],
    time_limit: Duration,
    max_bytes: usize,
) -> Result<ProcessOutput, ProcessError> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("stdout was configured as piped");
    let stderr = child.stderr.take().expect("stderr was configured as piped");
    let result = tokio::time::timeout(time_limit, async {
        let (stdout, stderr, status) = tokio::try_join!(
            read_bounded(stdout, max_bytes),
            read_bounded(stderr, max_bytes),
            async { child.wait().await.map_err(ProcessError::Io) },
        )?;
        Ok(ProcessOutput {
            status,
            stdout,
            stderr,
        })
    })
    .await;
    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            Err(ProcessError::Timeout)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounds_output_and_accepts_exact_limit() {
        assert_eq!(read_bounded(&b"abcd"[..], 4).await.unwrap(), b"abcd");
        assert!(matches!(
            read_bounded(&b"abcde"[..], 4).await,
            Err(ProcessError::OutputLimit)
        ));
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg on PATH"]
    async fn terminates_a_real_tool_when_output_or_time_limits_are_exceeded() {
        let ffmpeg = crate::discovery::find_executable(&["ffmpeg"])
            .await
            .expect("FFmpeg discovery must succeed")
            .expect("FFmpeg is required for this integration gate");
        let flood_args: Vec<OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=8000:cl=mono",
            "-t",
            "10",
            "-f",
            "s16le",
            "-",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let result = run_tool(&ffmpeg, &flood_args, Duration::from_secs(5), 1024).await;
        assert!(matches!(result, Err(ProcessError::OutputLimit)));

        let timed_args: Vec<OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=8000:cl=mono",
            "-t",
            "10",
            "-f",
            "null",
            "-",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let start = std::time::Instant::now();
        let result = run_tool(&ffmpeg, &timed_args, Duration::from_millis(100), 1024).await;
        assert!(matches!(result, Err(ProcessError::Timeout)));
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "Timeout cleanup should return promptly"
        );
    }
}
