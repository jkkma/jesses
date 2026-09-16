//! Standalone x264 emits a timed Matroska stream through its inherited stdout.
//! Keeping picture timestamps in this intermediate preserves B-frame ordering
//! before the common stream-copy mux and complete decoded-output validation.
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

pub(super) fn arguments(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let color = |value| match value {
        1 => "bt709",
        5 => "bt470bg",
        6 => "smpte170m",
        _ => unreachable!("validated SDR color"),
    };
    let range = if plan.full_range { "pc" } else { "tv" };
    let mut args: Vec<OsString> = [
        "--demuxer".into(),
        "y4m".into(),
        "--muxer".into(),
        "mkv".into(),
        "--force-cfr".into(),
        "--fps".into(),
        format!("{}/{}", plan.fps_num, plan.fps_den),
        "--output-depth".into(),
        plan.output_bit_depth().to_string(),
        "--output-csp".into(),
        "i420".into(),
        "--input-range".into(),
        range.into(),
        "--range".into(),
        range.into(),
        "--sar".into(),
        plan.output_sar().into(),
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
            _ => unreachable!("validated x264 chroma placement"),
        }
        .into(),
        "--preset".into(),
        PRESETS[usize::from(settings.preset)].into(),
        "-o".into(),
        "-".into(),
        "-".into(),
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.splice(
        args.len() - 3..args.len() - 3,
        if settings.lossless {
            vec!["--qp".into(), "0".into()]
        } else {
            vec!["--crf".into(), settings.crf.to_string().into()]
        },
    );
    args.splice(
        args.len() - 3..args.len() - 3,
        super::super::parameters::arguments(settings),
    );
    args
}

pub(super) fn validate_help(help: &str, depth: u8) -> Result<(), String> {
    let required = [
        "--demuxer",
        "--muxer",
        "--force-cfr",
        "--fps",
        "--output-depth",
        "--output-csp",
        "--input-range",
        "--range",
        "--sar",
        "--colorprim",
        "--transfer",
        "--colormatrix",
        "--chromaloc",
        "--crf",
        "--preset",
    ];
    if required
        .iter()
        .any(|option| !help.split_whitespace().any(|word| word == *option))
        || !advertises_choice(help, "--muxer", "mkv")
        || !advertises_choice(help, "--demuxer", "y4m")
        || !advertises_choice(help, "--output-csp", "i420")
    {
        return Err("The installed x264 must advertise Y4M input, Matroska output, and the required CFR, pixel format, and color options.".into());
    }
    if !help.lines().any(|line| {
        line.trim()
            .strip_prefix("Output bit depth:")
            .is_some_and(|value| {
                value
                    .split('/')
                    .any(|value| value.trim().parse::<u8>() == Ok(depth))
            })
    }) {
        return Err(format!(
            "The installed x264 does not advertise {depth}-bit output. Install a build supporting the selected source bit depth."
        ));
    }
    Ok(())
}

fn advertises_choice(help: &str, option: &str, choice: &str) -> bool {
    let mut lines = help.lines();
    if !lines.any(|line| line.split_whitespace().next() == Some(option)) {
        return false;
    }
    lines
        .next()
        .and_then(|line| line.trim().strip_prefix("- "))
        .is_some_and(|list| list.split(',').any(|value| value.trim() == choice))
}

