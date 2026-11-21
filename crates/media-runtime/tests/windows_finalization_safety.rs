//! Opt-in Windows package gate for cancel/reopen safety during final container conversion.
//! Run with `--ignored --test-threads=1` and `JESSES_TEST_TOOL_RESOURCES` set to
//! the root of an unpacked portable package. All media and history are test-owned.

#![cfg(windows)]

use std::{
    collections::HashSet,
    ffi::OsString,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_runtime::{
    EncodeBackend, EncodeRequest, EncodeSettings, JobManager, JobSnapshot, JobState, RemuxRequest,
    VideoEncoder,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const FRAMES: u32 = 2_400;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let resources = std::env::var_os("JESSES_TEST_TOOL_RESOURCES")
            .expect("set JESSES_TEST_TOOL_RESOURCES to the unpacked portable package root");
        media_runtime::configure_bundled_tools(PathBuf::from(resources))
            .expect("the package gate uses one absolute verified resource root");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "jesses-finalization-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Failed finalization fixture retained at {}",
                self.0.display()
            );
        } else {
            std::fs::remove_dir_all(&self.0)
                .expect("all package processes and owned file handles must be released");
        }
    }
}

#[derive(Clone, Debug)]
struct WindowsProcess {
    pid: u32,
    parent_pid: u32,
    name: String,
}

fn process_inventory() -> Vec<WindowsProcess> {
    use std::os::windows::process::CommandExt;
    let powershell = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows SystemRoot"))
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let output = Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference='Stop'; @(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name) | ConvertTo-Json -Compress",
        ])
        .creation_flags(0x08000000)
        .output()
        .expect("Windows process inventory must launch");
    assert!(
        output.status.success(),
        "Windows process inventory failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<Value>(&output.stdout)
        .unwrap()
        .as_array()
        .expect("PowerShell must return a process array")
        .iter()
        .filter_map(|value| {
            Some(WindowsProcess {
                pid: u32::try_from(value["ProcessId"].as_u64()?).ok()?,
                parent_pid: u32::try_from(value["ParentProcessId"].as_u64()?).ok()?,
                name: value["Name"].as_str()?.to_owned(),
            })
        })
        .collect()
}

fn descendants(root: u32, inventory: &[WindowsProcess]) -> Vec<WindowsProcess> {
    let mut pids = HashSet::from([root]);
    let mut found = Vec::new();
    loop {
        let mut changed = false;
        for process in inventory {
            if !pids.contains(&process.pid) && pids.contains(&process.parent_pid) {
                pids.insert(process.pid);
                found.push(process.clone());
                changed = true;
            }
        }
        if !changed {
            return found;
        }
    }
}

fn is_package_worker(process: &WindowsProcess) -> bool {
    matches!(
        process.name.to_ascii_lowercase().as_str(),
        "ffmpeg.exe" | "ffprobe.exe" | "x264.exe" | "svtav1encapp.exe"
    )
}

async fn assert_no_owned_workers() {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let inventory = tokio::task::spawn_blocking(process_inventory)
                .await
                .unwrap();
            let live = descendants(std::process::id(), &inventory)
                .into_iter()
                .filter(is_package_worker)
                .collect::<Vec<_>>();
            if live.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("shutdown/reopen must leave no owned package descendants");
}

async fn executable(id: &str) -> PathBuf {
    let tool = media_runtime::get_capabilities()
        .await
        .into_iter()
        .find(|tool| tool.id == id)
        .unwrap_or_else(|| panic!("missing package tool catalog entry: {id}"));
    assert!(
        tool.available,
        "required package tool is unavailable: {tool:?}"
    );
    PathBuf::from(tool.path.unwrap())
}

