//! Exact source Sample Encodes pixel menus, through real Jesses workflows.
use media_core::{CrfLadderRequest, LadderEncoder, LadderMetric, UtilityRequest, UtilityResult};
use media_runtime::supervisor::{self, CommandSpec};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-ladder-formats-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tool(name: &str) -> PathBuf {
    std::env::var_os(format!("JESSES_{}", name.to_uppercase()))
        .map_or_else(|| PathBuf::from(name), PathBuf::from)
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(Into::into).collect()
}
async fn command(name: &str, arguments: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = watch::channel(false);
    let output = supervisor::run_capture(
        &CommandSpec {
            executable: tool(name),
            args: arguments,
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
async fn source(path: &Path, alpha: bool) {
    let mut values = args(&[
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-filter_threads",
        "1",
        "-f",
        "lavfi",
        "-i",
        if alpha {
            "testsrc2=size=128x128:rate=24:duration=1,format=yuva420p,geq=lum='lum(X,Y)':cb='cb(X,Y)':cr='cr(X,Y)':a='if(lt(X,W/2),32,224)'"
        } else {
            "testsrc2=size=128x128:rate=24:duration=1,format=yuv444p10le"
        },
    ]);
    values.extend(args(&[
        "-vf",
        "setparams=field_mode=prog:color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv",
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-threads",
        "2",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
        "-chroma_sample_location",
        "left",
    ]));
    values.push(path.as_os_str().to_owned());
    command("ffmpeg", values).await;
}

#[tokio::test]
#[ignore = "requires qualified full FFmpeg/FFprobe with all five software wrappers and VMAF"]
async fn all_twenty_seven_source_software_formats_keep_exact_decoded_pixels_and_metrics() {
    let fixture = Fixture::new();
    let input = fixture.0.join("source.mkv");
    let alpha = fixture.0.join("alpha.mkv");
    source(&input, false).await;
    source(&alpha, true).await;
    let original = std::fs::read(&input).unwrap();
    let original_alpha = std::fs::read(&alpha).unwrap();
    let (_owner, cancel) = watch::channel(false);
    let mut completed = 0;
    let six = [
        "yuv420p",
        "yuv422p",
        "yuv444p",
        "yuv420p10le",
        "yuv422p10le",
        "yuv444p10le",
    ];
    for (encoder, preset, formats) in [
        (LadderEncoder::Av1, "12", vec!["yuv420p", "yuv420p10le"]),
        (LadderEncoder::AomAv1, "8", six.to_vec()),
        (LadderEncoder::Hevc, "ultrafast", six.to_vec()),
        (LadderEncoder::H264, "ultrafast", six.to_vec()),
        (
            LadderEncoder::Vp9,
            "8",
            vec![
                "yuv420p",
                "yuv422p",
                "yuva420p",
                "yuv444p",
                "yuv420p10le",
                "yuv422p10le",
                "yuv444p10le",
            ],
        ),
    ] {
        for pixel in formats {
            let samples = fixture.0.join(format!("{encoder:?}-{pixel}"));
            let metric = if encoder == LadderEncoder::H264 && pixel == "yuv444p10le" {
                LadderMetric::Vmaf
            } else if pixel == "yuva420p" {
                LadderMetric::Ssim
            } else {
                LadderMetric::Psnr
            };
            let request = UtilityRequest::CrfLadder(CrfLadderRequest {
                input_path: if pixel == "yuva420p" { &alpha } else { &input }
                    .to_string_lossy()
                    .into_owned(),
                video_stream_index: 0,
                encoder,
                preset: preset.into(),
                pixel_format: pixel.into(),
                crfs: vec![30],
                sample_count: 1,
                sample_seconds: 1.0,
                metric,
                recommendation_threshold: None,
                content_preset: None,
                parameters: vec![],
                vmaf_model: None,
                keep_samples_directory: Some(samples.to_string_lossy().into_owned()),
            });
            let result = media_runtime::run_utility(request.clone(), cancel.clone())
                .await
                .unwrap_or_else(|cause| panic!("{encoder:?} {pixel}: {cause:#?}"));
            let UtilityResult::CrfLadder(result) = result else {
                panic!("ladder result")
            };
            assert_eq!(result.rungs.len(), 1);
            assert!(result.rungs[0].encoded_bytes.parse::<u64>().unwrap() > 0);
            assert_eq!(
                result.samples_directory,
                Some(samples.to_string_lossy().into_owned())
            );
            let reference = samples.join("reference-000.mkv");
            let candidate = samples.join("candidate-30-000.mkv");
            assert!(reference.is_file() && candidate.is_file());
            for path in [&reference, &candidate] {
                let mut values = args(&["-v", "error", "-select_streams", "v:0"]);
                if pixel == "yuva420p" && path == &candidate {
                    values.extend(args(&["-c:v", "libvpx-vp9"]));
                }
                values.extend(args(&[
                    "-show_frames",
                    "-show_entries",
                    "frame=width,height,pix_fmt",
                    "-of",
                    "json",
                ]));
                values.push(path.as_os_str().to_owned());
                let frames: serde_json::Value =
                    serde_json::from_slice(&command("ffprobe", values).await).unwrap();
                let frames = frames["frames"].as_array().unwrap();
                assert_eq!(frames.len(), 24);
                assert!(
                    frames.iter().all(|frame| frame["width"] == 128
                        && frame["height"] == 128
                        && frame["pix_fmt"] == pixel),
                    "{encoder:?} {pixel}: {frames:?}"
                );
            }
            if pixel == "yuva420p" {
                let mut values = args(&["-v", "error", "-c:v", "libvpx-vp9", "-i"]);
                values.push(candidate.as_os_str().to_owned());
                values.extend(args(&[
                    "-vf",
                    "alphaextract",
                    "-frames:v",
                    "1",
                    "-f",
                    "rawvideo",
                    "-",
                ]));
                let plane = command("ffmpeg", values).await;
                assert_eq!(plane.len(), 128 * 128);
                assert!(
                    *plane.iter().min().unwrap() < 100 && *plane.iter().max().unwrap() > 200,
                    "VP9 must retain nonopaque, varying alpha"
                );
            }
            assert!(
                media_runtime::run_utility(request, cancel.clone())
                    .await
                    .is_err(),
                "retained output must not overwrite"
            );
            completed += 1;
            eprintln!("PASS {completed}/27 {encoder:?} {pixel} {metric:?}");
        }
    }
    assert_eq!(completed, 27);
    assert_eq!(std::fs::read(input).unwrap(), original);
    assert_eq!(std::fs::read(alpha).unwrap(), original_alpha);
}
