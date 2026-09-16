//! Standalone x265 writes raw HEVC; the job wraps it with mkvmerge before the
//! common final mux so B-frame presentation order receives rational timestamps.
use std::ffi::OsString;

use media_core::EncodeSettings;

use super::Plan;

const PRESETS: [&str; 10] = [
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

fn color(value: u8) -> &'static str {
    match value {
        1 => "bt709",
        5 => "bt470bg",
        6 => "smpte170m",
        _ => unreachable!("validated SDR color"),
    }
}

pub(super) fn arguments(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        // x265 accepts this documented input-mode switch even though some
        // Windows builds describe Y4M only in the syntax preamble.
        "--y4m".into(),
        "--input".into(),
        "-".into(),
        "--output".into(),
        "-".into(),
        "--fps".into(),
        format!("{}/{}", plan.fps_num, plan.fps_den).into(),
        "--output-depth".into(),
        plan.output_bit_depth().to_string().into(),
        "--preset".into(),
        PRESETS[usize::from(settings.preset)].into(),
        "--sar".into(),
        plan.output_sar().into(),
        "--range".into(),
        if plan.full_range { "full" } else { "limited" }.into(),
        "--colorprim".into(),
        color(plan.primaries).into(),
        "--transfer".into(),
        color(plan.transfer).into(),
        "--colormatrix".into(),
        color(plan.matrix).into(),
        "--chromaloc".into(),
        match plan.chroma {
            "left" => "0",
            "center" => "1",
            "topleft" => "2",
            _ => unreachable!("validated x265 chroma placement"),
        }
        .into(),
    ];
    if settings.lossless {
        args.push("--lossless".into());
    } else {
        args.extend(["--crf".into(), settings.crf.to_string().into()]);
    }
    args.extend(super::super::parameters::arguments(settings));
    args
}

pub(super) fn validate_help(help: &str, depth: u8, lossless: bool) -> Result<(), String> {
    let mut required = vec![
        "--input",
        "--output",
        "--fps",
        "--output-depth",
        "--preset",
        "--sar",
        "--range",
        "--colorprim",
        "--transfer",
        "--colormatrix",
        "--chromaloc",
        "--crf",
        "--bitrate",
        "--pass",
        "--stats",
    ];
    if lossless {
        required.push("--lossless");
    }
    let advertises = |option: &str| {
        help.split_whitespace().any(|word| {
            word == option || word.ends_with(option) || word.replace("[no-]", "") == option
        })
    };
    if required.iter().any(|option| !advertises(option))
        || !help.lines().any(|line| {
            line.split_whitespace()
                .any(|word| word.ends_with("--output-depth"))
                && line.split_whitespace().any(|word| {
                    word.split('|')
                        .any(|value| value.parse::<u8>() == Ok(depth))
                })
        })
    {
        return Err(format!(
            "The installed x265 CLI must advertise Y4M stdin, raw stdout, {depth}-bit output, color metadata, pass statistics, and the selected lossless capability."
        ));
    }
    Ok(())
}

pub(super) fn mkvmerge_arguments(
    input: &std::path::Path,
    output: &std::path::Path,
    plan: &Plan,
) -> Vec<OsString> {
    vec![
        "--quiet".into(),
        "--output".into(),
        output.as_os_str().to_owned(),
        "--default-duration".into(),
        format!("0:{}/{}p", plan.fps_num, plan.fps_den).into(),
        input.as_os_str().to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_x265_bracketed_boolean_lossless_switch() {
        let help = "--input --output --fps -D/--output-depth 8|10|12 --preset --sar --range \
                    --colorprim --transfer --colormatrix --chromaloc --crf --bitrate --pass \
                    --stats --[no-]lossless";
        assert!(validate_help(help, 10, true).is_ok());
    }
}
