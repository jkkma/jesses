//! Real-tool parity checks. Run explicitly with the qualified FFmpeg/FFprobe pair.
use media_core::{
    AudioCodec, AudioTrackSettings, EncodeRequest, EncodeSettings, JobState, RemuxRequest,
    VideoMode,
};
use media_runtime::{
    jobs::JobManager,
    supervisor::{self, CommandSpec},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe MPEG-TS MPEG2/MP2 and standalone x264"]
async fn transport_stream_audio_lead_keeps_packet_timestamps_after_normalization() {
    let fixture = Fixture::new();
    let input = fixture.0.join("transport.ts");
    let output = fixture.0.join("transport.mkv");
    let (_owner, cancel) = watch::channel(false);
    let mut args: Vec<std::ffi::OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=128x96:rate=24:duration=2",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=2",
        "-c:v",
        "mpeg2video",
        "-bf",
        "0",
        "-c:a",
        "mp2",
        "-f",
        "mpegts",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    args.push(input.as_os_str().to_owned());
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: "ffmpeg".into(),
            args,
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let original = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![0, 1],
            },
            settings: EncodeSettings {
                encoder: media_core::VideoEncoder::X264,
                preset: 0,
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let completed = finish(&manager, &job.id).await;
    assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    manager.shutdown().await;
    let (_owner, cancel) = watch::channel(false);
    let probe = supervisor::run_capture(
        &CommandSpec {
            executable: "ffprobe".into(),
            args: vec![
                "-v".into(),
                "error".into(),
                "-show_streams".into(),
                "-show_format".into(),
                "-of".into(),
                "json".into(),
                output.as_os_str().to_owned(),
            ],
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(doc["streams"][0]["start_time"], "0.000000");
    assert!(
        doc["streams"][1]["start_time"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            < 0.0
    );
    assert!(
        (doc["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            - 2.0)
            .abs()
            < 0.05
    );
    assert_eq!(std::fs::read(&input).unwrap(), original);
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-migration-{}-{}",
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
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn generate(path: &Path) {
    let (_owner, cancel) = watch::channel(false);
    let mut args: Vec<std::ffi::OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=128x96:rate=24:duration=2",
        "-f",
        "lavfi",
        "-i",
        "sine=sample_rate=48000:duration=2",
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "mpeg4",
        "-g",
        "12",
        "-bf",
        "0",
        "-c:a",
        "pcm_s16le",
        "-metadata",
        "title=Migration fixture",
        "-metadata:s:a:0",
        "language=eng",
        "-metadata:s:a:0",
        "title=Main audio",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    args.push(path.as_os_str().to_owned());
    let output = supervisor::run_capture(
        &CommandSpec {
            executable: "ffmpeg".into(),
            args,
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
async fn finish(manager: &JobManager, id: &str) -> media_core::JobSnapshot {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("conversion completed")
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe on PATH"]
async fn converts_audio_only_m4a_ogg_and_copy_video_without_a_video_encoder() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    generate(&input).await;
    let source_bytes = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    for (index, mode, codec, extension, expected) in [
        (0, VideoMode::Drop, AudioCodec::Flac, "mkv", "flac"),
        (1, VideoMode::Drop, AudioCodec::Aac, "m4a", "aac"),
        (2, VideoMode::Drop, AudioCodec::Opus, "ogg", "opus"),
        (3, VideoMode::Copy, AudioCodec::Flac, "mkv", "flac"),
        (4, VideoMode::Drop, AudioCodec::Vorbis, "ogg", "vorbis"),
    ] {
        let output = fixture.0.join(format!("output-{index}.{extension}"));
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: if mode == VideoMode::Drop {
                        vec![1]
                    } else {
                        vec![0, 1]
                    },
                },
                settings: EncodeSettings {
                    video_mode: mode,
                    audio: vec![AudioTrackSettings {
                        normalization: None,
                        stream_index: 1,
                        codec,
                        bitrate_kbps: 128,
                        channels: Default::default(),
                        gain: None,
                    }],
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let job = finish(&manager, &job.id).await;
        assert_eq!(
            job.state,
            JobState::Succeeded,
            "{extension} {codec:?}: {job:#?}"
        );
        let output_media = media_runtime::probe_media(output.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(
            output_media
                .streams
                .iter()
                .filter(|s| s.kind == "video")
                .count(),
            usize::from(mode == VideoMode::Copy)
        );
        assert_eq!(
            output_media
                .streams
                .iter()
                .find(|s| s.kind == "audio")
                .unwrap()
                .codec
                .as_deref(),
            Some(expected)
        );
        assert_eq!(std::fs::read(&input).unwrap(), source_bytes);
    }
    manager.shutdown().await;
}
#[tokio::test]
#[ignore = "requires real FFmpeg/FFprobe on PATH"]
async fn keyframe_query_preserves_an_exact_keyframe_and_snaps_between_keyframes() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    generate(&input).await;
    let (_owner, cancel) = watch::channel(false);
    for (wanted, expected) in [(0.0, 0.0), (0.5, 0.5), (0.75, 0.5), (1.49, 1.0)] {
        let result = media_runtime::snap_keyframe(
            media_core::KeyframeSnapRequest {
                input_path: input.to_string_lossy().into_owned(),
                video_stream_index: 0,
                position_seconds: wanted,
            },
            cancel.clone(),
        )
        .await
        .unwrap();
        assert!(
            (result.position_seconds.unwrap() - expected).abs() < 0.001,
            "{wanted}: {result:?}"
        );
        assert_eq!(result.source_fingerprint.len(), 64);
    }
    let result = media_runtime::detect_fields(
        media_core::FieldDetectionRequest {
            input_path: input.to_string_lossy().into_owned(),
            video_stream_index: 0,
        },
        cancel,
    )
    .await
    .unwrap();
    assert_eq!(
        result.order,
        media_core::DetectedFieldOrder::Progressive,
        "{result:?}"
    );
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe on PATH"]
async fn true_audio_only_input_and_sample_trim_and_keyframe_copy_trim() {
    let fixture = Fixture::new();
    let av = fixture.0.join("source.mkv");
    generate(&av).await;
    let audio = fixture.0.join("audio.wav");
    let (_owner, cancel) = watch::channel(false);
    let generated = supervisor::run_capture(
        &CommandSpec {
            executable: "ffmpeg".into(),
            args: vec![
                "-v".into(),
                "error".into(),
                "-i".into(),
                av.as_os_str().to_owned(),
                "-map".into(),
                "0:a:0".into(),
                "-c".into(),
                "copy".into(),
                audio.as_os_str().to_owned(),
            ],
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let original_audio = std::fs::read(&audio).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    for (name, input, mode, indices, audio_index, start_ms, end_ms, expected_duration) in [
        (
            "audio-only-full",
            &audio,
            VideoMode::Drop,
            vec![0],
            0,
            None,
            None,
            2.0,
        ),
        (
            "audio-only-trim",
            &audio,
            VideoMode::Drop,
            vec![0],
            0,
            Some(250),
            Some(1000),
            0.75,
        ),
        (
            "copy-video-trim",
            &av,
            VideoMode::Copy,
            vec![0, 1],
            1,
            Some(750),
            Some(1500),
            1.0,
        ),
    ] {
        let output = fixture.0.join(format!("{name}.mkv"));
        let settings = EncodeSettings {
            video_mode: mode,
            audio: vec![AudioTrackSettings {
                normalization: None,
                stream_index: audio_index,
                codec: AudioCodec::Flac,
                bitrate_kbps: 128,
                channels: Default::default(),
                gain: None,
            }],
            trim: start_ms
                .zip(end_ms)
                .map(
                    |(start_milliseconds, end_milliseconds)| media_core::VideoTrim {
                        time: Some(media_core::VideoTimeTrim {
                            seek_strategy: Default::default(),
                            start_milliseconds,
                            end_milliseconds,
                        }),
                        start_frame: 0,
                        end_frame_exclusive: 0,
                    },
                ),
            ..Default::default()
        };
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: indices,
                },
                settings,
            })
            .await
            .unwrap();
        let job = finish(&manager, &job.id).await;
        assert_eq!(job.state, JobState::Succeeded, "{name}: {job:#?}");
        let media = media_runtime::probe_media(output.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert!(
            (media.duration_seconds.unwrap() - expected_duration).abs() < 0.05,
            "{name}: {media:#?}"
        );
    }
    assert_eq!(std::fs::read(audio).unwrap(), original_audio);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe on PATH"]
async fn copy_video_conversion_preserves_chapters_metadata_and_requested_order() {
    let fixture = Fixture::new();
    let original = fixture.0.join("original.mkv");
    generate(&original).await;
    let chapter = fixture.0.join("chapters.ffmeta");
    std::fs::write(&chapter,";FFMETADATA1\ntitle=Container title\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1500\ntitle=Opening\n").unwrap();
    let input = fixture.0.join("chaptered.mkv");
    let (_owner, cancel) = watch::channel(false);
    let generated = supervisor::run_capture(
        &CommandSpec {
            executable: "ffmpeg".into(),
            args: vec![
                "-v".into(),
                "error".into(),
                "-i".into(),
                original.as_os_str().to_owned(),
                "-i".into(),
                chapter.as_os_str().to_owned(),
                "-map".into(),
                "0".into(),
                "-map_metadata".into(),
                "1".into(),
                "-map_chapters".into(),
                "1".into(),
                "-c".into(),
                "copy".into(),
                input.as_os_str().to_owned(),
            ],
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let manager = JobManager::new(fixture.0.join("logs"));
    let output = fixture.0.join("converted.mkv");
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![1, 0],
            },
            settings: EncodeSettings {
                video_mode: VideoMode::Copy,
                audio: vec![AudioTrackSettings {
                    normalization: None,
                    stream_index: 1,
                    codec: AudioCodec::Flac,
                    bitrate_kbps: 128,
                    channels: Default::default(),
                    gain: None,
                }],
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
    let media = media_runtime::probe_media(output.to_string_lossy().into_owned())
        .await
        .unwrap();
    assert_eq!(
        media
            .streams
            .iter()
            .map(|s| s.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["audio", "video"]
    );
    assert_eq!(media.title.as_deref(), Some("Container title"));
    assert_eq!(media.streams[0].title.as_deref(), Some("Main audio"));
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and standalone x264 on PATH"]
async fn every_resize_intent_runs_through_verified_lossless_x264() {
    use media_core::{
        ResizeFill as Fill, ResizeMode as Mode, ResizeSettings, VideoEncoder, VideoFraming,
    };
    let fixture = Fixture::new();
    let input = fixture.0.join("tagged.mkv");
    let (_owner, cancel) = watch::channel(false);
    let mut args: Vec<std::ffi::OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=256x192:rate=24:duration=0.25",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-pix_fmt",
        "yuv420p",
        "-chroma_sample_location",
        "left",
        "-c:v",
        "ffv1",
        "-level",
        "3",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    args.push(input.as_os_str().to_owned());
    let generated = supervisor::run_capture(
        &CommandSpec {
            executable: "ffmpeg".into(),
            args,
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let manager = JobManager::new(fixture.0.join("logs"));
    let base = ResizeSettings {
        width: 192,
        height: 108,
        allow_upscale: true,
        ..Default::default()
    };
    for (i, resize, dimensions) in [
        (
            0,
            ResizeSettings {
                mode: Mode::Fit,
                ..base
            },
            (144, 108),
        ),
        (
            1,
            ResizeSettings {
                mode: Mode::Height,
                height: 128,
                ..base
            },
            (170, 128),
        ),
        (
            2,
            ResizeSettings {
                mode: Mode::Width,
                ..base
            },
            (192, 144),
        ),
        (
            3,
            ResizeSettings {
                mode: Mode::Percent,
                percent: 50,
                ..base
            },
            (128, 96),
        ),
        (
            4,
            ResizeSettings {
                mode: Mode::Exact,
                fill: Fill::Pad,
                ..base
            },
            (192, 108),
        ),
        (
            5,
            ResizeSettings {
                mode: Mode::Exact,
                fill: Fill::Crop,
                ..base
            },
            (192, 108),
        ),
        (
            6,
            ResizeSettings {
                mode: Mode::Exact,
                fill: Fill::Stretch,
                ..base
            },
            (192, 108),
        ),
    ] {
        let output = fixture.0.join(format!("resize-{i}.mkv"));
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    encoder: VideoEncoder::X264,
                    preset: 0,
                    lossless: true,
                    framing: VideoFraming {
                        resize: Some(resize),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let job = finish(&manager, &job.id).await;
        assert_eq!(job.state, JobState::Succeeded, "{resize:?}: {job:#?}");
        let media = media_runtime::probe_media(output.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(
            (media.streams[0].width, media.streams[0].height),
            (Some(dimensions.0), Some(dimensions.1))
        );
        assert!((media.duration_seconds.unwrap() - 0.25).abs() < 0.01);
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and standalone x264 on PATH"]
async fn accepts_untagged_sdr_anamorphic_small_and_high_rate_sources_with_verified_output() {
    use media_core::VideoEncoder;
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    for (name, size, rate, frames, tagged, sar) in [
        ("untagged", "128x96", "24", 6, false, "1/1"),
        ("small", "32x24", "24", 6, true, "1/1"),
        ("high-rate", "128x96", "240", 12, true, "1/1"),
        ("anamorphic", "128x96", "24", 6, true, "4/3"),
    ] {
        let input = fixture.0.join(format!("{name}.mkv"));
        let output = fixture.0.join(format!("{name}-encoded.mkv"));
        let mut args: Vec<std::ffi::OsString> =
            ["-v", "error", "-nostdin", "-n", "-f", "lavfi", "-i"]
                .into_iter()
                .map(Into::into)
                .collect();
        args.push(format!("testsrc2=size={size}:rate={rate}").into());
        args.extend(["-frames:v".into(),frames.to_string().into(),"-vf".into(),if tagged {format!("setsar={sar},setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709").into()} else {format!("setsar={sar}").into()},"-c:v".into(),"ffv1".into(),"-level".into(),"3".into()]);
        args.push(input.as_os_str().to_owned());
        let (_owner, cancel) = watch::channel(false);
        let generated = supervisor::run_capture(
            &CommandSpec {
                executable: "ffmpeg".into(),
                args,
                cwd: None,
            },
            cancel,
            1024 * 1024,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    encoder: VideoEncoder::X264,
                    preset: 0,
                    lossless: true,
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let job = finish(&manager, &job.id).await;
        assert_eq!(job.state, JobState::Succeeded, "{name}: {job:#?}");
        if !tagged {
            assert!(
                job.logs
                    .iter()
                    .any(|line| line.contains("Unspecified source SDR")),
                "{job:#?}"
            );
        }
        let media = media_runtime::probe_media(output.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(
            media.streams[0].sample_aspect_ratio.as_deref(),
            Some(if sar == "4/3" { "4:3" } else { "1:1" })
        );
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and standalone x264 on PATH"]
async fn canonical_sources_preserve_audio_while_normalizing_shift_rotation_and_variable_timestamps()
{
    use media_core::VideoEncoder;
    let fixture = Fixture::new();
    let base = fixture.0.join("base.mkv");
    generate(&base).await;
    let manager = JobManager::new(fixture.0.join("logs"));
    for name in ["shifted", "vfr", "rotated", "rotated-cropped"] {
        let input = fixture.0.join(format!(
            "{name}.{}",
            if name.starts_with("rotated") {
                "mp4"
            } else {
                "mkv"
            }
        ));
        let mut args: Vec<std::ffi::OsString> = ["-v", "error", "-nostdin", "-n"]
            .into_iter()
            .map(Into::into)
            .collect();
        if name == "shifted" {
            args.extend([
                "-copyts".into(),
                "-itsoffset".into(),
                "2.5".into(),
                "-i".into(),
                base.as_os_str().to_owned(),
                "-map".into(),
                "0".into(),
                "-c".into(),
                "copy".into(),
                "-avoid_negative_ts".into(),
                "disabled".into(),
            ]);
        } else if name == "vfr" {
            args.extend(["-i".into(),base.as_os_str().to_owned(),"-map".into(),"0".into(),"-vf".into(),"setpts=PTS+if(gte(N\\,12)\\,0.25/TB\\,0),setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709".into(),"-fps_mode".into(),"vfr".into(),"-c:v".into(),"ffv1".into(),"-c:a".into(),"copy".into()]);
        } else {
            let unrotated = fixture.0.join(format!("{name}-unrotated.mp4"));
            let (_owner, cancel) = watch::channel(false);
            let generated = supervisor::run_capture(
                &CommandSpec {
                    executable: "ffmpeg".into(),
                    args: vec![
                        "-v".into(),
                        "error".into(),
                        "-i".into(),
                        base.as_os_str().to_owned(),
                        "-map".into(),
                        "0".into(),
                        "-c:v".into(),
                        "copy".into(),
                        "-c:a".into(),
                        "alac".into(),
                        unrotated.as_os_str().to_owned(),
                    ],
                    cwd: None,
                },
                cancel,
                1024 * 1024,
                Duration::from_secs(20),
            )
            .await
            .unwrap();
            assert!(
                generated.status.success(),
                "{}",
                String::from_utf8_lossy(&generated.stderr)
            );
            args.extend([
                "-display_rotation:v:0".into(),
                "90".into(),
                "-i".into(),
                unrotated.as_os_str().to_owned(),
                "-map".into(),
                "0".into(),
                "-c".into(),
                "copy".into(),
            ]);
        }
        args.push(input.as_os_str().to_owned());
        let (_owner, cancel) = watch::channel(false);
        let generated = supervisor::run_capture(
            &CommandSpec {
                executable: "ffmpeg".into(),
                args,
                cwd: None,
            },
            cancel,
            1024 * 1024,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(
            generated.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let original = std::fs::read(&input).unwrap();
        let output = fixture.0.join(format!("{name}-encoded.mkv"));
        let framing = if name == "rotated-cropped" {
            media_core::VideoFraming {
                crop: media_core::CropSettings {
                    top: 8,
                    right: 16,
                    bottom: 24,
                    left: 32,
                },
                ..Default::default()
            }
        } else {
            Default::default()
        };
        if name == "rotated-cropped" {
            let request:media_core::BatchEncodeRequest=serde_json::from_value(serde_json::json!({"inputs":[{"inputPath":input,"streamIndices":[0,1],"videoStreamIndex":0,"framing":framing}],"outputDirectory":fixture.0,"encoder":"x264","backend":"standalone","crf":23,"preset":0})).unwrap();
            let preview = manager.preview_encode_batch(request).await.unwrap();
            assert!(preview.items[0].error.is_none(), "{preview:#?}");
            assert_eq!(
                preview.items[0].request.as_ref().unwrap().settings.framing,
                framing
            );
            assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".source")
            }));
        }
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0, 1],
                },
                settings: EncodeSettings {
                    encoder: VideoEncoder::X264,
                    preset: 0,
                    lossless: true,
                    framing,
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let job = finish(&manager, &job.id).await;
        assert_eq!(job.state, JobState::Succeeded, "{name}: {job:#?}");
        let media = media_runtime::probe_media(output.to_string_lossy().into_owned())
            .await
            .unwrap();
        let video = media.streams.iter().find(|s| s.kind == "video").unwrap();
        assert_eq!(
            (video.width, video.height),
            if name == "rotated-cropped" {
                (Some(64), Some(80))
            } else if name == "rotated" {
                (Some(96), Some(128))
            } else {
                (Some(128), Some(96))
            }
        );
        if name == "rotated-cropped" {
            let mut hashes = Vec::new();
            for (path, filter) in [
                (
                    &input,
                    "crop=80:64:32:8:exact=1,transpose=cclock,format=yuv420p",
                ),
                (&output, "format=yuv420p"),
            ] {
                let (_owner, cancel) = watch::channel(false);
                let result = supervisor::run_capture(
                    &CommandSpec {
                        executable: "ffmpeg".into(),
                        args: vec![
                            "-v".into(),
                            "error".into(),
                            "-noautorotate".into(),
                            "-i".into(),
                            path.as_os_str().to_owned(),
                            "-map".into(),
                            "0:v:0".into(),
                            "-vf".into(),
                            filter.into(),
                            "-f".into(),
                            "hash".into(),
                            "-hash".into(),
                            "sha256".into(),
                            "-".into(),
                        ],
                        cwd: None,
                    },
                    cancel,
                    1024 * 1024,
                    Duration::from_secs(20),
                )
                .await
                .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                hashes.push(result.stdout);
            }
            assert_eq!(
                hashes[0], hashes[1],
                "Coded crop followed by display rotation must match every encoded pixel."
            );
        }
        assert!(
            (media.duration_seconds.unwrap() - if name == "vfr" { 2.25 } else { 2.0 }).abs() < 0.05,
            "{name}: {media:#?}"
        );
        assert!(
            job.logs
                .iter()
                .any(|line| line.contains("Verified lossless canonical source")),
            "{job:#?}"
        );
        assert_eq!(std::fs::read(&input).unwrap(), original);
        assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".source")
        }));
    }
    manager.shutdown().await;
}
