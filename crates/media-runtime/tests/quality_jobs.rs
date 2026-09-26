//! Opt-in actual frame comparison, with independent pixel-error arithmetic.
use media_core::{
    QualityAlignment, QualityMetric, QualityOptions, QualityRequest, QualityVmafModel,
};
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
        options: None,
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
    let sampled = analyze_quality(
        QualityRequest {
            options: Some(QualityOptions {
                subsample: 5,
                ..QualityOptions::default()
            }),
            metric: QualityMetric::Ssim,
            ..request.clone()
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(sampled.frame_count, 5);
    assert_eq!(
        sampled.points.iter().map(|p| p.frame).collect::<Vec<_>>(),
        vec![0, 5, 10, 15, 20]
    );
    let mut model_scores = Vec::new();
    for model in [
        QualityVmafModel::Standard,
        QualityVmafModel::Negative,
        QualityVmafModel::FourK,
    ] {
        let selected = analyze_quality(
            QualityRequest {
                options: Some(QualityOptions {
                    vmaf_model: model,
                    subsample: 4,
                    ..QualityOptions::default()
                }),
                metric: QualityMetric::Vmaf,
                ..request.clone()
            },
            cancel.clone(),
        )
        .await
        .unwrap();
        assert_eq!(selected.frame_count, 6);
        model_scores.push(selected.score.unwrap());
        assert_eq!(
            selected.model.as_deref(),
            Some(match model {
                QualityVmafModel::Standard => "vmaf_v0.6.1",
                QualityVmafModel::Negative => "vmaf_v0.6.1neg",
                QualityVmafModel::FourK => "vmaf_4k_v0.6.1",
            })
        );
    }
    assert!(
        model_scores[0] != model_scores[1]
            && model_scores[0] != model_scores[2]
            && model_scores[1] != model_scores[2],
        "VMAF model selection did not change the score: {model_scores:?}"
    );
    let padded = directory.join("reference padded.mkv");
    let mut arguments = args(&["-v", "error", "-nostdin", "-i"]);
    arguments.push(reference.as_os_str().into());
    arguments.extend(args(&["-vf", "pad=208:128:8:8:black", "-c:v", "ffv1"]));
    arguments.push(padded.as_os_str().into());
    capture(&ffmpeg, arguments).await;
    let crop_request = QualityRequest {
        reference_path: padded.to_string_lossy().into_owned(),
        metric: QualityMetric::Ssim,
        options: Some(QualityOptions {
            alignment: QualityAlignment::CropReference,
            ..QualityOptions::default()
        }),
        ..request.clone()
    };
    let cropped = analyze_quality(crop_request.clone(), cancel.clone())
        .await
        .unwrap();
    assert!(cropped.score.unwrap() > 0.9);
    assert!(
        cropped.message.contains("reference crop 192×112 at 8,8"),
        "{}",
        cropped.message
    );
    assert!(
        analyze_quality(
            QualityRequest {
                options: None,
                ..crop_request
            },
            cancel.clone()
        )
        .await
        .is_err()
    );
    let large = directory.join("reference large.mkv");
    let mut arguments = args(&["-v", "error", "-nostdin", "-i"]);
    arguments.push(reference.as_os_str().into());
    arguments.extend(args(&["-vf","scale=384:224,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709","-c:v","ffv1","-chroma_sample_location","left"]));
    arguments.push(large.as_os_str().into());
    capture(&ffmpeg, arguments).await;
    let resized = analyze_quality(
        QualityRequest {
            reference_path: large.to_string_lossy().into_owned(),
            metric: QualityMetric::Ssim,
            options: Some(QualityOptions {
                alignment: QualityAlignment::ResizeReference,
                ..QualityOptions::default()
            }),
            ..request.clone()
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(resized.score.unwrap() > 0.9);
    let large_padded = directory.join("reference large padded.mkv");
    let mut arguments = args(&["-v", "error", "-nostdin", "-i"]);
    arguments.push(large.as_os_str().into());
    arguments.extend(args(&["-vf", "pad=400:240:8:8:black", "-c:v", "ffv1"]));
    arguments.push(large_padded.as_os_str().into());
    capture(&ffmpeg, arguments).await;
    let crop_and_resized = analyze_quality(
        QualityRequest {
            reference_path: large_padded.to_string_lossy().into_owned(),
            metric: QualityMetric::Ssim,
            options: Some(QualityOptions {
                alignment: QualityAlignment::CropAndResizeReference,
                ..QualityOptions::default()
            }),
            ..request.clone()
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(crop_and_resized.score.unwrap() > 0.9);
    assert!(
        crop_and_resized
            .message
            .contains("reference crop 384×224 at 8,8")
    );
    let anamorphic = directory.join("reference anamorphic.mkv");
    let mut arguments = args(&["-v", "error", "-nostdin", "-i"]);
    arguments.push(reference.as_os_str().into());
    arguments.extend(args(&["-vf","scale=144:112,setsar=4/3,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709","-c:v","ffv1","-chroma_sample_location","left"]));
    arguments.push(anamorphic.as_os_str().into());
    capture(&ffmpeg, arguments).await;
    let desqueezed = analyze_quality(
        QualityRequest {
            reference_path: anamorphic.to_string_lossy().into_owned(),
            metric: QualityMetric::Ssim,
            ..request.clone()
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(desqueezed.score.unwrap() > 0.8);
    let retimed = directory.join("candidate retimed.mkv");
    let mut arguments = args(&["-v", "error", "-nostdin", "-i"]);
    arguments.push(candidate.as_os_str().into());
    arguments.extend(args(&["-vf","setpts=2*PTS,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709","-fps_mode","passthrough","-c:v","ffv1","-chroma_sample_location","left"]));
    arguments.push(retimed.as_os_str().into());
    capture(&ffmpeg, arguments).await;
    let timed = QualityRequest {
        candidate_path: retimed.to_string_lossy().into_owned(),
        metric: QualityMetric::Ssim,
        ..request.clone()
    };
    let timing_error = analyze_quality(timed.clone(), cancel.clone())
        .await
        .unwrap_err();
    assert!(
        timing_error.message.contains("timing differs"),
        "{}",
        timing_error.message
    );
    let fixed = analyze_quality(
        QualityRequest {
            options: Some(QualityOptions {
                fix_frame_rate: true,
                ..QualityOptions::default()
            }),
            ..timed
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(fixed.frame_count, 24);
    assert!(fixed.message.contains("paired by frame number"));
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
    for path in [&padded, &large, &large_padded, &anamorphic, &retimed] {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(directory).unwrap();
}
