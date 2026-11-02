//! Bounded encoder catalogs. User strings never become option names or paths.
use super::*;
use media_core::{EncodeBackend, EncoderParameterCatalog, EncoderParameterSpec, VideoEncoder};
mod catalog;
mod extra_catalog;
mod source_specs;

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

fn merged(mut first: Vec<Spec>, rest: Vec<Spec>) -> Vec<Spec> {
    for spec in rest {
        if !first.iter().any(|old| old.name == spec.name) {
            first.push(spec);
        }
    }
    first
}

fn specs(encoder: VideoEncoder) -> Vec<Spec> {
    let mut values = match encoder {
        VideoEncoder::X264 => merged(catalog::x264(), source_specs::native_x264()),
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
            merged(catalog::svt(), source_specs::native_svt())
        }
        VideoEncoder::X265 => merged(
            source_specs::library_x265(),
            vec![item("b-adapt", "B-frame adaptation", "b-adapt", 0, 2)],
        ),
        VideoEncoder::Vp9 => source_specs::library_vpx(),
        VideoEncoder::X265Standalone => merged(extra_catalog::x265(), source_specs::native_x265()),
        VideoEncoder::AomAv1 => merged(extra_catalog::aom(), source_specs::native_aom()),
        VideoEncoder::VpxStandalone => extra_catalog::vpx(),
        VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc => Vec::new(),
    };
    for spec in &mut values {
        if let Some((group, description)) = source_specs::source_text(encoder, spec.name) {
            spec.group = group;
            spec.description = description;
        }
    }
    values
}

/// Library controls used by the ladder share the source-checked scalar catalog
/// with regular encoding. Resource paths, HDR metadata and grain are typed jobs.
fn library_specs(encoder: media_core::LadderEncoder) -> Vec<Spec> {
    use media_core::LadderEncoder as L;
    match encoder {
        L::H264 => source_specs::library_x264(),
        L::Hevc => merged(
            source_specs::library_x265(),
            vec![item("b-adapt", "B-frame adaptation", "b-adapt", 0, 2)],
        ),
        L::Av1 => merged(source_specs::library_svt(), catalog::svt()),
        L::Vp9 => source_specs::library_vpx(),
        L::AomAv1 => source_specs::library_aom(),
        L::H264Nvenc | L::HevcNvenc => Vec::new(),
    }
}

pub(crate) fn library_parameter(
    encoder: media_core::LadderEncoder,
    value: &media_core::EncoderParameter,
) -> Result<media_core::EncoderParameter, AppError> {
    encoder.require_cpu_encoding()?;
    let spec = library_specs(encoder)
        .into_iter()
        .find(|spec| spec.name == value.name)
        .ok_or_else(|| {
            error(format!(
                "{} is not a supported scalar control for this library encoder.",
                value.name
            ))
        })?;
    let normalized = if matches!(spec.kind, ValueKind::PairWhole | ValueKind::PairDecimal) {
        value.value.replace(',', ":")
    } else {
        value.value.clone()
    };
    if !valid_value(&spec, &normalized)
        || (encoder == media_core::LadderEncoder::Hevc
            && spec.name == "qcomp"
            && normalized.parse::<f64>().is_ok_and(|value| value < 0.5))
    {
        return Err(error(format!(
            "{} has an invalid value for this library encoder.",
            value.name
        )));
    }
    if matches!(
        value.name.as_str(),
        "drop-threshold" | "lossless" | "highbitdepth"
    ) && normalized != "0"
    {
        return Err(error(
            "Frame dropping, lossless and bit-depth changes must use the job's typed settings; a CRF ladder keeps its sample frame count and format fixed.",
        ));
    }
    if matches!(value.name.as_str(), "rc" | "tune")
        && matches!(normalized.as_str(), "constqp" | "cbr" | "lossless")
    {
        return Err(error(
            "This value changes the ladder's CRF rate-control mode. Choose a regular encoding job for that mode.",
        ));
    }
    Ok(media_core::EncoderParameter {
        name: value.name.clone(),
        value: normalized,
    })
}

