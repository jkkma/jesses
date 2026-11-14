use super::*;
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

#[path = "pipeline_tests.rs"]
mod pipeline;

#[path = "streaming_tests.rs"]
mod streaming;

#[path = "path_tests.rs"]
mod path_overrides;

struct Fixture(PathBuf);
impl Fixture {
    fn new(mode: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-supervisor-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("mode"), mode).unwrap();
        Self(path)
    }
    fn spec(&self) -> CommandSpec {
        spec_at(&self.0)
    }
    async fn pid(&self, descendant: usize) -> u32 {
        let mut path = self.0.clone();
        for _ in 0..descendant {
            path.push("child");
        }
        let path = path.join("pid");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(value) = tokio::fs::read_to_string(&path).await
                    && let Ok(pid) = value.parse()
                {
                    return pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture process should start")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn spec_at(path: &Path) -> CommandSpec {
    CommandSpec {
        executable: std::env::current_exe().unwrap(),
        args: [
            "--exact",
            "supervisor::tests::fake_tool",
            "--ignored",
            "--nocapture",
        ]
        .iter()
        .map(OsString::from)
        .collect(),
        cwd: Some(path.to_owned()),
    }
}

#[allow(clippy::zombie_processes)] // Deliberately exercise orphan-tree cleanup.
fn spawn_fixture_child(path: &Path, mode: &str) {
    let path = path.join("child");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("mode"), mode).unwrap();
    let spec = spec_at(&path);
    let mut command = Command::new(spec.executable);
    command
        .args(spec.args)
        .current_dir(path)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    // Intentionally abandoned to exercise tree ownership rather than Child::kill.
    let _child = command.spawn().unwrap();
}

fn fixture_stdout_file() -> std::fs::File {
    #[cfg(unix)]
    use std::os::fd::AsFd;
    #[cfg(windows)]
    use std::os::windows::io::AsHandle;

    let stdout = std::io::stdout();
    stdout.lock().flush().unwrap();
    #[cfg(unix)]
    let owned = stdout.as_fd().try_clone_to_owned().unwrap();
    #[cfg(windows)]
    let owned = stdout.as_handle().try_clone_to_owned().unwrap();
    // Stdout suppresses EBADF as a successful no-op. Use the actual descriptor
    // so the relay exercises native write errors even on a read-only sink.
    std::fs::File::from(owned)
}

