//! Source utilities inspect registered original image packets and measured outputs.
use media_core::*;
#[path = "common/source_trim_fixture.rs"]
mod fixture;
use fixture::*;
use std::path::Path;

async fn imported(f: &Fixture) -> MediaFile {
    imported_at(
        f,
        FrameRate {
            numerator: 2,
            denominator: 1,
        },
    )
    .await
}
async fn imported_at(f: &Fixture, rate: FrameRate) -> MediaFile {
    let folder = f.0.join("ordered 'originals");
    std::fs::create_dir(&folder).unwrap();
    for (index, name) in [
        "frame1.png",
        "frame10.png",
        "frame2.png",
        "frame3.png",
        "frame4.png",
        "frame5.png",
    ]
    .iter()
    .enumerate()
    {
        let mut a = args(&["-v", "error", "-f", "lavfi", "-i"]);
        a.push(format!("testsrc2=s=64x48,noise=alls={}:allf=u", index * 8).into());
        a.extend(args(&["-frames:v", "1", "-threads", "1"]));
        a.push(folder.join(name).into());
        run("ffmpeg", a);
    }
    let cache = f.0.join("cache");
    std::fs::create_dir(&cache).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    media_runtime::import_image_sequence(
        ImageSequenceImportRequest {
            directory_path: folder.to_string_lossy().into_owned(),
            frame_rate: rate,
        },
        cache,
        cancel,
    )
    .await
    .unwrap()
}
fn release(f: &Fixture, media: &MediaFile) -> Result<(), AppError> {
    let Some(MediaSource::ImageSequence {
        sequence_fingerprint,
        ..
    }) = &media.source
    else {
        panic!("sequence")
    };
    media_runtime::release_image_sequence(
        ImageSequenceReleaseRequest {
            input_path: media.path.clone(),
            sequence_fingerprint: sequence_fingerprint.clone(),
        },
        f.0.join("cache"),
    )
}
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg/FFprobe PNG/FFV1"]
async fn registered_sequence_bitrate_uses_original_order_sizes_rate_and_guarded_lifetime() {
    let f = Fixture::new();
    let media = imported(&f).await;
    let request = BitrateRequest {
        input_path: media.path.clone(),
        stream_index: 0,
        window_seconds: 1.,
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = media_runtime::analyze_bitrate(request.clone(), cancel.clone())
        .await
        .unwrap();
    let folder = f.0.join("ordered 'originals");
    let sizes = [
        "frame1.png",
        "frame10.png",
        "frame2.png",
        "frame3.png",
        "frame4.png",
        "frame5.png",
    ]
    .map(|name| std::fs::metadata(folder.join(name)).unwrap().len());
    assert_eq!(result.packet_count, "6");
    assert_eq!(result.packet_bytes, sizes.iter().sum::<u64>().to_string());
    assert_eq!(result.end_seconds, Some(3.));
    assert_eq!(result.points.len(), 3);
    for (point, pair) in result.points.iter().zip(sizes.chunks(2)) {
        assert_eq!(point.packet_bytes, pair.iter().sum::<u64>().to_string());
    }
    assert!(!result.source_fingerprint.is_empty());
    // Pin the fork's successful byte-accounting remux separately from its
    // broken file-only concat timestamps. All original compressed packets survive.
    let literal_folder = f.0.join("literal-originals");
    std::fs::create_dir(&literal_folder).unwrap();
    let mut literal = String::new();
    for name in [
        "frame1.png",
        "frame10.png",
        "frame2.png",
        "frame3.png",
        "frame4.png",
        "frame5.png",
    ] {
        let path = literal_folder.join(name);
        std::fs::copy(folder.join(name), &path).unwrap();
        literal.push_str(&format!("file '{}'\n", path.display()));
    }
    let manifest = f.0.join("literal.concat");
    std::fs::write(&manifest, &literal).unwrap();
    let remux = f.0.join("literal-remux.mkv");
    let source = std::process::Command::new(tool("ffmpeg"))
        .args(["-v", "warning", "-f", "concat", "-safe", "0", "-i"])
        .arg(&manifest)
        .args(["-map", "0:0", "-c", "copy", "-f", "matroska"])
        .arg(&remux)
        .output()
        .unwrap();
    assert!(source.status.success());
    let read = probe(&remux);
    let source_packets = packets(&read, 0);
    assert_eq!(source_packets.len(), 6);
    for (packet, size) in source_packets.iter().zip(sizes) {
        assert_eq!(
            packet["size"].as_str().unwrap().parse::<u64>().unwrap(),
            size
        );
    }
    let receipt = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-artifacts/native-backlog/sequence-source-comparison.json");
    std::fs::write(receipt,serde_json::to_vec_pretty(&serde_json::json!({"literalManifest":literal,"literalRemuxDiagnostics":String::from_utf8_lossy(&source.stderr),"literalRemux":read,"correctedRegisteredRate":result})).unwrap()).unwrap();
    let missing = media_runtime::analyze_bitrate(
        BitrateRequest {
            stream_index: 1,
            ..request.clone()
        },
        cancel.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code, "ANALYSIS_FAILED");
    let (owner, canceled) = tokio::sync::watch::channel(false);
    owner.send_replace(true);
    assert_eq!(
        media_runtime::analyze_bitrate(request.clone(), canceled)
            .await
            .unwrap_err()
            .code,
        "ANALYSIS_CANCELLED"
    );
    assert!(
        !std::fs::read_dir(Path::new(&media.path).parent().unwrap())
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|value| value == "ffconcat"))
    );
    release(&f, &media).unwrap();
    assert!(!Path::new(&media.path).exists());
    let fraction = Fixture::new();
    let fractional = imported_at(
        &fraction,
        FrameRate {
            numerator: 24000,
            denominator: 1001,
        },
    )
    .await;
    let chart = media_runtime::analyze_bitrate(
        BitrateRequest {
            input_path: fractional.path.clone(),
            stream_index: 0,
            window_seconds: 0.1,
        },
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(chart.packet_count, "6");
    assert!((chart.end_seconds.unwrap() - 0.25025).abs() < 0.000002);
    assert_eq!(chart.points.len(), 3);
    assert_eq!(
        chart
            .points
            .iter()
            .map(|point| point.packet_bytes.parse::<u64>().unwrap())
            .sum::<u64>(),
        chart.packet_bytes.parse::<u64>().unwrap()
    );
    release(&fraction, &fractional).unwrap();
    let f = Fixture::new();
    let media = imported(&f).await;
    std::fs::write(f.0.join("ordered 'originals/frame1.png"), b"changed").unwrap();
    let error = media_runtime::analyze_bitrate(
        BitrateRequest {
            input_path: media.path.clone(),
            ..request
        },
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "IMAGE_SEQUENCE_CHANGED");
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg/FFprobe and Unix"]
async fn sequence_scan_cancel_joins_process_and_releases_manifest_and_registration_lease() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let media = imported(&f).await;
    let probe = std::env::var_os("JESSES_FFPROBE").expect("explicit qualified provider");
    let script = f.0.join("slow-ffprobe.sh");
    let marker = f.0.join("started");
    let quote =
        |value: &std::ffi::OsStr| format!("'{}'", value.to_string_lossy().replace('\'', "'\\''"));
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf started > {}\nsleep 10\nexec {} \"$@\"\n",
            quote(marker.as_os_str()),
            quote(&probe)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    // This standalone integration binary runs with --test-threads=1. No other
    // test or background task reads the override while it is changed.
    unsafe {
        std::env::set_var("JESSES_FFPROBE", &script);
    }
    let (owner, cancel) = tokio::sync::watch::channel(false);
    let running = tokio::spawn(media_runtime::analyze_bitrate(
        BitrateRequest {
            input_path: media.path.clone(),
            stream_index: 0,
            window_seconds: 1.,
        },
        cancel,
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(release(&f, &media).unwrap_err().code, "IMAGE_SEQUENCE_BUSY");
    owner.send_replace(true);
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap();
    unsafe {
        std::env::set_var("JESSES_FFPROBE", probe);
    }
    assert_eq!(result.unwrap_err().code, "ANALYSIS_CANCELLED");
    assert!(
        !std::fs::read_dir(Path::new(&media.path).parent().unwrap())
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|value| value == "ffconcat"))
    );
    release(&f, &media).unwrap();
}

