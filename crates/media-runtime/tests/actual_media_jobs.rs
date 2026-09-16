//! Opt-in qualification against user-selected read-only media.
//!
//! The source and new output directory are provided through environment
//! variables so personal filenames never enter the repository.

use media_core::{
    FrameRate, GrainRequest, GrainSource, ImageOutput, ImageRequest, KeyframeCutRequest,
    UtilityRequest,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn capture(program: &str, args: &[String]) -> Vec<u8> {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{program} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn file_sha256(path: &Path) -> String {
    let mut file = fs::File::open(path).unwrap();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    format!("{:x}", hasher.finalize())
}

fn media_summary(ffprobe: &str, path: &Path) -> Value {
    serde_json::from_slice(&capture(
        ffprobe,
        &[
            "-v".into(),
            "error".into(),
            "-show_streams".into(),
            "-show_format".into(),
            "-show_entries".into(),
            "stream=index,codec_name,codec_type,start_time,duration,nb_read_frames:stream_tags=language,title,filename,mimetype:format=duration,size".into(),
            "-of".into(),
            "json".into(),
            text(path),
        ],
    ))
    .unwrap()
}

fn validate_decode(ffmpeg: &str, path: &Path) {
    capture(
        ffmpeg,
        &[
            "-hide_banner".into(),
            "-v".into(),
            "error".into(),
            "-xerror".into(),
            "-err_detect".into(),
            "explode".into(),
            "-nostdin".into(),
            "-i".into(),
            text(path),
            "-map".into(),
            "0:v?".into(),
            "-map".into(),
            "0:a?".into(),
            "-sn".into(),
            "-dn".into(),
            "-f".into(),
            "null".into(),
            "-".into(),
        ],
    );
}

#[tokio::test]
#[ignore = "set JESSES_ACTUAL_MEDIA_SOURCE and JESSES_ACTUAL_MEDIA_OUTPUT; requires FFmpeg, FFprobe, and grav1synth"]
async fn actual_media_utility_and_image_routes_preserve_source_and_publish_receipt() {
    let source = PathBuf::from(env::var_os("JESSES_ACTUAL_MEDIA_SOURCE").unwrap());
    let output = PathBuf::from(env::var_os("JESSES_ACTUAL_MEDIA_OUTPUT").unwrap());
    assert!(source.is_absolute() && source.is_file());
    assert!(output.is_absolute() && !output.exists());
    assert!(output.parent().is_some_and(Path::is_dir));
    fs::create_dir(&output).unwrap();

    let ffmpeg = env::var("JESSES_ACTUAL_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    let ffprobe = env::var("JESSES_ACTUAL_FFPROBE").unwrap_or_else(|_| "ffprobe".into());
    let source_sha256_before = file_sha256(&source);
    let source_size = fs::metadata(&source).unwrap().len();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let mut utility_results = Vec::new();
    let mut image_results = Vec::new();

    let cut = output.join("lossless-keyframe-cut.mkv");
    utility_results.push(
        media_runtime::run_utility(
            UtilityRequest::KeyframeCut(KeyframeCutRequest {
                input_path: text(&source),
                output_path: text(&cut),
                start_seconds: 30.0,
                end_seconds: 40.0,
            }),
            cancel.clone(),
        )
        .await
        .unwrap(),
    );
    validate_decode(&ffmpeg, &cut);

    let grain_source = output.join("grain-source-av1.mkv");
    capture(
        &ffmpeg,
        &[
            "-hide_banner".into(),
            "-v".into(),
            "error".into(),
            "-nostdin".into(),
            "-ss".into(),
            "30".into(),
            "-i".into(),
            text(&source),
            "-t".into(),
            "5".into(),
            "-map".into(),
            "0:v:0".into(),
            "-map".into(),
            "0:a:0".into(),
            "-map".into(),
            "0:s:0".into(),
            "-map_metadata".into(),
            "0".into(),
            "-c:v".into(),
            "libsvtav1".into(),
            "-preset".into(),
            "12".into(),
            "-crf".into(),
            "45".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-c:a".into(),
            "flac".into(),
            "-c:s".into(),
            "copy".into(),
            "-y".into(),
            text(&grain_source),
        ],
    );
    validate_decode(&ffmpeg, &grain_source);
    let grain_source_sha256 = file_sha256(&grain_source);

    let grained = output.join("grain-applied.mkv");
    utility_results.push(
        media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::Apply {
                input_path: text(&grain_source),
                output_path: text(&grained),
                source: GrainSource::PhotonNoise {
                    iso: 400,
                    chroma: true,
                },
            }),
            cancel.clone(),
        )
        .await
        .unwrap(),
    );
    let grain_table = output.join("grain-table.txt");
    utility_results.push(
        media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::Extract {
                input_path: text(&grained),
                output_table_path: text(&grain_table),
            }),
            cancel.clone(),
        )
        .await
        .unwrap(),
    );
    let rewritten = output.join("grain-rewritten.mkv");
    utility_results.push(
        media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::RewriteHeaders {
                input_path: text(&grained),
                output_path: text(&rewritten),
                source: GrainSource::Table {
                    table_path: text(&grain_table),
                },
            }),
            cancel.clone(),
        )
        .await
        .unwrap(),
    );
    let grain_removed = output.join("grain-removed.mkv");
    utility_results.push(
        media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::Remove {
                input_path: text(&rewritten),
                output_path: text(&grain_removed),
            }),
            cancel.clone(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(file_sha256(&grain_source), grain_source_sha256);
    for path in [&grained, &rewritten, &grain_removed] {
        validate_decode(&ffmpeg, path);
    }

    for (format, name, start_frame, frame_count) in [
        (ImageOutput::Png, "real-frame.png", 5, 1),
        (ImageOutput::Jpeg, "real-frame.jpg", 6, 1),
        (ImageOutput::Gif, "real-preview.gif", 7, 4),
    ] {
        image_results.push(
            media_runtime::jobs::run_image_job(
                ImageRequest::Export {
                    input_path: text(&cut),
                    stream_index: 0,
                    start_frame,
                    frame_count,
                    format,
                    output_path: text(&output.join(name)),
                    width: Some(640),
                },
                cancel.clone(),
            )
            .await
            .unwrap(),
        );
    }
    let sequence = output.join("real-png-sequence");
    image_results.push(
        media_runtime::jobs::run_image_job(
            ImageRequest::Export {
                input_path: text(&cut),
                stream_index: 0,
                start_frame: 12,
                frame_count: 4,
                format: ImageOutput::PngSequence,
                output_path: text(&sequence),
                width: Some(640),
            },
            cancel.clone(),
        )
        .await
        .unwrap(),
    );
    let mut frames = fs::read_dir(&sequence)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    frames.sort();
    let roundtrip = output.join("real-sequence-roundtrip.mkv");
    image_results.push(
        media_runtime::jobs::run_image_job(
            ImageRequest::ImportSequence {
                paths: frames.iter().map(|path| text(path)).collect(),
                frame_rate: FrameRate {
                    numerator: 24_000,
                    denominator: 1_001,
                },
                output_path: text(&roundtrip),
            },
            cancel,
        )
        .await
        .unwrap(),
    );
    validate_decode(&ffmpeg, &roundtrip);

    let source_sha256_after = file_sha256(&source);
    assert_eq!(source_sha256_after, source_sha256_before);
    assert_eq!(fs::metadata(&source).unwrap().len(), source_size);
    let source_probe = media_summary(&ffprobe, &source);
    let media = [
        &cut,
        &grain_source,
        &grained,
        &rewritten,
        &grain_removed,
        &roundtrip,
    ]
    .into_iter()
    .map(|path| {
        json!({
            "path": text(path),
            "sha256": file_sha256(path),
            "probe": media_summary(&ffprobe, path),
        })
    })
    .collect::<Vec<_>>();
    let mut validated_files = vec![
        output.join("real-frame.png"),
        output.join("real-frame.jpg"),
        output.join("real-preview.gif"),
        grain_table.clone(),
    ];
    validated_files.extend(frames);
    let validated_files = validated_files
        .into_iter()
        .map(|path| {
            json!({
                "path": text(&path),
                "sizeBytes": fs::metadata(&path).unwrap().len().to_string(),
                "sha256": file_sha256(&path),
            })
        })
        .collect::<Vec<_>>();
    let receipt = json!({
        "schemaVersion": 1,
        "completedUnixSeconds": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        "source": {
            "path": text(&source),
            "sizeBytes": source_size.to_string(),
            "sha256Before": source_sha256_before,
            "sha256After": source_sha256_after,
            "unchanged": true,
            "probe": source_probe,
        },
        "utilityResults": utility_results,
        "imageResults": image_results,
        "validatedMedia": media,
        "validatedFiles": validated_files,
    });
    fs::write(
        output.join("validation-receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
}
