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

use media_core::StandaloneRecoveryPhase;
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
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference='Stop'; @(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name) | ConvertTo-Json -Compress",
        ])
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
            let final_checkpoint = snapshot
                .standalone_recovery
                .as_ref()
                .is_some_and(|recovery| recovery.phase == StandaloneRecoveryPhase::Finalizing);
            if final_checkpoint && expected.is_file() {
                return expected;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the package final-container attempt must be observed after its durable checkpoint")
}

async fn observe_final_container_attempt(manager: &JobManager, id: &str, root: &Path) -> PathBuf {
    observe_final_attempt(
        manager,
        id,
        root.join(format!(".jesses-{id}-container.partial.mp4")),
    )
    .await
}

async fn lock_recovery_final_stage(
    manager: &JobManager,
    id: &str,
) -> (PathBuf, PathBuf, std::fs::File) {
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|snapshot| snapshot.id == id)
                .unwrap();
            assert!(!snapshot.state.is_terminal(), "{snapshot:#?}");
            if let Some(recovery) = snapshot
                .standalone_recovery
                .filter(|recovery| recovery.phase == StandaloneRecoveryPhase::Finalizing)
            {
                let workspace = PathBuf::from(recovery.workspace);
                if let Ok(bytes) = std::fs::read(workspace.join("manifest.json"))
                    && let Ok(manifest) = serde_json::from_slice::<Value>(&bytes)
                    && let Some(path) = manifest["final_stage"]["path"].as_str()
                {
                    let stage = PathBuf::from(path);
                    if let Ok(lock) = std::fs::OpenOptions::new()
                        .read(true)
                        .share_mode(1 | 2) // FILE_SHARE_READ | FILE_SHARE_WRITE; deny deletion.
                        .open(&stage)
                    {
                        return (workspace, stage, lock);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the committed standalone final stage must be lockable before publication cleanup")
}

fn assert_no_partial_siblings(root: &Path) {
    let leftovers = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".partial."))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "owned partial outputs remained: {leftovers:#?}"
    );
}

async fn qualify(encoder: VideoEncoder, label: &str) {
    let fixture = Fixture::new();
    let input = fixture.0.join(format!("{label}-source.mkv"));
    let output = fixture.0.join(format!("{label}-output.mp4"));
    let history = fixture.0.join("history");
    let logs = fixture.0.join("logs");
    synthesize(&input).await;
    let source_digest = digest(&input);

    let manager = JobManager::open(logs.clone(), history.clone()).await;
    manager.ready().await.unwrap();
    let submitted = manager
        .start_encode(request(&input, &output, encoder))
        .await
        .unwrap();
    let finalizer = observe_final_container_attempt(&manager, &submitted.id, &fixture.0).await;
    assert!(
        finalizer.is_file(),
        "the observed final-container attempt must exist before cancellation"
    );

    manager.cancel_job(submitted.id.clone()).await.unwrap();
    let canceled = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    assert_eq!(canceled.state, JobState::Canceled, "{canceled:#?}");
    assert_eq!(
        canceled
            .standalone_recovery
            .as_ref()
            .map(|recovery| recovery.phase),
        Some(StandaloneRecoveryPhase::Finalizing),
        "canceling the final container must retain the fully verified Matroska checkpoint"
    );
    assert!(!output.exists(), "cancellation published an output");
    assert_eq!(
        digest(&input),
        source_digest,
        "the source changed during cancellation"
    );
    tokio::time::timeout(Duration::from_secs(15), manager.shutdown())
        .await
        .expect("shutdown must join final-container cancellation and cleanup");
    assert_no_owned_workers().await;
    assert_no_partial_siblings(&fixture.0);
    drop(manager);

    let reopened = JobManager::open(logs.clone(), history.clone()).await;
    reopened.ready().await.unwrap();
    assert_no_owned_workers().await;
    let restored = reopened
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.id == submitted.id)
        .unwrap();
    assert_eq!(restored.state, JobState::Canceled, "{restored:#?}");
    assert_eq!(
        restored
            .standalone_recovery
            .as_ref()
            .map(|recovery| recovery.phase),
        Some(StandaloneRecoveryPhase::Finalizing)
    );

    reopened.resume_job(submitted.id.clone()).await.unwrap();
    let second_finalizer =
        observe_final_container_attempt(&reopened, &submitted.id, &fixture.0).await;
    assert!(second_finalizer.is_file());
    reopened.cancel_job(submitted.id.clone()).await.unwrap();
    let canceled_again = wait_for(&reopened, &submitted.id, |job| job.state.is_terminal()).await;
    assert_eq!(
        canceled_again.state,
        JobState::Canceled,
        "{canceled_again:#?}"
    );
    assert_eq!(
        canceled_again
            .standalone_recovery
            .as_ref()
            .map(|recovery| recovery.phase),
        Some(StandaloneRecoveryPhase::Finalizing),
        "a repeated cancellation must keep the resumed durable final checkpoint"
    );
    assert!(
        !output.exists(),
        "repeated cancellation published an output"
    );
    assert_eq!(digest(&input), source_digest);
    tokio::time::timeout(Duration::from_secs(15), reopened.shutdown())
        .await
        .expect("repeated cancellation must join cleanup");
    assert_no_owned_workers().await;
    assert_no_partial_siblings(&fixture.0);
    drop(reopened);

    let resumed = JobManager::open(logs, history).await;
    resumed.ready().await.unwrap();
    assert_no_owned_workers().await;
    let restored_again = resumed
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.id == submitted.id)
        .unwrap();
    assert_eq!(
        restored_again.state,
        JobState::Canceled,
        "{restored_again:#?}"
    );
    assert_eq!(
        restored_again
            .standalone_recovery
            .as_ref()
            .map(|recovery| recovery.phase),
        Some(StandaloneRecoveryPhase::Finalizing)
    );

    resumed.resume_job(submitted.id.clone()).await.unwrap();
    let succeeded = wait_for(&resumed, &submitted.id, |job| job.state.is_terminal()).await;
    tokio::time::timeout(Duration::from_secs(30), resumed.shutdown())
        .await
        .expect("successful restart must join cleanup");
    let settled = resumed
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.id == submitted.id)
        .unwrap();
    assert_eq!(succeeded.state, JobState::Succeeded, "{succeeded:#?}");
    assert_eq!(settled.state, JobState::Succeeded, "{settled:#?}");
    assert!(
        settled
            .logs
            .iter()
            .any(|line| { line.contains("Reusing the fully verified durable Matroska stage") }),
        "resume replayed encoding instead of reusing the final checkpoint: {settled:#?}"
    );
    assert!(settled.standalone_recovery.is_none(), "{settled:#?}");
    assert!(output.is_file() && std::fs::metadata(&output).unwrap().len() > 0);
    assert_eq!(
        digest(&input),
        source_digest,
        "the source changed during restart"
    );
    assert_no_partial_siblings(&fixture.0);
    assert_no_owned_workers().await;

    let probe = tool(
        "ffprobe",
        [
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=codec_name,nb_read_frames",
            "-of",
            "json",
        ]
        .into_iter()
        .map(OsString::from)
        .chain([output.as_os_str().to_owned()])
        .collect(),
        Duration::from_secs(60),
    )
    .await;
    let document: Value = serde_json::from_slice(&probe).unwrap();
    assert_eq!(document["streams"][0]["nb_read_frames"], FRAMES.to_string());
    assert_eq!(
        document["streams"][0]["codec_name"],
        if encoder == VideoEncoder::X264 {
            "h264"
        } else {
            "av1"
        }
    );
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

