//! A PowerShell 7 console displays the native AV1AN VT stream. The actual
//! command stays under the media supervisor, with its original literal argv.
use super::*;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, CONSOLE_SCREEN_BUFFER_INFO, ENABLE_PROCESSED_OUTPUT,
    ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetConsoleScreenBufferInfo, STD_HANDLE,
    STD_INPUT_HANDLE, SetConsoleMode,
};

pub(super) const FLAG: &str = "--jesses-av1an-terminal";
const PRESENTER: &str = r#"
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -ne 7) { exit 2 }
$start = [Diagnostics.ProcessStartInfo]::new($env:JESSES_TERMINAL_HOST)
$start.UseShellExecute = $false
$start.ArgumentList.Add('--jesses-av1an-terminal')
$start.ArgumentList.Add($env:JESSES_TERMINAL_TITLE)
$start.ArgumentList.Add($PSVersionTable.PSVersion.ToString())
$relay = [Diagnostics.Process]::Start($start)
$relay.WaitForExit()
exit $relay.ExitCode
"#;

struct Presenter {
    input: Option<tokio::fs::File>,
    ready: tokio::fs::File,
    // Retain the redirected error reader until the viewer exits. Startup
    // failures still have a timeout even if a broken installation floods it.
    _stderr: tokio::fs::File,
    cancel: watch::Sender<bool>,
    done: watch::Receiver<Option<Result<(), String>>>,
}

impl Drop for Presenter {
    fn drop(&mut self) {
        self.cancel.send_replace(true);
    }
}

async fn powershell() -> Result<PathBuf, String> {
    // Standard MSI installs remain discoverable when the desktop application
    // was started with an older PATH. Also support portable copies on PATH.
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        let path = PathBuf::from(program_files).join("PowerShell/7/pwsh.exe");
        if path.is_absolute() && path.is_file() {
            return Ok(path);
        }
    }
    crate::discovery::find_executable(&["pwsh"])
        .await?
        .ok_or_else(|| "PowerShell 7 (pwsh.exe) was not found.".to_owned())
}

impl Presenter {
    fn start(host: &Path, powershell: PathBuf, log: &Path) -> io::Result<Self> {
        let mut registry = active()
            .lock()
            .map_err(|_| io::Error::other("Console registry was poisoned."))?;
        registry.retain(|_, entry| !entry.task.is_finished());
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let title = format!("av1an · PowerShell 7 · {}-{id}", std::process::id());
        let encoded = STANDARD.encode(
            PRESENTER
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let command = CommandSpec {
            executable: powershell,
            args: [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encoded,
            ]
            .map(OsString::from)
            .to_vec(),
            cwd: None,
        };
        let mut child = platform::OwnedChild::spawn_presenter(
            &command,
            &ChildEnvironment {
                variables: vec![
                    ("JESSES_TERMINAL_HOST", Some(host.as_os_str().to_owned())),
                    ("JESSES_TERMINAL_TITLE", Some(title.clone().into())),
                ],
                ..Default::default()
            },
        )?;
        let (ready, stderr) = child.take_pipes();
        let input = child.take_stdin();
        let (cancel, canceled) = watch::channel(false);
        let (done_owner, done) = watch::channel(None);
        let task = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancelled(canceled) => {},
                _ = child.wait_and_terminate_descendants() => {},
            }
            let result = child
                .terminate_and_wait()
                .await
                .map_err(|error| error.to_string());
            done_owner.send_replace(Some(result));
        });
        registry.insert(
            id,
            Active {
                cancel: cancel.clone(),
                task,
                title,
                directory: log.to_owned(),
            },
        );
        Ok(Self {
            input: Some(input),
            ready,
            _stderr: stderr,
            cancel,
            done,
        })
    }

    async fn ready(&mut self) -> io::Result<((i16, i16), String)> {
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        tokio::io::BufReader::new((&mut self.ready).take(128))
            .read_line(&mut line)
            .await?;
        let mut values = line.split_whitespace();
        let size = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "The PowerShell console did not report its terminal size.",
            )
        };
        if values.next() != Some("READY") {
            return Err(size());
        }
        let columns = values
            .next()
            .and_then(|value| value.parse::<i16>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(size)?;
        let rows = values
            .next()
            .and_then(|value| value.parse::<i16>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(size)?;
        let version = values
            .next()
            .filter(|value| value.starts_with("7."))
            .ok_or_else(|| io::Error::other("The console is not running PowerShell 7."))?;
        Ok(((columns, rows), version.to_owned()))
    }

    async fn joined(&mut self) -> io::Result<()> {
        loop {
            if let Some(result) = self.done.borrow().clone() {
                return result.map_err(io::Error::other);
            }
            self.done
                .changed()
                .await
                .map_err(|_| io::Error::other("PowerShell cleanup did not report completion."))?;
        }
    }

    async fn stop(&mut self) -> Result<(), SupervisorError> {
        self.cancel.send_replace(true);
        self.input.take();
        self.joined().await.map_err(SupervisorError::Cleanup)
    }
}

