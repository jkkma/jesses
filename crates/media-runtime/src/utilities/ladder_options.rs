//! Fork content presets are shared with the UI and admitted by real wrapper probes.
use super::*;
use media_core::EncoderParameter;
#[derive(Deserialize)]
struct ContentPreset {
    name: String,
    parameters: Vec<EncoderParameter>,
}
pub(super) struct Prepared {
    pub arguments: Vec<OsString>,
    pub parameters: Vec<EncoderParameter>,
    pub diagnostics: Vec<String>,
}
fn family(encoder: LadderEncoder) -> Option<&'static str> {
    match encoder {
        LadderEncoder::H264 => Some("x264"),
        LadderEncoder::Hevc => Some("x265"),
        LadderEncoder::Av1 => Some("svtAv1"),
        _ => None,
    }
}
fn presets() -> Result<BTreeMap<String, Vec<ContentPreset>>, AppError> {
    serde_json::from_str(include_str!("../../../../src/lib/content-presets.json"))
        .map_err(|cause| error("UTILITY_PRESET_INVALID", cause.to_string(), None))
}
fn normalize(
    encoder: LadderEncoder,
    parameter: &EncoderParameter,
) -> Result<EncoderParameter, AppError> {
    crate::jobs::parameters::library_parameter(encoder, parameter)
}
fn arguments(encoder: LadderEncoder, values: &[EncoderParameter]) -> Vec<OsString> {
    crate::jobs::parameters::library_arguments(encoder, values)
}
fn probe_args(request: &CrfLadderRequest, values: &[EncoderParameter]) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-v",
        "warning",
        "-nostdin",
        "-filter_threads",
        "1",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=128x128:r=24",
        "-frames:v",
        "24",
        "-pix_fmt",
        &request.pixel_format,
        "-c:v",
        codec_name(request.encoder),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    match request.encoder {
        LadderEncoder::H264
        | LadderEncoder::Hevc
        | LadderEncoder::Av1
        | LadderEncoder::H264Nvenc
        | LadderEncoder::HevcNvenc => {
            args.extend(["-preset".into(), request.preset.clone().into()])
        }
        LadderEncoder::Vp9 | LadderEncoder::AomAv1 => {
            args.extend(["-cpu-used".into(), request.preset.clone().into()])
        }
    }
    args.extend(arguments(request.encoder, values));
    args.extend(["-f".into(), "null".into(), "-".into()]);
    args
}
async fn accepted(
    ffmpeg: &Path,
    request: &CrfLadderRequest,
    values: &[EncoderParameter],
    cancel: &watch::Receiver<bool>,
) -> Result<bool, AppError> {
    let result = capture(
        ffmpeg.to_owned(),
        probe_args(request, values),
        cancel,
        Duration::from_secs(60),
        Some(Path::new(&request.input_path)),
    )
    .await?;
    let text = String::from_utf8_lossy(&result.stderr).to_lowercase();
    Ok(result.status.success()
        && ![
            "error parsing",
            "unknown parameter",
            "unknown option",
            "unrecognized option",
            "failed to parse",
            "invalid parameter",
            "unsupported parameter",
            "incompatible pixel format",
            "auto-selecting",
        ]
        .iter()
        .any(|message| text.contains(message)))
}
pub(super) async fn prepare(
    ffmpeg: &Path,
    request: &CrfLadderRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<Prepared, AppError> {
    if request.parameters.len() > 64 {
        return Err(error(
            "UTILITY_LADDER_PARAMETER_INVALID",
            "Use at most 64 content controls.",
            None,
        ));
    }
    let mut values = BTreeMap::<String, EncoderParameter>::new();
    let mut preset_names = BTreeSet::new();
    if let Some(name) = request
        .content_preset
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        let all = presets()?;
        let preset = family(request.encoder)
            .and_then(|family| all.get(family))
            .and_then(|presets| presets.iter().find(|preset| preset.name == name))
            .ok_or_else(|| {
                error(
                    "UTILITY_LADDER_PRESET_INVALID",
                    "Choose a content preset offered for the selected ladder encoder.",
                    None,
                )
            })?;
        for parameter in &preset.parameters {
            let parameter = normalize(request.encoder, parameter)?;
            preset_names.insert(parameter.name.clone());
            values.insert(parameter.name.clone(), parameter);
        }
    }
    let mut supplied = BTreeSet::new();
    for parameter in &request.parameters {
        if !supplied.insert(parameter.name.clone()) {
            return Err(error(
                "UTILITY_LADDER_PARAMETER_INVALID",
                "A content control was supplied more than once.",
                None,
            ));
        }
        let parameter = normalize(request.encoder, parameter)?;
        values.insert(parameter.name.clone(), parameter);
    }
    let mut applied = Vec::new();
    let mut diagnostics = Vec::new();
    if request.encoder == LadderEncoder::Av1 {
        diagnostics.push("SVT sample encoding uses one processing lane (lp=1). This bounds short-clip resource use and preserves high-bit-depth mode decision plus overlays without the observed SVT4.2 parallel execution crash.".into());
    }
    for parameter in values.into_values() {
        if accepted(ffmpeg, request, std::slice::from_ref(&parameter), cancel).await? {
            applied.push(parameter);
        } else if preset_names.contains(&parameter.name) {
            diagnostics.push(format!("Content preset control {}={} was omitted because the installed {} wrapper rejected it.", parameter.name, parameter.value, codec_name(request.encoder)));
        } else {
            return Err(error(
                "UTILITY_LADDER_PARAMETER_UNSUPPORTED",
                format!(
                    "The installed {} rejected {}={}. Remove this override.",
                    codec_name(request.encoder),
                    parameter.name,
                    parameter.value
                ),
                None,
            ));
        }
    }
    if (!applied.is_empty()
        || matches!(
            request.encoder,
            LadderEncoder::H264Nvenc | LadderEncoder::HevcNvenc
        ))
        && !accepted(ffmpeg, request, &applied, cancel).await?
    {
        return Err(error(
            if matches!(
                request.encoder,
                LadderEncoder::H264Nvenc | LadderEncoder::HevcNvenc
            ) {
                "NVENC_HARDWARE_UNAVAILABLE"
            } else {
                "UTILITY_LADDER_PARAMETER_UNSUPPORTED"
            },
            "The installed encoder rejected this ladder's complete settings during its bounded initialization probe.",
            None,
        ));
    }
    if let Some(name) = &request.content_preset {
        diagnostics.push(format!("Content preset {name}: applied {} supported controls. Every applied value is retained with the result.", applied.len()));
    }
    Ok(Prepared {
        arguments: arguments(request.encoder, &applied),
        parameters: applied,
        diagnostics,
    })
}

