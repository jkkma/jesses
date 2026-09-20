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
            let mut stdout = std::io::stdout().lock();
            std::io::copy(&mut std::io::stdin().lock(), &mut stdout).unwrap();
            stdout.flush().unwrap();
            std::process::exit(0);
        }
        "seekable-consumer" => {
            use std::io::{Seek, SeekFrom};
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
            let mut file = std::fs::File::from(owned);
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
    let task = tokio::spawn(async move {
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
    control.set_paused(false).unwrap();
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
    assert!(
        tokio::time::timeout(Duration::from_millis(2300), &mut task)
            .await
            .is_err(),
        "suspended time consumed the encode timeout"
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
    drain(&b"one\rtwo\r\nlast"[..], false, &sender, &log)
        .await
        .unwrap();
    let mut records = Vec::new();
    while let Ok(ProcessEvent::Stdout(value)) = receiver.try_recv() {
        records.push(value);
    }
    assert_eq!(records, ["one", "two", "last"]);
}

#[tokio::test]
async fn capture_accepts_exact_limit_and_rejects_excess() {
    assert_eq!(capture(&b"abcd"[..], 4).await.unwrap(), b"abcd");
    assert!(matches!(
        capture(&b"abcde"[..], 4).await,
        Err(SupervisorError::OutputLimit)
    ));
}
