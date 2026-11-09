//! Actual source raw graphs run once, with guarded spool publication.
use media_core::*;
use media_runtime::{
    jobs::{JobManager, run_image_job},
    supervisor::{CommandSpec, run_capture},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::watch;
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(Into::into).collect()
}
async fn ffmpeg(values: Vec<OsString>) {
    let (_owner, cancel) = watch::channel(false);
    let result = run_capture(
        &CommandSpec {
            executable: std::env::var_os("JESSES_FFMPEG").unwrap().into(),
            args: values,
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn executions(marker: &str) -> usize {
    let log = std::env::var_os("JESSES_TEST_FILTER_INVOCATIONS")
        .expect("Use the qualification argv-counting FFmpeg wrapper");
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
        .filter(|arguments| {
            arguments.iter().any(|arg| arg.contains(marker))
                && arguments.iter().any(|arg| {
                    arg.contains(&format!("/source-filter-image/{}/", std::process::id()))
                })
        })
        .count()
}
#[inline(never)]
fn native_future<F: std::future::Future>(make: impl FnOnce() -> F) -> std::pin::Pin<Box<F>> {
    Box::pin(make())
}
async fn finish(manager: &JobManager, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let snapshot = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|entry| entry.id == id)
                .unwrap();
            if snapshot.state.is_terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
fn request(input: &Path, output: &Path, format: ImageOutput, graph: String) -> ImageRequest {
    ImageRequest::Export {
        input_path: input.to_string_lossy().into_owned(),
        output_path: output.to_string_lossy().into_owned(),
        stream_index: 0,
        start_frame: 0,
        frame_count: 0,
        full_source: true,
        format,
        width: None,
        pixel_format: None,
        quality: match format {
            ImageOutput::Gif => Some(128),
            ImageOutput::Jpeg | ImageOutput::JpegSequence => Some(2),
            _ => None,
        },
        processing: Some(ImageProcessingSettings {
            source_filters: true,
            filters: vec![graph],
            ..Default::default()
        }),
    }
}
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg and JESSES_TEST_FILTER_INVOCATIONS argv wrapper"]
async fn source_image_graphs_execute_once_for_normal_held_and_edited_outputs() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-artifacts/source-filter-image")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root).unwrap();
    let input = root.join("source.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=128x96:rate=4:duration=2",
        "-c:v",
        "ffv1",
        "-threads",
        "2",
    ]);
    a.push(input.as_os_str().to_owned());
    ffmpeg(a).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::open(root.join("logs"), root.join("history")).await;
    for held in [false, true] {
        for (format, label) in [
            (ImageOutput::PngSequence, "png"),
            (ImageOutput::JpegSequence, "jpg"),
            (ImageOutput::Gif, "gif"),
        ] {
            let name = format!("{label}-{held}");
            let marker = format!("jesses_single_image_filter_{name}");
            let effects = root.join(format!("{name}-effects.txt"));
            let graph = format!(
                "split[discard][picture];[discard]nullsink;[picture]scale=80:48,fps=2,metadata=mode=add:key={marker}:value=once,metadata=mode=print:file='{}'",
                effects.display()
            );
            let output = root.join(if format == ImageOutput::Gif {
                format!("{name}.gif")
            } else {
                name.clone()
            });
            let request = request(&input, &output, format, graph);
            let result = if held {
                let (_owner, cancel) = watch::channel(false);
                let review = manager
                    .prepare_command_edit(
                        CommandEditRequest::Image {
                            request: Box::new(request),
                        },
                        cancel.clone(),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    executions(&marker),
                    0,
                    "filter must not run before held confirmation"
                );
                assert!(!effects.exists());
                let result = manager
                    .confirm_command_edit(
                        CommandEditConfirmation {
                            id: review.id,
                            arguments: review.arguments,
                        },
                        cancel,
                    )
                    .await
                    .unwrap();
                match result {
                    CommandEditResult::Image { result } => result,
                    _ => panic!("image result"),
                }
            } else {
                let (_owner, cancel) = watch::channel(false);
                run_image_job(request, cancel).await.unwrap()
            };
            assert_eq!(
                (result.width, result.height, result.frame_count),
                (80, 48, 4)
            );
            assert_eq!(
                executions(&marker),
                1,
                "source arbitrary graph executes once including GIF palette generation"
            );
            assert_eq!(
                std::fs::read_to_string(&effects)
                    .unwrap()
                    .lines()
                    .filter(|line| line.starts_with("frame:"))
                    .count(),
                4
            );
            if format != ImageOutput::Gif {
                let mut names = std::fs::read_dir(&output)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                names.sort();
                assert_eq!(
                    names,
                    (1..=4)
                        .map(|index| format!("{index:08}.{label}"))
                        .collect::<Vec<_>>()
                );
            }
            eprintln!(
                "{name}: one actual labeled graph invocation,4frames80x48, metadata side effect exactly4frames, valid selected family"
            );
        }
    }
    let interlaced = root.join("interlaced.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(input.as_os_str().to_owned());
    a.extend(args(&[
        "-vf",
        "setfield=tff,setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-threads",
        "2",
        "-chroma_sample_location",
        "left",
    ]));
    a.push(interlaced.as_os_str().to_owned());
    ffmpeg(a).await;
    let marker = "jesses_single_image_filter_qtgmc";
    let output = root.join("qtgmc-png");
    let mut selected = request(
        &interlaced,
        &output,
        ImageOutput::PngSequence,
        format!("scale=80:48,fps=2,metadata=mode=add:key={marker}:value=once"),
    );
    if let ImageRequest::Export {
        processing: Some(settings),
        ..
    } = &mut selected
    {
        settings.temporal = Some(TemporalSettings {
            qtgmc: Some(QtgmcSettings {
                mode: DeinterlaceMode::Bob,
                field_order: FieldOrder::TopFirst,
                preset: QtgmcPreset::Fast,
            }),
            ..Default::default()
        });
    }
    let (_owner, cancel) = watch::channel(false);
    let review = manager
        .prepare_command_edit(
            CommandEditRequest::Image {
                request: Box::new(selected),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    assert_eq!(executions(marker), 0);
    let result = manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id,
                arguments: review.arguments,
            },
            cancel,
        )
        .await
        .unwrap();
    let CommandEditResult::Image { result } = result else {
        panic!("image")
    };
    assert_eq!(
        (result.width, result.height, result.frame_count),
        (80, 48, 4)
    );
    assert_eq!(executions(marker), 1);
    eprintln!("Held QTGMC Bob plus raw geometry/fps: one custom-filter invocation,4frames80x48");
    for edited in [false, true] {
        let marker = format!("jesses_single_image_filter_qtgmc_cpu_{edited}");
        let output = root.join(format!("qtgmc-cpu-{edited}.mkv"));
        let settings = EncodeSettings {
            encoder: VideoEncoder::X264,
            preset: 0,
            workers: 1,
            source_filters: true,
            av1an_filters: vec![format!(
                "scale=80:48,fps=2,metadata=mode=add:key={marker}:value=once"
            )],
            temporal: Some(TemporalSettings {
                qtgmc: Some(QtgmcSettings {
                    mode: DeinterlaceMode::Bob,
                    field_order: FieldOrder::TopFirst,
                    preset: QtgmcPreset::Fast,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let request = EncodeRequest {
            source: RemuxRequest {
                input_path: interlaced.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings,
        };
        let (_owner, cancel) = watch::channel(false);
        let review = manager
            .prepare_command_edit(
                CommandEditRequest::Encode {
                    request: Box::new(request),
                },
                cancel.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            executions(&marker),
            0,
            "ordinary CPU QTGMC user filtering follows source review boundary"
        );
        assert!(review.arguments.contains("vspipe") && review.arguments.contains(&marker));
        let result = manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments: if edited {
                        review.arguments.replace("scale=80:48", "scale=64:48")
                    } else {
                        review.arguments
                    },
                },
                cancel,
            )
            .await
            .unwrap();
        let CommandEditResult::Encode { job } = result else {
            panic!("encode")
        };
        let done = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let snapshot = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|entry| entry.id == job.id)
                    .unwrap();
                if snapshot.state.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
        assert_eq!(executions(&marker), 1);
        eprintln!(
            "Ordinary CPU held QTGMC edited{edited}: filtering after review,one execution,source wrapper precedence retained"
        );
    }
    let marker = "jesses_single_image_filter_qtgmc_recovery";
    let output = root.join("qtgmc-recovery.mkv");
    let settings = EncodeSettings {
        encoder: VideoEncoder::X264,
        preset: 0,
        workers: 1,
        source_filters: true,
        av1an_filters: vec![format!(
            "scale=80:48,fps=2,metadata=mode=add:key={marker}:value=once"
        )],
        temporal: Some(TemporalSettings {
            qtgmc: Some(QtgmcSettings {
                mode: DeinterlaceMode::Bob,
                field_order: FieldOrder::TopFirst,
                preset: QtgmcPreset::Fast,
            }),
            ..Default::default()
        }),
        // Only thorough runs keep a resume workspace.
        encoding_preferences: Some(media_core::EncodingPreferences {
            keyframe_interval_seconds: 10,
            mp4_faststart: true,
            full_output_verification: true,
        }),
        ..Default::default()
    };
    let selected = EncodeRequest {
        source: RemuxRequest {
            input_path: interlaced.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0],
        },
        settings,
    };
    let (_owner, cancel) = watch::channel(false);
    let review = manager
        .prepare_command_edit(
            CommandEditRequest::Encode {
                request: Box::new(selected),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    let job_id = review.job.as_ref().unwrap().id.clone();
    assert_eq!(executions(marker), 0);
    std::fs::write(
        std::env::var_os("JESSES_TEST_FAIL_MUX")
            .expect("Use the owned one-shot mux-fault control file"),
        &job_id,
    )
    .unwrap();
    manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id,
                arguments: review.arguments,
            },
            cancel,
        )
        .await
        .unwrap();
    let stopped = finish(&manager, &job_id).await;
    assert_eq!(stopped.state, JobState::Failed, "{stopped:#?}");
    assert_eq!(executions(marker), 1);
    let workspace = PathBuf::from(&stopped.standalone_recovery.as_ref().unwrap().workspace);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["source_qtgmc"]["state"], "rendered");
    assert_eq!(
        manifest["source_qtgmc"]["artifact"]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert!(
        manifest["source_qtgmc"]["decoded_identity"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(
        manifest["source_qtgmc"]["decoded_identity"]
            .as_str()
            .unwrap()
            .ends_with(";frames:4")
    );
    let cache = PathBuf::from(
        manifest["source_qtgmc"]["artifact"]["path"]
            .as_str()
            .unwrap(),
    );
    assert!(
        !cache.exists(),
        "the temporary rendered cache is not adopted as durable recovery input"
    );
    assert_eq!(manifest["source_filter_contract"]["frames"], 4);
    native_future(|| manager.resume_job(job_id.clone()))
        .await
        .unwrap();
    let done = finish(&manager, &job_id).await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    assert_eq!(
        executions(marker),
        1,
        "the verified encoded checkpoint must skip raw QTGMC filtering on resume"
    );
    assert!(!workspace.exists());
    eprintln!(
        "Source QTGMC Saved recovery: real rendered byte+decoded receipts,4-frame contract; deleted temporary cache not reused; sealed encoded checkpoint resumes with zero raw-filter reruns"
    );
    let marker = "jesses_single_image_filter_cancel";
    let output = root.join("canceled-png");
    let selected = request(
        &input,
        &output,
        ImageOutput::PngSequence,
        format!("realtime=speed=0.01,metadata=mode=add:key={marker}:value=once"),
    );
    let (_owner, cancel) = watch::channel(false);
    let review = manager
        .prepare_command_edit(
            CommandEditRequest::Image {
                request: Box::new(selected),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    let id = review.id.clone();
    let active = manager.clone();
    let confirm = tokio::spawn(async move {
        active
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments: review.arguments,
                },
                cancel,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while executions(marker) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    manager.set_command_edit_paused(&id, true).await.unwrap();
    manager.cancel_command_edit(&id).await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), confirm)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "JOB_CANCELED");
    assert!(!output.exists());
    assert!(!std::fs::read_dir(&root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("review-spool")
    }));
    eprintln!("Active source-filter image Stop while paused: child joined, no output/spool");
    let marker = "jesses_single_image_filter_edited";
    let output = root.join("edited-png");
    let request = request(
        &input,
        &output,
        ImageOutput::PngSequence,
        format!("scale=80:48,metadata=mode=add:key={marker}:value=once"),
    );
    let (_owner, cancel) = watch::channel(false);
    let review = manager
        .prepare_command_edit(
            CommandEditRequest::Image {
                request: Box::new(request),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    assert_eq!(executions(marker), 0);
    let result = manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id,
                arguments: review.arguments.replace("scale=80:48", "scale=64:48"),
            },
            cancel,
        )
        .await
        .unwrap();
    let CommandEditResult::Image { result } = result else {
        panic!("image")
    };
    assert_eq!(
        (result.width, result.height, result.frame_count),
        (64, 48, 8)
    );
    assert_eq!(executions(marker), 1);
    assert_eq!(std::fs::read(&input).unwrap(), original);
    manager.shutdown().await;
    eprintln!("Retained image source-filter proof {}", root.display());
}
