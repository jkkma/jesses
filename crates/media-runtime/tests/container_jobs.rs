//! Actual-tool multi-source Matroska mapping, integrity and history gates.
use media_runtime::{
    JobManager, JobSnapshot, JobState, MuxRequest, MuxSource, MuxTrack, RemuxRequest,
    supervisor::{CommandSpec, run_capture},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-mux-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Mux fixture retained at {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
async fn tool(name: &str, args: Vec<OsString>) -> Vec<u8> {
    let tools = media_runtime::get_capabilities().await;
    let executable = PathBuf::from(
        tools
            .into_iter()
            .find(|tool| tool.id == name)
            .unwrap()
            .path
            .expect("fixture tool required"),
    );
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(
        &CommandSpec {
            executable,
            args,
            cwd: None,
        },
        cancel,
        8 * 1024 * 1024,
        Duration::from_secs(30),
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
async fn finish(manager: &JobManager, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(40), async {
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
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("mux must finish")
}

async fn fixture(directory: &Path, webm: bool) -> PathBuf {
    let subtitle = directory.join("caption.srt");
    let chapters = directory.join("chapters.txt");
    std::fs::write(&subtitle, "1\n00:00:00,250 --> 00:00:00,850\nFirst & bold <b>cue</b>\n\n2\n00:00:01,100 --> 00:00:01,750\n日本語 second cue\n").unwrap();
    std::fs::write(&chapters, ";FFMETADATA1\ntitle=Container qualification\ncomment=Keep this comment\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1000\ntitle=Opening\n").unwrap();
    let output = directory.join(if webm { "web-source.mkv" } else { "source.mkv" });
    let mut cmd = args(&[
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
        "sine=sample_rate=48000:duration=2",
        "-i",
    ]);
    cmd.push(subtitle.into_os_string());
    cmd.extend(args(&["-f", "ffmetadata", "-i"]));
    cmd.push(chapters.into_os_string());
    cmd.extend(args(&[
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
        "-c:v",
        if webm { "libvpx-vp9" } else { "libx264" },
        "-threads:v",
        "2",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-color_range",
        "tv",
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-c:a",
        if webm { "libopus" } else { "aac" },
        "-c:s",
        "srt",
        "-metadata:s:v",
        "title=Picture",
        "-metadata:s:a",
        "title=Sound",
        "-metadata:s:a",
        "language=jpn",
        "-metadata:s:s",
        "title=Captions",
        "-metadata:s:s",
        "language=eng",
    ]));
    cmd.push(output.as_os_str().to_owned());
    tool("ffmpeg", cmd).await;
    output
}

#[tokio::test]
#[ignore = "requires FFmpeg with libx264/libvpx/libopus and FFprobe"]
async fn containers_keep_order_timing_metadata_chapters_and_convert_text() {
    let directory = Fixture::new();
    for extension in ["mp4", "mov", "webm"] {
        let local = directory.0.join(extension);
        std::fs::create_dir(&local).unwrap();
        let source = fixture(&local, extension == "webm").await;
        let before = (
            Sha256::digest(std::fs::read(&source).unwrap()),
            std::fs::metadata(&source).unwrap().modified().unwrap(),
        );
        let output = local.join(format!("output's 日本語.{extension}"));
        let manager = JobManager::new(local.join("logs"));
        let job = manager
            .start_remux(RemuxRequest {
                input_path: source.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![2, 0, 1],
            })
            .await
            .unwrap();
        let job = finish(&manager, &job.id).await;
        assert_eq!(
            job.state,
            JobState::Succeeded,
            "{extension}: {:?} {:?}",
            job.error,
            job.logs
        );
        let mut cmd = args(&[
            "-v",
            "error",
            "-show_streams",
            "-show_chapters",
            "-show_format",
            "-count_frames",
            "-of",
            "json",
            "-i",
        ]);
        cmd.push(output.into_os_string());
        let probe: serde_json::Value = serde_json::from_slice(&tool("ffprobe", cmd).await).unwrap();
        let streams = probe["streams"].as_array().unwrap();
        assert_eq!(
            streams[0]["codec_name"],
            if extension == "webm" {
                "webvtt"
            } else {
                "mov_text"
            }
        );
        assert_eq!(streams[1]["nb_read_frames"], "48");
        assert_eq!(streams[2]["tags"]["language"], "jpn");
        assert_eq!(probe["chapters"][0]["tags"]["title"], "Opening");
        assert_eq!(probe["format"]["tags"]["title"], "Container qualification");
        assert_eq!(before.0, Sha256::digest(std::fs::read(&source).unwrap()));
        assert_eq!(
            before.1,
            std::fs::metadata(&source).unwrap().modified().unwrap()
        );
        assert!(!std::fs::read_dir(&local).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".jesses-")
        }));
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn multi_source_mp4_keeps_edits_and_incompatible_webm_fails_before_output() {
    let directory = Fixture::new();
    let source = fixture(&directory.0, false).await;
    let manager = JobManager::new(directory.0.join("logs"));
    let output = directory.0.join("multi.mp4");
    let request = MuxRequest {
        sources: vec![MuxSource {
            id: "source".into(),
            input_path: source.to_string_lossy().into_owned(),
        }],
        tracks: [1, 0, 2]
            .into_iter()
            .map(|stream_index| MuxTrack {
                source_id: "source".into(),
                stream_index,
                title: Some(format!("Renamed {stream_index}")),
                language: Some("eng".into()),
                default: Some(true),
                forced: Some(false),
            })
            .collect(),
        metadata_source_id: "source".into(),
        chapters_source_id: Some("source".into()),
        output_path: output.to_string_lossy().into_owned(),
    };
    let job = manager.start_mux(request).await.unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?} {:?}",
        job.error,
        job.logs
    );
    let rejected = directory.0.join("incompatible.webm");
    let job = manager
        .start_remux(RemuxRequest {
            input_path: source.to_string_lossy().into_owned(),
            output_path: rejected.to_string_lossy().into_owned(),
            stream_indices: vec![0, 1],
        })
        .await
        .unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(job.error.unwrap().code, "CONTAINER_INCOMPATIBLE");
    assert!(!rejected.exists());
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn quick_encode_trim_to_mp4_and_convert_imported_mp4_text_back_to_matroska() {
    let directory = Fixture::new();
    let source = fixture(&directory.0, false).await;
    let manager = JobManager::new(directory.0.join("logs"));
    let output = directory.0.join("encoded.mp4");
    let request = media_runtime::EncodeRequest {
        source: RemuxRequest {
            input_path: source.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0, 2],
        },
        settings: media_core::EncodeSettings {
            encoder: media_core::VideoEncoder::X264,
            preset: 0,
            trim: Some(media_core::VideoTrim {
                start_frame: 12,
                end_frame_exclusive: 36,
                time: None,
            }),
            ..Default::default()
        },
    };
    let job = manager.start_encode(request).await.unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?} {:?}",
        job.error,
        job.logs
    );
    let final_output = directory.0.join("imported-text.mkv");
    let request = media_runtime::EncodeRequest {
        source: RemuxRequest {
            input_path: output.to_string_lossy().into_owned(),
            output_path: final_output.to_string_lossy().into_owned(),
            stream_indices: vec![0, 1],
        },
        settings: media_core::EncodeSettings {
            encoder: media_core::VideoEncoder::X264,
            preset: 0,
            subtitles: vec![media_core::SubtitleTrackSettings {
                stream_index: 1,
                mode: media_core::SubtitleMode::SubRip,
            }],
            ..Default::default()
        },
    };
    let job = manager.start_encode(request).await.unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?} {:?}",
        job.error,
        job.logs
    );
    let mut cmd = args(&[
        "-v",
        "error",
        "-show_streams",
        "-count_frames",
        "-of",
        "json",
        "-i",
    ]);
    cmd.push(final_output.into_os_string());
    let probe: serde_json::Value = serde_json::from_slice(&tool("ffprobe", cmd).await).unwrap();
    assert_eq!(probe["streams"][0]["nb_read_frames"], "24");
    assert_eq!(probe["streams"][1]["codec_name"], "subrip");
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone x264"]
async fn fractional_b_frames_keep_the_exact_presentation_clock_in_mp4() {
    let directory = Fixture::new();
    let source = directory.0.join("fractional.mkv");
    let mut cmd = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24000/1001",
        "-frames:v",
        "48",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "libx264",
        "-threads:v",
        "2",
    ]);
    cmd.push(source.as_os_str().to_owned());
    tool("ffmpeg", cmd).await;
    let output = directory.0.join("fractional.mp4");
    let manager = JobManager::new(directory.0.join("logs"));
    let request = media_runtime::EncodeRequest {
        source: RemuxRequest {
            input_path: source.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0],
        },
        settings: media_core::EncodeSettings {
            encoder: media_core::VideoEncoder::X264,
            preset: 5,
            ..Default::default()
        },
    };
    let job = manager.start_encode(request).await.unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?} {:?}",
        job.error,
        job.logs
    );
    let mut cmd = args(&[
        "-v",
        "error",
        "-show_streams",
        "-count_frames",
        "-of",
        "json",
        "-i",
    ]);
    cmd.push(output.into_os_string());
    let probe: serde_json::Value = serde_json::from_slice(&tool("ffprobe", cmd).await).unwrap();
    assert_eq!(probe["streams"][0]["nb_read_frames"], "48");
    assert_eq!(probe["streams"][0]["avg_frame_rate"], "24000/1001");
    assert!(probe["streams"][0]["has_b_frames"].as_u64().unwrap() > 0);
}
