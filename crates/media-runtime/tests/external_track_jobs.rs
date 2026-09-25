//! Mixed-source encodes use generated media and independent packet evidence.
use media_runtime::{
    EncodeRequest, EncodeSettings, ExternalTrack, JobManager, JobSnapshot, JobState, RemuxRequest,
    VideoEncoder,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        if let Some(resources) = std::env::var_os("JESSES_TEST_TOOL_RESOURCES") {
            media_runtime::configure_bundled_tools(PathBuf::from(resources)).unwrap();
        }
        let base = std::env::var_os("JESSES_TEST_EXTERNAL_TRACK_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join(format!(
            "jesses-external-{}-{}",
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
        if std::thread::panicking()
            || std::env::var_os("JESSES_TEST_EXTERNAL_TRACK_EVIDENCE").is_some()
        {
            eprintln!("Mixed-source evidence retained: {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
async fn tool(name: &str, args: Vec<OsString>) -> Vec<u8> {
    // Discovery checks all tools; keep its future off the native test's stack.
    let capability = Box::pin(media_runtime::get_capabilities())
        .await
        .into_iter()
        .find(|tool| tool.id == name)
        .unwrap();
    let path = capability
        .path
        .as_ref()
        .unwrap_or_else(|| panic!("required fixture tool {name}: {capability:?}"));
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(
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
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
async fn fixtures(root: &Path) -> (PathBuf, PathBuf) {
    let primary = root.join("picture's $主.mkv");
    let external = root.join("音声 and subtitles.mkv");
    let subtitle = root.join("cues.srt");
    let chapters = root.join("chapters.txt");
    let font = root.join("font.ttf");
    std::fs::write(&subtitle, "1\n00:00:00,300 --> 00:00:00,900\nFirst cue\n\n2\n00:00:01,100 --> 00:00:01,800\nSecond cue\n").unwrap();
    std::fs::write(&chapters, ";FFMETADATA1\ntitle=Video owner\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1000\ntitle=Opening\n").unwrap();
    std::fs::write(&font, b"attachment integrity fixture").unwrap();
    let mut command = args(&[
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=2",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=2",
        "-i",
    ]);
    command.push(subtitle.clone().into_os_string());
    command.extend(args(&["-f", "ffmetadata", "-i"]));
    command.push(chapters.into_os_string());
    command.extend(args(&[
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map",
        "2:s",
        "-map_metadata",
        "3",
        "-map_chapters",
        "3",
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
        "-c:a",
        "flac",
        "-c:s",
        "srt",
        "-metadata:s:a:0",
        "language=eng",
        "-attach",
    ]));
    command.push(font.clone().into_os_string());
    command.extend(args(&[
        "-metadata:s:t:0",
        "mimetype=application/x-truetype-font",
        "-metadata:s:t:0",
        "filename=primary.ttf",
    ]));
    command.push(primary.clone().into_os_string());
    tool("ffmpeg", command).await;
    let mut command = args(&[
        "-v",
        "error",
        "-nostdin",
        "-copyts",
        "-itsoffset",
        "0.25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=660:sample_rate=48000:duration=1.75",
        "-i",
    ]);
    command.push(subtitle.into_os_string());
    command.extend(args(&[
        "-map",
        "0:a",
        "-map",
        "1:s",
        "-c:a",
        "flac",
        "-c:s",
        "srt",
        "-avoid_negative_ts",
        "disabled",
        "-metadata",
        "title=External owner must not replace video owner",
        "-metadata:s:a:0",
        "language=spa",
        "-metadata:s:a:0",
        "title=Dub's $ audio",
        "-disposition:a:0",
        "0",
        "-metadata:s:s:0",
        "language=jpn",
        "-disposition:s:0",
        "forced",
        "-attach",
    ]));
    command.push(font.into_os_string());
    command.extend(args(&[
        "-metadata:s:t:0",
        "mimetype=application/x-truetype-font",
        "-metadata:s:t:0",
        "filename=external.ttf",
    ]));
    command.push(external.clone().into_os_string());
    tool("ffmpeg", command).await;
    (primary, external)
}
fn request(primary: &Path, external: &Path, output: &Path) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: primary.to_string_lossy().into(),
            output_path: output.to_string_lossy().into(),
            stream_indices: vec![0, 1, 2, 3],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::X264,
            preset: 0,
            external_tracks: [2, 1, 0]
                .into_iter()
                .map(|stream_index| ExternalTrack {
                    offset_milliseconds: 0,
                    subtitle_mode: None,
                    title: None,
                    language: None,
                    default: None,
                    forced: None,
                    audio: None,
                    input_path: external.to_string_lossy().into(),
                    stream_index,
                })
                .collect(),
            subtitles: vec![media_core::SubtitleTrackSettings {
                stream_index: 2,
                mode: media_core::SubtitleMode::Ass,
            }],
            ..Default::default()
        },
    }
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
    .expect("encode must finish")
}
async fn probe(path: &Path, options: &[&str]) -> Value {
    let mut command = args(&["-v", "error", "-of", "json"]);
    command.extend(args(options));
    command.push(path.as_os_str().to_owned());
    serde_json::from_slice(&tool("ffprobe", command).await).unwrap()
}
async fn packet_evidence(path: &Path, index: u32) -> Value {
    let value = probe(
        path,
        &[
            "-select_streams",
            &index.to_string(),
            "-show_packets",
            "-show_data_hash",
            "sha256",
        ],
    )
    .await;
    Value::Array(value["packets"].as_array().unwrap().iter().map(|p|json!({"hash":p["data_hash"],"pts":p["pts_time"],"dts":p["dts_time"],"flags":p["flags"],"size":p["size"]})).collect())
}

fn audio(
    codec: media_core::AudioCodec,
    channels: media_core::AudioChannels,
) -> media_core::ExternalAudioSettings {
    media_core::ExternalAudioSettings {
        gain: None,
        codec,
        bitrate_kbps: 128,
        channels,
    }
}

async fn tone_source(
    root: &Path,
    name: &str,
    frequency: u32,
    start: &str,
    channels: u8,
    language: &str,
    title: &str,
) -> PathBuf {
    let path = root.join(name);
    let mut command = args(&[
        "-v",
        "error",
        "-nostdin",
        "-copyts",
        "-itsoffset",
        start,
        "-f",
        "lavfi",
    ]);
    command.push("-i".into());
    command.push(format!("sine=frequency={frequency}:sample_rate=48000:duration=1.5").into());
    command.extend([
        "-ac".into(),
        channels.to_string().into(),
        "-c:a".into(),
        "flac".into(),
        "-avoid_negative_ts".into(),
        "disabled".into(),
        "-metadata:s:a:0".into(),
        format!("language={language}").into(),
        "-metadata:s:a:0".into(),
        format!("title={title}").into(),
        path.as_os_str().to_owned(),
    ]);
    tool("ffmpeg", command).await;
    path
}

async fn pcm_source(root: &Path, name: &str, start: &str) -> PathBuf {
    let path = root.join(name);
    let mut command = args(&[
        "-v",
        "error",
        "-nostdin",
        "-copyts",
        "-itsoffset",
        start,
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=660:sample_rate=48000:duration=1.75",
        "-c:a",
        "pcm_s16le",
        "-avoid_negative_ts",
        "disabled",
        "-metadata:s:a:0",
        "language=spa",
    ]);
    command.push(path.as_os_str().to_owned());
    tool("ffmpeg", command).await;
    assert_eq!(
        probe(&path, &["-show_streams"]).await["streams"][0]["codec_name"],
        "pcm_s16le"
    );
    path
}

async fn pcm(path: &Path, index: u32) -> Vec<u8> {
    let mut command = args(&["-v", "error", "-nostdin", "-i"]);
    command.push(path.as_os_str().to_owned());
    command.extend([
        "-map".into(),
        format!("0:{index}").into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        "48000".into(),
        "-f".into(),
        "s16le".into(),
        "pipe:1".into(),
    ]);
    tool("ffmpeg", command).await
}

fn tone_power(pcm: &[u8], frequency: f64) -> f64 {
    let samples = pcm.as_chunks::<2>().0.iter().take(24_000);
    let (re, im) = samples
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (index, sample)| {
            let value = f64::from(i16::from_le_bytes([sample[0], sample[1]]));
            let phase = std::f64::consts::TAU * frequency * index as f64 / 48_000.0;
            (re + value * phase.cos(), im + value * phase.sin())
        });
    re * re + im * im
}

async fn decoded_bounds(path: &Path, index: u32) -> (f64, f64) {
    let frames = probe(
        path,
        &["-select_streams", &index.to_string(), "-show_frames"],
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
    (first, end)
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn copied_external_tracks_keep_order_timestamps_metadata_and_sources() {
    let root = Fixture::new();
    let (primary, external) = fixtures(&root.0).await;
    let originals = [&primary, &external].map(|p| {
        (
            Sha256::digest(std::fs::read(p).unwrap()),
            std::fs::metadata(p).unwrap().modified().unwrap(),
        )
    });
    let output = root.0.join("combined.mkv");
    let request = request(&primary, &external, &output);
    let history = root.0.join("history");
    let logs = root.0.join("logs");
    let manager = JobManager::open(logs.clone(), history.clone()).await;
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
        .filter(|p| p[0] == "-map")
        .map(|p| p[1].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        maps,
        vec!["1:v:0", "0:1", "3:0", "2:1", "2:0", "0:3", "2:2"]
    );
    let started = manager.start_encode(request.clone()).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    assert_eq!(
        completed.state,
        JobState::Succeeded,
        "{:?}\n{:?}",
        completed.error,
        completed.logs
    );
    let metadata = probe(
        &output,
        &[
            "-show_streams",
            "-show_format",
            "-show_chapters",
            "-count_frames",
        ],
    )
    .await;
    assert_eq!(
        metadata["streams"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["codec_type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "video",
            "audio",
            "subtitle",
            "subtitle",
            "audio",
            "attachment",
            "attachment"
        ]
    );
    assert_eq!(metadata["streams"][0]["nb_read_frames"], "48");
    assert_eq!(metadata["streams"][2]["codec_name"], "ass");
    assert_eq!(metadata["streams"][3]["disposition"]["forced"], 1);
    assert_eq!(metadata["streams"][4]["tags"]["language"], "spa");
    assert_eq!(metadata["streams"][4]["disposition"]["default"], 0);
    assert_eq!(metadata["streams"][5]["tags"]["filename"], "primary.ttf");
    assert_eq!(metadata["streams"][6]["tags"]["filename"], "external.ttf");
    assert_eq!(metadata["format"]["tags"]["title"], "Video owner");
    assert_eq!(metadata["chapters"][0]["tags"]["title"], "Opening");
    assert_eq!(
        packet_evidence(&external, 0).await,
        packet_evidence(&output, 4).await
    );
    assert_eq!(
        packet_evidence(&external, 1).await,
        packet_evidence(&output, 3).await
    );
    for (path, (hash, modified)) in [&primary, &external].into_iter().zip(originals) {
        assert_eq!(Sha256::digest(std::fs::read(path).unwrap()), hash);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            modified
        );
    }
    manager.shutdown().await;
    drop(manager);
    let reopened = JobManager::open(logs, history).await;
    assert_eq!(
        reopened.list_jobs().await[0]
            .encode_settings
            .as_ref()
            .unwrap()
            .external_tracks,
        request.settings.external_tracks
    );
    reopened.shutdown().await;
    assert!(std::fs::read_dir(&root.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jesses-")
    }));
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg, FFprobe and standalone x264"]
async fn converted_external_audio_keeps_distinct_sources_timestamps_and_primary_tracks() {
    use media_core::{AudioChannels as Channels, AudioCodec as Codec};
    let root = Fixture::new();
    let (primary, first) = fixtures(&root.0).await;
    let second = tone_source(&root.0, "second.mka", 880, "0.5", 2, "fra", "Second tone").await;
    let third = tone_source(&root.0, "third.mka", 990, "0.75", 1, "deu", "Third tone").await;
    let sources = [&primary, &first, &second, &third];
    let originals = sources.map(|path| Sha256::digest(std::fs::read(path).unwrap()));
    let output = root.0.join("converted.mkv");
    let mut mixed = request(&primary, &first, &output);
    let selected = |path: &Path, index, audio| ExternalTrack {
        offset_milliseconds: 0,
        subtitle_mode: None,
        title: None,
        language: None,
        default: None,
        forced: None,
        input_path: path.to_string_lossy().into(),
        stream_index: index,
        audio,
    };
    mixed.settings.external_tracks = vec![
        selected(&first, 0, Some(audio(Codec::Aac, Channels::Stereo))),
        selected(&second, 0, Some(audio(Codec::Opus, Channels::Mono))),
        selected(&third, 0, Some(audio(Codec::Flac, Channels::Preserve))),
        selected(&first, 1, None),
        selected(&first, 2, None),
    ];
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let preview = media_runtime::preview_encode_plan(mixed.clone(), cancel)
        .await
        .unwrap();
    let mux = preview
        .stages
        .iter()
        .find(|stage| stage.label.contains("Selected tracks"))
        .unwrap();
    let mapped = mux
        .arguments
        .windows(2)
        .filter(|pair| pair[0] == "-map")
        .map(|pair| pair[1].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        mapped,
        vec![
            "1:v:0", "0:1", "5:0", "2:0", "3:0", "4:0", "2:1", "0:3", "2:2"
        ]
    );
    for (position, encoder, channels) in [(3, "aac", "2"), (4, "libopus", "1"), (5, "flac", "")] {
        assert!(
            mux.arguments
                .windows(2)
                .any(|pair| pair[0] == format!("-c:{position}") && pair[1] == encoder)
        );
        if !channels.is_empty() {
            assert!(
                mux.arguments
                    .windows(2)
                    .any(|pair| pair[0] == format!("-ac:{position}") && pair[1] == channels)
            );
        }
    }
    let manager = JobManager::new(root.0.join("logs"));
    let started = manager.start_encode(mixed).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    std::fs::write(
        root.0.join("snapshot.json"),
        serde_json::to_vec_pretty(&completed).unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    let metadata = probe(
        &output,
        &[
            "-show_streams",
            "-show_format",
            "-show_chapters",
            "-count_frames",
        ],
    )
    .await;
    assert_eq!(metadata["streams"][0]["nb_read_frames"], "48");
    assert_eq!(metadata["streams"][2]["codec_name"], "ass");
    for (index, codec, channels, language, title) in [
        (3, "aac", 2, "spa", "Dub's $ audio"),
        (4, "opus", 1, "fra", "Second tone"),
        (5, "flac", 1, "deu", "Third tone"),
    ] {
        assert_eq!(metadata["streams"][index]["codec_name"], codec);
        assert_eq!(metadata["streams"][index]["channels"], channels);
        assert_eq!(metadata["streams"][index]["tags"]["language"], language);
        assert_eq!(metadata["streams"][index]["tags"]["title"], title);
    }
    assert_eq!(metadata["streams"][7]["tags"]["filename"], "primary.ttf");
    assert_eq!(metadata["streams"][8]["tags"]["filename"], "external.ttf");
    assert_eq!(metadata["format"]["tags"]["title"], "Video owner");
    assert_eq!(metadata["chapters"][0]["tags"]["title"], "Opening");
    assert_eq!(
        packet_evidence(&primary, 1).await,
        packet_evidence(&output, 1).await
    );
    assert_eq!(
        packet_evidence(&first, 1).await,
        packet_evidence(&output, 6).await
    );
    for (path, index, output_index, expected, rivals) in [
        (&first, 0, 3, 660.0, [880.0, 990.0]),
        (&second, 0, 4, 880.0, [660.0, 990.0]),
        (&third, 0, 5, 990.0, [660.0, 880.0]),
    ] {
        let decoded = pcm(&output, output_index).await;
        assert!(
            decoded.len() >= 48_000,
            "converted audio must decode at least half a second"
        );
        let wanted = tone_power(&decoded, expected);
        assert!(
            rivals
                .into_iter()
                .all(|other| wanted > tone_power(&decoded, other) * 20.0),
            "wrong source at output stream {output_index}"
        );
        let before = decoded_bounds(path, index).await;
        let after = decoded_bounds(&output, output_index).await;
        assert!(
            (before.0 - after.0).abs() <= 0.002 && (before.1 - after.1).abs() <= 0.022,
            "timeline changed for stream {output_index}: {before:?} -> {after:?}"
        );
    }
    assert_eq!(
        pcm(&third, 0).await,
        pcm(&output, 5).await,
        "FLAC conversion changed decoded PCM"
    );
    for (path, hash) in sources.into_iter().zip(originals) {
        assert_eq!(Sha256::digest(std::fs::read(path).unwrap()), hash);
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn invalid_external_sources_fail_without_publishing_or_modifying_media() {
    let root = Fixture::new();
    let (primary, external) = fixtures(&root.0).await;
    let original = Sha256::digest(std::fs::read(&external).unwrap());
    let manager = JobManager::new(root.0.join("logs"));
    for (label, path, index, extension) in [
        ("primary", &primary, 0, "mkv"),
        ("missing-track", &external, 99, "mkv"),
        ("attachment-container", &external, 2, "mp4"),
    ] {
        let output = root.0.join(format!("rejected-{label}.{extension}"));
        let mut request = request(&primary, &external, &output);
        request.source.stream_indices = vec![0];
        request.settings.subtitles.clear();
        request.settings.external_tracks = vec![ExternalTrack {
            offset_milliseconds: 0,
            subtitle_mode: None,
            title: None,
            language: None,
            default: None,
            forced: None,
            audio: None,
            input_path: path.to_string_lossy().into(),
            stream_index: index,
        }];
        let started = manager.start_encode(request).await.unwrap();
        let completed = finish(&manager, &started.id).await;
        assert_eq!(completed.state, JobState::Failed, "{label}: {completed:?}");
        assert!(!output.exists());
    }
    let mut collision = request(&primary, &external, &external);
    collision.settings.subtitles.clear();
    let started = manager.start_encode(collision).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    assert_eq!(completed.state, JobState::Failed, "{completed:?}");
    assert_eq!(Sha256::digest(std::fs::read(&external).unwrap()), original);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn target_size_measurement_includes_external_tracks() {
    let root = Fixture::new();
    let (primary, external) = fixtures(&root.0).await;
    let output = root.0.join("target-size.mkv");
    let mut mixed = request(&primary, &external, &output);
    mixed.settings.rate_control =
        Some(media_core::VideoRateControl::TargetSize { target_size_mib: 1 });
    let mut original = mixed.clone();
    original.settings.external_tracks.clear();
    let preview_bitrate = |plan: &media_core::EncodeCommandPlan| -> u32 {
        plan.stages
            .iter()
            .flat_map(|stage| stage.arguments.windows(2))
            .find(|pair| pair[0] == "--bitrate")
            .expect("two-pass x264 bitrate")[1]
            .parse()
            .unwrap()
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let base = media_runtime::preview_encode_plan(original, cancel.clone())
        .await
        .unwrap();
    let added = media_runtime::preview_encode_plan(mixed.clone(), cancel)
        .await
        .unwrap();
    assert!(
        preview_bitrate(&added) < preview_bitrate(&base),
        "external bytes must reduce video budget"
    );
    let mut converted = mixed.clone();
    converted.settings.external_tracks[2].audio = Some(audio(
        media_core::AudioCodec::Aac,
        media_core::AudioChannels::Stereo,
    ));
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let planned = media_runtime::preview_encode_plan(converted.clone(), cancel)
        .await
        .unwrap();
    assert!(
        preview_bitrate(&planned) < preview_bitrate(&base),
        "converted audio overhead must reduce the target-size video budget"
    );
    assert_ne!(
        preview_bitrate(&planned),
        preview_bitrate(&added),
        "converted audio must be planned by encoded overhead, not source packet size"
    );
    let manager = JobManager::new(root.0.join("logs"));
    let started = manager.start_encode(converted).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    std::fs::write(
        root.0.join("snapshot.json"),
        serde_json::to_vec_pretty(&completed).unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    let metadata = probe(&output, &["-show_streams"]).await;
    assert_eq!(metadata["streams"][4]["codec_name"], "aac");
    assert_eq!(metadata["streams"][4]["channels"], 2);
    let before = decoded_bounds(&external, 0).await;
    let after = decoded_bounds(&output, 4).await;
    assert!((before.0 - after.0).abs() <= 0.002 && (before.1 - after.1).abs() <= 0.022);
    assert!(
        std::fs::metadata(&output).unwrap().len() <= 1_258_291,
        "two-pass target-size output exceeded its 1 MiB target by over 20%"
    );
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg, FFprobe and standalone x264"]
async fn mp4_rejects_copied_external_pcm_but_accepts_aac_conversion() {
    let root = Fixture::new();
    let (primary, _) = fixtures(&root.0).await;
    let pcm_external = pcm_source(&root.0, "pcm-zero.mka", "0").await;
    let output = root.0.join("converted.mp4");
    let mut mixed = request(&primary, &pcm_external, &output);
    mixed.source.stream_indices = vec![0];
    mixed.settings.subtitles.clear();
    mixed.settings.external_tracks = vec![ExternalTrack {
        offset_milliseconds: 0,
        subtitle_mode: None,
        title: None,
        language: None,
        default: None,
        forced: None,
        input_path: pcm_external.to_string_lossy().into(),
        stream_index: 0,
        audio: Some(audio(
            media_core::AudioCodec::Aac,
            media_core::AudioChannels::Stereo,
        )),
    }];
    let mut copied = mixed.clone();
    copied.settings.external_tracks[0].audio = None;
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    assert!(
        media_runtime::preview_encode_plan(copied, cancel.clone())
            .await
            .is_err(),
        "MP4 must reject the original external PCM codec when copied"
    );
    let preview = media_runtime::preview_encode_plan(mixed.clone(), cancel)
        .await
        .unwrap();
    assert!(preview.stages.iter().any(|stage| {
        stage
            .arguments
            .windows(2)
            .any(|pair| pair[0] == "-c:1" && pair[1] == "aac")
    }));
    let manager = JobManager::new(root.0.join("logs"));
    let started = manager.start_encode(mixed).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    std::fs::write(
        root.0.join("snapshot.json"),
        serde_json::to_vec_pretty(&completed).unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    let metadata = probe(&output, &["-show_streams", "-count_frames"]).await;
    assert_eq!(metadata["streams"].as_array().unwrap().len(), 3);
    assert_eq!(metadata["streams"][0]["codec_name"], "h264");
    assert_eq!(metadata["streams"][0]["nb_read_frames"], "48");
    assert_eq!(metadata["streams"][1]["codec_name"], "aac");
    assert_eq!(metadata["streams"][1]["channels"], 2);
    assert_eq!(metadata["streams"][1]["tags"]["language"], "spa");
    assert_eq!(metadata["streams"][2]["codec_type"], "data");
    assert_eq!(metadata["streams"][2]["codec_name"], "bin_data");
    let decoded = pcm(&output, 1).await;
    assert!(tone_power(&decoded, 660.0) > tone_power(&decoded, 440.0) * 20.0);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg and FFprobe"]
async fn delayed_external_aac_rejects_mp4_and_mov_before_video_encoding() {
    let root = Fixture::new();
    let (primary, _) = fixtures(&root.0).await;
    let delayed = pcm_source(&root.0, "pcm-delayed.mka", "0.25").await;
    assert!((decoded_bounds(&delayed, 0).await.0 - 0.25).abs() <= 0.002);
    let sources = [&primary, &delayed];
    let originals = sources.map(|path| Sha256::digest(std::fs::read(path).unwrap()));
    let manager = JobManager::new(root.0.join("logs"));
    for extension in ["mp4", "mov"] {
        let output = root.0.join(format!("delayed.{extension}"));
        let mut mixed = request(&primary, &delayed, &output);
        mixed.source.stream_indices = vec![0];
        mixed.settings.subtitles.clear();
        mixed.settings.external_tracks = vec![ExternalTrack {
            offset_milliseconds: 0,
            subtitle_mode: None,
            title: None,
            language: None,
            default: None,
            forced: None,
            input_path: delayed.to_string_lossy().into(),
            stream_index: 0,
            audio: Some(audio(
                media_core::AudioCodec::Aac,
                media_core::AudioChannels::Stereo,
            )),
        }];
        let (_owner, cancel) = tokio::sync::watch::channel(false);
        let preview = media_runtime::preview_encode_plan(mixed.clone(), cancel)
            .await
            .unwrap_err();
        assert_eq!(
            preview.code, "AUDIO_CONTAINER_UNSUPPORTED",
            "{extension}: {preview:?}"
        );
        match manager.start_encode(mixed).await {
            Ok(started) => {
                let completed = finish(&manager, &started.id).await;
                assert_eq!(
                    completed.state,
                    JobState::Failed,
                    "{extension}: {completed:?}"
                );
                assert_eq!(
                    completed.error.as_ref().unwrap().code,
                    "AUDIO_CONTAINER_UNSUPPORTED"
                );
                assert!(completed.standalone_recovery.is_none());
                assert!(
                    !completed
                        .logs
                        .iter()
                        .any(|line| line.contains("Encoding H.264"))
                );
            }
            Err(error) => assert_eq!(error.code, "AUDIO_CONTAINER_UNSUPPORTED"),
        }
        assert!(!output.exists());
    }
    for (path, hash) in sources.into_iter().zip(originals) {
        assert_eq!(Sha256::digest(std::fs::read(path).unwrap()), hash);
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg, FFprobe and standalone x264"]
async fn delayed_primary_audio_trimmed_to_zero_can_convert_to_mp4_aac() {
    let root = Fixture::new();
    let (picture, _) = fixtures(&root.0).await;
    let delayed = pcm_source(&root.0, "trim-source-audio.mka", "0.25").await;
    let primary = root.0.join("delayed-primary.mkv");
    let mut command = args(&["-v", "error", "-nostdin", "-copyts", "-i"]);
    command.push(picture.as_os_str().to_owned());
    command.push("-i".into());
    command.push(delayed.as_os_str().to_owned());
    command.extend([
        "-map".into(),
        "0:0".into(),
        "-map".into(),
        "1:0".into(),
        "-map_metadata".into(),
        "0".into(),
        "-map_chapters".into(),
        "0".into(),
        "-c".into(),
        "copy".into(),
        "-avoid_negative_ts".into(),
        "disabled".into(),
        primary.as_os_str().to_owned(),
    ]);
    tool("ffmpeg", command).await;
    assert!((decoded_bounds(&primary, 1).await.0 - 0.25).abs() <= 0.002);
    let output = root.0.join("trimmed-primary.mp4");
    let mut mixed = request(&primary, &delayed, &output);
    mixed.source.stream_indices = vec![0, 1];
    mixed.settings.external_tracks.clear();
    mixed.settings.subtitles.clear();
    mixed.settings.trim = Some(media_core::VideoTrim {
        start_frame: 24,
        end_frame_exclusive: 48,
        time: None,
    });
    mixed.settings.audio = vec![media_core::AudioTrackSettings {
        stream_index: 1,
        codec: media_core::AudioCodec::Aac,
        bitrate_kbps: 128,
        channels: media_core::AudioChannels::Stereo,
        gain: None,
    }];
    let manager = JobManager::new(root.0.join("logs"));
    let started = manager.start_encode(mixed).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    std::fs::write(
        root.0.join("snapshot.json"),
        serde_json::to_vec_pretty(&completed).unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    let streams = probe(&output, &["-show_streams", "-count_frames"]).await;
    assert_eq!(streams["streams"][0]["nb_read_frames"], "24");
    assert_eq!(streams["streams"][1]["codec_name"], "aac");
    let (start, end) = decoded_bounds(&output, 1).await;
    assert!(
        start.abs() <= 0.002 && (end - 1.0).abs() <= 0.022,
        "{start}..{end}"
    );
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires packaged FFmpeg and FFprobe"]
async fn invalid_external_audio_settings_fail_before_video_or_publication() {
    use media_core::{AudioChannels as Channels, AudioCodec as Codec};
    let root = Fixture::new();
    let (primary, external) = fixtures(&root.0).await;
    let original = Sha256::digest(std::fs::read(&external).unwrap());
    let manager = JobManager::new(root.0.join("logs"));
    for (label, index, settings) in [
        ("subtitle", 1, audio(Codec::Aac, Channels::Stereo)),
        ("attachment", 2, audio(Codec::Aac, Channels::Stereo)),
        (
            "bitrate",
            0,
            media_core::ExternalAudioSettings {
                bitrate_kbps: 0,
                ..audio(Codec::Opus, Channels::Mono)
            },
        ),
        ("copy-layout", 0, audio(Codec::Copy, Channels::Stereo)),
        ("codec-layout", 0, audio(Codec::Mp3, Channels::Surround51)),
    ] {
        let output = root.0.join(format!("invalid-{label}.mkv"));
        let mut mixed = request(&primary, &external, &output);
        mixed.source.stream_indices = vec![0];
        mixed.settings.subtitles.clear();
        mixed.settings.external_tracks = vec![ExternalTrack {
            offset_milliseconds: 0,
            subtitle_mode: None,
            title: None,
            language: None,
            default: None,
            forced: None,
            input_path: external.to_string_lossy().into(),
            stream_index: index,
            audio: Some(settings),
        }];
        match manager.start_encode(mixed).await {
            Ok(started) => {
                let completed = finish(&manager, &started.id).await;
                assert_eq!(completed.state, JobState::Failed, "{label}: {completed:?}");
                assert!(
                    completed.standalone_recovery.is_none(),
                    "{label}: video work began"
                );
            }
            Err(error) => assert!(
                matches!(
                    error.code.as_str(),
                    "AUDIO_SETTINGS_INVALID" | "EXTERNAL_TRACKS_INVALID"
                ),
                "{label}: unexpected rejection: {error:?}"
            ),
        }
        assert!(!output.exists(), "{label}: invalid output was published");
    }
    let malformed = json!({
        "inputPath": external,
        "streamIndex": 0,
        "audio": {"codec":"notARealCodec", "bitrateKbps":128, "channels":"preserve"}
    });
    assert!(serde_json::from_value::<ExternalTrack>(malformed).is_err());
    assert_eq!(Sha256::digest(std::fs::read(&external).unwrap()), original);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires packaged av1an, SVT-AV1-HDR, FFmpeg, FFprobe and VapourSynth"]
async fn av1an_mixed_tracks_preserve_external_packets_and_primary_video() {
    let root = Fixture::new();
    let (primary, external) = fixtures(&root.0).await;
    let output = root.0.join("chunked.mkv");
    let mut mixed = request(&primary, &external, &output);
    mixed.settings.backend = media_runtime::EncodeBackend::Av1an;
    mixed.settings.subtitles.clear();
    mixed.settings.encoder = VideoEncoder::SvtAv1Hdr;
    mixed.settings.preset = 12;
    mixed.settings.workers = 1;
    mixed.settings.av1an_options = Some(media_core::Av1anOptions {
        split_method: media_core::Av1anSplitMethod::FixedChunks,
        maximum_chunk_frames: 48,
        scene_downscale_height: None,
        ..Default::default()
    });
    let originals = [&primary, &external].map(|path| Sha256::digest(std::fs::read(path).unwrap()));
    let manager = JobManager::new(root.0.join("logs"));
    let started = manager.start_encode(mixed).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    std::fs::write(
        root.0.join("snapshot.json"),
        serde_json::to_vec_pretty(&completed).unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    let metadata = probe(&output, &["-show_streams", "-count_frames"]).await;
    assert_eq!(metadata["streams"][0]["codec_name"], "av1");
    assert_eq!(metadata["streams"][0]["nb_read_frames"], "48");
    assert_eq!(
        packet_evidence(&external, 0).await,
        packet_evidence(&output, 4).await
    );
    assert_eq!(
        packet_evidence(&external, 1).await,
        packet_evidence(&output, 3).await
    );
    for (path, hash) in [&primary, &external].into_iter().zip(originals) {
        assert_eq!(Sha256::digest(std::fs::read(path).unwrap()), hash);
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires packaged av1an, SVT-AV1-HDR, FFmpeg, FFprobe and VapourSynth"]
async fn av1an_svt_hdr_converts_external_audio_with_primary_video() {
    let root = Fixture::new();
    let (primary, external) = fixtures(&root.0).await;
    let output = root.0.join("chunked-converted.mkv");
    let mut mixed = request(&primary, &external, &output);
    mixed.settings.backend = media_runtime::EncodeBackend::Av1an;
    mixed.settings.subtitles.clear();
    mixed.settings.encoder = VideoEncoder::SvtAv1Hdr;
    mixed.settings.preset = 12;
    mixed.settings.workers = 1;
    mixed.settings.av1an_options = Some(media_core::Av1anOptions {
        split_method: media_core::Av1anSplitMethod::FixedChunks,
        maximum_chunk_frames: 48,
        scene_downscale_height: None,
        ..Default::default()
    });
    mixed.settings.external_tracks[2].audio = Some(audio(
        media_core::AudioCodec::Opus,
        media_core::AudioChannels::Mono,
    ));
    let manager = JobManager::new(root.0.join("logs"));
    let started = manager.start_encode(mixed).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    std::fs::write(
        root.0.join("snapshot.json"),
        serde_json::to_vec_pretty(&completed).unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    let metadata = probe(&output, &["-show_streams", "-count_frames"]).await;
    assert_eq!(metadata["streams"][0]["codec_name"], "av1");
    assert_eq!(metadata["streams"][0]["nb_read_frames"], "48");
    assert_eq!(metadata["streams"][4]["codec_name"], "opus");
    assert_eq!(metadata["streams"][4]["channels"], 1);
    assert_eq!(metadata["streams"][4]["tags"]["language"], "spa");
    let decoded = pcm(&output, 4).await;
    assert!(tone_power(&decoded, 660.0) > tone_power(&decoded, 440.0) * 20.0);
    assert_eq!(
        packet_evidence(&external, 1).await,
        packet_evidence(&output, 3).await
    );
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn late_external_audio_uses_packet_end_instead_of_header_duration() {
    let root = Fixture::new();
    let (primary, _) = fixtures(&root.0).await;
    let external = root.0.join("late-audio.m4a");
    let mut command = args(&[
        "-v",
        "error",
        "-nostdin",
        "-copyts",
        "-itsoffset",
        "5",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=880:sample_rate=48000:duration=1",
        "-c:a",
        "aac",
        "-avoid_negative_ts",
        "disabled",
    ]);
    command.push(external.clone().into_os_string());
    tool("ffmpeg", command).await;
    let metadata = probe(&external, &["-show_streams"]).await;
    let start = metadata["streams"][0]["start_time"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    let span = metadata["streams"][0]["duration"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!(start > 4.9 && span < 1.1, "{metadata}");
    let output = root.0.join("late-output.mkv");
    let mut mixed = request(&primary, &external, &output);
    mixed.settings.external_tracks = vec![ExternalTrack {
        offset_milliseconds: 0,
        subtitle_mode: None,
        title: None,
        language: None,
        default: None,
        forced: None,
        audio: None,
        input_path: external.to_string_lossy().into(),
        stream_index: 0,
    }];
    let manager = JobManager::new(root.0.join("logs"));
    let started = manager.start_encode(mixed).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    std::fs::write(
        root.0.join("snapshot.json"),
        serde_json::to_vec_pretty(&completed).unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    let metadata = probe(&output, &["-show_format"]).await;
    assert!(
        metadata["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            > 5.9
    );
    let source_packets = packet_evidence(&external, 0).await;
    let output_packets = packet_evidence(&output, 3).await;
    let output_streams = probe(&output, &["-show_streams"]).await;
    let time_base = output_streams["streams"][3]["time_base"].as_str().unwrap();
    let (numerator, denominator) = time_base.split_once('/').unwrap();
    let half_tick = numerator.parse::<f64>().unwrap() / denominator.parse::<f64>().unwrap() / 2.0;
    let source_packets = source_packets.as_array().unwrap();
    let output_packets = output_packets.as_array().unwrap();
    assert_eq!(source_packets.len(), output_packets.len());
    for (source, copied) in source_packets.iter().zip(output_packets) {
        for field in ["hash", "flags", "size"] {
            assert_eq!(source[field], copied[field], "packet {field}");
        }
        // AAC's 1/48000 clock is rounded onto Matroska's 1/1000 clock.
        // Payloads stay exact; times may differ by half one output tick.
        for field in ["pts", "dts"] {
            let before = source[field].as_str().unwrap().parse::<f64>().unwrap();
            let after = copied[field].as_str().unwrap().parse::<f64>().unwrap();
            assert!((before - after).abs() <= half_tick + 0.000001);
        }
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn negative_external_audio_timestamps_do_not_shift_any_output_track() {
    let root = Fixture::new();
    let (primary, _) = fixtures(&root.0).await;
    let external = root.0.join("early-audio.mka");
    let mut command = args(&[
        "-v",
        "error",
        "-nostdin",
        "-copyts",
        "-itsoffset",
        "-0.25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=880:sample_rate=48000:duration=2",
        "-c:a",
        "flac",
        "-avoid_negative_ts",
        "disabled",
    ]);
    command.push(external.clone().into_os_string());
    tool("ffmpeg", command).await;
    let source_packets = packet_evidence(&external, 0).await;
    assert!(
        source_packets[0]["pts"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            < -0.2
    );
    let output = root.0.join("early-output.mkv");
    let mut mixed = request(&primary, &external, &output);
    mixed.settings.external_tracks = vec![ExternalTrack {
        offset_milliseconds: 0,
        subtitle_mode: None,
        title: None,
        language: None,
        default: None,
        forced: None,
        audio: None,
        input_path: external.to_string_lossy().into(),
        stream_index: 0,
    }];
    let manager = JobManager::new(root.0.join("logs"));
    let started = manager.start_encode(mixed).await.unwrap();
    let completed = finish(&manager, &started.id).await;
    std::fs::write(
        root.0.join("snapshot.json"),
        serde_json::to_vec_pretty(&completed).unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    assert_eq!(source_packets, packet_evidence(&output, 3).await);
    assert_eq!(
        packet_evidence(&primary, 1).await,
        packet_evidence(&output, 1).await
    );
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn stop_reopen_resume_binds_external_bytes_and_preserves_saved_work() {
    let root = Fixture::new();
    let (primary, external) = fixtures(&root.0).await;
    let output = root.0.join("resumed.mkv");
    let history = root.0.join("history");
    let logs = root.0.join("logs");
    let manager = JobManager::open(logs.clone(), history.clone()).await;
    let mut request = request(&primary, &external, &output);
    request.settings.external_tracks[2].audio = Some(audio(
        media_core::AudioCodec::Aac,
        media_core::AudioChannels::Stereo,
    ));
    let started = manager.start_encode(request.clone()).await.unwrap();
    let checkpoint = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == started.id)
                .unwrap();
            if job.standalone_recovery.is_some() {
                break job;
            }
            assert!(!job.state.is_terminal(), "No reusable checkpoint: {job:?}");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    manager.stop_job(started.id.clone()).await.unwrap();
    let stopped = finish(&manager, &started.id).await;
    assert_eq!(stopped.state, JobState::Stopped, "{stopped:?}");
    assert!(!output.exists());
    let workspace = PathBuf::from(&checkpoint.standalone_recovery.unwrap().workspace);
    let before = std::fs::read(workspace.join("manifest.json")).unwrap();
    manager.shutdown().await;
    drop(manager);
    let history_file = history.join("jobs.json");
    let original_history = std::fs::read(&history_file).unwrap();
    let mut saved: Value = serde_json::from_slice(&original_history).unwrap();
    assert_eq!(
        saved["jobs"][0]["encodeSettings"]["externalTracks"][2]["audio"],
        json!({"codec":"aac", "bitrateKbps":128, "channels":"stereo"}),
        "raw history must retain the converted external track settings"
    );
    saved["jobs"][0]["encodeSettings"]["externalTracks"][2]["audio"]["channels"] = "mono".into();
    std::fs::write(&history_file, serde_json::to_vec(&saved).unwrap()).unwrap();
    let altered = JobManager::open(logs.clone(), history.clone()).await;
    altered.ready().await.unwrap();
    assert!(altered.resume_job(started.id.clone()).await.is_err());
    assert_eq!(
        std::fs::read(workspace.join("manifest.json")).unwrap(),
        before
    );
    assert!(!output.exists());
    altered.shutdown().await;
    drop(altered);
    std::fs::write(&history_file, original_history).unwrap();
    let original = std::fs::read(&external).unwrap();
    let mut changed = original.clone();
    let last = changed.len() - 1;
    changed[last] ^= 1;
    std::fs::write(&external, &changed).unwrap();
    let reopened = JobManager::open(logs, history).await;
    reopened.ready().await.unwrap();
    assert_eq!(
        reopened.list_jobs().await[0]
            .encode_settings
            .as_ref()
            .unwrap()
            .external_tracks,
        request.settings.external_tracks,
        "history must retain nested external audio settings"
    );
    assert!(reopened.resume_job(started.id.clone()).await.is_err());
    assert_eq!(
        std::fs::read(workspace.join("manifest.json")).unwrap(),
        before
    );
    assert_eq!(std::fs::read(&external).unwrap(), changed);
    assert!(!output.exists());
    std::fs::write(&external, &original).unwrap();
    reopened.resume_job(started.id.clone()).await.unwrap();
    let completed = finish(&reopened, &started.id).await;
    std::fs::write(
        root.0.join("snapshot.json"),
        serde_json::to_vec_pretty(&completed).unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    let metadata = probe(&output, &["-show_streams"]).await;
    assert_eq!(metadata["streams"][4]["codec_name"], "aac");
    assert_eq!(metadata["streams"][4]["channels"], 2);
    let decoded = pcm(&output, 4).await;
    assert!(tone_power(&decoded, 660.0) > tone_power(&decoded, 440.0) * 20.0);
    assert_eq!(std::fs::read(&external).unwrap(), original);
    assert!(!workspace.exists());
    reopened.shutdown().await;
}
