//! Opt-in integration gate using locally synthesized media.
use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn create() -> Self {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("jesses-remux-real-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn finish(manager: &JobManager) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let job = manager.list_jobs().await.remove(0);
            if job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("remux job completed in 30 seconds")
}

#[tokio::test]
#[ignore = "requires real FFmpeg and FFprobe on PATH"]
async fn remux_preserves_selected_order_tags_chapters_attachments_and_source_bytes() {
    let fixture = Fixture::create();
    let input = fixture.0.join("- source's & $ % 日本語.mkv");
    let output = fixture.0.join("- output's & $ % 日本語.mkv");
    let chapter_path = fixture.0.join("chapters.txt");
    let subtitle_path = fixture.0.join("subtitle.srt");
    let attachment_path = fixture.0.join("attached-note.txt");
    std::fs::write(&chapter_path, ";FFMETADATA1\ntitle=Original movie\ncomment=Preserve this comment\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=500\ntitle=Opening\n").unwrap();
    std::fs::write(
        &subtitle_path,
        "1\n00:00:00,000 --> 00:00:00,450\nA subtitle\n",
    )
    .unwrap();
    std::fs::write(
        &attachment_path,
        "Attachment contents preserved by stream copy.",
    )
    .unwrap();
    let ffmpeg = find_executable(&["ffmpeg"])
        .await
        .unwrap()
        .expect("FFmpeg installed");
    // Build native arguments without spawning outside the owned supervisor.
    let mut command = std::process::Command::new(ffmpeg);
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "color=c=orange:s=96x64:r=24",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-i",
        ])
        .arg(&subtitle_path)
        .args(["-f", "ffmetadata", "-i"])
        .arg(&chapter_path)
        .args([
            "-t",
            "0.5",
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
            "ffv1",
            "-c:a",
            "pcm_s16le",
            "-c:s",
            "srt",
            "-metadata:s:a:0",
            "language=jpn",
            "-metadata:s:a:0",
            "title=Original mix",
            "-metadata:s:s:0",
            "language=eng",
            "-disposition:s:0",
            "forced",
            "-attach",
        ])
        .arg(&attachment_path)
        .args(["-metadata:s:t:0", "mimetype=text/plain"])
        .arg(&input);
    let spec = crate::supervisor::CommandSpec {
        executable: command.get_program().into(),
        args: command.get_args().map(Into::into).collect(),
        cwd: None,
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let generated =
        crate::supervisor::run_capture(&spec, cancel, 4 * 1024 * 1024, Duration::from_secs(20))
            .await
            .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let source_bytes = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    let request = RemuxRequest {
        input_path: input.to_string_lossy().into(),
        output_path: output.to_string_lossy().into(),
        stream_indices: vec![1, 0, 2, 3],
    };
    manager.start_remux(request.clone()).await.unwrap();
    let job = finish(&manager).await;
    assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
    let media = crate::probe_media(request.output_path.clone())
        .await
        .unwrap();
    assert_eq!(
        media
            .streams
            .iter()
            .map(|s| s.kind.as_str())
            .collect::<Vec<_>>(),
        ["audio", "video", "subtitle", "attachment"]
    );
    assert_eq!(media.streams[0].language.as_deref(), Some("jpn"));
    assert_eq!(std::fs::read(&input).unwrap(), source_bytes);
    assert!(
        std::fs::metadata(job.log_path.as_ref().unwrap())
            .unwrap()
            .len()
            > 0
    );
    assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial.mkv")
    }));

    let output_bytes = std::fs::read(&output).unwrap();
    manager.start_remux(request).await.unwrap();
    let conflict = finish(&manager).await;
    assert_eq!(conflict.state, JobState::Failed);
    assert_eq!(conflict.error.unwrap().code, "OUTPUT_EXISTS");
    assert_eq!(std::fs::read(output).unwrap(), output_bytes);
    assert_eq!(std::fs::read(input).unwrap(), source_bytes);
    manager.shutdown().await;
}