pub(super) fn retain_samples(
    output: Option<&str>,
    scratch: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<String>, AppError> {
    let Some(text) = output else {
        return Ok(None);
    };
    let path = valid_absolute(text)?;
    ensure_absent(path)?;
    let parent = path
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .ok_or_else(|| {
            error(
                "INVALID_OUTPUT",
                "Choose an existing parent for a new samples directory.",
                Some(path),
            )
        })?;
    let output = parent.join(
        path.file_name()
            .ok_or_else(|| error("INVALID_OUTPUT", "Choose a directory name.", Some(path)))?,
    );
    let mut staging = crate::jobs::images::Directory::new(&parent)?;
    let mut samples = fs::read_dir(scratch)
        .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(scratch)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(scratch)))?;
    samples.sort_by_key(|entry| entry.file_name());
    for entry in samples {
        check_cancel(cancel)?;
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            return Err(error(
                "UTILITY_SAMPLE_CHANGED",
                "A validated sample was replaced.",
                Some(&entry.path()),
            ));
        }
        let mut input = fs::File::open(entry.path())
            .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(&entry.path())))?;
        let mut target = staging.create_file(staging.path.join(entry.file_name()))?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            check_cancel(cancel)?;
            let count = input.read(&mut buffer).map_err(|cause| {
                error("OUTPUT_UNREADABLE", cause.to_string(), Some(&entry.path()))
            })?;
            if count == 0 {
                break;
            }
            target.write_all(&buffer[..count]).map_err(|cause| {
                error(
                    "OUTPUT_WRITE_FAILED",
                    cause.to_string(),
                    Some(&staging.path),
                )
            })?;
        }
        target.sync_all().map_err(|cause| {
            error(
                "OUTPUT_WRITE_FAILED",
                cause.to_string(),
                Some(&staging.path),
            )
        })?;
    }
    check_cancel(cancel)?;
    staging.publish(&output)?;
    Ok(Some(output.to_string_lossy().into_owned()))
}

