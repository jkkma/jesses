//! Streaming encoder pipelines with bounded diagnostics and owned process trees.

use std::{
    collections::VecDeque,
    ffi::OsString,
    io,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::watch,
    task::JoinSet,
};

const MAX_LINE_BYTES: usize = 4096;
const MAX_TAIL_BYTES: usize = 16 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(25);

pub(crate) struct CommandSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RunError {
    #[error("The encoding was cancelled.")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

#[derive(Default)]
struct DiagnosticTail {
    lines: VecDeque<String>,
    bytes: usize,
}

impl DiagnosticTail {
    fn push(&mut self, line: String) {
        self.bytes += line.len();
        self.lines.push_back(line);
        while self.bytes > MAX_TAIL_BYTES {
            if let Some(line) = self.lines.pop_front() {
                self.bytes -= line.len();
            }
        }
    }

    fn describe(&self) -> String {
        self.lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

type LineCallback = Arc<dyn Fn(String) + Send + Sync>;

/// CR progress updates and LF diagnostics are both records. A tool writing an
/// unending line cannot make the buffer grow; its excess bytes are discarded.
async fn read_lines(
    mut stream: impl AsyncRead + Unpin,
    tail: Arc<Mutex<DiagnosticTail>>,
    on_line: LineCallback,
) -> io::Result<()> {
    let mut input = [0_u8; 8192];
    let mut line = Vec::with_capacity(MAX_LINE_BYTES);
    let mut truncated = false;
    loop {
        let count = stream.read(&mut input).await?;
        for &byte in &input[..count] {
            if byte == b'\r' || byte == b'\n' {
                emit_line(&mut line, &mut truncated, &tail, &on_line);
            } else if line.len() < MAX_LINE_BYTES {
                line.push(byte);
            } else {
                truncated = true;
            }
        }
        if count == 0 {
            emit_line(&mut line, &mut truncated, &tail, &on_line);
            return Ok(());
        }
    }
}

fn emit_line(
    bytes: &mut Vec<u8>,
    truncated: &mut bool,
    tail: &Mutex<DiagnosticTail>,
    on_line: &LineCallback,
) {
    if bytes.is_empty() && !*truncated {
        return;
    }
    let mut line = String::from_utf8_lossy(bytes).into_owned();
    if *truncated {
        line.push_str(" [line truncated]");
    }
    bytes.clear();
    *truncated = false;
    tail.lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(line.clone());
    on_line(line);
}

struct Pipeline {
    tree: platform::ProcessTree,
    children: Vec<Child>,
    readers: JoinSet<io::Result<()>>,
}

impl Pipeline {
    async fn stop(&mut self) {
        // Kill the tree first: killing only its parent can orphan tools that
        // still hold the output file or an inherited pipe open.
        self.tree.terminate();
        for child in &mut self.children {
            let _ = child.start_kill();
        }
        for child in &mut self.children {
            let _ = child.wait().await;
        }
        self.tree.wait_empty().await;
        while self.readers.join_next().await.is_some() {}
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        // Also runs when the caller aborts/drops the future or unwinds.
        self.tree.terminate();
        for child in &mut self.children {
            let _ = child.start_kill();
        }
        self.readers.abort_all();
    }
}

/// Adjacent stages are connected by an asynchronous bounded copy. The final
/// stdout and every stderr are drained independently, even while a stage waits.
pub(crate) async fn run(
    commands: Vec<CommandSpec>,
    mut cancel: watch::Receiver<bool>,
    on_line: LineCallback,
) -> Result<(), RunError> {
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Err(RunError::Cancelled);
    }
    if commands.is_empty() || commands.len() > 32 {
        return Err(RunError::Failed(
            "An encoder pipeline requires 1–32 stages.".into(),
        ));
    }
    let tree = platform::ProcessTree::new()
        .map_err(|error| RunError::Failed(format!("Cannot create process supervisor: {error}")))?;
    let mut pipeline = Pipeline {
        tree,
        children: Vec::with_capacity(commands.len()),
        readers: JoinSet::new(),
    };
    let tail = Arc::new(Mutex::new(DiagnosticTail::default()));
    let names: Vec<_> = commands
        .iter()
        .map(|c| c.executable.display().to_string())
        .collect();
    let mut setup_error = None;
    for (index, spec) in commands.into_iter().enumerate() {
        if *cancel.borrow() || cancel.has_changed().is_err() {
            setup_error = Some(RunError::Cancelled);
            break;
        }
        let mut command = Command::new(&spec.executable);
        command
            .args(spec.args)
            .stdin(if index == 0 {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        pipeline.tree.configure(&mut command);
        match command.spawn() {
            Ok(child) => {
                pipeline.children.push(child);
                if let Err(error) = pipeline.tree.enroll(pipeline.children.last().unwrap()) {
                    setup_error = Some(RunError::Failed(format!(
                        "Cannot supervise {}: {error}",
                        names[index]
                    )));
                    break;
                }
            }
            Err(error) => {
                setup_error = Some(RunError::Failed(format!(
                    "Cannot start {}: {error}",
                    names[index]
                )));
                break;
            }
        }
    }
    if let Some(error) = setup_error {
        pipeline.stop().await;
        return Err(error);
    }

    for index in 0..pipeline.children.len() {
        let stderr = pipeline.children[index]
            .stderr
            .take()
            .expect("piped stderr");
        pipeline
            .readers
            .spawn(read_lines(stderr, Arc::clone(&tail), Arc::clone(&on_line)));
        let stdout = pipeline.children[index]
            .stdout
            .take()
            .expect("piped stdout");
        if index + 1 == pipeline.children.len() {
            pipeline
                .readers
                .spawn(read_lines(stdout, Arc::clone(&tail), Arc::clone(&on_line)));
        } else {
            let mut stdin = pipeline.children[index + 1]
                .stdin
                .take()
                .expect("piped stdin");
            pipeline.readers.spawn(async move {
                let mut stdout = stdout;
                tokio::io::copy(&mut stdout, &mut stdin).await?;
                stdin.shutdown().await
            });
        }
    }

    let mut exited = vec![false; pipeline.children.len()];
    let mut ticks = tokio::time::interval(POLL_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut all_exited = false;
    let outcome = loop {
        if *cancel.borrow() {
            break Err(RunError::Cancelled);
        }
        if all_exited && pipeline.readers.is_empty() {
            break Ok(());
        }
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break Err(RunError::Cancelled);
                }
            }
            reader = pipeline.readers.join_next(), if !pipeline.readers.is_empty() => {
                match reader {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(error))) => break Err(RunError::Failed(format!("Encoder pipe failed: {error}"))),
                    Some(Err(error)) => break Err(RunError::Failed(format!("Encoder output reader failed: {error}"))),
                    None => {}
                }
            }
            _ = ticks.tick() => {
                let mut failure = None;
                for (index, child) in pipeline.children.iter_mut().enumerate() {
                    if exited[index] { continue; }
                    match platform::poll_status(child) {
                        Ok(Some(status)) => {
                            exited[index] = true;
                            if !status.success() {
                                failure = Some(format!("{} exited with {status}", names[index]));
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            failure = Some(format!("Cannot wait for {}: {error}", names[index]));
                            break;
                        }
                    }
                }
                if let Some(failure) = failure {
                    break Err(RunError::Failed(failure));
                }
                if !all_exited && exited.iter().all(|done| *done) {
                    all_exited = true;
                    // No legitimate pipeline work remains. Terminate any
                    // descendants that outlived their launcher so pipe EOF is
                    // guaranteed before collecting the final diagnostics.
                    pipeline.tree.terminate();
                }
            }
        }
    };
    pipeline.stop().await;
    match outcome {
        Err(RunError::Failed(message)) => {
            let diagnostics = tail.lock().unwrap_or_else(|p| p.into_inner()).describe();
            Err(RunError::Failed(if diagnostics.is_empty() {
                message
            } else {
                format!("{message}\n{diagnostics}")
            }))
        }
        other => other,
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
                QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
            },
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
            },
        },
    };

    pub(super) struct ProcessTree {
        job: OwnedHandle,
    }

    pub(super) fn poll_status(child: &mut Child) -> io::Result<Option<std::process::ExitStatus>> {
        child.try_wait()
    }

    impl ProcessTree {
        pub(super) fn new() -> io::Result<Self> {
            // SAFETY: null attributes/name create a private, non-inheritable
            // Job Object. OwnedHandle closes each successful native handle.
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = unsafe { OwnedHandle::from_raw_handle(raw) };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = unsafe {
                SetInformationJobObject(
                    job.as_raw_handle() as HANDLE,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            };
            if configured == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { job })
        }

        pub(super) fn configure(&self, command: &mut Command) {
            // Enrollment happens before the first instruction executes. This
            // prevents a fast launcher from creating a child outside our job.
            command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        }

        pub(super) fn enroll(&mut self, child: &Child) -> io::Result<()> {
            let handle = child
                .raw_handle()
                .ok_or_else(|| io::Error::other("Missing process handle"))?;
            // SAFETY: the live Child and this ProcessTree own both handles.
            if unsafe {
                AssignProcessToJobObject(self.job.as_raw_handle() as HANDLE, handle as HANDLE)
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let pid = child
                .id()
                .ok_or_else(|| io::Error::other("Missing process identifier"))?;
            // Rust's stable Child API does not expose its primary thread.
            // A suspended new process has one initial thread; locate it using
            // the documented snapshot API, then resume only that owned child.
            let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
            if raw == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let mut found =
                unsafe { Thread32First(snapshot.as_raw_handle() as HANDLE, &mut entry) };
            while found != 0 {
                if entry.th32OwnerProcessID == pid {
                    let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                    if raw.is_null() {
                        return Err(io::Error::last_os_error());
                    }
                    let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
                    if unsafe { ResumeThread(thread.as_raw_handle() as HANDLE) } == u32::MAX {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(());
                }
                found = unsafe { Thread32Next(snapshot.as_raw_handle() as HANDLE, &mut entry) };
            }
            Err(io::Error::other("Cannot find the suspended encoder thread"))
        }

        pub(super) fn terminate(&self) {
            // SAFETY: this handle refers only to this pipeline's private job.
            unsafe {
                TerminateJobObject(self.job.as_raw_handle() as HANDLE, 1);
            }
        }

        pub(super) async fn wait_empty(&mut self) {
            loop {
                let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
                let queried = unsafe {
                    QueryInformationJobObject(
                        self.job.as_raw_handle() as HANDLE,
                        JobObjectBasicAccountingInformation,
                        (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                        std::mem::size_of_val(&info) as u32,
                        std::ptr::null_mut(),
                    )
                };
                if queried == 0 || info.ActiveProcesses == 0 {
                    return;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    pub(super) struct ProcessTree {
        groups: Vec<i32>,
    }

    pub(super) fn poll_status(child: &mut Child) -> io::Result<Option<std::process::ExitStatus>> {
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("Missing process identifier"))?;
        // Retain exited group leaders until all groups have been terminated.
        // Reaping an early pipeline stage here could allow its PID/group ID to
        // be recycled while another stage continues encoding for hours.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { info.si_pid() } == 0 {
            return Ok(None);
        }
        let status = unsafe { info.si_status() };
        let raw = if info.si_code == libc::CLD_EXITED {
            status << 8
        } else if info.si_code == libc::CLD_DUMPED {
            status | 0x80
        } else {
            status
        };
        Ok(Some(std::process::ExitStatus::from_raw(raw)))
    }

    impl ProcessTree {
        pub(super) fn new() -> io::Result<Self> {
            Ok(Self { groups: Vec::new() })
        }

        pub(super) fn configure(&self, command: &mut Command) {
            command.as_std_mut().process_group(0);
        }

        pub(super) fn enroll(&mut self, child: &Child) -> io::Result<()> {
            let pid = child
                .id()
                .ok_or_else(|| io::Error::other("Missing process identifier"))?;
            self.groups.push(pid as i32);
            Ok(())
        }

        pub(super) fn terminate(&self) {
            for &pid in &self.groups {
                // SAFETY: each group was created for one of our direct children.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }

        pub(super) async fn wait_empty(&mut self) {
            // Direct children are reaped above. Their killed descendants are
            // reaped by their parent/init; waiting for a group to disappear
            // would hang on zombies we cannot reap.
            self.groups.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(name: &str) -> CommandSpec {
        CommandSpec {
            executable: std::env::current_exe().unwrap(),
            args: [
                "--ignored",
                "--exact",
                &format!("supervisor::tests::{name}"),
                "--nocapture",
            ]
            .map(OsString::from)
            .into(),
        }
    }

    fn is_fixture_invocation(name: &str) -> bool {
        let args: Vec<_> = std::env::args().collect();
        args.iter().any(|arg| arg == "--exact")
            && args
                .iter()
                .any(|arg| arg == &format!("supervisor::tests::{name}"))
    }

    #[tokio::test]
    async fn bounds_lines_and_tail_and_accepts_crlf_progress() {
        let mut input = vec![b'x'; 2 * MAX_LINE_BYTES];
        input.extend_from_slice(b"\rprogress=continue\r\nlast");
        let tail = Arc::new(Mutex::new(DiagnosticTail::default()));
        let lines = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&lines);
        read_lines(
            &input[..],
            Arc::clone(&tail),
            Arc::new(move |s| captured.lock().unwrap().push(s)),
        )
        .await
        .unwrap();
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].len(), MAX_LINE_BYTES + " [line truncated]".len());
        assert_eq!(lines[1], "progress=continue");
        assert_eq!(lines[2], "last");
        let mut tail = tail.lock().unwrap();
        for _ in 0..100 {
            tail.push("x".repeat(MAX_LINE_BYTES));
        }
        assert!(tail.bytes <= MAX_TAIL_BYTES);
        assert_eq!(tail.lines.len(), 4);
    }

    #[tokio::test]
    async fn completes_a_streaming_pipeline() {
        let (_sender, receiver) = watch::channel(false);
        let lines = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&lines);
        tokio::time::timeout(
            Duration::from_secs(10),
            run(
                vec![fixture("fixture_produce"), fixture("fixture_consume")],
                receiver,
                Arc::new(move |line| captured.lock().unwrap().push(line)),
            ),
        )
        .await
        .expect("pipeline must finish")
        .unwrap();
        assert!(
            lines
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.starts_with("received="))
        );
    }

    #[tokio::test]
    async fn either_pipeline_stage_failure_stops_the_other_stage() {
        for commands in [
            vec![fixture("fixture_fail"), fixture("fixture_wait")],
            vec![fixture("fixture_flood"), fixture("fixture_fail")],
        ] {
            let (_sender, receiver) = watch::channel(false);
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                run(commands, receiver, Arc::new(|_| {})),
            )
            .await
            .expect("stage failure must stop the pipeline promptly");
            assert!(matches!(result, Err(RunError::Failed(_))));
        }
    }

    #[tokio::test]
    async fn cancellation_kills_the_child_tree_before_returning() {
        let (sender, receiver) = watch::channel(false);
        let (pid_sender, mut pids) = tokio::sync::mpsc::unbounded_channel();
        let running = tokio::spawn(run(
            vec![fixture("fixture_spawn_tree")],
            receiver,
            Arc::new(move |line| {
                if let Some(pid) = line
                    .strip_prefix("descendant=")
                    .and_then(|s| s.parse::<u32>().ok())
                {
                    let _ = pid_sender.send(pid);
                }
            }),
        ));
        let pid = tokio::time::timeout(Duration::from_secs(5), pids.recv())
            .await
            .unwrap()
            .unwrap();
        sender.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Err(RunError::Cancelled)));
        assert_process_gone(pid).await;
    }

    #[tokio::test]
    async fn dropping_the_supervisor_kills_its_child_tree() {
        let (_sender, receiver) = watch::channel(false);
        let (pid_sender, mut pids) = tokio::sync::mpsc::unbounded_channel();
        let running = tokio::spawn(run(
            vec![fixture("fixture_spawn_tree")],
            receiver,
            Arc::new(move |line| {
                if let Some(pid) = line
                    .strip_prefix("descendant=")
                    .and_then(|s| s.parse::<u32>().ok())
                {
                    let _ = pid_sender.send(pid);
                }
            }),
        ));
        let pid = tokio::time::timeout(Duration::from_secs(5), pids.recv())
            .await
            .unwrap()
            .unwrap();
        running.abort();
        assert!(running.await.unwrap_err().is_cancelled());
        assert_process_gone(pid).await;
    }

    #[tokio::test]
    async fn callback_panic_stops_the_process_tree() {
        let (_sender, receiver) = watch::channel(false);
        let (pid_sender, mut pids) = tokio::sync::mpsc::unbounded_channel();
        let running = tokio::spawn(run(
            vec![fixture("fixture_spawn_tree")],
            receiver,
            Arc::new(move |line| {
                if let Some(pid) = line
                    .strip_prefix("descendant=")
                    .and_then(|s| s.parse::<u32>().ok())
                {
                    let _ = pid_sender.send(pid);
                    panic!("intentional progress callback panic");
                }
            }),
        ));
        let pid = tokio::time::timeout(Duration::from_secs(5), pids.recv())
            .await
            .unwrap()
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Err(RunError::Failed(_))));
        assert_process_gone(pid).await;
    }

    async fn assert_process_gone(pid: u32) {
        #[cfg(windows)]
        {
            use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
            use windows_sys::Win32::System::Threading::{
                OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
            };
            let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            if raw.is_null() {
                return;
            }
            let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
            let result = tokio::task::spawn_blocking(move || unsafe {
                WaitForSingleObject(handle.as_raw_handle(), 5000)
            })
            .await
            .unwrap();
            assert_eq!(result, 0, "descendant process must be terminated");
        }
        #[cfg(unix)]
        {
            // Reparented zombies can still have a PID; Linux marks them Z.
            for _ in 0..100 {
                if unsafe { libc::kill(pid as i32, 0) } != 0 {
                    return;
                }
                #[cfg(target_os = "linux")]
                if std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
                    s.rsplit_once(") ")
                        .is_some_and(|(_, tail)| tail.starts_with('Z'))
                }) {
                    return;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            panic!("descendant process {pid} survived cancellation");
        }
    }

    #[test]
    #[ignore = "subprocess fixture, invoked by supervisor tests"]
    fn fixture_produce() {
        if !is_fixture_invocation("fixture_produce") {
            return;
        }
        std::io::stdout()
            .write_all(&vec![b'x'; 2 * 1024 * 1024])
            .unwrap();
    }

    #[test]
    #[ignore = "subprocess fixture, invoked by supervisor tests"]
    fn fixture_consume() {
        if !is_fixture_invocation("fixture_consume") {
            return;
        }
        let bytes = std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
        assert!(bytes >= 2 * 1024 * 1024);
        println!("received={bytes}");
    }

    #[test]
    #[ignore = "subprocess fixture, invoked by supervisor tests"]
    fn fixture_fail() {
        if !is_fixture_invocation("fixture_fail") {
            return;
        }
        eprintln!("intentional encoder failure");
        std::process::exit(7);
    }

    #[test]
    #[ignore = "subprocess fixture, invoked by supervisor tests"]
    fn fixture_wait() {
        if !is_fixture_invocation("fixture_wait") {
            return;
        }
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    #[ignore = "subprocess fixture, invoked by supervisor tests"]
    fn fixture_flood() {
        if !is_fixture_invocation("fixture_flood") {
            return;
        }
        let block = [b'x'; 8192];
        let mut output = std::io::stdout().lock();
        while output.write_all(&block).is_ok() {}
    }

    #[test]
    #[ignore = "subprocess fixture, invoked by supervisor tests"]
    fn fixture_spawn_tree() {
        if !is_fixture_invocation("fixture_spawn_tree") {
            return;
        }
        let spec = fixture("fixture_wait");
        let mut command = std::process::Command::new(spec.executable);
        command
            .args(spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().unwrap();
        println!("descendant={}", child.id());
        std::io::stdout().flush().unwrap();
        let _ = child.wait();
    }
}
