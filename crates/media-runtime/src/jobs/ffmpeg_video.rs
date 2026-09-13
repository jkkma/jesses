//! FFmpeg library encoders consume the validated Y4M pipe and emit timed MKV
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
        _ => unreachable!("FFmpeg video encoder"),
    }
}

pub(super) fn arguments(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "warning",
        "-xerror",
        "-protocol_whitelist",
        "pipe",
        "-f",
        "yuv4mpegpipe",
        "-chroma_sample_location",
        plan.chroma,
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
    .collect();
    // The '+' prevents FFmpeg from choosing an alternative pixel format when
    // the installed encoder cannot produce the source's validated bit depth.
    args.push(format!("+{}", plan.output_pixel_format).into());
    args.extend([
        // Y4M carries pixel depth/range but not all colorimetry. Reattach the
        // validated tags without transforming samples or allowing auto-scale.
        "-vf".into(),
        format!(
            "setparams=field_mode=prog:range={}:color_primaries={}:color_trc={}:colorspace={}",
            if plan.full_range { "full" } else { "limited" },
            plan.primaries,
            plan.transfer,
            plan.matrix
        )
        .into(),
        "-crf".into(),
        settings.crf.to_string().into(),
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
        VideoEncoder::X265 => args.extend([
            "-preset".into(),
            X265_PRESETS[usize::from(settings.preset)].into(),
            // Disable unmanaged external side effects such as CSV/stats files.
            // All options are fixed scalars; no input path reaches this parser.
            "-x265-params".into(),
            format!(
                "pools=4:frame-threads=2:log-level=warning{}",
                super::super::parameters::x265_suffix(settings)
            )
            .into(),
        ]),
        VideoEncoder::Vp9 => args.extend([
            "-b:v".into(),
            "0".into(),
            "-deadline".into(),
            "good".into(),
            "-cpu-used".into(),
            settings.preset.to_string().into(),
            "-row-mt".into(),
            "1".into(),
            "-threads".into(),
            "4".into(),
            "-profile:v".into(),
            if plan.output_bit_depth() == 10 {
                "2"
            } else {
                "0"
            }
            .into(),
        ]),
        _ => unreachable!("FFmpeg video encoder"),
    }
    if settings.encoder == VideoEncoder::Vp9 {
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
    let options: &[&str] = if encoder == VideoEncoder::X265 {
        &["-crf", "-preset", "-x265-params"]
    } else {
        &["-crf", "-cpu-used", "-deadline", "-row-mt"]
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
