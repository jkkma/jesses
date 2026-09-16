use super::*;
use media_core::{
    Av1anChunkMethod, Av1anChunkOrder, Av1anOptions, Av1anSceneDetection, Av1anSplitMethod,
};

pub(in crate::jobs) fn validate_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    let Some(options) = settings.av1an_options else {
        return Ok(());
    };
    let invalid = |message| AppError::new("ENCODE_SETTINGS_INVALID", message, None);
    if settings.backend != media_core::EncodeBackend::Av1an {
        return Err(invalid(
            "Scene, chunk, and quality target settings belong to the av1an workflow.",
        ));
    }
    if options.maximum_chunk_frames > 100_000
        || !(1..=100_000).contains(&options.minimum_scene_frames)
        || (options.maximum_chunk_frames > 0
            && options.minimum_scene_frames > options.maximum_chunk_frames)
        || options
            .scene_downscale_height
            .is_some_and(|height| !(64..=4320).contains(&height) || height % 2 != 0)
    {
        return Err(invalid(
            "Use a maximum chunk length of 0–100000 frames (0 disables the limit), a minimum scene length of 1–100000 not exceeding an enabled chunk limit, and an even scene height of 64–4320 or original resolution.",
        ));
    }
    if let Some(target) = options.target_quality
        && (target.minimum_score_tenths > target.maximum_score_tenths
            || target.maximum_score_tenths > 1000
            || !(1..=63).contains(&target.minimum_crf)
            || !(target.minimum_crf..=63).contains(&target.maximum_crf)
            || !(1..=10).contains(&target.probes)
            || !(1..=4).contains(&target.probing_rate)
            || [target.probe_width, target.probe_height]
                .into_iter()
                .any(|size| !(128..=8192).contains(&size) || size % 2 != 0))
    {
        return Err(invalid(
            "Quality targets require an ordered score range from 0–100, ordered CRF bounds from 1–63, 1–10 probes, frame sampling from 1–4, and even evaluation dimensions from 128–8192.",
        ));
    }
    if options
        .target_quality
        .is_some_and(metrics::needs_vapoursynth)
        && plugin(options).is_none()
    {
        return Err(invalid(
            "SSIMULACRA2, Butteraugli, and sampled XPSNR require a VapourSynth source reader (L-SMASH, FFMS2, or BestSource). XPSNR with every frame can use FFmpeg select or hybrid.",
        ));
    }
    Ok(())
}

pub(super) fn chunk_method(options: Av1anOptions) -> &'static str {
    match options.chunk_method {
        Av1anChunkMethod::Lsmash => "lsmash",
        Av1anChunkMethod::Ffms2 => "ffms2",
        Av1anChunkMethod::Bestsource => "bestsource",
        Av1anChunkMethod::Select => "select",
        Av1anChunkMethod::Hybrid => "hybrid",
    }
}

pub(super) fn plugin(options: Av1anOptions) -> Option<(&'static str, &'static str)> {
    match options.chunk_method {
        Av1anChunkMethod::Lsmash => Some(("systems.innocent.lsmas", "L-SMASH Works")),
        Av1anChunkMethod::Ffms2 => Some(("com.vapoursynth.ffms2", "FFMS2")),
        Av1anChunkMethod::Bestsource => Some(("com.vapoursynth.bestsource", "BestSource")),
        Av1anChunkMethod::Select | Av1anChunkMethod::Hybrid => None,
    }
}

pub(super) fn validate_plugin(version: &str, options: Av1anOptions) -> Result<(), String> {
    if let Some(target) = options.target_quality {
        metrics::validate_plugin(version, target)?;
    }
    if (matches!(
        options.chunk_method,
        Av1anChunkMethod::Select | Av1anChunkMethod::Hybrid
    ) || options
        .target_quality
        .is_some_and(|target| target.probing_rate > 1))
        && !version.contains("ffmpeg9-passthrough-v1")
    {
        return Err("This selection requires av1an with the ffmpeg9-passthrough-v1 compatibility fix. Older select source pipes duplicate frames and older sampled probes use removed FFmpeg arguments.".into());
    }
    if let Some((identifier, name)) = plugin(options)
        && !version.lines().any(|line| {
            line.split_once(':')
                .is_some_and(|(key, value)| key.trim() == identifier && value.trim() == "Found")
        })
    {
        return Err(format!(
            "The selected {} chunk method requires VapourSynth with the {name} plugin ({identifier}). av1an did not report that dependency as available.",
            chunk_method(options)
        ));
    }
    Ok(())
}