/// A separate invocation of this test binary acts as a deterministic native tool.
#[test]
#[ignore = "subprocess fixture invoked by supervisor tests"]
fn fake_tool() {
    // `--include-ignored` runs every integration gate, including this fixture.
    // Only our explicit, exact subprocess invocation may read fixture files;
    // missing files during a real helper invocation must still fail loudly.
    let args: Vec<_> = std::env::args_os().collect();
    if !args
        .windows(2)
        .any(|pair| pair[0] == "--exact" && pair[1] == "supervisor::tests::fake_tool")
        || !args.iter().any(|arg| arg == "--ignored")
    {
        return;
    }
    let path = std::env::current_dir().unwrap();
    let mode = std::fs::read_to_string(path.join("mode")).unwrap();
    // Descendants use PID-file existence as their ready signal. Publish only
    // after the complete PID is written, so cancellation cannot leave an empty
    // file between create and write when a parent announces that the tree is ready.
    let pending_pid = path.join("pid.pending");
    std::fs::write(&pending_pid, std::process::id().to_string()).unwrap();
    std::fs::rename(pending_pid, path.join("pid")).unwrap();
    match mode.as_str() {
        "pause-tree" | "pause-branch" | "pause-leaf" => {
            if mode == "pause-tree" {
                spawn_fixture_child(&path, "pause-branch");
            }
            if mode == "pause-branch" {
                spawn_fixture_child(&path, "pause-leaf");
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path.join("ticks"))
                .unwrap();
            loop {
                file.write_all(b"x").unwrap();
                file.flush().unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        "environment" => {
            path_overrides::dump_environment(&path);
            std::process::exit(0);
        }
        "environment-parent" => {
            path_overrides::dump_environment(&path);
            spawn_fixture_child(&path, "environment");
            while !path.join("child/environment.json").exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::process::exit(0);
        }
        "stream-large" => streaming::large_output(),
        "stream-reject-tree" => {
            spawn_fixture_child(&path, "branch");
            while !path.join("child/child/pid").exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::io::stdout()
                .write_all(b"\nSTREAM_TREE_READY\n")
                .unwrap();
        }
        "binary-producer" => {
            let diagnostics = std::thread::spawn(|| {
                let mut stderr = std::io::stderr().lock();
                for _ in 0..32 {
                    stderr.write_all(&[b'p'; RECORD_BYTES]).unwrap();
                }
                stderr.write_all(b"\nproducer diagnostic\n").unwrap();
            });
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(pipeline::RAW_MARKER).unwrap();
            let block = pipeline::binary_block();
            for _ in 0..pipeline::BINARY_CHUNKS {
                stdout.write_all(&block).unwrap();
            }
            stdout.flush().unwrap();
            drop(stdout);
            diagnostics.join().unwrap();
            std::process::exit(0);
        }
        "binary-consumer" => {
            let stdout = std::thread::spawn(|| {
                let mut stdout = std::io::stdout().lock();
                for _ in 0..32 {
                    stdout.write_all(&[b'c'; RECORD_BYTES]).unwrap();
                }
                stdout.write_all(b"\nconsumer complete\n").unwrap();
            });
            let stderr = std::thread::spawn(|| {
                let mut stderr = std::io::stderr().lock();
                for _ in 0..32 {
                    stderr.write_all(&[b'e'; RECORD_BYTES]).unwrap();
                }
                stderr.write_all(b"\nconsumer diagnostic\n").unwrap();
            });
            let mut file = std::fs::File::create(path.join("received.bin")).unwrap();
            std::io::copy(&mut std::io::stdin().lock(), &mut file).unwrap();
            file.flush().unwrap();
            stdout.join().unwrap();
            stderr.join().unwrap();
            std::process::exit(0);
        }
        "binary-relay" => {
            std::io::stderr()
                .write_all(b"binary relay diagnostic\n")
                .unwrap();
            let mut stdout = fixture_stdout_file();
            std::io::copy(&mut std::io::stdin().lock(), &mut stdout).unwrap();
            stdout.flush().unwrap();
            std::process::exit(0);
        }
        "seekable-consumer" => {
            use std::io::{Seek, SeekFrom};
            let mut file = fixture_stdout_file();
            // IVF encoders write a provisional header, then seek back to record
            // their final frame count. A pipe relay cannot support this contract.
            file.seek(SeekFrom::Start(0)).unwrap();
            file.write_all(b"HEAD0000").unwrap();
            std::io::copy(&mut std::io::stdin().lock(), &mut file).unwrap();
            file.seek(SeekFrom::Start(0)).unwrap();
            file.write_all(b"HEADdone").unwrap();
            file.flush().unwrap();
            std::process::exit(0);
        }
        "binary-fail" => {
            wait_peer(&path);
            std::io::stdout().write_all(b"partial binary data").unwrap();
            std::process::exit(7);
        }
        "read-then-hang" => {
            std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink()).unwrap();
        }
        "early-consumer" | "failed-consumer" => {
            wait_peer(&path);
            std::process::exit(if mode == "early-consumer" { 0 } else { 9 });
        }
        "binary-producer-tree" => {
            spawn_fixture_child(&path, "branch");
            // The child harness writes its header to inherited stdout before
            // fake_tool starts. Let both descendants start before flooding that
            // pipe, otherwise backpressure correctly blocks their startup too.
            while !path.join("child/child/pid").exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            let mut stdout = std::io::stdout().lock();
            loop {
                stdout.write_all(&pipeline::binary_block()).unwrap();
            }
        }
        "blocked-consumer-tree" => spawn_fixture_child(&path, "branch"),
        #[cfg(windows)]
        "supervisor-host" => {
            let child_path = path.join("child");
            std::fs::create_dir(&child_path).unwrap();
            std::fs::write(child_path.join("mode"), "tree").unwrap();
            let spec = spec_at(&child_path);
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let (_cancel, receiver) = watch::channel(false);
                let _task = tokio::spawn(async move {
                    run_capture(&spec, receiver, 65536, Duration::from_secs(30)).await
                });
                let marker = child_path.join("child/child/pid");
                while !marker.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                // No destructors: only the OS closing the host's job handle can
                // terminate this tree. Inherited job handles would break this.
                std::process::exit(0);
            });
        }
        "tree" => spawn_fixture_child(&path, "branch"),
        "branch" => spawn_fixture_child(&path, "sleep"),
        "exit-parent" => {
            spawn_fixture_child(&path, "sleep");
            while !path.join("child/pid").exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::process::exit(0);
        }
        "flood" => {
            let writer = std::thread::spawn(|| {
                let mut stderr = std::io::stderr().lock();
                for _ in 0..800 {
                    stderr.write_all(&[0xff; RECORD_BYTES]).unwrap();
                }
                stderr.write_all(b"\rfinal-stderr").unwrap();
                stderr.flush().unwrap();
            });
            let mut stdout = std::io::stdout().lock();
            for _ in 0..800 {
                stdout.write_all(&[b'x'; RECORD_BYTES]).unwrap();
            }
            stdout.write_all(b"\rprogress=10\r\nfinal-stdout").unwrap();
            stdout.flush().unwrap();
            drop(stdout);
            writer.join().unwrap();
            std::process::exit(0);
        }
        "relay-flood" => {
            let mut stderr = std::io::stderr().lock();
            let payload = "x".repeat(1024);
            for index in 0..512 {
                writeln!(stderr, "{index:04} {payload}").unwrap();
            }
            stderr.flush().unwrap();
            std::process::exit(0);
        }
        "watchdog-output" | "watchdog-stalled-output" | "watchdog-progress-file" => {
            for frames in 1..=30 {
                if mode != "watchdog-progress-file" {
                    let mut stderr = std::io::stderr().lock();
                    let position = if mode == "watchdog-output" { frames } else { 7 };
                    // Vary the elapsed time, speed and ETA independently of
                    // frame position. The real terminal also does this when idle.
                    write!(stderr, "\x1b[1").unwrap();
                    stderr.flush().unwrap();
                    std::thread::sleep(Duration::from_millis(5));
                    write!(
                        stderr,
                        "m00:00:{frames:02}\x1b[0m [0/4 Chunks] ▐██░░▌ 5% {position}/"
                    )
                    .unwrap();
                    stderr.flush().unwrap();
                    std::thread::sleep(Duration::from_millis(5));
                    write!(stderr, "100 ({} fps, eta {frames}s)\r", 120 / frames).unwrap();
                    stderr.flush().unwrap();
                } else {
                    let data = "x".repeat(frames);
                    std::fs::write(path.join("done.json"), data).unwrap();
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            std::process::exit(0);
        }
        "small" => {
            std::io::stdout()
                .write_all(b"binary\0\xff\r\ncomplete")
                .unwrap();
            std::io::stderr().write_all(b"stderr-complete").unwrap();
            std::process::exit(0);
        }
        "nonzero" => std::process::exit(7),
        "sleep" => {}
        _ => panic!("unknown fixture mode: {mode}"),
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn wait_peer(path: &Path) {
    let peer = std::fs::read_to_string(path.join("peer")).unwrap();
    while !Path::new(&peer).join("pid").exists() {
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[tokio::test]
async fn live_pause_halts_descendants_is_idempotent_and_cancel_kills_paused_tree() {
    let fixture = Fixture::new("pause-tree");
    let spec = fixture.spec();
    let path = fixture.0.join("tool.log");
    let control = PauseControl::default();
    let active = control.clone();
    let (owner, cancel) = watch::channel(false);
    let (events, _receiver) = mpsc::channel(64);
    let mut task = tokio::spawn(async move {
        run_with_pause(
            &spec,
            cancel,
            events,
            &path,
            Duration::from_secs(30),
            None,
            Some(&active),
        )
        .await
    });
    let pids = [
        fixture.pid(0).await,
        fixture.pid(1).await,
        fixture.pid(2).await,
    ];
    let paths = [
        fixture.0.join("ticks"),
        fixture.0.join("child/ticks"),
        fixture.0.join("child/child/ticks"),
    ];
    tokio::time::sleep(Duration::from_millis(40)).await;
    control.set_paused(true).unwrap();
    control.set_paused(true).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let lengths: Vec<_> = paths
        .iter()
        .map(|path| std::fs::metadata(path).unwrap().len())
        .collect();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        lengths,
        paths
            .iter()
            .map(|path| std::fs::metadata(path).unwrap().len())
            .collect::<Vec<_>>()
    );
    if let Err(error) = control.set_paused(false) {
        let result = tokio::time::timeout(Duration::from_secs(1), &mut task).await;
        panic!("could not resume paused tree: {error}; operation result: {result:?}");
    }
    control.set_paused(false).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    for (path, previous) in paths.iter().zip(lengths) {
        assert!(std::fs::metadata(path).unwrap().len() > previous);
    }
    control.set_paused(true).unwrap();
    owner.send_replace(true);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap(),
        Err(SupervisorError::Cancelled)
    ));
    for pid in pids {
        assert!(!alive(pid), "paused descendant {pid} survived cancellation");
    }
    assert!(control.set_paused(false).is_err());
}

#[tokio::test]
async fn live_pause_excludes_suspended_time_from_timeout() {
    let fixture = Fixture::new("pause-tree");
    let spec = fixture.spec();
    let path = fixture.0.join("tool.log");
    let control = PauseControl::default();
    let active = control.clone();
    let (_owner, cancel) = watch::channel(false);
    let (events, _receiver) = mpsc::channel(64);
    let mut task = tokio::spawn(async move {
        run_with_pause(
            &spec,
            cancel,
            events,
            &path,
            Duration::from_secs(2),
            None,
            Some(&active),
        )
        .await
    });
    let pids = [
        fixture.pid(0).await,
        fixture.pid(1).await,
        fixture.pid(2).await,
    ];
    control.set_paused(true).unwrap();
    let paused_result = tokio::time::timeout(Duration::from_millis(2300), &mut task).await;
    assert!(
        paused_result.is_err(),
        "operation finished while its tree was suspended: {paused_result:?}"
    );
    control.set_paused(false).unwrap();
    // Allow the remaining two-second active limit plus Windows' five-second
    // tree teardown budget, with slack for scheduling and flushing the log.
    let result = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("resumed process should time out and finish cleanup")
        .unwrap();
    assert!(matches!(result, Err(SupervisorError::Timeout)));
    assert_dead(&pids).await;
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    // SAFETY: process IDs come only from the isolated subprocess fixtures.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let running = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        CloseHandle(handle);
        running
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // An orphaned zombie is terminated and cannot execute; init owns its reaping.
    #[cfg(target_os = "linux")]
    if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        && status
            .rsplit_once(") ")
            .is_some_and(|(_, tail)| tail.starts_with('Z'))
    {
        return false;
    }
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

async fn assert_dead(pids: &[u32]) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while pids.iter().any(|&pid| alive(pid)) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("supervisor must terminate every descendant");
}

#[tokio::test]
async fn cancellation_kills_grandchildren() {
    let fixture = Fixture::new("tree");
    let spec = fixture.spec();
    let (cancel, receiver) = watch::channel(false);
    let task =
        tokio::spawn(
            async move { run_capture(&spec, receiver, 65536, Duration::from_secs(30)).await },
        );
    let pids = [
        fixture.pid(0).await,
        fixture.pid(1).await,
        fixture.pid(2).await,
    ];
    cancel.send(true).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(SupervisorError::Cancelled)
    ));
    assert_dead(&pids).await;
}

