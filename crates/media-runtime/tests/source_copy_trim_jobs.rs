//! Source Quick Copy uses the original mapped inputs and one seek/mux process.
use media_core::*;
use media_runtime::JobManager;
use serde_json::{Value, json};
use std::path::Path;
#[path = "common/source_trim_fixture.rs"]
mod fixture;
use fixture::*;
fn compare_packets(a: &Value, b: &Value) {
    assert_eq!(
        a["streams"].as_array().unwrap().len(),
        b["streams"].as_array().unwrap().len()
    );
    for (index, stream) in a["streams"].as_array().unwrap().iter().enumerate() {
        assert_eq!(stream["codec_type"], b["streams"][index]["codec_type"]);
        assert_eq!(stream["codec_name"], b["streams"][index]["codec_name"]);
        let a = packets(a, index as u64);
        let b = packets(b, index as u64);
        assert_eq!(a.len(), b.len());
        for (a, b) in a.into_iter().zip(b) {
            assert_eq!(a["data_hash"], b["data_hash"]);
            for key in ["pts_time", "dts_time", "duration_time"] {
                if a[key].is_string() && b[key].is_string() {
                    assert!(
                        (number(a, key) - number(b, key)).abs() < 0.0021,
                        "{key}: {a:?} {b:?}"
                    );
                } else {
                    assert_eq!(a[key], b[key]);
                }
            }
        }
    }
}
fn direct_reference(input: &Path, output: &Path, start: f64, end: f64, indices: &[u32]) {
    let mut a = args(&["-v", "error", "-ss"]);
    a.push(format!("{start:.3}").into());
    a.push("-i".into());
    a.push(input.into());
    for index in indices {
        a.extend(args(&["-map"]));
        a.push(format!("0:{index}").into());
    }
    a.extend(args(&["-c", "copy", "-t"]));
    a.push(format!("{:.3}", end - start).into());
    a.push(output.into());
    run("ffmpeg", a);
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg9"]
async fn copied_seek_modes_match_source_original_command_packets_and_keep_original_request() {
    let f = Fixture::new();
    let input = fixture(&f);
    let before = std::fs::read(&input).unwrap();
    let manager = JobManager::new(f.0.join("logs"));
    let cases = [
        json!({"startFrame":24,"endFrameExclusive":72}),
        json!({"startFrame":0,"endFrameExclusive":0,"time":{"startMilliseconds":1000,"endMilliseconds":3000,"seekStrategy":"decodeFromStart"}}),
        json!({"startFrame":0,"endFrameExclusive":0,"time":{"startMilliseconds":1000,"endMilliseconds":3000,"seekStrategy":"inputSeek"}}),
    ];
    for (index, trim) in cases.into_iter().enumerate() {
        let output = f.0.join(format!("copy-{index}.mkv"));
        let mut request = request(&input, &output, serde_json::from_value(trim).unwrap());
        request.settings.video_mode = VideoMode::Copy;
        let frozen = request.clone();
        let job = manager.start_encode(request).await.unwrap();
        let finished = terminal(&manager, &job.id).await;
        assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
        assert_eq!(finished.request, frozen.source);
        assert_eq!(finished.encode_settings, Some(frozen.settings));
        let reference = f.0.join(format!("reference-{index}.mkv"));
        direct_reference(&input, &reference, 1., 3., &[2, 0, 1]);
        compare_packets(&probe(&reference), &probe(&output));
    }
    assert_eq!(std::fs::read(input).unwrap(), before);
    manager.shutdown().await;
}

fn multiple_sources(f: &Fixture) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    for (name, second) in [("primary.mkv", true), ("external.mkv", false)] {
        let mut a = args(&[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=8:duration=4",
        ]);
        if second {
            a.extend(args(&[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=80x64:rate=10:duration=4",
            ]));
        }
        a.extend(args(&[
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=48000:duration=4",
            "-map",
            "0:v",
        ]));
        if second {
            a.extend(args(&["-map", "1:v"]));
        }
        a.extend(args(&[
            "-map",
            if second { "2:a" } else { "1:a" },
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "16",
            "-threads",
            "1",
            "-c:a",
            "pcm_s16le",
            "-metadata",
            "title=Original input",
            "-metadata:s:v:0",
            "language=jpn",
        ]));
        a.push(f.0.join(name).into());
        run("ffmpeg", a);
    }
    let meta = f.0.join("donor.ffmeta");
    std::fs::write(&meta,";FFMETADATA1\ntitle=Donor title\nlanguage=eng\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=2000\ntitle=Opening\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=2000\nEND=4000\ntitle=Rest\n").unwrap();
    let donor = f.0.join("donor.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(f.0.join("external.mkv").into());
    a.extend(args(&["-f", "ffmetadata", "-i"]));
    a.push(meta.into());
    a.extend(args(&[
        "-map",
        "0",
        "-map_metadata",
        "1",
        "-map_chapters",
        "1",
        "-c",
        "copy",
    ]));
    a.push(donor.clone().into());
    run("ffmpeg", a);
    (f.0.join("primary.mkv"), f.0.join("external.mkv"), donor)
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg9"]
async fn copied_nonfirst_and_external_videos_keep_order_offsets_metadata_and_chapter_donors() {
    let f = Fixture::new();
    let (primary, external, donor) = multiple_sources(&f);
    let originals = [&primary, &external, &donor].map(|path| std::fs::read(path).unwrap());
    let output = f.0.join("mapped-copy.mkv");
    let mut req = request(
        &primary,
        &output,
        serde_json::from_value(json!({"startFrame":5,"endFrameExclusive":25})).unwrap(),
    );
    req.source.stream_indices = vec![1, 2];
    req.settings.video_mode = VideoMode::Copy;
    req.settings.video_stream_index = 1;
    req.settings.external_tracks=serde_json::from_value(json!([{"inputPath":external,"streamIndex":0,"offsetMilliseconds":250},{"inputPath":external,"streamIndex":1,"offsetMilliseconds":250}])).unwrap();
    req.settings.track_order = vec![
        EncodeTrackRef {
            input_path: None,
            stream_index: 1,
        },
        EncodeTrackRef {
            input_path: Some(external.to_string_lossy().into_owned()),
            stream_index: 0,
        },
        EncodeTrackRef {
            input_path: None,
            stream_index: 2,
        },
        EncodeTrackRef {
            input_path: Some(external.to_string_lossy().into_owned()),
            stream_index: 1,
        },
    ];
    req.settings.metadata_source_path = Some(donor.to_string_lossy().into_owned());
    req.settings.chapters_source_path = Some(donor.to_string_lossy().into_owned());
    req.settings.output_metadata = Some(OutputMetadata {
        title: Some("Copied title 字".into()),
        language: Some("jpn".into()),
    });
    let frozen = req.clone();
    let manager = JobManager::new(f.0.join("logs"));
    let job = manager.start_encode(req).await.unwrap();
    let finished = terminal(&manager, &job.id).await;
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    assert_eq!(finished.encode_settings, Some(frozen.settings));
    let reference = f.0.join("independent.mkv");
    let mut a = args(&["-v", "error", "-ss", "0.500", "-i"]);
    a.push(primary.as_os_str().into());
    a.extend(args(&["-itsoffset", "0.250", "-ss", "0.500", "-i"]));
    a.push(external.as_os_str().into());
    a.extend(args(&["-ss", "0.500", "-i"]));
    a.push(donor.as_os_str().into());
    a.extend(args(&[
        "-map",
        "0:1",
        "-map",
        "1:0",
        "-map",
        "0:2",
        "-map",
        "1:1",
        "-c",
        "copy",
        "-map_metadata",
        "2",
        "-map_chapters",
        "2",
        "-metadata",
        "title=Copied title 字",
        "-metadata",
        "language=jpn",
        "-t",
        "2.000",
    ]));
    a.push(reference.as_os_str().into());
    run("ffmpeg", a);
    let actual = probe(&output);
    let expected = probe(&reference);
    compare_packets(&expected, &actual);
    assert_eq!(actual["format"]["tags"]["title"], "Copied title 字");
    assert_eq!(actual["chapters"], expected["chapters"]);
    assert_eq!(actual["streams"][0]["width"], 80);
    assert_eq!(actual["streams"][1]["width"], 64);
    for (path, original) in [&primary, &external, &donor].into_iter().zip(originals) {
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
    manager.shutdown().await;
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg9"]
async fn copied_video_with_converted_normalized_audio_keeps_source_seek_samples_and_clock() {
    let f = Fixture::new();
    let (primary, _, _) = multiple_sources(&f);
    let output = f.0.join("normalized.mkv");
    let mut req = request(
        &primary,
        &output,
        serde_json::from_value(json!({"startFrame":5,"endFrameExclusive":25})).unwrap(),
    );
    req.source.stream_indices = vec![1, 2];
    req.settings.video_mode = VideoMode::Copy;
    req.settings.video_stream_index = 1;
    req.settings.audio=serde_json::from_value(json!([{"streamIndex":2,"codec":"flac","channels":"preserve","bitrateKbps":128,"normalization":{"target":"lufs16"}}])).unwrap();
    let manager = JobManager::new(f.0.join("logs"));
    let job = manager.start_encode(req).await.unwrap();
    let finished = terminal(&manager, &job.id).await;
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    assert!(
        finished
            .logs
            .iter()
            .any(|line| line.contains("LUFS") || line.contains("normalization"))
    );
    let actual = probe(&output);
    assert_eq!(actual["streams"][0]["codec_name"], "h264");
    assert_eq!(actual["streams"][1]["codec_name"], "flac");
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg9"]
async fn copied_video_trim_direct_subtitle_conversion_matches_source_cue_packets() {
    let f = Fixture::new();
    let original = fixture(&f);
    let webm_source = f.0.join("vp9-opus.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(original.as_os_str().into());
    a.extend(args(&[
        "-map",
        "0",
        "-c",
        "copy",
        "-c:v",
        "libvpx-vp9",
        "-cpu-used",
        "8",
        "-deadline",
        "realtime",
        "-b:v",
        "0",
        "-crf",
        "45",
        "-c:a",
        "libopus",
    ]));
    a.push(webm_source.as_os_str().into());
    run("ffmpeg", a);
    let manager = JobManager::new(f.0.join("logs"));
    for (index, (input, mode, codec, extension)) in [
        (&original, SubtitleMode::SubRip, "subrip", "mkv"),
        (&original, SubtitleMode::MovText, "mov_text", "mov"),
        (&webm_source, SubtitleMode::WebVtt, "webvtt", "webm"),
    ]
    .into_iter()
    .enumerate()
    {
        let output = f.0.join(format!("subtitles-{index}.{extension}"));
        let mut req = request(
            input,
            &output,
            serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap(),
        );
        req.settings.video_mode = VideoMode::Copy;
        req.settings.subtitles = vec![SubtitleTrackSettings {
            stream_index: 2,
            mode,
            burn_in: false,
        }];
        let job = manager.start_encode(req).await.unwrap();
        let finished = terminal(&manager, &job.id).await;
        assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
        let reference = f.0.join(format!("subtitle-reference-{index}.{extension}"));
        let mut a = args(&["-v", "error", "-ss", "1.000", "-i"]);
        a.push(input.as_os_str().into());
        a.extend(args(&[
            "-map", "0:2", "-map", "0:0", "-map", "0:1", "-c", "copy", "-c:0", codec, "-t", "2.000",
        ]));
        a.push(reference.as_os_str().into());
        run("ffmpeg", a);
        compare_packets(&probe(&reference), &probe(&output));
    }
    manager.shutdown().await;
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg9"]
async fn copied_nonzero_origin_and_external_normalized_audio_follow_original_input_seek_clock() {
    let f = Fixture::new();
    let (primary, external, _) = multiple_sources(&f);
    let shifted = f.0.join("shifted.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(primary.as_os_str().into());
    a.extend(args(&["-map", "0", "-c", "copy", "-output_ts_offset", "2"]));
    a.push(shifted.as_os_str().into());
    run("ffmpeg", a);
    let manager = JobManager::new(f.0.join("logs"));
    let output = f.0.join("origin-copy.mkv");
    let mut req = request(
        &shifted,
        &output,
        serde_json::from_value(json!({"startFrame":5,"endFrameExclusive":25})).unwrap(),
    );
    req.source.stream_indices = vec![1, 2];
    req.settings.video_mode = VideoMode::Copy;
    req.settings.video_stream_index = 1;
    let job = manager.start_encode(req).await.unwrap();
    let finished = terminal(&manager, &job.id).await;
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    let reference = f.0.join("origin-reference.mkv");
    direct_reference(&shifted, &reference, 0.5, 2.5, &[1, 2]);
    compare_packets(&probe(&reference), &probe(&output));
    let output = f.0.join("external-normalized.mkv");
    let mut req = request(
        &primary,
        &output,
        serde_json::from_value(json!({"startFrame":5,"endFrameExclusive":25})).unwrap(),
    );
    req.source.stream_indices = vec![1, 2];
    req.settings.video_mode = VideoMode::Copy;
    req.settings.video_stream_index = 1;
    req.settings.external_tracks=serde_json::from_value(json!([{"inputPath":external,"streamIndex":1,"offsetMilliseconds":250,"audio":{"codec":"flac","channels":"preserve","bitrateKbps":128,"normalization":{"target":"lufs16"}}}])).unwrap();

    let job = manager.start_encode(req).await.unwrap();
    let finished = terminal(&manager, &job.id).await;
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    let actual = probe(&output);
    assert_eq!(actual["streams"][2]["codec_name"], "flac");
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg9"]
async fn held_copy_seek_cancel_collision_mutation_and_b_frame_input_keep_owned_publication_guards()
{
    let f = Fixture::new();
    let (primary, external, donor) = multiple_sources(&f);
    let bframes = f.0.join("reordered-source.mp4");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(primary.as_os_str().into());
    a.extend(args(&[
        "-map", "0:1", "-c:v", "libx264", "-bf", "3", "-g", "16", "-threads", "1",
    ]));
    a.push(bframes.as_os_str().into());
    run("ffmpeg", a);
    let manager = JobManager::new(f.0.join("logs"));
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    for (index, action) in ["unchanged", "cancel", "collision", "mutation"]
        .into_iter()
        .enumerate()
    {
        let output = f.0.join(format!("held-{index}.mkv"));
        let mut req = request(
            &bframes,
            &output,
            serde_json::from_value(json!({"startFrame":8,"endFrameExclusive":24})).unwrap(),
        );
        req.source.stream_indices = vec![0];
        req.settings.video_mode = VideoMode::Copy;
        req.settings.video_stream_index = 0;
        if action == "mutation" {
            req.settings.metadata_source_path = Some(donor.to_string_lossy().into_owned());
            req.settings.external_tracks =
                serde_json::from_value(json!([{"inputPath":external,"streamIndex":0}])).unwrap();
        }
        let review = manager
            .prepare_command_edit(
                CommandEditRequest::Encode {
                    request: Box::new(req),
                },
                cancel.clone(),
            )
            .await
            .unwrap();
        let id = review.job.as_ref().unwrap().id.clone();
        assert!(!output.exists());
        assert!(
            review.arguments.contains("-ss")
                && review.arguments.contains("0.800")
                && review.arguments.contains("-t")
                && review.arguments.contains("1.600")
        );
        if action == "cancel" {
            manager.cancel_command_edit(&review.id).await.unwrap();
            let job = terminal(&manager, &id).await;
            assert_eq!(job.state, JobState::Canceled);
            assert!(!output.exists());
        } else {
            if action == "collision" {
                std::fs::write(&output, b"later owner").unwrap();
            }
            let mutated = if action == "mutation" {
                let mut bytes = std::fs::read(&donor).unwrap();
                bytes.extend_from_slice(b"later owner mutation");
                std::fs::write(&donor, &bytes).unwrap();
                Some(bytes)
            } else {
                None
            };
            manager
                .confirm_command_edit(
                    CommandEditConfirmation {
                        id: review.id,
                        arguments: review.arguments,
                    },
                    cancel.clone(),
                )
                .await
                .unwrap();
            let job = terminal(&manager, &id).await;
            match action {
                "unchanged" => {
                    assert_eq!(job.state, JobState::Succeeded, "{job:?}");
                    let reference = f.0.join("b-frame-reference.mkv");
                    direct_reference(&bframes, &reference, 0.8, 2.4, &[0]);
                    compare_packets(&probe(&reference), &probe(&output));
                }
                "collision" => {
                    assert_eq!(job.state, JobState::Failed);
                    assert_eq!(job.error.unwrap().code, "OUTPUT_EXISTS");
                    assert_eq!(std::fs::read(&output).unwrap(), b"later owner");
                }
                "mutation" => {
                    assert_eq!(job.state, JobState::Failed, "{job:?}");
                    assert!(
                        job.error
                            .unwrap()
                            .message
                            .to_lowercase()
                            .contains("changed")
                    );
                    assert!(!output.exists());
                    assert_eq!(std::fs::read(&donor).unwrap(), mutated.unwrap());
                }
                _ => unreachable!(),
            }
        }
        assert!(
            !std::fs::read_dir(&f.0).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".jesses-")),
            "owned artifacts left after {action}"
        );
    }
    manager.shutdown().await;
}
