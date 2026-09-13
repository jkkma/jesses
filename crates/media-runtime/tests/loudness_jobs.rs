//! Opt-in full-track measurement and actual lossless gain qualification.
use media_core::{
    AudioChannels, AudioCodec, AudioGain, AudioTrackSettings, EncodeRequest, EncodeSettings,
    LoudnessRequest, RemuxRequest, VideoEncoder,
};
use media_runtime::{
    JobManager, JobState, measure_loudness,
    supervisor::{CommandSpec, run_capture},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

async fn capture(executable: &Path, args: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let output = run_capture(
        &CommandSpec {
            executable: executable.into(),
            args,
            cwd: None,
        },
        cancel,
        8 * 1024 * 1024,
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
fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
async fn samples(ffmpeg: &Path, source: &Path, stream: &str) -> Vec<f32> {
    let mut args = strings(&["-v", "error", "-nostdin", "-i"]);
    args.push(source.as_os_str().into());
    args.extend(strings(&["-map", stream, "-f", "f32le", "-"]));
    capture(ffmpeg, args)
        .await
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect()
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, and x264"]
async fn measures_selected_track_applies_flat_gain_and_rejects_stale_source() {
    let tools = media_runtime::get_capabilities().await;
    let ffmpeg = PathBuf::from(
        tools
            .iter()
            .find(|tool| tool.id == "ffmpeg")
            .unwrap()
            .path
            .as_ref()
            .unwrap(),
    );
    let directory = std::env::temp_dir().join(format!(
        "jesses-loudness-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let source = directory.join("source's 视频 $.mkv");
    let mut args = strings(&[
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=64x64:r=24:d=6",
        "-f",
        "lavfi",
        "-i",
        "sine=f=1000:r=48000:d=6",
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=48000:cl=stereo:d=6",
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map",
        "2:a",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
        "-c:a",
        "pcm_s24le",
    ]);
    args.push(source.as_os_str().into());
    capture(&ffmpeg, args).await;
    let before = Sha256::digest(std::fs::read(&source).unwrap());
    let before_time = std::fs::metadata(&source).unwrap().modified().unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let request = LoudnessRequest {
        input_path: source.to_string_lossy().into_owned(),
        stream_index: 1,
        channels: AudioChannels::Preserve,
        target_lufs: -23.0,
        peak_limit_dbfs: -1.0,
    };
    let result = measure_loudness(request.clone(), cancel.clone())
        .await
        .unwrap();
    // FFmpeg's sine source has amplitude 1/8; the independent sample maximum
    // and mean-square level constrain both the filter's peak and LUFS report.
    let original = samples(&ffmpeg, &source, "0:1").await;
    let max = original
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    let peak = 20.0 * f64::from(max).log10();
    assert!((result.true_peak_dbfs.unwrap() - peak).abs() < 0.05);
    assert!((-21.2..-20.8).contains(&result.integrated_lufs.unwrap()));
    assert!((-22..=-18).contains(&result.suggested_gain_tenths_db.unwrap()));
    assert_eq!(result.source_fingerprint.len(), 64);
    let silent = measure_loudness(
        LoudnessRequest {
            stream_index: 2,
            ..request.clone()
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(silent.integrated_lufs.is_none());
    assert!(silent.suggested_gain_tenths_db.is_none());
    let manager = JobManager::open(directory.join("logs"), directory.join("history")).await;
    let destination = directory.join("gained.mkv");
    let submitted = EncodeRequest {
        source: RemuxRequest {
            input_path: source.to_string_lossy().into_owned(),
            output_path: destination.to_string_lossy().into_owned(),
            stream_indices: vec![0, 1],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::X264,
            crf: 23,
            preset: 0,
            audio: vec![AudioTrackSettings {
                stream_index: 1,
                codec: AudioCodec::Flac,
                bitrate_kbps: 128,
                channels: AudioChannels::Preserve,
                gain: Some(AudioGain {
                    tenths_db: -60,
                    source_fingerprint: Some(result.source_fingerprint.clone()),
                }),
            }],
            ..Default::default()
        },
    };
    let started = manager.start_encode(submitted.clone()).await.unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == started.id)
                .unwrap();
            if job.state.is_terminal() {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        finished.state,
        JobState::Succeeded,
        "{:?}\n{:?}",
        finished.error,
        finished.logs
    );
    let gained = samples(&ffmpeg, &destination, "0:1").await;
    assert_eq!(original.len(), gained.len());
    let factor = 10.0_f64.powf(-6.0 / 20.0);
    let maximum_error = original
        .iter()
        .zip(&gained)
        .map(|(a, b)| (f64::from(*b) - f64::from(*a) * factor).abs())
        .fold(0.0, f64::max);
    assert!(maximum_error < 0.000_001, "gain error {maximum_error}");
    let gained_measurement = measure_loudness(
        LoudnessRequest {
            input_path: destination.to_string_lossy().into_owned(),
            ..request.clone()
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(
        (gained_measurement.integrated_lufs.unwrap() - result.integrated_lufs.unwrap() + 6.0).abs()
            <= 0.05
    );
    let mut stale = submitted;
    stale.source.output_path = directory.join("stale.mkv").to_string_lossy().into_owned();
    stale.settings.audio[0]
        .gain
        .as_mut()
        .unwrap()
        .source_fingerprint = Some("0".repeat(64));
    let stale_job = manager.start_encode(stale).await.unwrap();
    let failure = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == stale_job.id)
                .unwrap();
            if job.state.is_terminal() {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(failure.state, JobState::Failed);
    assert_eq!(failure.error.unwrap().code, "SOURCE_CHANGED");
    assert!(!directory.join("stale.mkv").exists());
    let (canceled_owner, canceled) = tokio::sync::watch::channel(true);
    assert_eq!(
        measure_loudness(request, canceled).await.unwrap_err().code,
        "ANALYSIS_CANCELLED"
    );
    drop(canceled_owner);
    manager.shutdown().await;
    drop(manager);
    assert_eq!(Sha256::digest(std::fs::read(&source).unwrap()), before);
    assert_eq!(
        std::fs::metadata(&source).unwrap().modified().unwrap(),
        before_time
    );
    // Only this test's newly created fixture is removed after all owners close.
    std::fs::remove_dir_all(directory).unwrap();
}
