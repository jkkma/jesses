//! Bounded encoder catalogs. User strings never become option names or paths.
use super::*;
use media_core::{EncodeBackend, EncoderParameterCatalog, EncoderParameterSpec, VideoEncoder};
mod catalog;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Whole,
    Decimal,
    PairWhole,
    PairDecimal,
    Choice,
    ChoiceList,
}

impl ValueKind {
    fn name(self) -> &'static str {
        match self {
            Self::Whole => "whole",
            Self::Decimal => "decimal",
            Self::PairWhole => "pairWhole",
            Self::PairDecimal => "pairDecimal",
            Self::Choice => "choice",
            Self::ChoiceList => "choiceList",
        }
    }
}

#[derive(Clone, Copy)]
struct Spec {
    name: &'static str,
    label: &'static str,
    flag: &'static str,
    min: i32,
    max: i32,
    kind: ValueKind,
    choices: &'static [&'static str],
    group: &'static str,
    description: &'static str,
    example: &'static str,
}

const fn item(
    name: &'static str,
    label: &'static str,
    flag: &'static str,
    min: u16,
    max: u16,
) -> Spec {
    Spec {
        name,
        label,
        flag,
        min: min as i32,
        max: max as i32,
        kind: ValueKind::Whole,
        choices: &[],
        group: "Advanced",
        description: "",
        example: "",
    }
}

fn specs(encoder: VideoEncoder) -> Vec<Spec> {
    match encoder {
        VideoEncoder::X264 => catalog::x264(),
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
            catalog::svt()
        }
        VideoEncoder::X265 => vec![
            item("ref", "Reference frames", "ref", 1, 6),
            item("bframes", "Maximum consecutive B-frames", "bframes", 0, 16),
            item("b-adapt", "B-frame adaptation", "b-adapt", 0, 2),
            item("aq-mode", "Adaptive quantization mode", "aq-mode", 0, 4),
            item("sao", "Sample adaptive offset (0 off, 1 on)", "sao", 0, 1),
            item(
                "cutree",
                "CU-tree rate control (0 off, 1 on)",
                "cutree",
                0,
                1,
            ),
        ],
        VideoEncoder::Vp9 => vec![
            item("aq-mode", "Adaptive quantization mode", "-aq-mode", 0, 4),
            item("lag-in-frames", "Lookahead frames", "-lag-in-frames", 0, 25),
            item(
                "auto-alt-ref",
                "Alternate reference frames (0 off, 1 on)",
                "-auto-alt-ref",
                0,
                1,
            ),
        ],
        VideoEncoder::X265Standalone => vec![
            item("ref", "Reference frames", "--ref", 1, 6),
            item(
                "bframes",
                "Maximum consecutive B-frames",
                "--bframes",
                0,
                16,
            ),
            item("b-adapt", "B-frame adaptation", "--b-adapt", 0, 2),
            item("aq-mode", "Adaptive quantization mode", "--aq-mode", 0, 4),
            item("sao", "Sample adaptive offset (0 off, 1 on)", "--sao", 0, 1),
            item(
                "cutree",
                "CU-tree rate control (0 off, 1 on)",
                "--cutree",
                0,
                1,
            ),
        ],
        VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone => vec![
            item("aq-mode", "Adaptive quantization mode", "--aq-mode", 0, 4),
            item(
                "lag-in-frames",
                "Lookahead frames",
                "--lag-in-frames",
                0,
                25,
            ),
            item(
                "auto-alt-ref",
                "Alternate reference frames",
                "--auto-alt-ref",
                0,
                1,
            ),
        ],
        VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc => vec![
            item("bf", "B-frames", "-bf", 0, 4),
            item(
                "rc-lookahead",
                "Rate-control lookahead",
                "-rc-lookahead",
                0,
                32,
            ),
            item(
                "spatial-aq",
                "Spatial adaptive quantization",
                "-spatial-aq",
                0,
                1,
            ),
            item(
                "temporal-aq",
                "Temporal adaptive quantization",
                "-temporal-aq",
                0,
                1,
            ),
        ],
    }
}

fn error(message: impl Into<String>) -> AppError {
    AppError::new("ENCODER_PARAMETERS_INVALID", message, None)
}

fn whole(value: &str, min: i32, max: i32) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty()
        && value.len() <= 10
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && (min..=max).contains(&value.parse::<i32>().unwrap_or(i32::MAX))
}

