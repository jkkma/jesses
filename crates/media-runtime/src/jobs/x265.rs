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

fn color(value: u8, kind: &str) -> &'static str {
    match value {
        1 => "bt709",
        5 => "bt470bg",
        6 => "smpte170m",
        16 if kind == "transfer" => "smpte2084",
        9 if kind == "matrix" => "bt2020nc",
        9 => "bt2020",
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
        color(plan.primaries, "primaries").into(),
        "--transfer".into(),
        color(plan.transfer, "transfer").into(),
        "--colormatrix".into(),
        color(plan.matrix, "matrix").into(),
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
    args.extend(plan.x265_hdr_arguments());
    if let Some(frames) = super::super::encoding_preferences::keyframes(plan, settings) {
        args.extend(["--keyint".into(), frames.to_string().into()]);
    }
    args.extend(super::super::parameters::arguments(settings));
    // Source custom rows may set a new sample aspect. Let the actual Y4M
    // header reach the encoder; the frozen pre-filter SAR is not an override.
    if settings.source_filters
        && let Some(index) = args.iter().position(|arg| arg == "--sar")
    {
        args.drain(index..index + 2);
    }
    args
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
