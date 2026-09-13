use std::{ffi::OsString, io, path::Path, process::ExitStatus, time::Duration};

use crate::supervisor::{self, CommandSpec, SupervisorError};

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

/// Launch a known executable directly. Both pipes are drained concurrently and
/// bounded independently. Inspection uses the same owned process launcher as
/// encoding: on Windows every spawn must explicitly restrict handle inheritance,
/// so a concurrent probe cannot inherit another encoder's protected output.
pub(crate) async fn run_tool(
    executable: &Path,
    args: &[OsString],
    time_limit: Duration,
    max_bytes: usize,
) -> Result<ProcessOutput, ProcessError> {
    run_tool_with_environment(executable, args, time_limit, max_bytes, None).await
}

pub(crate) async fn run_tool_with_environment(
    executable: &Path,
    args: &[OsString],
    time_limit: Duration,
    max_bytes: usize,
    environment: Option<&supervisor::ChildEnvironment>,
) -> Result<ProcessOutput, ProcessError> {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable: executable.to_owned(),
            args: args.to_owned(),
            cwd: None,
        },
        cancel,
        max_bytes,
        time_limit,
        environment,
    )
    .await
    .map_err(|error| match error {
        SupervisorError::Timeout => ProcessError::Timeout,
        SupervisorError::OutputLimit => ProcessError::OutputLimit,
        SupervisorError::Io(error) | SupervisorError::Cleanup(error) => ProcessError::Io(error),
        error => ProcessError::Io(io::Error::other(error)),
    })?;
    Ok(ProcessOutput {
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    #[ignore = "exact subprocess fixture for handle inheritance regression"]
    fn inspection_fixture() {
        let args: Vec<_> = std::env::args_os().collect();
        if !args
            .windows(2)
            .any(|pair| pair[0] == "--exact" && pair[1] == "process::tests::inspection_fixture")
            || !args.iter().any(|arg| arg == "--ignored")
        {
            return;
        }
        let directory = std::path::PathBuf::from(args.last().unwrap());
        std::fs::write(directory.join("inspecting"), b"ready").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !directory.join("release").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "inspection fixture timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        std::process::exit(0);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn inspection_cannot_inherit_a_concurrent_encoders_protected_output() {
        use std::os::windows::{
            fs::OpenOptionsExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        };
        use windows_sys::Win32::{
            Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle},
            System::Threading::GetCurrentProcess,
        };
        let directory = std::env::temp_dir().join(format!(
            "jesses-inspection-handles-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        let output = directory.join("owned.partial.mkv");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(1 | 2)
            .open(&output)
            .unwrap();
        let mut raw = std::ptr::null_mut();
        // Reproduce the exact encoder-spawn window deterministically: its owned
        // stdout duplicate is inheritable until CreateProcessW consumes it.
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    file.as_raw_handle(),
                    GetCurrentProcess(),
                    &mut raw,
                    0,
                    1,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        // SAFETY: DuplicateHandle succeeded and returned a uniquely owned handle.
        let inheritable = unsafe { OwnedHandle::from_raw_handle(raw) };
        let args: Vec<OsString> = [
            "--exact",
            "process::tests::inspection_fixture",
            "--ignored",
            "--nocapture",
        ]
        .into_iter()
        .map(OsString::from)
        .chain([directory.as_os_str().to_owned()])
        .collect();
        let inspection = tokio::spawn(async move {
            run_tool(
                &std::env::current_exe().unwrap(),
                &args,
                Duration::from_secs(15),
                65536,
            )
            .await
        });
        let started = tokio::time::timeout(Duration::from_secs(5), async {
            while !directory.join("inspecting").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        drop(inheritable);
        drop(file);
        // Cleanup must be possible while inspection is still running, rather
        // than relying on eventual inspection exit or a deletion retry.
        let exclusive_delete = std::fs::OpenOptions::new()
            .access_mode(0x0001_0000)
            .share_mode(1 | 2)
            .open(&output);
        std::fs::write(directory.join("release"), b"done").unwrap();
        let inspected = inspection.await.unwrap();
        assert!(
            started.is_ok(),
            "inspection must start before checking inheritance"
        );
        assert!(inspected.unwrap().status.success());
        drop(exclusive_delete.expect("inspection inherited the unrelated protected output handle"));
        std::fs::remove_file(output).unwrap();
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
