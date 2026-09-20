//! Opt-in packaged Windows gate for collision handling and source locks:
//! `cargo test -p media-runtime --test windows_output_safety -- --ignored --nocapture`.
//! Every source is synthesized inside a unique test-owned directory.

#![cfg(windows)]

use std::{
    collections::HashSet,
    ffi::OsString,
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_core::{AppError, RecoveryPhase, StandaloneRecoveryPhase, ToolInfo};
use media_runtime::{
    EncodeBackend, EncodeRequest, EncodeSettings, JobManager, JobSnapshot, JobState, RemuxRequest,
    VideoEncoder, get_capabilities,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::os::windows::fs::OpenOptionsExt;

const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const FILE_SHARE_DELETE: u32 = 4;

static TOOLS: tokio::sync::OnceCell<Vec<ToolInfo>> = tokio::sync::OnceCell::const_new();

#[derive(Clone, Copy, Debug)]
struct Workflow {
    name: &'static str,
    backend: EncodeBackend,
    encoder: VideoEncoder,
    preset: u8,
}

const WORKFLOWS: [Workflow; 2] = [
    Workflow {
        name: "standalone-x264",
        backend: EncodeBackend::Standalone,
        encoder: VideoEncoder::X264,
        // Slow enough for the acceptance test to observe the live source lock.
        preset: 9,
    },
    Workflow {
        name: "av1an-mainline-svt",
        backend: EncodeBackend::Av1an,
        encoder: VideoEncoder::SvtAv1,
        preset: 0,
    },
];

struct Fixture(PathBuf);

impl Fixture {
    async fn new(label: &str) -> Self {
        verified_package_tools().await;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "jesses-output-safety-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn case(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir(&path).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Failed output-safety fixture retained at {}",
                self.0.display()
            );
        } else {
            // Passing cases already removed their subdirectories after shutdown.
            std::fs::remove_dir_all(&self.0)
                .expect("the empty test-owned fixture root must be removable");
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
        "ffmpeg.exe"
            | "ffprobe.exe"
            | "x264.exe"
            | "svtav1encapp.exe"
            | "av1an.exe"
            | "python.exe"
            | "vspipe.exe"
    )
}

async fn observe_owned_workers(expected_name: &str) -> Vec<WindowsProcess> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let inventory = tokio::task::spawn_blocking(process_inventory)
                .await
                .unwrap();
            let workers = descendants(std::process::id(), &inventory)
                .into_iter()
                .filter(is_package_worker)
                .collect::<Vec<_>>();
            if workers
                .iter()
                .any(|process| process.name.eq_ignore_ascii_case(expected_name))
            {
                eprintln!(
                    "OBSERVED_OWNED_WORKERS {}",
                    workers
                        .iter()
                        .map(|process| format!(
                            "{}:{}<-{}",
                            process.name, process.pid, process.parent_pid
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                return workers;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("the live encode must expose its test-owned {expected_name} process")
    })
}

async fn assert_owned_workers_exited(observed: &[WindowsProcess]) {
    let observed_identities = observed
        .iter()
        .map(|process| (process.pid, process.name.to_ascii_lowercase()))
        .collect::<HashSet<_>>();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let inventory = tokio::task::spawn_blocking(process_inventory)
                .await
                .unwrap();
            let exact_survivors = inventory
                .iter()
                .filter(|process| {
                    observed_identities.contains(&(process.pid, process.name.to_ascii_lowercase()))
                })
                .collect::<Vec<_>>();
            let owned_survivors = descendants(std::process::id(), &inventory)
                .into_iter()
                .filter(is_package_worker)
                .collect::<Vec<_>>();
            if exact_survivors.is_empty() && owned_survivors.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("manager.shutdown must reap the observed test-owned package workers");
}

async fn assert_no_owned_workers() {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let inventory = tokio::task::spawn_blocking(process_inventory)
                .await
                .unwrap();
            let survivors = descendants(std::process::id(), &inventory)
                .into_iter()
                .filter(is_package_worker)
                .collect::<Vec<_>>();
            if survivors.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("manager.shutdown must leave no test-owned package workers");
}

#[derive(Debug)]
struct SourceSeal {
    bytes: Vec<u8>,
    sha256: String,
    modified: SystemTime,
}

impl SourceSeal {
    fn read(path: &Path) -> Self {
        let bytes = std::fs::read(path).unwrap();
        Self {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
            modified: std::fs::metadata(path).unwrap().modified().unwrap(),
        }
    }

    fn assert_unchanged(&self, path: &Path) {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(
            bytes,
            self.bytes,
            "Source bytes changed: {}",
            path.display()
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            self.sha256,
            "Source SHA-256 changed: {}",
            path.display()
        );
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            self.modified,
            "Source mtime changed: {}",
            path.display()
        );
    }
}

async fn verified_package_tools() -> &'static [ToolInfo] {
    TOOLS
        .get_or_init(|| async {
            let root = PathBuf::from(
                std::env::var_os("JESSES_TEST_TOOL_RESOURCES")
                    .expect("set JESSES_TEST_TOOL_RESOURCES to the packaged portable root"),
            );
            assert!(
                root.is_absolute(),
                "The package resource root must be absolute"
            );
            media_runtime::configure_bundled_tools(root.clone())
                .expect("the gate uses one absolute verified package resource root");
            let tools = get_capabilities().await;
            let tool_root = std::fs::canonicalize(root.join("resources/tools"))
                .expect("the package tools directory must exist");
            for id in ["ffmpeg", "ffprobe", "x264", "svt-av1", "av1an"] {
                let tool = tools
                    .iter()
                    .find(|tool| tool.id == id)
                    .unwrap_or_else(|| panic!("The package manifest must include {id}"));
                assert!(tool.available, "Packaged tool is unavailable: {tool:#?}");
                let path = std::fs::canonicalize(tool.path.as_ref().unwrap()).unwrap();
                assert!(
                    path.starts_with(&tool_root),
                    "{id} escaped the verified package: {}",
                    path.display()
                );
                assert!(
                    tool.version
                        .as_ref()
                        .is_some_and(|version| !version.is_empty()),
                    "{id} has no verified revision: {tool:#?}"
                );
                eprintln!(
                    "PACKAGE_TOOL {id} | {} | {}",
                    tool.version.as_deref().unwrap(),
                    path.display()
                );
            }
            tools
        })
        .await
}