#[tokio::test]
async fn dropping_future_kills_grandchildren() {
    let fixture = Fixture::new("tree");
    let spec = fixture.spec();
    let (_cancel, receiver) = watch::channel(false);
    let task =
        tokio::spawn(
            async move { run_capture(&spec, receiver, 65536, Duration::from_secs(30)).await },
        );
    let pids = [
        fixture.pid(0).await,
        fixture.pid(1).await,
        fixture.pid(2).await,
    ];
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_dead(&pids).await;
}

#[cfg(windows)]
#[tokio::test]
async fn abrupt_supervisor_host_exit_closes_noninherited_job() {
    let fixture = Fixture::new("supervisor-host");
    let spec = fixture.spec();
    // The host itself deliberately has no supervisor-owned Job Object, so this
    // test cannot accidentally pass through an outer job's cleanup.
    let status = platform::run_unowned_test_host(&spec).await.unwrap();
    assert!(status.success());
    assert_dead(&[
        fixture.pid(1).await,
        fixture.pid(2).await,
        fixture.pid(3).await,
    ])
    .await;
}

#[tokio::test]
async fn parent_exit_terminates_descendant_holding_pipes() {
    let fixture = Fixture::new("exit-parent");
    let (_cancel, receiver) = watch::channel(false);
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        run_capture(&fixture.spec(), receiver, 65536, Duration::from_secs(20)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(output.status.success());
    assert_dead(&[fixture.pid(0).await, fixture.pid(1).await]).await;
}

#[tokio::test]
async fn timeout_and_output_limit_terminate_tools() {
    let fixture = Fixture::new("sleep");
    let (_cancel, receiver) = watch::channel(false);
    let result = run_capture(&fixture.spec(), receiver, 65536, Duration::from_millis(250)).await;
    assert!(matches!(result, Err(SupervisorError::Timeout)));
    if let Ok(value) = std::fs::read_to_string(fixture.0.join("pid")) {
        assert_dead(&[value.parse().unwrap()]).await;
    }
    let fixture = Fixture::new("flood");
    let (_cancel, receiver) = watch::channel(false);
    assert!(matches!(
        run_capture(&fixture.spec(), receiver, 32768, Duration::from_secs(10)).await,
        Err(SupervisorError::OutputLimit)
    ));
    assert_dead(&[fixture.pid(0).await]).await;
}

#[tokio::test]
async fn drains_binary_flood_with_blocked_event_consumer_and_rotates_logs() {
    let fixture = Fixture::new("flood");
    let (_cancel, receiver) = watch::channel(false);
    let (events, _blocked_consumer) = mpsc::channel(1);
    let log = fixture.0.join("job.log");
    let result = run(
        &fixture.spec(),
        receiver,
        events,
        &log,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(result.status.success());
    assert!(std::fs::metadata(&log).unwrap().len() <= LOG_BYTES);
    assert!(
        std::fs::metadata(fixture.0.join("job.log.1"))
            .unwrap()
            .len()
            <= LOG_BYTES
    );
    let retained = std::fs::read(&log).unwrap();
    assert!(
        retained
            .windows(b"final-stderr".len())
            .any(|chunk| chunk == b"final-stderr")
    );
    assert!(
        retained
            .windows(b"final-stdout".len())
            .any(|chunk| chunk == b"final-stdout")
    );
}

#[tokio::test]
async fn unread_terminal_does_not_block_child_or_drop_logged_records() {
    let viewer_fixture = Fixture::new("sleep");
    let mut viewer = platform::OwnedChild::spawn_with_stdin(&viewer_fixture.spec()).unwrap();
    let mut writer = viewer.take_stdin();
    let child_fixture = Fixture::new("relay-flood");
    let child = platform::OwnedChild::spawn_with_environment(&child_fixture.spec(), None).unwrap();
    let (_owner, cancel) = watch::channel(false);
    let (events, _blocked_consumer) = mpsc::channel(1);
    let path = child_fixture.0.join("job.log");
    let log = Mutex::new(RotatingLog::open(&path).await.unwrap());
    let (terminal, mut reads) = mpsc::channel::<Vec<u8>>(16);
    let relay = tokio::spawn(async move {
        while let Some(bytes) = reads.recv().await {
            writer.write_all(&bytes).await?;
        }
        writer.flush().await
    });
    let result = run_logged_child(
        child,
        cancel,
        events,
        &log,
        RunDeadline::Runtime(Duration::from_secs(3)),
        None,
        Some(&terminal),
    )
    .await;
    viewer.terminate_and_wait().await.unwrap();
    assert_eq!(
        relay.await.unwrap().unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
    assert!(result.unwrap().status.success());
    let records = std::fs::read_to_string(path).unwrap();
    let payload = "x".repeat(1024);
    assert_eq!(
        records
            .lines()
            .filter(|line| line.ends_with(&payload))
            .collect::<Vec<_>>(),
        (0..512)
            .map(|index| format!("[stderr] {index:04} {payload}"))
            .collect::<Vec<_>>(),
    );
}

const WATCHDOG_LIMIT: Duration = Duration::from_secs(1);

fn start_watchdog(
    fixture: &Fixture,
    cancel: watch::Receiver<bool>,
    pause: Option<PauseControl>,
) -> tokio::task::JoinHandle<Result<ProcessResult, SupervisorError>> {
    let spec = fixture.spec();
    let work = fixture.0.clone();
    tokio::spawn(async move {
        let (events, _blocked_consumer) = mpsc::channel(1);
        with_console_profile(ConsoleProfile::Hidden, || async {
            run_with_environment_until_inactive(
                &spec,
                cancel,
                events,
                &work.join("job.log"),
                WATCHDOG_LIMIT,
                [&work.join("done.json"), &work.join("av1an.log")],
                None,
                pause.as_ref(),
            )
            .await
        })
        .await
    })
}

#[tokio::test]
async fn watchdog_allows_active_output_or_progress_files_past_the_limit() {
    for mode in ["watchdog-output", "watchdog-progress-file"] {
        let fixture = Fixture::new(mode);
        let (_owner, cancel) = watch::channel(false);
        let started = std::time::Instant::now();
        let result = start_watchdog(&fixture, cancel, None).await.unwrap();
        assert!(result.unwrap().status.success(), "watchdog fixture {mode}");
        assert!(started.elapsed() > WATCHDOG_LIMIT);
    }
}

#[tokio::test]
async fn watchdog_times_out_despite_steady_redraws_without_frame_progress() {
    let fixture = Fixture::new("watchdog-stalled-output");
    let (_owner, cancel) = watch::channel(false);
    let task = start_watchdog(&fixture, cancel, None);
    let pid = fixture.pid(0).await;
    let result = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(SupervisorError::InactivityTimeout)));
    assert_dead(&[pid]).await;
}

#[tokio::test]
async fn watchdog_parses_progress_across_split_ansi_and_conpty_redraws() {
    use tokio::io::AsyncWriteExt;

    let fixture = Fixture::new("watchdog-ansi");
    let log = Mutex::new(RotatingLog::open(&fixture.0.join("job.log")).await.unwrap());
    let (events, _blocked_consumer) = mpsc::channel(1);
    let (terminal, _viewer) = mpsc::channel(1);
    let (activity, mut updates) = watch::channel(());
    // Capacity one forces each test write to survive across multiple reads.
    let (mut source, stream) = tokio::io::duplex(1);
    let input = async {
        for (position, elapsed, advances) in [(7, 1, true), (7, 2, false), (8, 3, true)] {
            let record = format!(
                "\x1b[3;1H\x1b[1m00:00:{elapsed:02}\x1b[0m [1/4 Chunks] ▐██░░▌ 5% {position}/100 ({} fps, eta {elapsed}s)\x1b[2;1H",
                120 / elapsed,
            );
            source.write_all(record.as_bytes()).await.unwrap();
            if advances {
                tokio::time::timeout(Duration::from_secs(1), updates.changed())
                    .await
                    .expect("advancing frame counter resets inactivity promptly")
                    .unwrap();
            } else {
                // Send another completed cursor boundary so the end of the
                // preceding write has definitely reached the record parser.
                source.write_all(b"\x1b[3;1H").await.unwrap();
                assert!(!updates.has_changed().unwrap());
            }
        }
        drop(source);
    };
    let (_, result) = tokio::join!(
        input,
        drain_stage_every(
            stream,
            true,
            None,
            &events,
            &log,
            REDRAW_LOG_INTERVAL,
            DrainExtras {
                terminal: Some(&terminal),
                activity: Some(&activity)
            },
        )
    );
    result.unwrap();
}

#[tokio::test]
async fn watchdog_ignores_stale_files_and_stops_silent_tree() {
    let fixture = Fixture::new("tree");
    std::fs::write(fixture.0.join("done.json"), "prior progress").unwrap();
    std::fs::write(fixture.0.join("av1an.log"), "prior activity").unwrap();
    let (_owner, cancel) = watch::channel(false);
    let task = start_watchdog(&fixture, cancel, None);
    let pids = [
        fixture.pid(0).await,
        fixture.pid(1).await,
        fixture.pid(2).await,
    ];
    let result = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(SupervisorError::InactivityTimeout)));
    assert_dead(&pids).await;
}

