//! Real source-equivalent copied audio/subtitle seek and final common clock.
use media_core::*;
use media_runtime::JobManager;
use serde_json::{Value, json};
#[path = "common/source_trim_fixture.rs"]
mod fixture;
use fixture::*;
#[tokio::test]
#[ignore = "requires qualified FFmpeg9 libx264, libvpx-vp9 and AAC"]
async fn source_copy_trim_preserves_ffmpeg_preroll_ass_packets_and_common_video_clock() {
    for mode in ["frames", "inputSeek", "decodeFromStart"] {
        let f = Fixture::new();
        let input = fixture(&f);
        let out = f.0.join("target.mkv");
        let trim = if mode == "frames" {
            serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap()
        } else {
            serde_json::from_value(json!({"time":{"startMilliseconds":979,"endMilliseconds":2999,"seekStrategy":mode}})).unwrap()
        };
        let manager = JobManager::new(f.0.join("logs"));
        let job = manager
            .start_encode(request(&input, &out, trim))
            .await
            .unwrap();
        let result = terminal(&manager, &job.id).await;
        manager.shutdown().await;
        assert_eq!(result.state, JobState::Succeeded, "{mode}: {result:#?}");
        let actual = probe(&out);
        let video = packets(&actual, 1);
        let origin = number(video[0], "pts_time");
        assert!((origin - 0.979).abs() < 0.003, "{mode}: {origin}");
        assert_eq!(video.len(), 48);
        // The source final mux uses input-side seek for both time strategies, and
        // the half-frame/truncated-ms window for frame mode. Supply an independently
        // produced zero-origin video to the exact source-equivalent final command.
        let encoded = f.0.join("encoded.mkv");
        let mut a = args(&["-v", "error", "-i"]);
        a.push(input.clone().into());
        a.extend(args(&[
            "-map",
            "0:v",
            "-vf",
            "trim=start_frame=24:end_frame=72,setpts=PTS-STARTPTS",
            "-c:v",
            "libvpx-vp9",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
        ]));
        a.push(encoded.clone().into());
        run("ffmpeg", a);
        let reference = f.0.join("source-equivalent.mkv");
        let mut a = args(&["-v", "error", "-ss", "0.979", "-i"]);
        a.push(input.clone().into());
        a.push("-i".into());
        a.push(encoded.into());
        a.extend(args(&[
            "-map",
            "0:s",
            "-map",
            "1:v",
            "-map",
            "0:a",
            "-c",
            "copy",
            "-t",
            "2.020",
            "-max_interleave_delta",
            "0",
        ]));
        a.push(reference.clone().into());
        run("ffmpeg", a);
        let expected = probe(&reference);
        for index in [0, 2] {
            let a = packets(&actual, index);
            let b = packets(&expected, index);
            assert_eq!(a.len(), b.len(), "{mode} track{index}");
            for (a, b) in a.iter().zip(b) {
                assert_eq!(a["data_hash"], b["data_hash"]);
                for key in ["pts_time", "dts_time", "duration_time"] {
                    assert!(
                        (number(a, key) - number(b, key)).abs() < 0.0021,
                        "{mode} {key}: {a:?} {b:?}"
                    );
                }
            }
        }
        assert!((number(packets(&expected, 1)[0], "pts_time") - origin).abs() < 0.002);
        assert!(std::fs::read_dir(&f.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".partial.")
        }));
    }
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg9 libx264, libvpx-vp9, AAC and FLAC"]
async fn source_copy_trim_mixed_conversion_external_offsets_and_nonzero_origins() {
    for offset in [-250, 250] {
        let f = Fixture::new();
        let input = fixture(&f);
        let external = f.0.join("external.mkv");
        std::fs::copy(&input, &external).unwrap();
        let converted = f.0.join("converted.mkv");
        std::fs::copy(&input, &converted).unwrap();
        let out = f.0.join("mixed.mkv");
        let mut req = request(
            &input,
            &out,
            serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap(),
        );
        req.settings.external_tracks=serde_json::from_value(json!([
   {"inputPath":external,"streamIndex":1,"offsetMilliseconds":offset},
   {"inputPath":converted,"streamIndex":1,"offsetMilliseconds":100,"audio":{"codec":"flac","bitrateKbps":128,"channels":"preserve"}}
  ])).unwrap();
        let manager = JobManager::new(f.0.join("logs"));
        let job = manager.start_encode(req).await.unwrap();
        let result = terminal(&manager, &job.id).await;
        manager.shutdown().await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "offset{offset}: {result:#?}"
        );
        let actual = probe(&out);
        let origin = number(packets(&actual, 1)[0], "pts_time");
        assert!((origin - (if offset < 0 { 1.208 } else { 0.979 })).abs() < 0.003);
        assert_eq!(actual["streams"][4]["codec_name"], "flac");
        assert!(
            result
                .logs
                .iter()
                .any(|line| line.contains("Source audio stream") && line.contains("sample")),
            "decoded converted samples must be verified: {:?}",
            result.logs
        );
    }
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg9 libx264/libvpx-vp9 and Unix shell"]
async fn source_copy_trim_held_commands_and_batch_freeze_source_semantics() {
    let f = Fixture::new();
    let input = fixture(&f);
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    manager.ready().await.unwrap();
    let trim = serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap();
    for edited in [false, true] {
        let req = request(
            &input,
            &f.0.join(if edited {
                "edited.mkv"
            } else {
                "unchanged.mkv"
            }),
            trim,
        );
        let (_owner, cancel) = tokio::sync::watch::channel(false);
        let review = manager
            .prepare_command_edit(
                CommandEditRequest::Encode {
                    request: Box::new(req),
                },
                cancel.clone(),
            )
            .await
            .unwrap();
        assert!(review.arguments.contains("0.979"));
        assert!(review.arguments.contains("2.020"));
        assert!(
            review
                .job
                .as_ref()
                .unwrap()
                .encode_settings
                .as_ref()
                .unwrap()
                .source_trim
        );
        let arguments = if edited {
            format!("{} && printf source-trim-edited", review.arguments)
        } else {
            review.arguments
        };
        let CommandEditResult::Encode { job } = manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments,
                },
                cancel,
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let result = terminal(&manager, &job.id).await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "edited{edited}: {result:#?}"
        );
        assert_eq!(result.edited_command.is_some(), edited);
    }
    let batch:BatchEncodeRequest=serde_json::from_value(json!({"inputs":[{"inputPath":input,"streamIndices":[2,0,1],"videoStreamIndex":0,"trim":{"startFrame":24,"endFrameExclusive":72}}],"sourceTrim":true,"outputDirectory":f.0,"outputContainer":"matroska","backend":"standalone","encoder":"vp9","workers":2,"crf":30,"preset":4})).unwrap();
    let preview = manager.preview_encode_batch(batch).await.unwrap();
    let frozen = preview.items[0].request.clone().unwrap();
    assert!(frozen.settings.source_trim);
    assert_eq!(frozen.settings.trim, Some(trim));
    let job = manager.start_encode(frozen).await.unwrap();
    let result = terminal(&manager, &job.id).await;
    assert_eq!(result.state, JobState::Succeeded, "{result:#?}");
    manager.shutdown().await;
    let history = std::fs::read_to_string(f.0.join("history/jobs.json")).unwrap();
    let saved: Value = serde_json::from_str(&history).unwrap();
    assert!(
        saved["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|job| job["encodeSettings"]["sourceTrim"] == true)
    );
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg9 libx264/libvpx-vp9 and AAC"]
async fn source_copy_trim_nonzero_input_origins_keep_source_packet_clock() {
    for origin in [3.0, -0.5] {
        let f = Fixture::new();
        let base = fixture(&f);
        let input = f.0.join(if origin < 0.0 {
            "shifted-source.ts"
        } else {
            "shifted-source.mkv"
        });
        let mut a = args(&["-v", "error", "-copyts", "-itsoffset"]);
        a.push(origin.to_string().into());
        a.push("-i".into());
        a.push(base.into());
        if origin < 0.0 {
            a.extend(args(&[
                "-map",
                "0:v",
                "-map",
                "0:a",
                "-c:v",
                "copy",
                "-c:a",
                "mp2",
                "-b:a",
                "128k",
                "-avoid_negative_ts",
                "disabled",
                "-mpegts_copyts",
                "1",
                "-muxdelay",
                "0",
                "-muxpreload",
                "0",
            ]));
        } else {
            a.extend(args(&[
                "-map",
                "0",
                "-c",
                "copy",
                "-avoid_negative_ts",
                "disabled",
            ]));
        }
        a.push(input.clone().into());
        run("ffmpeg", a);
        let out = f.0.join("trimmed.mkv");
        let trim = serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap();
        let manager = JobManager::new(f.0.join("logs"));
        let mut req = request(&input, &out, trim);
        if origin < 0.0 {
            req.source.stream_indices = vec![0, 1];
        }
        let job = manager.start_encode(req).await.unwrap();
        let result = terminal(&manager, &job.id).await;
        manager.shutdown().await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "origin{origin}: {result:#?}"
        );
        let actual = probe(&out);
        assert_eq!(packets(&actual, if origin < 0.0 { 0 } else { 1 }).len(), 48);
        let audio = packets(&actual, if origin < 0.0 { 1 } else { 2 });
        assert!(!audio.is_empty());
        assert!(
            result.logs.iter().any(|line| line.contains("canonical")),
            "{origin}: {:?}",
            result.logs
        );
    }
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg9 AAC ADTS-to-ASC and libvpx-vp9"]
async fn source_copy_trim_aac_transport_origins_preserve_access_units_samples_and_sync() {
    for origin in [-0.5, 3.0] {
        let f = Fixture::new();
        let base = fixture(&f);
        let input = f.0.join("transport.ts");
        let mut a = args(&["-v", "error", "-copyts", "-itsoffset"]);
        a.push(origin.to_string().into());
        a.push("-i".into());
        a.push(base.into());
        a.extend(args(&[
            "-map",
            "0:v",
            "-map",
            "0:a",
            "-c",
            "copy",
            "-avoid_negative_ts",
            "disabled",
            "-mpegts_copyts",
            "1",
            "-muxdelay",
            "0",
            "-muxpreload",
            "0",
        ]));
        a.push(input.clone().into());
        run("ffmpeg", a);
        let out = f.0.join("trimmed.mkv");
        let mut req = request(
            &input,
            &out,
            serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap(),
        );
        req.source.stream_indices = vec![0, 1];
        let manager = JobManager::new(f.0.join("logs"));
        let job = manager.start_encode(req).await.unwrap();
        let result = terminal(&manager, &job.id).await;
        manager.shutdown().await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "AAC origin{origin}: {result:#?}"
        );
        assert!(
            result
                .logs
                .iter()
                .any(|line| line.contains("Verified ADTS-to-ASC AAC")
                    && line.contains("decoded PCM bytes")
                    && line.contains("exact access-unit hashes")),
            "{:?}",
            result.logs
        );
        let actual = probe(&out);
        assert_eq!(packets(&actual, 0).len(), 48);
        assert!(packets(&actual, 1).len() > 90);
    }
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg9 libx264, libvpx-vp9 and authored copied PGS"]
async fn source_direct_trim_copies_complete_pgs_display_sets_with_source_seek_clock() {
    let f = Fixture::new();
    let base = fixture(&f);
    let pgs = pgs_fixture(&f.0);
    let input = f.0.join("pgs-source.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(base.into());
    a.push("-i".into());
    a.push(pgs.into());
    a.extend(args(&[
        "-map", "0:v", "-map", "0:a", "-map", "1:s", "-c", "copy",
    ]));
    a.push(input.clone().into());
    run("ffmpeg", a);
    let output = f.0.join("pgs-final.mkv");
    let trim = serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap();
    let manager = JobManager::new(f.0.join("logs"));
    let job = manager
        .start_encode(request(&input, &output, trim))
        .await
        .unwrap();
    let done = terminal(&manager, &job.id).await;
    manager.shutdown().await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    let actual = probe(&output);
    let picture = f.0.join("picture.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(input.clone().into());
    a.extend(args(&[
        "-map",
        "0:v",
        "-vf",
        "trim=start_frame=24:end_frame=72,setpts=PTS-STARTPTS",
        "-c:v",
        "libvpx-vp9",
        "-deadline",
        "realtime",
        "-cpu-used",
        "8",
    ]));
    a.push(picture.clone().into());
    run("ffmpeg", a);
    let reference = f.0.join("pgs-source-equivalent.mkv");
    let mut a = args(&["-v", "error", "-ss", "0.979", "-i"]);
    a.push(input.into());
    a.push("-i".into());
    a.push(picture.into());
    a.extend(args(&[
        "-map",
        "0:s",
        "-map",
        "1:v",
        "-map",
        "0:a",
        "-c",
        "copy",
        "-t",
        "2.020",
        "-max_interleave_delta",
        "0",
    ]));
    a.push(reference.clone().into());
    run("ffmpeg", a);
    let expected = probe(&reference);
    for index in [0, 2] {
        let copied = packets(&actual, index);
        let expected = packets(&expected, index);
        assert!(!copied.is_empty());
        assert_eq!(copied.len(), expected.len());
        for (a, b) in copied.iter().zip(expected) {
            assert_eq!(a["data_hash"], b["data_hash"]);
            for key in ["pts_time", "dts_time", "duration_time"] {
                if a[key].is_string() && b[key].is_string() {
                    assert!((number(a, key) - number(b, key)).abs() < 0.0021);
                } else {
                    assert_eq!(a[key], b[key]);
                }
            }
        }
    }
    assert_eq!(packets(&actual, 1).len(), 48);
}

/// A trim only needs its own frames decoded and verified: the scan beside the
/// encoder stops after the trim end plus look-ahead, and still covers the whole
/// source when that margin reaches the end.
#[tokio::test]
#[ignore = "requires qualified FFmpeg9 libx264 and libvpx-vp9"]
async fn source_trim_verifies_decoded_frames_through_the_trim_end() {
    for (end, expected) in [
        (12, "Verified the first 76 decoded source frames"),
        (72, "Verified all 96 decoded source frames"),
    ] {
        let f = Fixture::new();
        let input = fixture(&f);
        let out = f.0.join("target.mkv");
        let trim = serde_json::from_value(json!({"startFrame":0,"endFrameExclusive":end})).unwrap();
        let mut req = request(&input, &out, trim);
        req.source.stream_indices = vec![0];
        let manager = JobManager::new(f.0.join("logs"));
        let job = manager.start_encode(req).await.unwrap();
        let result = terminal(&manager, &job.id).await;
        manager.shutdown().await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "{end}: {:?}",
            result.error
        );
        assert!(
            result.logs.iter().any(|line| line.starts_with(expected)),
            "{end}: {:#?}",
            result.logs
        );
        assert_eq!(packets(&probe(&out), 0).len(), end as usize);
    }
}

/// Converted audio in a trim is decoded only through the trim end plus margin,
/// and the clipped timeline still passes the complete output verification.
#[tokio::test]
#[ignore = "requires qualified FFmpeg9 libx264, libvpx-vp9 and libopus"]
async fn source_trim_decodes_converted_audio_only_through_the_trim_end() {
    let f = Fixture::new();
    let input = f.0.join("long.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=24:d=30",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=30",
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-g",
        "48",
        "-c:a",
        "aac",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(input.clone().into());
    run("ffmpeg", a);
    let out = f.0.join("target.mkv");
    let trim = serde_json::from_value(json!({"startFrame":0,"endFrameExclusive":48})).unwrap();
    let mut req = request(&input, &out, trim);
    req.source.stream_indices = vec![0, 1];
    req.settings.audio = vec![AudioTrackSettings {
        normalization: None,
        stream_index: 1,
        codec: AudioCodec::Opus,
        bitrate_kbps: 96,
        channels: AudioChannels::Preserve,
        gain: None,
    }];
    let manager = JobManager::new(f.0.join("logs"));
    let job = manager.start_encode(req).await.unwrap();
    let result = terminal(&manager, &job.id).await;
    manager.shutdown().await;
    assert_eq!(result.state, JobState::Succeeded, "{:?}", result.error);
    assert!(
        result
            .logs
            .iter()
            .any(|line| line.starts_with("Decoding selected audio through the trim end")),
        "{:#?}",
        result.logs
    );
    assert!(
        result
            .logs
            .iter()
            .any(|line| line
                .starts_with("Source audio stream 1: Validated audio: 96000 source samples")),
        "{:#?}",
        result.logs
    );
    let actual = probe(&out);
    assert_eq!(actual["streams"][1]["codec_name"], "opus");
}