#[tokio::test]
#[ignore = "requires the packaged Windows FFmpeg, FFprobe and x264 tools"]
async fn locked_recovery_final_stage_is_retained_and_reported_after_publication() {
    let fixture = Fixture::new();
    let input = fixture.0.join("locked-recovery-source.mkv");
    let output = fixture.0.join("locked-recovery-output.mp4");
    synthesize(&input).await;
    let source_digest = digest(&input);
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let submitted = manager
        .start_encode(request(&input, &output, VideoEncoder::X264))
        .await
        .unwrap();
    let (workspace, final_stage, lock) = lock_recovery_final_stage(&manager, &submitted.id).await;

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
    let error = settled
        .error
        .as_ref()
        .expect("retained recovery cleanup failure must be visible on the successful job");
    assert_eq!(error.code, "RECOVERY_INVALID", "{settled:#?}");
    assert_eq!(
        error.path.as_deref(),
        Some(final_stage.to_string_lossy().as_ref())
    );
    assert!(settled.logs.iter().any(|line| {
        line.contains("Output succeeded; standalone recovery files were retained")
    }));
    assert_eq!(
        settled
            .standalone_recovery
            .as_ref()
            .map(|recovery| Path::new(&recovery.workspace)),
        Some(workspace.as_path())
    );
    assert!(
        workspace.is_dir(),
        "the recovery workspace was not retained"
    );
    assert!(
        final_stage.is_file(),
        "the locked recovery artifact was not retained"
    );
    assert!(output.is_file() && std::fs::metadata(&output).unwrap().len() > 0);
    assert_eq!(digest(&input), source_digest);
    assert_no_partial_siblings(&fixture.0);
    assert_no_owned_workers().await;

    let probe = tool(
        "ffprobe",
        [
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=codec_name,nb_read_frames",
            "-of",
            "json",
        ]
        .into_iter()
        .map(OsString::from)
        .chain([output.as_os_str().to_owned()])
        .collect(),
        Duration::from_secs(60),
    )
    .await;
    let document: Value = serde_json::from_slice(&probe).unwrap();
    assert_eq!(document["streams"][0]["codec_name"], "h264");
    assert_eq!(document["streams"][0]["nb_read_frames"], FRAMES.to_string());

    drop(lock);
}

#[tokio::test]
#[ignore = "requires the packaged Windows FFmpeg, FFprobe and x264 tools"]
async fn locked_fresh_matroska_attempt_cleanup_is_reported_after_recovery_clear() {
    let fixture = Fixture::new();
    let input = fixture.0.join("locked-matroska-source.mkv");
    let output = fixture.0.join("locked-matroska-output.mkv");
    synthesize(&input).await;
    let source_digest = digest(&input);
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let submitted = manager
        .start_encode(request(&input, &output, VideoEncoder::X264))
        .await
        .unwrap();
    let partial = observe_final_attempt(
        &manager,
        &submitted.id,
        fixture
            .0
            .join(format!(".jesses-{}.partial.mkv", submitted.id)),
    )
    .await;
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
        "the fresh Matroska attempt must remain job-owned until cleanup can report its denied delete: {settled:#?}"
    );
    assert!(
        settled
            .logs
            .iter()
            .any(|line| line.contains("owned temporary output could not be removed safely"))
    );
    assert!(settled.standalone_recovery.is_none(), "{settled:#?}");
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

#[tokio::test]
#[ignore = "requires the packaged Windows FFmpeg, FFprobe, x264 and SVT-AV1 tools"]
async fn package_x264_and_svt_cancel_final_container_then_reopen_and_resume_safely() {
    qualify(VideoEncoder::X264, "x264").await;
    qualify(VideoEncoder::SvtAv1, "svt").await;
}
