//! Opt-in packet accounting against an independently parsed FFprobe JSON scan.
use media_core::BitrateRequest;
use media_runtime::{
    analyze_bitrate,
    supervisor::{CommandSpec, run_capture},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

async fn capture(executable: PathBuf, args: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(
        &CommandSpec {
            executable,
            args,
            cwd: None,
        },
        cancel,
        4 * 1024 * 1024,
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

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn packet_windows_match_independent_json_and_preserve_sources() {
    let tools = media_runtime::get_capabilities().await;
    let tool = |id: &str| {
        PathBuf::from(
            tools
                .iter()
                .find(|tool| tool.id == id)
                .unwrap()
                .path
                .as_ref()
                .expect("media tool is required"),
        )
    };
    let directory = std::env::temp_dir().join(format!(
        "jesses-bitrate-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let source = directory.join("source's 视频 $.mkv");
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=4",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=4",
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "ffv1",
        "-c:a",
        "pcm_s24le",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(source.as_os_str().to_owned());
    capture(tool("ffmpeg"), args).await;
    let before = Sha256::digest(std::fs::read(&source).unwrap());
    let before_time = std::fs::metadata(&source).unwrap().modified().unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    for stream_index in [0, 1] {
        let request = BitrateRequest {
            input_path: source.to_string_lossy().into_owned(),
            stream_index,
            window_seconds: 1.0,
        };
        let result = analyze_bitrate(request.clone(), cancel.clone())
            .await
            .unwrap();
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-select_streams",
            &stream_index.to_string(),
            "-show_packets",
            "-show_entries",
            "packet=pts_time,duration_time,size",
            "-of",
            "json",
            "-i",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        args.push(source.as_os_str().to_owned());
        let independent: serde_json::Value =
            serde_json::from_slice(&capture(tool("ffprobe"), args).await).unwrap();
        let packets = independent["packets"].as_array().unwrap();
        let mut buckets = [0_u64; 4];
        for packet in packets {
            let time: f64 = packet["pts_time"].as_str().unwrap().parse().unwrap();
            let bytes: u64 = packet["size"].as_str().unwrap().parse().unwrap();
            buckets[time.floor() as usize] += bytes;
        }
        assert_eq!(result.packet_count, packets.len().to_string());
        assert_eq!(result.packet_bytes, buckets.iter().sum::<u64>().to_string());
        assert_eq!(result.points.len(), buckets.len());
        for (point, bytes) in result.points.iter().zip(buckets) {
            assert_eq!(point.packet_bytes, bytes.to_string());
            assert_eq!(point.megabits_per_second, bytes as f64 * 8.0 / 1_000_000.0);
        }
        assert_eq!(result.untimed_packet_count, "0");
        assert!(result.average_megabits_per_second.unwrap() > 0.0);
        assert_eq!(result.source_fingerprint.len(), 64);
        let (owner, canceled) = tokio::sync::watch::channel(true);
        assert_eq!(
            analyze_bitrate(request, canceled).await.unwrap_err().code,
            "ANALYSIS_CANCELLED"
        );
        drop(owner);
    }
    assert_eq!(Sha256::digest(std::fs::read(&source).unwrap()), before);
    assert_eq!(
        std::fs::metadata(&source).unwrap().modified().unwrap(),
        before_time
    );
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    // Only this test's unique directory is removed after source handles close.
    std::fs::remove_dir_all(directory).unwrap();
}