#[tokio::test]
async fn watchdog_ignores_pauses_and_stops_if_silent_after_resume() {
    let fixture = Fixture::new("sleep");
    let (_owner, cancel) = watch::channel(false);
    let pause = PauseControl::default();
    let mut task = start_watchdog(&fixture, cancel, Some(pause.clone()));
    let pid = fixture.pid(0).await;
    pause.set_paused(true).unwrap();
    let result = tokio::time::timeout(WATCHDOG_LIMIT + Duration::from_millis(250), &mut task).await;
    assert!(result.is_err(), "idle clock ran during pause: {result:?}");
    pause.set_paused(false).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(SupervisorError::InactivityTimeout)));
    assert_dead(&[pid]).await;
}

#[tokio::test]
async fn watchdog_still_accepts_cancellation_while_paused() {
    let fixture = Fixture::new("sleep");
    let (owner, cancel) = watch::channel(false);
    let pause = PauseControl::default();
    let task = start_watchdog(&fixture, cancel, Some(pause.clone()));
    let pid = fixture.pid(0).await;
    pause.set_paused(true).unwrap();
    owner.send_replace(true);
    let result = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(SupervisorError::Cancelled)));
    assert_dead(&[pid]).await;
}

#[tokio::test]
async fn capture_preserves_binary_bytes_and_exit_codes() {
    let fixture = Fixture::new("small");
    let (_cancel, receiver) = watch::channel(false);
    let output = run_capture(&fixture.spec(), receiver, 65536, Duration::from_secs(10))
        .await
        .unwrap();
    assert!(output.stdout.ends_with(b"binary\0\xff\r\ncomplete"));
    assert!(output.stderr.ends_with(b"stderr-complete"));
    assert!(output.status.success());
    let fixture = Fixture::new("nonzero");
    let (_cancel, receiver) = watch::channel(false);
    assert_eq!(
        run_capture(&fixture.spec(), receiver, 65536, Duration::from_secs(10))
            .await
            .unwrap()
            .status
            .code(),
        Some(7)
    );
}

