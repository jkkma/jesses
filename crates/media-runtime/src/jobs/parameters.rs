//! A deliberately bounded scalar catalog. User strings never become option names,
//! paths, filters, shell commands, or nested parser separators.
use super::*;
use media_core::{EncodeBackend, EncoderParameterCatalog, EncoderParameterSpec, VideoEncoder};

#[derive(Clone, Copy)]
struct Spec {
    name: &'static str,
    label: &'static str,
    flag: &'static str,
    min: u16,
    max: u16,
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
        min,
        max,
    }
}

fn specs(encoder: VideoEncoder) -> Vec<Spec> {
    match encoder {
        VideoEncoder::X264 => vec![
            item("ref", "Reference frames", "--ref", 1, 6),
            item(
                "bframes",
                "Maximum consecutive B-frames",
                "--bframes",
                0,
                16,
            ),
            item("b-adapt", "B-frame adaptation", "--b-adapt", 0, 2),
            item("aq-mode", "Adaptive quantization mode", "--aq-mode", 0, 3),
            item("trellis", "Trellis quantization", "--trellis", 0, 2),
            item(
                "rc-lookahead",
                "Rate-control lookahead frames",
                "--rc-lookahead",
                0,
                100,
            ),
        ],
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => vec![
            item("aq-mode", "Adaptive quantization mode", "--aq-mode", 0, 2),
            item(
                "enable-tf",
                "Temporal filtering (0 off, 1 on)",
                "--enable-tf",
                0,
                1,
            ),
            item(
                "enable-overlays",
                "Overlay frames (0 off, 1 on)",
                "--enable-overlays",
                0,
                1,
            ),
            item(
                "hierarchical-levels",
                "Hierarchical prediction levels",
                "--hierarchical-levels",
                3,
                5,
            ),
        ],
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
    }
}

fn error(message: impl Into<String>) -> AppError {
    AppError::new("ENCODER_PARAMETERS_INVALID", message, None)
}

pub(super) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    validate_values(settings.encoder, settings.backend, &settings.parameters)
}

pub(crate) fn validate_values(
    encoder: VideoEncoder,
    backend: EncodeBackend,
    parameters: &[media_core::EncoderParameter],
) -> Result<(), AppError> {
    if parameters.len() > 16 {
        return Err(error("At most 16 encoder overrides are allowed."));
    }
    if backend == EncodeBackend::Av1an && !encoder.is_svt() && !parameters.is_empty() {
        return Err(error(
            "Av1an overrides currently require a supported standalone SVT encoder.",
        ));
    }
    let mut names = std::collections::HashSet::new();
    for value in parameters {
        let spec = specs(encoder).into_iter().find(|spec| spec.name == value.name)
            .ok_or_else(|| error(format!("{} is not in this encoder's qualified parameter catalog. Input, output, timing, color, rate control and process options belong to the application.", value.name)))?;
        if !names.insert(&value.name) {
            return Err(error(format!(
                "{} was supplied more than once.",
                value.name
            )));
        }
        let number = value.value.parse::<u16>().ok().filter(|number| {
            !value.value.is_empty()
                && value.value.len() <= 4
                && value.value.bytes().all(|byte| byte.is_ascii_digit())
                && (spec.min..=spec.max).contains(number)
        });
        if number.is_none() {
            return Err(error(format!(
                "{} requires a whole number from {} through {}.",
                spec.label, spec.min, spec.max
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
            args.extend([
                spec.flag.into(),
                value
                    .value
                    .parse::<u16>()
                    .expect("validated scalar")
                    .to_string()
                    .into(),
            ]);
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
    if backend == EncodeBackend::Av1an && !encoder.is_svt() {
        return Err(error(
            "Choose a supported SVT encoder for av1an parameters.",
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
                if encoder.is_ffmpeg() {
                    "-version"
                } else {
                    "--version"
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
                if encoder == VideoEncoder::X265 {
                    "libx265"
                } else {
                    "libvpx-vp9"
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
            minimum: spec.min,
            maximum: spec.max,
        })
        .collect();
    Ok(EncoderParameterCatalog{encoder,backend,route:if encoder.is_ffmpeg(){"FFmpeg library"}else{"Standalone encoder CLI"}.into(),tool_path:path.to_string_lossy().into_owned(),tool_version:identity.lines().find(|line|!line.is_empty()).unwrap_or("unknown").into(),parameters,notes:vec!["The encoder speed preset applies first. Explicit overrides apply afterward. Missing overrides retain that build's preset defaults.".into(),"Only catalog scalars are accepted. Input/output paths, timing, color, filters, rate control and process flags cannot be edited here.".into(),if encoder==VideoEncoder::X265 {"Nested libx265 parameters receive a bounded real encoding probe before execution."} else {"Every requested flag must be advertised by the selected installed build before execution."}.into()]})
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
}
