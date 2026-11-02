//! Actual fixed-format caption timing, original source protection and naming.
use media_core::*;
use media_runtime::{
    jobs::run_image_job,
    supervisor::{CommandSpec, run_capture},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-image-captions-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
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
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
async fn tool(name: &str, a: Vec<OsString>) -> Vec<u8> {
    let tools = media_runtime::get_capabilities().await;
    let executable = tools
        .into_iter()
        .find(|tool| tool.id == name)
        .unwrap()
        .path
        .unwrap();
    let (_owner, cancel) = watch::channel(false);
    let result = run_capture(
        &CommandSpec {
            executable: executable.into(),
            args: a,
            cwd: None,
        },
        cancel,
        32 * 1024 * 1024,
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
async fn fixture(root: &Path, frames: u32) -> (PathBuf, PathBuf) {
    let subtitle = root.join("caption ' : [,].srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,500 --> 00:00:01,500\nVISIBLE CAPTION\n\n",
    )
    .unwrap();
    let input = root.join("picture ' : [,].mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        &format!(
            "color=black:size=320x180:rate=4:duration={}",
            frames as f64 / 4.0
        ),
        "-i",
    ]);
    a.push(subtitle.as_os_str().to_owned());
    a.extend(args(&[
        "-map",
        "0:v:0",
        "-map",
        "1:s:0",
        "-c:v",
        "ffv1",
        "-threads",
        "2",
        "-c:s",
        "copy",
        "-attach",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "-metadata:s:t",
        "mimetype=application/x-truetype-font",
    ]));
    a.push(input.as_os_str().to_owned());
    tool("ffmpeg", a).await;
    (input, subtitle)
}
fn request(
    input: &Path,
    output: &Path,
    format: ImageOutput,
    selection: ImageSubtitleSource,
) -> ImageRequest {
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
        processing: Some(ImageProcessingSettings {
            subtitle_burn_in: Some(selection),
            trim: Some(VideoTrim {
                start_frame: 1,
                end_frame_exclusive: 9,
                time: None,
            }),
            framing: VideoFraming {
                crop: CropSettings {
                    top: 4,
                    left: 4,
                    right: 4,
                    bottom: 4,
                },
                resize_width: Some(156),
                borders: BorderSettings {
                    top: 8,
                    left: 8,
                    right: 8,
                    bottom: 8,
                },
                ..Default::default()
            },
            ..Default::default()
        }),
    }
}
async fn decode(output: &Path, format: ImageOutput) -> Vec<Vec<u8>> {
    let mut a = args(&["-v", "error"]);
    let path = match format {
        ImageOutput::PngSequence => output.join("%8d.png"),
        ImageOutput::JpegSequence => output.join("%8d.jpg"),
        _ => output.to_owned(),
    };
    if matches!(format, ImageOutput::PngSequence | ImageOutput::JpegSequence) {
        a.extend(args(&["-f", "image2", "-framerate", "4"]));
    }
    a.push("-i".into());
    a.push(path.into_os_string());
    a.extend(args(&[
        "-map",
        "0:v:0",
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-",
    ]));
    tool("ffmpeg", a)
        .await
        .as_chunks::<{ 172 * 102 * 3 }>()
        .0
        .iter()
        .map(|frame| frame.to_vec())
        .collect()
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe libass and PNG/JPEG/GIF encoders"]
async fn primary_and_external_captions_burn_into_every_fixed_format_at_trimmed_clock() {
    let root = Fixture::new();
    let (input, subtitle) = fixture(&root.0, 12).await;
    let original = std::fs::read(&input).unwrap();
    let original_subtitle = std::fs::read(&subtitle).unwrap();
    for external in [false, true] {
        for format in [
            ImageOutput::PngSequence,
            ImageOutput::JpegSequence,
            ImageOutput::Gif,
        ] {
            let output = root.0.join(format!(
                "out ' : [,]-{external}-{format:?}{}",
                if format == ImageOutput::Gif {
                    ".gif"
                } else {
                    ""
                }
            ));
            let selection = if external {
                ImageSubtitleSource::External {
                    input_path: subtitle.to_string_lossy().into_owned(),
                    stream_index: 0,
                    offset_milliseconds: 250,
                }
            } else {
                ImageSubtitleSource::Primary { stream_index: 1 }
            };
            let (_owner, cancel) = watch::channel(false);
            let result = run_image_job(request(&input, &output, format, selection), cancel)
                .await
                .unwrap();
            assert_eq!(result.frame_count, 8);
            assert_eq!((result.width, result.height), (172, 102));
            let frames = decode(&output, format).await;
            assert_eq!(frames.len(), 8);
            let visible: Vec<_> = frames
                .iter()
                .map(|frame| {
                    frame
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .filter(|rgb| rgb.iter().any(|value| *value > 80))
                        .count()
                })
                .collect();
            let expected = if external { 2..6 } else { 1..5 };
            for (index, pixels) in visible.iter().enumerate() {
                assert_eq!(
                    *pixels > 10,
                    expected.contains(&index),
                    "{external} {format:?}: {visible:?}"
                );
            }
            for frame in &frames {
                assert!(
                    frame[..172 * 7 * 3].iter().all(|value| *value < 25),
                    "Caption must remain inside the picture above the added border"
                );
            }
        }
    }
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_eq!(std::fs::read(&subtitle).unwrap(), original_subtitle);
    assert!(std::fs::read_dir(&root.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jesses-")
    }));
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe libass and PNG encoder"]
async fn resolver_to_execution_race_never_overwrites_and_sequence_default_keeps_original_folder() {
    let root = Fixture::new();
    let (input, subtitle) = fixture(&root.0, 12).await;
    let resolved = media_runtime::resolve_output_path(OutputPathRequest {
        input_path: input.to_string_lossy().into_owned(),
        output_stem: Some(root.0.join("result").to_string_lossy().into_owned()),
        default_output_directory: None,
        target: OutputTarget::Image {
            format: ImageOutput::Gif,
        },
    })
    .await
    .unwrap();
    assert!(!Path::new(&resolved.output_path).exists());
    std::fs::write(&resolved.output_path, b"a later owner's output").unwrap();
    let (_owner, cancel) = watch::channel(false);
    assert!(
        run_image_job(
            request(
                &input,
                Path::new(&resolved.output_path),
                ImageOutput::Gif,
                ImageSubtitleSource::Primary { stream_index: 1 }
            ),
            cancel
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read(&resolved.output_path).unwrap(),
        b"a later owner's output"
    );
    let folder = root.0.join("source.frames.v1");
    std::fs::create_dir(&folder).unwrap();
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=black:size=32x32",
        "-frames:v",
        "1",
        "-threads",
        "2",
    ]);
    a.push(folder.join("not-numeric.png").into_os_string());
    tool("ffmpeg", a).await;
    let (_owner, cancel) = watch::channel(false);
    let media = media_runtime::import_image_sequence(
        ImageSequenceImportRequest {
            directory_path: folder.to_string_lossy().into_owned(),
            frame_rate: FrameRate {
                numerator: 30,
                denominator: 1,
            },
        },
        root.0.join("cache"),
        cancel,
    )
    .await
    .unwrap();
    let naming = media_runtime::resolve_output_path(OutputPathRequest {
        input_path: media.path.clone(),
        output_stem: None,
        default_output_directory: None,
        target: OutputTarget::Image {
            format: ImageOutput::PngSequence,
        },
    })
    .await
    .unwrap();
    assert_eq!(
        Path::new(&naming.output_path),
        root.0.join("source.frames.v1 (1)")
    );
    assert!(!Path::new(&naming.output_path).exists());
    let Some(MediaSource::ImageSequence {
        sequence_fingerprint,
        ..
    }) = media.source
    else {
        panic!()
    };
    media_runtime::release_image_sequence(
        ImageSequenceReleaseRequest {
            input_path: media.path,
            sequence_fingerprint,
        },
        root.0.join("cache"),
    )
    .unwrap();
    assert!(subtitle.exists());
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe libass and PNG encoder"]
async fn changed_subtitle_or_picture_and_active_cancel_never_publish_or_remove_sources() {
    let root = Fixture::new();
    let (input, subtitle) = fixture(&root.0, 4000).await;
    let original_input = std::fs::read(&input).unwrap();
    let original_subtitle = std::fs::read(&subtitle).unwrap();
    for action in ["subtitle", "picture", "cancel"] {
        let output = root.0.join(format!("guard-{action}"));
        let mut r = request(
            &input,
            &output,
            ImageOutput::PngSequence,
            ImageSubtitleSource::External {
                input_path: subtitle.to_string_lossy().into_owned(),
                stream_index: 0,
                offset_milliseconds: 0,
            },
        );
        if let ImageRequest::Export {
            processing: Some(processing),
            ..
        } = &mut r
        {
            processing.trim = None;
        }
        let (owner, cancel) = watch::channel(false);
        let worker = tokio::spawn(async move { run_image_job(r, cancel).await });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if std::fs::read_dir(&root.0).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with("-subtitles")
            }) {
                break;
            }
            assert!(
                !worker.is_finished(),
                "Image job finished before guard test could act"
            );
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        match action {
            "subtitle" => std::fs::write(&subtitle, b"mutated subtitle").unwrap(),
            "picture" => {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&input)
                    .unwrap();
                use std::io::Write;
                file.write_all(b"mutated").unwrap();
            }
            _ => {
                owner.send_replace(true);
            }
        }
        let failure = worker.await.unwrap().unwrap_err();
        if action == "cancel" {
            assert_eq!(failure.code, "JOB_CANCELED", "{failure:#?}");
        }
        assert!(!output.exists());
        assert!(std::fs::read_dir(&root.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".jesses-")
        }));
        std::fs::write(&input, &original_input).unwrap();
        std::fs::write(&subtitle, &original_subtitle).unwrap();
    }
    assert_eq!(std::fs::read(&input).unwrap(), original_input);
    assert_eq!(std::fs::read(&subtitle).unwrap(), original_subtitle);
}

