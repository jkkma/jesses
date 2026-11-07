//! Source subtitle conversion and intact animation rendering are separate consumers.
use media_core::*;
use media_runtime::JobManager;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
#[path = "common/source_trim_fixture.rs"]
mod fixture;
use fixture::*;
fn source(f: &Fixture) -> PathBuf {
    fixture(f);
    let ass = f.0.join("captions.ass");
    let mut events = std::fs::read_to_string(&ass).unwrap();
    events.push_str("Dialogue: 1,0:00:00.25,0:00:03.50,Default,,0,0,0,Scroll up;0;60;25,{\\k40}Ka{\\k40}ra{\\k40}oke\n");
    std::fs::write(ass, events).unwrap();
    let input = f.0.join("zero-origin.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=24:d=4",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=4",
        "-i",
    ]);
    a.push(f.0.join("captions.ass").into());
    a.extend(args(&[
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map",
        "2:s",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-g",
        "48",
        "-c:a",
        "pcm_s16le",
        "-c:s",
        "copy",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]));
    a.push(input.clone().into());
    run("ffmpeg", a);
    input
}
fn pixels(path: &Path) -> Vec<u8> {
    let mut a = args(&["-v", "error", "-i"]);
    a.push(path.into());
    a.extend(args(&[
        "-map",
        "0:v:0",
        "-pix_fmt",
        "yuv420p",
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-",
    ]));
    run("ffmpeg", a)
}
fn assert_packets(actual: &Value, reference: &Value, index: u64) {
    let (a, b) = (packets(actual, index), packets(reference, index));
    assert_eq!(a.len(), b.len());
    for (a, b) in a.iter().zip(b) {
        assert_eq!(a["data_hash"], b["data_hash"]);
        for key in ["pts_time", "dts_time", "duration_time"] {
            if a[key].is_string() && b[key].is_string() {
                assert!(
                    (number(a, key) - number(b, key)).abs() < 0.0021,
                    "{key} {a:?} {b:?}"
                );
            } else {
                assert_eq!(a[key], b[key]);
            }
        }
    }
}

