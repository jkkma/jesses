//! Actual source copied-cut orientation is consumed by each AV1AN reader.
use media_core::*;
use media_runtime::JobManager;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
#[path = "common/source_trim_fixture.rs"]
mod fixture;
use fixture::*;
fn dimensions(document: &Value) -> (u64, u64) {
    let video = document["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    (
        video["width"].as_u64().unwrap(),
        video["height"].as_u64().unwrap(),
    )
}
fn first_pixels(path: &Path, raw: bool) -> Vec<u8> {
    let mut a = args(&["-v", "error"]);
    if raw {
        a.push("-noautorotate".into());
    }
    a.push("-i".into());
    a.push(path.into());
    a.extend(args(&[
        "-map",
        "0:v:0",
        "-frames:v",
        "1",
        "-pix_fmt",
        "yuv420p",
        "-f",
        "rawvideo",
        "-",
    ]));
    run("ffmpeg", a)
}
fn cut_path(root: &Path) -> PathBuf {
    let dir = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".source-cut-")
        })
        .unwrap();
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("cut.")
        })
        .unwrap()
}
fn equal_packets(expected: &Value, actual: &Value) {
    for (index, _) in expected["streams"].as_array().unwrap().iter().enumerate() {
        let a = packets(expected, index as u64);
        let b = packets(actual, index as u64);
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b) {
            assert_eq!(a["data_hash"], b["data_hash"]);
            for key in ["pts_time", "dts_time", "duration_time"] {
                if a[key].is_string() && b[key].is_string() {
                    assert!((number(a, key) - number(b, key)).abs() < 0.0021, "{key}");
                } else {
                    assert_eq!(a[key], b[key]);
                }
            }
        }
    }
}
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9/AV1AN, x264 and R79"]
async fn source_rotated_cut_preserves_original_packets_then_rotates_each_chunk_without_preencode() {
    let f = Fixture::new();
    let base = f.0.join("coded.mp4");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=red:s=96x64:r=24:d=4,drawbox=x=0:y=0:w=48:h=32:color=blue:t=fill,drawbox=x=48:y=32:w=48:h=32:color=green:t=fill",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=4",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "0",
        "-g",
        "48",
        "-c:a",
        "aac",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(base.clone().into());
    run("ffmpeg", a);
    let input = f.0.join("original rotated.mp4");
    let mut a = args(&["-v", "error", "-display_rotation:v:0", "90", "-i"]);
    a.push(base.into());
    a.extend(args(&["-map", "0", "-c", "copy"]));
    a.push(input.clone().into());
    run("ffmpeg", a);
    let original = std::fs::read(&input).unwrap();
    let source = probe(&input);
    assert_eq!(dimensions(&source), (96, 64));
    let reference = f.0.join("literal-source-cut.mp4");
    let mut a = args(&["-v", "error", "-ss", "1.000", "-i"]);
    a.push(input.clone().into());
    a.extend(args(&[
        "-t",
        "2.000",
        "-map",
        "0",
        "-c",
        "copy",
        "-avoid_negative_ts",
        "make_zero",
        "-ignore_unknown",
    ]));
    a.push(reference.clone().into());
    run("ffmpeg", a);
    let literal = f.0.join("literal-engine.mkv");
    let mut a = args(&[
        "-y",
        "--keep",
        "--verbose",
        "--encoder",
        "x264",
        "--passes",
        "1",
        "--no-defaults",
        "--concat",
        "ffmpeg",
        "--chunk-method",
        "select",
        "--split-method",
        "none",
        "-x",
        "36",
        "--workers",
        "1",
        "--pix-format",
        "yuv420p",
        "--video-params",
        "--preset ultrafast --crf 0 --threads 1 --fps 24",
        "--audio-params",
        "-an -sn -dn",
        "--temp",
    ]);
    a.push(f.0.join("literal-work").into());
    a.push("-i".into());
    a.push(reference.clone().into());
    a.push("-o".into());
    a.push(literal.clone().into());
    run("av1an", a);
    assert_eq!(dimensions(&probe(&literal)), (64, 96));
    let expected_pixels = first_pixels(&reference, false);
    assert_eq!(first_pixels(&literal, false), expected_pixels);
    assert_ne!(expected_pixels, first_pixels(&reference, true));
    let output = f.0.join("original rotated output.mkv");
    let trim = serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap();
    let mut request = request(&input, &output, trim);
    request.source.stream_indices = vec![0, 1];
    request.settings.encoder = VideoEncoder::X264;
    request.settings.backend = EncodeBackend::Av1an;
    request.settings.crf = 0;
    request.settings.preset = 0;
    request.settings.source_filters = true;
    request.settings.av1an_options = Some(Av1anOptions {
        source_workflow: true,
        chunk_method: Av1anChunkMethod::Select,
        split_method: Av1anSplitMethod::FixedChunks,
        maximum_chunk_frames: 36,
        concat_method: Av1anConcatMethod::Ffmpeg,
        pixel_format: Some(Av1anPixelFormat::Yuv420p),
        ..Default::default()
    });
    let manager = JobManager::new(f.0.join("logs"));
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let held = manager
        .prepare_command_edit(
            CommandEditRequest::Encode {
                request: Box::new(request.clone()),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    let cut = cut_path(&f.0);
    equal_packets(&probe(&reference), &probe(&cut));
    let cut_bytes = std::fs::read(&cut).unwrap();
    assert_eq!(first_pixels(&cut, false), expected_pixels);
    assert_eq!(held.output_path, request.source.output_path);
    assert_eq!(
        held.job.as_ref().unwrap().request.input_path,
        request.source.input_path
    );
    assert!(
        !held.arguments.contains("ffv1"),
        "rotation must not add a preprocessing encode: {}",
        held.arguments
    );
    let CommandEditResult::Encode { job } = manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: held.id,
                arguments: held.arguments,
            },
            cancel,
        )
        .await
        .unwrap()
    else {
        panic!("expected encode")
    };
    let done = terminal(&manager, &job.id).await;
    assert_eq!(
        done.state,
        JobState::Succeeded,
        "{:?} {:?}",
        done.error,
        done.logs
    );
    let actual = probe(&output);
    assert_eq!(dimensions(&actual), (64, 96));
    assert_eq!(first_pixels(&output, false), expected_pixels);
    assert_eq!(std::fs::read(&input).unwrap(), original);
    // The cut lives only until successful cleanup; if retained by recovery it is
    // still exactly the verified stream copy, never an oriented replacement.
    if cut.exists() {
        assert_eq!(std::fs::read(&cut).unwrap(), cut_bytes);
    }
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-artifacts/native-backlog/rotation");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("native-pixel-receipt.json"),serde_json::to_vec_pretty(&json!({"sourceCoded":[96,64],"literalOutput":[64,96],"actualOutput":[64,96],"literalFirstFrameEqualsActual":true,"cutPacketsEqualSourceCommand":true,"originalUnchanged":true,"job":done})).unwrap()).unwrap();
}
