//! Standalone AOM/VPX drivers consume validated planar video and emit timed IVF.
use std::ffi::OsString;

use media_core::{EncodeSettings, VideoEncoder};

use super::Plan;

fn aom_color(value: u8, kind: &str) -> &'static str {
    match (kind, value) {
        ("primaries", 1) | ("transfer", 1) | ("matrix", 1) => "bt709",
        ("primaries", 5) | ("transfer", 5) | ("matrix", 5) => "bt470bg",
        ("primaries", 6) | ("transfer", 6) | ("matrix", 6) => "bt601",
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
    let profile = match settings.encoder {
        VideoEncoder::AomAv1 => 0,
        VideoEncoder::VpxStandalone => u8::from(plan.output_bit_depth() == 10) * 2,
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
                format!(
                    "--chroma-sample-position={}",
                    match plan.chroma {
                        "left" => "vertical",
                        "topleft" => "colocated",
                        _ => unreachable!("validated AOM chroma position"),
                    }
                )
                .into(),
            ]);
            if raw_i420(plan, settings) {
                args.extend([
                    "--i420".into(),
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
    args.extend(super::super::parameters::arguments(settings));
    args.push("-".into());
    args
}

/// AOM's high-bit-depth Y4M token does not distinguish left-positioned
/// 4:2:0 from centered chroma. Feed raw I420 instead and bind the otherwise
/// header-owned geometry, cadence and depth through explicit aomenc options.
fn raw_i420(plan: &Plan, settings: &EncodeSettings) -> bool {
    settings.encoder == VideoEncoder::AomAv1 && plan.output_bit_depth() > 8
}

pub(super) fn configure_producer_output(
    args: &mut [OsString],
    plan: &Plan,
    settings: &EncodeSettings,
) -> bool {
    if !raw_i420(plan, settings) {
        return false;
    }
    let format = args
        .windows(2)
        .position(|pair| pair[0] == "-f" && pair[1] == "yuv4mpegpipe")
        .expect("validated standalone decoder output format");
    args[format + 1] = "rawvideo".into();
    true
}

pub(super) fn validate_help(
    help: &str,
    encoder: VideoEncoder,
    depth: u8,
    lossless: bool,
) -> Result<(), String> {
    let mut required = vec![
        "--ivf",
        "--output",
        "--passes",
        "--pass",
        "--fpf",
        "--end-usage",
        "--target-bitrate",
        "--cq-level",
        "--cpu-used",
        "--row-mt",
        "--profile",
        "--bit-depth",
        "--input-bit-depth",
        "--fps",
        "--disable-warning-prompt",
    ];
    match encoder {
        VideoEncoder::AomAv1 => required.extend([
            "--color-primaries",
            "--transfer-characteristics",
            "--matrix-coefficients",
            "--chroma-sample-position",
        ]),
        VideoEncoder::VpxStandalone => required.extend(["--codec", "--color-space"]),
        _ => unreachable!("standalone AOM/VPX encoder"),
    }
    if lossless {
        required.push("--lossless");
    }
    if encoder == VideoEncoder::AomAv1 && depth > 8 {
        required.extend(["--i420", "--width", "--height"]);
    }
    if required.iter().any(|flag| {
        !help
            .split_whitespace()
            .any(|word| word.trim_end_matches("=<arg>") == *flag)
    }) || !help.contains(&format!(" {depth}"))
    {
        return Err(format!(
            "The installed {} must advertise timed IVF input/output, the selected Y4M or raw-I420 input controls, {depth}-bit coding, bounded rate control, color signaling, and the selected lossless capability.",
            encoder.name()
        ));
    }
    Ok(())
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
    fn capability_check_requires_lossless_and_depth_when_selected() {
        let common = "--ivf --output=<arg> --passes=<arg> --pass=<arg> --fpf=<arg> --end-usage=<arg> --target-bitrate=<arg> --cq-level=<arg> --cpu-used=<arg> --row-mt=<arg> --profile=<arg> --bit-depth=<arg> 8, 10 --input-bit-depth=<arg> --fps=<arg> --disable-warning-prompt";
        let aom = format!(
            "{common} --lossless=<arg> --color-primaries=<arg> --transfer-characteristics=<arg> --matrix-coefficients=<arg> --chroma-sample-position=<arg> --i420 --width=<arg> --height=<arg>"
        );
        assert!(validate_help(&aom, VideoEncoder::AomAv1, 10, true).is_ok());
        assert!(
            validate_help(
                &aom.replace("--lossless=<arg>", ""),
                VideoEncoder::AomAv1,
                10,
                true
            )
            .is_err()
        );
        let vpx = format!("{common} --codec=<arg> --color-space=<arg>");
        assert!(validate_help(&vpx, VideoEncoder::VpxStandalone, 8, false).is_ok());
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
}