/// External subtitles are a separate source reader, preserving intact events.
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9 libx264/libvpx-vp9/libass"]
async fn source_trim_external_ass_burn_retains_original_animation_clock() {
    for offset in [0] {
        let f = Fixture::new();
        let input = source(&f);
        let external = f.0.join("captions.ass");
        let output = f.0.join("external-burn.mkv");
        let trim = serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap();
        let mut req = request(&input, &output, trim);
        req.source.stream_indices = vec![0, 1];
        req.settings.lossless = true;
        req.settings.external_tracks = serde_json::from_value(json!([{
            "inputPath":external,
            "streamIndex":0,
            "offsetMilliseconds":offset,
            "subtitleMode":"burnIn"
        }]))
        .unwrap();
        let manager = JobManager::new(f.0.join("logs"));
        let job = manager.start_encode(req).await.unwrap();
        let done = terminal(&manager, &job.id).await;
        manager.shutdown().await;
        assert_eq!(
            done.state,
            JobState::Succeeded,
            "offset {offset}: {done:#?}"
        );

        let reference = f.0.join("external-reference.mkv");
        let shift = f64::from(offset) / 1000.0;
        let mut a = args(&["-v", "error", "-i"]);
        a.push(input.into());
        a.extend(args(&["-map", "0:v", "-vf"]));
        a.push(format!(
            "setpts=PTS-({shift:.3})/TB,subtitles=filename='{}',setpts=PTS+({shift:.3})/TB,trim=start_frame=24:end_frame=72,setpts=PTS-STARTPTS",
            external.display()
        ).into());
        a.extend(args(&["-c:v", "ffv1"]));
        a.push(reference.clone().into());
        run("ffmpeg", a);
        let actual_pixels = pixels(&output);
        let reference_pixels = pixels(&reference);
        assert!(
            actual_pixels == reference_pixels,
            "external intact ASS animation offset {offset}; lengths {}/{}; first difference {:?}",
            actual_pixels.len(),
            reference_pixels.len(),
            actual_pixels
                .iter()
                .zip(&reference_pixels)
                .position(|(a, b)| a != b)
        );
        assert_eq!(actual_pixels.len(), 48 * 96 * 64 * 3 / 2);
    }
}
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9 libx264/libvpx-vp9/libass"]
async fn source_trim_converts_and_burns_intact_animated_ass_as_independent_consumers() {
    for mode in ["frames", "inputSeek", "decodeFromStart"] {
        for (convert, burn, copy_burn) in [
            (true, false, false),
            (false, true, false),
            (true, true, false),
            (false, true, true),
        ] {
            let f = Fixture::new();
            let input = source(&f);
            let out = f.0.join("actual.mkv");
            let trim=serde_json::from_value(if mode=="frames"{json!({"startFrame":24,"endFrameExclusive":72})}else{json!({"time":{"startMilliseconds":1000,"endMilliseconds":3000,"seekStrategy":mode}})}).unwrap();
            let mut req = request(&input, &out, trim);
            req.settings.lossless = true;
            req.settings.subtitles = vec![SubtitleTrackSettings {
                stream_index: 2,
                mode: if convert {
                    SubtitleMode::SubRip
                } else if copy_burn {
                    SubtitleMode::Copy
                } else {
                    SubtitleMode::BurnIn
                },
                burn_in: burn && (convert || copy_burn),
            }];
            let manager = JobManager::new(f.0.join("logs"));
            let job = manager.start_encode(req).await.unwrap();
            let done = terminal(&manager, &job.id).await;
            manager.shutdown().await;
            assert_eq!(
                done.state,
                JobState::Succeeded,
                "{mode} convert={convert} burn={burn}: {done:#?}"
            );
            let picture = f.0.join("picture.mkv");
            let mut a = args(&["-v", "error", "-i"]);
            a.push(input.clone().into());
            a.extend(args(&["-map", "0:v", "-vf"]));
            let mut graph = if burn {
                format!("subtitles=filename='{}':si=0,", input.display())
            } else {
                String::new()
            };
            graph.push_str("trim=start_frame=24:end_frame=72,setpts=PTS-STARTPTS");
            a.push(graph.into());
            a.extend(args(&["-c:v", "ffv1"]));
            a.push(picture.clone().into());
            run("ffmpeg", a);
            let (actual_pixels, reference_pixels) = (pixels(&out), pixels(&picture));
            assert!(
                actual_pixels == reference_pixels,
                "intact source animation phase {mode} convert={convert} burn={burn}; lengths {}/{}; first difference {:?}",
                actual_pixels.len(),
                reference_pixels.len(),
                actual_pixels
                    .iter()
                    .zip(&reference_pixels)
                    .position(|(a, b)| a != b)
            );
            if convert {
                let reference = f.0.join("reference.mkv");
                let mut a = args(&[
                    "-v",
                    "error",
                    "-ss",
                    if mode == "frames" { "0.979" } else { "1.000" },
                    "-i",
                ]);
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
                    "-c:s",
                    "subrip",
                    "-t",
                    if mode == "frames" { "2.020" } else { "2.000" },
                    "-max_interleave_delta",
                    "0",
                ]));
                a.push(reference.clone().into());
                run("ffmpeg", a);
                let (actual, expected) = (probe(&out), probe(&reference));
                assert_packets(&actual, &expected, 0);
                assert_packets(&actual, &expected, 2);
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9 libx264/libvpx-vp9/libass"]
async fn source_trim_subtitle_formats_and_no_audio_keep_source_mux_clocks() {
    for (mode, extension, codec) in [
        (SubtitleMode::SubRip, "mkv", "subrip"),
        (SubtitleMode::WebVtt, "webm", "webvtt"),
        (SubtitleMode::MovText, "mp4", "mov_text"),
    ] {
        let f = Fixture::new();
        let input = source(&f);
        let out = f.0.join(format!("actual.{extension}"));
        let trim = serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap();
        let mut req = request(&input, &out, trim);
        req.source.stream_indices = vec![0, 2];
        req.settings.subtitles = vec![SubtitleTrackSettings {
            stream_index: 2,
            mode,
            burn_in: false,
        }];
        if mode == SubtitleMode::MovText {
            req.settings.encoder = VideoEncoder::X264;
            req.settings.encoding_preferences = Some(EncodingPreferences {
                mp4_faststart: true,
                keyframe_interval_seconds: 10,
                full_output_verification: true,
            });
        }
        let manager = JobManager::new(f.0.join("logs"));
        let job = manager.start_encode(req).await.unwrap();
        let done = terminal(&manager, &job.id).await;
        manager.shutdown().await;
        assert_eq!(done.state, JobState::Succeeded, "{codec}: {done:#?}");
        let actual = probe(&out);
        assert_eq!(actual["streams"][1]["codec_name"], codec);
        if mode == SubtitleMode::MovText {
            let bytes = std::fs::read(&out).unwrap();
            let mut offset = 0;
            let (mut moov, mut mdat) = (None, None);
            while offset + 8 <= bytes.len() {
                let size =
                    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
                let kind = &bytes[offset + 4..offset + 8];
                if kind == b"moov" {
                    moov = Some(offset);
                }
                if kind == b"mdat" {
                    mdat = Some(offset);
                }
                if size < 8 {
                    break;
                }
                offset += size;
            }
            assert!(
                moov.zip(mdat).is_some_and(|(moov, mdat)| moov < mdat),
                "requested faststart must be physically applied before sealing"
            );
        }
        let picture = f.0.join("picture.mkv");
        let mut a = args(&["-v", "error", "-i"]);
        a.push(out.clone().into());
        a.extend(args(&["-map", "0:v", "-c", "copy"]));
        a.push(picture.clone().into());
        run("ffmpeg", a);
        let reference = f.0.join(format!("reference.{extension}"));
        let mut a = args(&["-v", "error", "-ss", "0.979", "-i"]);
        a.push(input.into());
        a.push("-i".into());
        a.push(picture.into());
        a.extend(args(&[
            "-map",
            "1:v",
            "-map",
            "0:s",
            "-c",
            "copy",
            "-c:s",
            codec,
            "-t",
            "2.020",
            "-max_interleave_delta",
            "0",
        ]));
        a.push(reference.clone().into());
        run("ffmpeg", a);
        assert_packets(&actual, &probe(&reference), 1);
    }
}

#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9 libx264/libvpx-vp9 and authored PGS"]
async fn source_bitmap_trim_modes_render_on_source_clock_before_crop() {
    for mode in ["frames", "inputSeek", "decodeFromStart"] {
        let f = Fixture::new();
        let pgs = pgs_fixture(&f.0);
        let input = f.0.join("bitmap-primary.mkv");
        let mut a = args(&[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=black:s=320x180:r=24:d=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=4",
            "-i",
        ]);
        a.push(pgs.into());
        a.extend(args(&[
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:s",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "pcm_s16le",
            "-c:s",
            "copy",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-chroma_sample_location",
            "left",
        ]));
        a.push(input.clone().into());
        run("ffmpeg", a);
        let output = f.0.join("bitmap-final.mkv");
        let trim = serde_json::from_value(if mode == "frames" {
            json!({"startFrame":12,"endFrameExclusive":48})
        } else {
            json!({"time":{"startMilliseconds":500,"endMilliseconds":2000,"seekStrategy":mode}})
        })
        .unwrap();
        let mut req = request(&input, &output, trim);
        req.source.stream_indices = vec![0, 1, 2];
        req.settings.lossless = true;
        req.settings.subtitles = vec![SubtitleTrackSettings {
            stream_index: 2,
            mode: SubtitleMode::BurnIn,
            burn_in: false,
        }];
        req.settings.framing.crop.left = 16;
        req.settings.framing.crop.right = 16;
        let manager = JobManager::new(f.0.join("logs"));
        let job = manager.start_encode(req).await.unwrap();
        let done = terminal(&manager, &job.id).await;
        manager.shutdown().await;
        assert_eq!(done.state, JobState::Succeeded, "{mode}: {done:#?}");
        let reference = f.0.join("bitmap-reference.mkv");
        let mut a = args(&["-v", "error"]);
        if mode == "inputSeek" {
            a.extend(args(&["-ss", "0.500"]));
        }
        a.push("-i".into());
        a.push(input.into());
        a.extend(args(&[
            "-filter_complex",
            "[0:0][0:2]overlay=shortest=1,crop=288:180:16:0[out]",
            "-map",
            "[out]",
        ]));
        if mode != "inputSeek" {
            a.extend(args(&[
                "-ss",
                if mode == "frames" { "0.479" } else { "0.500" },
            ]));
        }
        a.extend(args(&[
            "-t",
            if mode == "frames" { "1.520" } else { "1.500" },
            "-frames:v",
            "36",
            "-an",
            "-sn",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "ffv1",
        ]));
        a.push(reference.clone().into());
        run("ffmpeg", a);
        let (actual, expected) = (pixels(&output), pixels(&reference));
        assert!(
            actual == expected,
            "{mode} bitmap bytes {}/{}; first difference {:?}",
            actual.len(),
            expected.len(),
            actual.iter().zip(&expected).position(|(a, b)| a != b)
        );
    }
}