// Original PGS data: an opaque white 40x20 rectangle at (20,80) on a
// 320x180 canvas, visible on the half-open interval [0.25,1.5) seconds.
fn bitmap_fixture(path: &Path) {
    fn segment(bytes: &mut Vec<u8>, pts: u32, kind: u8, payload: &[u8]) {
        bytes.extend(b"PG");
        bytes.extend(pts.to_be_bytes());
        bytes.extend(pts.to_be_bytes());
        bytes.push(kind);
        bytes.extend((payload.len() as u16).to_be_bytes());
        bytes.extend(payload);
    }
    let mut bytes = Vec::new();
    segment(
        &mut bytes,
        22_500,
        0x16,
        &[
            1, 64, 0, 180, 0x10, 0, 0, 0x80, 0, 0, 1, 0, 0, 0, 0, 0, 20, 0, 80,
        ],
    );
    segment(&mut bytes, 22_500, 0x17, &[1, 0, 0, 0, 0, 0, 1, 64, 0, 180]);
    segment(
        &mut bytes,
        22_500,
        0x14,
        &[0, 0, 0, 16, 128, 128, 0, 1, 235, 128, 128, 255],
    );
    let mut rle = Vec::new();
    for _ in 0..20 {
        rle.extend([1; 40]);
        rle.extend([0, 0]);
    }
    let mut object = vec![0, 0, 0, 0xc0];
    object.extend(&((rle.len() + 4) as u32).to_be_bytes()[1..]);
    object.extend([0, 40, 0, 20]);
    object.extend(rle);
    segment(&mut bytes, 22_500, 0x15, &object);
    segment(&mut bytes, 22_500, 0x80, &[]);
    segment(
        &mut bytes,
        135_000,
        0x16,
        &[1, 64, 0, 180, 0x10, 0, 1, 0, 0, 0, 0],
    );
    segment(&mut bytes, 135_000, 0x80, &[]);
    std::fs::write(path, bytes).unwrap();
}

