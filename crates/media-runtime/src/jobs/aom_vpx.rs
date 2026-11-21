//! Standalone AOM/VPX drivers consume validated planar video and emit timed IVF.
use std::ffi::OsString;

use media_core::{EncodeSettings, VideoEncoder};

use super::Plan;

fn aom_color(value: u8, kind: &str) -> &'static str {
    match (kind, value) {
        ("primaries", 1) | ("transfer", 1) | ("matrix", 1) => "bt709",
        ("primaries", 5) | ("transfer", 5) | ("matrix", 5) => "bt470bg",
        ("primaries", 6) | ("transfer", 6) | ("matrix", 6) => "bt601",
        ("primaries", 9) => "bt2020",
        ("transfer", 16) => "smpte2084",
        ("matrix", 9) => "bt2020ncl",
        _ => unreachable!("validated SDR color"),
    }
}

fn vpx_color(matrix: u8) -> &'static str {
    match matrix {
        1 => "bt709",
        5 => "bt601",
        6 => "smpte170",
        _ => unreachable!("validated SDR color"),
    }
}

pub(super) fn arguments(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let depth = plan.output_bit_depth().to_string();
    // AV1 Main profile carries both 8-bit and 10-bit 4:2:0. VP9 uses profile 2
    // for 10-bit 4:2:0. Y4M used to make aomenc repair the invalid AV1 profile
    // selection implicitly; raw input requires the correct profile up front.
    let chroma = if plan.output_pixel_format.contains("444") {
        "444"
    } else if plan.output_pixel_format.contains("422") {
        "422"
    } else {
        "420"
    };
    let profile = match (settings.encoder, chroma, plan.output_bit_depth()) {
        (VideoEncoder::AomAv1, "420", _) => 0,
        (VideoEncoder::AomAv1, "444", _) => 1,
        (VideoEncoder::AomAv1, "422", _) => 2,
        (VideoEncoder::VpxStandalone, "420", 8) => 0,
        (VideoEncoder::VpxStandalone, "444", 8) => 1,
        (VideoEncoder::VpxStandalone, "420", 10) => 2,
        (VideoEncoder::VpxStandalone, "444", 10) => 3,
        _ => unreachable!("standalone AOM/VPX encoder"),
    };
    let mut args: Vec<OsString> = vec![
        "--ivf".into(),
        "--output=-".into(),
        "--passes=1".into(),
        "--end-usage=q".into(),
        format!("--cpu-used={}", settings.preset).into(),
        "--row-mt=1".into(),
        format!("--profile={profile}").into(),
        format!("--bit-depth={depth}").into(),
        format!("--input-bit-depth={depth}").into(),
        format!("--fps={}/{}", plan.fps_num, plan.fps_den).into(),
        "--disable-warning-prompt".into(),
    ];
    // Row multithreading spreads rows over the threads the encoder is given.
    if let Some(threads) = plan.machine_encoder_threads() {
        args.push(format!("--threads={threads}").into());
    }
    if let Some(frames) = super::super::encoding_preferences::keyframes(plan, settings) {
        args.push(format!("--kf-max-dist={frames}").into());
    }
    if settings.lossless {
        args.push("--lossless=1".into());
    } else {
        args.push(format!("--cq-level={}", settings.crf).into());
    }
    match settings.encoder {
        VideoEncoder::AomAv1 => {
            args.extend([
                format!(
                    "--color-primaries={}",
                    aom_color(plan.primaries, "primaries")
                )
                .into(),
                format!(
                    "--transfer-characteristics={}",
                    aom_color(plan.transfer, "transfer")
                )
                .into(),
                format!("--matrix-coefficients={}", aom_color(plan.matrix, "matrix")).into(),
            ]);
            if chroma == "420" {
                args.push(
                    format!(
                        "--chroma-sample-position={}",
                        match plan.chroma {
                            "left" => "vertical",
                            "topleft" => "colocated",
                            _ => "unknown",
                        }
                    )
                    .into(),
                );
            }
            if raw_video_input(plan, settings) {
                args.extend([
                    format!("--i{chroma}").into(),
                    format!("--width={}", plan.width).into(),
                    format!("--height={}", plan.height).into(),
                ]);
            }
        }
        VideoEncoder::VpxStandalone => args.extend([
            "--codec=vp9".into(),
            format!("--color-space={}", vpx_color(plan.matrix)).into(),
        ]),
        _ => unreachable!("standalone AOM/VPX encoder"),
    }
    if settings.encoder == VideoEncoder::AomAv1 {
        args.extend(super::av1an::aom_grain_parameters(settings));
    }
    if settings.source_filters {
        args.retain(|argument| !argument.to_string_lossy().starts_with("--fps="));
    }
    args.extend(super::super::parameters::arguments(settings));
    args.push("-".into());
    args
}