async fn tool_path(id: &str) -> PathBuf {
    PathBuf::from(
        verified_package_tools()
            .await
            .iter()
            .find(|tool| tool.id == id && tool.available)
            .and_then(|tool| tool.path.as_ref())
            .unwrap_or_else(|| panic!("verified package tool {id} is unavailable")),
    )
}

async fn capture(executable: PathBuf, args: Vec<OsString>) -> Vec<u8> {
    let spec = CommandSpec {
        executable,
        args,
        cwd: None,
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = run_capture(&spec, cancel, 4 * 1024 * 1024, Duration::from_secs(45))
        .await
        .unwrap();
    assert!(
        result.status.success(),
        "Packaged fixture tool failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

async fn synthesize(path: &Path, frames: u32) {
    let frame_count = frames.to_string();
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=640x360:rate=24000/1001,format=yuv420p10le,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-frames:v",
        &frame_count,
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-pix_fmt",
        "yuv420p10le",
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
    .collect();
    args.push(path.as_os_str().to_owned());
    capture(tool_path("ffmpeg").await, args).await;
}

fn request(workflow: Workflow, source: &Path, destination: &Path) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: source.to_string_lossy().into_owned(),
            output_path: destination.to_string_lossy().into_owned(),
            stream_indices: vec![0],
        },
        settings: EncodeSettings {
            backend: workflow.backend,
            encoder: workflow.encoder,
            workers: 1,
            video_stream_index: 0,
            crf: 32,
            preset: workflow.preset,
            ..Default::default()
        },
    }
}

