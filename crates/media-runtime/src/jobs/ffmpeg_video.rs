//! FFmpeg library encoders consume validated Y4M (rawvideo for VP9 alpha) and emit timed MKV
//! through the supervisor's already-owned output handle.
use std::ffi::OsString;

use media_core::{EncodeSettings, VideoEncoder};

use super::Plan;

const X265_PRESETS: [&str; 10] = [
    "ultrafast",
    "superfast",
    "veryfast",
    "faster",
    "fast",
    "medium",
    "slow",
    "slower",
    "veryslow",
    "placebo",
];

pub(super) fn library(encoder: VideoEncoder) -> &'static str {
    match encoder {
        VideoEncoder::X265 => "libx265",
        VideoEncoder::Vp9 => "libvpx-vp9",
        VideoEncoder::H264Nvenc => "h264_nvenc",
        VideoEncoder::HevcNvenc => "hevc_nvenc",
        _ => unreachable!("FFmpeg video encoder"),
    }
}

pub(super) fn encoder_pixel_format(encoder: VideoEncoder, depth: u8) -> &'static str {
    if matches!(encoder, VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc) && depth == 10 {
        "p010le"
    } else if depth == 10 {
        "yuv420p10le"
    } else {
        "yuv420p"
    }
}

pub(super) fn output_format(plan: &Plan) -> &'static str {
    if matches!(
        plan.encoder,
        VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc
    ) {
        encoder_pixel_format(plan.encoder, plan.output_bit_depth())
    } else {
        plan.output_pixel_format
    }
}

