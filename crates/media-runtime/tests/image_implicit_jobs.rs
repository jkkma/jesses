//! Direct source-command parity for fixed outputs with no checked video.
use media_core::*;
use media_runtime::{
    jobs::{JobManager, run_image_job},
    supervisor::{CapturedOutput, CommandSpec, run_capture},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::sync::watch;
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/image-implicit");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(std::fs::canonicalize(root).unwrap())
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
async fn command(a: Vec<OsString>) -> CapturedOutput {
    let (_owner, cancel) = watch::channel(false);
    run_capture(
        &CommandSpec {
            executable: std::env::var_os("JESSES_FFMPEG")
                .unwrap_or_else(|| "ffmpeg".into())
                .into(),
            args: a,
            cwd: None,
        },
        cancel,
        32 * 1024 * 1024,
        Duration::from_secs(90),
    )
    .await
    .unwrap()
}
async fn run(a: Vec<OsString>) -> Vec<u8> {
    let r = command(a).await;
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    r.stdout
}
async fn source(root: &Path, name: &str, large: bool) -> PathBuf {
    let sub = root.join(format!("{name}.srt"));
    std::fs::write(&sub, "1\n00:00:00,000 --> 00:00:01,000\nCaption\n").unwrap();
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        if large {
            "color=green:size=80x60:rate=8:duration=1"
        } else {
            "color=red:size=64x48:rate=4:duration=1"
        },
    ]);
    if large {
        a.extend(args(&[
            "-f",
            "lavfi",
            "-i",
            "color=blue:size=128x96:rate=8:duration=1",
        ]));
    }
    a.extend(args(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=700:duration=1",
        "-i",
    ]));
    a.push(sub.as_os_str().to_owned());
    a.extend(args(&["-map", "0:v:0"]));
    if large {
        a.extend(args(&["-map", "1:v:0"]));
    }
    a.extend(args(&[
        "-map",
        if large { "2:a:0" } else { "1:a:0" },
        "-map",
        if large { "3:s:0" } else { "2:s:0" },
        "-c:v",
        "ffv1",
        "-c:a",
        "pcm_s16le",
        "-c:s",
        "srt",
        "-disposition:v",
        "0",
        "-threads",
        "2",
    ]));
    let path = root.join(format!("{name} ' [source].mkv"));
    a.push(path.as_os_str().to_owned());
    run(a).await;
    path
}
fn req(
    inputs: &[PathBuf],
    checked: Vec<ImageInputTrack>,
    output: &Path,
    format: ImageOutput,
) -> ImageRequest {
    ImageRequest::ImplicitExport {
        inputs: inputs
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        checked,
        current_input: 0,
        format,
        output_path: output.to_string_lossy().into_owned(),
        audio: None,
        subtitle_codec: ImageImplicitSubtitle::Copy,
        metadata: Default::default(),
        trim: None,
        keep_frame_count: true,
    }
}
fn t(input_index: u32, stream_index: u32) -> ImageInputTrack {
    ImageInputTrack {
        input_index,
        stream_index,
    }
}
async fn native(request: ImageRequest) -> Result<ImageResult, AppError> {
    let (_owner, cancel) = watch::channel(false);
    run_image_job(request, cancel).await
}
fn files(root: &Path) -> Vec<PathBuf> {
    let mut files = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    files.sort();
    files
}
fn clean(root: &Path) {
    assert!(
        !files(root).iter().any(|path| path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".jesses-")),
        "Owned stages leaked"
    );
}
async fn direct(
    inputs: &[PathBuf],
    out: &Path,
    format: ImageOutput,
    options: &[&str],
) -> CapturedOutput {
    let mut a = args(&["-v", "error", "-nostdin", "-y"]);
    for input in inputs {
        a.push("-i".into());
        a.push(input.as_os_str().to_owned());
    }
    a.extend(args(options));
    a.extend(args(&["-map_metadata", "-1", "-map_chapters", "-1"]));
    if format != ImageOutput::Gif {
        std::fs::create_dir(out).unwrap();
        a.push(
            out.join(if format == ImageOutput::PngSequence {
                "%8d.png"
            } else {
                "%8d.jpg"
            })
            .into_os_string(),
        );
    } else {
        a.push(out.as_os_str().to_owned());
    }
    command(a).await
}
async fn pixels(path: &Path) -> Vec<u8> {
    let mut a = args(&["-v", "error", "-i"]);
    a.push(path.as_os_str().to_owned());
    a.extend(args(&[
        "-map", "0:v:0", "-pix_fmt", "rgb24", "-f", "rawvideo", "-",
    ]));
    run(a).await
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn nonvideo_modes_correct_source_corruption_and_gif_errors_with_valid_image_packets() {
    let f = Fixture::new();
    let input = source(&f.0, "modes", false).await;
    // The source always emits subtitle codec arguments, even for audio-only checks.
    let modes = [
        (
            "subcopy",
            None,
            ImageImplicitSubtitle::Copy,
            vec![t(0, 2)],
            vec!["-c:s", "copy"],
        ),
        (
            "substripped",
            None,
            ImageImplicitSubtitle::Strip,
            vec![t(0, 2)],
            vec!["-sn"],
        ),
        (
            "audcopy",
            Some(ImageImplicitAudio::Copy),
            ImageImplicitSubtitle::Strip,
            vec![t(0, 1)],
            vec!["-c:a", "copy", "-sn"],
        ),
        (
            "audstrip",
            Some(ImageImplicitAudio::Strip),
            ImageImplicitSubtitle::Strip,
            vec![t(0, 1)],
            vec!["-an", "-sn"],
        ),
        (
            "audaac",
            Some(ImageImplicitAudio::Encode {
                codec: AudioCodec::Aac,
                tracks: vec![ImageImplicitAudioTrack {
                    source: t(0, 1),
                    channels: 1,
                    bitrate_kbps: Some(64),
                    normalization: None,
                }],
            }),
            ImageImplicitSubtitle::Strip,
            vec![t(0, 1)],
            vec![
                "-c:a",
                "aac",
                "-aac_coder",
                "twoloop",
                "-b:a:0",
                "64k",
                "-ac:a:0",
                "1",
                "-sn",
            ],
        ),
    ];
    let original = std::fs::read(&input).unwrap();
    for (name, audio, subtitle, checked, options) in modes {
        for format in [
            ImageOutput::PngSequence,
            ImageOutput::JpegSequence,
            ImageOutput::Gif,
        ] {
            let out = f.0.join(format!(
                "{name}-{format:?}.{}",
                if format == ImageOutput::Gif {
                    "gif"
                } else {
                    "dir"
                }
            ));
            let reference = f.0.join(format!(
                "ref-{name}-{format:?}.{}",
                if format == ImageOutput::Gif {
                    "gif"
                } else {
                    "dir"
                }
            ));
            let mut request = req(std::slice::from_ref(&input), checked.clone(), &out, format);
            if let ImageRequest::ImplicitExport {
                audio: a,
                subtitle_codec: s,
                ..
            } = &mut request
            {
                *a = audio.clone();
                *s = subtitle;
            }
            let source_result =
                direct(std::slice::from_ref(&input), &reference, format, &options).await;
            let source_problem = name == "subcopy";
            if format == ImageOutput::Gif {
                assert_eq!(
                    source_result.status.success(),
                    !source_problem,
                    "{name}: {}",
                    String::from_utf8_lossy(&source_result.stderr)
                );
                if source_problem {
                    assert!(
                        String::from_utf8_lossy(&source_result.stderr)
                            .contains("does not support any stream of type")
                    );
                }
            } else {
                assert!(source_result.status.success());
                assert_eq!(
                    files(&reference).len() > 4,
                    source_problem,
                    "source {name} {format:?}"
                );
                if name == "subcopy" {
                    assert!(
                        files(&reference)
                            .iter()
                            .any(|path| std::fs::read(path).unwrap() == b"Caption")
                    );
                }
            }
            eprintln!(
                "source {name} {format:?}: success={}, numberedFiles={}",
                source_result.status.success(),
                if format == ImageOutput::Gif {
                    0
                } else {
                    files(&reference).len()
                }
            );
            let corrected = f.0.join(format!(
                "valid-{name}-{format:?}.{}",
                if format == ImageOutput::Gif {
                    "gif"
                } else {
                    "dir"
                }
            ));
            assert!(
                direct(
                    std::slice::from_ref(&input),
                    &corrected,
                    format,
                    &["-an", "-sn", "-dn"]
                )
                .await
                .status
                .success()
            );
            let result = native(request).await.unwrap();
            assert_eq!(
                (result.frame_count, result.width, result.height),
                (4, 64, 48)
            );
            if format != ImageOutput::Gif {
                let a = files(&out);
                let b = files(&corrected);
                assert_eq!(a.len(), 4);
                for (index, path) in a.iter().enumerate() {
                    assert_eq!(
                        path.file_name().unwrap().to_string_lossy(),
                        format!(
                            "{:08}.{}",
                            index + 1,
                            if format == ImageOutput::PngSequence {
                                "png"
                            } else {
                                "jpg"
                            }
                        )
                    );
                }
                assert_eq!(a.len(), b.len());
                for (a, b) in a.iter().zip(&b) {
                    assert_eq!(a.file_name(), b.file_name(), "Source %8d filename differs");
                    assert_eq!(
                        std::fs::read(a).unwrap(),
                        std::fs::read(b).unwrap(),
                        "Encoded packets differ for corrected {name} {format:?}"
                    );
                }
            } else {
                assert_eq!(pixels(&out).await, pixels(&corrected).await);
            }
            clean(&f.0);
        }
    }
    assert_eq!(std::fs::read(input).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn all_loaded_inputs_and_multiple_videos_participate_in_automatic_selection() {
    let f = Fixture::new();
    let small = source(&f.0, "small", false).await;
    let large = source(&f.0, "large", true).await;
    for (order, current) in [
        (vec![small.clone(), large.clone()], 0),
        (vec![large.clone(), small.clone()], 1),
    ] {
        for format in [ImageOutput::PngSequence, ImageOutput::JpegSequence] {
            let out = f.0.join(format!("selected-{current}-{format:?}"));
            let reference = f.0.join(format!("ref-selected-{current}-{format:?}"));
            let mut request = req(&order, vec![t(current, 2)], &out, format);
            if let ImageRequest::ImplicitExport {
                current_input,
                subtitle_codec,
                ..
            } = &mut request
            {
                *current_input = current;
                *subtitle_codec = ImageImplicitSubtitle::Strip;
            }
            let actual = native(request).await.unwrap();
            assert_eq!(
                (actual.frame_count, actual.width, actual.height),
                (8, 128, 96)
            );
            assert!(
                direct(&order, &reference, format, &["-sn"])
                    .await
                    .status
                    .success()
            );
            for (a, b) in files(&out).iter().zip(files(&reference)) {
                assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
            }
        }
    }
    clean(&f.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn source_trim_uses_current_file_clock_and_repeats_input_seek() {
    let f = Fixture::new();
    let small = source(&f.0, "clock", false).await;
    let large = source(&f.0, "automatic", true).await;
    let inputs = vec![small, large];
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    for (index, trim, keep, expected) in [
        (
            0,
            VideoTrim {
                start_frame: 1,
                end_frame_exclusive: 3,
                time: None,
            },
            true,
            2,
        ),
        (
            1,
            VideoTrim {
                start_frame: 1,
                end_frame_exclusive: 3,
                time: None,
            },
            false,
            5,
        ),
        (
            2,
            VideoTrim {
                start_frame: 0,
                end_frame_exclusive: 0,
                time: Some(VideoTimeTrim {
                    start_milliseconds: 250,
                    end_milliseconds: 750,
                    seek_strategy: VideoSeekStrategy::InputSeek,
                }),
            },
            true,
            4,
        ),
        (
            3,
            VideoTrim {
                start_frame: 0,
                end_frame_exclusive: 0,
                time: Some(VideoTimeTrim {
                    start_milliseconds: 250,
                    end_milliseconds: 750,
                    seek_strategy: VideoSeekStrategy::DecodeFromStart,
                }),
            },
            true,
            4,
        ),
    ] {
        let out = f.0.join(format!("trim-{index}"));
        let mut request = req(&inputs, vec![t(0, 2)], &out, ImageOutput::PngSequence);
        if let ImageRequest::ImplicitExport {
            trim: t,
            keep_frame_count: k,
            subtitle_codec,
            ..
        } = &mut request
        {
            *t = Some(trim);
            *k = keep;
            *subtitle_codec = ImageImplicitSubtitle::Strip;
        }
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
        assert!(!review.arguments.contains("'-map'"));
        assert!(!review.arguments.contains("'-vf'"));
        assert!(!review.arguments.contains("'-c:v'"));
        if index == 2 {
            assert_eq!(review.arguments.matches("'-ss'").count(), 2);
        }
        if index == 0 {
            assert!(review.arguments.contains("'0.125'"));
            assert!(review.arguments.contains("'0.625'"));
        }
        let CommandEditResult::Image { result } = manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments: review.arguments,
                },
                cancel,
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(result.frame_count, expected, "trim mode {index}");
        assert_eq!((result.width, result.height), (128, 96));
    }
    manager.shutdown().await;
    clean(&f.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn invalid_selection_no_video_and_checked_video_remain_distinct() {
    let f = Fixture::new();
    let input = source(&f.0, "valid", false).await;
    for checked in [
        vec![],
        vec![t(0, 0)],
        vec![t(2, 1)],
        vec![t(0, 999)],
        vec![t(0, 2), t(0, 2)],
    ] {
        let out =
            f.0.join(format!("invalid-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        assert_eq!(
            native(req(
                std::slice::from_ref(&input),
                checked,
                &out,
                ImageOutput::PngSequence
            ))
            .await
            .unwrap_err()
            .code,
            "IMAGE_SELECTION_INVALID"
        );
        assert!(!out.exists());
    }
    let empty = f.0.join("empty");
    assert_eq!(
        native(req(&[], vec![t(0, 0)], &empty, ImageOutput::PngSequence))
            .await
            .unwrap_err()
            .code,
        "IMAGE_SELECTION_INVALID"
    );
    let audio = f.0.join("audio.mka");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "sine=duration=1",
        "-c:a",
        "pcm_s16le",
    ]);
    a.push(audio.as_os_str().to_owned());
    run(a).await;
    let out = f.0.join("novideo");
    let mut request = req(
        std::slice::from_ref(&audio),
        vec![t(0, 0)],
        &out,
        ImageOutput::PngSequence,
    );
    if let ImageRequest::ImplicitExport { audio, .. } = &mut request {
        *audio = Some(ImageImplicitAudio::Strip);
    }
    let error = native(request).await.unwrap_err();
    assert!(
        error.message.contains("does not contain any stream"),
        "{error:?}"
    );
    assert!(!out.exists());
    clean(&f.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and Unix shell"]
async fn review_holds_all_candidates_and_preserves_mutation_collision_cancellation_guards() {
    let f = Fixture::new();
    let small = source(&f.0, "guard", false).await;
    let large = source(&f.0, "candidate", true).await;
    let inputs = vec![small.clone(), large.clone()];
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let prepare = |out: &Path| {
        let mut request = req(&inputs, vec![t(0, 2)], out, ImageOutput::PngSequence);
        if let ImageRequest::ImplicitExport { subtitle_codec, .. } = &mut request {
            *subtitle_codec = ImageImplicitSubtitle::Strip;
        }
        request
    };
    let (_owner, cancel) = watch::channel(false);
    let output = f.0.join("closed");
    let review = manager
        .prepare_command_edit(
            CommandEditRequest::Image {
                request: Box::new(prepare(&output)),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    assert!(!output.exists());
    let spool = files(&f.0)
        .into_iter()
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains("implicit-spool")
        })
        .unwrap();
    assert_eq!(std::fs::metadata(&spool).unwrap().len(), 0);
    manager.cancel_command_edit(&review.id).await.unwrap();
    assert!(!spool.exists());
    let output = f.0.join("collision");
    let review = manager
        .prepare_command_edit(
            CommandEditRequest::Image {
                request: Box::new(prepare(&output)),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    std::fs::create_dir(&output).unwrap();
    std::fs::write(output.join("personal.txt"), "keep").unwrap();
    let error = manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id,
                arguments: review.arguments,
            },
            cancel.clone(),
        )
        .await
        .unwrap_err();
    assert!(
        error.message.contains("exist") || error.code == "OUTPUT_EXISTS",
        "{error:?}"
    );
    assert_eq!(std::fs::read(output.join("personal.txt")).unwrap(), b"keep");
    clean(&f.0);
    let output = f.0.join("mutated");
    let review = manager
        .prepare_command_edit(
            CommandEditRequest::Image {
                request: Box::new(prepare(&output)),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    let original = std::fs::read(&large).unwrap();
    std::fs::write(&large, b"changed automatic candidate").unwrap();
    assert_eq!(
        manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments: review.arguments
                },
                cancel.clone()
            )
            .await
            .unwrap_err()
            .code,
        "SOURCE_CHANGED"
    );
    std::fs::write(&large, original).unwrap();
    assert!(!output.exists());
    clean(&f.0);
    #[cfg(unix)]
    {
        let output = f.0.join("cancelled");
        let review = manager
            .prepare_command_edit(
                CommandEditRequest::Image {
                    request: Box::new(prepare(&output)),
                },
                cancel.clone(),
            )
            .await
            .unwrap();
        let program = format!("sleep 30 && {}", review.arguments);
        let confirming = manager.confirm_command_edit(
            CommandEditConfirmation {
                id: review.id,
                arguments: program,
            },
            cancel.clone(),
        );
        let cancelling = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            manager.cancel_command_reviews().await.unwrap();
        };
        let (result, ()) = tokio::join!(confirming, cancelling);
        assert!(result.is_err());
        assert!(!output.exists());
        clean(&f.0);
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and Unix shell"]
async fn frozen_nonvideo_settings_are_ignored_while_metadata_trim_and_edited_geometry_apply() {
    let f = Fixture::new();
    let input = source(&f.0, "metadata", false).await;
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let (_owner, cancel) = watch::channel(false);
    for edited in [false, true] {
        let output = f.0.join(format!("metadata-{edited}"));
        let mut request = req(
            std::slice::from_ref(&input),
            vec![t(0, 2), t(0, 1)],
            &output,
            ImageOutput::PngSequence,
        );
        if let ImageRequest::ImplicitExport {
            audio,
            subtitle_codec,
            metadata,
            trim,
            ..
        } = &mut request
        {
            *audio = Some(ImageImplicitAudio::Encode {
                codec: AudioCodec::Aac,
                tracks: vec![ImageImplicitAudioTrack {
                    source: t(0, 1),
                    channels: 2,
                    bitrate_kbps: Some(192),
                    normalization: Some(AudioNormalization {
                        target: LoudnessTarget::Lufs16,
                    }),
                }],
            });
            *subtitle_codec = ImageImplicitSubtitle::MovText;
            *metadata = ImageImplicitMetadata {
                source_input: Some(0),
                chapters_input: Some(0),
                title: Some("Frozen ' title".into()),
                language: Some("eng".into()),
                tracks: vec![
                    ImageImplicitTrackMetadata {
                        title: Some("checked subtitle output ordinal zero".into()),
                        language: Some("jpn".into()),
                    },
                    ImageImplicitTrackMetadata {
                        title: Some("checked audio output ordinal one".into()),
                        language: None,
                    },
                ],
                audio_dispositions: vec![true],
                subtitle_dispositions: vec![false],
                ..Default::default()
            };
            *trim = Some(VideoTrim {
                start_frame: 0,
                end_frame_exclusive: 0,
                time: Some(VideoTimeTrim {
                    start_milliseconds: 250,
                    end_milliseconds: 750,
                    seek_strategy: VideoSeekStrategy::DecodeFromStart,
                }),
            });
        }
        let review = manager
            .prepare_command_edit(
                CommandEditRequest::Image {
                    request: Box::new(request),
                },
                cancel.clone(),
            )
            .await
            .unwrap();
        assert!(review.arguments.contains("'-an' '-sn' '-dn'"));
        assert!(!review.arguments.contains("loudnorm"));
        assert!(!review.arguments.contains("'-c:a'"));
        assert!(!review.arguments.contains("'-c:s'"));
        assert!(review.arguments.contains("'-map_metadata' '0'"));
        assert!(review.arguments.contains("'-map_chapters' '0'"));
        assert!(
            review
                .arguments
                .contains("'-metadata:s:0' 'title=checked subtitle output ordinal zero'")
        );
        assert!(
            review
                .arguments
                .contains("'-metadata:s:1' 'title=checked audio output ordinal one'")
        );
        let arguments = if edited {
            review
                .arguments
                .replace("'-f' 'image2pipe'", "'-vf' 'scale=32:24' '-f' 'image2pipe'")
        } else {
            review.arguments
        };
        let CommandEditResult::Image { result } = manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments,
                },
                cancel.clone(),
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(result.frame_count, 2);
        assert_eq!(
            (result.width, result.height),
            if edited { (32, 24) } else { (64, 48) }
        );
        assert_eq!(files(&output).len(), 2);
    }
    manager.shutdown().await;
    clean(&f.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn equal_resolution_ties_follow_loaded_input_order() {
    let f = Fixture::new();
    let red = source(&f.0, "tie", false).await;
    let blue = f.0.join("blue.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(red.as_os_str().to_owned());
    a.extend(args(&[
        "-map",
        "0",
        "-vf",
        "lutrgb=r=0:g=0:b=255",
        "-c:v",
        "ffv1",
        "-c:a",
        "copy",
        "-c:s",
        "copy",
        "-disposition:v",
        "0",
        "-threads",
        "2",
    ]));
    a.push(blue.as_os_str().to_owned());
    run(a).await;
    for (index, order) in [vec![red.clone(), blue.clone()], vec![blue, red]]
        .into_iter()
        .enumerate()
    {
        let out = f.0.join(format!("tie-{index}"));
        let mut request = req(&order, vec![t(0, 2)], &out, ImageOutput::PngSequence);
        if let ImageRequest::ImplicitExport { subtitle_codec, .. } = &mut request {
            *subtitle_codec = ImageImplicitSubtitle::Strip;
        }
        let result = native(request).await.unwrap();
        assert_eq!(
            (result.frame_count, result.width, result.height),
            (4, 64, 48)
        );
        let pixel = pixels(&files(&out)[0]).await;
        assert!(
            if index == 0 {
                pixel[0] > 240 && pixel[2] < 10
            } else {
                pixel[0] < 10 && pixel[2] > 240
            },
            "order {index}: {:?}",
            &pixel[..3]
        );
    }
    clean(&f.0);
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and Unix shell"]
#[cfg(unix)]
async fn implicit_active_image_spooling_is_canceled_and_joined_before_return() {
    let f = Fixture::new();
    let input = source(&f.0, "spooling", false).await;
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let output = f.0.join("stopped");
    let (_owner, cancel) = watch::channel(false);
    let request = req(
        std::slice::from_ref(&input),
        vec![t(0, 2)],
        &output,
        ImageOutput::PngSequence,
    );
    let review = manager
        .prepare_command_edit(
            CommandEditRequest::Image {
                request: Box::new(request),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    let spool = files(&f.0)
        .into_iter()
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains("implicit-spool")
        })
        .unwrap();
    let arguments = review.arguments.replace("'-i'", "'-re' '-i'").replace(
        "'-f' 'image2pipe'",
        "'-vf' 'noise=alls=100:allf=t' '-f' 'image2pipe'",
    );
    let confirmation = manager.confirm_command_edit(
        CommandEditConfirmation {
            id: review.id,
            arguments,
        },
        cancel,
    );
    let cancellation = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if std::fs::metadata(&spool).is_ok_and(|metadata| metadata.len() > 0) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        manager.cancel_command_reviews().await.unwrap();
    };
    let (result, ()) = tokio::join!(confirmation, cancellation);
    assert!(result.is_err());
    assert!(!output.exists());
    assert!(!spool.exists());
    clean(&f.0);
    manager.shutdown().await;
}