fn directory_bytes(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directory_bytes(&path)
            } else {
                std::fs::metadata(path).unwrap().len()
            }
        })
        .sum()
}
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg/libx264 and grav1synth"]
async fn grain_table_and_retained_ladder_report_measured_bytes_only() {
    let f = Fixture::new();
    let input = f.0.join("grain-source.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=128x128:r=24:d=1,noise=alls=8:allf=t+u,setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-pix_fmt",
        "yuv420p",
        "-color_range",
        "tv",
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(input.clone().into());
    run("ffmpeg", a);
    let before = std::fs::read(&input).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let samples = f.0.join("retained samples");
    for keep in [false, true] {
        let result = media_runtime::run_utility(
            UtilityRequest::CrfLadder(CrfLadderRequest {
                input_path: input.to_string_lossy().into_owned(),
                video_stream_index: 0,
                encoder: LadderEncoder::H264,
                preset: "ultrafast".into(),
                pixel_format: "yuv420p".into(),
                crfs: vec![30],
                sample_count: 1,
                sample_seconds: 1.,
                metric: LadderMetric::Psnr,
                recommendation_threshold: None,
                content_preset: None,
                parameters: vec![],
                vmaf_model: None,
                keep_samples_directory: keep.then(|| samples.to_string_lossy().into_owned()),
            }),
            cancel.clone(),
        )
        .await
        .unwrap();
        let UtilityResult::CrfLadder(result) = result else {
            panic!("ladder")
        };
        if keep {
            assert_eq!(
                result.samples_size_bytes,
                Some(directory_bytes(&samples).to_string())
            );
            assert!(samples.join("reference-000.mkv").is_file());
            assert!(samples.join("candidate-30-000.mkv").is_file());
            assert!(
                directory_bytes(&samples) > result.rungs[0].encoded_bytes.parse::<u64>().unwrap()
            );
        } else {
            assert!(result.samples_directory.is_none() && result.samples_size_bytes.is_none());
            assert!(!samples.exists());
        }
    }
    let table = f.0.join("grain.tbl");
    let denoised = f.0.join("denoised.mkv");
    let result = media_runtime::run_utility(
        UtilityRequest::Grain(GrainRequest::MeasureSource {
            source_path: input.to_string_lossy().into_owned(),
            output_table_path: table.to_string_lossy().into_owned(),
            denoise_strength: 4,
            denoised_output_path: Some(denoised.to_string_lossy().into_owned()),
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    let UtilityResult::GrainTable(result) = result else {
        panic!("table")
    };
    assert_eq!(
        result.size_bytes,
        Some(std::fs::metadata(&table).unwrap().len().to_string())
    );
    assert!(denoised.exists());
    assert_ne!(
        result.size_bytes,
        Some(
            (std::fs::metadata(&table).unwrap().len()
                + std::fs::metadata(&denoised).unwrap().len())
            .to_string()
        )
    );
    let batch_paths = [f.0.join("batch-one.tbl"), f.0.join("batch-two.tbl")];
    let batch = media_runtime::run_utility_batch(
        UtilityBatchRequest {
            requests: batch_paths
                .iter()
                .map(|path| {
                    UtilityRequest::Grain(GrainRequest::Measure {
                        source_path: input.to_string_lossy().into_owned(),
                        denoised_path: denoised.to_string_lossy().into_owned(),
                        output_table_path: path.to_string_lossy().into_owned(),
                    })
                })
                .collect(),
        },
        cancel,
    )
    .await
    .unwrap();
    assert_eq!(batch.completed, 2);
    for (entry, path) in batch.entries.iter().zip(batch_paths) {
        let Some(UtilityResult::GrainTable(table)) = &entry.result else {
            panic!("{entry:?}")
        };
        assert_eq!(
            table.size_bytes,
            Some(std::fs::metadata(path).unwrap().len().to_string())
        );
    }
    assert_eq!(std::fs::read(input).unwrap(), before);
}