async fn wait_for(
    manager: &JobManager,
    id: &str,
    ready: impl Fn(&JobSnapshot) -> bool,
) -> JobSnapshot {
    let result = tokio::time::timeout(Duration::from_secs(180), async {
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
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if result.is_err() {
        manager.shutdown().await;
    }
    result.expect("packaged Windows job must progress within three minutes")
}

async fn wait_for_owned_matroska(manager: &JobManager, id: &str) -> PathBuf {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if let Some(path) = job
                .logs
                .iter()
                .find_map(|line| line.strip_prefix("Owned temporary Matroska: "))
                .map(PathBuf::from)
            {
                assert!(
                    path.is_file(),
                    "Owned temporary was not created: {}",
                    path.display()
                );
                assert!(!job.state.is_terminal(), "{job:#?}");
                return path;
            }
            assert!(!job.state.is_terminal(), "{job:#?}");
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("execution must create its owned Matroska after destination preflight")
}

async fn reject(manager: &JobManager, request: EncodeRequest, code: &str) -> AppError {
    let started = manager.start_encode(request).await.unwrap();
    let failed = wait_for(manager, &started.id, |job| job.state.is_terminal()).await;
    assert_eq!(failed.state, JobState::Failed, "{failed:#?}");
    let error = failed
        .error
        .clone()
        .expect("failed job must retain its exact error");
    assert_eq!(error.code, code, "{failed:#?}");
    error
}

fn assert_no_owned_output(root: &Path) {
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                assert!(
                    !name.contains(".partial.") && !name.starts_with(".jesses-"),
                    "Owned output was left behind: {}",
                    entry.path().display()
                );
            }
        }
    }
}

fn remove_after_shutdown(path: &Path) {
    std::fs::remove_dir_all(path).unwrap_or_else(|error| {
        panic!(
            "manager.shutdown must release every fixture handle at {}: {error}",
            path.display()
        )
    });
}

#[tokio::test]
#[ignore = "requires the verified packaged FFmpeg, FFprobe, x264, mainline SVT-AV1, av1an and VapourSynth runtime"]
async fn existing_destinations_and_source_aliases_fail_closed() {
    let fixture = Fixture::new("existing").await;
    for workflow in WORKFLOWS {
        let case = fixture.case(workflow.name);
        let source = case.join("CaseFold Source.mkv");
        synthesize(&source, 24).await;
        let seal = SourceSeal::read(&source);

        let exact_manager = JobManager::new(case.join("exact-logs"));
        let exact = reject(
            &exact_manager,
            request(workflow, &source, &source),
            "SOURCE_OUTPUT_COLLISION",
        )
        .await;
        assert!(exact.message.contains("destination must differ"));
        exact_manager.shutdown().await;
        seal.assert_unchanged(&source);

        let casefold_path = case.join("casefold source.MKV");
        let casefold_manager = JobManager::new(case.join("casefold-logs"));
        let casefold = reject(
            &casefold_manager,
            request(workflow, &source, &casefold_path),
            "SOURCE_OUTPUT_COLLISION",
        )
        .await;
        assert!(casefold.message.contains("destination must differ"));
        casefold_manager.shutdown().await;
        seal.assert_unchanged(&source);

        let hardlink = case.join("hardlink source alias.mkv");
        std::fs::hard_link(&source, &hardlink).unwrap();
        let hardlink_manager = JobManager::new(case.join("hardlink-logs"));
        let hardlink_error = reject(
            &hardlink_manager,
            request(workflow, &source, &hardlink),
            "OUTPUT_EXISTS",
        )
        .await;
        assert!(hardlink_error.message.contains("never replaced"));
        hardlink_manager.shutdown().await;
        seal.assert_unchanged(&source);
        assert_eq!(std::fs::read(&hardlink).unwrap(), seal.bytes);

        let existing = case.join("existing destination.mkv");
        let sentinel = format!("existing-destination-sentinel:{}", workflow.name).into_bytes();
        std::fs::write(&existing, &sentinel).unwrap();
        let existing_manager = JobManager::new(case.join("existing-logs"));
        let existing_error = reject(
            &existing_manager,
            request(workflow, &source, &existing),
            "OUTPUT_EXISTS",
        )
        .await;
        assert!(existing_error.message.contains("never replaced"));
        existing_manager.shutdown().await;
        assert_eq!(std::fs::read(&existing).unwrap(), sentinel);
        seal.assert_unchanged(&source);
        assert_no_owned_output(&case);
        eprintln!(
            "ACCEPTED {} existing/source-alias collisions | source-sha256={}",
            workflow.name, seal.sha256
        );
        remove_after_shutdown(&case);
    }
}

