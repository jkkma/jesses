//! Additional standalone controls, filtered against the selected binary's help.
use super::{Spec, ValueKind};

macro_rules! number {
    ($name:literal, $label:literal, $min:expr, $max:expr) => {
        Spec {
            name: $name,
            label: $label,
            flag: concat!("--", $name),
            min: $min,
            max: $max,
            kind: ValueKind::Whole,
            choices: &[],
            group: "Advanced",
            description: "",
            example: "",
        }
    };
}
macro_rules! decimal {
    ($name:literal, $label:literal, $min:expr, $max:expr) => {
        Spec {
            kind: ValueKind::Decimal,
            ..number!($name, $label, $min, $max)
        }
    };
}
macro_rules! choice {
    ($name:literal, $label:literal, $choices:expr) => {
        Spec {
            kind: ValueKind::Choice,
            choices: $choices,
            ..number!($name, $label, 0, 0)
        }
    };
}

pub(super) fn x265() -> Vec<Spec> {
    vec![
        number!("keyint", "Maximum keyframe interval", 1, 10000),
        choice!(
            "tune",
            "Tuning",
            &[
                "psnr",
                "ssim",
                "grain",
                "zerolatency",
                "fastdecode",
                "animation"
            ]
        ),
        choice!(
            "profile",
            "Profile",
            &[
                "main",
                "main10",
                "main12",
                "mainstillpicture",
                "main-intra",
                "main10-intra",
                "main12-intra",
                "main422-10",
                "main422-12",
                "main422-10-intra",
                "main422-12-intra",
                "main444-8",
                "main444-10",
                "main444-12",
                "main444-intra",
                "main444-10-intra",
                "main444-12-intra",
                "main444-stillpicture"
            ]
        ),
        choice!(
            "level-idc",
            "Minimum decoder level",
            &[
                "0", "1", "2", "2.1", "3", "3.1", "4", "4.1", "5", "5.1", "5.2", "6", "6.1", "6.2",
                "10", "20", "21", "30", "31", "40", "41", "50", "51", "52", "60", "61", "62"
            ]
        ),
        Spec {
            kind: ValueKind::PairWhole,
            example: "0:0",
            ..number!("deblock", "Deblocking offsets", -6, 6)
        },
        number!("selective-sao", "Selective sample adaptive offset", 0, 4),
        number!("bframes", "Maximum consecutive B-frames", 0, 16),
        number!("b-adapt", "B-frame adaptation", 0, 2),
        number!("ref", "Reference frames", 1, 8),
        number!("rc-lookahead", "Lookahead frames", 0, 250),
        number!("subme", "Subpixel motion search", 0, 7),
        number!("merange", "Motion search range", 0, 32767),
        choice!(
            "me",
            "Motion search method",
            &["dia", "hex", "umh", "star", "sea", "full"]
        ),
        number!("max-merge", "Merge candidates", 1, 5),
        number!("limit-refs", "Reference limiting", 0, 3),
        number!("bframe-bias", "B-frame bias", -90, 100),
        number!("aq-mode", "Adaptive quantization mode", 0, 4),
        decimal!("aq-strength", "Adaptive quantization strength", 0, 3),
        decimal!("psy-rd", "Psychovisual rate distortion", 0, 5),
        decimal!("psy-rdoq", "Psychovisual quantization", 0, 50),
        number!("rd", "Rate distortion level", 1, 6),
        decimal!("qcomp", "Quantizer curve compression", 0, 1),
        number!("rdoq-level", "Quantization optimization level", 0, 2),
        number!("cbqpoffs", "Cb quantizer offset", -12, 12),
        number!("crqpoffs", "Cr quantizer offset", -12, 12),
        choice!("ctu", "Coding tree unit size", &["16", "32", "64"]),
        choice!(
            "min-cu-size",
            "Minimum coding unit size",
            &["8", "16", "32"]
        ),
        number!("tu-intra-depth", "Intra transform depth", 1, 4),
        number!("tu-inter-depth", "Inter transform depth", 1, 4),
        number!("rdpenalty", "Intra rate distortion penalty", 0, 2),
        number!("qpmin", "Minimum quantizer", 0, 69),
        number!("qpmax", "Maximum quantizer", 0, 69),
        Spec {
            example: "1.40",
            ..decimal!("ipratio", "I/P quantizer ratio", 1, 10)
        },
        Spec {
            example: "1.30",
            ..decimal!("pbratio", "P/B quantizer ratio", 1, 10)
        },
        number!("sao", "Sample adaptive offset (0 off, 1 on)", 0, 1),
        number!("cutree", "CU-tree rate control (0 off, 1 on)", 0, 1),
    ]
}

