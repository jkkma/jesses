//! Linux real-process evidence for session-owned fixed-image pause controls.
#![cfg(target_os = "linux")]
use media_core::*;
use media_runtime::{
    jobs::JobManager,
    supervisor::{CommandSpec, run_capture},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/image-pause");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Retained {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn args(v: &[&str]) -> Vec<OsString> {
    v.iter().map(OsString::from).collect()
}
async fn tool(a: Vec<OsString>) {
    let (_owner, cancel) = watch::channel(false);
    let r = run_capture(
        &CommandSpec {
            executable: std::env::var_os("JESSES_FFMPEG")
                .unwrap_or_else(|| "ffmpeg".into())
                .into(),
            args: a,
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(90),
    )
    .await
    .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
}
async fn source(root: &Path, large: bool) -> PathBuf {
    let caption = root.join("caption.srt");
    std::fs::write(&caption, "1\n00:00:00,000 --> 00:00:03,000\nSubtitle\n").unwrap();
    let input = root.join("source.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        if large {
            "testsrc2=size=960x540:rate=30:duration=2"
        } else {
            "testsrc2=size=320x180:rate=30:duration=2"
        },
        "-i",
    ]);
    a.push(caption.as_os_str().to_owned());
    a.extend(args(&[
        "-map", "0:v", "-map", "1:s", "-c:v", "ffv1", "-c:s", "srt", "-threads", "2",
    ]));
    a.push(input.as_os_str().to_owned());
    tool(a).await;
    input
}
fn request(input: &Path, output: &Path, format: ImageOutput, implicit: bool) -> ImageRequest {
    if implicit {
        ImageRequest::ImplicitExport {
            inputs: vec![input.to_string_lossy().into_owned()],
            checked: vec![ImageInputTrack {
                input_index: 0,
                stream_index: 1,
            }],
            current_input: 0,
            format,
            output_path: output.to_string_lossy().into_owned(),
            audio: None,
            subtitle_codec: Default::default(),
            metadata: Default::default(),
            trim: None,
            keep_frame_count: true,
        }
    } else {
        ImageRequest::Export {
            input_path: input.to_string_lossy().into_owned(),
            stream_index: 0,
            start_frame: 0,
            frame_count: 0,
            full_source: true,
            format,
            output_path: output.to_string_lossy().into_owned(),
            width: None,
            pixel_format: None,
            quality: None,
            processing: None,
        }
    }
}
async fn prepare(manager: &JobManager, request: ImageRequest) -> CommandEditReview {
    let (_owner, cancel) = watch::channel(false);
    tokio::time::timeout(
        Duration::from_secs(60),
        manager.prepare_command_edit(
            CommandEditRequest::Image {
                request: Box::new(request),
            },
            cancel,
        ),
    )
    .await
    .unwrap()
    .unwrap()
}
fn clean(root: &Path) {
    assert!(
        !std::fs::read_dir(root).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jesses-")),
        "Owned image assets leaked"
    );
}
#[derive(Clone, Debug)]
struct Process {
    pid: u32,
    state: char,
    ticks: u64,
    args: Vec<String>,
}
fn processes(root: &Path) -> Vec<Process> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().filter_map(Result::ok) {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(bytes) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args = bytes
            .split(|b| *b == 0)
            .filter(|v| !v.is_empty())
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .collect::<Vec<_>>();
        if args.is_empty()
            || !(args.iter().any(|arg| arg.contains(root.to_str().unwrap()))
                || std::fs::read_link(entry.path().join("fd/1"))
                    .is_ok_and(|path| path.starts_with(root)))
        {
            continue;
        }
        let Some((state, ticks)) = state(pid) else {
            continue;
        };
        result.push(Process {
            pid,
            state,
            ticks,
            args,
        });
    }
    result
}
fn state(pid: u32) -> Option<(char, u64)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, fields) = text.rsplit_once(") ")?;
    let fields = fields.split_whitespace().collect::<Vec<_>>();
    Some((
        fields[0].chars().next()?,
        fields[11].parse::<u64>().ok()? + fields[12].parse::<u64>().ok()?,
    ))
}
fn terminated(pid: u32) -> bool {
    let text = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => text,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    let Some((_, tail)) = text.rsplit_once(") ") else {
        return false;
    };
    let fields = tail.split_whitespace().collect::<Vec<_>>();
    // A zombie leader can still retain live worker threads. Only the dead
    // one-thread state is harmless; init owns reaping orphan descendants.
    matches!(fields.first(), Some(&"Z" | &"X" | &"x"))
        && fields.get(17).and_then(|value| value.parse::<u32>().ok()) == Some(1)
}
fn encoder(process: &Process) -> bool {
    let name = Path::new(&process.args[0])
        .file_name()
        .unwrap()
        .to_string_lossy();
    name == "ffmpeg"
        && !process
            .args
            .iter()
            .any(|arg| arg == "framemd5" || arg.contains("palettegen="))
        && process.args.iter().any(|arg| arg.contains(".jesses-"))
}
async fn active_encoder(
    root: &Path,
    task: &tokio::task::JoinHandle<Result<CommandEditResult, AppError>>,
) -> Process {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(process) = processes(root)
                .into_iter()
                .find(|process| encoder(process) && process.ticks > 0)
            {
                return process;
            }
            assert!(
                !task.is_finished(),
                "Image finished before its real encoder was observed"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap()
}
async fn stopped(pid: u32) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if state(pid).is_some_and(|(state, _)| state == 'T') {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}
fn owned_bytes(root: &Path) -> u64 {
    std::fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".jesses-"))
        .map(|entry| {
            if entry.path().is_dir() {
                std::fs::read_dir(entry.path())
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.metadata().ok())
                    .map(|metadata| metadata.len())
                    .sum::<u64>()
            } else {
                entry.metadata().unwrap().len()
            }
        })
        .sum()
}
async fn pause_proof(manager: &JobManager, id: &str, process: &Process, root: &Path) {
    manager.set_command_edit_paused(id, true).await.unwrap();
    manager.set_command_edit_paused(id, true).await.unwrap();
    stopped(process.pid).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let before = state(process.pid).unwrap();
    let bytes = owned_bytes(root);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        owned_bytes(root),
        bytes,
        "Image output advanced while its encoder was paused"
    );
    assert_eq!(
        state(process.pid),
        Some(before),
        "Paused FFmpeg consumed CPU or resumed"
    );
    eprintln!(
        "session {id}: pid {} stopped, CPU ticks {} unchanged for 200 ms; originally {} at {} ticks",
        process.pid, before.1, process.state, process.ticks
    );
}
fn start(
    manager: &Arc<JobManager>,
    review: &CommandEditReview,
    arguments: String,
) -> tokio::task::JoinHandle<Result<CommandEditResult, AppError>> {
    let manager = manager.clone();
    let id = review.id.clone();
    tokio::spawn(async move {
        let (_owner, cancel) = watch::channel(false);
        manager
            .confirm_command_edit(CommandEditConfirmation { id, arguments }, cancel)
            .await
    })
}
async fn success(
    task: tokio::task::JoinHandle<Result<CommandEditResult, AppError>>,
) -> ImageResult {
    let CommandEditResult::Image { result } = tokio::time::timeout(Duration::from_secs(60), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
    else {
        panic!()
    };
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires real FFmpeg/FFprobe and Linux process control"]
async fn ordinary_edited_and_implicit_formats_pause_real_encoder_and_resume_to_verified_outputs() {
    let f = Fixture::new();
    let input = source(&f.0, true).await;
    let original = std::fs::read(&input).unwrap();
    let manager = Arc::new(JobManager::open(f.0.join("logs"), f.0.join("history")).await);
    for route in ["ordinary", "edited", "implicit"] {
        for format in [
            ImageOutput::Gif,
            ImageOutput::PngSequence,
            ImageOutput::JpegSequence,
        ] {
            let output = f.0.join(format!(
                "{route}-{format:?}.{}",
                if format == ImageOutput::Gif {
                    "gif"
                } else {
                    "dir"
                }
            ));
            let review = prepare(
                &manager,
                request(&input, &output, format, route == "implicit"),
            )
            .await;
            let arguments = if route == "edited" {
                format!("{} && true", review.arguments)
            } else {
                review.arguments.clone()
            };
            let task = start(&manager, &review, arguments);
            let process = active_encoder(&f.0, &task).await;
            pause_proof(&manager, &review.id, &process, &f.0).await;
            assert!(!task.is_finished());
            assert!(!output.exists());
            manager
                .set_command_edit_paused(&review.id, false)
                .await
                .unwrap();
            manager
                .set_command_edit_paused(&review.id, false)
                .await
                .unwrap();
            let result = success(task).await;
            assert_eq!(
                (result.frame_count, result.width, result.height),
                (60, 960, 540)
            );
            assert!(output.exists());
            assert_eq!(
                manager
                    .set_command_edit_paused(&review.id, true)
                    .await
                    .unwrap_err()
                    .code,
                "COMMAND_EDIT_EXPIRED"
            );
            clean(&f.0);
            eprintln!("{route} {format:?}: paused and resumed 60 actual frames");
        }
    }
    assert_eq!(std::fs::read(input).unwrap(), original);
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires real FFmpeg/FFprobe and Linux process control"]
async fn paused_stop_navigation_and_shutdown_join_children_without_publishing() {
    let f = Fixture::new();
    let input = source(&f.0, true).await;
    for action in ["stop", "abort", "navigation", "shutdown"] {
        let manager = Arc::new(
            JobManager::open(
                f.0.join(format!("logs-{action}")),
                f.0.join(format!("history-{action}")),
            )
            .await,
        );
        let output = f.0.join(format!("cancel-{action}"));
        let review = prepare(
            &manager,
            request(
                &input,
                &output,
                ImageOutput::PngSequence,
                action == "navigation",
            ),
        )
        .await;
        let (owner, cancel) = watch::channel(false);
        let worker = manager.clone();
        let id = review.id.clone();
        let arguments = format!("{} && true", review.arguments);
        let task = tokio::spawn(async move {
            worker
                .confirm_command_edit(CommandEditConfirmation { id, arguments }, cancel)
                .await
        });
        let process = active_encoder(&f.0, &task).await;
        pause_proof(&manager, &review.id, &process, &f.0).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            match action {
                "stop" => manager.cancel_command_edit(&review.id).await.unwrap(),
                "abort" => {
                    owner.send_replace(true);
                }
                "navigation" => manager.cancel_command_reviews().await.unwrap(),
                _ => manager.shutdown().await,
            }
        })
        .await
        .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert!(terminated(process.pid), "Paused child survived {action}");
        assert!(!output.exists());
        clean(&f.0);
        manager.shutdown().await;
        eprintln!("paused {action}: child joined and owned output removed");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires real FFmpeg/FFprobe and Linux process control"]
async fn held_pause_is_session_local_and_stale_batch_id_cannot_pause_next_file_or_queue_job() {
    let f = Fixture::new();
    let input = source(&f.0, false).await;
    let manager = Arc::new(JobManager::open(f.0.join("logs"), f.0.join("history")).await);
    let first = f.0.join("held");
    let review = prepare(
        &manager,
        request(&input, &first, ImageOutput::PngSequence, false),
    )
    .await;
    manager
        .set_command_edit_paused(&review.id, true)
        .await
        .unwrap();
    let task = start(&manager, &review, review.arguments.clone());
    let process = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(process) = processes(&f.0)
                .into_iter()
                .find(|process| Path::new(&process.args[0]).file_name().unwrap() == "ffmpeg")
            {
                return process;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    stopped(process.pid).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!task.is_finished());
    manager
        .set_command_edit_paused(&review.id, false)
        .await
        .unwrap();
    assert_eq!(success(task).await.frame_count, 60);
    let next = f.0.join("next");
    let second = prepare(
        &manager,
        request(&input, &next, ImageOutput::JpegSequence, true),
    )
    .await;
    assert_ne!(review.id, second.id);
    assert_eq!(
        manager
            .set_command_edit_paused(&review.id, true)
            .await
            .unwrap_err()
            .code,
        "COMMAND_EDIT_EXPIRED"
    );
    assert_eq!(
        success(start(&manager, &second, second.arguments.clone()))
            .await
            .frame_count,
        60
    );
    let (_owner, cancel) = watch::channel(false);
    let ordinary = manager
        .prepare_command_edit(
            CommandEditRequest::Encode {
                request: Box::new(EncodeRequest {
                    source: RemuxRequest {
                        input_path: input.to_string_lossy().into_owned(),
                        output_path: f.0.join("ordinary.mkv").to_string_lossy().into_owned(),
                        stream_indices: vec![0],
                    },
                    settings: EncodeSettings {
                        video_mode: VideoMode::Copy,
                        ..Default::default()
                    },
                }),
            },
            cancel,
        )
        .await
        .unwrap();
    assert_eq!(
        manager
            .set_command_edit_paused(&ordinary.id, true)
            .await
            .unwrap_err()
            .code,
        "COMMAND_EDIT_PAUSE_UNSUPPORTED"
    );
    manager.cancel_command_edit(&ordinary.id).await.unwrap();
    let (_owner, cancel) = watch::channel(false);
    let ordinary = manager
        .prepare_command_edit(
            CommandEditRequest::Encode {
                request: Box::new(EncodeRequest {
                    source: RemuxRequest {
                        input_path: input.to_string_lossy().into_owned(),
                        output_path: f
                            .0
                            .join("ordinary-paused.mkv")
                            .to_string_lossy()
                            .into_owned(),
                        stream_indices: vec![0],
                    },
                    settings: EncodeSettings {
                        video_mode: VideoMode::Copy,
                        ..Default::default()
                    },
                }),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    let CommandEditResult::Encode { job } = manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: ordinary.id.clone(),
                arguments: ordinary.arguments.replace("'-i'", "'-re' '-i'"),
            },
            cancel,
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        manager
            .set_command_edit_paused(&ordinary.id, true)
            .await
            .unwrap_err()
            .code,
        "COMMAND_EDIT_EXPIRED"
    );
    let process = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(process) = processes(&f.0).into_iter().find(encoder) {
                break process;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        manager
            .set_job_paused(job.id.clone(), true)
            .await
            .unwrap()
            .state,
        JobState::Paused
    );
    stopped(process.pid).await;
    let before = state(process.pid);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(state(process.pid), before);
    manager.set_job_paused(job.id.clone(), false).await.unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|current| current.id == job.id)
                .unwrap();
            if job.state.is_terminal() {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    // Queue snapshots become terminal before the worker drops its last private
    // preparation objects. Shutdown is the existing worker-join boundary.
    manager.shutdown().await;
    clean(&f.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires qualified R79/QTGMC, FFmpeg/FFprobe and Linux process control"]
async fn qtgmc_normal_and_edited_pipelines_pause_both_real_stages_and_resume_or_cancel() {
    let f = Fixture::new();
    let input = source(&f.0, false).await;
    for (edited, cancelled) in [(false, false), (true, false), (false, true), (true, true)] {
        let manager = Arc::new(
            JobManager::open(
                f.0.join(format!("logs-{edited}-{cancelled}")),
                f.0.join(format!("history-{edited}-{cancelled}")),
            )
            .await,
        );
        let output = f.0.join(format!("qtgmc-{edited}-{cancelled}"));
        let mut value = request(&input, &output, ImageOutput::PngSequence, false);
        if let ImageRequest::Export { processing, .. } = &mut value {
            *processing = Some(ImageProcessingSettings {
                temporal: Some(TemporalSettings {
                    qtgmc: Some(QtgmcSettings {
                        preset: QtgmcPreset::Placebo,
                        mode: DeinterlaceMode::Frame,
                        field_order: FieldOrder::TopFirst,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        let review = prepare(&manager, value).await;
        let arguments = if edited {
            format!("{} && true", review.arguments)
        } else {
            review.arguments.clone()
        };
        let task = start(&manager, &review, arguments);
        let stages = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let current = processes(&f.0);
                let producer = current.iter().find(|process| {
                    Path::new(&process.args[0])
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("vspipe")
                        && process.ticks > 0
                });
                let consumer = current.iter().find(|process| {
                    Path::new(&process.args[0]).file_name().unwrap() == "ffmpeg"
                        && process.args.iter().any(|arg| arg == "ffv1")
                        && process.ticks > 0
                });
                if let (Some(producer), Some(consumer)) = (producer, consumer) {
                    break vec![producer.clone(), consumer.clone()];
                }
                assert!(
                    !task.is_finished(),
                    "QTGMC finished before its actual two stages could be observed"
                );
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        manager
            .set_command_edit_paused(&review.id, true)
            .await
            .unwrap();
        for process in &stages {
            stopped(process.pid).await;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        let before = stages
            .iter()
            .map(|process| state(process.pid).unwrap())
            .collect::<Vec<_>>();
        let bytes = owned_bytes(&f.0);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            owned_bytes(&f.0),
            bytes,
            "QTGMC output advanced while suspended"
        );
        for (process, before) in stages.iter().zip(before) {
            assert_eq!(state(process.pid), Some(before));
            eprintln!(
                "QTGMC stage pid {}: stopped with CPU ticks {} unchanged",
                process.pid, before.1
            );
        }
        if cancelled {
            tokio::time::timeout(
                Duration::from_secs(5),
                manager.cancel_command_edit(&review.id),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_secs(5), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            assert!(!output.exists());
        } else {
            manager
                .set_command_edit_paused(&review.id, false)
                .await
                .unwrap();
            assert_eq!(success(task).await.frame_count, 60);
            assert!(output.exists());
        }
        for process in stages {
            assert!(
                terminated(process.pid),
                "QTGMC child {} still has executable work: {:?}",
                process.pid,
                state(process.pid)
            );
        }
        clean(&f.0);
        manager.shutdown().await;
        eprintln!(
            "QTGMC edited={edited} canceled={cancelled}: both stages halted; output progress stopped; cleanup joined"
        );
    }
}