/// AOM's high-bit-depth 4:2:0 Y4M token cannot distinguish left from centered
/// chroma. Raw input also avoids ambiguous non-4:2:0 Y4M geometry/format.
fn raw_video_input(plan: &Plan, settings: &EncodeSettings) -> bool {
    settings.encoder == VideoEncoder::AomAv1
        && !settings.source_filters
        && (plan.output_bit_depth() > 8 || !plan.output_pixel_format.contains("420"))
}

pub(super) fn configure_producer_output(
    args: &mut [OsString],
    plan: &Plan,
    settings: &EncodeSettings,
) -> bool {
    if !raw_video_input(plan, settings) {
        return false;
    }
    let format = args
        .windows(2)
        .position(|pair| pair[0] == "-f" && pair[1] == "yuv4mpegpipe")
        .expect("validated standalone decoder output format");
    args[format + 1] = "rawvideo".into();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::metadata::Document;

    fn plan(depth: u8, encoder: VideoEncoder) -> (Plan, EncodeSettings) {
        let document: Document = serde_json::from_value(serde_json::json!({
            "format":{"start_time":"0"},
            "streams":[{"index":0,"codec_type":"video","codec_name":"ffv1","width":128,"height":72,
                "pix_fmt":if depth == 8 {"yuv420p"} else {"yuv420p10le"},
                "sample_aspect_ratio":"1:1","avg_frame_rate":"24/1","start_time":"0",
                "chroma_location":"left","color_space":"bt709","color_primaries":"bt709",
                "color_transfer":"bt709","color_range":"tv"}]
        }))
        .unwrap();
        let settings = EncodeSettings {
            encoder,
            video_stream_index: 0,
            preset: 8,
            ..Default::default()
        };
        (
            Plan::build(&document, &[&document.streams[0]], &settings).unwrap(),
            settings,
        )
    }

    #[test]
    fn machine_sized_runs_pass_a_thread_count_and_recorded_arguments_stay_fixed() {
        let threads = |args: &[OsString]| {
            args.iter()
                .filter_map(|argument| {
                    argument
                        .to_str()?
                        .strip_prefix("--threads=")?
                        .parse::<usize>()
                        .ok()
                })
                .collect::<Vec<_>>()
        };
        for encoder in [VideoEncoder::AomAv1, VideoEncoder::VpxStandalone] {
            let (mut plan, settings) = plan(8, encoder);
            assert!(
                threads(&arguments(&plan, &settings)).is_empty(),
                "{encoder:?}"
            );
            plan.machine_threads = true;
            let counts = threads(&arguments(&plan, &settings));
            assert_eq!(counts.len(), 1, "{encoder:?}");
            assert!((1..=64).contains(&counts[0]), "{encoder:?}");
        }
    }

    #[test]
    fn ten_bit_aom_uses_explicit_raw_main_profile_while_other_routes_keep_y4m() {
        let (aom_10, aom_settings) = plan(10, VideoEncoder::AomAv1);
        let aom_args = arguments(&aom_10, &aom_settings);
        for option in [
            "--profile=0",
            "--input-bit-depth=10",
            "--i420",
            "--width=128",
            "--height=72",
            "--chroma-sample-position=vertical",
        ] {
            assert!(
                aom_args.iter().any(|argument| argument == option),
                "{option}"
            );
        }
        let mut producer = vec!["-f".into(), "yuv4mpegpipe".into(), "pipe:1".into()];
        assert!(configure_producer_output(
            &mut producer,
            &aom_10,
            &aom_settings
        ));
        assert_eq!(producer, ["-f", "rawvideo", "pipe:1"]);

        let (aom_8, aom_8_settings) = plan(8, VideoEncoder::AomAv1);
        let aom_8_args = arguments(&aom_8, &aom_8_settings);
        assert!(aom_8_args.iter().any(|argument| argument == "--profile=0"));
        assert!(!aom_8_args.iter().any(|argument| argument == "--i420"));
        let mut aom_8_producer = vec!["-f".into(), "yuv4mpegpipe".into()];
        assert!(!configure_producer_output(
            &mut aom_8_producer,
            &aom_8,
            &aom_8_settings
        ));
        assert_eq!(aom_8_producer, ["-f", "yuv4mpegpipe"]);

        let (vpx_10, vpx_settings) = plan(10, VideoEncoder::VpxStandalone);
        let vpx_args = arguments(&vpx_10, &vpx_settings);
        assert!(vpx_args.iter().any(|argument| argument == "--profile=2"));
        assert!(!vpx_args.iter().any(|argument| argument == "--i420"));
    }

    #[test]
    fn aom_arguments_never_panic_on_center_chroma() {
        let (mut p, settings) = plan(8, VideoEncoder::AomAv1);
        p.chroma = "center";
        let args = arguments(&p, &settings);
        assert!(
            args.iter()
                .any(|arg| arg == "--chroma-sample-position=unknown")
        );
    }
}