async fn tool(id: &str, args: Vec<OsString>, timeout: Duration) -> Vec<u8> {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(
        &CommandSpec {
            executable: executable(id).await,
            args,
            cwd: None,
        },
        cancel,
        8 * 1024 * 1024,
        timeout,
    )
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{id} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

async fn synthesize(path: &Path) {
    let duration = (f64::from(FRAMES) / 24.0).to_string();
    let args = [
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=640x360:rate=24",
        "-frames:v",
        &FRAMES.to_string(),
        "-t",
        &duration,
        "-vf",
        "format=yuv420p,setparams=field_mode=prog:range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "18",
        "-bf",
        "0",
        "-color_range",
        "tv",
        "-colorspace",
        "bt709",
        "-color_trc",
        "bt709",
        "-color_primaries",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]
    .into_iter()
    .map(OsString::from)
    .chain([path.as_os_str().to_owned()])
    .collect();
    tool("ffmpeg", args, Duration::from_secs(120)).await;
}

fn digest(path: &Path) -> [u8; 32] {
    Sha256::digest(std::fs::read(path).unwrap()).into()
}

fn request(input: &Path, output: &Path, encoder: VideoEncoder) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0],
        },
        settings: EncodeSettings {
            backend: EncodeBackend::Standalone,
            encoder,
            video_stream_index: 0,
            crf: 30,
            preset: if encoder == VideoEncoder::X264 { 0 } else { 12 },
            ..Default::default()
        },
    }
}

async fn wait_for(
    manager: &JobManager,
    id: &str,
    mut ready: impl FnMut(&JobSnapshot) -> bool,
) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|snapshot| snapshot.id == id)
                .unwrap();
            if ready(&snapshot) || snapshot.state.is_terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("package encode must reach the requested phase within three minutes")
}

async fn observe_final_attempt(manager: &JobManager, id: &str, expected: PathBuf) -> PathBuf {
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|snapshot| snapshot.id == id)
                .unwrap();
            assert!(!snapshot.state.is_terminal(), "{snapshot:#?}");
            if expected.is_file() {
                return expected;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the package final-container attempt must be observed before publication")
}

async fn observe_final_container_attempt(manager: &JobManager, id: &str, root: &Path) -> PathBuf {
    observe_final_attempt(
        manager,
        id,
        root.join(format!(".jesses-{id}-container.partial.mp4")),
    )
    .await
}

#[tokio::test]
#[ignore = "requires the packaged Windows FFmpeg, FFprobe and x264 tools"]
async fn locked_final_container_cleanup_is_reported_after_safe_publication() {
    let fixture = Fixture::new();
    let input = fixture.0.join("locked-source.mkv");
    let output = fixture.0.join("locked-output.mp4");
    synthesize(&input).await;
    let source_digest = digest(&input);
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let submitted = manager
        .start_encode(request(&input, &output, VideoEncoder::X264))
        .await
        .unwrap();
    let partial = observe_final_container_attempt(&manager, &submitted.id, &fixture.0).await;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2) // FILE_SHARE_READ | FILE_SHARE_WRITE; deny deletion.
        .open(&partial)
        .unwrap();

    let terminal = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    assert_eq!(terminal.state, JobState::Succeeded, "{terminal:#?}");
    manager.shutdown().await;
    let settled = manager
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.id == submitted.id)
        .unwrap();
    assert_eq!(settled.state, JobState::Succeeded, "{settled:#?}");
    assert_eq!(
        settled.error.as_ref().map(|error| error.code.as_str()),
        Some("OUTPUT_CLEANUP_FAILED"),
        "the published output may succeed, but its locked final-container sibling must not fail cleanup silently: {settled:#?}"
    );
    assert!(
        settled
            .logs
            .iter()
            .any(|line| { line.contains("owned temporary output could not be removed safely") })
    );
    assert!(output.is_file(), "verified output was not published");
    assert!(
        partial.is_file(),
        "the locked exact sibling was not retained"
    );
    assert_eq!(digest(&input), source_digest);
    assert_no_owned_workers().await;

    drop(lock);
    std::fs::remove_file(&partial).unwrap();
}