#[tokio::test]
async fn cancellation_before_launch_does_not_start_tool() {
    let fixture = Fixture::new("sleep");
    let (_cancel, receiver) = watch::channel(true);
    assert!(matches!(
        run_capture(&fixture.spec(), receiver, 65536, Duration::from_secs(10)).await,
        Err(SupervisorError::Cancelled)
    ));
    assert!(!fixture.0.join("pid").exists());
}

#[tokio::test]
async fn records_cr_progress_and_flushes_partial_final_line() {
    let fixture = Fixture::new("small");
    let log = Mutex::new(
        RotatingLog::open(&fixture.0.join("lines.log"))
            .await
            .unwrap(),
    );
    let (sender, mut receiver) = mpsc::channel(10);
    drain_stage(&b"one\rtwo\r\nlast"[..], false, None, &sender, &log)
        .await
        .unwrap();
    let mut records = Vec::new();
    while let Ok(ProcessEvent::Stdout(value)) = receiver.try_recv() {
        records.push(value);
    }
    assert_eq!(records, ["one", "two", "last"]);
}

// SvtAv1EncApp --progress 2 as captured: CRLF lines around bare-CR redraws.
const SVT_PROGRESS: &[u8] = b"banner\r\nEncoding          \rEncoding: 1 Frames\rEncoding: 2 Frames\rEncoding: 3 Frames\rEncoding: 3 Frames\r\nSUMMARY\r\n";