fn decimal(value: &str, min: i32, max: i32) -> bool {
    let mut parts = value.split('.');
    let integer = parts.next().unwrap_or("");
    let fraction = parts.next();
    if parts.next().is_some()
        || value.len() > 16
        || integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    let fraction = match fraction {
        Some(digits)
            if !digits.is_empty()
                && digits.len() <= 3
                && digits.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            digits
        }
        Some(_) => return false,
        None => "",
    };
    let integer = integer.parse::<i64>().unwrap_or(i64::MAX);
    let fraction = fraction.parse::<i64>().unwrap_or(0) * 10_i64.pow((3 - fraction.len()) as u32);
    let scaled = integer.saturating_mul(1000).saturating_add(fraction);
    scaled >= i64::from(min) * 1000
        && scaled <= i64::from(max) * 1000
        && !(min == 1 && (scaled <= 1000))
}

fn valid_value(spec: &Spec, value: &str) -> bool {
    match spec.kind {
        ValueKind::Whole => whole(value, spec.min, spec.max),
        ValueKind::Decimal => decimal(value, spec.min, spec.max),
        ValueKind::PairWhole | ValueKind::PairDecimal => {
            let Some((left, right)) = value.split_once(':') else {
                return false;
            };
            let valid = |part| {
                if spec.kind == ValueKind::PairWhole {
                    whole(part, spec.min, spec.max)
                } else {
                    decimal(part, spec.min, spec.max)
                }
            };
            valid(left) && valid(right)
        }
        ValueKind::Choice => spec.choices.contains(&value),
        ValueKind::ChoiceList => {
            let parts: Vec<_> = value.split(',').collect();
            !parts.is_empty()
                && parts.len() <= spec.choices.len()
                && parts.iter().all(|part| spec.choices.contains(part))
                && parts
                    .iter()
                    .enumerate()
                    .all(|(i, part)| !parts[..i].contains(part))
                && (spec.name != "partitions"
                    || parts.len() == 1
                    || !parts.iter().any(|part| *part == "all" || *part == "none"))
        }
    }
}

pub(super) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    validate_values(settings.encoder, settings.backend, &settings.parameters)
}

pub(crate) fn validate_values(
    encoder: VideoEncoder,
    backend: EncodeBackend,
    parameters: &[media_core::EncoderParameter],
) -> Result<(), AppError> {
    let maximum = if encoder.is_svt() || encoder == VideoEncoder::X264 {
        64
    } else {
        16
    };
    if parameters.len() > maximum {
        return Err(error(format!(
            "At most {maximum} encoder overrides are allowed."
        )));
    }
    if backend == EncodeBackend::Av1an
        && !encoder.is_svt()
        && encoder != VideoEncoder::X264
        && !parameters.is_empty()
    {
        return Err(error(
            "Av1an overrides require a supported SVT or x264 encoder.",
        ));
    }
    let mut names = std::collections::HashSet::new();
    for value in parameters {
        let spec = specs(encoder).into_iter().find(|spec| spec.name == value.name)
            .ok_or_else(|| error(format!("{} is not in this encoder's qualified parameter catalog. Input, output, timing, source filters, primary quality and pass mode, and process options belong to the application.", value.name)))?;
        if !names.insert(&value.name) {
            return Err(error(format!(
                "{} was supplied more than once.",
                value.name
            )));
        }
        if !valid_value(&spec, &value.value) {
            return Err(error(format!(
                "{} requires {}{}.",
                spec.label,
                match spec.kind {
                    ValueKind::Whole =>
                        format!("a whole number from {} through {}", spec.min, spec.max),
                    ValueKind::Decimal =>
                        format!("a decimal from {} through {}", spec.min, spec.max),
                    ValueKind::PairWhole => format!(
                        "two whole numbers from {} through {}, separated by ':'",
                        spec.min, spec.max
                    ),
                    ValueKind::PairDecimal => format!(
                        "two decimals from {} through {}, separated by ':'",
                        spec.min, spec.max
                    ),
                    ValueKind::Choice => format!("one of {}", spec.choices.join(", ")),
                    ValueKind::ChoiceList => format!(
                        "one or more distinct choices from {}, separated by ','",
                        spec.choices.join(", ")
                    ),
                },
                if spec.example.is_empty() {
                    String::new()
                } else {
                    format!(" Example: {}", spec.example)
                }
            )));
        }
    }
    Ok(())
}

