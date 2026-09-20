//! Opt-in real av1an stop/reopen/resume gate. Run with `--ignored --test-threads=1`.
//! Fixtures and changed receipts are test-owned; installed tools are never edited.
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_core::{
    AudioChannels, AudioCodec, AudioTrackSettings, BorderSettings, CropSettings, VideoFraming,
};
use media_runtime::{
    EncodeBackend, EncodeRequest, EncodeSettings, JobManager, JobSnapshot, JobState, RecoveryPhase,
    RemuxRequest, VideoEncoder,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        if let Some(resources) = std::env::var_os("JESSES_TEST_TOOL_RESOURCES") {
            media_runtime::configure_bundled_tools(PathBuf::from(resources))
                .expect("the package gate uses one absolute verified resource root");
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("jesses-recovery-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Recovery fixture retained at {}", self.0.display());
        } else {
            // Every file belongs to this fresh fixture, including its job workspaces.
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

async fn output(command: &mut Command) -> Vec<u8> {
    static TOOLS: tokio::sync::OnceCell<Vec<media_runtime::ToolInfo>> =
        tokio::sync::OnceCell::const_new();
    let tools = TOOLS
        .get_or_init(|| Box::pin(media_runtime::get_capabilities()))
        .await;
    let tool = tools
        .iter()
        .find(|tool| tool.id == command.get_program().to_string_lossy())
        .unwrap();
    assert!(tool.available, "Required tool unavailable: {tool:?}");
    let spec = CommandSpec {
        executable: PathBuf::from(tool.path.as_ref().unwrap()),
        args: command.get_args().map(Into::into).collect(),
        cwd: None,
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = Box::pin(run_capture(
        &spec,
        cancel,
        16 * 1024 * 1024,
        Duration::from_secs(60),
    ))
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "Native tool failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

const FRAMES: u64 = 720;
async fn synthesize(path: &Path, hdr: bool) {
    synthesize_frames(path, hdr, FRAMES).await;
}

async fn synthesize_frames(path: &Path, hdr: bool, frames: u64) {
    let attachment = path.parent().unwrap().join("retained-font.ttf");
    std::fs::write(&attachment, [b'A'; 4096]).unwrap();
    let subtitles = path.parent().unwrap().join("retained-subtitles.srt");
    std::fs::write(&subtitles, "1\n00:00:01,000 --> 00:00:02,000\nRetained first cue\n\n2\n00:00:20,000 --> 00:00:21,000\nRetained last cue\n").unwrap();
    let (primaries, transfer, matrix) = if hdr {
        ("bt2020", "smpte2084", "bt2020nc")
    } else {
        ("bt709", "bt709", "bt709")
    };
    let filter = format!(
        "testsrc2=size=320x180:rate=24,format=yuv420p10le,setparams=range=limited:color_primaries={primaries}:color_trc={transfer}:colorspace={matrix}"
    );
    let mut command = Command::new("ffmpeg");
    command.args([
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        &filter,
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000",
        "-i",
        subtitles.to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map",
        "2:s",
        "-c:s",
        "srt",
        "-frames:v",
        &frames.to_string(),
        "-t",
        &(frames as f64 / 24.0).to_string(),
        "-c:a",
        "pcm_s16le",
        "-color_primaries",
        primaries,
        "-color_trc",
        transfer,
        "-colorspace",
        matrix,
        "-color_range",
        "tv",
        "-chroma_sample_location",
        "left",
        "-metadata:s:a:0",
        "language=jpn",
        "-metadata:s:a:0",
        "title=Retained tone",
    ]);
    if hdr {
        command.args(["-c:v", "libx265", "-preset", "ultrafast", "-x265-params", "log-level=error:pools=none:frame-threads=1:bframes=0:hdr10=1:chromaloc=0:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=200,142"]);
    } else {
        command.args(["-c:v", "ffv1", "-level", "3", "-g", "1"]);
    }
    command.arg("-attach").arg(&attachment).args([
        "-metadata:s:t:0",
        "mimetype=application/x-truetype-font",
        "-metadata:s:t:0",
        "filename=retained-font.ttf",
    ]);
    output(command.arg(path)).await;
}

fn request(input: &Path, output: &Path, encoder: VideoEncoder) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![1, 0, 2, 3],
        },
        settings: EncodeSettings {
            backend: EncodeBackend::Av1an,
            encoder,
            workers: 1,
            video_stream_index: 0,
            crf: 32,
            preset: 6,
            ..Default::default()
        },
    }
}

async fn wait_for(
    manager: &JobManager,
    id: &str,
    ready: impl Fn(&JobSnapshot) -> bool,
) -> JobSnapshot {
    let result = tokio::time::timeout(Duration::from_secs(240), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if ready(&job) || job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if result.is_err() {
        manager.shutdown().await;
    }
    result.expect("Recovery job must progress within four minutes")
}

async fn pause_live(manager: &JobManager, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            match manager.set_job_paused(id.into(), true).await {
                Ok(job) => break job,
                Err(error) => {
                    let job = manager
                        .list_jobs()
                        .await
                        .into_iter()
                        .find(|job| job.id == id)
                        .unwrap();
                    assert!(
                        !job.state.is_terminal(),
                        "av1an ended before pause: {:?} {:?}",
                        job.error,
                        error
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    })
    .await
    .expect("av1an must expose a pausable process")
}

#[cfg(windows)]
const CRASH_CHILD_ENV: &str = "JESSES_AV1AN_CRASH_CHILD";
#[cfg(windows)]
const CRASH_ROOT_ENV: &str = "JESSES_AV1AN_CRASH_ROOT";
#[cfg(windows)]
const CRASH_FRAMES: u64 = 2_400;

#[cfg(windows)]
#[derive(Clone, Debug)]
struct WindowsProcess {
    pid: u32,
    parent_pid: u32,
    name: String,
    executable_path: Option<String>,
}

#[cfg(windows)]
fn windows_process_inventory() -> Vec<WindowsProcess> {
    let result = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference='Stop'; @(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,ExecutablePath) | ConvertTo-Json -Compress",
        ])
        .output()
        .expect("Windows process inventory must launch");
    assert!(
        result.status.success(),
        "Windows process inventory failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let values: Value = serde_json::from_slice(&result.stdout).unwrap();
    values
        .as_array()
        .expect("PowerShell must return a process array")
        .iter()
        .filter_map(|value| {
            Some(WindowsProcess {
                pid: u32::try_from(value["ProcessId"].as_u64()?).ok()?,
                parent_pid: u32::try_from(value["ParentProcessId"].as_u64()?).ok()?,
                name: value["Name"].as_str()?.to_owned(),
                executable_path: value["ExecutablePath"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

#[cfg(windows)]
fn descendants(root: u32, inventory: &[WindowsProcess]) -> Vec<WindowsProcess> {
    let mut pids = std::collections::HashSet::from([root]);
    let mut result = Vec::new();
    loop {
        let mut changed = false;
        for process in inventory {
            if !pids.contains(&process.pid) && pids.contains(&process.parent_pid) {
                pids.insert(process.pid);
                result.push(process.clone());
                changed = true;
            }
        }
        if !changed {
            return result;
        }
    }
}

#[cfg(windows)]
struct ObservedWindowsProcess {
    process: WindowsProcess,
    handle: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
fn open_live_process(process: WindowsProcess) -> Option<ObservedWindowsProcess> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};

    // SAFETY: the PID came from Win32 immediately before this call. A non-null
    // result is a new owned synchronization handle, immune to PID reuse.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, process.pid) };
    (!handle.is_null()).then(|| ObservedWindowsProcess {
        process,
        handle: unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle) },
    })
}

#[cfg(windows)]
fn process_is_running(handle: &std::os::windows::io::OwnedHandle) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{Foundation::WAIT_TIMEOUT, System::Threading::WaitForSingleObject};

    // SAFETY: the handle remains owned and valid for the duration of the call.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) == WAIT_TIMEOUT }
}

#[cfg(windows)]
async fn observe_packaged_process_tree(
    host_pid: u32,
    resources: &Path,
    workspace: &Path,
) -> Vec<ObservedWindowsProcess> {
    let normalize = |path: &str| {
        path.strip_prefix(r"\\?\")
            .unwrap_or(path)
            .replace('/', r"\")
            .to_lowercase()
    };
    let resources = normalize(&std::fs::canonicalize(resources).unwrap().to_string_lossy());
    let workspace = normalize(&std::fs::canonicalize(workspace).unwrap().to_string_lossy());
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(Path::new(&resources).join("resources/tools/manifest.json")).unwrap(),
    )
    .unwrap();
    let expected_hash = |id: &str| {
        manifest["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["id"] == id)
            .and_then(|tool| tool["sha256"].as_str())
            .unwrap()
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let inventory = tokio::task::spawn_blocking(windows_process_inventory)
            .await
            .unwrap();
        let observed: Vec<_> = descendants(host_pid, &inventory)
            .into_iter()
            .filter_map(open_live_process)
            .collect();
        let named = |name: &str| {
            observed.iter().find(|process| {
                process.process.name.eq_ignore_ascii_case(name)
                    && process_is_running(&process.handle)
            })
        };
        if let (Some(av1an), Some(svt)) = (named("av1an.exe"), named("SvtAv1EncApp.exe")) {
            for (process, expected) in [
                (av1an, expected_hash("av1an")),
                (svt, expected_hash("svt-av1")),
            ] {
                let path = process
                    .process
                    .executable_path
                    .as_deref()
                    .expect("WMI must expose the package tool executable path");
                let path = normalize(path);
                assert!(
                    path.starts_with(&workspace) || path.starts_with(&resources),
                    "the verified package tool escaped its resource and recovery roots: {path}"
                );
                assert_eq!(
                    format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap())),
                    expected,
                    "the live executable differs from the packaged tool receipt: {path}"
                );
            }
            return observed;
        }
        let last: Vec<_> = observed
            .into_iter()
            .map(|process| process.process)
            .collect();
        assert!(
            tokio::time::Instant::now() < deadline,
            "the real packaged av1an/SVT process tree must become observable; last descendants: {last:#?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(windows)]
async fn assert_process_tree_dead(host_pid: u32, observed: &[ObservedWindowsProcess]) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let inventory = tokio::task::spawn_blocking(windows_process_inventory)
                .await
                .unwrap();
            if observed
                .iter()
                .all(|process| !process_is_running(&process.handle))
                && descendants(host_pid, &inventory).is_empty()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("hard host termination must leave no live owned descendants");
}

#[cfg(windows)]
async fn run_crash_child(root: &Path) {
    if let Some(resources) = std::env::var_os("JESSES_TEST_TOOL_RESOURCES") {
        media_runtime::configure_bundled_tools(PathBuf::from(resources))
            .expect("the crash child uses one absolute verified resource root");
    }
    let input = root.join("source.mkv");
    let destination = root.join("published.mkv");
    let mut encode = request(&input, &destination, VideoEncoder::SvtAv1);
    encode.settings.preset = 2;
    let manager = JobManager::open(root.join("logs"), root.join("history")).await;
    manager.ready().await.unwrap();
    let submitted = manager.start_encode(encode).await.unwrap();
    let checkpoint = wait_for(&manager, &submitted.id, |job| {
        job.recovery
            .as_ref()
            .is_some_and(|recovery| recovery.completed_frames > 0)
    })
    .await;
    assert!(!checkpoint.state.is_terminal(), "{checkpoint:#?}");
    let workspace = PathBuf::from(&checkpoint.recovery.as_ref().unwrap().workspace);
    let attempts: Vec<_> = std::fs::read_dir(&workspace)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("attempt-") && name.ends_with(".partial.mkv"))
        })
        .collect();
    assert_eq!(
        attempts.len(),
        1,
        "one exact mux attempt must be owned by the recovery workspace"
    );
    let temporary = root.join("crash-ready.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec(&json!({
            "id": checkpoint.id,
            "recovery": checkpoint.recovery,
            "attempt": attempts[0],
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::rename(temporary, root.join("crash-ready.json")).unwrap();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(windows)]
struct CrashHost(std::process::Child);

#[cfg(windows)]
impl Drop for CrashHost {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "requires the packaged Windows av1an, VapourSynth/L-SMASH and SVT-AV1 tools"]
async fn windows_hard_crash_cleanup_reopens_rejects_changed_tool_and_recovers() {
    if std::env::var_os(CRASH_CHILD_ENV).is_some() {
        let root = PathBuf::from(std::env::var_os(CRASH_ROOT_ENV).unwrap());
        run_crash_child(&root).await;
        unreachable!();
    }

    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    let destination = fixture.0.join("published.mkv");
    synthesize_frames(&input, false, CRASH_FRAMES).await;
    let source_bytes = std::fs::read(&input).unwrap();
    let resources = PathBuf::from(
        std::env::var_os("JESSES_TEST_TOOL_RESOURCES")
            .expect("the Windows package gate must name its verified resource root"),
    );

    let executable = std::env::current_exe().unwrap();
    let mut host = CrashHost(
        Command::new(executable)
            .args([
                "--exact",
                "windows_hard_crash_cleanup_reopens_rejects_changed_tool_and_recovers",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CRASH_CHILD_ENV, "1")
            .env(CRASH_ROOT_ENV, &fixture.0)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let host_pid = host.0.id();
    let ready_path = fixture.0.join("crash-ready.json");
    tokio::time::timeout(Duration::from_secs(240), async {
        loop {
            if ready_path.exists() {
                return;
            }
            assert!(
                host.0.try_wait().unwrap().is_none(),
                "the crash host exited before saving a reusable checkpoint"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the crash child must save progress within four minutes");
    let ready: Value = serde_json::from_slice(&std::fs::read(&ready_path).unwrap()).unwrap();
    let id = ready["id"].as_str().unwrap().to_owned();
    assert!(ready["recovery"]["completedFrames"].as_u64().unwrap() > 0);
    assert!(!destination.exists(), "a partial output was published");

    let workspace = PathBuf::from(ready["recovery"]["workspace"].as_str().unwrap());
    let crashed_attempt = PathBuf::from(ready["attempt"].as_str().unwrap());
    assert!(
        crashed_attempt.starts_with(&workspace),
        "the owned attempt must remain inside its recovery workspace"
    );
    let observed = observe_packaged_process_tree(host_pid, &resources, &workspace).await;
    assert!(
        observed.iter().any(
            |process| process.process.name.eq_ignore_ascii_case("av1an.exe")
                && process_is_running(&process.handle)
        ),
        "the owned av1an leader must remain live at the hard-kill boundary"
    );
    let process_evidence: Vec<_> = observed
        .iter()
        .map(|process| {
            format!(
                "owned descendant before crash: pid={} parent={} name={} path={}",
                process.process.pid,
                process.process.parent_pid,
                process.process.name,
                process
                    .process
                    .executable_path
                    .as_deref()
                    .unwrap_or("<unknown>")
            )
        })
        .collect();

    host.0.kill().expect("TerminateProcess must kill the host");
    let status = host.0.wait().unwrap();
    assert!(
        !status.success(),
        "the host must not perform orderly shutdown"
    );
    assert_process_tree_dead(host_pid, &observed).await;
    for evidence in process_evidence {
        eprintln!("{evidence}");
    }
    assert!(!destination.exists(), "a partial output was published");
    assert!(crashed_attempt.is_file());
    assert_eq!(std::fs::metadata(&crashed_attempt).unwrap().len(), 0);

    let history = fixture.0.join("history");
    let manager = JobManager::open(fixture.0.join("logs"), history).await;
    manager.ready().await.unwrap();
    let interrupted = manager
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.id == id)
        .unwrap();
    assert_eq!(interrupted.state, JobState::Interrupted, "{interrupted:#?}");
    assert!(interrupted.recovery.is_some(), "{interrupted:#?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(manager.list_jobs().await[0].state, JobState::Interrupted);
    assert!(!destination.exists());

    assert_eq!(
        interrupted.recovery.as_ref().unwrap().workspace,
        workspace.to_string_lossy()
    );
    let manifest_path = workspace.join("manifest.json");
    let original_manifest = std::fs::read(&manifest_path).unwrap();
    let baseline: Value = serde_json::from_slice(&original_manifest).unwrap();
    let retained: Vec<_> = baseline["completed"]
        .as_object()
        .unwrap()
        .keys()
        .map(|name| RetainedFile::read(workspace.join("chunks/encode").join(format!("{name}.ivf"))))
        .collect();
    assert!(!retained.is_empty());

    let mut changed_tool = baseline;
    changed_tool["tools"][0]["sha256"] = json!("0".repeat(64));
    std::fs::write(&manifest_path, serde_json::to_vec(&changed_tool).unwrap()).unwrap();
    manager.resume_job(id.clone()).await.unwrap();
    let rejected = wait_for(&manager, &id, |job| job.state.is_terminal()).await;
    assert_eq!(rejected.state, JobState::Failed, "{rejected:#?}");
    assert_eq!(rejected.error.as_ref().unwrap().code, "RECOVERY_INVALID");
    assert!(!destination.exists());
    assert!(workspace.exists());
    for file in &retained {
        file.assert_unchanged();
    }

    std::fs::write(&manifest_path, &original_manifest).unwrap();
    manager.resume_job(id.clone()).await.unwrap();
    let terminal = wait_for(&manager, &id, |job| job.state.is_terminal()).await;
    assert_eq!(terminal.state, JobState::Succeeded, "{terminal:#?}");
    manager.shutdown().await;
    let finished = manager
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.id == id)
        .unwrap();
    assert_eq!(finished.state, JobState::Succeeded, "{finished:#?}");
    assert!(finished.recovery.is_none());
    assert!(!workspace.exists());
    assert!(
        !crashed_attempt.exists(),
        "the exact pre-crash owned mux attempt survived successful recovery"
    );
    for file in &retained {
        assert!(
            !file.path.exists(),
            "settled recovery workspace was retained"
        );
    }
    assert_eq!(
        std::fs::read(&input).unwrap(),
        source_bytes,
        "source changed"
    );
    assert!(destination.exists());
    let probed: Value = serde_json::from_slice(
        &output(
            Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-count_frames",
                    "-show_entries",
                    "stream=nb_read_frames",
                    "-of",
                    "json",
                ])
                .arg(&destination),
        )
        .await,
    )
    .unwrap();
    assert_eq!(
        probed["streams"][0]["nb_read_frames"],
        CRASH_FRAMES.to_string()
    );
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "requires the packaged Windows av1an, VapourSynth/L-SMASH and SVT-AV1 tools"]
async fn windows_locked_attempt_retains_recovery_workspace_after_publication() {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};

    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    let destination = fixture.0.join("published.mkv");
    synthesize_frames(&input, false, 240).await;

    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let submitted = manager
        .start_encode(request(&input, &destination, VideoEncoder::SvtAv1))
        .await
        .unwrap();
    let (workspace, attempt) = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == submitted.id)
                .unwrap();
            assert!(!snapshot.state.is_terminal(), "{snapshot:#?}");
            if let Some(recovery) = snapshot.recovery {
                let workspace = PathBuf::from(recovery.workspace);
                if let Some(attempt) = std::fs::read_dir(&workspace)
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                name.starts_with("attempt-") && name.ends_with(".partial.mkv")
                            })
                    })
                {
                    break (workspace, attempt);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the recovery-scoped final attempt must be reserved while encoding");
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1 | 2) // FILE_SHARE_READ | FILE_SHARE_WRITE; deny deletion.
        .open(&attempt)
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
    assert!(destination.is_file(), "verified output was not published");
    assert_eq!(
        settled.recovery.as_ref().unwrap().workspace,
        workspace.to_string_lossy(),
        "the recovery receipt was cleared after exact attempt cleanup failed"
    );
    assert!(workspace.is_dir());
    assert!(attempt.is_file());
    assert!(settled.logs.iter().any(|line| {
        line.contains("Output succeeded; recovery files were retained")
            && line.contains("could not be removed safely")
    }));

    drop(lock);
}

#[tokio::test]
#[ignore = "requires real av1an, VapourSynth/L-SMASH and 5fish"]
async fn live_pause_continues_exact_output_and_cancels_while_frozen() {
    let fixture = Fixture::new();
    let input = fixture.0.join("pause-source.mkv");
    synthesize(&input, false).await;
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let destination = fixture.0.join("continued.mkv");
    let submitted = manager
        .start_encode(request(&input, &destination, VideoEncoder::SvtAv1FiveFish))
        .await
        .unwrap();
    assert_eq!(
        pause_live(&manager, &submitted.id).await.state,
        JobState::Paused
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(manager.list_jobs().await[0].state, JobState::Paused);
    assert_eq!(
        manager
            .set_job_paused(submitted.id.clone(), false)
            .await
            .unwrap()
            .state,
        JobState::Running
    );
    let finished = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    assert_eq!(finished.state, JobState::Succeeded, "{:?}", finished.error);
    let probed: Value = serde_json::from_slice(
        &output(
            Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-count_frames",
                    "-show_entries",
                    "stream=nb_read_frames",
                    "-of",
                    "json",
                ])
                .arg(&destination),
        )
        .await,
    )
    .unwrap();
    assert_eq!(probed["streams"][0]["nb_read_frames"], FRAMES.to_string());
    let canceled_output = fixture.0.join("cancel-paused.mkv");
    let next = manager
        .start_encode(request(
            &input,
            &canceled_output,
            VideoEncoder::SvtAv1FiveFish,
        ))
        .await
        .unwrap();
    pause_live(&manager, &next.id).await;
    manager.cancel_job(next.id.clone()).await.unwrap();
    let canceled = wait_for(&manager, &next.id, |job| job.state.is_terminal()).await;
    assert_eq!(canceled.state, JobState::Canceled, "{:?}", canceled.error);
    assert!(!canceled_output.exists());
    manager.shutdown().await;
    drop(manager);
    assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
    let moved = fixture.0.join("released-source.mkv");
    std::fs::rename(&input, &moved).unwrap();
    std::fs::rename(&moved, &input).unwrap();
}

struct RetainedFile {
    path: PathBuf,
    bytes: Vec<u8>,
    modified: SystemTime,
}
impl RetainedFile {
    fn read(path: PathBuf) -> Self {
        Self {
            bytes: std::fs::read(&path).unwrap(),
            modified: std::fs::metadata(&path).unwrap().modified().unwrap(),
            path,
        }
    }
    fn assert_unchanged(&self) {
        assert_eq!(
            std::fs::read(&self.path).unwrap(),
            self.bytes,
            "A completed chunk was overwritten"
        );
        assert_eq!(
            std::fs::metadata(&self.path).unwrap().modified().unwrap(),
            self.modified,
            "A completed chunk was re-encoded"
        );
    }
}

async fn reject_changed_receipts(manager: &JobManager, job: &JobSnapshot, input: &Path) {
    let root = Path::new(&job.recovery.as_ref().unwrap().workspace);
    let manifest_path = root.join("manifest.json");
    let original = std::fs::read(&manifest_path).unwrap();
    let mut manifest: Value = serde_json::from_slice(&original).unwrap();
    manifest["settings"]["crf"] = json!(10);
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(
        manager.resume_job(job.id.clone()).await.unwrap_err().code,
        "RECOVERY_INVALID"
    );
    std::fs::write(&manifest_path, &original).unwrap();

    // A forged segment fingerprint must not substitute content from a different
    // source interval: the full decoded-source comparison remains authoritative.
    let baseline: Value = serde_json::from_slice(&original).unwrap();
    if let Some(segments) = baseline["segments"]
        .as_array()
        .filter(|segments| segments.len() > 1)
    {
        let segment = PathBuf::from(segments[0]["path"].as_str().unwrap());
        let bytes = std::fs::read(&segment).unwrap();
        let substituted = std::fs::read(segments[1]["path"].as_str().unwrap()).unwrap();
        std::fs::write(&segment, &substituted).unwrap();
        let mut forged = baseline.clone();
        forged["segments"][0]["length"] = json!(substituted.len());
        forged["segments"][0]["sha256"] = json!(format!("{:x}", Sha256::digest(&substituted)));
        std::fs::write(&manifest_path, serde_json::to_vec(&forged).unwrap()).unwrap();
        manager.resume_job(job.id.clone()).await.unwrap();
        let rejected = wait_for(manager, &job.id, |job| job.state.is_terminal()).await;
        assert_eq!(rejected.state, JobState::Failed, "{rejected:#?}");
        assert_eq!(rejected.error.as_ref().unwrap().code, "RECOVERY_INVALID");
        assert!(
            rejected
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("decoded source frames"),
            "{rejected:#?}"
        );
        std::fs::write(&segment, bytes).unwrap();
        std::fs::write(&manifest_path, &original).unwrap();
    }

    // av1an builds its concat list by enumerating the entire encode directory.
    // An unrecorded file must be rejected before it reaches that native parser.
    let foreign_chunk = root.join("chunks/encode/99999.ivf");
    std::fs::write(&foreign_chunk, [b'X'; 64]).unwrap();
    manager.resume_job(job.id.clone()).await.unwrap();
    let rejected = wait_for(manager, &job.id, |job| job.state.is_terminal()).await;
    assert_eq!(rejected.state, JobState::Failed, "{rejected:#?}");
    assert_eq!(rejected.error.unwrap().code, "RECOVERY_INVALID");
    assert_eq!(std::fs::read(&foreign_chunk).unwrap(), [b'X'; 64]);
    std::fs::remove_file(&foreign_chunk).unwrap();

    // Change only the saved tool fingerprint. Never modify an installed executable.
    let mut manifest: Value = serde_json::from_slice(&original).unwrap();
    manifest["tools"][0]["sha256"] = json!("0".repeat(64));
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    manager.resume_job(job.id.clone()).await.unwrap();
    let rejected = wait_for(manager, &job.id, |job| job.state.is_terminal()).await;
    assert_eq!(rejected.state, JobState::Failed, "{rejected:#?}");
    assert_eq!(rejected.error.unwrap().code, "RECOVERY_INVALID");
    std::fs::write(&manifest_path, &original).unwrap();

    // Appending harmless padding keeps the generated container readable while
    // changing its content fingerprint. Restore its exact bytes afterwards.
    let length = std::fs::metadata(input).unwrap().len();
    std::fs::OpenOptions::new()
        .append(true)
        .open(input)
        .unwrap()
        .write_all(&[0; 16])
        .unwrap();
    manager.resume_job(job.id.clone()).await.unwrap();
    let rejected = wait_for(manager, &job.id, |job| job.state.is_terminal()).await;
    assert_eq!(rejected.state, JobState::Failed, "{rejected:#?}");
    assert_eq!(rejected.error.unwrap().code, "RECOVERY_INVALID");
    std::fs::OpenOptions::new()
        .write(true)
        .open(input)
        .unwrap()
        .set_len(length)
        .unwrap();
}

async fn qualify(encoder: VideoEncoder, transformed: bool) {
    qualify_options(encoder, transformed, None).await;
}

async fn qualify_options(
    encoder: VideoEncoder,
    transformed: bool,
    options: Option<media_core::Av1anOptions>,
) {
    let fixture = Fixture::new();
    let input = fixture.0.join("source's 日本語.mkv");
    let destination = fixture.0.join("resumed.mkv");
    let history = fixture.0.join("history");
    let hdr = encoder == VideoEncoder::SvtAv1Hdr
        && options.is_none_or(|options| options.target_quality.is_none());
    synthesize(&input, hdr).await;
    let source_bytes = std::fs::read(&input).unwrap();
    let mut original_request = request(&input, &destination, encoder);
    original_request.settings.av1an_options = options;
    if options
        .and_then(|options| options.target_quality)
        .is_some_and(|target| target.metric != media_core::Av1anTargetMetric::Vmaf)
    {
        original_request.settings.parameters = vec![
            media_core::EncoderParameter {
                name: "enable-tf".into(),
                value: "0".into(),
            },
            media_core::EncoderParameter {
                name: "aq-mode".into(),
                value: "2".into(),
            },
        ];
    }
    if transformed {
        original_request.settings.framing = VideoFraming {
            crop: CropSettings {
                top: 8,
                right: 8,
                bottom: 4,
                left: 8,
            },
            resize_width: Some(160),
            borders: BorderSettings {
                top: 8,
                right: 8,
                bottom: 16,
                left: 16,
            },
        };
        original_request.settings.audio = vec![AudioTrackSettings {
            stream_index: 1,
            codec: if hdr {
                AudioCodec::Aac
            } else {
                AudioCodec::Opus
            },
            bitrate_kbps: 96,
            channels: AudioChannels::Stereo,
            gain: None,
        }];
    }
    let manager = JobManager::open(fixture.0.join("logs"), history.clone()).await;
    manager.ready().await.unwrap();
    let submitted = manager
        .start_encode(original_request.clone())
        .await
        .unwrap();
    let checkpoint = wait_for(&manager, &submitted.id, |job| {
        job.recovery
            .as_ref()
            .is_some_and(|recovery| recovery.completed_frames > 0)
    })
    .await;
    assert!(
        !checkpoint.state.is_terminal(),
        "No partial checkpoint: {checkpoint:#?}"
    );
    manager.stop_job(submitted.id.clone()).await.unwrap();
    let stopped = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
    assert_eq!(stopped.state, JobState::Stopped, "{stopped:#?}");
    let recovery = stopped.recovery.as_ref().unwrap();
    assert_eq!(recovery.phase, RecoveryPhase::Encoding);
    assert!(recovery.completed_frames > 0 && recovery.completed_frames < FRAMES);
    assert_eq!(recovery.total_frames, FRAMES);
    assert!(!destination.exists());
    let workspace = PathBuf::from(&recovery.workspace);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(workspace.join("manifest.json")).unwrap()).unwrap();
    let retained: Vec<_> = manifest["completed"]
        .as_object()
        .unwrap()
        .keys()
        .map(|name| RetainedFile::read(workspace.join("chunks/encode").join(format!("{name}.ivf"))))
        .collect();
    assert!(!retained.is_empty());
    manager.shutdown().await;
    drop(manager);

    let manager = JobManager::open(fixture.0.join("logs"), history).await;
    manager.ready().await.unwrap();
    let saved = manager
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.id == submitted.id)
        .unwrap();
    assert_eq!(saved.state, JobState::Stopped);
    assert_eq!(saved.request, original_request.source);
    assert_eq!(
        saved.encode_settings.as_ref(),
        Some(&original_request.settings)
    );
    assert_eq!(saved.recovery, stopped.recovery);
    // Older av1an extraction can leave a >1000-byte attachment-only or invalid
    // audio.mkv. Its FFmpeg concatenator blindly opens that path by size. Resume
    // must safely remove this test-owned leftover before concatenating video.
    std::fs::write(workspace.join("chunks/audio.mkv"), [0; 2048]).unwrap();
    if !hdr {
        reject_changed_receipts(&manager, &saved, &input).await;
        for file in &retained {
            file.assert_unchanged();
        }
    }

    // A second job occupies the worker so the resumed job must join the queue
    // tail; concurrent resume requests must admit the original id exactly once.
    let blocker = manager
        .start_encode(request(&input, &fixture.0.join("preceding.mkv"), encoder))
        .await
        .unwrap();
    let active = wait_for(&manager, &blocker.id, |job| job.state == JobState::Running).await;
    assert_eq!(active.state, JobState::Running, "{active:#?}");
    let (first, second) = tokio::join!(
        manager.resume_job(saved.id.clone()),
        manager.resume_job(saved.id.clone())
    );
    assert_eq!(
        usize::from(first.is_ok()) + usize::from(second.is_ok()),
        1,
        "{first:?}; {second:?}"
    );
    let resumed = first.or(second).unwrap();
    assert_eq!(resumed.id, submitted.id);
    assert_eq!(resumed.state, JobState::Queued);
    let queue = manager.list_jobs().await;
    assert_eq!(queue.iter().filter(|job| job.id == submitted.id).count(), 1);
    // Public history is newest-first, the reverse of worker admission order.
    assert!(
        queue.iter().position(|job| job.id == blocker.id).unwrap()
            > queue.iter().position(|job| job.id == submitted.id).unwrap()
    );
    manager.cancel_job(blocker.id.clone()).await.unwrap();
    let canceled = wait_for(&manager, &blocker.id, |job| job.state.is_terminal()).await;
    assert_eq!(canceled.state, JobState::Canceled);

    let result = tokio::time::timeout(Duration::from_secs(240), async {
        let mut observed_reuse = false;
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == submitted.id)
                .unwrap();
            if job.state.is_terminal() {
                return (job, observed_reuse);
            }
            if job.state == JobState::Running {
                for file in &retained {
                    file.assert_unchanged();
                }
                observed_reuse = true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    manager.shutdown().await;
    let (_, observed_reuse) = result.expect("Resume must finish within four minutes");
    // Terminal publication precedes the worker's final cleanup update. Shutdown
    // joins that worker, so inspect the settled snapshot for cleanup assertions.
    let finished = manager
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.id == submitted.id)
        .unwrap();
    assert_eq!(finished.state, JobState::Succeeded, "{finished:#?}");
    if let Some(target) = options.and_then(|options| options.target_quality) {
        let detail = std::fs::read_to_string(
            Path::new(finished.log_path.as_ref().unwrap()).with_extension("av1an.log"),
        )
        .unwrap();
        assert!(
            detail.contains("TQ-Probes") && detail.contains("Final Score"),
            "Actual target search must retain probe and score evidence: {detail}"
        );
        for value in detail
            .lines()
            .filter_map(|line| line.split_once("Final Score=").map(|(_, value)| value))
        {
            let score = value.trim().parse::<f64>().unwrap();
            assert!(
                score.is_finite(),
                "The owned probe must produce a finite score"
            );
            // These exact testsrc2 fixtures at CRF <= 42 score well above these
            // floors. Untagged Y4M matrix conversion incorrectly yielded VMAF
            // about 48 and XPSNR about 9 despite correct encoded pixels.
            match target.metric {
                media_core::Av1anTargetMetric::Vmaf => {
                    assert!(score > 80.0, "VMAF reference pixels changed: {score}");
                }
                media_core::Av1anTargetMetric::Xpsnr => {
                    assert!(score > 20.0, "XPSNR reference pixels changed: {score}");
                }
                _ => {}
            }
        }
        for line in detail
            .lines()
            .filter(|line| line.contains("TQ-Probes") || line.contains("Final Score"))
        {
            eprintln!("{}", line.trim());
        }
    }
    assert!(
        observed_reuse,
        "The test must observe unchanged completed chunks during encoding"
    );
    assert_eq!(finished.request, original_request.source);
    assert_eq!(
        finished.encode_settings.as_ref(),
        Some(&original_request.settings)
    );
    assert!(finished.recovery.is_none());
    assert!(!workspace.exists());
    assert_eq!(
        std::fs::read(&input).unwrap(),
        source_bytes,
        "Source changed"
    );

    let inspected: Value = serde_json::from_slice(
        &output(
            Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-count_frames",
                    "-show_streams",
                    "-show_data_hash",
                    "sha256",
                    "-of",
                    "json",
                ])
                .arg(&destination),
        )
        .await,
    )
    .unwrap();
    let streams = inspected["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 4);
    assert_eq!(
        streams[0]["codec_name"],
        if transformed {
            if hdr { "aac" } else { "opus" }
        } else {
            "pcm_s16le"
        }
    );
    if transformed {
        assert_eq!(streams[0]["channels"], 2);
        assert!(
            finished
                .logs
                .iter()
                .any(|line| line.contains("Validated audio:")),
            "Converted audio must pass decoded timeline verification"
        );
    }
    assert_eq!(streams[0]["tags"]["language"], "jpn");
    assert_eq!(streams[1]["codec_name"], "av1");
    assert_eq!(streams[1]["nb_read_frames"], FRAMES.to_string());
    assert_eq!(streams[1]["r_frame_rate"], "24/1");
    assert_eq!(
        streams[1]["color_primaries"],
        if hdr { "bt2020" } else { "bt709" }
    );
    assert_eq!(
        streams[1]["color_transfer"],
        if hdr { "smpte2084" } else { "bt709" }
    );
    assert_eq!(streams[1]["pix_fmt"], "yuv420p10le");
    let (width, height) = if transformed { (184, 112) } else { (320, 180) };
    assert_eq!(streams[1]["width"], width);
    assert_eq!(streams[1]["height"], height);
    assert_eq!(streams[2]["codec_type"], "subtitle");
    assert_eq!(streams[3]["codec_type"], "attachment");
    assert_eq!(streams[3]["tags"]["filename"], "retained-font.ttf");
    assert_eq!(
        streams[3]["tags"]["mimetype"],
        "application/x-truetype-font"
    );
    assert_eq!(streams[3]["extradata_size"], 4096);
    assert_eq!(
        streams[3]["extradata_hash"],
        format!("SHA256:{:x}", Sha256::digest([b'A'; 4096]))
    );
    let audio = |path: &Path| {
        let mut command = Command::new("ffmpeg");
        command
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(path)
            .args([
                "-map", "0:a:0", "-c", "copy", "-f", "hash", "-hash", "sha256", "-",
            ]);
        command
    };
    if !transformed {
        assert_eq!(
            output(&mut audio(&input)).await,
            output(&mut audio(&destination)).await,
            "Copied audio payload changed"
        );
    } else {
        let decoded = output(
            Command::new("ffmpeg")
                .args(["-v", "error", "-nostdin", "-i"])
                .arg(&destination)
                .args([
                    "-map",
                    "0:v:0",
                    "-frames:v",
                    "1",
                    "-pix_fmt",
                    "yuv420p10le",
                    "-f",
                    "rawvideo",
                    "-",
                ]),
        )
        .await;
        let samples: Vec<_> = decoded
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_le_bytes([v[0], v[1]]))
            .collect();
        assert_eq!(samples.len(), width as usize * height as usize * 3 / 2);
        // The outer corner is far from the picture boundary: black must remain
        // limited-range 10-bit black with neutral chroma after lossy encoding.
        for y in 0..4 {
            for x in 0..4 {
                assert!(samples[y * width as usize + x].abs_diff(64) <= 4);
                for plane in 0..2 {
                    let offset = width as usize * height as usize
                        + plane * width as usize * height as usize / 4;
                    assert!(samples[offset + y * width as usize / 2 + x].abs_diff(512) <= 4);
                }
            }
        }
    }
    let subtitles = |path: &Path| {
        let mut command = Command::new("ffmpeg");
        command
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(path)
            .args(["-map", "0:s:0", "-c", "copy", "-f", "srt", "-"]);
        command
    };
    assert_eq!(
        output(&mut subtitles(&input)).await,
        output(&mut subtitles(&destination)).await,
        "Subtitle timing or text changed"
    );
}

