//! Opt-in native gate: `cargo test -p media-runtime --test audio_jobs -- --include-ignored`.
//! All sources are synthesized locally; no user media is read or modified.
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_runtime::{
    AudioChannels, AudioCodec, AudioTrackSettings, BatchEncodeRequest, EncodeBackend,
    EncodeRequest, EncodeSettings, JobManager, JobSnapshot, JobState, RemuxRequest, VideoEncoder,
    supervisor::{CommandSpec, run_capture},
};
use serde_json::{Value, json};

struct Fixture(PathBuf);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jesses-audio-{}-{nonce}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Every file is owned exclusively by this newly created test fixture.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn command(executable: impl AsRef<std::ffi::OsStr>) -> Command {
    // This builder only collects arguments. All native spawns below use the
    // owned supervisor and its explicit Windows inherited-handle list.
    Command::new(executable)
}

async fn output(command: &mut Command) -> Vec<u8> {
    static TOOLS: tokio::sync::OnceCell<Vec<media_runtime::ToolInfo>> =
        tokio::sync::OnceCell::const_new();
    let program = Path::new(command.get_program());
    let executable = if program.is_absolute() {
        program.to_path_buf()
    } else {
        let tools = TOOLS
            .get_or_init(|| Box::pin(media_runtime::get_capabilities()))
            .await;
        let tool = tools
            .iter()
            .find(|tool| tool.id == program.to_string_lossy())
            .unwrap();
        assert!(
            tool.available,
            "Required fixture tool is unavailable: {tool:?}"
        );
        PathBuf::from(tool.path.as_ref().unwrap())
    };
    let spec = CommandSpec {
        executable,
        args: command.get_args().map(std::ffi::OsString::from).collect(),
        cwd: command.get_current_dir().map(Path::to_path_buf),
    };
    assert_eq!(command.get_envs().count(), 0);
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = Box::pin(run_capture(
        &spec,
        cancel,
        8 * 1024 * 1024,
        Duration::from_secs(45),
    ))
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "Native tool failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

async fn synthesize(input: &Path, seconds: u32) {
    let parent = input.parent().unwrap();
    let subtitle = parent.join("captions.srt");
    let attachment = parent.join("font.ttf");
    let chapters = parent.join("chapters.txt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,100 --> 00:00:01,000\nAudio fixture caption\n",
    )
    .unwrap();
    std::fs::write(&attachment, b"owned font attachment fixture").unwrap();
    std::fs::write(&chapters, ";FFMETADATA1\ntitle=Owned audio fixture\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1000\ntitle=Opening\n").unwrap();
    output(command("ffmpeg").args(["-hide_banner","-v","error","-y",
        "-f","lavfi","-i","testsrc2=size=64x64:rate=24000/1001:duration=1.5015",
        "-f","lavfi","-i", &format!("sine=frequency=440:sample_rate=44100:duration={seconds}"),
        "-f","lavfi","-i", &format!("sine=frequency=880:sample_rate=48000:duration={seconds}"),
        "-f","lavfi","-i", &format!("sine=frequency=220:sample_rate=48000:duration={seconds}"),
        "-i"]).arg(&subtitle).args(["-f","ffmetadata","-i"]).arg(&chapters)
        .args(["-filter_complex","[1:a]asetpts=PTS+0.012/TB[a1];[2:a]pan=5.1|FL=c0|FR=0.7*c0|FC=0.5*c0|LFE=0.1*c0|BL=0.3*c0|BR=0.2*c0[a2]",
        "-map","0:v","-map","[a1]","-map","[a2]","-map","3:a","-map","4:s",
        "-map_metadata","5","-map_chapters","5","-c:v","ffv1","-pix_fmt","yuv420p",
        "-vf","setsar=1,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-color_range","tv","-colorspace","bt709","-color_trc","bt709","-color_primaries","bt709","-chroma_sample_location","left",
        "-c:a","flac","-c:s","srt",
        "-metadata:s:a:0","language=jpn","-metadata:s:a:0","title=Delayed original",
        "-metadata:s:a:0","BPS=705600","-disposition:a:0","original",
        "-metadata:s:a:1","language=eng","-metadata:s:a:1","title=Surround mix","-disposition:a:1","default",
        "-metadata:s:a:2","title=Copied audio","-metadata:s:s:0","language=eng","-disposition:s:0","forced",
        "-attach"]).arg(attachment).args(["-metadata:s:t:0","mimetype=application/x-truetype-font","-avoid_negative_ts","disabled"]).arg(input)).await;
}

fn track(stream_index: u32, codec: AudioCodec, channels: AudioChannels) -> AudioTrackSettings {
    AudioTrackSettings {
        stream_index,
        codec,
        bitrate_kbps: if stream_index == 2 { 256 } else { 128 },
        channels,
    }
}

fn request(input: &Path, destination: &Path, audio: Vec<AudioTrackSettings>) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: destination.to_string_lossy().into_owned(),
            stream_indices: vec![2, 0, 3, 1, 4, 5],
        },
        settings: EncodeSettings {
            audio,
            backend: EncodeBackend::Standalone,
            encoder: VideoEncoder::X264,
            crf: 23,
            preset: 0,
            ..Default::default()
        },
    }
}

