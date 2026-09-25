//! Opt-in native checks for per-track offsets and external measured gain.
use media_core::{
    AudioChannels, AudioCodec, AudioGain, EncodeTrackOverride, EncodeTrackRef, LoudnessRequest,
    VideoTrim,
};
use media_runtime::{
    EncodeRequest, EncodeSettings, ExternalAudioSettings, ExternalTrack, JobManager, JobSnapshot,
    JobState, RemuxRequest, VideoEncoder, measure_loudness,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Once,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static BUNDLED_TOOLS: Once = Once::new();
static FIXTURE_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        if let Some(root) = std::env::var_os("JESSES_TEST_TOOL_RESOURCES") {
            BUNDLED_TOOLS.call_once(|| {
                media_runtime::configure_bundled_tools(PathBuf::from(root)).unwrap();
            });
        }
        let base = std::env::var_os("JESSES_TEST_EXTERNAL_CONTROL_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join(format!(
            "jesses-external-controls-{}-{}-{}",
            std::process::id(),
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
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
        if std::thread::panicking()
            || std::env::var_os("JESSES_TEST_EXTERNAL_CONTROL_EVIDENCE").is_some()
        {
            eprintln!("External control evidence retained: {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

async fn tool(name: &str, args: Vec<OsString>) -> Vec<u8> {
    let capability = media_runtime::get_capabilities()
        .await
        .into_iter()
        .find(|tool| tool.id == name)
        .unwrap();
    let path = capability
        .path
        .unwrap_or_else(|| panic!("required native tool {name}: {:?}", capability.detail));
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = run_capture(
        &CommandSpec {
            executable: path.into(),
            args,
            cwd: None,
        },
        cancel,
        16 * 1024 * 1024,
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

async fn fixture_media(root: &Path) -> (PathBuf, PathBuf) {
    let primary = root.join("primary.mkv");
    let external = root.join("two-tones.mka");
    let mut args = [
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=2",
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
    .map(OsString::from)
    .to_vec();
    args.push(primary.as_os_str().to_owned());
    tool("ffmpeg", args).await;
    let mut args = [
        "-v",
        "error",
        "-nostdin",
        "-copyts",
        "-itsoffset",
        "0.25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=667:sample_rate=48000:duration=2",
        "-itsoffset",
        "0.25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=880:sample_rate=48000:duration=2",
        "-map",
        "0:a",
        "-map",
        "1:a",
        "-c:a",
        "flac",
        "-metadata:s:a:0",
        "language=spa",
        "-metadata:s:a:1",
        "language=fra",
        "-avoid_negative_ts",
        "disabled",
    ]
    .map(OsString::from)
    .to_vec();
    args.push(external.as_os_str().to_owned());
    tool("ffmpeg", args).await;
    (primary, external)
}

async fn donor(root: &Path, name: &str, title: &str, chapter: Option<&str>) -> PathBuf {
    let metadata = root.join(format!("{name}.ffmeta"));
    let mut content = format!(";FFMETADATA1\ntitle={title}\n");
    if let Some(chapter) = chapter {
        content.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART=500\nEND=1500\ntitle={chapter}\n"
        ));
    }
    std::fs::write(&metadata, content).unwrap();
    let output = root.join(format!("{name}.mka"));
    let mut args = [
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=2",
        "-f",
        "ffmetadata",
        "-i",
    ]
    .map(OsString::from)
    .to_vec();
    args.push(metadata.as_os_str().to_owned());
    args.extend([
        "-map".into(),
        "0:a:0".into(),
        "-map_metadata".into(),
        "1".into(),
        "-map_chapters".into(),
        "1".into(),
        "-c:a".into(),
        "flac".into(),
    ]);
    args.push(output.as_os_str().to_owned());
    tool("ffmpeg", args).await;
    output
}

async fn probe(path: &Path, options: &[&str]) -> Value {
    let mut args = ["-v", "error", "-of", "json"].map(OsString::from).to_vec();
    args.extend(options.iter().map(OsString::from));
    args.push(path.as_os_str().to_owned());
    serde_json::from_slice(&tool("ffprobe", args).await).unwrap()
}

async fn pcm(path: &Path, index: u32) -> Vec<f32> {
    let mut args = ["-v", "error", "-nostdin", "-i"]
        .map(OsString::from)
        .to_vec();
    args.push(path.as_os_str().to_owned());
    args.extend([
        "-map".into(),
        format!("0:{index}").into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        "48000".into(),
        "-c:a".into(),
        "pcm_f32le".into(),
        "-f".into(),
        "f32le".into(),
        "pipe:1".into(),
    ]);
    let bytes = tool("ffmpeg", args).await;
    let (samples, remainder) = bytes.as_chunks::<4>();
    assert!(remainder.is_empty());
    samples
        .iter()
        .map(|sample| f32::from_le_bytes(*sample))
        .collect()
}

async fn decoded_bounds(path: &Path, stream_index: u32) -> (f64, f64, usize) {
    let frames = probe(
        path,
        &[
            "-select_streams",
            &stream_index.to_string(),
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp_time,nb_samples",
        ],
    )
    .await;
    let frames = frames["frames"].as_array().unwrap();
    let first = frames.first().unwrap()["best_effort_timestamp_time"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    let last = frames.last().unwrap();
    let end = last["best_effort_timestamp_time"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap()
        + last["nb_samples"].as_u64().unwrap() as f64 / 48_000.0;
    (first, end, frames.len())
}

async fn finish(manager: &JobManager, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(120), async {
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
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("native encode must finish")
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg, FFprobe, x264 and loudness filter"]
async fn mixed_offsets_and_external_measured_gain_bind_the_right_file_and_track() {
    let fixture = Fixture::new();
    let (primary, external) = fixture_media(&fixture.0).await;
    let source_hashes =
        [&primary, &external].map(|path| Sha256::digest(std::fs::read(path).unwrap()));
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let measurement = measure_loudness(
        LoudnessRequest {
            input_path: external.to_string_lossy().into(),
            stream_index: 0,
            channels: AudioChannels::Preserve,
            target_lufs: -23.0,
            peak_limit_dbfs: -1.0,
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    let output = fixture.0.join("shifted.mkv");
    let mut request = EncodeRequest {
        source: RemuxRequest {
            input_path: primary.to_string_lossy().into(),
            output_path: output.to_string_lossy().into(),
            stream_indices: vec![0],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::X264,
            preset: 0,
            external_tracks: vec![
                ExternalTrack {
                    input_path: external.to_string_lossy().into(),
                    stream_index: 0,
                    offset_milliseconds: 500,
                    subtitle_mode: None,
                    title: None,
                    language: None,
                    default: None,
                    forced: None,
                    audio: Some(ExternalAudioSettings {
                        codec: AudioCodec::Flac,
                        bitrate_kbps: 128,
                        channels: AudioChannels::Preserve,
                        gain: Some(AudioGain {
                            tenths_db: -60,
                            source_fingerprint: Some(measurement.source_fingerprint),
                        }),
                    }),
                },
                ExternalTrack {
                    input_path: external.to_string_lossy().into(),
                    stream_index: 1,
                    offset_milliseconds: -125,
                    subtitle_mode: None,
                    title: None,
                    language: None,
                    default: None,
                    forced: None,
                    audio: None,
                },
            ],
            ..Default::default()
        },
    };
    let plan = media_runtime::preview_encode_plan(request.clone(), cancel)
        .await
        .unwrap();
    let mux = plan
        .stages
        .iter()
        .find(|stage| stage.label.contains("Selected tracks"))
        .unwrap();
    let maps = mux
        .arguments
        .windows(2)
        .filter(|pair| pair[0] == "-map")
        .map(|pair| pair[1].as_str())
        .collect::<Vec<_>>();
    assert_eq!(maps, vec!["1:v:0", "2:0", "3:1"]);
    let offsets = mux
        .arguments
        .windows(2)
        .filter(|pair| pair[0] == "-itsoffset")
        .map(|pair| pair[1].as_str())
        .collect::<Vec<_>>();
    assert_eq!(offsets, vec!["0.500", "-0.125"]);
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager.start_encode(request.clone()).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    std::fs::write(
        fixture.0.join("snapshot.json"),
        serde_json::to_vec_pretty(&completed).unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    let streams = probe(&output, &["-show_streams", "-count_frames"]).await;
    assert_eq!(streams["streams"][0]["nb_read_frames"], "48");
    assert_eq!(streams["streams"][1]["codec_name"], "flac");
    assert_eq!(streams["streams"][2]["codec_name"], "flac");
    assert_eq!(streams["streams"][1]["tags"]["language"], "spa");
    assert_eq!(streams["streams"][2]["tags"]["language"], "fra");
    let before = pcm(&external, 0).await;
    let gained = pcm(&output, 1).await;
    assert_eq!(before.len(), gained.len());
    let original_rms = (before
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        / before.len() as f64)
        .sqrt();
    let gained_rms = (gained
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        / gained.len() as f64)
        .sqrt();
    assert!((gained_rms / original_rms - 10.0_f64.powf(-6.0 / 20.0)).abs() < 0.001);
    let source_packets = probe(
        &external,
        &[
            "-select_streams",
            "1",
            "-show_packets",
            "-show_data_hash",
            "sha256",
        ],
    )
    .await;
    let output_packets = probe(
        &output,
        &[
            "-select_streams",
            "2",
            "-show_packets",
            "-show_data_hash",
            "sha256",
        ],
    )
    .await;
    let source_packets = source_packets["packets"].as_array().unwrap();
    let output_packets = output_packets["packets"].as_array().unwrap();
    assert_eq!(source_packets.len(), output_packets.len());
    for (source, shifted) in source_packets.iter().zip(output_packets) {
        for field in ["data_hash", "size", "duration_time", "flags"] {
            assert_eq!(source[field], shifted[field], "copied packet {field}");
        }
        for field in ["pts_time", "dts_time"] {
            let before = source[field].as_str().unwrap().parse::<f64>().unwrap();
            let after = shifted[field].as_str().unwrap().parse::<f64>().unwrap();
            assert!(
                (after - before + 0.125).abs() <= 0.001001,
                "{field}: {before} -> {after}"
            );
        }
    }
    for (path, hash) in [&primary, &external].into_iter().zip(source_hashes) {
        assert_eq!(Sha256::digest(std::fs::read(path).unwrap()), hash);
    }
    request.source.output_path = fixture.0.join("stale.mkv").to_string_lossy().into();
    request.settings.external_tracks[0]
        .audio
        .as_mut()
        .unwrap()
        .gain
        .as_mut()
        .unwrap()
        .source_fingerprint = Some("0".repeat(64));
    let stale = manager.start_encode(request).await.unwrap();
    let failed = finish(&manager, &stale.id).await;
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.error.as_ref().unwrap().code, "SOURCE_CHANGED");
    assert!(failed.standalone_recovery.is_none());
    assert!(!fixture.0.join("stale.mkv").exists());
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg, FFprobe and x264"]
async fn external_mov_timecode_is_appended_after_verified_video_stage() {
    let fixture = Fixture::new();
    let (primary, _) = fixture_media(&fixture.0).await;
    let timecode_source = fixture.0.join("timecode-source.mov");
    let mut args = [
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=2",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-pix_fmt",
        "yuv420p",
        "-c:v",
        "libx264",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
        "-metadata:s:v:0",
        "handler_name=Old Label",
        "-timecode",
        "01:02:03:04",
    ]
    .map(OsString::from)
    .to_vec();
    args.push(timecode_source.as_os_str().to_owned());
    tool("ffmpeg", args).await;
    let original = probe(
        &timecode_source,
        &[
            "-show_streams",
            "-show_packets",
            "-show_data_hash",
            "sha256",
        ],
    )
    .await;
    assert_eq!(original["streams"][1]["codec_tag_string"], "tmcd");
    assert_eq!(original["streams"][0]["tags"]["handler_name"], "Old Label");
    let hashes =
        [&primary, &timecode_source].map(|path| Sha256::digest(std::fs::read(path).unwrap()));
    let output = fixture.0.join("with-timecode.mov");
    let mut request = EncodeRequest {
        source: RemuxRequest {
            input_path: primary.to_string_lossy().into(),
            output_path: output.to_string_lossy().into(),
            stream_indices: vec![0],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::X264,
            preset: 0,
            mov_timecode_track: Some(EncodeTrackRef {
                input_path: Some(timecode_source.to_string_lossy().into()),
                stream_index: 1,
            }),
            ..Default::default()
        },
    };
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager.start_encode(request.clone()).await.unwrap();
    let finished = finish(&manager, &started.id).await;
    std::fs::write(
        fixture.0.join("timecode-snapshot.json"),
        serde_json::to_vec_pretty(&finished).unwrap(),
    )
    .unwrap();
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    let final_probe = probe(
        &output,
        &[
            "-show_streams",
            "-show_packets",
            "-show_data_hash",
            "sha256",
        ],
    )
    .await;
    assert_eq!(final_probe["streams"][0]["codec_name"], "h264");
    assert_eq!(final_probe["streams"][1]["codec_type"], "data");
    assert_eq!(final_probe["streams"][1]["codec_tag_string"], "tmcd");
    assert_eq!(final_probe["streams"][1]["tags"]["timecode"], "01:02:03:04");
    let before = original["packets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|packet| packet["stream_index"] == 1)
        .unwrap();
    let after = final_probe["packets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|packet| packet["stream_index"] == 1)
        .unwrap();
    assert_eq!(before["data_hash"], after["data_hash"]);
    assert_eq!(before["pts_time"], after["pts_time"]);
    assert_eq!(before["duration_time"], after["duration_time"]);
    for (path, hash) in [&primary, &timecode_source].into_iter().zip(hashes) {
        assert_eq!(Sha256::digest(std::fs::read(path).unwrap()), hash);
    }
    let mut primary_timecode = request.clone();
    primary_timecode.source.input_path = timecode_source.to_string_lossy().into();
    primary_timecode.source.output_path = fixture
        .0
        .join("primary-timecode.mov")
        .to_string_lossy()
        .into();
    primary_timecode.settings.mov_timecode_track = Some(EncodeTrackRef {
        input_path: None,
        stream_index: 1,
    });
    primary_timecode.settings.track_overrides = vec![EncodeTrackOverride {
        stream_index: 0,
        title: Some(String::new()),
        language: None,
        default: None,
        forced: None,
    }];
    let started = manager.start_encode(primary_timecode).await.unwrap();
    let finished = finish(&manager, &started.id).await;
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    let primary_result = probe(&fixture.0.join("primary-timecode.mov"), &["-show_streams"]).await;
    assert_eq!(primary_result["streams"][1]["codec_tag_string"], "tmcd");
    assert!(primary_result["streams"][0]["tags"]["title"].is_null());
    assert_ne!(
        primary_result["streams"][0]["tags"]["handler_name"],
        "Old Label"
    );
    assert_eq!(
        primary_result["streams"][1]["tags"]["timecode"],
        "01:02:03:04"
    );
    request.settings.trim = Some(VideoTrim {
        start_frame: 0,
        end_frame_exclusive: 24,
        time: None,
    });
    request.source.output_path = fixture
        .0
        .join("forbidden-timecode.mov")
        .to_string_lossy()
        .into();
    let rejected = manager.start_encode(request).await.unwrap_err();
    assert_eq!(rejected.code, "EXTERNAL_TRACKS_INVALID");
    assert!(!fixture.0.join("forbidden-timecode.mov").exists());
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg, FFprobe and x264"]
async fn manual_external_gain_and_offset_trim_preserve_exact_decoded_samples() {
    let fixture = Fixture::new();
    let (primary, external) = fixture_media(&fixture.0).await;
    let original_hash = Sha256::digest(std::fs::read(&external).unwrap());
    let output = fixture.0.join("trimmed-manual.mkv");
    let request = EncodeRequest {
        source: RemuxRequest {
            input_path: primary.to_string_lossy().into(),
            output_path: output.to_string_lossy().into(),
            stream_indices: vec![0],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::X264,
            preset: 0,
            trim: Some(VideoTrim {
                start_frame: 24,
                end_frame_exclusive: 48,
                time: None,
            }),
            external_tracks: vec![ExternalTrack {
                input_path: external.to_string_lossy().into(),
                stream_index: 0,
                offset_milliseconds: 500,
                subtitle_mode: None,
                title: None,
                language: None,
                default: None,
                forced: None,
                audio: Some(ExternalAudioSettings {
                    codec: AudioCodec::Flac,
                    bitrate_kbps: 128,
                    channels: AudioChannels::Preserve,
                    gain: Some(AudioGain {
                        tenths_db: -30,
                        source_fingerprint: None,
                    }),
                }),
            }],
            ..Default::default()
        },
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let plan = media_runtime::preview_encode_plan(request.clone(), cancel)
        .await
        .unwrap();
    let mux = plan
        .stages
        .iter()
        .find(|stage| stage.label.contains("Selected tracks"))
        .unwrap();
    assert!(
        mux.arguments
            .windows(2)
            .any(|pair| pair == ["-itsoffset", "0.500"])
    );
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager.start_encode(request).await.unwrap();
    let finished = finish(&manager, &started.id).await;
    std::fs::write(
        fixture.0.join("trim-snapshot.json"),
        serde_json::to_vec_pretty(&finished).unwrap(),
    )
    .unwrap();
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    let video = probe(
        &output,
        &["-select_streams", "v:0", "-count_frames", "-show_streams"],
    )
    .await;
    assert_eq!(video["streams"][0]["nb_read_frames"], "24");
    let (start, end, _) = decoded_bounds(&output, 1).await;
    assert!(start.abs() < 0.002, "trimmed audio start {start}");
    assert!((end - 1.0).abs() < 0.002, "trimmed audio end {end}");
    let original = pcm(&external, 0).await;
    let converted = pcm(&output, 1).await;
    assert_eq!(converted.len(), 48_000);
    let expected = &original[12_000..60_000];
    let ratio = (converted
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>()
        / expected
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>())
    .sqrt();
    assert!(
        (ratio - 10.0_f64.powf(-3.0 / 20.0)).abs() < 0.002,
        "gain ratio {ratio}"
    );
    let correlation = converted
        .iter()
        .zip(expected)
        .map(|(actual, original)| f64::from(*actual) * f64::from(*original))
        .sum::<f64>();
    assert!(
        correlation > 0.0,
        "trimmed audio must preserve the selected tone phase"
    );
    let expected_gain = 10.0_f64.powf(-3.0 / 20.0);
    let rms_sample_error = (converted
        .iter()
        .zip(expected)
        .map(|(actual, source)| (f64::from(*actual) - f64::from(*source) * expected_gain).powi(2))
        .sum::<f64>()
        / converted.len() as f64)
        .sqrt();
    assert!(
        rms_sample_error < 0.0001,
        "trimmed audio sample error {rms_sample_error}"
    );
    assert_eq!(
        Sha256::digest(std::fs::read(&external).unwrap()),
        original_hash
    );
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg, FFprobe and x264"]
async fn cross_file_order_track_overrides_and_independent_donor_only_sources() {
    let fixture = Fixture::new();
    let (primary, external) = fixture_media(&fixture.0).await;
    let metadata_donor = donor(&fixture.0, "container-tags", "Donor Container", None).await;
    let chapter_donor = donor(
        &fixture.0,
        "chapter-list",
        "Not Selected",
        Some("Donor Chapter"),
    )
    .await;
    let sources = [&primary, &external, &metadata_donor, &chapter_donor];
    let source_hashes = sources.map(|path| Sha256::digest(std::fs::read(path).unwrap()));
    let output = fixture.0.join("reordered.mkv");
    let request = EncodeRequest {
        source: RemuxRequest {
            input_path: primary.to_string_lossy().into(),
            output_path: output.to_string_lossy().into(),
            stream_indices: vec![0],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::X264,
            preset: 0,
            external_tracks: vec![
                ExternalTrack {
                    input_path: external.to_string_lossy().into(),
                    stream_index: 0,
                    offset_milliseconds: 0,
                    subtitle_mode: None,
                    audio: Some(ExternalAudioSettings {
                        codec: AudioCodec::Flac,
                        bitrate_kbps: 128,
                        channels: AudioChannels::Preserve,
                        gain: None,
                    }),
                    title: Some("Spanish Guest".into()),
                    language: Some("spa".into()),
                    default: Some(true),
                    forced: Some(false),
                },
                ExternalTrack {
                    input_path: external.to_string_lossy().into(),
                    stream_index: 1,
                    offset_milliseconds: 0,
                    subtitle_mode: None,
                    audio: None,
                    title: Some("French Guest".into()),
                    language: Some("fra".into()),
                    default: Some(false),
                    forced: Some(true),
                },
            ],
            track_overrides: vec![EncodeTrackOverride {
                stream_index: 0,
                title: Some("Encoded Picture".into()),
                language: None,
                default: Some(false),
                forced: None,
            }],
            track_order: vec![
                EncodeTrackRef {
                    input_path: Some(external.to_string_lossy().into()),
                    stream_index: 1,
                },
                EncodeTrackRef {
                    input_path: None,
                    stream_index: 0,
                },
                EncodeTrackRef {
                    input_path: Some(external.to_string_lossy().into()),
                    stream_index: 0,
                },
            ],
            metadata_source_path: Some(metadata_donor.to_string_lossy().into()),
            chapters_source_path: Some(chapter_donor.to_string_lossy().into()),
            ..Default::default()
        },
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let preview = media_runtime::preview_encode_plan(request.clone(), cancel)
        .await
        .unwrap();
    let mux = preview
        .stages
        .iter()
        .find(|stage| stage.label.contains("Selected tracks"))
        .unwrap();
    let maps = mux
        .arguments
        .windows(2)
        .filter(|args| args[0] == "-map")
        .map(|args| args[1].as_str())
        .collect::<Vec<_>>();
    assert_eq!(maps, ["2:1", "1:v:0", "2:0"]);
    assert!(
        mux.arguments
            .windows(2)
            .any(|args| args == ["-map_metadata", "3"])
    );
    assert!(
        mux.arguments
            .windows(2)
            .any(|args| args == ["-map_chapters", "4"])
    );
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager.start_encode(request.clone()).await.unwrap();
    let finished = finish(&manager, &started.id).await;
    std::fs::write(
        fixture.0.join("reordered-snapshot.json"),
        serde_json::to_vec_pretty(&finished).unwrap(),
    )
    .unwrap();
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    let result = probe(
        &output,
        &["-show_streams", "-show_chapters", "-show_format"],
    )
    .await;
    let streams = result["streams"].as_array().unwrap();
    assert_eq!(streams[0]["codec_type"], "audio");
    assert_eq!(streams[1]["codec_type"], "video");
    assert_eq!(streams[2]["codec_type"], "audio");
    assert_eq!(streams[0]["tags"]["title"], "French Guest");
    assert_eq!(streams[0]["tags"]["language"], "fra");
    assert_eq!(streams[0]["disposition"]["forced"], 1);
    assert_eq!(streams[0]["disposition"]["default"], 0);
    assert_eq!(streams[1]["tags"]["title"], "Encoded Picture");
    assert_eq!(streams[2]["tags"]["title"], "Spanish Guest");
    assert_eq!(streams[2]["tags"]["language"], "spa");
    assert_eq!(streams[2]["disposition"]["default"], 1);
    assert_eq!(result["format"]["tags"]["title"], "Donor Container");
    assert_eq!(result["chapters"][0]["tags"]["title"], "Donor Chapter");
    let mut trimmed = request;
    let trimmed_output = fixture.0.join("trimmed-donor.mkv");
    trimmed.source.output_path = trimmed_output.to_string_lossy().into();
    trimmed.settings.external_tracks.clear();
    trimmed.settings.track_order.clear();
    trimmed.settings.track_overrides.clear();
    trimmed.settings.trim = Some(VideoTrim {
        start_frame: 24,
        end_frame_exclusive: 48,
        time: None,
    });
    let started = manager.start_encode(trimmed).await.unwrap();
    let finished = finish(&manager, &started.id).await;
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    let clipped = probe(&trimmed_output, &["-show_chapters", "-show_format"]).await;
    assert_eq!(clipped["format"]["tags"]["title"], "Donor Container");
    let chapters = clipped["chapters"].as_array().unwrap();
    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0]["tags"]["title"], "Donor Chapter");
    let chapter_start: f64 = chapters[0]["start_time"].as_str().unwrap().parse().unwrap();
    let chapter_end: f64 = chapters[0]["end_time"].as_str().unwrap().parse().unwrap();
    assert!(
        chapter_start.abs() <= 0.002,
        "chapter start {chapter_start}"
    );
    assert!(
        (chapter_end - 0.5).abs() <= 0.002,
        "chapter end {chapter_end}"
    );
    for (path, hash) in sources.into_iter().zip(source_hashes) {
        assert_eq!(Sha256::digest(std::fs::read(path).unwrap()), hash);
    }
    manager.shutdown().await;
}