pub(super) fn arguments(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let alpha = settings.encoder == VideoEncoder::Vp9 && plan.output_pixel_format == "yuva420p";
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "warning",
        "-xerror",
        "-protocol_whitelist",
        "pipe",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    if alpha {
        args.extend([
            "-f".into(),
            "rawvideo".into(),
            "-pixel_format".into(),
            "yuva420p".into(),
            "-video_size".into(),
            format!("{}x{}", plan.width, plan.height).into(),
            "-framerate".into(),
            format!("{}/{}", plan.fps_num, plan.fps_den).into(),
        ]);
    } else {
        args.extend([
            "-f".into(),
            "yuv4mpegpipe".into(),
            "-chroma_sample_location".into(),
            plan.chroma.into(),
        ]);
    }
    args.extend(
        [
            "-i",
            "pipe:0",
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-dn",
            "-c:v",
            library(settings.encoder),
            "-pix_fmt",
        ]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>(),
    );
    // The '+' prevents FFmpeg from choosing an alternative pixel format when
    // the installed encoder cannot produce the source's validated bit depth.
    let encoder_format = output_format(plan);
    args.push(format!("+{encoder_format}").into());
    args.extend([
        // Y4M carries pixel depth/range but not all colorimetry. Reattach the
        // validated tags without transforming samples or allowing auto-scale.
        "-vf".into(),
        format!(
            "{}setsar={},setparams=field_mode=prog:range={}:color_primaries={}:color_trc={}:colorspace={}",
            if encoder_format == plan.output_pixel_format {
                String::new()
            } else {
                format!("format={encoder_format},")
            },
            plan.output_sar(),
            if plan.full_range { "full" } else { "limited" },
            plan.primaries,
            plan.transfer,
            plan.matrix
        )
        .into(),
        "-color_primaries".into(),
        plan.primaries.to_string().into(),
        "-color_trc".into(),
        plan.transfer.to_string().into(),
        "-colorspace".into(),
        plan.matrix.to_string().into(),
        "-color_range".into(),
        if plan.full_range { "pc" } else { "tv" }.into(),
        "-chroma_sample_location".into(),
        plan.chroma.into(),
    ]);
    match settings.encoder {
        VideoEncoder::X265 => {
            if !settings.lossless {
                args.extend(["-crf".into(), settings.crf.to_string().into()]);
            }
            args.extend([
                "-preset".into(),
                X265_PRESETS[usize::from(settings.preset)].into(),
                // Disable unmanaged external side effects such as CSV/stats files.
                // All options are fixed scalars; no input path reaches this parser.
                "-x265-params".into(),
                format!(
                    "pools=4:frame-threads=2:log-level=warning{}{}",
                    if settings.lossless { ":lossless=1" } else { "" },
                    super::super::parameters::x265_suffix(settings)
                )
                .into(),
            ]);
        }
        VideoEncoder::Vp9 => {
            if !settings.lossless {
                args.extend(["-crf".into(), settings.crf.to_string().into()]);
            }
            args.extend([
                "-b:v".into(),
                "0".into(),
                "-lossless".into(),
                if settings.lossless { "1" } else { "0" }.into(),
                "-deadline".into(),
                "good".into(),
                "-cpu-used".into(),
                settings.preset.to_string().into(),
                "-row-mt".into(),
                "1".into(),
                "-threads".into(),
                "4".into(),
                "-profile:v".into(),
                match (
                    plan.output_pixel_format.starts_with("yuv444"),
                    plan.output_bit_depth(),
                ) {
                    (false, 8) => "0",
                    (true, 8) => "1",
                    (false, _) => "2",
                    (true, _) => "3",
                }
                .into(),
            ]);
            if alpha {
                args.extend([
                    "-auto-alt-ref".into(),
                    "0".into(),
                    "-metadata:s:v:0".into(),
                    "alpha_mode=1".into(),
                ]);
            }
        }
        VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc => {
            const PRESETS: [&str; 7] = ["p1", "p2", "p3", "p4", "p5", "p6", "p7"];
            args.extend([
                "-preset".into(),
                PRESETS[usize::from(settings.preset)].into(),
            ]);
            if settings.lossless {
                args.extend([
                    "-tune".into(),
                    "lossless".into(),
                    "-rc".into(),
                    "constqp".into(),
                    "-qp".into(),
                    "0".into(),
                ]);
            } else {
                args.extend([
                    "-rc".into(),
                    "vbr".into(),
                    "-cq".into(),
                    settings.crf.to_string().into(),
                    "-b:v".into(),
                    "0".into(),
                ]);
            }
        }
        _ => unreachable!("FFmpeg video encoder"),
    }
    if matches!(
        settings.encoder,
        VideoEncoder::Vp9 | VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc
    ) {
        args.extend(super::super::parameters::arguments(settings));
    }
    args.extend(
        [
            "-fps_mode",
            "passthrough",
            "-avoid_negative_ts",
            "disabled",
            "-progress",
            "pipe:2",
            "-nostats",
            "-f",
            "matroska",
            "pipe:1",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args
}

pub(super) fn validate_help(help: &str, encoder: VideoEncoder, format: &str) -> Result<(), String> {
    let name = library(encoder);
    let has_encoder = help
        .lines()
        .any(|line| line.starts_with(&format!("Encoder {name} [")));
    let has_format = help
        .lines()
        .find_map(|line| line.trim().strip_prefix("Supported pixel formats:"))
        .is_some_and(|formats| formats.split_whitespace().any(|value| value == format));
    let options: &[&str] = match encoder {
        VideoEncoder::X265 => &["-crf", "-preset", "-x265-params"],
        VideoEncoder::Vp9 if format == "yuva420p" => &[
            "-crf",
            "-cpu-used",
            "-deadline",
            "-row-mt",
            "-lossless",
            "-auto-alt-ref",
        ],
        VideoEncoder::Vp9 => &["-crf", "-cpu-used", "-deadline", "-row-mt", "-lossless"],
        VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc => {
            &["-cq", "-preset", "-tune", "-rc", "-qp"]
        }
        _ => unreachable!("FFmpeg video encoder"),
    };
    if !has_encoder
        || !has_format
        || options
            .iter()
            .any(|option| !help.split_whitespace().any(|word| word == *option))
    {
        return Err(format!(
            "FFmpeg must include {name} with {format} output and the required quality/speed options. The source bit depth will not be reduced."
        ));
    }
    Ok(())
}

pub(super) fn hardware_probe_arguments(
    encoder: VideoEncoder,
    depth: u8,
    lossless: bool,
) -> Vec<OsString> {
    let format = encoder_pixel_format(encoder, depth);
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=s=64x64:r=1:d=1",
        "-frames:v",
        "1",
        "-an",
        "-vf",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.extend([
        format!("format={format}").into(),
        "-c:v".into(),
        library(encoder).into(),
        "-preset".into(),
        "p1".into(),
    ]);
    if lossless {
        args.extend([
            "-tune".into(),
            "lossless".into(),
            "-rc".into(),
            "constqp".into(),
            "-qp".into(),
            "0".into(),
        ]);
    } else {
        args.extend([
            "-rc".into(),
            "vbr".into(),
            "-cq".into(),
            "23".into(),
            "-b:v".into(),
            "0".into(),
        ]);
    }
    args.extend(["-f".into(), "null".into(), "-".into()]);
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_or_depth_is_rejected_instead_of_falling_back() {
        let help = "Encoder libx265 [HEVC]:\n Supported pixel formats: yuv420p\n -crf -preset -x265-params";
        assert!(validate_help(help, VideoEncoder::X265, "yuv420p").is_ok());
        assert!(validate_help(help, VideoEncoder::X265, "yuv420p10le").is_err());
        assert!(validate_help(help, VideoEncoder::Vp9, "yuv420p").is_err());
        assert!(validate_help("Codec not recognized", VideoEncoder::Vp9, "yuv420p").is_err());
    }
}
