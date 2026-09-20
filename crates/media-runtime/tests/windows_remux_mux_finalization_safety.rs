//! Windows remux and multi-source mux finalization safety gates.
//!
//! The restart-reporting regression runs with the normal test suite. The
//! actual-tool gates are opt-in and use only the installed package resources:
//! `cargo test -p media-runtime --test windows_remux_mux_finalization_safety -- --ignored --nocapture`.

#![cfg(windows)]

use std::{
    collections::HashSet,
    ffi::OsString,
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_core::{AppError, JobSnapshot, MuxRequest, MuxSource, MuxTrack, ToolInfo};
use media_runtime::{
    JobManager, JobState, RemuxRequest, get_capabilities,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::os::windows::fs::OpenOptionsExt;

const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
static TOOLS: tokio::sync::OnceCell<Vec<ToolInfo>> = tokio::sync::OnceCell::const_new();

struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "jesses-remux-mux-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Failed remux/mux fixture retained at {}", self.0.display());
        } else {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

#[derive(Debug)]
struct SourceSeal {
    bytes: Vec<u8>,
    modified: SystemTime,
}

impl SourceSeal {
    fn read(path: &Path) -> Self {
        Self {
            bytes: std::fs::read(path).unwrap(),
            modified: std::fs::metadata(path).unwrap().modified().unwrap(),
        }
    }

    fn assert_unchanged(&self, path: &Path) {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(
            bytes,
            self.bytes,
            "source bytes changed: {}",
            path.display()
        );
        assert_eq!(
            Sha256::digest(&bytes),
            Sha256::digest(&self.bytes),
            "source digest changed: {}",
            path.display()
        );
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            self.modified,
            "source mtime changed: {}",
            path.display()
        );
    }
}

fn remux_request(source: &Path, output: &Path) -> RemuxRequest {
    RemuxRequest {
        input_path: source.to_string_lossy().into_owned(),
        output_path: output.to_string_lossy().into_owned(),
        stream_indices: vec![0],
    }
}

fn mux_request(video: &Path, audio: &Path, output: &Path) -> MuxRequest {
    MuxRequest {
        sources: vec![
            MuxSource {
                id: "video".into(),
                input_path: video.to_string_lossy().into_owned(),
            },
            MuxSource {
                id: "audio".into(),
                input_path: audio.to_string_lossy().into_owned(),
            },
        ],
        tracks: vec![
            MuxTrack {
                source_id: "video".into(),
                stream_index: 0,
                title: None,
                language: None,
                default: Some(true),
                forced: None,
            },
            MuxTrack {
                source_id: "audio".into(),
                stream_index: 0,
                title: None,
                language: Some("eng".into()),
                default: Some(true),
                forced: None,
            },
        ],
        metadata_source_id: "video".into(),
        chapters_source_id: None,
        output_path: output.to_string_lossy().into_owned(),
    }
}

fn history_snapshot(
    id: &str,
    request: RemuxRequest,
    mux_request: Option<MuxRequest>,
) -> JobSnapshot {
    JobSnapshot {
        id: id.into(),
        state: JobState::Finalizing,
        request,
        mux_request,
        encode_settings: None,
        recovery: None,
        standalone_recovery: None,
        progress_seconds: Some(1.0),
        duration_seconds: Some(2.0),
        logs: vec!["Final container conversion started.".into()],
        error: None,
        log_path: None,
    }
}

fn write_history(directory: &Path, jobs: &[JobSnapshot]) {
    std::fs::create_dir(directory).unwrap();
    std::fs::write(
        directory.join("jobs.json"),
        serde_json::to_vec(&serde_json::json!({"version": 1, "jobs": jobs})).unwrap(),
    )
    .unwrap();
}

fn partials(root: &Path, id: &str, extension: &str) -> [PathBuf; 2] {
    [
        root.join(format!(".jesses-{id}.partial.mkv")),
        root.join(format!(".jesses-{id}-container.partial.{extension}")),
    ]
}