async fn staged_records(input: &[u8], interval: Duration) -> (Vec<String>, Vec<String>) {
    let fixture = Fixture::new("small");
    let path = fixture.0.join("progress.log");
    let log = Mutex::new(RotatingLog::open(&path).await.unwrap());
    let (sender, mut receiver) = mpsc::channel(32);
    drain_stage_every(
        input,
        true,
        Some("consumer"),
        &sender,
        &log,
        interval,
        DrainExtras::default(),
    )
    .await
    .unwrap();
    log.lock().await.flush().await.unwrap();
    let mut events = Vec::new();
    while let Ok(ProcessEvent::Stderr(value)) = receiver.try_recv() {
        events.push(value);
    }
    let lines = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    (events, lines)
}

#[tokio::test]
async fn job_log_thins_progress_redraws_while_events_keep_every_record() {
    let (events, lines) = staged_records(SVT_PROGRESS, Duration::from_secs(3600)).await;
    assert_eq!(
        events,
        [
            "[consumer] banner",
            "[consumer] Encoding          ",
            "[consumer] Encoding: 1 Frames",
            "[consumer] Encoding: 2 Frames",
            "[consumer] Encoding: 3 Frames",
            "[consumer] Encoding: 3 Frames",
            "[consumer] SUMMARY",
        ]
    );
    assert_eq!(
        lines,
        [
            "[stderr] [consumer] banner",
            "[stderr] [consumer] Encoding: 3 Frames",
            "[stderr] [consumer] SUMMARY",
        ]
    );
}