#[tokio::test]
#[ignore = "requires pinned av1an, 5fish, FFmpeg, VapourSynth and L-SMASH"]
async fn fivefish_stop_reopen_reject_mismatches_and_resume_existing_chunks() {
    qualify(VideoEncoder::SvtAv1FiveFish, false).await;
}

#[tokio::test]
#[ignore = "requires pinned av1an, SVT-AV1-HDR, FFmpeg, VapourSynth and L-SMASH"]
async fn hdr_stop_reopen_and_resume_existing_chunks() {
    qualify(VideoEncoder::SvtAv1Hdr, false).await;
}

#[tokio::test]
#[ignore = "requires pinned av1an, 5fish, matching modern FFmpeg/FFprobe, VapourSynth and L-SMASH"]
async fn framed_fivefish_opus_stop_reopen_and_resume_existing_chunks() {
    qualify(VideoEncoder::SvtAv1FiveFish, true).await;
}

#[tokio::test]
#[ignore = "requires pinned av1an, HDR, matching modern FFmpeg/FFprobe, VapourSynth and L-SMASH"]
async fn framed_hdr_aac_stop_reopen_and_resume_existing_chunks() {
    qualify(VideoEncoder::SvtAv1Hdr, true).await;
}

#[tokio::test]
#[ignore = "requires actual av1an, FFMS2, BestSource, FFmpeg and 5fish"]
async fn configured_source_readers_and_fixed_chunks_stop_reopen_and_resume() {
    use media_core::{Av1anChunkMethod, Av1anChunkOrder, Av1anOptions, Av1anSplitMethod};
    for method in [Av1anChunkMethod::Ffms2, Av1anChunkMethod::Bestsource] {
        eprintln!("Qualifying reader {method:?}");
        qualify_options(
            VideoEncoder::SvtAv1FiveFish,
            true,
            Some(Av1anOptions {
                chunk_method: method,
                split_method: Av1anSplitMethod::FixedChunks,
                maximum_chunk_frames: 120,
                minimum_scene_frames: 12,
                scene_downscale_height: None,
                chunk_order: Av1anChunkOrder::Sequential,
                ..Default::default()
            }),
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "requires actual av1an, libvmaf, L-SMASH and 5fish"]
async fn vmaf_target_controls_with_framing_stop_reopen_and_resume() {
    use media_core::{Av1anOptions, Av1anSceneDetection, Av1anTargetQuality};
    qualify_options(
        VideoEncoder::SvtAv1FiveFish,
        true,
        Some(Av1anOptions {
            maximum_chunk_frames: 120,
            minimum_scene_frames: 12,
            scene_detection: Av1anSceneDetection::Fast,
            target_quality: Some(Av1anTargetQuality {
                metric: Default::default(),
                minimum_score_tenths: 920,
                maximum_score_tenths: 990,
                minimum_crf: 20,
                maximum_crf: 42,
                probes: 3,
                probing_rate: 1,
                probe_width: 320,
                probe_height: 180,
            }),
            ..Default::default()
        }),
    )
    .await;
}

#[tokio::test]
#[ignore = "requires actual av1an, libvmaf, L-SMASH, mainline and HDR SVT"]
async fn vmaf_targets_qualify_other_svt_builds_on_sdr() {
    use media_core::{Av1anOptions, Av1anTargetQuality};
    for encoder in [VideoEncoder::SvtAv1, VideoEncoder::SvtAv1Hdr] {
        qualify_options(
            encoder,
            false,
            Some(Av1anOptions {
                maximum_chunk_frames: 240,
                minimum_scene_frames: 24,
                target_quality: Some(Av1anTargetQuality {
                    metric: Default::default(),
                    minimum_score_tenths: 900,
                    maximum_score_tenths: 990,
                    minimum_crf: 20,
                    maximum_crf: 42,
                    probes: 2,
                    probing_rate: 2,
                    probe_width: 320,
                    probe_height: 180,
                }),
                ..Default::default()
            }),
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "requires patched av1an ffmpeg9-passthrough-v1 and 5fish"]
async fn corrected_select_and_hybrid_stop_reopen_resume_exact_source() {
    use media_core::{Av1anChunkMethod, Av1anOptions, Av1anSplitMethod};
    for chunk_method in [Av1anChunkMethod::Select, Av1anChunkMethod::Hybrid] {
        eprintln!("Qualifying corrected reader {chunk_method:?}");
        qualify_options(
            VideoEncoder::SvtAv1FiveFish,
            true,
            Some(Av1anOptions {
                chunk_method,
                split_method: Av1anSplitMethod::FixedChunks,
                maximum_chunk_frames: 120,
                minimum_scene_frames: 12,
                ..Default::default()
            }),
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "requires corrected av1an, vszip/Julek scorer plugins, source readers, and 5fish"]
async fn additional_perceptual_targets_stop_reopen_resume_with_advanced_parameters() {
    use media_core::{
        Av1anChunkMethod, Av1anOptions, Av1anSplitMethod, Av1anTargetMetric, Av1anTargetQuality,
    };
    for (metric, probing_rate, minimum_score_tenths, maximum_score_tenths) in [
        (Av1anTargetMetric::Ssimulacra2, 2, 600, 950),
        (Av1anTargetMetric::Butteraugli, 2, 5, 50),
        (Av1anTargetMetric::Xpsnr, 1, 250, 500),
        (Av1anTargetMetric::Xpsnr, 2, 250, 500),
    ] {
        eprintln!(
            "Qualifying {metric:?}, sampling {probing_rate}, explicit enable-tf=0 and aq-mode=2"
        );
        qualify_options(
            VideoEncoder::SvtAv1FiveFish,
            // Non-VMAF reference readers cannot apply direct filters. Qualify
            // their scoring/recovery with an unfiltered source; direct-filter
            // rejection is covered separately by the dependency tests.
            false,
            Some(Av1anOptions {
                chunk_method: if metric == Av1anTargetMetric::Xpsnr && probing_rate == 1 {
                    Av1anChunkMethod::Select
                } else {
                    Av1anChunkMethod::Lsmash
                },
                split_method: Av1anSplitMethod::FixedChunks,
                maximum_chunk_frames: 120,
                minimum_scene_frames: 12,
                target_quality: Some(Av1anTargetQuality {
                    metric,
                    minimum_score_tenths,
                    maximum_score_tenths,
                    minimum_crf: 20,
                    maximum_crf: 42,
                    probes: 3,
                    probing_rate,
                    probe_width: 320,
                    probe_height: 180,
                }),
                ..Default::default()
            }),
        )
        .await;
    }
}