async fn wait_for(
    manager: &JobManager,
    id: &str,
    ready: impl Fn(&JobSnapshot) -> bool,
) -> JobSnapshot {
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if ready(&job) || job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    if result.is_err() {
        manager.shutdown().await;
    }
    result.expect("native audio job must progress within 90 seconds")
}

async fn probe(path: &Path, args: &[&str]) -> Value {
    serde_json::from_slice(
        &output(
            command("ffprobe")
                .args(["-v", "error", "-of", "json"])
                .args(args)
                .arg(path),
        )
        .await,
    )
    .unwrap()
}

async fn packet_hashes(path: &Path, index: u32) -> Value {
    probe(
        path,
        &[
            "-select_streams",
            &index.to_string(),
            "-show_packets",
            "-show_data_hash",
            "sha256",
            "-show_entries",
            "packet=data_hash",
        ],
    )
    .await["packets"]
        .clone()
}

async fn decoded_timeline(path: &Path, index: u32) -> (f64, u64) {
    let data = probe(
        path,
        &[
            "-select_streams",
            &index.to_string(),
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp_time,nb_samples",
        ],
    )
    .await;
    let frames = data["frames"].as_array().unwrap();
    (
        frames[0]["best_effort_timestamp_time"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
        frames
            .iter()
            .map(|frame| frame["nb_samples"].as_u64().unwrap())
            .sum(),
    )
}

#[tokio::test]
#[ignore = "requires real FFmpeg with native AAC and libopus, FFprobe, and x264"]
async fn reordered_mixed_audio_converts_aac_opus_and_preserves_copied_payloads_metadata_and_timing()
{
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    let destination = fixture.0.join("output.mkv");
    synthesize(&input, 2).await;
    let original_bytes = std::fs::read(&input).unwrap();
    let source = probe(&input, &["-show_streams", "-show_chapters"]).await;
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let submitted = request(
        &input,
        &destination,
        vec![
            track(1, AudioCodec::Aac, AudioChannels::Stereo),
            track(2, AudioCodec::Opus, AudioChannels::Preserve),
        ],
    );
    let started = manager.start_encode(submitted.clone()).await.unwrap();
    let completed = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    assert_eq!(
        completed.encode_settings.as_ref(),
        Some(&submitted.settings)
    );
    assert_eq!(std::fs::read(&input).unwrap(), original_bytes);
    let output = probe(&destination, &["-show_streams", "-show_chapters"]).await;
    let streams = output["streams"].as_array().unwrap();
    assert_eq!(
        streams
            .iter()
            .map(|s| s["codec_type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["audio", "video", "audio", "audio", "subtitle", "attachment"]
    );
    assert_eq!(streams[0]["codec_name"], "opus");
    assert_eq!(streams[0]["channels"], 6);
    assert_eq!(streams[0]["channel_layout"], "5.1");
    assert_eq!(streams[0]["tags"]["title"], "Surround mix");
    assert_eq!(streams[0]["disposition"]["default"], 1);
    assert_eq!(streams[3]["codec_name"], "aac");
    assert_eq!(streams[3]["sample_rate"], "44100");
    assert_eq!(streams[3]["channels"], 2);
    assert_eq!(streams[3]["tags"]["language"], "jpn");
    assert_eq!(streams[3]["tags"]["title"], "Delayed original");
    assert_eq!(streams[3]["disposition"]["original"], 1);
    assert!(streams[3]["tags"].get("BPS").is_none());
    assert_eq!(
        packet_hashes(&input, 3).await,
        packet_hashes(&destination, 2).await
    );
    assert_eq!(
        packet_hashes(&input, 4).await,
        packet_hashes(&destination, 4).await
    );
    assert_eq!(source["chapters"], output["chapters"]);
    assert_eq!(
        streams[5]["extradata_size"],
        source["streams"][5]["extradata_size"]
    );
    let first = decoded_timeline(&input, 1).await;
    let second = decoded_timeline(&destination, 3).await;
    assert!((first.0 - 0.012).abs() < 0.002);
    assert!((first.0 - second.0).abs() <= 0.002);
    assert!((first.1..first.1 + 1024).contains(&second.1));
    assert_eq!(
        decoded_timeline(&input, 2).await.1,
        decoded_timeline(&destination, 0).await.1
    );
    let video = probe(
        &destination,
        &[
            "-select_streams",
            "1",
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp_time",
        ],
    )
    .await;
    let frames = video["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 36);
    for (index, frame) in frames.iter().enumerate() {
        let time: f64 = frame["best_effort_timestamp_time"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((time - index as f64 * 1001.0 / 24000.0).abs() <= 0.002);
    }
    drop(manager);
    let recovered = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    assert_eq!(
        recovered.list_jobs().await[0].encode_settings.as_ref(),
        Some(&submitted.settings)
    );
    recovered.shutdown().await;
}

#[tokio::test]
#[ignore = "requires real FFmpeg with native AAC and libopus, FFprobe, and x264"]
async fn audio_channel_conversion_and_resampling_keep_the_decoded_timeline() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    synthesize(&input, 2).await;
    let cases = [
        (1, AudioCodec::Opus, AudioChannels::Preserve, 1, 48000),
        (1, AudioCodec::Opus, AudioChannels::Stereo, 2, 48000),
        (2, AudioCodec::Opus, AudioChannels::Mono, 1, 48000),
        (2, AudioCodec::Aac, AudioChannels::Preserve, 6, 48000),
        (2, AudioCodec::Aac, AudioChannels::Stereo, 2, 48000),
        (1, AudioCodec::Aac, AudioChannels::Mono, 1, 44100),
    ];
    let manager = JobManager::new(fixture.0.join("logs"));
    for (ordinal, (source_index, codec, channels, count, rate)) in cases.into_iter().enumerate() {
        let destination = fixture.0.join(format!("converted-{ordinal}.mkv"));
        let started = manager
            .start_encode(request(
                &input,
                &destination,
                vec![track(source_index, codec, channels)],
            ))
            .await
            .unwrap();
        let completed = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
        assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
        let index = if source_index == 1 { 3 } else { 0 };
        let document = probe(&destination, &["-show_streams"]).await;
        assert_eq!(document["streams"][index]["channels"], count);
        assert_eq!(document["streams"][index]["sample_rate"], rate.to_string());
        let source = decoded_timeline(&input, source_index).await;
        let output = decoded_timeline(&destination, index as u32).await;
        assert!((source.0 - output.0).abs() <= 0.002);
        let source_rate = if source_index == 1 { 44100.0 } else { 48000.0 };
        let difference = output.1 as f64 - source.1 as f64 * rate as f64 / source_rate;
        assert!(
            difference >= -2.0
                && difference
                    <= if codec == AudioCodec::Aac {
                        1025.0
                    } else {
                        4.0
                    }
        );
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires real FFmpeg with native AAC and libopus, FFprobe, and x264"]
async fn batch_freezes_per_file_audio_and_rejects_tampered_audio_before_admission() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    synthesize(&input, 2).await;
    let manager = JobManager::new(fixture.0.join("logs"));
    let request: BatchEncodeRequest=serde_json::from_value(json!({
        "backend":"standalone","encoder":"x264","workers":2,"crf":23,"preset":0,
        "outputDirectory":fixture.0,
        "inputs":[
            {"inputPath":input,"streamIndices":[2,0,3,1,4,5],"videoStreamIndex":0,"audio":[{"streamIndex":1,"codec":"opus","channels":"stereo","bitrateKbps":96}]},
            {"inputPath":input,"streamIndices":[0,1],"videoStreamIndex":0,"audio":[{"streamIndex":1,"codec":"aac","channels":"mono","bitrateKbps":128}]}
        ]
    })).unwrap();
    let preview = manager.preview_encode_batch(request).await.unwrap();
    assert!(
        preview.items.iter().all(|item| item.error.is_none()),
        "{preview:#?}"
    );
    let requests: Vec<_> = preview
        .items
        .into_iter()
        .map(|item| item.request.unwrap())
        .collect();
    assert_eq!(requests[0].settings.audio[0].bitrate_kbps, 96);
    assert_eq!(requests[1].settings.audio[0].codec, AudioCodec::Aac);
    let mut altered = requests.clone();
    altered[1].settings.audio[0].stream_index = 0;
    assert!(manager.enqueue_encode_batch(altered).await.is_err());
    assert!(manager.list_jobs().await.is_empty());
    let started = manager
        .enqueue_encode_batch(requests.clone())
        .await
        .unwrap();
    for (job, request) in started.iter().zip(&requests) {
        let completed = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
        assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
        assert_eq!(completed.encode_settings.as_ref(), Some(&request.settings));
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires real FFmpeg with native AAC and libopus, FFprobe, and x264"]
async fn cancel_during_audio_finalization_keeps_source_and_removes_owned_partials() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    let destination = fixture.0.join("canceled.mkv");
    synthesize(&input, 120).await;
    let source_len = std::fs::metadata(&input).unwrap().len();
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager
        .start_encode(request(
            &input,
            &destination,
            vec![
                track(1, AudioCodec::Aac, AudioChannels::Stereo),
                track(2, AudioCodec::Opus, AudioChannels::Preserve),
            ],
        ))
        .await
        .unwrap();
    let finalizing = wait_for(&manager, &started.id, |job| {
        job.state == JobState::Finalizing
    })
    .await;
    assert_eq!(finalizing.state, JobState::Finalizing, "{finalizing:#?}");
    manager.cancel_job(started.id.clone()).await.unwrap();
    let completed = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    assert_eq!(completed.state, JobState::Canceled, "{completed:#?}");
    assert!(!destination.exists());
    assert_eq!(std::fs::metadata(&input).unwrap().len(), source_len);
    assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial.")
    }));
}

#[tokio::test]
#[ignore = "requires real FFmpeg with native AAC and libopus, FFprobe, and x264"]
async fn low_sample_rate_aac_validates_last_frame_padding_without_shifting_video() {
    let fixture = Fixture::new();
    let input = fixture.0.join("low-rate.mkv");
    let destination = fixture.0.join("aac.mkv");
    output(command("ffmpeg").args(["-hide_banner","-v","error","-y",
        "-f","lavfi","-i","testsrc2=size=64x64:rate=24000/1001:duration=0.5",
        "-f","lavfi","-i","sine=frequency=440:sample_rate=7350:duration=0.979591837",
        "-map","0:v","-map","1:a","-c:v","ffv1","-pix_fmt","yuv420p",
        "-vf","setsar=1,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-color_range","tv","-colorspace","bt709","-color_trc","bt709","-color_primaries","bt709","-chroma_sample_location","left","-c:a","flac"]).arg(&input)).await;
    let mut audio = track(1, AudioCodec::Aac, AudioChannels::Preserve);
    audio.bitrate_kbps = 32;
    let mut submitted = request(&input, &destination, vec![audio]);
    submitted.source.stream_indices = vec![0, 1];
    let manager = JobManager::new(fixture.0.join("logs"));
    let job = manager.start_encode(submitted).await.unwrap();
    let completed = wait_for(&manager, &job.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    let first = decoded_timeline(&input, 1).await;
    let second = decoded_timeline(&destination, 1).await;
    assert_eq!(first.1, 7200);
    assert_eq!(second.1, 8192);
    assert!((first.0 - second.0).abs() <= 0.002);
}

#[tokio::test]
#[ignore = "requires real FFmpeg with native AAC and libopus, FFprobe, and x264"]
async fn negative_container_timeline_is_rejected_before_encoding_without_rebasing_audio() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    let negative = fixture.0.join("negative.mkv");
    let destination = fixture.0.join("rejected.mkv");
    synthesize(&input, 2).await;
    output(
        command("ffmpeg")
            .args(["-hide_banner", "-v", "error", "-y", "-copyts", "-i"])
            .arg(&input)
            .args([
                "-map",
                "0",
                "-c",
                "copy",
                "-c:a:0",
                "flac",
                "-filter:a:0",
                "asetpts=PTS-0.1/TB",
                "-avoid_negative_ts",
                "disabled",
            ])
            .arg(&negative),
    )
    .await;
    let document = probe(&negative, &["-show_format", "-show_streams"]).await;
    assert!(
        document["format"]["start_time"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            < 0.0
    );
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager
        .start_encode(request(
            &negative,
            &destination,
            vec![track(1, AudioCodec::Opus, AudioChannels::Preserve)],
        ))
        .await
        .unwrap();
    let completed = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    assert_eq!(completed.state, JobState::Failed, "{completed:#?}");
    assert_eq!(completed.error.unwrap().code, "ENCODE_INPUT_UNSUPPORTED");
    assert!(!destination.exists());
    assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial.")
    }));
}

#[tokio::test]
#[ignore = "requires real FFmpeg with native AAC and libopus, FFprobe, and x264"]
async fn bounded_authored_audio_timestamp_jitter_uses_sample_clock_without_sample_loss() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    let jittered = fixture.0.join("jittered.mkv");
    let destination = fixture.0.join("normalized.mkv");
    synthesize(&input, 2).await;
    output(
        command("ffmpeg")
            .args(["-hide_banner", "-v", "error", "-y", "-copyts", "-i"])
            .arg(&input)
            .args([
                "-map",
                "0",
                "-c",
                "copy",
                "-c:a:0",
                "flac",
                "-filter:a:0",
                "asetpts='PTS-if(eq(mod(N,32256),0)*gt(N,0),0.003/TB,0)'",
                "-avoid_negative_ts",
                "disabled",
            ])
            .arg(&jittered),
    )
    .await;
    let frames = probe(
        &jittered,
        &[
            "-select_streams",
            "1",
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp_time,nb_samples",
        ],
    )
    .await;
    let frames = frames["frames"].as_array().unwrap();
    let start: f64 = frames[0]["best_effort_timestamp_time"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let mut samples = 0_u64;
    let mut residual = 0.0_f64;
    for frame in frames {
        let timestamp: f64 = frame["best_effort_timestamp_time"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        residual = residual.max((timestamp - start - samples as f64 / 44_100.0).abs());
        samples += frame["nb_samples"].as_u64().unwrap();
    }
    assert!(
        residual > 0.002 && residual < 0.0031,
        "fixture must exercise authored quantization beyond2ms: {residual}"
    );
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager
        .start_encode(request(
            &jittered,
            &destination,
            vec![track(1, AudioCodec::Opus, AudioChannels::Preserve)],
        ))
        .await
        .unwrap();
    let completed = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    let decoded = decoded_timeline(&destination, 3).await;
    assert!((decoded.0 - start).abs() <= 0.002);
    assert!((decoded.1 as f64 - samples as f64 * 48000.0 / 44100.0).abs() <= 2.0);
    assert!(
        completed
            .logs
            .iter()
            .any(|line| line.contains("Source continuity bound"))
    );
}

#[tokio::test]
#[ignore = "requires real FFmpeg/FFprobe and x264; optional JESSES_TEST_LEGACY_AUDIO_TOOLS=1 checks rejection"]
async fn audio_toolchain_priming_preflight() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    let destination = fixture.0.join("output.mkv");
    synthesize(&input, 2).await;
    let manager = JobManager::new(fixture.0.join("logs"));
    let started = manager
        .start_encode(request(
            &input,
            &destination,
            vec![track(1, AudioCodec::Opus, AudioChannels::Preserve)],
        ))
        .await
        .unwrap();
    let completed = wait_for(&manager, &started.id, |job| job.state.is_terminal()).await;
    manager.shutdown().await;
    if std::env::var("JESSES_TEST_LEGACY_AUDIO_TOOLS").as_deref() == Ok("1") {
        assert_eq!(completed.state, JobState::Failed, "{completed:#?}");
        assert_eq!(
            completed.error.as_ref().unwrap().code,
            "AUDIO_TOOL_UNSUPPORTED"
        );
        assert!(
            !completed
                .logs
                .iter()
                .any(|line| line.contains("progressive CFR frames"))
        );
        assert!(!destination.exists());
    } else {
        assert_eq!(completed.state, JobState::Succeeded, "{completed:#?}");
    }
    assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial.")
    }));
}
