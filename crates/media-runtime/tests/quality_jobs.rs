//! Opt-in actual frame comparison, with independent pixel-error arithmetic.
use media_core::{QualityMetric, QualityRequest};
use media_runtime::{
    analyze_quality,
    supervisor::{CommandSpec, run_capture},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
async fn capture(executable: &Path, arguments: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = run_capture(
        &CommandSpec {
            executable: executable.into(),
            args: arguments,
            cwd: None,
        },
        cancel,
        8 * 1024 * 1024,
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
async fn pixels(ffmpeg: &Path, path: &Path) -> Vec<u8> {
    let mut arguments = args(&["-v", "error", "-i"]);
    arguments.push(path.as_os_str().into());
    arguments.extend(args(&[
        "-map",
        "0:0",
        "-vf",
        "trim=start_frame=12:end_frame=36",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "yuv420p",
        "-",
    ]));
    capture(ffmpeg, arguments).await
}
#[tokio::test]
#[ignore = "requires FFmpeg with libvmaf and FFprobe"]
async fn scores_match_pixels_and_reject_invalid_alignment_without_source_changes() {
    let tools = media_runtime::get_capabilities().await;
    let ffmpeg = PathBuf::from(
        tools
            .iter()
            .find(|t| t.id == "ffmpeg")
            .unwrap()
            .path
            .as_ref()
            .unwrap(),
    );
    let directory = std::env::temp_dir().join(format!(
        "jesses-quality-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let reference = directory.join("reference's 视频 $.mkv");
    let candidate = directory.join("candidate.mkv");
    let mut arguments = args(&[
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=192x112:r=24:d=2",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
    ]);
    arguments.push(reference.as_os_str().into());
    capture(&ffmpeg, arguments).await;
    let mut arguments = args(&["-v", "error", "-nostdin", "-i"]);
    arguments.push(reference.as_os_str().into());
    arguments.extend(args(&[
        "-vf",
        "lut=y=val+3",
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
    ]));
    arguments.push(candidate.as_os_str().into());
    capture(&ffmpeg, arguments).await;
    let before = (&reference, &candidate);
    let integrity = [
        (
            Sha256::digest(std::fs::read(before.0).unwrap()),
            std::fs::metadata(before.0).unwrap().modified().unwrap(),
        ),
        (
            Sha256::digest(std::fs::read(before.1).unwrap()),
            std::fs::metadata(before.1).unwrap().modified().unwrap(),
        ),
    ];
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let request = QualityRequest {
        reference_path: reference.to_string_lossy().into_owned(),
        reference_stream_index: 0,
        reference_start_frame: 12,
        candidate_path: candidate.to_string_lossy().into_owned(),
        candidate_stream_index: 0,
        candidate_start_frame: 12,
        frame_count: 24,
        metric: QualityMetric::Psnr,
    };
    let source_pixels = pixels(&ffmpeg, &reference).await;
    let candidate_pixels = pixels(&ffmpeg, &candidate).await;
    assert_eq!(source_pixels.len(), candidate_pixels.len());
    assert_eq!(source_pixels.len(), 192 * 112 * 3 / 2 * 24);
    let mse = source_pixels
        .iter()
        .zip(candidate_pixels)
        .map(|(a, b)| (f64::from(*a) - f64::from(b)).powi(2))
        .sum::<f64>()
        / source_pixels.len() as f64;
    let expected = 10.0 * (255.0_f64.powi(2) / mse).log10();
    for metric in [
        QualityMetric::Psnr,
        QualityMetric::Ssim,
        QualityMetric::Vmaf,
    ] {
        let result = analyze_quality(
            QualityRequest {
                metric,
                ..request.clone()
            },
            cancel.clone(),
        )
        .await
        .unwrap();
        assert_eq!(result.points.len(), 24);
        assert_eq!(result.reference_fingerprint.len(), 64);
        match metric {
            QualityMetric::Psnr => assert!(
                (result.score.unwrap() - expected).abs() < 0.00001,
                "{:?} != {expected}",
                result.score
            ),
            QualityMetric::Ssim => assert!((0.9..1.0).contains(&result.score.unwrap())),
            QualityMetric::Vmaf => {
                assert!((0.0..=100.0).contains(&result.score.unwrap()));
                assert_eq!(result.model.as_deref(), Some("vmaf_v0.6.1"));
            }
        }
    }
    let identical = QualityRequest {
        candidate_path: reference.to_string_lossy().into_owned(),
        ..request.clone()
    };
    assert!(
        analyze_quality(identical.clone(), cancel.clone())
            .await
            .unwrap()
            .score
            .is_none()
    );
    assert_eq!(
        analyze_quality(
            QualityRequest {
                metric: QualityMetric::Ssim,
                ..identical
            },
            cancel.clone()
        )
        .await
        .unwrap()
        .score,
        Some(1.0)
    );
    assert!(
        analyze_quality(
            QualityRequest {
                candidate_start_frame: 36,
                ..request.clone()
            },
            cancel.clone()
        )
        .await
        .unwrap_err()
        .message
        .contains("beyond")
    );
    let (canceled_owner, canceled) = tokio::sync::watch::channel(true);
    assert_eq!(
        analyze_quality(request, canceled).await.unwrap_err().code,
        "ANALYSIS_CANCELLED"
    );
    drop(canceled_owner);
    for (path, (hash, time)) in [&reference, &candidate].into_iter().zip(integrity) {
        assert_eq!(Sha256::digest(std::fs::read(path).unwrap()), hash);
        assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), time);
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(directory).unwrap();
}
