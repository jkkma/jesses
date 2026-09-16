//! Explicit local integration gate: `cargo test -p media-runtime --test real_tools -- --include-ignored`.
//! All media is synthesized locally; no fixtures or network downloads are needed.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use media_runtime::supervisor::{CommandSpec, run_capture};
use media_runtime::{get_capabilities, probe_media};

struct FixtureDirectory(PathBuf);

impl FixtureDirectory {
    fn create() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("jesses-probe-test-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        // This unique directory is created and owned solely by this test.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe on PATH"]
async fn discovers_tools_and_probes_a_synthetic_multistream_file() {
    let capabilities = get_capabilities().await;
    let mut tool_ids = capabilities
        .iter()
        .map(|tool| tool.id.as_str())
        .collect::<Vec<_>>();
    tool_ids.sort_unstable();
    assert_eq!(
        tool_ids,
        [
            "aomenc",
            "av1an",
            "ffmpeg",
            "ffprobe",
            "mkvmerge",
            "svt-av1",
            "svt-av1-5fish",
            "svt-av1-hdr",
            "vpxenc",
            "x264",
            "x265",
        ]
    );
    for id in ["svt-av1-5fish", "svt-av1-hdr"] {
        let fork = capabilities.iter().find(|tool| tool.id == id).unwrap();
        if fork.available {
            let marker = if id == "svt-av1-5fish" {
                "[5fish]"
            } else {
                "SVT-AV1-HDR"
            };
            assert!(
                fork.version
                    .as_deref()
                    .is_some_and(|version| version.contains(marker))
            );
            assert!(fork.path.is_some());
        }
    }
    let ffmpeg = capabilities
        .iter()
        .find(|tool| tool.id == "ffmpeg")
        .unwrap();
    let ffprobe = capabilities
        .iter()
        .find(|tool| tool.id == "ffprobe")
        .unwrap();
    assert!(
        ffmpeg.available,
        "FFmpeg is required for this integration gate: {:?}",
        ffmpeg.detail
    );
    assert!(
        ffprobe.available,
        "FFprobe is required for this integration gate: {:?}",
        ffprobe.detail
    );
    assert!(
        ffprobe
            .version
            .as_ref()
            .is_some_and(|version| version.to_lowercase().contains("ffprobe"))
    );

    let directory = FixtureDirectory::create();
    let path = directory.0.join("- jesses's & $ % 测试.mkv");
    // Build native arguments without spawning outside the owned supervisor.
    let mut command = std::process::Command::new(ffmpeg.path.as_ref().unwrap());
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "color=c=orange:s=96x64:r=24000/1001",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "0.5",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
            "-metadata:s:a:0",
            "language=eng",
            "-metadata:s:a:0",
            "title=Test tone",
        ])
        .arg(&path);
    let spec = CommandSpec {
        executable: command.get_program().into(),
        args: command.get_args().map(Into::into).collect(),
        cwd: None,
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(&spec, cancel, 4 * 1024 * 1024, Duration::from_secs(20))
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "Synthetic fixture creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let before = std::fs::read(&path).unwrap();
    let media = probe_media(path.to_string_lossy().into_owned())
        .await
        .unwrap();
    assert_eq!(media.name, "- jesses's & $ % 测试.mkv");
    assert_eq!(media.size_bytes, before.len().to_string());
    assert!(
        media
            .duration_seconds
            .is_some_and(|duration| (0.45..0.60).contains(&duration))
    );
    assert_eq!(media.streams.len(), 2);
    let video = &media.streams[0];
    assert_eq!(video.index, 0);
    assert_eq!(video.kind, "video");
    assert_eq!(video.codec.as_deref(), Some("ffv1"));
    assert_eq!((video.width, video.height), (Some(96), Some(64)));
    assert_eq!(video.frame_rate.as_deref(), Some("24000/1001"));
    let audio = &media.streams[1];
    assert_eq!(audio.index, 1);
    assert_eq!(audio.kind, "audio");
    assert_eq!(audio.sample_rate, Some(48000));
    assert_eq!(audio.channels, Some(1));
    assert_eq!(audio.language.as_deref(), Some("eng"));
    assert_eq!(audio.title.as_deref(), Some("Test tone"));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "Inspection must preserve the source file"
    );
    assert_eq!(probe_media(media.path.clone()).await.unwrap().id, media.id);

    let corrupt = directory.0.join("corrupt.mkv");
    std::fs::write(&corrupt, b"This is not a media container.").unwrap();
    let error = probe_media(corrupt.to_string_lossy().into_owned())
        .await
        .unwrap_err();
    assert_eq!(error.code, "PROBE_FAILED");
    assert!(error.path.is_some());
}