async fn notice(
    log: &Mutex<RotatingLog>,
    events: &mpsc::Sender<ProcessEvent>,
    message: String,
) -> io::Result<()> {
    log.lock()
        .await
        .write_record(message.as_bytes(), true)
        .await?;
    let _ = events.try_send(ProcessEvent::Stderr(message));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(in crate::supervisor) async fn run_av1an_terminal(
    spec: &CommandSpec,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log_path: &Path,
    time_limit: Duration,
    environment: Option<&ChildEnvironment>,
    pause: Option<&PauseControl>,
) -> Result<ProcessResult, SupervisorError> {
    if *cancel.borrow() {
        return Err(SupervisorError::Cancelled);
    }
    let Some(host) = HOST.get() else {
        return run_with_environment_inner(
            spec,
            cancel,
            events,
            log_path,
            time_limit,
            environment,
            pause,
        )
        .await;
    };
    let log = Mutex::new(RotatingLog::open(log_path).await?);
    let startup = async {
        let pwsh = powershell().await.map_err(io::Error::other)?;
        Presenter::start(host, pwsh, log_path)
    };
    let presenter = tokio::select! {
        biased;
        _ = cancelled(cancel.clone()) => return Err(SupervisorError::Cancelled),
        result = startup => result,
    };
    let failure = match presenter {
        Err(error) => error,
        Ok(mut presenter) => {
            let stop = presenter.cancel.subscribe();
            let readiness = tokio::select! {
                biased;
                _ = cancelled(cancel.clone()) => None,
                _ = cancelled(stop) => None,
                result = tokio::time::timeout(Duration::from_secs(10), presenter.ready()) => Some(result),
            };
            let readiness = match readiness {
                None => {
                    presenter.stop().await?;
                    return Err(SupervisorError::Cancelled);
                }
                Some(Ok(result)) => result,
                Some(Err(_)) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The PowerShell 7 console did not become ready.",
                )),
            };
            let child = match readiness {
                Ok((size, version)) => {
                    if let Err(error) = notice(
                        &log,
                        &events,
                        format!("PowerShell {version} opened AV1AN's live progress console."),
                    )
                    .await
                    {
                        presenter.stop().await?;
                        return Err(error.into());
                    }
                    if *cancel.borrow() || *presenter.cancel.borrow() {
                        presenter.stop().await?;
                        return Err(SupervisorError::Cancelled);
                    }
                    // Indicatif hides progress for TERM=dumb even on real
                    // console handles. This child now has a VT terminal.
                    let mut terminal_environment = environment.cloned().unwrap_or_default();
                    terminal_environment
                        .dynamic_variables
                        .push(("TERM".into(), Some("xterm-256color".into())));
                    platform::OwnedChild::spawn_terminal(spec, Some(&terminal_environment), size)
                }
                Err(error) => Err(error),
            };
            match child {
                Ok(child) => {
                    return run_with_presenter(
                        child, presenter, cancel, events, &log, time_limit, pause,
                    )
                    .await;
                }
                Err(error) => {
                    presenter.stop().await?;
                    error
                }
            }
        }
    };
    notice(&log, &events, format!("AV1AN's PowerShell 7 console could not open: {failure} Encoding continues with the job log.")).await?;
    log.lock().await.flush().await?;
    run_with_environment_inner(
        spec,
        cancel,
        events,
        log_path,
        time_limit,
        environment,
        pause,
    )
    .await
}