#[tokio::test]
#[ignore = "requires live packaged standalone x264 and mainline SVT-AV1/av1an workflows"]
async fn late_collision_preserves_sentinel_and_live_source_lock() {
    let fixture = Fixture::new("late").await;
    for workflow in WORKFLOWS {
        let case = fixture.case(workflow.name);
        let blocker_source = case.join("live lock source.mkv");
        let target_source = case.join("late collision source.mkv");
        synthesize(&blocker_source, 480).await;
        synthesize(&target_source, 48).await;
        let blocker_seal = SourceSeal::read(&blocker_source);
        let target_seal = SourceSeal::read(&target_source);
        let blocker_output = case.join("canceled blocker output.mkv");
        let target_output = case.join("late collision output.mkv");
        let manager = JobManager::new(case.join("logs"));
        let admitted = manager
            .enqueue_encode_batch(vec![
                request(workflow, &blocker_source, &blocker_output),
                request(workflow, &target_source, &target_output),
            ])
            .await
            .expect("both destinations are absent during batch preflight");
        assert_eq!(admitted.len(), 2);
        let running = wait_for(&manager, &admitted[0].id, |job| {
            job.state == JobState::Running
        })
        .await;
        assert_eq!(running.state, JobState::Running, "{running:#?}");
        let expected_worker = match workflow.encoder {
            VideoEncoder::X264 => "x264.exe",
            VideoEncoder::SvtAv1 => "SvtAv1EncApp.exe",
            _ => unreachable!("bounded output-safety workflow matrix"),
        };
        let observed_workers = observe_owned_workers(expected_worker).await;

        assert_eq!(std::fs::read(&blocker_source).unwrap(), blocker_seal.bytes);
        let write_attempt = OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&blocker_source);
        assert!(
            write_attempt.is_err(),
            "The runtime must deny writes while retaining a readable source handle"
        );
        assert!(
            std::fs::remove_file(&blocker_source).is_err(),
            "The runtime must deny deletion while the job owns the source"
        );

        let sentinel = format!("late-collision-sentinel:{}", workflow.name).into_bytes();
        std::fs::write(&target_output, &sentinel).unwrap();
        manager.cancel_job(admitted[0].id.clone()).await.unwrap();
        let canceled = wait_for(&manager, &admitted[0].id, |job| job.state.is_terminal()).await;
        assert_eq!(canceled.state, JobState::Canceled, "{canceled:#?}");
        let failed = wait_for(&manager, &admitted[1].id, |job| job.state.is_terminal()).await;
        assert_eq!(failed.state, JobState::Failed, "{failed:#?}");
        let error = failed.error.as_ref().unwrap();
        assert_eq!(error.code, "OUTPUT_EXISTS", "{failed:#?}");
        assert!(error.message.contains("never replaced"));
        manager.shutdown().await;
        assert_owned_workers_exited(&observed_workers).await;

        assert_eq!(std::fs::read(&target_output).unwrap(), sentinel);
        assert!(
            !blocker_output.exists(),
            "Canceled blocker published output"
        );
        blocker_seal.assert_unchanged(&blocker_source);
        target_seal.assert_unchanged(&target_source);
        assert_no_owned_output(&case);
        eprintln!(
            "ACCEPTED {} late collision | code={} | source-sha256={}",
            workflow.name, error.code, target_seal.sha256
        );
        remove_after_shutdown(&case);
    }
}