pub(crate) fn library_arguments(
    encoder: media_core::LadderEncoder,
    values: &[media_core::EncoderParameter],
) -> Vec<OsString> {
    use media_core::LadderEncoder as L;
    if values.is_empty() && encoder != L::Av1 {
        return Vec::new();
    }
    if matches!(encoder, L::H264 | L::Hevc | L::Av1 | L::AomAv1) {
        let flag = match encoder {
            L::H264 => "-x264-params",
            L::Hevc => "-x265-params",
            L::Av1 => "-svtav1-params",
            _ => "-aom-params",
        };
        let mut parameters = values
            .iter()
            .map(|item| format!("{}={}", item.name, item.value.replace(':', ",")))
            .collect::<Vec<_>>();
        if encoder == L::Av1 {
            // Bound utility resource use just as ordinary standalone workers do.
            // SVT4.2 high-bit-depth mode decision plus overlays can crash with
            // parallel short clips; level1 preserves both selected controls.
            parameters.insert(0, "lp=1".into());
        }
        vec![flag.into(), parameters.join(":").into()]
    } else {
        values
            .iter()
            .flat_map(|item| [format!("-{}", item.name).into(), item.value.clone().into()])
            .collect()
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

/// SVT4.2's high-bit-depth mode decision plus overlays crashes in parallel
/// short-clip execution. Both features remain enabled with one processing lane.
pub(in crate::jobs) fn requires_serial_svt(settings: &EncodeSettings) -> bool {
    settings.encoder.is_svt()
        && ["hbd-mds", "enable-overlays"].iter().all(|name| {
            settings
                .parameters
                .iter()
                .any(|value| value.name == *name && value.value == "1")
        })
}

pub(super) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    validate_values(settings.encoder, settings.backend, &settings.parameters)?;
    if settings.encoder.is_svt()
        && settings
            .parameters
            .iter()
            .any(|value| value.name == "hbd-mds" && value.value == "1")
        && settings
            .output_pixel_format
            .or_else(|| {
                settings
                    .av1an_options
                    .and_then(|options| options.pixel_format)
            })
            .is_some_and(|format| !format.ffmpeg().contains("10"))
    {
        return Err(error(
            "SVT hbd-mds=1 requires 10-bit output. Choose a 10-bit output format or remove that content-preset override before queueing.",
        ));
    }
    if let Some(quantizer) = settings.constant_quantizer
        && (quantizer > 51
            || !matches!(
                settings.encoder,
                VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc
            )
            || settings.lossless
            || settings.rate_control.is_some())
    {
        return Err(error(
            "Constant quantizer requires NVENC, a QP from 0 through 51, and no lossless or target-bitrate/size mode.",
        ));
    }
    for parameter in &settings.parameters {
        match (parameter.name.as_str(), parameter.value.as_str()) {
            ("drop-threshold", value) if value != "0" => {
                return Err(error(
                    "Frame dropping would violate the job's planned frame count; drop-threshold must be 0.",
                ));
            }
            ("lossless", value) if (value == "1") != settings.lossless => {
                return Err(error(
                    "Set lossless through the main encoding control so the planned verification matches the encoder.",
                ));
            }
            ("rc", "constqp")
                if !settings.lossless
                    && settings.constant_quantizer.is_none()
                    && !settings.source_nvenc_quality =>
            {
                return Err(error(
                    "constqp requires the job's constant-quantizer or lossless mode; primary quality is controlled by the encoding settings.",
                ));
            }
            ("rc", "vbr" | "cbr") if settings.constant_quantizer.is_some() => {
                return Err(error(
                    "The selected rate-control override conflicts with the job's constant-quantizer mode.",
                ));
            }
            ("rc", "cbr") if settings.rate_control.is_none() && !settings.source_nvenc_quality => {
                return Err(error(
                    "CBR requires a target bitrate or size in the main encoding settings.",
                ));
            }
            ("tune", "lossless") if !settings.lossless && !settings.source_nvenc_quality => {
                return Err(error(
                    "Select lossless mode before using the lossless encoder tune.",
                ));
            }
            ("highbitdepth", "1")
                if !matches!(
                    settings.output_pixel_format,
                    Some(
                        media_core::Av1anPixelFormat::Yuv420p10le
                            | media_core::Av1anPixelFormat::Yuv422p10le
                            | media_core::Av1anPixelFormat::Yuv444p10le
                    )
                ) =>
            {
                return Err(error(
                    "highbitdepth requires an explicitly planned 10-bit output pixel format.",
                ));
            }
            ("vbr-corpus-complexity-lap", value)
                if value != "0"
                    && !matches!(
                        settings.rate_control,
                        Some(media_core::VideoRateControl::Bitrate {
                            two_pass: false,
                            ..
                        })
                    ) =>
            {
                return Err(error(
                    "Corpus complexity requires single-pass target-bitrate mode.",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn validate_values(
    encoder: VideoEncoder,
    backend: EncodeBackend,
    parameters: &[media_core::EncoderParameter],
) -> Result<(), AppError> {
    encoder.require_cpu_encoding()?;
    let maximum = 64;
    if parameters.len() > maximum {
        return Err(error(format!(
            "At most {maximum} encoder overrides are allowed."
        )));
    }
    if backend == EncodeBackend::Av1an
        && !matches!(
            encoder,
            VideoEncoder::SvtAv1
                | VideoEncoder::SvtAv1FiveFish
                | VideoEncoder::SvtAv1Hdr
                | VideoEncoder::X264
                | VideoEncoder::AomAv1
                | VideoEncoder::VpxStandalone
                | VideoEncoder::X265Standalone
        )
        && !parameters.is_empty()
    {
        return Err(error(
            "Av1an overrides require a supported standalone encoder.",
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
        if matches!(encoder, VideoEncoder::X265 | VideoEncoder::X265Standalone)
            && spec.name == "qcomp"
            && value.value.parse::<f64>().is_ok_and(|value| value < 0.5)
        {
            return Err(error("x265 qcomp requires a decimal from 0.5 through 1."));
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
            if settings.encoder == VideoEncoder::AomAv1 && spec.name == "disable-kf" {
                args.push("--disable-kf".into());
            } else if matches!(
                settings.encoder,
                VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone
            ) {
                args.push(format!("{}={}", spec.flag, value.value).into());
            } else if (settings.encoder == VideoEncoder::X265Standalone
                && matches!(spec.name, "sao" | "cutree" | "rc-grain" | "hdr10-opt"))
                || (settings.encoder == VideoEncoder::X264
                    && matches!(
                        spec.name,
                        "cabac" | "8x8dct" | "psy" | "dct-decimate" | "mbtree"
                    ))
            {
                args.push(
                    if value.value == "0" {
                        format!("--no-{}", spec.name)
                    } else {
                        spec.flag.into()
                    }
                    .into(),
                );
            } else {
                args.extend([spec.flag.into(), value.value.clone().into()]);
            }
        }
    }
    args
}

pub(super) fn x265_suffix(settings: &EncodeSettings) -> String {
    specs(settings.encoder)
        .into_iter()
        .filter_map(|spec| {
            settings
                .parameters
                .iter()
                .find(|value| value.name == spec.name)
                .map(|value| format!(":{}={}", spec.name, value.value.replace(':', ",")))
        })
        .collect()
}

fn advertised(help: &str, spec: &Spec, encoder: VideoEncoder) -> bool {
    if encoder == VideoEncoder::X265 {
        return help.split_whitespace().any(|word| word == "-x265-params");
    }
    // These are FFmpeg's generic video codec options, absent from encoder-only help.
    if encoder.is_ffmpeg() && matches!(spec.name, "g" | "bf" | "refs" | "qmin" | "qmax") {
        return true;
    }
    let wanted = spec.flag.trim_end_matches(":v");
    help.split_whitespace().any(|word| {
        word.split('/').any(|alias| {
            let flag = alias.split('=').next().unwrap_or(alias);
            flag == wanted
                || flag.replace("[no-]", "") == wanted
                || (encoder == VideoEncoder::X264
                    && flag
                        .strip_prefix("--no-")
                        .is_some_and(|name| wanted.strip_prefix("--") == Some(name)))
        })
    })
}

fn installed_choices<'a>(help: &str, spec: &'a Spec, encoder: VideoEncoder) -> Vec<&'a str> {
    if !matches!(spec.kind, ValueKind::Choice | ValueKind::ChoiceList) {
        return Vec::new();
    }
    let wanted = spec.flag.trim_end_matches(":v");
    let lines = help.lines().collect::<Vec<_>>();
    let start = lines.iter().position(|line| {
        line.split_whitespace()
            .any(|word| word.split('=').next() == Some(wanted))
    });
    let Some(start) = start else {
        return spec.choices.to_vec();
    };
    let block = lines[start + 1..]
        .iter()
        .take_while(|line| !line.trim_start().starts_with('-'))
        .copied()
        .collect::<Vec<_>>();
    let advertised = if encoder.is_ffmpeg() {
        block
            .iter()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let name = fields.next()?;
                fields.next()?.parse::<i64>().ok()?;
                Some(name)
            })
            .collect::<Vec<_>>()
    } else if matches!(encoder, VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone) {
        block
            .iter()
            .flat_map(|line| line.split(|c: char| c.is_whitespace() || c == ','))
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if advertised.is_empty() {
        return spec.choices.to_vec();
    }
    spec.choices
        .iter()
        .copied()
        .filter(|choice| advertised.contains(choice))
        .collect()
}

pub(super) async fn check_help(
    path: &Path,
    settings: &EncodeSettings,
    help: &str,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    validate(settings)?;
    if settings.constant_quantizer.is_some() && !help.split_whitespace().any(|word| word == "-qp") {
        return Err(error(
            "The installed NVENC wrapper does not advertise the explicit -qp control.",
        ));
    }
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
        if matches!(spec.kind, ValueKind::Choice | ValueKind::ChoiceList) {
            let choices = installed_choices(help, &spec, settings.encoder);
            if !value.value.split(',').all(|part| choices.contains(&part)) {
                return Err(error(format!(
                    "The installed {} does not advertise {}={} among this build's supported choices.",
                    settings.encoder.name(),
                    spec.name,
                    value.value
                )));
            }
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
    encoder.require_cpu_encoding()?;
    let _permit = crate::analysis::permit(&cancel).await?;
    check_cancel(&cancel)?;
    if backend == EncodeBackend::Av1an
        && !matches!(
            encoder,
            VideoEncoder::SvtAv1
                | VideoEncoder::SvtAv1FiveFish
                | VideoEncoder::SvtAv1Hdr
                | VideoEncoder::X264
                | VideoEncoder::AomAv1
                | VideoEncoder::VpxStandalone
                | VideoEncoder::X265Standalone
        )
    {
        return Err(error(
            "Choose a supported standalone encoder for av1an parameters.",
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
            if matches!(encoder, VideoEncoder::X264 | VideoEncoder::X265Standalone) {
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
            minimum_value: if spec.name == "qcomp"
                && matches!(encoder, VideoEncoder::X265 | VideoEncoder::X265Standalone)
            {
                "0.5".into()
            } else {
                spec.min.to_string()
            },
            maximum_value: spec.max.to_string(),
            choices: installed_choices(&help, spec, encoder)
                .into_iter()
                .map(String::from)
                .collect(),
            group: spec.group.into(),
            description: spec.description.into(),
            example: spec.example.into(),
        })
        .collect();
    Ok(EncoderParameterCatalog{encoder,backend,route:if encoder.is_ffmpeg(){"FFmpeg library"}else if backend == EncodeBackend::Av1an {"av1an child encoder CLI"} else {"Standalone encoder CLI"}.into(),tool_path:path.to_string_lossy().into_owned(),tool_version:identity.lines().find(|line|!line.is_empty()).unwrap_or("unknown").into(),parameters,notes:vec!["The encoder speed preset applies first. Explicit overrides apply afterward. Missing overrides retain that build's preset defaults.".into(),"Only catalog values are accepted. Input/output paths, timing, source filters, primary quality and pass mode, and process flags remain application-controlled.".into(),if encoder==VideoEncoder::X265 {"Nested libx265 parameters receive a bounded real encoding probe before execution."} else {"Every requested flag must be advertised by the selected installed build before execution."}.into()]})
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
        let aom = setting(VideoEncoder::AomAv1, "disable-kf", "1");
        assert!(validate(&aom).is_ok());
        assert_eq!(arguments(&aom), vec![OsString::from("--disable-kf")]);
        assert!(validate(&setting(VideoEncoder::AomAv1, "disable-kf", "0")).is_err());
        for (encoder, name, value) in [
            (VideoEncoder::AomAv1, "kf-min-dist", "12"),
            (VideoEncoder::AomAv1, "kf-max-dist", "240"),
            (VideoEncoder::AomAv1, "tile-columns", "1"),
            (VideoEncoder::VpxStandalone, "kf-max-dist", "240"),
            (VideoEncoder::VpxStandalone, "tile-rows", "0"),
            (VideoEncoder::SvtAv1, "keyint", "240"),
        ] {
            let settings = setting(encoder, name, value);
            assert!(validate(&settings).is_ok(), "{encoder:?} {name}");
        }
    }

    #[test]
    fn native_catalogs_have_typed_examples_and_no_path_parameters() {
        let x264 = specs(VideoEncoder::X264);
        let svt = specs(VideoEncoder::SvtAv1);
        assert_eq!(x264.len(), 37);
        assert_eq!(svt.len(), 45);
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

    #[test]
    fn extra_native_help_aliases_and_boolean_arguments_match_cli_syntax() {
        for (encoder, name, help) in [
            (
                VideoEncoder::AomAv1,
                "sharpness",
                "--sharpness=<arg> Sharpness",
            ),
            (VideoEncoder::VpxStandalone, "tune", "--tune=<arg> Tuning"),
            (VideoEncoder::X265Standalone, "tune", "-t/--tune <string>"),
            (VideoEncoder::X265Standalone, "sao", "--[no-]sao Enable SAO"),
        ] {
            let spec = specs(encoder).into_iter().find(|s| s.name == name).unwrap();
            assert!(advertised(help, &spec, encoder));
            assert!(!advertised(&format!("{}-other", spec.flag), &spec, encoder));
        }
        for value in ["0", "1"] {
            let settings = setting(VideoEncoder::X265Standalone, "sao", value);
            assert!(validate(&settings).is_ok());
            assert_eq!(
                arguments(&settings),
                [OsString::from(if value == "0" {
                    "--no-sao"
                } else {
                    "--sao"
                })]
            );
        }
        assert!(validate(&setting(VideoEncoder::X265Standalone, "deblock", "-1:1")).is_ok());
        assert!(validate(&setting(VideoEncoder::X265Standalone, "psy-rd", "1.5")).is_ok());
        assert!(validate(&setting(VideoEncoder::AomAv1, "tune", "psnr")).is_ok());
        assert_eq!(
            arguments(&setting(VideoEncoder::AomAv1, "tune", "psnr")),
            [OsString::from("--tune=psnr")]
        );
        assert!(
            validate(&setting(
                VideoEncoder::VpxStandalone,
                "tune-content",
                "screen"
            ))
            .is_ok()
        );
        assert!(
            validate(&setting(
                VideoEncoder::VpxStandalone,
                "tune",
                "psnr --output bad"
            ))
            .is_err()
        );
    }

    #[test]
    fn every_fork_catalog_row_has_an_explicit_scalar_or_typed_resource_disposition() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/nmkoder-encoder-catalog.json"
        ))
        .unwrap();
        let mut total = 0;
        let mut scalar = 0;
        let mut resources = 0;
        let mut excluded_hardware = 0;
        for catalog in fixture["catalogs"].as_array().unwrap() {
            let file = catalog["sourceFile"].as_str().unwrap();
            let name = Path::new(file).file_stem().unwrap().to_str().unwrap();
            let library = catalog["sourceWrapper"] == "ffmpeg";
            if matches!(name, "H264Nvenc" | "H265Nvenc") {
                excluded_hardware += catalog["options"].as_array().unwrap().len();
                continue;
            }
            let values = if library {
                library_specs(match name {
                    "Libx264" => media_core::LadderEncoder::H264,
                    "Libx265" => media_core::LadderEncoder::Hevc,
                    "LibSvtAv1" => media_core::LadderEncoder::Av1,
                    "LibAomAv1" => media_core::LadderEncoder::AomAv1,
                    "LibVpx" => media_core::LadderEncoder::Vp9,
                    "H264Nvenc" => media_core::LadderEncoder::H264Nvenc,
                    "H265Nvenc" => media_core::LadderEncoder::HevcNvenc,
                    _ => panic!("unmapped source catalog {name}"),
                })
            } else {
                specs(match name {
                    "X264" => VideoEncoder::X264,
                    "X265" => VideoEncoder::X265Standalone,
                    "SvtAv1" => VideoEncoder::SvtAv1,
                    "AomAv1" => VideoEncoder::AomAv1,
                    "Vpx" => VideoEncoder::VpxStandalone,
                    _ => panic!("unmapped source catalog {name}"),
                })
            };
            for row in catalog["options"].as_array().unwrap() {
                total += 1;
                let key = row["option"].as_str().unwrap();
                if let Some(spec) = values.iter().find(|spec| spec.name == key) {
                    scalar += 1;
                    if let Some(bounds) = row["rangeBounds"].as_array() {
                        for bound in bounds {
                            let value = bound.as_str().unwrap();
                            if matches!(spec.kind, ValueKind::Whole | ValueKind::Decimal) {
                                assert!(
                                    valid_value(spec, value),
                                    "{file}/{key}: source bound {value} must be represented"
                                );
                            }
                        }
                    }
                    assert!(
                        valid_value(spec, spec.example) || spec.example.is_empty(),
                        "{file}/{key} example: {}",
                        spec.example
                    );
                } else {
                    assert!(
                        matches!(
                            (name, key),
                            (
                                "SvtAv1",
                                "fgs-table" | "dolby-vision-rpu" | "hdr10plus-json"
                            ) | ("X265" | "Libx265", "master-display" | "max-cll")
                                | ("LibSvtAv1", "film-grain" | "film-grain-denoise")
                                | ("LibAomAv1", "denoise-noise-level" | "enable-dnl-denoising")
                        ),
                        "{file}/{key} has no disposition"
                    );
                    resources += 1;
                }
            }
        }
        assert_eq!(total + excluded_hardware, 375);
        assert_eq!(resources, 11);
        assert_eq!(scalar + resources, total);
        assert_eq!(excluded_hardware, 67);
    }

    #[test]
    fn expanded_scalar_catalogs_preserve_cli_and_nested_value_semantics() {
        for name in ["cabac", "8x8dct", "psy", "dct-decimate", "mbtree"] {
            for value in ["0", "1"] {
                let setting = setting(VideoEncoder::X264, name, value);
                assert!(validate(&setting).is_ok());
                assert_eq!(
                    arguments(&setting),
                    [OsString::from(format!(
                        "--{}{name}",
                        if value == "0" { "no-" } else { "" }
                    ))]
                );
                assert!(advertised(
                    &format!("--no-{name}"),
                    &specs(VideoEncoder::X264)
                        .into_iter()
                        .find(|spec| spec.name == name)
                        .unwrap(),
                    VideoEncoder::X264
                ));
            }
        }
        let pair = setting(VideoEncoder::X265, "deblock", "-2:1");
        assert!(validate(&pair).is_ok());
        assert_eq!(x265_suffix(&pair), ":deblock=-2,1");
        assert!(validate(&setting(VideoEncoder::X265, "qcomp", "0.49")).is_err());
        assert!(validate(&setting(VideoEncoder::X265, "qcomp", "0.5")).is_ok());
        assert!(validate(&setting(VideoEncoder::Vp9, "drop-threshold", "1")).is_err());
        assert!(validate(&setting(VideoEncoder::Vp9, "drop-threshold", "0")).is_ok());
        assert!(validate(&setting(VideoEncoder::H264Nvenc, "rc", "cbr")).is_err());
        assert_eq!(
            validate(&setting(VideoEncoder::H264Nvenc, "rc", "cbr"))
                .unwrap_err()
                .code,
            "HARDWARE_ENCODING_UNSUPPORTED"
        );
        for encoder in [
            media_core::LadderEncoder::H264,
            media_core::LadderEncoder::Hevc,
        ] {
            let parameter = media_core::EncoderParameter {
                name: "deblock".into(),
                value: "-2,1".into(),
            };
            let normalized = library_parameter(encoder, &parameter).unwrap();
            assert_eq!(normalized.value, "-2:1");
            let args = library_arguments(encoder, &[normalized]);
            assert_eq!(args[1], "deblock=-2,1");
        }
    }

    #[test]
    fn svt_high_bit_depth_preset_constraint_is_rejected_before_queueing() {
        let mut settings = setting(VideoEncoder::SvtAv1, "hbd-mds", "1");
        settings.parameters.push(media_core::EncoderParameter {
            name: "enable-overlays".into(),
            value: "1".into(),
        });
        assert!(requires_serial_svt(&settings));
        settings.output_pixel_format = Some(media_core::Av1anPixelFormat::Yuv420p);
        assert!(validate(&settings).unwrap_err().message.contains("10-bit"));
        settings.output_pixel_format = Some(media_core::Av1anPixelFormat::Yuv420p10le);
        validate(&settings).unwrap();
        settings.parameters.pop();
        assert!(!requires_serial_svt(&settings));
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