pub(super) fn frame_counter(line: &str) -> Option<u64> {
    let line = line
        .trim()
        .strip_prefix("[consumer] ")
        .unwrap_or(line.trim());
    let line = line.strip_prefix("encoded ").unwrap_or(line);
    let line = if line.starts_with('[') {
        let (percent, rest) = line.split_once("] ")?;
        percent.ends_with('%').then_some(rest)?
    } else {
        line
    };
    let mut words = line.split_whitespace();
    let count = words.next()?.split('/').next()?.parse().ok()?;
    matches!(words.next()?, "frames:" | "frames,").then_some(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::metadata::Document;
    use media_core::VideoEncoder;

    fn plan(depth: u8, chroma: &str, full_range: bool) -> (Plan, EncodeSettings) {
        let document: Document = serde_json::from_value(serde_json::json!({
            "format":{"start_time":"0"},
            "streams":[{"index":2,"codec_type":"video","codec_name":"h264","width":128,"height":96,
                "pix_fmt":if depth == 8 {"yuv420p"} else {"yuv420p10le"},
                "sample_aspect_ratio":"1:1","avg_frame_rate":"24000/1001","start_time":"0",
                "chroma_location":chroma,"color_space":"bt470bg","color_primaries":"bt709",
                "color_transfer":"smpte170m","color_range":if full_range {"pc"} else {"tv"}}]
        }))
        .unwrap();
        let settings = EncodeSettings {
            encoder: VideoEncoder::X264,
            video_stream_index: 2,
            crf: 23,
            preset: 5,
            ..Default::default()
        };
        (
            Plan::build(&document, &[&document.streams[0]], &settings).unwrap(),
            settings,
        )
    }

    #[test]
    fn writes_timed_stdout_and_preserves_validated_depth_color_and_cadence() {
        for (depth, chroma, location, full) in [
            (8, "left", "0", false),
            (10, "center", "1", true),
            (8, "topleft", "2", true),
        ] {
            let (plan, mut settings) = plan(depth, chroma, full);
            for preset in 0..=9 {
                settings.preset = preset;
                let args = arguments(&plan, &settings);
                for (option, value) in [
                    ("--muxer", "mkv"),
                    ("--demuxer", "y4m"),
                    ("--fps", "24000/1001"),
                    ("--output-depth", if depth == 8 { "8" } else { "10" }),
                    ("--input-range", if full { "pc" } else { "tv" }),
                    ("--range", if full { "pc" } else { "tv" }),
                    ("--colorprim", "bt709"),
                    ("--transfer", "smpte170m"),
                    ("--colormatrix", "bt470bg"),
                    ("--chromaloc", location),
                    ("--preset", PRESETS[usize::from(preset)]),
                ] {
                    assert!(
                        args.windows(2).any(|pair| pair == [option, value]),
                        "{option}={value}"
                    );
                }
                assert_eq!(&args[args.len() - 3..], ["-o", "-", "-"]);
                assert!(args.iter().any(|arg| arg == "--force-cfr"));
                assert!(
                    !args
                        .iter()
                        .any(|arg| arg == "--bframes" || arg == "--frames")
                );
            }
        }
    }

    #[test]
    fn rejects_missing_container_and_depth_capabilities() {
        let help = "--demuxer <string>\n - auto, raw, y4m\n--muxer <string>\n - auto, raw, mkv\n--output-csp <string>\n - i420, i444\n--force-cfr --fps --output-depth --input-range --range --sar --colorprim --transfer --colormatrix --chromaloc --crf --preset\nOutput bit depth: 8/10\n";
        assert!(validate_help(help, 8).is_ok());
        assert!(validate_help(help, 10).is_ok());
        assert!(validate_help(&help.replace("8/10", "8"), 10).is_err());
        assert!(validate_help(&help.replace(", mkv", ""), 8).is_err());
        assert!(validate_help(&help.replace(", y4m", ""), 8).is_err());
        assert!(validate_help(&help.replace("--force-cfr", ""), 8).is_err());
    }

    #[test]
    fn parses_unknown_length_and_known_length_progress_without_summary_confusion() {
        assert_eq!(
            frame_counter("[consumer] 120 frames: 20.5 fps, 100.00 kb/s"),
            Some(120)
        );
        assert_eq!(
            frame_counter("[consumer] [25.0%] 30/120 frames, 20.5 fps"),
            Some(30)
        );
        assert_eq!(
            frame_counter("[consumer] encoded 120 frames, 50 fps"),
            Some(120)
        );
        assert_eq!(
            frame_counter("[consumer] x264 [info]: frame B:89 Avg QP:24"),
            None
        );
        assert_eq!(
            frame_counter("[producer] 400 frames: unrelated output"),
            None
        );
    }
}