pub(super) fn aom() -> Vec<Spec> {
    vec![
        number!("disable-kf", "Disable automatic keyframes", 1, 1),
        number!("kf-min-dist", "Minimum keyframe interval", 0, 10000),
        number!("kf-max-dist", "Maximum keyframe interval", 1, 10000),
        number!("tile-rows", "Tile rows (log2)", 0, 6),
        number!("tile-columns", "Tile columns (log2)", 0, 6),
        choice!(
            "tune-content",
            "Content tuning",
            &["default", "screen", "film"]
        ),
        number!("aq-mode", "Adaptive quantization mode", 0, 3),
        number!("enable-qm", "Quantization matrices", 0, 1),
        number!("quant-b-adapt", "Quantizer adaptation", 0, 1),
        choice!(
            "tune",
            "Metric tuning",
            &["psnr", "ssim", "iq", "ssimulacra2"]
        ),
        number!("qm-min", "Minimum quantization matrix", 0, 15),
        number!("qm-max", "Maximum quantization matrix", 0, 15),
        number!("sharpness", "Sharpness", 0, 7),
        number!("deltaq-mode", "Delta quantization mode", 0, 6),
        number!("enable-chroma-deltaq", "Chroma delta quantization", 0, 1),
        number!("enable-cdef", "Directional enhancement filter", 0, 3),
        number!("arnr-maxframes", "Maximum temporal filter frames", 0, 15),
        number!("arnr-strength", "Temporal filter strength", 0, 6),
        number!("enable-keyframe-filtering", "Keyframe filtering", 0, 2),
        number!("enable-restoration", "Loop restoration", 0, 1),
        number!("lag-in-frames", "Lookahead frames", 0, 48),
        number!("mv-cost-upd-freq", "Motion cost update frequency", 0, 3),
        number!("enable-fwd-kf", "Forward keyframes", 0, 1),
        number!("min-q", "Minimum quantizer", 0, 63),
        number!("max-q", "Maximum quantizer", 0, 63),
        number!("undershoot-pct", "Rate control undershoot", 0, 100),
        number!("overshoot-pct", "Rate control overshoot", 0, 100),
        number!("auto-alt-ref", "Alternate reference frames", 0, 1),
    ]
}

pub(super) fn vpx() -> Vec<Spec> {
    vec![
        number!("kf-max-dist", "Maximum keyframe interval", 1, 10000),
        number!("tile-rows", "Tile rows (log2)", 0, 2),
        number!("tile-columns", "Tile columns (log2)", 0, 6),
        choice!("tune", "Metric tuning", &["psnr", "ssim"]),
        choice!(
            "tune-content",
            "Content tuning",
            &["default", "screen", "film"]
        ),
        number!("sharpness", "Sharpness", 0, 7),
        number!("static-thresh", "Static block threshold", 0, 65535),
        number!("frame-boost", "Frame boost", 0, 1),
        number!("noise-sensitivity", "Noise sensitivity", 0, 6),
        number!("auto-alt-ref", "Alternate reference frames", 0, 6),
        number!("lag-in-frames", "Lookahead frames", 0, 25),
        number!("arnr-maxframes", "Maximum temporal filter frames", 0, 15),
        number!("arnr-strength", "Temporal filter strength", 0, 6),
        number!("alt-ref-aq", "Alternate reference quantization", 0, 1),
        number!("frame-parallel", "Frame parallel decoding", 0, 1),
        number!("aq-mode", "Adaptive quantization mode", 0, 4),
        number!("min-q", "Minimum quantizer", 0, 63),
        number!("max-q", "Maximum quantizer", 0, 63),
        number!("enable-tpl", "Temporal dependency model", 0, 1),
        number!("corpus-complexity", "Corpus complexity", 0, 10000),
        number!("undershoot-pct", "Rate control undershoot", 0, 100),
        number!("overshoot-pct", "Rate control overshoot", 0, 100),
        number!("bias-pct", "Rate control bias", 0, 100),
        number!("minsection-pct", "Minimum section rate", 0, 100),
        number!("maxsection-pct", "Maximum section rate", 0, 65535),
    ]
}
