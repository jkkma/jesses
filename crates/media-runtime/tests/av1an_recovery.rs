//! Opt-in real av1an stop/reopen/resume gate. Run with `--ignored --test-threads=1`.
//! Fixtures and changed receipts are test-owned; installed tools are never edited.
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
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
    let attachment = path.parent().unwrap().join("retained-font.ttf");
    std::fs::write(&attachment, [b'A'; 4096]).unwrap();
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
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-frames:v",
        &FRAMES.to_string(),
        "-t",
        "30",
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
        command.args(["-c:v", "ffv1", "-level", "3"]);
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
            stream_indices: vec![1, 0, 2],
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

async fn qualify(encoder: VideoEncoder) {
    let fixture = Fixture::new();
    let input = fixture.0.join("source's 日本語.mkv");
    let destination = fixture.0.join("resumed.mkv");
    let history = fixture.0.join("history");
    let hdr = encoder == VideoEncoder::SvtAv1Hdr;
    synthesize(&input, hdr).await;
    let source_bytes = std::fs::read(&input).unwrap();
    let original_request = request(&input, &destination, encoder);
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
    assert_eq!(streams.len(), 3);
    assert_eq!(streams[0]["codec_name"], "pcm_s16le");
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
    assert_eq!(streams[2]["codec_type"], "attachment");
    assert_eq!(streams[2]["tags"]["filename"], "retained-font.ttf");
    assert_eq!(
        streams[2]["tags"]["mimetype"],
        "application/x-truetype-font"
    );
    assert_eq!(streams[2]["extradata_size"], 4096);
    assert_eq!(
        streams[2]["extradata_hash"],
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
    assert_eq!(
        output(&mut audio(&input)).await,
        output(&mut audio(&destination)).await,
        "Copied audio payload changed"
    );
}

#[tokio::test]
#[ignore = "requires pinned av1an, 5fish, FFmpeg, VapourSynth and L-SMASH"]
async fn fivefish_stop_reopen_reject_mismatches_and_resume_existing_chunks() {
    qualify(VideoEncoder::SvtAv1FiveFish).await;
}

#[tokio::test]
#[ignore = "requires pinned av1an, SVT-AV1-HDR, FFmpeg, VapourSynth and L-SMASH"]
async fn hdr_stop_reopen_and_resume_existing_chunks() {
    qualify(VideoEncoder::SvtAv1Hdr).await;
}