#[tokio::test]
async fn conpty_cursor_redraws_are_thinned_and_raw_terminal_output_is_preserved() {
    // A Windows ConPTY capture has cursor positions between full-width redraws,
    // and CRLF for real diagnostic lines. Shorten its padding for this test.
    let input = concat!(
        "\x1b[?9001h\x1b[Hdiagnostic before 日本語\r\n",
        "\x1b[?25h[Chunk  1] 10 frames   [Chunk  3] 9 frames   ",
        "\x1b[3;142H\x1b[?25l\x1b[2;1H[Chunk  1] 20 frames   [Chunk  3] 19 frames   ",
        "\x1b[?25h\x1b[3;141H\r\ndiagnostic after café\r\n\x1b[?9001l",
    )
    .as_bytes();
    let fixture = Fixture::new("conpty-redraw");
    let log_path = fixture.0.join("progress.log");
    let (relay, mut reads) = mpsc::channel(1);
    let log = Mutex::new(RotatingLog::open(&log_path).await.unwrap());
    let (sender, _receiver) = mpsc::channel(16);
    drain_stage_every(
        input,
        true,
        None,
        &sender,
        &log,
        Duration::from_secs(3600),
        DrainExtras {
            terminal: Some(&relay),
            activity: None,
        },
    )
    .await
    .unwrap();
    log.lock().await.flush().await.unwrap();
    assert_eq!(reads.try_recv().unwrap(), input);
    let records = std::fs::read_to_string(log_path).unwrap();
    let lines = records.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            "[stderr] diagnostic before 日本語",
            "[stderr] [Chunk  1] 20 frames   [Chunk  3] 19 frames",
            "[stderr] diagnostic after café",
        ],
    );
}