#[tokio::test]
async fn interrupted_remux_and_mux_report_retained_temporary_pathnames_without_touching_them() {
    let fixture = Fixture::new("reopen-report");
    let source = fixture.0.join("source.mkv");
    let second = fixture.0.join("second.mkv");
    let remux_output = fixture.0.join("remux.mp4");
    let mux_output = fixture.0.join("mux.mov");
    std::fs::write(&source, b"original remux source").unwrap();
    std::fs::write(&second, b"original mux source").unwrap();
    std::fs::write(&remux_output, b"late destination remains untouched").unwrap();
    let source_seal = SourceSeal::read(&source);
    let second_seal = SourceSeal::read(&second);

    let remux_id = "101-20260920-1";
    let mux_id = "101-20260920-2";
    let old_interrupted_id = "101-20260920-3";
    let remux_paths = partials(&fixture.0, remux_id, "mp4");
    let mux_paths = partials(&fixture.0, mux_id, "mov");
    for (index, path) in remux_paths.iter().chain(&mux_paths).enumerate() {
        std::fs::write(path, format!("retained interrupted attempt {index}")).unwrap();
    }
    let mux = mux_request(&source, &second, &mux_output);
    let mut previously_restored =
        history_snapshot(remux_id, remux_request(&source, &remux_output), None);
    previously_restored.state = JobState::Interrupted;
    previously_restored.error = Some(AppError::new(
        "JOB_INTERRUPTED",
        "The previous session ended before completion was recorded.",
        Some(remux_output.to_string_lossy().into_owned()),
    ));
    let mut old_interrupted_without_leftovers = history_snapshot(
        old_interrupted_id,
        remux_request(&source, &fixture.0.join("old-interrupted.mp4")),
        None,
    );
    old_interrupted_without_leftovers.state = JobState::Interrupted;
    old_interrupted_without_leftovers.error = Some(AppError::new(
        "JOB_INTERRUPTED",
        "The previous session ended before completion was recorded. Saved encoder work can be resumed after verification.",
        Some(
            fixture
                .0
                .join("old-interrupted.mp4")
                .to_string_lossy()
                .into_owned(),
        ),
    ));
    write_history(
        &fixture.0.join("history"),
        &[
            previously_restored,
            history_snapshot(
                mux_id,
                RemuxRequest {
                    input_path: source.to_string_lossy().into_owned(),
                    output_path: mux_output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                Some(mux),
            ),
            old_interrupted_without_leftovers,
        ],
    );

    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let restored = manager.list_jobs().await;
    assert_eq!(restored.len(), 3);
    for (id, expected) in [(remux_id, &remux_paths), (mux_id, &mux_paths)] {
        let job = restored.iter().find(|job| job.id == id).unwrap();
        assert_eq!(job.state, JobState::Interrupted, "{job:#?}");
        assert_eq!(
            job.error.as_ref().map(|error| error.code.as_str()),
            Some("JOB_INTERRUPTED")
        );
        assert!(
            job.error
                .as_ref()
                .unwrap()
                .message
                .contains("former temporary pathnames"),
            "{job:#?}"
        );
        assert!(
            job.error
                .as_ref()
                .unwrap()
                .message
                .contains("cannot be resumed"),
            "{job:#?}"
        );
        assert!(
            !job.error
                .as_ref()
                .unwrap()
                .message
                .contains("Saved encoder work"),
            "{job:#?}"
        );
        for path in expected {
            let reported = std::fs::canonicalize(path).unwrap();
            assert!(
                job.logs
                    .iter()
                    .any(|line| line.contains(reported.to_string_lossy().as_ref())),
                "retained pathname was not reported: {}\n{job:#?}",
                path.display()
            );
            assert!(path.is_file(), "restore removed {}", path.display());
        }
        assert_eq!(
            manager.resume_job(job.id.clone()).await.unwrap_err().code,
            "JOB_RESUME_UNAVAILABLE"
        );
    }
    let migrated = restored
        .iter()
        .find(|job| job.id == old_interrupted_id)
        .unwrap();
    assert!(
        migrated
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("cannot be resumed"),
        "{migrated:#?}"
    );
    assert!(
        !migrated
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Saved encoder work"),
        "{migrated:#?}"
    );
    assert_eq!(
        manager
            .resume_job(migrated.id.clone())
            .await
            .unwrap_err()
            .code,
        "JOB_RESUME_UNAVAILABLE"
    );
    manager.shutdown().await;
    drop(manager);

    let reopened = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    reopened.ready().await.unwrap();
    let restored_again = reopened.list_jobs().await;
    assert!(
        restored_again
            .iter()
            .all(|job| job.state == JobState::Interrupted)
    );
    for (id, expected) in [(remux_id, &remux_paths), (mux_id, &mux_paths)] {
        let job = restored_again.iter().find(|job| job.id == id).unwrap();
        for path in expected {
            let name = path.file_name().unwrap().to_string_lossy();
            assert_eq!(
                job.logs
                    .iter()
                    .filter(|line| line.contains(name.as_ref()))
                    .count(),
                1,
                "reopening duplicated or lost the retained pathname report: {job:#?}"
            );
        }
    }
    reopened.shutdown().await;
    drop(reopened);
    source_seal.assert_unchanged(&source);
    second_seal.assert_unchanged(&second);
    assert_eq!(
        std::fs::read(&remux_output).unwrap(),
        b"late destination remains untouched"
    );
    for path in remux_paths.iter().chain(&mux_paths) {
        assert!(path.is_file());
    }
}

async fn verified_package_tools() -> &'static [ToolInfo] {
    TOOLS
        .get_or_init(|| async {
            let root = PathBuf::from(
                std::env::var_os("JESSES_TEST_TOOL_RESOURCES")
                    .expect("set JESSES_TEST_TOOL_RESOURCES to the packaged portable root"),
            );
            assert!(root.is_absolute());
            media_runtime::configure_bundled_tools(root.clone()).unwrap();
            let tools = get_capabilities().await;
            let expected_root = std::fs::canonicalize(root.join("resources/tools")).unwrap();
            for id in ["ffmpeg", "ffprobe"] {
                let tool = tools.iter().find(|tool| tool.id == id).unwrap();
                assert!(tool.available, "{tool:#?}");
                let path = std::fs::canonicalize(tool.path.as_ref().unwrap()).unwrap();
                assert!(path.starts_with(&expected_root), "{}", path.display());
                assert!(tool.version.as_ref().is_some_and(|value| !value.is_empty()));
            }
            tools
        })
        .await
}

