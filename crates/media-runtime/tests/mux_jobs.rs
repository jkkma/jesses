//! Actual-tool multi-source Matroska mapping, integrity and history gates.
use media_runtime::{
    JobManager, JobSnapshot, JobState, MuxRequest, MuxSource, MuxTrack,
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
async fn fixtures(directory: &Path) -> (PathBuf, PathBuf) {
    let a = directory.join("picture's $.mkv");
    let b = directory.join("音声.mkv");
    let chapters = directory.join("chapters.ffmetadata");
    let subtitle = directory.join("captions.srt");
    let font = directory.join("font.ttf");
    std::fs::write(&chapters, b";FFMETADATA1\ntitle=Picture owner\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1000\ntitle=Opening\n").unwrap();
    std::fs::write(&subtitle, b"1\n00:00:00,250 --> 00:00:00,850\nFirst cue\n\n2\n00:00:01,100 --> 00:00:01,700\nSecond cue\n").unwrap();
    std::fs::write(&font, b"synthetic attachment integrity payload").unwrap();
    let mut first = args(&[
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=2",
        "-f",
        "ffmetadata",
        "-i",
    ]);
    first.push(chapters.as_os_str().to_owned());
    first.extend(args(&[
        "-map",
        "0:v",
        "-map_metadata",
        "1",
        "-map_chapters",
        "1",
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-metadata:s:v:0",
        "title=Original picture",
        "-disposition:v:0",
        "default",
    ]));
    first.push(a.as_os_str().to_owned());
    tool("ffmpeg", first).await;
    let mut second = args(&[
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=2",
        "-i",
    ]);
    second.push(subtitle.as_os_str().to_owned());
    second.extend(args(&[
        "-map",
        "0:a",
        "-map",
        "1:s",
        "-c:a",
        "flac",
        "-c:s",
        "srt",
        "-metadata",
        "title=Audio owner",
        "-metadata:s:a:0",
        "language=eng",
        "-metadata:s:a:0",
        "title=Original sound",
        "-attach",
    ]));
    second.push(font.as_os_str().to_owned());
    second.extend(args(&[
        "-metadata:s:t:0",
        "mimetype=application/x-truetype-font",
        "-metadata:s:t:0",
        "filename=font.ttf",
    ]));
    second.push(b.as_os_str().to_owned());
    tool("ffmpeg", second).await;
    (a, b)
}
fn track(id: &str, index: u32) -> MuxTrack {
    MuxTrack {
        source_id: id.into(),
        stream_index: index,
        title: None,
        language: None,
        default: None,
        forced: None,
    }
}
fn request(a: &Path, b: &Path, output: &Path) -> MuxRequest {
    MuxRequest {
        sources: vec![
            MuxSource {
                id: "audio".into(),
                input_path: b.to_string_lossy().into_owned(),
            },
            MuxSource {
                id: "picture".into(),
                input_path: a.to_string_lossy().into_owned(),
            },
        ],
        tracks: vec![
            MuxTrack {
                title: Some("Dub's ${title} 日本語".into()),
                language: Some("spa".into()),
                default: Some(true),
                ..track("audio", 0)
            },
            MuxTrack {
                default: Some(false),
                ..track("picture", 0)
            },
            track("audio", 1),
            track("audio", 2),
        ],
        metadata_source_id: "audio".into(),
        chapters_source_id: Some("picture".into()),
        output_path: output.to_string_lossy().into_owned(),
    }
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

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn independent_sources_track_edits_chapters_and_history_are_preserved() {
    let directory = Fixture::new();
    let (a, b) = fixtures(&directory.0).await;
    let originals = [&a, &b].map(|path| {
        (
            Sha256::digest(std::fs::read(path).unwrap()),
            std::fs::metadata(path).unwrap().modified().unwrap(),
        )
    });
    let request = request(&a, &b, &directory.0.join("combined.mkv"));
    let history = directory.0.join("history");
    let logs = directory.0.join("logs");
    let manager = JobManager::open(logs.clone(), history.clone()).await;
    let job = manager.start_mux(request.clone()).await.unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?} {:?}",
        job.error,
        job.logs
    );
    assert_eq!(job.mux_request.as_ref(), Some(&request));
    assert!(job.encode_settings.is_none());
    assert!(job.recovery.is_none());
    let mut probe = args(&[
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
    probe.push(request.output_path.clone().into());
    let output: serde_json::Value = serde_json::from_slice(&tool("ffprobe", probe).await).unwrap();
    let streams = output["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 4);
    assert_eq!(streams[0]["codec_type"], "audio");
    assert_eq!(streams[0]["tags"]["language"], "spa");
    assert_eq!(streams[0]["tags"]["title"], "Dub's ${title} 日本語");
    assert_eq!(streams[0]["disposition"]["default"], 1);
    assert_eq!(streams[1]["codec_type"], "video");
    assert_eq!(streams[1]["nb_read_frames"], "48");
    assert_eq!(streams[1]["disposition"]["default"], 0);
    assert_eq!(streams[2]["codec_type"], "subtitle");
    assert_eq!(streams[3]["codec_type"], "attachment");
    assert_eq!(output["format"]["tags"]["title"], "Audio owner");
    assert_eq!(output["chapters"][0]["tags"]["title"], "Opening");
    manager.shutdown().await;
    drop(manager);
    let reopened = JobManager::open(logs, history).await;
    reopened.ready().await.unwrap();
    assert_eq!(
        reopened.list_jobs().await[0].mux_request.as_ref(),
        Some(&request)
    );
    reopened.shutdown().await;
    drop(reopened);
    for (path, (hash, modified)) in [&a, &b].into_iter().zip(originals) {
        assert_eq!(Sha256::digest(std::fs::read(path).unwrap()), hash);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            modified
        );
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn rejects_source_collisions_and_cancellation_releases_all_inputs() {
    let directory = Fixture::new();
    let (a, b) = fixtures(&directory.0).await;
    let manager = JobManager::new(directory.0.join("logs"));
    let collision = request(&a, &b, &a);
    let job = manager.start_mux(collision).await.unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.error.unwrap().code, "SOURCE_OUTPUT_COLLISION");
    let output = directory.0.join("canceled.mkv");
    let job = manager.start_mux(request(&a, &b, &output)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !manager
            .list_jobs()
            .await
            .iter()
            .any(|candidate| candidate.id == job.id && candidate.state == JobState::Preparing)
        {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    manager.cancel_job(job.id.clone()).await.unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(job.state, JobState::Canceled);
    assert!(!output.exists());
    manager.shutdown().await;
    std::fs::rename(&a, directory.0.join("released-a.mkv")).unwrap();
    std::fs::rename(&b, directory.0.join("released-b.mkv")).unwrap();
    assert!(!std::fs::read_dir(&directory.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial")
    }));
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn external_subtitles_clear_tags_and_omit_chapters_without_changing_cues() {
    let directory = Fixture::new();
    let (a, b) = fixtures(&directory.0).await;
    let subtitles = directory.0.join("captions.srt");
    let original = std::fs::read(&subtitles).unwrap();
    let mut request = request(&a, &b, &directory.0.join("external.mkv"));
    request.sources.push(MuxSource {
        id: "external".into(),
        input_path: subtitles.to_string_lossy().into_owned(),
    });
    request.tracks[0].title = Some(String::new());
    request.tracks[0].language = Some(String::new());
    request.tracks[2] = MuxTrack {
        title: Some("External captions".into()),
        language: Some("eng".into()),
        forced: Some(true),
        ..track("external", 0)
    };
    request.chapters_source_id = None;
    let manager = JobManager::new(directory.0.join("logs"));
    let job = manager.start_mux(request.clone()).await.unwrap();
    let job = finish(&manager, &job.id).await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?} {:?}",
        job.error,
        job.logs
    );
    let mut probe = args(&[
        "-v",
        "error",
        "-show_streams",
        "-show_chapters",
        "-of",
        "json",
        "-i",
    ]);
    probe.push(request.output_path.clone().into());
    let output: serde_json::Value = serde_json::from_slice(&tool("ffprobe", probe).await).unwrap();
    assert!(output["chapters"].as_array().unwrap().is_empty());
    assert!(output["streams"][0]["tags"].get("title").is_none());
    assert!(output["streams"][0]["tags"].get("language").is_none());
    assert_eq!(output["streams"][2]["tags"]["title"], "External captions");
    assert_eq!(output["streams"][2]["disposition"]["forced"], 1);
    let mut extract = args(&["-v", "error", "-nostdin", "-i"]);
    extract.push(request.output_path.into());
    extract.extend(args(&["-map", "0:2", "-f", "srt", "pipe:1"]));
    let cues = String::from_utf8(tool("ffmpeg", extract).await)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(
        cues.trim(),
        String::from_utf8(original.clone()).unwrap().trim()
    );
    assert_eq!(std::fs::read(subtitles).unwrap(), original);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn cancellation_during_output_verification_removes_reserved_output() {
    let directory = Fixture::new();
    let (a, b) = fixtures(&directory.0).await;
    let output = directory.0.join("cancel-finalize.mkv");
    let manager = JobManager::new(directory.0.join("logs"));
    let job = manager.start_mux(request(&a, &b, &output)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let finalizing = manager
                .list_jobs()
                .await
                .iter()
                .any(|entry| entry.id == job.id && entry.state == JobState::Finalizing);
            if finalizing {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("reach actual output verification");
    manager.cancel_job(job.id.clone()).await.unwrap();
    let terminal = finish(&manager, &job.id).await;
    assert_eq!(terminal.state, JobState::Canceled);
    assert!(!output.exists());
    manager.shutdown().await;
    std::fs::rename(a, directory.0.join("released-picture.mkv")).unwrap();
    std::fs::rename(b, directory.0.join("released-audio.mkv")).unwrap();
    assert!(!std::fs::read_dir(&directory.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial")
    }));
}