/// Called only after validation. Ordering is deterministic and overrides follow
/// the speed preset while the application's protected options remain separate.
pub(super) fn arguments(settings: &EncodeSettings) -> Vec<OsString> {
    let mut args = Vec::new();
    for spec in specs(settings.encoder) {
        if let Some(value) = settings
            .parameters
            .iter()
            .find(|value| value.name == spec.name)
        {
            args.extend([spec.flag.into(), value.value.clone().into()]);
        }
    }
    args
}

pub(super) fn x265_suffix(settings: &EncodeSettings) -> String {
    arguments(settings)
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            format!(
                ":{}={}",
                pair[0].to_string_lossy(),
                pair[1].to_string_lossy()
            )
        })
        .collect()
}

fn advertised(help: &str, spec: &Spec, encoder: VideoEncoder) -> bool {
    help.split_whitespace().any(|word| {
        word == if encoder == VideoEncoder::X265 {
            "-x265-params"
        } else {
            spec.flag
        }
    })
}

pub(super) async fn check_help(
    path: &Path,
    settings: &EncodeSettings,
    help: &str,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    validate(settings)?;
    for value in &settings.parameters {
        let spec = specs(settings.encoder)
            .into_iter()
            .find(|spec| spec.name == value.name)
            .expect("validated catalog name");
        if !advertised(help, &spec, settings.encoder) {
            return Err(error(format!(
                "The installed {} build does not advertise {}. Remove that override or select a build that supports it.",
                settings.encoder.name(),
                spec.flag
            )));
        }
    }
    if settings.encoder == VideoEncoder::X265 && !settings.parameters.is_empty() {
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: path.to_owned(),
                args: [
                    "-v",
                    "warning",
                    "-nostdin",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=s=64x64:r=24:d=0.125",
                    "-frames:v",
                    "3",
                    "-an",
                    "-c:v",
                    "libx265",
                    "-preset",
                    "ultrafast",
                    "-x265-params",
                ]
                .into_iter()
                .map(OsString::from)
                .chain([format!(
                    "pools=1:frame-threads=1:log-level=warning{}",
                    x265_suffix(settings)
                )
                .into()])
                .chain(["-f", "null", "-"].into_iter().map(OsString::from))
                .collect(),
                cwd: None,
            },
            cancel.clone(),
            256 * 1024,
            Duration::from_secs(15),
        )
        .await
        .map_err(|e| process_error(e, path))?;
        let report = String::from_utf8_lossy(&result.stderr).to_ascii_lowercase();
        if !result.status.success()
            || [
                "unknown option",
                "unknown parameter",
                "invalid value",
                "error parsing",
                "unrecognized option",
            ]
            .iter()
            .any(|message| report.contains(message))
        {
            return Err(error(
                "The installed libx265 rejected the selected overrides during its bounded encoding probe. Remove the overrides reported as unsupported.",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn catalog(
    encoder: VideoEncoder,
    backend: EncodeBackend,
    cancel: watch::Receiver<bool>,
) -> Result<EncoderParameterCatalog, AppError> {
    let _permit = crate::analysis::permit(&cancel).await?;
    check_cancel(&cancel)?;
    if backend == EncodeBackend::Av1an && !encoder.is_svt() && encoder != VideoEncoder::X264 {
        return Err(error(
            "Choose a supported SVT or x264 encoder for av1an parameters.",
        ));
    }
    let path = crate::discovery::find_video_encoder(encoder)
        .await
        .map_err(error)?
        .ok_or_else(|| error("The selected encoder is not installed. Check Tools first."))?;
    let version = supervisor::run_capture(
        &CommandSpec {
            executable: path.clone(),
            args: vec![
                match encoder {
                    VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone => "--help",
                    VideoEncoder::X265Standalone => "--fullhelp",
                    encoder if encoder.is_ffmpeg() => "-version",
                    _ => "--version",
                }
                .into(),
            ],
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(5),
    )
    .await
    .map_err(|e| process_error(e, &path))?;
    let identity = format!(
        "{}\n{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    if !version.status.success() {
        return Err(error("The selected encoder failed its version check."));
    }
    crate::discovery::validate_video_encoder_version(encoder, &identity).map_err(error)?;
    let help_args = if encoder.is_ffmpeg() {
        vec![
            "-hide_banner".into(),
            "-h".into(),
            format!(
                "encoder={}",
                match encoder {
                    VideoEncoder::X265 => "libx265",
                    VideoEncoder::Vp9 => "libvpx-vp9",
                    VideoEncoder::H264Nvenc => "h264_nvenc",
                    VideoEncoder::HevcNvenc => "hevc_nvenc",
                    _ => unreachable!("FFmpeg encoder"),
                }
            )
            .into(),
        ]
    } else {
        vec![
            if encoder == VideoEncoder::X264 {
                "--fullhelp"
            } else {
                "--help"
            }
            .into(),
        ]
    };
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: path.clone(),
            args: help_args,
            cwd: None,
        },
        cancel.clone(),
        512 * 1024,
        Duration::from_secs(5),
    )
    .await
    .map_err(|e| process_error(e, &path))?;
    if !result.status.success() {
        return Err(error("The selected encoder failed its bounded help query."));
    }
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    check_cancel(&cancel)?;
    let parameters = specs(encoder)
        .iter()
        .filter(|spec| advertised(&help, spec, encoder))
        .map(|spec| EncoderParameterSpec {
            name: spec.name.into(),
            label: spec.label.into(),
            argument: spec.flag.into(),
            minimum: spec.min.max(0).min(u16::MAX.into()) as u16,
            maximum: spec.max.max(0).min(u16::MAX.into()) as u16,
            value_kind: spec.kind.name().into(),
            minimum_value: spec.min.to_string(),
            maximum_value: spec.max.to_string(),
            choices: spec.choices.iter().map(|choice| (*choice).into()).collect(),
            group: spec.group.into(),
            description: spec.description.into(),
            example: spec.example.into(),
        })
        .collect();
    Ok(EncoderParameterCatalog{encoder,backend,route:if encoder.is_ffmpeg(){"FFmpeg library"}else{"Standalone encoder CLI"}.into(),tool_path:path.to_string_lossy().into_owned(),tool_version:identity.lines().find(|line|!line.is_empty()).unwrap_or("unknown").into(),parameters,notes:vec!["The encoder speed preset applies first. Explicit overrides apply afterward. Missing overrides retain that build's preset defaults.".into(),"Only catalog values are accepted. Input/output paths, timing, source filters, primary quality and pass mode, and process flags remain application-controlled.".into(),if encoder==VideoEncoder::X265 {"Nested libx265 parameters receive a bounded real encoding probe before execution."} else {"Every requested flag must be advertised by the selected installed build before execution."}.into()]})
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_core::EncoderParameter;
    fn setting(encoder: VideoEncoder, name: &str, value: &str) -> EncodeSettings {
        EncodeSettings {
            encoder,
            parameters: vec![EncoderParameter {
                name: name.into(),
                value: value.into(),
            }],
            ..Default::default()
        }
    }
    #[test]
    fn only_catalog_scalars_can_reach_nested_or_native_parsers() {
        for name in [
            "-i",
            "--output",
            "fps",
            "--fps-num",
            "film-grain-denoise",
            "csv",
            "stats",
            "x265-params",
            "colorprim",
        ] {
            assert!(validate(&setting(VideoEncoder::X264, name, "1")).is_err());
        }
        for value in [
            "1 2",
            "1:csv=out",
            "$(touch)",
            "%TEMP%",
            "'1'",
            "1;exit",
            "-1",
            "65536",
        ] {
            assert!(validate(&setting(VideoEncoder::X265, "ref", value)).is_err());
        }
        let mut settings = setting(VideoEncoder::X264, "ref", "3");
        settings.parameters.push(settings.parameters[0].clone());
        assert!(validate(&settings).is_err());
        assert!(validate(&setting(VideoEncoder::X264, "sao", "0")).is_err());
        let x265 = setting(VideoEncoder::X265, "sao", "0");
        assert!(validate(&x265).is_ok());
        assert_eq!(x265_suffix(&x265), ":sao=0");
        let svt = setting(VideoEncoder::SvtAv1, "enable-tf", "0");
        assert_eq!(
            arguments(&svt),
            vec![OsString::from("--enable-tf"), OsString::from("0")]
        );
    }

    #[test]
    fn native_catalogs_have_typed_examples_and_no_path_parameters() {
        let x264 = specs(VideoEncoder::X264);
        let svt = specs(VideoEncoder::SvtAv1);
        assert_eq!(x264.len(), 31);
        assert_eq!(svt.len(), 43);
        for spec in x264.iter().chain(&svt) {
            assert!(spec.flag.starts_with("--"));
            assert!(valid_value(spec, spec.example), "{} example", spec.name);
            assert!(!spec.description.is_empty());
        }
        for name in [
            "fgs-table",
            "dolby-vision-rpu",
            "hdr10plus-json",
            "output",
            "input",
        ] {
            assert!(svt.iter().all(|spec| spec.name != name));
        }
    }

    #[test]
    fn x264_av1an_values_are_typed_and_deterministically_ordered() {
        let mut settings = setting(VideoEncoder::X264, "psy-rd", "1.2:0.15");
        settings.backend = EncodeBackend::Av1an;
        settings.parameters.push(media_core::EncoderParameter {
            name: "tune".into(),
            value: "grain".into(),
        });
        assert!(validate(&settings).is_ok());
        assert_eq!(
            arguments(&settings),
            ["--tune", "grain", "--psy-rd", "1.2:0.15"].map(OsString::from)
        );
        for bad in [
            "1:2:3",
            "1.2:../oops",
            "1.1234:0",
            "1e2:0",
            "$(calc)",
            "1:NaN",
        ] {
            assert!(
                validate(&setting(VideoEncoder::X264, "psy-rd", bad)).is_err(),
                "{bad}"
            );
        }
        for bad in ["film,film", "--grain", "GRain", "../grain", "film,unknown"] {
            assert!(
                validate(&setting(VideoEncoder::X264, "tune", bad)).is_err(),
                "{bad}"
            );
        }
        assert!(validate(&setting(VideoEncoder::X264, "tune", "film,fastdecode")).is_ok());
        assert!(validate(&setting(VideoEncoder::X264, "partitions", "p8x8,b8x8")).is_ok());
        assert!(validate(&setting(VideoEncoder::X264, "partitions", "all,b8x8")).is_err());
        assert!(validate(&setting(VideoEncoder::X264, "deblock", "-6:6")).is_ok());
        assert!(validate(&setting(VideoEncoder::X264, "deblock", "-7:0")).is_err());
        assert!(validate(&setting(VideoEncoder::X264, "ipratio", "1.0")).is_err());
    }

    #[test]
    fn svt_signed_decimal_and_installed_help_gates_are_exact() {
        assert!(validate(&setting(VideoEncoder::SvtAv1Hdr, "sharpness", "-7")).is_ok());
        assert!(validate(&setting(VideoEncoder::SvtAv1Hdr, "ac-bias", "4.125")).is_ok());
        assert!(validate(&setting(VideoEncoder::SvtAv1Hdr, "noise-size", "-2")).is_err());
        let spec = specs(VideoEncoder::SvtAv1Hdr)
            .into_iter()
            .find(|spec| spec.name == "ac-bias")
            .unwrap();
        assert!(advertised(
            "  --ac-bias  Strength",
            &spec,
            VideoEncoder::SvtAv1Hdr
        ));
        assert!(!advertised(
            "  --ac-bias-extra Strength",
            &spec,
            VideoEncoder::SvtAv1Hdr
        ));
    }

    #[test]
    fn previous_svt_overrides_remain_accepted_in_both_routes() {
        for backend in [EncodeBackend::Standalone, EncodeBackend::Av1an] {
            let mut settings = setting(VideoEncoder::SvtAv1, "aq-mode", "2");
            settings.backend = backend;
            settings.parameters.extend(
                [
                    ("enable-tf", "0"),
                    ("enable-overlays", "1"),
                    ("hierarchical-levels", "4"),
                ]
                .map(|(name, value)| media_core::EncoderParameter {
                    name: name.into(),
                    value: value.into(),
                }),
            );
            assert!(validate(&settings).is_ok());
        }
    }

    #[tokio::test]
    #[ignore = "requires selected installed x264 and SVT executables"]
    async fn selected_native_catalogs_advertise_typed_overrides() {
        for (encoder, expected) in [
            (VideoEncoder::X264, ["tune", "psy-rd", "partitions"]),
            (VideoEncoder::SvtAv1, ["tune", "aq-mode", "enable-tf"]),
        ] {
            let (_owner, cancel) = watch::channel(false);
            let catalog = catalog(encoder, EncodeBackend::Av1an, cancel)
                .await
                .unwrap();
            for name in expected {
                assert!(
                    catalog.parameters.iter().any(|spec| spec.name == name),
                    "{} lacks {name}",
                    catalog.tool_path
                );
            }
            assert!(
                catalog
                    .parameters
                    .iter()
                    .all(|spec| !spec.value_kind.is_empty())
            );
        }
    }
}
