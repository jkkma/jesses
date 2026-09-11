//! Real standalone-encoder gate; run explicitly with FFmpeg, FFprobe and SVT-AV1 on PATH.
use std::{path::Path, process::Stdio, sync::Arc, time::Duration};

use media_core::{EncodeJob, EncodeRequest, JobStatus};
use media_runtime::{JobManager, probe_media};
use tokio::process::Command;

async fn ffmpeg(args: &[&str], output: &Path) {
    let mut command = Command::new("ffmpeg");
    command
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-n"])
        .args(args)
        .arg(output)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let result = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("fixture generation timed out")
        .expect("FFmpeg must be on PATH");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

async fn probe_json(path: &Path) -> serde_json::Value {
    let mut command = Command::new("ffprobe");
    command
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_streams",
            "-show_chapters",
            "-of",
            "json",
            "-i",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn request(input: &Path, output: &Path) -> EncodeRequest {
    EncodeRequest {
        input_path: input.to_string_lossy().into_owned(),
        output_path: output.to_string_lossy().into_owned(),
        crf: 35,
        preset: 12,
        audio_bitrate_kbps: 64,
        audio_channels: None,
    }
}

async fn finished(manager: &JobManager, id: &str) -> EncodeJob {
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let job = manager
                .list()
                .unwrap()
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if !job.status.is_active() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("encoding job did not finish")
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone SvtAv1EncApp on PATH"]
async fn converts_real_media_preserves_tracks_and_does_not_clobber() {
    let temporary = tempfile::tempdir().unwrap();
    let input = temporary.path().join("café 東京 & input.mkv");
    let output = temporary.path().join("café 東京 & output.mkv");
    let subtitle = temporary.path().join("subtitles.srt");
    let attachment = temporary.path().join("attached.txt");
    let metadata = temporary.path().join("chapters.txt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,100 --> 00:00:00,900\nPreserve this subtitle.\n",
    )
    .unwrap();
    std::fs::write(&attachment, "Keep the attachment bytes.").unwrap();
    std::fs::write(&metadata, ";FFMETADATA1\ntitle=Encoding fixture\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1000\ntitle=First chapter\n").unwrap();
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=24:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=660:sample_rate=48000:duration=1",
            "-i",
            subtitle.to_str().unwrap(),
            "-f",
            "ffmetadata",
            "-i",
            metadata.to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:a",
            "-map",
            "3:s",
            "-map_metadata",
            "4",
            "-map_chapters",
            "4",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "pcm_s16le",
            "-c:s",
            "srt",
            "-metadata:s:a:0",
            "language=jpn",
            "-metadata:s:a:1",
            "language=eng",
            "-metadata:s:s:0",
            "language=spa",
            "-disposition:a:0",
            "default",
            "-disposition:a:1",
            "0",
            "-disposition:s:0",
            "forced",
            "-attach",
            attachment.to_str().unwrap(),
            "-metadata:s:t:0",
            "mimetype=text/plain",
            "-metadata:s:t:0",
            "filename=attached.txt",
            "-t",
            "1",
        ],
        &input,
    )
    .await;
    let original = std::fs::read(&input).unwrap();
    let store = temporary.path().join("jobs");
    let manager = Arc::new(JobManager::open(store.clone()).unwrap());
    let job = manager.start(request(&input, &output)).await.unwrap();
    let job = finished(&manager, &job.id).await;
    assert_eq!(job.status, JobStatus::Completed, "{}", job.message);
    let result = probe_media(output.to_string_lossy().into_owned())
        .await
        .unwrap();
    assert_eq!(
        result
            .streams
            .iter()
            .filter(|stream| stream.codec.as_deref() == Some("av1"))
            .count(),
        1
    );
    assert_eq!(
        result
            .streams
            .iter()
            .filter(|stream| stream.codec.as_deref() == Some("opus"))
            .count(),
        2
    );
    assert_eq!(
        result
            .streams
            .iter()
            .filter(|stream| stream.kind == "subtitle")
            .count(),
        1
    );
    assert_eq!(
        result
            .streams
            .iter()
            .filter(|stream| stream.kind == "attachment")
            .count(),
        1
    );
    assert!(
        result
            .streams
            .iter()
            .any(|stream| stream.kind == "audio" && stream.language.as_deref() == Some("jpn"))
    );
    assert!(
        result
            .streams
            .iter()
            .any(|stream| stream.kind == "audio" && stream.language.as_deref() == Some("eng"))
    );
    assert!(
        result
            .streams
            .iter()
            .any(|stream| stream.kind == "subtitle" && stream.language.as_deref() == Some("spa"))
    );
    let document = probe_json(&output).await;
    let streams = document["streams"].as_array().unwrap();
    let video = streams
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    assert_eq!(video["nb_read_frames"], "24");
    let audio: Vec<_> = streams
        .iter()
        .filter(|stream| stream["codec_type"] == "audio")
        .collect();
    assert_eq!(audio[0]["disposition"]["default"], 1);
    assert_eq!(audio[1]["disposition"]["default"], 0);
    let subtitle = streams
        .iter()
        .find(|stream| stream["codec_type"] == "subtitle")
        .unwrap();
    assert_eq!(subtitle["disposition"]["forced"], 1);
    let attachment = streams
        .iter()
        .find(|stream| stream["codec_type"] == "attachment")
        .unwrap();
    assert_eq!(attachment["tags"]["filename"], "attached.txt");
    assert_eq!(attachment["tags"]["mimetype"], "text/plain");
    assert_eq!(document["chapters"][0]["tags"]["title"], "First chapter");
    assert_eq!(document["chapters"][0]["start_time"], "0.000000");
    assert_eq!(document["chapters"][0]["end_time"], "1.000000");
    ffmpeg(
        &[
            "-i",
            output.to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "0:a",
            "-f",
            "null",
        ],
        Path::new("-"),
    )
    .await;
    let published = std::fs::read(&output).unwrap();
    match manager.start(request(&input, &output)).await {
        Err(_) => {}
        Ok(collision) => assert_eq!(
            finished(&manager, &collision.id).await.status,
            JobStatus::Failed
        ),
    }
    assert_eq!(std::fs::read(&output).unwrap(), published);
    assert_eq!(std::fs::read(&input).unwrap(), original);
    manager.wait_idle().await;
    drop(manager);
    let reopened = JobManager::open(store).unwrap();
    assert!(
        reopened
            .list()
            .unwrap()
            .iter()
            .any(|entry| entry.id == job.id && entry.status == JobStatus::Completed)
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe and standalone SvtAv1EncApp on PATH"]
async fn cancellation_never_publishes_partial_media() {
    let temporary = tempfile::tempdir().unwrap();
    let input = temporary.path().join("input.mkv");
    let output = temporary.path().join("cancelled.mkv");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=24:duration=5",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv420p",
        ],
        &input,
    )
    .await;
    let manager = Arc::new(JobManager::open(temporary.path().join("jobs")).unwrap());
    let mut plan = request(&input, &output);
    plan.preset = 0;
    let job = manager.start(plan).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let job = manager
                .list()
                .unwrap()
                .into_iter()
                .find(|entry| entry.id == job.id)
                .unwrap();
            if job.status == JobStatus::Encoding {
                break;
            }
            assert!(job.status.is_active(), "{}", job.message);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    manager.cancel(&job.id).unwrap();
    assert_eq!(
        finished(&manager, &job.id).await.status,
        JobStatus::Cancelled
    );
    assert!(!output.exists());
    assert!(input.exists());
}