#[tokio::test]
#[ignore = "requires packaged x264 and mainline SVT-AV1/av1an to finish encode, mux and validation"]
async fn final_publication_collision_preserves_destination_and_recovery() {
    let fixture = Fixture::new("publication").await;
    for workflow in WORKFLOWS {
        let case = fixture.case(workflow.name);
        let source = case.join("publication race source.mkv");
        let destination = case.join("late publication destination.mkv");
        synthesize(&source, 120).await;
        let seal = SourceSeal::read(&source);
        let manager = JobManager::new(case.join("logs"));
        let mut encode = request(workflow, &source, &destination);
        encode.settings.preset = match workflow.encoder {
            VideoEncoder::X264 => 5,
            VideoEncoder::SvtAv1 => 12,
            _ => unreachable!("bounded output-safety workflow matrix"),
        };
        let submitted = manager.start_encode(encode).await.unwrap();
        let owned_matroska = wait_for_owned_matroska(&manager, &submitted.id).await;
        assert!(
            !destination.exists(),
            "Destination must be absent after execution preflight"
        );

        let sentinel = format!("final-publication-sentinel:{}", workflow.name).into_bytes();
        std::fs::write(&destination, &sentinel).unwrap();
        let failed = wait_for(&manager, &submitted.id, |job| job.state.is_terminal()).await;
        assert_eq!(failed.state, JobState::Failed, "{failed:#?}");
        let error = failed.error.as_ref().unwrap();
        assert_eq!(error.code, "OUTPUT_EXISTS", "{failed:#?}");
        assert!(error.message.contains("never replaced"));
        assert!(failed.logs.iter().any(|line| line.starts_with("Mux log: ")));
        assert!(failed.logs.iter().any(|line| {
            line.contains("Decoding the completed output to verify exact frame count")
        }));

        let recovery_workspace = match workflow.backend {
            EncodeBackend::Standalone => {
                assert!(failed.recovery.is_none(), "{failed:#?}");
                let recovery = failed.standalone_recovery.as_ref().unwrap();
                assert_eq!(recovery.phase, StandaloneRecoveryPhase::Finalizing);
                assert_eq!(recovery.completed_frames, 120);
                assert_eq!(recovery.total_frames, 120);
                PathBuf::from(&recovery.workspace)
            }
            EncodeBackend::Av1an => {
                assert!(failed.standalone_recovery.is_none(), "{failed:#?}");
                let recovery = failed.recovery.as_ref().unwrap();
                assert_eq!(recovery.phase, RecoveryPhase::Finalizing);
                assert_eq!(recovery.completed_frames, 120);
                assert_eq!(recovery.total_frames, 120);
                PathBuf::from(&recovery.workspace)
            }
        };
        assert!(recovery_workspace.join("manifest.json").is_file());
        manager.shutdown().await;
        assert_no_owned_workers().await;

        assert_eq!(std::fs::read(&destination).unwrap(), sentinel);
        assert!(
            !owned_matroska.exists(),
            "Failed publication left its scratch Matroska behind"
        );
        assert!(
            recovery_workspace.is_dir(),
            "Verified finalizing recovery must remain resumable"
        );
        seal.assert_unchanged(&source);
        eprintln!(
            "ACCEPTED {} final publication collision | code={} | recovery={} | source-sha256={}",
            workflow.name,
            error.code,
            recovery_workspace.display(),
            seal.sha256
        );
        remove_after_shutdown(&case);
    }
}

#[tokio::test]
#[ignore = "requires the verified packaged x264 and mainline SVT workflow configuration"]
async fn source_locked_against_reading_fails_actionably_without_publication() {
    let fixture = Fixture::new("read-lock").await;
    for workflow in WORKFLOWS {
        let case = fixture.case(workflow.name);
        let source = case.join("externally locked source.mkv");
        let destination = case.join("must not publish.mkv");
        synthesize(&source, 24).await;
        let seal = SourceSeal::read(&source);
        let external_lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&source)
            .unwrap();
        let manager = JobManager::new(case.join("logs"));
        let error = reject(
            &manager,
            request(workflow, &source, &destination),
            "FILE_UNREADABLE",
        )
        .await;
        assert!(
            error.message.contains("source could not be opened safely"),
            "The failure must tell the user which safe source operation failed: {error:#?}"
        );
        let canonical_source = std::fs::canonicalize(&source).unwrap();
        assert_eq!(
            error.path.as_deref(),
            Some(canonical_source.to_string_lossy().as_ref())
        );
        manager.shutdown().await;
        drop(external_lock);

        assert!(!destination.exists(), "Unreadable source published output");
        seal.assert_unchanged(&source);
        assert_no_owned_output(&case);
        eprintln!(
            "ACCEPTED {} external read lock | code={} | source-sha256={}",
            workflow.name, error.code, seal.sha256
        );
        remove_after_shutdown(&case);
    }
}