async fn png_frames(output: &Path, width: usize, height: usize) -> Vec<Vec<u8>> {
    let mut command = args(&["-v", "error", "-f", "image2", "-framerate", "4", "-i"]);
    command.push(output.join("%8d.png").into_os_string());
    command.extend(args(&[
        "-map",
        "0:v:0",
        "-fps_mode",
        "passthrough",
        "-pix_fmt",
        "rgb24",
        "-f",
        "rawvideo",
        "-",
    ]));
    let data = tool("ffmpeg", command).await;
    assert_eq!(data.len() % (width * height * 3), 0);
    data.chunks_exact(width * height * 3)
        .map(|frame| frame.to_vec())
        .collect()
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe PGS decoder, overlay and PNG encoder"]
async fn image_bitmap_captions_use_source_geometry_and_primary_or_external_clock() {
    let root = Fixture::new();
    let bitmap = root.0.join("caption ' : [,].sup");
    bitmap_fixture(&bitmap);
    let bitmap_original = std::fs::read(&bitmap).unwrap();
    for shifted in [false, true] {
        let input = root.0.join(format!("bitmap-{shifted}.mkv"));
        let mut create = args(&[
            "-v",
            "error",
            "-copyts",
            "-f",
            "lavfi",
            "-i",
            "color=black:size=320x180:rate=4:duration=3",
            "-i",
        ]);
        create.push(bitmap.as_os_str().to_owned());
        create.extend(args(&[
            "-map", "0:v:0", "-map", "1:s:0", "-c:v", "ffv1", "-threads", "2", "-c:s", "copy",
        ]));
        if shifted {
            create.extend(args(&["-output_ts_offset", "5"]));
        }
        create.push(input.as_os_str().to_owned());
        tool("ffmpeg", create).await;
        let original = std::fs::read(&input).unwrap();
        for (external, offset) in [(false, 0), (true, 0), (true, 250), (true, -250)] {
            let output = root.0.join(format!("bitmap-{shifted}-{external}-{offset}"));
            let selection = if external {
                ImageSubtitleSource::External {
                    input_path: bitmap.to_string_lossy().into_owned(),
                    stream_index: 0,
                    offset_milliseconds: offset,
                }
            } else {
                ImageSubtitleSource::Primary { stream_index: 1 }
            };
            let mut request = request(&input, &output, ImageOutput::PngSequence, selection);
            if let ImageRequest::Export {
                processing: Some(settings),
                ..
            } = &mut request
            {
                settings.framing = VideoFraming {
                    crop: CropSettings {
                        left: 32,
                        ..Default::default()
                    },
                    resize_width: Some(144),
                    borders: BorderSettings {
                        top: 8,
                        right: 16,
                        bottom: 24,
                        left: 16,
                    },
                    ..Default::default()
                };
            }
            let (_owner, cancel) = watch::channel(false);
            let result = run_image_job(request, cancel).await.unwrap();
            assert_eq!(
                (result.width, result.height, result.frame_count),
                (176, 122, 8)
            );
            let frames = png_frames(&output, 176, 122).await;
            assert_eq!(frames.len(), 8);
            for (index, frame) in frames.iter().enumerate() {
                let time = (index + 1) as f64 / 4.0;
                let expected = (0.25 + f64::from(offset) / 1000.0
                    ..1.5 + f64::from(offset) / 1000.0)
                    .contains(&time);
                let lit: Vec<_> = frame
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .enumerate()
                    .filter(|(_, rgb)| rgb.iter().all(|value| *value > 220))
                    .map(|(index, _)| (index % 176, index / 176))
                    .collect();
                assert_eq!(
                    !lit.is_empty(),
                    expected,
                    "shifted={shifted}, external={external}, offset={offset}, frame={index}"
                );
                if expected {
                    assert!(
                        (100..=160).contains(&lit.len()),
                        "{} white pixels",
                        lit.len()
                    );
                    assert!(
                        lit.iter()
                            .all(|(x, y)| (16..30).contains(x) && (48..58).contains(y)),
                        "Wrong bitmap placement: {lit:?}"
                    );
                }
            }
        }
        assert_eq!(std::fs::read(input).unwrap(), original);
    }
    assert_eq!(std::fs::read(bitmap).unwrap(), bitmap_original);
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe FFV1, libass and PNG encoder"]
async fn shifted_vfr_captions_use_actual_retained_frame_clock_for_frame_and_time_trims() {
    let root = Fixture::new();
    let subtitle = root.0.join("vfr.srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,750 --> 00:00:01,750\nVISIBLE CAPTION\n\n",
    )
    .unwrap();
    for shifted in [false, true] {
        let input = root.0.join(format!("vfr-{shifted}.mkv"));
        let mut create = args(&[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=black:size=320x180:rate=4:duration=2",
            "-i",
        ]);
        create.push(subtitle.as_os_str().to_owned());
        create.extend(args(&[
            "-map",
            "0:v:0",
            "-map",
            "1:s:0",
            "-vf",
            "setpts='if(lt(N,4),N/4,(N-4)/2+1)/TB'",
            "-fps_mode",
            "passthrough",
            "-c:v",
            "ffv1",
            "-threads",
            "2",
            "-c:s",
            "copy",
        ]));
        if shifted {
            create.extend(args(&["-output_ts_offset", "5"]));
        }
        create.push(input.as_os_str().to_owned());
        tool("ffmpeg", create).await;
        let original = std::fs::read(&input).unwrap();
        let mut probe = args(&[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp_time",
            "-of",
            "json",
            "-i",
        ]);
        probe.push(input.as_os_str().to_owned());
        let document: serde_json::Value =
            serde_json::from_slice(&tool("ffprobe", probe).await).unwrap();
        let times: Vec<_> = document["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|frame| {
                frame["best_effort_timestamp_time"]
                    .as_str()
                    .unwrap()
                    .parse::<f64>()
                    .unwrap()
            })
            .collect();
        let origin = if shifted { 5.0 } else { 0.0 };
        assert_eq!(
            times,
            vec![
                origin,
                origin + 0.25,
                origin + 0.5,
                origin + 0.75,
                origin + 1.0,
                origin + 1.5,
                origin + 2.0,
                origin + 2.5
            ]
        );
        for external in [false, true] {
            for mode in ["full", "frames", "time"] {
                let output = root.0.join(format!("vfr-{shifted}-{external}-{mode}"));
                let selection = if external {
                    ImageSubtitleSource::External {
                        input_path: subtitle.to_string_lossy().into_owned(),
                        stream_index: 0,
                        offset_milliseconds: 0,
                    }
                } else {
                    ImageSubtitleSource::Primary { stream_index: 1 }
                };
                let mut request = request(&input, &output, ImageOutput::PngSequence, selection);
                let selected: Vec<_> = match mode {
                    "full" => (0..8).collect(),
                    _ => (5..8).collect(),
                };
                if let ImageRequest::Export {
                    processing: Some(settings),
                    ..
                } = &mut request
                {
                    settings.framing = Default::default();
                    settings.trim = match mode {
                        "full" => None,
                        "frames" => Some(VideoTrim {
                            start_frame: 5,
                            end_frame_exclusive: 8,
                            time: None,
                        }),
                        _ => Some(VideoTrim {
                            start_frame: 0,
                            end_frame_exclusive: 0,
                            time: Some(VideoTimeTrim {
                                start_milliseconds: 1300,
                                end_milliseconds: 2600,
                                ..Default::default()
                            }),
                        }),
                    };
                }
                let (_owner, cancel) = watch::channel(false);
                let result = run_image_job(request, cancel).await.unwrap();
                assert_eq!(result.frame_count, selected.len() as u32);
                let frames = png_frames(&output, 320, 180).await;
                assert_eq!(frames.len(), selected.len());
                for (frame, index) in frames.iter().zip(selected) {
                    let visible = frame
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .filter(|rgb| rgb.iter().any(|value| *value > 80))
                        .count()
                        > 10;
                    assert_eq!(
                        visible,
                        (0.75..1.75).contains(&(times[index] - origin)),
                        "shifted={shifted}, external={external}, mode={mode}, source frame={index}"
                    );
                }
            }
        }
        assert_eq!(std::fs::read(input).unwrap(), original);
    }
}

#[tokio::test]
#[ignore = "requires real VSPipe/R79, havsfunc with QTGMC plugins, FFmpeg/FFprobe libass/PGS/PNG"]
async fn qtgmc_fixed_images_render_text_and_bitmap_before_verified_intermediate_export() {
    let root = Fixture::new();
    let subtitle = root.0.join("caption.srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,250 --> 00:00:01,500\nVISIBLE CAPTION\n\n",
    )
    .unwrap();
    let bitmap = root.0.join("caption.sup");
    bitmap_fixture(&bitmap);
    let input = root.0.join("interlaced.mkv");
    let mut create = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=black:size=320x180:rate=4:duration=3",
        "-vf",
        "setfield=tff,setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-threads",
        "2",
        "-chroma_sample_location",
        "left",
    ]);
    create.push(input.as_os_str().to_owned());
    tool("ffmpeg", create).await;
    let original = std::fs::read(&input).unwrap();
    for (name, subtitle) in [("text", subtitle), ("bitmap", bitmap)] {
        for mode in [DeinterlaceMode::Frame, DeinterlaceMode::Bob] {
            let original_subtitle = std::fs::read(&subtitle).unwrap();
            let output = root.0.join(format!("qtgmc-{name}-{mode:?}"));
            let selection = ImageSubtitleSource::External {
                input_path: subtitle.to_string_lossy().into_owned(),
                stream_index: 0,
                offset_milliseconds: 0,
            };
            let mut request = request(&input, &output, ImageOutput::PngSequence, selection);
            if let ImageRequest::Export {
                processing: Some(settings),
                ..
            } = &mut request
            {
                settings.framing = Default::default();
                settings.temporal = Some(TemporalSettings {
                    qtgmc: Some(QtgmcSettings {
                        mode,
                        field_order: FieldOrder::TopFirst,
                        preset: QtgmcPreset::Fast,
                    }),
                    ..Default::default()
                });
            }
            let (_owner, cancel) = watch::channel(false);
            let result = run_image_job(request, cancel).await.unwrap();
            let factor = if mode == DeinterlaceMode::Bob { 2 } else { 1 };
            assert_eq!(
                (result.width, result.height, result.frame_count),
                (320, 180, 8 * factor)
            );
            let frames = png_frames(&output, 320, 180).await;
            assert_eq!(frames.len(), (8 * factor) as usize);
            for (index, frame) in frames.iter().enumerate() {
                let time = 0.25 + index as f64 / f64::from(4 * factor);
                let visible = frame
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .filter(|rgb| rgb.iter().any(|value| *value > 80))
                    .count()
                    > 10;
                assert_eq!(
                    visible,
                    (0.25..1.5).contains(&time),
                    "{name}, {mode:?}, frame {index}"
                );
            }
            assert_eq!(std::fs::read(&subtitle).unwrap(), original_subtitle);
        }
    }
    assert_eq!(std::fs::read(input).unwrap(), original);
    assert!(std::fs::read_dir(&root.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jesses-")
    }));
}
