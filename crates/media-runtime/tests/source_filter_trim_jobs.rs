//! Source output-side seek preserves arbitrary-filter state and executes once.
use media_core::*;
use media_runtime::{
    jobs::{JobManager, run_image_job},
    supervisor::{CommandSpec, run_capture},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::watch;

async fn ffmpeg(args: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = watch::channel(false);
    let result = run_capture(
        &CommandSpec {
            executable: std::env::var_os("JESSES_FFMPEG").unwrap().into(),
            args,
            cwd: None,
        },
        cancel,
        4 * 1024 * 1024,
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
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(Into::into).collect()
}
async fn first(path: &Path) -> Vec<u8> {
    let mut a = args(&["-v", "error", "-i"]);
    a.push(path.into());
    a.extend(args(&[
        "-frames:v",
        "1",
        "-pix_fmt",
        "rgb24",
        "-f",
        "rawvideo",
        "-",
    ]));
    ffmpeg(a).await
}
async fn finish(manager: &JobManager, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn effect(graph: &str, path: &Path) -> String {
    let path = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:")
        .replace('\'', "'\\''");
    format!(
        "{graph},signalstats,metadata=mode=print:key=lavfi.signalstats.YAVG:file='{}'",
        path
    )
}
fn single_execution(path: &Path) {
    let text = std::fs::read_to_string(path).unwrap();
    assert_eq!(text.matches("frame:0 ").count(), 1, "{text}");
}

#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg/FFprobe and standalone x264"]
async fn source_output_trim_preserves_state_for_cpu_and_images_and_legacy_stays_separate() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-artifacts/source-filter-trim")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let input = root.join("source.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=96x64:rate=30:duration=3",
        "-c:v",
        "ffv1",
        "-threads",
        "2",
    ]);
    a.push(input.clone().into_os_string());
    ffmpeg(a).await;
    let original = std::fs::read(&input).unwrap();
    let graph = "negate=enable='lt(n,30)'";
    let manager = JobManager::open(root.join("logs"), root.join("history")).await;
    for (mode, strategy) in [
        ("input", Some(VideoSeekStrategy::InputSeek)),
        ("exact", Some(VideoSeekStrategy::DecodeFromStart)),
        ("frames", None),
    ] {
        let trim = VideoTrim {
            start_frame: 30,
            end_frame_exclusive: 60,
            time: strategy.map(|seek_strategy| VideoTimeTrim {
                start_milliseconds: 1000,
                end_milliseconds: 2000,
                seek_strategy,
            }),
        };
        let effects = root.join(format!("{mode}-effects.txt"));
        let output = root.join(format!("{mode}.mkv"));
        let request = EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::X264,
                preset: 0,
                workers: 2,
                crf: 12,
                source_trim: true,
                source_filters: true,
                av1an_filters: vec![effect(graph, &effects)],
                trim: Some(trim),
                ..Default::default()
            },
        };
        let job = manager.start_encode(request).await.unwrap();
        let done = finish(&manager, &job.id).await;
        assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
        single_execution(&effects);
        let mut reference = args(&["-v", "error"]);
        if mode == "input" {
            reference.extend(args(&["-ss", "1"]));
        }
        reference.push("-i".into());
        reference.push(input.clone().into_os_string());
        reference.extend(args(&["-vf", graph]));
        if mode != "input" {
            reference.extend(args(&["-ss", if mode == "frames" { "0.983" } else { "1" }]));
        }
        reference.extend(args(&[
            "-t",
            if mode == "frames" { "1.016" } else { "1" },
            "-frames:v",
            "1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ]));
        let expected = ffmpeg(reference).await;
        let actual = first(&output).await;
        assert_eq!(actual.len(), expected.len());
        let mae = actual
            .iter()
            .zip(&expected)
            .map(|(a, b)| f64::from(a.abs_diff(*b)))
            .sum::<f64>()
            / actual.len() as f64;
        assert!(mae < 8.0, "{mode} MAE {mae}");
        if mode == "exact" {
            let image_effects = root.join("image-effects.txt");
            let image_output = root.join("png");
            let (_owner, cancel) = watch::channel(false);
            let result = run_image_job(
                ImageRequest::Export {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: image_output.to_string_lossy().into_owned(),
                    stream_index: 0,
                    start_frame: 0,
                    frame_count: 0,
                    full_source: true,
                    format: ImageOutput::PngSequence,
                    width: None,
                    pixel_format: None,
                    quality: None,
                    processing: Some(ImageProcessingSettings {
                        source_filters: true,
                        filters: vec![effect(graph, &image_effects)],
                        trim: Some(trim),
                        ..Default::default()
                    }),
                },
                cancel,
            )
            .await
            .unwrap();
            assert_eq!(result.frame_count, 30);
            single_execution(&image_effects);
            assert_eq!(first(&image_output.join("00000001.png")).await, expected);
        }
    }
    // Historical raw API deliberately retains the old filter-local trim.
    let legacy = root.join("legacy.mkv");
    let settings = EncodeSettings {
        encoder: VideoEncoder::X264,
        preset: 0,
        workers: 2,
        crf: 12,
        av1an_filters: vec![graph.into()],
        trim: Some(VideoTrim {
            start_frame: 30,
            end_frame_exclusive: 60,
            time: None,
        }),
        ..Default::default()
    };
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: legacy.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings,
        })
        .await
        .unwrap();
    let done = finish(&manager, &job.id).await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    let actual = first(&legacy).await;
    let source_exact = first(&root.join("exact.mkv")).await;
    let difference = actual
        .iter()
        .zip(source_exact)
        .map(|(a, b)| usize::from(a.abs_diff(b)))
        .sum::<usize>() as f64
        / actual.len() as f64;
    assert!(
        difference > 80.0,
        "legacy must retain its pretrim filter state: {difference}"
    );
    manager.shutdown().await;
    assert_eq!(std::fs::read(&input).unwrap(), original);
    eprintln!(
        "source trim modes3, exact PNG30, stateful single-execution and legacy isolation passed: {}",
        root.display()
    );
}
