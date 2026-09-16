//! Opt-in command-preview qualification with an installed av1an runtime.
use media_core::{
    EncodeBackend, EncodeRequest, EncodeSettings, FrameRate, RemuxRequest, TemporalSettings,
    VideoEncoder, VideoTimeTrim, VideoTrim,
};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, standalone SVT-AV1 and av1an"]
async fn processed_av1an_preview_uses_one_prepared_source_and_original_mux() {
    if let Some(resources) = std::env::var_os("JESSES_TEST_TOOL_RESOURCES") {
        media_runtime::configure_bundled_tools(resources.into()).unwrap();
    }
    let directory = std::env::temp_dir().join(format!(
        "jesses-preview-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let input = directory.join("source $ ' 日本語.mkv");
    let output = directory.join("output.mkv");
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
    let fixture = std::process::Command::new(ffmpeg).args([
        "-v", "error", "-n", "-f", "lavfi", "-i", "testsrc2=s=192x112:r=24:d=2", "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v", "ffv1", "-chroma_sample_location", "left", "-level", "3",
    ]).arg(&input).output().unwrap();
    assert!(
        fixture.status.success(),
        "{}",
        String::from_utf8_lossy(&fixture.stderr)
    );
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let request = EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::SvtAv1,
            backend: EncodeBackend::Av1an,
            crf: 30,
            preset: 10,
            trim: Some(VideoTrim {
                start_frame: 0,
                end_frame_exclusive: 0,
                time: Some(VideoTimeTrim {
                    start_milliseconds: 250,
                    end_milliseconds: 1250,
                }),
            }),
            temporal: Some(TemporalSettings {
                frame_rate: Some(FrameRate {
                    numerator: 12,
                    denominator: 1,
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let preview = media_runtime::preview_encode_plan(request, cancel)
        .await
        .unwrap();
    assert_eq!(preview.output_frame_count, "12");
    assert_eq!(preview.output_frame_rate, "12/1");
    let preparation = preview
        .stages
        .iter()
        .find(|stage| stage.label == "Av1an lossless processed source")
        .unwrap();
    assert!(
        preparation
            .arguments
            .iter()
            .any(|arg| arg.contains("trim=start_frame=6:end_frame=30"))
    );
    assert!(
        preparation
            .arguments
            .iter()
            .any(|arg| arg.contains("fps=12/1"))
    );
    assert!(preparation.arguments.iter().any(|arg| arg == "ffv1"));
    let av1an = preview
        .stages
        .iter()
        .find(|stage| stage.label == "Av1an scene/chunk encoding")
        .unwrap();
    assert!(!av1an.arguments.iter().any(|arg| arg == "--ffmpeg"));
    assert!(
        av1an
            .arguments
            .windows(2)
            .any(|pair| pair[0] == "-i" && pair[1].ends_with("prepared.mkv"))
    );
    let original = input.canonicalize().unwrap().to_string_lossy().into_owned();
    let mux = preview
        .stages
        .iter()
        .find(|stage| stage.label == "Selected tracks and metadata: Matroska stage")
        .unwrap();
    assert!(
        mux.arguments
            .windows(2)
            .any(|pair| pair[0] == "-i" && pair[1] == original)
    );
    assert!(!output.exists());
    assert_eq!(before, Sha256::digest(std::fs::read(&input).unwrap()));
    assert_eq!(
        std::fs::read_dir(&directory).unwrap().count(),
        1,
        "Preview leaked an owned intermediate"
    );
    std::fs::remove_file(input).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
