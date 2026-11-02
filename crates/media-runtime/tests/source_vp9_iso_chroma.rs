//! ISO VP9 omits Matroska chroma siting while preserving verified video packets.
use media_core::*;
use media_runtime::JobManager;
use serde_json::json;
use std::path::PathBuf;
#[path = "common/source_trim_fixture.rs"]
mod fixture;
use fixture::*;
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9/libvpx"]
async fn source_vp9_mp4_movtext_retains_verified_video_chroma_contract() {
    let f = Fixture::new();
    let input = fixture(&f);
    let encoded = f.0.join("verified-vp9.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(input.clone().into());
    a.extend(args(&[
        "-map",
        "0:v",
        "-vf",
        "trim=start_frame=24:end_frame=72,setpts=PTS-STARTPTS",
        "-c:v",
        "libvpx-vp9",
        "-deadline",
        "realtime",
        "-cpu-used",
        "8",
        "-chroma_sample_location",
        "left",
    ]));
    a.push(encoded.clone().into());
    run("ffmpeg", a);
    let reference = f.0.join("source-vp9-movtext.mp4");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(encoded.clone().into());
    a.push("-i".into());
    a.push(input.clone().into());
    a.extend(args(&[
        "-map",
        "0:v",
        "-map",
        "1:s",
        "-c:v",
        "copy",
        "-c:s",
        "mov_text",
        "-chroma_sample_location",
        "left",
        "-movflags",
        "write_colr",
    ]));
    a.push(reference.clone().into());
    run("ffmpeg", a);
    let (before, after) = (probe(&encoded), probe(&reference));
    let (first, second) = (packets(&before, 0), packets(&after, 0));
    assert_eq!(first.len(), second.len());
    for (a, b) in first.iter().zip(second) {
        assert_eq!(a["data_hash"], b["data_hash"]);
        assert_eq!(a["size"], b["size"]);
    }
    let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/migration-validation/source-av1an-trim-20261101");
    std::fs::create_dir_all(&proof).unwrap();
    std::fs::write(
        proof.join("open-vp9-mp4-chroma-packets.json"),
        serde_json::to_vec_pretty(&json!({"verifiedMkv":before,"copiedMp4":after})).unwrap(),
    )
    .unwrap();
    let output = f.0.join("native.mp4");
    let trim = serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap();
    let mut req = request(&input, &output, trim);
    req.source.stream_indices = vec![0, 2];
    req.settings.subtitles = vec![SubtitleTrackSettings {
        stream_index: 2,
        mode: SubtitleMode::MovText,
        burn_in: false,
    }];
    let manager = JobManager::new(f.0.join("logs"));
    let job = manager.start_encode(req).await.unwrap();
    let result = terminal(&manager, &job.id).await;
    manager.shutdown().await;
    std::fs::write(
        proof.join("open-vp9-mp4-native-job.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    assert_eq!(
        result.state,
        JobState::Succeeded,
        "Source VP9/MP4 chroma representation: {:?}",
        result.error
    );
}