/// Source ReportOutput counts every retained file, including references,
/// encoded rungs and scorer diagnostics, rather than summing projected sizes.
pub(super) fn retained_size(path: &Path, cancel: &watch::Receiver<bool>) -> Result<u64, AppError> {
    let root = fs::symlink_metadata(path)
        .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(path)))?;
    if !root.is_dir() || root.file_type().is_symlink() {
        return Err(error(
            "UTILITY_SAMPLE_CHANGED",
            "The retained directory was redirected.",
            Some(path),
        ));
    }
    let mut directories = vec![path.to_owned()];
    let mut total = 0_u64;
    while let Some(directory) = directories.pop() {
        check_cancel(cancel)?;
        for entry in fs::read_dir(&directory)
            .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(&directory)))?
        {
            check_cancel(cancel)?;
            let entry = entry
                .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(&directory)))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(&path)))?;
            let redirected = metadata.file_type().is_symlink();
            #[cfg(windows)]
            let redirected = {
                use std::os::windows::fs::MetadataExt;
                redirected || metadata.file_attributes() & 0x400 != 0
            };
            if redirected {
                return Err(error(
                    "UTILITY_SAMPLE_CHANGED",
                    "A retained sample path was redirected.",
                    Some(&path),
                ));
            }
            if metadata.is_dir() {
                directories.push(path);
            } else if metadata.is_file() {
                total = total.checked_add(metadata.len()).ok_or_else(|| {
                    error(
                        "OUTPUT_UNREADABLE",
                        "Retained directory byte count overflowed.",
                        Some(&path),
                    )
                })?;
            } else {
                return Err(error(
                    "UTILITY_SAMPLE_CHANGED",
                    "A retained sample is not a regular file.",
                    Some(&path),
                ));
            }
        }
    }
    Ok(total)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_directory_measurement_counts_every_extension_and_nested_file() {
        let scratch = Scratch::create("retained-size-test").unwrap();
        fs::write(scratch.path.join("reference.mkv"), b"reference").unwrap();
        fs::write(scratch.path.join("candidate.mkv"), b"encoded").unwrap();
        fs::write(scratch.path.join("score.json"), b"{}").unwrap();
        let nested = scratch.path.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("log.txt"), b"tail").unwrap();
        let (owner, cancel) = watch::channel(false);
        assert_eq!(retained_size(&scratch.path, &cancel).unwrap(), 22);
        assert!(
            retain_samples(None, &scratch.path, &cancel)
                .unwrap()
                .is_none()
        );
        owner.send_replace(true);
        assert_eq!(
            retained_size(&scratch.path, &cancel).unwrap_err().code,
            "ANALYSIS_CANCELLED"
        );
        fs::remove_dir_all(nested).unwrap();
    }
    #[test]
    fn shared_fork_presets_preserve_all_six_named_parameter_sets() {
        let presets = presets().unwrap();
        assert_eq!(presets.len(), 3);
        assert_eq!(
            presets["svtAv1"]
                .iter()
                .map(|preset| preset.parameters.len())
                .collect::<Vec<_>>(),
            vec![8, 8]
        );
        assert_eq!(
            presets["x264"]
                .iter()
                .map(|preset| preset.parameters.len())
                .collect::<Vec<_>>(),
            vec![5, 8]
        );
        assert_eq!(
            presets["x265"]
                .iter()
                .map(|preset| preset.parameters.len())
                .collect::<Vec<_>>(),
            vec![9, 10]
        );
        for (family, sets) in presets {
            let encoder = match family.as_str() {
                "x264" => LadderEncoder::H264,
                "x265" => LadderEncoder::Hevc,
                _ => LadderEncoder::Av1,
            };
            for set in sets {
                for parameter in set.parameters {
                    normalize(encoder, &parameter).unwrap();
                }
            }
        }
    }
    #[test]
    fn parameter_admission_rejects_io_and_preserves_tuple_semantics() {
        for (name, value) in [
            ("input", "/tmp/source"),
            ("deblock", "1:2:3"),
            ("aq-strength", "1;movie=/tmp/a"),
            ("aq-strength", "4"),
        ] {
            assert!(
                normalize(
                    LadderEncoder::H264,
                    &EncoderParameter {
                        name: name.into(),
                        value: value.into()
                    }
                )
                .is_err()
            );
        }
        let value = normalize(
            LadderEncoder::H264,
            &EncoderParameter {
                name: "psy-rd".into(),
                value: "1.1:0.15".into(),
            },
        )
        .unwrap();
        assert_eq!(value.value, "1.1:0.15");
        assert_eq!(
            arguments(LadderEncoder::H264, &[value])[1],
            "psy-rd=1.1,0.15"
        );
    }
    #[test]
    fn svt_probes_keep_bounded_parallelism_and_exercise_interframes() {
        let request = CrfLadderRequest {
            input_path: "/source.mkv".into(),
            video_stream_index: 0,
            encoder: LadderEncoder::Av1,
            preset: "8".into(),
            pixel_format: "yuv420p10le".into(),
            crfs: vec![28],
            sample_count: 1,
            sample_seconds: 1.0,
            metric: LadderMetric::Vmaf,
            recommendation_threshold: None,
            content_preset: None,
            parameters: Vec::new(),
            vmaf_model: None,
            keep_samples_directory: None,
        };
        let values = vec![EncoderParameter {
            name: "hbd-mds".into(),
            value: "1".into(),
        }];
        let args = probe_args(&request, &values);
        assert!(args.windows(2).any(|pair| pair == ["-frames:v", "24"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-i", "testsrc2=s=128x128:r=24"])
        );
        assert_eq!(
            arguments(LadderEncoder::Av1, &values),
            [
                OsString::from("-svtav1-params"),
                OsString::from("lp=1:hbd-mds=1")
            ]
        );
        assert_eq!(
            arguments(LadderEncoder::Av1, &[]),
            [OsString::from("-svtav1-params"), OsString::from("lp=1")]
        );
    }
}