async fn run_with_presenter(
    child: platform::OwnedChild,
    mut presenter: Presenter,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProcessEvent>,
    log: &Mutex<RotatingLog>,
    time_limit: Duration,
    pause: Option<&PauseControl>,
) -> Result<ProcessResult, SupervisorError> {
    let mut input = presenter.input.take().expect("PowerShell relay pipe");
    let (owner, stop) = watch::channel(false);
    let result = {
        let running = run_logged_child(
            child,
            stop,
            events,
            log,
            time_limit,
            pause,
            Some(&mut input),
        );
        tokio::pin!(running);
        tokio::select! {
            biased;
            _ = cancelled(cancel.clone()) => { owner.send_replace(true); running.await },
            _ = presenter.joined() => { owner.send_replace(true); running.await },
            result = &mut running => result,
        }
    };
    if let Ok(output) = &result {
        let message = format!("\r\nExit status: {}\r\n", output.status);
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            input.write_all(message.as_bytes()).await?;
            input.flush().await
        })
        .await;
        drop(input);
        tokio::select! {
            biased;
            _ = cancelled(cancel) => { presenter.stop().await?; return Err(SupervisorError::Cancelled); },
            joined = tokio::time::timeout(Duration::from_secs(8), presenter.joined()) => {
                match joined {
                    Ok(result) => result.map_err(SupervisorError::Cleanup)?,
                    Err(_) => presenter.stop().await?,
                }
            },
        }
    } else {
        drop(input);
        presenter.stop().await?;
    }
    result
}

pub(super) fn helper(mut args: impl Iterator<Item = OsString>) -> io::Result<i32> {
    let title = args
        .next()
        .ok_or_else(|| io::Error::other("Terminal title is missing."))?;
    let version = args
        .next()
        .ok_or_else(|| io::Error::other("PowerShell version is missing."))?;
    let title = std::os::windows::ffi::OsStrExt::encode_wide(title.as_os_str())
        .chain([0])
        .collect::<Vec<_>>();
    // GUI-subsystem release binaries can start detached. Preserve the original
    // relay and readiness pipes before AttachConsole can replace std handles.
    let inherited_pipe = |stream: STD_HANDLE| -> io::Result<File> {
        let handle = unsafe { GetStdHandle(stream) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::other(
                "The inherited PowerShell relay pipe is missing.",
            ));
        }
        // SAFETY: the handle is borrowed from this process, and only its
        // duplicate is owned by the relay.
        let pipe = unsafe { BorrowedHandle::borrow_raw(handle) }.try_clone_to_owned()?;
        Ok(File::from(pipe))
    };
    let mut input = inherited_pipe(STD_INPUT_HANDLE)?;
    let mut ready = inherited_pipe(STD_OUTPUT_HANDLE)?;
    let open_console = || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("CONOUT$")
    };
    let console = match open_console() {
        Ok(console) => console,
        Err(_) => {
            // SAFETY: attaches this owned helper to its direct PowerShell parent.
            if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
                return Err(io::Error::last_os_error());
            }
            open_console()?
        }
    };
    let handle = console.as_raw_handle();
    let mut mode = 0;
    let mut geometry = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: the console handle and out parameters remain valid for these calls.
    if unsafe {
        GetConsoleMode(handle, &mut mode) == 0
            || SetConsoleMode(
                handle,
                mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            ) == 0
            || GetConsoleScreenBufferInfo(handle, &mut geometry) == 0
            || SetConsoleTitleW(title.as_ptr()) == 0
    } {
        return Err(io::Error::last_os_error());
    }
    let window = geometry.srWindow;
    writeln!(
        ready,
        "READY {} {} {}",
        window.Right - window.Left + 1,
        window.Bottom - window.Top + 1,
        version.to_string_lossy()
    )?;
    ready.flush()?;
    let mut buffer = [0; RECORD_BYTES];
    let mut pending = Vec::new();
    loop {
        let read = input.read(&mut buffer)?;
        pending.extend_from_slice(&buffer[..read]);
        decode_pending(&mut pending, &console, read == 0)?;
        if read == 0 {
            break;
        }
    }
    // Preserve the brief post-exit review window while the job is finalizing.
    std::thread::sleep(Duration::from_secs(5));
    Ok(0)
}