#[tokio::test]
async fn job_log_keeps_each_redraw_once_its_interval_has_passed() {
    let (_, lines) = staged_records(SVT_PROGRESS, Duration::ZERO).await;
    assert_eq!(
        lines,
        [
            "[stderr] [consumer] banner",
            "[stderr] [consumer] Encoding          ",
            "[stderr] [consumer] Encoding: 1 Frames",
            "[stderr] [consumer] Encoding: 2 Frames",
            "[stderr] [consumer] Encoding: 3 Frames",
            "[stderr] [consumer] Encoding: 3 Frames",
            "[stderr] [consumer] SUMMARY",
        ]
    );
}

#[tokio::test]
async fn job_log_keeps_the_last_redraw_when_output_ends_mid_progress() {
    let (_, lines) = staged_records(
        b"start\r\nframe=1\rframe=2\rframe=3\r",
        Duration::from_secs(3600),
    )
    .await;
    assert_eq!(
        lines,
        ["[stderr] [consumer] start", "[stderr] [consumer] frame=3"]
    );
    let (_, lines) = staged_records(b"frame=1\rframe=2\rtail", Duration::from_secs(3600)).await;
    assert_eq!(
        lines,
        [
            "[stderr] [consumer] frame=1",
            "[stderr] [consumer] frame=2",
            "[stderr] [consumer] tail",
        ]
    );
}

#[tokio::test]
async fn capture_accepts_exact_limit_and_rejects_excess() {
    assert_eq!(capture(&b"abcd"[..], 4, false).await.unwrap(), b"abcd");
    assert!(matches!(
        capture(&b"abcde"[..], 4, false).await,
        Err(SupervisorError::OutputLimit)
    ));
}