async fn executable(id: &str) -> PathBuf {
    PathBuf::from(
        verified_package_tools()
            .await
            .iter()
            .find(|tool| tool.id == id)
            .and_then(|tool| tool.path.as_ref())
            .unwrap(),
    )
}

async fn tool(id: &str, args: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(
        &CommandSpec {
            executable: executable(id).await,
            args,
            cwd: None,
        },
        cancel,
        8 * 1024 * 1024,
        Duration::from_secs(120),
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

async fn synthesize(video: &Path, audio: &Path) {
    let mut video_args = [
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=640x360:rate=60:duration=30",
        "-c:v",
        "mpeg4",
        "-q:v",
        "2",
        "-an",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    video_args.push(video.as_os_str().to_owned());
    tool("ffmpeg", video_args).await;
    let mut audio_args = [
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=30",
        "-c:a",
        "aac",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    audio_args.push(audio.as_os_str().to_owned());
    tool("ffmpeg", audio_args).await;
}

async fn wait_for(
    manager: &JobManager,
    id: &str,
    ready: impl Fn(&JobSnapshot) -> bool,
) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(180), async {
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
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("remux/mux job must progress within three minutes")
}

async fn wait_for_path(manager: &JobManager, id: &str, path: &Path) {
    let snapshot = wait_for(manager, id, |_| path.is_file()).await;
    assert!(
        path.is_file(),
        "job ended before {} appeared: {snapshot:#?}",
        path.display()
    );
}

fn assert_no_partials(root: &Path) {
    let leftovers = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".partial."))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert!(leftovers.is_empty(), "unreported partials: {leftovers:#?}");
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
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice::<Value>(&output.stdout)
        .unwrap()
        .as_array()
        .unwrap()
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

async fn assert_no_owned_workers() {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let inventory = tokio::task::spawn_blocking(process_inventory)
                .await
                .unwrap();
            let mut owners = HashSet::from([std::process::id()]);
            let mut workers = Vec::new();
            loop {
                let mut changed = false;
                for process in &inventory {
                    if !owners.contains(&process.pid) && owners.contains(&process.parent_pid) {
                        owners.insert(process.pid);
                        if matches!(
                            process.name.to_ascii_lowercase().as_str(),
                            "ffmpeg.exe" | "ffprobe.exe"
                        ) {
                            workers.push(process.clone());
                        }
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            if workers.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("shutdown/reopen left an owned FFmpeg or FFprobe descendant");
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg/FFprobe and JESSES_AAC_LOOP_SEAM_INPUT"]
async fn real_aac_loop_seam_passes_only_after_complete_packet_and_audio_validation() {
    verified_package_tools().await;
    let fixture = Fixture::new("aac-loop-seam");
    let reference = PathBuf::from(
        std::env::var_os("JESSES_AAC_LOOP_SEAM_INPUT")
            .expect("set JESSES_AAC_LOOP_SEAM_INPUT to the read-only real loop fixture"),
    );
    assert!(reference.is_absolute() && reference.is_file());
    let reference_seal = SourceSeal::read(&reference);
    let source = fixture.0.join("single-loop-seam.mkv");
    let output = fixture.0.join("looped.mp4");
    let mut fixture_args = ["-v", "error", "-nostdin", "-i"]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    fixture_args.push(reference.as_os_str().to_owned());
    fixture_args.extend(
        ["-t", "50", "-map", "0:0", "-map", "0:1", "-c", "copy"]
            .into_iter()
            .map(OsString::from),
    );
    fixture_args.push(source.as_os_str().to_owned());
    tool("ffmpeg", fixture_args).await;
    let seal = SourceSeal::read(&source);

    let manager = JobManager::new(fixture.0.join("logs"));
    let mut request = remux_request(&source, &output);
    request.stream_indices = vec![0, 1];
    let started = manager.start_remux(request).await.unwrap();
    let terminal = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
    assert_eq!(terminal.state, JobState::Succeeded, "{terminal:#?}");
    assert!(output.is_file() && std::fs::metadata(&output).unwrap().len() > 0);
    manager.shutdown().await;
    assert_no_owned_workers().await;
    assert_no_partials(&fixture.0);
    seal.assert_unchanged(&source);
    reference_seal.assert_unchanged(&reference);
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg/FFprobe and JESSES_AAC_LOOP_SEAM_INPUT"]
async fn accumulated_real_aac_loop_gaps_remain_fail_closed() {
    verified_package_tools().await;
    let fixture = Fixture::new("aac-accumulated-gaps");
    let source = PathBuf::from(
        std::env::var_os("JESSES_AAC_LOOP_SEAM_INPUT")
            .expect("set JESSES_AAC_LOOP_SEAM_INPUT to the read-only repeated-loop fixture"),
    );
    assert!(source.is_absolute() && source.is_file());
    let seal = SourceSeal::read(&source);
    let output = fixture.0.join("rejected.mp4");
    let mut request = remux_request(&source, &output);
    request.stream_indices = vec![0, 1];
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager.start_remux(request).await.unwrap();
    let terminal = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
    assert_eq!(terminal.state, JobState::Failed, "{terminal:#?}");
    assert_eq!(
        terminal.error.as_ref().map(|error| error.code.as_str()),
        Some("AUDIO_TIMELINE_INVALID"),
        "repeated timestamp gaps must not be hidden by the AAC duration exception: {terminal:#?}"
    );
    assert!(!output.exists());
    manager.shutdown().await;
    assert_no_owned_workers().await;
    assert_no_partials(&fixture.0);
    seal.assert_unchanged(&source);
}

#[tokio::test]
#[ignore = "requires the packaged Windows FFmpeg and FFprobe tools"]
async fn locked_existing_and_late_destinations_fail_closed_for_remux_and_mux() {
    verified_package_tools().await;
    let fixture = Fixture::new("destinations");
    let video = fixture.0.join("video.mkv");
    let audio = fixture.0.join("audio.mkv");
    synthesize(&video, &audio).await;
    let video_seal = SourceSeal::read(&video);
    let audio_seal = SourceSeal::read(&audio);
    let manager = JobManager::new(fixture.0.join("logs"));

    for (label, mux) in [("remux", false), ("mux", true)] {
        let existing = fixture.0.join(format!("existing-{label}.mp4"));
        std::fs::write(&existing, b"existing destination sentinel").unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&existing)
            .unwrap();
        let started = if mux {
            manager
                .start_mux(mux_request(&video, &audio, &existing))
                .await
                .unwrap()
        } else {
            manager
                .start_remux(remux_request(&video, &existing))
                .await
                .unwrap()
        };
        let failed = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
        assert_eq!(failed.state, JobState::Failed, "{failed:#?}");
        assert_eq!(failed.error.unwrap().code, "OUTPUT_EXISTS");
        assert_eq!(
            std::fs::read(&existing).unwrap(),
            b"existing destination sentinel"
        );
        drop(lock);

        let late = fixture.0.join(format!("late-{label}.mp4"));
        let started = if mux {
            manager
                .start_mux(mux_request(&video, &audio, &late))
                .await
                .unwrap()
        } else {
            manager
                .start_remux(remux_request(&video, &late))
                .await
                .unwrap()
        };
        let primary = fixture
            .0
            .join(format!(".jesses-{}.partial.mkv", started.id));
        wait_for_path(&manager, &started.id, &primary).await;
        std::fs::write(&late, b"late destination sentinel").unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&late)
            .unwrap();
        let failed = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
        assert_eq!(failed.state, JobState::Failed, "{failed:#?}");
        assert_eq!(failed.error.unwrap().code, "OUTPUT_EXISTS");
        assert_eq!(std::fs::read(&late).unwrap(), b"late destination sentinel");
        drop(lock);
    }
    manager.shutdown().await;
    assert_no_owned_workers().await;
    assert_no_partials(&fixture.0);
    video_seal.assert_unchanged(&video);
    audio_seal.assert_unchanged(&audio);
}

#[tokio::test]
#[ignore = "requires the packaged Windows FFmpeg and FFprobe tools"]
async fn dual_locked_remux_siblings_are_both_reported_after_safe_publication() {
    verified_package_tools().await;
    let fixture = Fixture::new("dual-locked-cleanup");
    let video = fixture.0.join("video.mkv");
    let audio = fixture.0.join("audio.mkv");
    synthesize(&video, &audio).await;
    let seal = SourceSeal::read(&video);
    let output = fixture.0.join("published.mp4");
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager
        .start_remux(remux_request(&video, &output))
        .await
        .unwrap();
    let primary = fixture
        .0
        .join(format!(".jesses-{}.partial.mkv", started.id));
    let final_container = fixture
        .0
        .join(format!(".jesses-{}-container.partial.mp4", started.id));
    wait_for_path(&manager, &started.id, &primary).await;
    let primary_lock = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&primary)
        .unwrap();
    wait_for_path(&manager, &started.id, &final_container).await;
    let final_lock = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&final_container)
        .unwrap();

    let terminal = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
    assert_eq!(terminal.state, JobState::Succeeded, "{terminal:#?}");
    manager.shutdown().await;
    let settled = manager
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.id == started.id)
        .unwrap();
    assert_eq!(settled.state, JobState::Succeeded, "{settled:#?}");
    assert_eq!(
        settled.error.as_ref().map(|error| error.code.as_str()),
        Some("OUTPUT_CLEANUP_FAILED")
    );
    for path in [&primary, &final_container] {
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(
            settled.logs.iter().any(|line| line.contains(name.as_ref())),
            "retained cleanup pathname was not reported: {}\n{settled:#?}",
            path.display()
        );
        assert!(path.is_file());
    }
    assert!(output.is_file() && std::fs::metadata(&output).unwrap().len() > 0);
    seal.assert_unchanged(&video);
    assert_no_owned_workers().await;

    drop(final_lock);
    drop(primary_lock);
    std::fs::remove_file(final_container).unwrap();
    std::fs::remove_file(primary).unwrap();
}

#[tokio::test]
#[ignore = "requires the packaged Windows FFmpeg and FFprobe tools"]
async fn cancel_final_container_attempt_then_reopen_cleans_remux_and_mux_safely() {
    verified_package_tools().await;
    let fixture = Fixture::new("cancel-reopen");
    let video = fixture.0.join("video.mkv");
    let audio = fixture.0.join("audio.mkv");
    synthesize(&video, &audio).await;
    let video_seal = SourceSeal::read(&video);
    let audio_seal = SourceSeal::read(&audio);
    let logs = fixture.0.join("logs");
    let history = fixture.0.join("history");
    let manager = JobManager::open(logs.clone(), history.clone()).await;
    manager.ready().await.unwrap();

    let mut ids = Vec::new();
    for (label, mux) in [("remux", false), ("mux", true)] {
        let output = fixture.0.join(format!("canceled-{label}.mp4"));
        let started = if mux {
            manager
                .start_mux(mux_request(&video, &audio, &output))
                .await
                .unwrap()
        } else {
            manager
                .start_remux(remux_request(&video, &output))
                .await
                .unwrap()
        };
        let final_attempt = fixture
            .0
            .join(format!(".jesses-{}-container.partial.mp4", started.id));
        wait_for_path(&manager, &started.id, &final_attempt).await;
        manager.cancel_job(started.id.clone()).await.unwrap();
        let canceled = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
        assert_eq!(canceled.state, JobState::Canceled, "{canceled:#?}");
        assert!(
            !output.exists(),
            "cancellation published {}",
            output.display()
        );
        ids.push(started.id);
    }
    manager.shutdown().await;
    assert_no_owned_workers().await;
    assert_no_partials(&fixture.0);
    drop(manager);

    let reopened = JobManager::open(logs, history).await;
    reopened.ready().await.unwrap();
    let jobs = reopened.list_jobs().await;
    for id in ids {
        let job = jobs.iter().find(|job| job.id == id).unwrap();
        assert_eq!(job.state, JobState::Canceled, "{job:#?}");
        assert_eq!(
            reopened.resume_job(job.id.clone()).await.unwrap_err().code,
            "JOB_RESUME_UNAVAILABLE"
        );
    }
    reopened.shutdown().await;
    assert_no_owned_workers().await;
    drop(reopened);
    video_seal.assert_unchanged(&video);
    audio_seal.assert_unchanged(&audio);
    assert_no_partials(&fixture.0);
}
