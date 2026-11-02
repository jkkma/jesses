//! Real held image programs, packet-preserving extraction and owned cleanup.
use media_core::*;
use media_runtime::{
    jobs::JobManager,
    supervisor::{CommandSpec, run_capture},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "jesses-image-review-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
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
async fn tool(v: Vec<OsString>) -> Vec<u8> {
    let ffmpeg = std::env::var_os("JESSES_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    let (_owner, cancel) = watch::channel(false);
    let r = run_capture(
        &CommandSpec {
            executable: ffmpeg.into(),
            args: v,
            cwd: None,
        },
        cancel,
        32 * 1024 * 1024,
        Duration::from_secs(90),
    )
    .await
    .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    r.stdout
}
async fn source(root: &Path, long: bool) -> PathBuf {
    let input = root.join("picture ' [review].mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        if long {
            "testsrc2=size=2x2:rate=1000"
        } else {
            "testsrc2=size=128x96:rate=4"
        },
        "-frames:v",
        if long { "15000" } else { "8" },
        "-c:v",
        "ffv1",
        "-threads",
        "2",
    ]);
    a.push(input.as_os_str().to_owned());
    tool(a).await;
    input
}
fn request(input: &Path, output: &Path, format: ImageOutput) -> ImageRequest {
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
async fn prepare(manager: &JobManager, request: ImageRequest) -> CommandEditReview {
    let (_owner, cancel) = watch::channel(false);
    manager
        .prepare_command_edit(
            CommandEditRequest::Image {
                request: Box::new(request),
            },
            cancel,
        )
        .await
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "set VK_DRIVER_FILES to an absent file; requires qualified FFmpeg/FFprobe"]
async fn held_image_spline_graph_is_editable_without_a_vulkan_device() {
    let driver = std::env::var_os("VK_DRIVER_FILES").expect("Use an absent Vulkan ICD file");
    assert!(!Path::new(&driver).exists());
    let fixture = Fixture::new();
    let input = fixture.0.join("HDR-source.mkv");
    let mut command = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=128x96:r=4:d=1,format=yuv420p10le,setparams=range=tv:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc",
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
    ]);
    command.push(input.as_os_str().to_owned());
    tool(command).await;
    let original = std::fs::read(&input).unwrap();
    let ffmpeg = std::env::var_os("JESSES_FFMPEG").unwrap();
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\"'\"'"));
    for (index, format, edited) in [
        (0, ImageOutput::PngSequence, false),
        (1, ImageOutput::PngSequence, true),
        (2, ImageOutput::JpegSequence, true),
        (3, ImageOutput::Gif, true),
    ] {
        let output = fixture.0.join(if format == ImageOutput::Gif {
            format!("out-{index}.gif")
        } else {
            format!("out-{index}")
        });
        let mut request = request(&input, &output, format);
        if let ImageRequest::Export { processing, .. } = &mut request {
            *processing=Some(serde_json::from_value(serde_json::json!({"toneMap":{"sourcePeakNits":1000,"backend":"auto","peakMode":"measured","algorithm":"spline"}})).unwrap());
        }
        let manager = JobManager::new(fixture.0.join(format!("logs-{index}")));
        let review = prepare(&manager, request).await;
        assert!(
            review.arguments.contains("tonemapping=spline") && review.arguments.contains("vulkan")
        );
        assert!(!output.exists());
        let extension = match format {
            ImageOutput::Gif => "gif",
            ImageOutput::JpegSequence => "jpg",
            _ => "png",
        };
        let private_spool = spool(&fixture.0, extension);
        let arguments = if edited {
            let codec = match format {
                ImageOutput::Gif => "gif",
                ImageOutput::JpegSequence => "mjpeg",
                _ => "png",
            };
            let mux = if format == ImageOutput::Gif {
                "gif"
            } else {
                "image2pipe"
            };
            format!(
                "{} -v error -nostdin -i {} -map 0:v:0 -an -sn -dn -c:v {} -threads 1 -f {} -y {}",
                quote(&ffmpeg.to_string_lossy()),
                quote(&input.to_string_lossy()),
                codec,
                mux,
                quote(&private_spool.to_string_lossy())
            )
        } else {
            review.arguments
        };
        let (_owner, cancel) = watch::channel(false);
        let result = manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments,
                },
                cancel,
            )
            .await;
        if edited {
            assert!(
                matches!(result, Ok(CommandEditResult::Image { .. })),
                "{result:?}"
            );
            assert!(output.exists());
        } else {
            assert_eq!(result.unwrap_err().code, "TONE_MAP_GPU_UNAVAILABLE");
            assert!(!output.exists());
        }
        manager.shutdown().await;
        assert!(!private_spool.exists());
    }
    assert_eq!(std::fs::read(&input).unwrap(), original);
}
async fn confirm(
    manager: &JobManager,
    review: &CommandEditReview,
    arguments: String,
) -> ImageResult {
    let (_owner, cancel) = watch::channel(false);
    match manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id.clone(),
                arguments,
            },
            cancel,
        )
        .await
        .unwrap()
    {
        CommandEditResult::Image { result } => result,
        _ => panic!("Wrong review result"),
    }
}
fn clean(root: &Path) {
    assert!(
        !std::fs::read_dir(root).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jesses-")),
        "Owned image preparation leaked"
    );
}
fn spool(root: &Path, extension: &str) -> PathBuf {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains("review-spool")
                && path.extension().unwrap() == extension
        })
        .unwrap()
}
fn quote(value: &Path) -> String {
    if cfg!(windows) {
        format!("\"{}\"", value.display())
    } else {
        format!("'{}'", value.to_string_lossy().replace('\'', "'\"'\"'"))
    }
}
fn witness(program: String, from: &Path, to: &Path) -> String {
    format!(
        "{program} &&\n{} {} {}",
        if cfg!(windows) { "copy /b" } else { "cp --" },
        quote(from),
        quote(to)
    )
}
fn encoded_frames(bytes: &[u8], png: bool) -> Vec<&[u8]> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let end = if png {
            assert_eq!(&bytes[start..start + 8], b"\x89PNG\r\n\x1a\n");
            let mut at = start + 8;
            loop {
                let n = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
                let end = at + 12 + n;
                if &bytes[at + 4..at + 8] == b"IEND" {
                    break end;
                }
                at = end;
            }
        } else {
            assert_eq!(&bytes[start..start + 2], &[0xff, 0xd8]);
            start
                + 2
                + bytes[start + 2..]
                    .windows(2)
                    .position(|v| v == [0xff, 0xd9])
                    .unwrap()
                + 2
        };
        result.push(&bytes[start..end]);
        start = end;
    }
    result
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe image codecs"]
async fn held_png_jpeg_programs_accept_changed_trim_geometry_and_preserve_encoded_packet_bytes() {
    let root = Fixture::new();
    let input = source(&root.0, false).await;
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(root.0.join("logs"));
    for (format, extension) in [
        (ImageOutput::PngSequence, "png"),
        (ImageOutput::JpegSequence, "jpg"),
    ] {
        let output = root.0.join(extension);
        let review = prepare(&manager, request(&input, &output, format)).await;
        assert!(!output.exists());
        assert!(review.arguments.contains("framemd5"));
        assert!(review.arguments.contains("image2pipe"));
        let spool = spool(&root.0, extension);
        assert_eq!(std::fs::metadata(&spool).unwrap().len(), 0);
        let mut program = review.arguments.replace(
            "setpts=PTS-STARTPTS",
            "setpts=PTS-STARTPTS,trim=start_frame=1:end_frame=3,setpts=PTS-STARTPTS,scale=64:48",
        );
        if format == ImageOutput::PngSequence {
            program = program.replace("'-pix_fmt' 'rgb24'", "'-pix_fmt' 'gray'");
        }
        let saved = root.0.join(format!("witness.{extension}"));
        let result = confirm(&manager, &review, witness(program, &spool, &saved)).await;
        assert_eq!(
            (result.frame_count, result.width, result.height),
            (2, 64, 48)
        );
        let bytes = std::fs::read(saved).unwrap();
        let encoded = encoded_frames(&bytes, extension == "png");
        assert_eq!(encoded.len(), 2);
        for (index, packet) in encoded.iter().enumerate() {
            let actual =
                std::fs::read(output.join(format!("{:08}.{extension}", index + 1))).unwrap();
            assert_eq!(
                Sha256::digest(packet),
                Sha256::digest(&actual),
                "Encoded bytes changed during extraction"
            );
        }
        assert!(!spool.exists());
        clean(&root.0);
    }
    assert_eq!(std::fs::read(input).unwrap(), original);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe GIF/PNG codecs"]
async fn edited_gif_palette_and_unchanged_image_review_retain_source_behaviour() {
    let root = Fixture::new();
    let input = source(&root.0, false).await;
    let manager = JobManager::new(root.0.join("logs"));
    let output = root.0.join("edited.gif");
    let review = prepare(&manager, request(&input, &output, ImageOutput::Gif)).await;
    assert_eq!(std::fs::metadata(spool(&root.0, "gif")).unwrap().len(), 0);
    assert!(review.arguments.contains("palettegen=max_colors=128"));
    let result = confirm(
        &manager,
        &review,
        review.arguments.replace("max_colors=128", "max_colors=8"),
    )
    .await;
    assert_eq!(result.frame_count, 8);
    let mut a = args(&["-v", "error", "-i"]);
    a.push(output.as_os_str().to_owned());
    a.extend(args(&["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]));
    let values = tool(a).await;
    assert!(
        values
            .as_chunks::<3>()
            .0
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            <= 8
    );
    for (format, name) in [
        (ImageOutput::PngSequence, "unchanged-png"),
        (ImageOutput::JpegSequence, "unchanged-jpeg"),
        (ImageOutput::Gif, "unchanged.gif"),
    ] {
        let output = root.0.join(name);
        let review = prepare(&manager, request(&input, &output, format)).await;
        let result = confirm(&manager, &review, review.arguments.clone()).await;
        assert_eq!(
            (result.frame_count, result.width, result.height),
            (8, 128, 96)
        );
    }
    clean(&root.0);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe image codecs"]
async fn closing_clearing_and_shutdown_release_held_image_assets_without_conversion() {
    let root = Fixture::new();
    let input = source(&root.0, false).await;
    for action in ["close", "clear", "shutdown"] {
        let manager = JobManager::new(root.0.join(format!("logs-{action}")));
        let output = root.0.join(action);
        let review = prepare(&manager, request(&input, &output, ImageOutput::PngSequence)).await;
        assert_eq!(std::fs::metadata(spool(&root.0, "png")).unwrap().len(), 0);
        match action {
            "close" => manager.cancel_command_edit(&review.id).await.unwrap(),
            "clear" => {
                let (_owner, cancel) = watch::channel(false);
                assert_eq!(
                    manager
                        .confirm_command_edit(
                            CommandEditConfirmation {
                                id: review.id,
                                arguments: " \n ".into()
                            },
                            cancel
                        )
                        .await
                        .unwrap_err()
                        .code,
                    "COMMAND_EDIT_CANCELED"
                );
            }
            _ => manager.shutdown().await,
        }
        assert!(!output.exists());
        clean(&root.0);
        manager.shutdown().await;
    }
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe PNG encoder"]
async fn canceling_active_spooling_and_packet_extraction_never_publishes() {
    let root = Fixture::new();
    let input = source(&root.0, true).await;
    let original = std::fs::read(&input).unwrap();
    for phase in ["spool", "extract"] {
        let manager = JobManager::new(root.0.join(format!("logs-{phase}")));
        let output = root.0.join(phase);
        let review = prepare(&manager, request(&input, &output, ImageOutput::PngSequence)).await;
        let spool = spool(&root.0, "png");
        let mut lines: Vec<String> = review.arguments.split(" &&\n").map(str::to_owned).collect();
        if phase == "spool" {
            let last = lines.last_mut().unwrap();
            *last = last.replacen("'-i'", "'-re' '-i'", 1);
        } else {
            let last = lines.last_mut().unwrap();
            *last = last.replace("'-compression_level' '3'", "'-compression_level' '4'");
        }
        let program = lines.join(" &&\n");
        let (owner, cancel) = watch::channel(false);
        let worker_manager = manager.clone();
        let task = tokio::spawn(async move {
            worker_manager
                .confirm_command_edit(
                    CommandEditConfirmation {
                        id: review.id,
                        arguments: program,
                    },
                    cancel,
                )
                .await
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let active = if phase == "spool" {
                std::fs::metadata(&spool).is_ok_and(|m| m.len() > 1024)
            } else {
                std::fs::read_dir(&root.0)
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .any(|path| {
                        path.is_dir()
                            && path
                                .file_name()
                                .unwrap()
                                .to_string_lossy()
                                .starts_with(".jesses-images")
                            && std::fs::metadata(path.join("00000001.png"))
                                .is_ok_and(|m| m.len() > 0)
                            && std::fs::metadata(path.join("00015000.png"))
                                .is_ok_and(|m| m.len() == 0)
                    })
            };
            if active {
                break;
            }
            assert!(
                !task.is_finished(),
                "Task completed before {phase} cancellation"
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "Did not observe active {phase}"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        owner.send_replace(true);
        assert!(task.await.unwrap().is_err());
        assert!(!output.exists());
        clean(&root.0);
        manager.shutdown().await;
    }
    assert_eq!(std::fs::read(input).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires real QTGMC/R79 provider, FFmpeg/FFprobe libass/PNG"]
async fn reviewed_qtgmc_keeps_caption_and_script_assets_alive_until_confirmed_program_finishes() {
    let root = Fixture::new();
    let input = root.0.join("interlaced.mkv");
    let subtitle = root.0.join("caption.srt");
    std::fs::write(&subtitle, "1\n00:00:00,000 --> 00:00:01,000\nCAPTION\n\n").unwrap();
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=black:size=320x180:rate=24:duration=0.5",
        "-vf",
        "setfield=tff,setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-threads",
        "2",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(input.as_os_str().to_owned());
    tool(a).await;
    let original = std::fs::read(&input).unwrap();
    let original_subtitle = std::fs::read(&subtitle).unwrap();
    let output = root.0.join("qtgmc-reviewed");
    let mut r = request(&input, &output, ImageOutput::PngSequence);
    if let ImageRequest::Export { processing, .. } = &mut r {
        *processing = Some(ImageProcessingSettings {
            subtitle_burn_in: Some(ImageSubtitleSource::External {
                input_path: subtitle.to_string_lossy().into_owned(),
                stream_index: 0,
                offset_milliseconds: 0,
            }),
            temporal: Some(TemporalSettings {
                qtgmc: Some(QtgmcSettings {
                    mode: DeinterlaceMode::Bob,
                    field_order: FieldOrder::TopFirst,
                    preset: QtgmcPreset::Fast,
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
    }
    let manager = JobManager::new(root.0.join("logs"));
    let review = prepare(&manager, r).await;
    assert!(review.arguments.contains("vspipe"));
    assert!(review.arguments.contains(" | "));
    assert!(review.arguments.contains("subtitles="));
    assert_eq!(std::fs::metadata(spool(&root.0, "png")).unwrap().len(), 0);
    assert!(
        std::fs::read_dir(&root.0)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().ends_with("-subtitles"))
    );
    let mut lines: Vec<String> = review.arguments.split(" &&\n").map(str::to_owned).collect();
    let last = lines.last_mut().unwrap();
    *last = last.replace(
        "setpts=PTS-STARTPTS",
        "setpts=PTS-STARTPTS,trim=start_frame=0:end_frame=4,scale=160:90",
    );
    let result = confirm(&manager, &review, lines.join(" &&\n")).await;
    assert_eq!(
        (result.frame_count, result.width, result.height),
        (4, 160, 90)
    );
    let mut a = args(&["-v", "error", "-i"]);
    a.push(output.join("00000001.png").into_os_string());
    a.extend(args(&["-f", "rawvideo", "-pix_fmt", "gray", "-"]));
    assert!(tool(a).await.iter().any(|pixel| *pixel > 100));
    assert_eq!(std::fs::read(input).unwrap(), original);
    assert_eq!(std::fs::read(subtitle).unwrap(), original_subtitle);
    clean(&root.0);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe PNG encoder"]
async fn reviewed_image_source_mutation_destination_collision_and_active_shutdown_are_guarded() {
    let root = Fixture::new();
    let input = source(&root.0, true).await;
    let original = std::fs::read(&input).unwrap();
    for action in ["source", "collision", "shutdown"] {
        let manager = JobManager::new(root.0.join(format!("logs-{action}")));
        let output = root.0.join(action);
        let review = prepare(&manager, request(&input, &output, ImageOutput::PngSequence)).await;
        let mut lines: Vec<String> = review.arguments.split(" &&\n").map(str::to_owned).collect();
        let last = lines.last_mut().unwrap();
        *last = last.replace("'-compression_level' '3'", "'-compression_level' '4'");
        if action == "source" {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(&input)
                .unwrap()
                .write_all(b"changed")
                .unwrap();
        }
        if action == "collision" {
            std::fs::create_dir(&output).unwrap();
            std::fs::write(output.join("personal.txt"), b"later owner").unwrap();
        }
        if action == "shutdown" {
            *lines.last_mut().unwrap() = lines.last().unwrap().replacen("'-i'", "'-re' '-i'", 1);
        }
        let (_owner, cancel) = watch::channel(false);
        let copied = manager.clone();
        let task = tokio::spawn(async move {
            copied
                .confirm_command_edit(
                    CommandEditConfirmation {
                        id: review.id,
                        arguments: lines.join(" &&\n"),
                    },
                    cancel,
                )
                .await
        });
        if action == "shutdown" {
            let spool = spool(&root.0, "png");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            while !std::fs::metadata(&spool).is_ok_and(|m| m.len() > 1024) {
                assert!(!task.is_finished());
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            manager.shutdown().await;
            clean(&root.0);
        }
        assert!(task.await.unwrap().is_err());
        clean(&root.0);
        if action == "collision" {
            assert_eq!(
                std::fs::read(output.join("personal.txt")).unwrap(),
                b"later owner"
            );
        } else {
            assert!(!output.exists());
        }
        manager.shutdown().await;
        std::fs::write(&input, &original).unwrap();
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and Unix shell"]
#[cfg(unix)]
async fn renderer_navigation_cancels_held_image_review_but_confirmed_native_job_continues() {
    let root = Fixture::new();
    let input = source(&root.0, false).await;
    let manager = JobManager::open(root.0.join("logs"), root.0.join("history")).await;
    let (_owner, cancel) = watch::channel(false);
    let ordinary = manager
        .prepare_command_edit(
            CommandEditRequest::Encode {
                request: Box::new(EncodeRequest {
                    source: RemuxRequest {
                        input_path: input.to_string_lossy().into_owned(),
                        output_path: root.0.join("ordinary.mkv").to_string_lossy().into_owned(),
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
                id: ordinary.id,
                arguments: format!("sleep 0.5 && {}", ordinary.arguments),
            },
            cancel.clone(),
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(!job.state.is_terminal());
    let output = root.0.join("held-images");
    let image = prepare(&manager, request(&input, &output, ImageOutput::PngSequence)).await;
    assert_eq!(std::fs::metadata(spool(&root.0, "png")).unwrap().len(), 0);
    manager.cancel_command_reviews().await.unwrap();
    assert!(!output.exists());
    assert_eq!(
        manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: image.id,
                    arguments: image.arguments
                },
                cancel
            )
            .await
            .unwrap_err()
            .code,
        "COMMAND_EDIT_EXPIRED"
    );
    let finished = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let current = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|current| current.id == job.id)
                .unwrap();
            if current.state.is_terminal() {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    assert!(root.0.join("ordinary.mkv").is_file());
    clean(&root.0);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn explicit_fixed_images_apply_guarded_source_metadata_and_source_sequence_names() {
    let root = Fixture::new();
    let input = source(&root.0, false).await;
    let donor = root.0.join("metadata donor.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(input.as_os_str().to_owned());
    a.extend(args(&[
        "-map",
        "0",
        "-c",
        "copy",
        "-metadata",
        "title=Inherited donor",
        "-metadata",
        "language=eng",
        "-metadata:s:0",
        "title=Inherited track",
    ]));
    a.push(donor.as_os_str().to_owned());
    tool(a).await;
    let original = std::fs::read(&donor).unwrap();
    let manager = JobManager::open(root.0.join("logs"), root.0.join("history")).await;
    for format in [
        ImageOutput::PngSequence,
        ImageOutput::JpegSequence,
        ImageOutput::Gif,
    ] {
        for mode in 0..3 {
            let extension = if format == ImageOutput::PngSequence {
                "png"
            } else if format == ImageOutput::JpegSequence {
                "jpg"
            } else {
                "gif"
            };
            let output = root
                .0
                .join(format!("metadata-{format:?}-{mode}.{extension}"));
            let mut value = request(&input, &output, format);
            if let ImageRequest::Export { processing, .. } = &mut value {
                *processing = Some(ImageProcessingSettings {
                    metadata: Some(Box::new(ImageExportMetadata {
                        inputs: vec![
                            input.to_string_lossy().into_owned(),
                            donor.to_string_lossy().into_owned(),
                        ],
                        options: ImageImplicitMetadata {
                            source_input: (mode == 1).then_some(1),
                            chapters_input: (mode == 1).then_some(1),
                            title: (mode != 2).then_some(
                                if mode == 0 { "" } else { "Explicit file title" }.into(),
                            ),
                            language: (mode != 2)
                                .then_some(if mode == 0 { "" } else { "jpn" }.into()),
                            tracks: if mode == 2 {
                                vec![]
                            } else {
                                vec![ImageImplicitTrackMetadata {
                                    title: Some("Explicit track title".into()),
                                    language: Some("eng".into()),
                                }]
                            },
                            ..Default::default()
                        },
                    })),
                    ..Default::default()
                });
            }
            let review = prepare(&manager, value.clone()).await;
            let base = if format == ImageOutput::Gif { 2 } else { 1 };
            let donor_index = if mode == 1 {
                (base + 1).to_string()
            } else {
                "-1".into()
            };
            assert!(
                review
                    .arguments
                    .contains(&format!("'-map_metadata' '{donor_index}'"))
            );
            assert!(
                review
                    .arguments
                    .contains(&format!("'-map_chapters' '{donor_index}'"))
            );
            if mode == 0 {
                assert!(review.arguments.contains("'-metadata' 'title='"));
                assert!(review.arguments.contains("'-metadata' 'language='"));
            } else if mode == 1 {
                assert!(
                    review
                        .arguments
                        .contains("'-metadata' 'title=Explicit file title'")
                );
            } else {
                assert!(!review.arguments.contains("'-metadata'"));
            }
            assert!(review.arguments.contains(&quote(&donor)));
            eprintln!(
                "{format:?} metadata mode{mode}: remapped donor {donor_index}, explicit grid={}",
                mode != 2
            );
            let result = confirm(&manager, &review, review.arguments.clone()).await;
            assert_eq!(result.frame_count, 8);
            let files = if format == ImageOutput::Gif {
                vec![output.clone()]
            } else {
                let mut paths = std::fs::read_dir(&output)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect::<Vec<_>>();
                paths.sort();
                assert_eq!(paths.len(), 8);
                for (index, path) in paths.iter().enumerate() {
                    assert_eq!(
                        path.file_name().unwrap().to_string_lossy(),
                        format!("{:08}.{extension}", index + 1)
                    );
                }
                paths
            };
            let actual = media_runtime::probe_media(files[0].to_string_lossy().into_owned())
                .await
                .unwrap();
            assert_eq!(actual.title, None);
            assert_eq!(actual.language, None);
            assert_eq!(actual.streams[0].title, None);
            assert_eq!(actual.streams[0].language, None);
            // The selected fixed image formats do not persist these title/language
            // fields in the source FFmpeg command either. Assert the real outcome.
            let reference = root
                .0
                .join(format!("source-metadata-{format:?}-{mode}.{extension}"));
            let mut a = args(&["-v", "error", "-i"]);
            a.push(input.as_os_str().to_owned());
            a.push("-i".into());
            a.push(donor.as_os_str().to_owned());
            a.extend(args(&[
                "-map",
                "0:v:0",
                "-an",
                "-sn",
                "-frames:v",
                "1",
                "-map_metadata",
                if mode == 1 { "1" } else { "-1" },
                "-map_chapters",
                if mode == 1 { "1" } else { "-1" },
            ]));
            if mode != 2 {
                a.extend(args(&[
                    "-metadata",
                    if mode == 0 {
                        "title="
                    } else {
                        "title=Explicit file title"
                    },
                    "-metadata",
                    if mode == 0 {
                        "language="
                    } else {
                        "language=jpn"
                    },
                    "-metadata:s:0",
                    "title=Explicit track title",
                    "-metadata:s:0",
                    "language=eng",
                ]));
            }
            a.push(reference.as_os_str().to_owned());
            tool(a).await;
            let source = media_runtime::probe_media(reference.to_string_lossy().into_owned())
                .await
                .unwrap();
            assert_eq!(
                (
                    source.title,
                    source.language,
                    source.streams[0].title.clone(),
                    source.streams[0].language.clone()
                ),
                (None, None, None, None)
            );
            if mode == 2 {
                let normal = root
                    .0
                    .join(format!("normal-metadata-{format:?}.{extension}"));
                if let ImageRequest::Export { output_path, .. } = &mut value {
                    *output_path = normal.to_string_lossy().into_owned();
                }
                let (_owner, cancel) = watch::channel(false);
                let normal = media_runtime::jobs::run_image_job(value, cancel)
                    .await
                    .unwrap();
                assert_eq!(normal.frame_count, 8);
            }
            clean(&root.0);
        }
    }
    let output = root.0.join("metadata-source-change");
    let mut value = request(&input, &output, ImageOutput::PngSequence);
    if let ImageRequest::Export { processing, .. } = &mut value {
        *processing = Some(ImageProcessingSettings {
            metadata: Some(Box::new(ImageExportMetadata {
                inputs: vec![donor.to_string_lossy().into_owned()],
                options: Default::default(),
            })),
            ..Default::default()
        });
    }
    let review = prepare(&manager, value).await;
    std::fs::write(&donor, b"changed donor").unwrap();
    let (_owner, cancel) = watch::channel(false);
    assert_eq!(
        manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments: review.arguments
                },
                cancel
            )
            .await
            .unwrap_err()
            .code,
        "SOURCE_CHANGED"
    );
    assert!(!output.exists());
    std::fs::write(&donor, original).unwrap();
    clean(&root.0);
    manager.shutdown().await;
}
