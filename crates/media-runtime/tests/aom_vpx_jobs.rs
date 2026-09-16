//! Opt-in native gate: `cargo test -p media-runtime --test aom_vpx_jobs -- --include-ignored`.
//! All sources are synthesized locally; no user media is read or modified.

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_runtime::{
    EncodeRequest, EncodeSettings, JobManager, JobState, RemuxRequest, VideoEncoder,
    supervisor::{CommandSpec, run_capture},
};

struct Fixture(PathBuf);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone aomenc with 10-bit support"]
async fn aom_10bit_left_chroma_raw_input_preserves_four_lossless_frames() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    let destination = fixture.0.join("output.mkv");
    output(
        command("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=128x72:r=24,format=yuv420p10le",
                "-frames:v",
                "4",
                "-vf",
                "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
                "-pix_fmt",
                "yuv420p10le",
                "-chroma_sample_location",
                "left",
                "-c:v",
                "ffv1",
                "-level",
                "3",
            ])
            .arg(&input),
    )
    .await;
    let source_before = std::fs::read(&input).unwrap();

    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: destination.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::AomAv1,
                preset: 8,
                lossless: true,
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == started.id)
                .unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    assert!(completed.logs.iter().any(|line| line.contains("raw I420")));
    assert!(
        completed
            .logs
            .iter()
            .any(|line| line.contains("Verified lossless decoded pixel"))
    );
    assert_eq!(std::fs::read(&input).unwrap(), source_before);
    let frames = output(
        command("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_frames",
                "-show_entries",
                "stream=nb_read_frames",
                "-of",
                "default=nokey=1:noprint_wrappers=1",
                "-i",
            ])
            .arg(&destination),
    )
    .await;
    assert_eq!(String::from_utf8(frames).unwrap().trim(), "4");

    let mut hashes = Vec::new();
    for path in [&input, &destination] {
        hashes.push(
            output(
                command("ffmpeg")
                    .args(["-v", "error", "-xerror", "-i"])
                    .arg(path)
                    .args([
                        "-map",
                        "0:v:0",
                        "-frames:v",
                        "4",
                        "-pix_fmt",
                        "yuv420p10le",
                        "-c:v",
                        "rawvideo",
                        "-f",
                        "hash",
                        "-hash",
                        "sha256",
                        "-",
                    ]),
            )
            .await,
        );
    }
    assert_eq!(hashes[0], hashes[1]);
}

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jesses-aom-vpx-{}-{nonce}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Every file is owned exclusively by this newly created test fixture.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn command(executable: impl AsRef<std::ffi::OsStr>) -> Command {
    // This builder only collects arguments. All native spawns below use the
    // owned supervisor and its explicit Windows inherited-handle list.
    Command::new(executable)
}

async fn output(command: &mut Command) -> Vec<u8> {
    static TOOLS: tokio::sync::OnceCell<Vec<media_runtime::ToolInfo>> =
        tokio::sync::OnceCell::const_new();
    let program = Path::new(command.get_program());
    let executable = if program.is_absolute() {
        program.to_path_buf()
    } else {
        let tools = TOOLS
            .get_or_init(|| Box::pin(media_runtime::get_capabilities()))
            .await;
        let tool = tools
            .iter()
            .find(|tool| tool.id == program.to_string_lossy())
            .unwrap();
        assert!(
            tool.available,
            "Required fixture tool is unavailable: {tool:?}"
        );
        PathBuf::from(tool.path.as_ref().unwrap())
    };
    let spec = CommandSpec {
        executable,
        args: command.get_args().map(std::ffi::OsString::from).collect(),
        cwd: command.get_current_dir().map(Path::to_path_buf),
    };
    assert_eq!(command.get_envs().count(), 0);
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = Box::pin(run_capture(
        &spec,
        cancel,
        8 * 1024 * 1024,
        Duration::from_secs(45),
    ))
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "Native tool failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