pub(super) fn append(args: &mut Vec<OsString>, settings: &EncodeSettings, params: &str) {
    let options = settings.av1an_options.unwrap_or_default();
    for (key, value) in [
        ("--chunk-method", chunk_method(options).to_owned()),
        (
            "--split-method",
            if options.split_method == Av1anSplitMethod::SceneDetection {
                "av-scenechange"
            } else {
                "none"
            }
            .into(),
        ),
        (
            "--sc-method",
            if options.scene_detection == Av1anSceneDetection::Standard {
                "standard"
            } else {
                "fast"
            }
            .into(),
        ),
        ("--extra-split", options.maximum_chunk_frames.to_string()),
        ("--min-scene-len", options.minimum_scene_frames.to_string()),
        (
            "--chunk-order",
            match options.chunk_order {
                Av1anChunkOrder::LongToShort => "long-to-short",
                Av1anChunkOrder::ShortToLong => "short-to-long",
                Av1anChunkOrder::Sequential => "sequential",
                Av1anChunkOrder::Random => "random",
            }
            .into(),
        ),
    ] {
        args.extend([key.into(), value.into()]);
    }
    if let Some(height) = options.scene_downscale_height {
        args.extend(["--sc-downscale-height".into(), height.to_string().into()]);
    }
    if let Some(target) = options.target_quality {
        for (key, value) in [
            ("--target-metric", metrics::cli(target.metric).into()),
            (
                "--target-quality",
                format!(
                    "{:.1}-{:.1}",
                    f64::from(target.minimum_score_tenths) / 10.0,
                    f64::from(target.maximum_score_tenths) / 10.0
                ),
            ),
            (
                "--qp-range",
                format!("{}-{}", target.minimum_crf, target.maximum_crf),
            ),
            ("--probes", target.probes.to_string()),
            ("--probing-rate", target.probing_rate.to_string()),
            (
                "--probe-res",
                format!("{}x{}", target.probe_width, target.probe_height),
            ),
            (
                "--vmaf-res",
                format!("{}x{}", target.probe_width, target.probe_height),
            ),
            ("--vmaf-threads", "2".into()),
            ("--probing-stat", "mean".into()),
            // Explicit copied scalars keep the selected build, preset, color
            // and grain controls without av1an's probe-file reuse shortcut.
            ("--probe-video-params", params.into()),
        ] {
            args.extend([key.into(), value.into()]);
        }
    }
}

pub(super) async fn capabilities(
    executable: &Path,
    ffmpeg: &Path,
    environment: &supervisor::ChildEnvironment,
    work: &Path,
    settings: &EncodeSettings,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let options = settings.av1an_options.unwrap_or_default();
    let help = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable: executable.to_owned(),
            args: vec!["--help".into()],
            cwd: Some(work.to_owned()),
        },
        cancel.clone(),
        512 * 1024,
        Duration::from_secs(15),
        Some(environment),
    )
    .await
    .map_err(|e| process_error(e, executable))?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    let mut required = vec![
        "--chunk-method",
        "--split-method",
        "--sc-method",
        "--extra-split",
        "--min-scene-len",
        "--sc-downscale-height",
        "--chunk-order",
    ];
    if options.target_quality.is_some() {
        required.extend([
            "--target-metric",
            "--target-quality",
            "--qp-range",
            "--probes",
            "--probing-rate",
            "--probe-res",
            "--probing-stat",
            "--probe-video-params",
            "--vmaf-threads",
            "--vmaf-filter",
        ]);
    }
    if !help.status.success()
        || required
            .iter()
            .any(|option| !text.split_whitespace().any(|word| word == *option))
    {
        return Err(files::error(
            "AV1AN_CAPABILITY_UNSUPPORTED",
            "This av1an does not advertise every required scene/chunk or quality-target option. Install a compatible av1an version.",
            executable,
        ));
    }
    if let Some(target) = options.target_quality {
        metrics::check(ffmpeg, environment, work, target, cancel).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_core::Av1anTargetQuality;
    #[test]
    fn validates_scalar_contract_and_real_dependency_receipts() {
        let mut settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            av1an_options: Some(Av1anOptions::default()),
            ..Default::default()
        };
        validate_settings(&settings).unwrap();
        settings
            .av1an_options
            .as_mut()
            .unwrap()
            .maximum_chunk_frames = 12;
        assert!(validate_settings(&settings).is_err());
        settings.av1an_options = Some(Av1anOptions::default());
        settings.av1an_options.as_mut().unwrap().target_quality = Some(Av1anTargetQuality {
            metric: Default::default(),
            minimum_score_tenths: 940,
            maximum_score_tenths: 960,
            minimum_crf: 15,
            maximum_crf: 50,
            probes: 4,
            probing_rate: 2,
            probe_width: 1920,
            probe_height: 1080,
        });
        let options = settings.av1an_options.unwrap();
        let supported_version = concat!(
            "ffmpeg9-passthrough-v1\n",
            "ffmpeg-metric-matrix-v1\n",
            "lsmash-software-probes-v1\n",
            "systems.innocent.lsmas : Found",
        );
        validate_plugin(supported_version, options).unwrap();
        for missing_fix in ["ffmpeg9-passthrough-v1", "ffmpeg-metric-matrix-v1"] {
            let version = supported_version.replace(missing_fix, "");
            assert!(
                validate_plugin(&version, options)
                    .unwrap_err()
                    .contains(missing_fix)
            );
        }
        assert!(
            validate_plugin(
                &supported_version.replace("lsmas : Found", "lsmas : Not found"),
                options
            )
            .unwrap_err()
            .contains("L-SMASH")
        );
        for (method, identifier, label) in [
            (Av1anChunkMethod::Ffms2, "com.vapoursynth.ffms2", "FFMS2"),
            (
                Av1anChunkMethod::Bestsource,
                "com.vapoursynth.bestsource",
                "BestSource",
            ),
        ] {
            let mut reader = options;
            reader.chunk_method = method;
            let version = format!(
                "ffmpeg9-passthrough-v1\nffmpeg-metric-matrix-v1\n{identifier} : Not found"
            );
            let error = validate_plugin(&version, reader).unwrap_err();
            assert!(error.contains(label));
            assert!(error.contains(identifier));
        }
        settings
            .av1an_options
            .as_mut()
            .unwrap()
            .target_quality
            .as_mut()
            .unwrap()
            .minimum_score_tenths = 970;
        assert!(validate_settings(&settings).is_err());
        settings.av1an_options = Some(Av1anOptions::default());
        settings.backend = media_core::EncodeBackend::Standalone;
        assert!(validate_settings(&settings).is_err());
    }
}
