//! Read-only import against a real probed source; run with --include-ignored.
use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn imported_sidecar_uses_probed_indices_and_preserves_source_and_recovery() {
    let root = std::env::temp_dir().join(format!(
        "jesses-saved-import-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let input = root.join("source & sample.mkv");
    let output = root.join("destination.mkv");
    let ffmpeg = std::env::var_os("JESSES_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    let made = Command::new(ffmpeg)
        .args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=0.5",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=24:duration=0.5",
            "-map",
            "0:a:0",
            "-map",
            "1:v:0",
            "-c:a",
            "pcm_s16le",
            "-c:v",
            "ffv1",
            "-color_range",
            "tv",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
        ])
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let recovery = root.join("saved");
    std::fs::create_dir(&recovery).unwrap();
    let sentinel = recovery.join("done.json");
    std::fs::write(&sentinel, b"foreign engine data stays untouched").unwrap();
    let sidecar = root.join("saved.json");
    let data = serde_json::to_vec(&serde_json::json!({
        "filePath":input, "fileName":"source & sample.mkv", "tempFolderName":"saved",
        "creationTimestamp":"100", "lastRunTimestamp":"200",
        "args":format!(r#"-r --temp "{}" --log-file "{}" -i "{}" -y --verbose --keep --split-method none -m lsmash -c mkvmerge --chunk-order sequential -e x264 --force -v "--crf 23 --preset ultrafast --threads 0 --tune film" --pix-format yuv420p -a "-c:a copy -sn -dn -map -0:t?" -w 1 -o "{}""#, recovery.display(), recovery.join("av1an.log").display(), input.display(), output.display())
    })).unwrap();
    std::fs::write(&sidecar, &data).unwrap();
    let source = std::fs::read(&input).unwrap();
    let inspected =
        media_runtime::inspect_saved_job_with_media(sidecar.to_string_lossy().into_owned())
            .await
            .unwrap();
    assert!(inspected.compatible, "{}", inspected.message);
    let request = inspected.request.unwrap();
    assert_eq!(request.settings.video_stream_index, 1);
    assert_eq!(request.source.stream_indices, vec![1, 0]);
    assert_eq!(request.settings.encoder, media_core::VideoEncoder::X264);
    assert_eq!(request.settings.crf, 23);
    let options = request.settings.av1an_options.unwrap();
    assert_eq!(options.chunk_method, media_core::Av1anChunkMethod::Lsmash);
    assert_eq!(
        options.concat_method,
        media_core::Av1anConcatMethod::Mkvmerge
    );
    assert_eq!(options.encoder_threads, Some(0));
    assert_eq!(std::fs::read(&input).unwrap(), source);
    assert_eq!(std::fs::read(&sidecar).unwrap(), data);
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"foreign engine data stays untouched"
    );
    assert!(!output.exists());
    assert!(!recovery.join("av1an.log").exists());
    std::fs::remove_dir_all(root).unwrap();
}
