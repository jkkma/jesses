use super::*;
use media_core::{
    Av1anChunkMethod, Av1anChunkOrder, Av1anOptions, Av1anSceneDetection, Av1anSplitMethod,
};

/// The fork captures -x before custom filters execute, including Split=None.
/// Keep the incoming draft immutable so Saved/New settings preserve the source
/// profile rather than converting its hidden defaults into explicit API values.
pub(super) fn effective_chunks(
    settings: &EncodeSettings,
    typed_rate: (u32, u32),
    source_height: u32,
) -> Av1anOptions {
    let mut options = settings.av1an_options.unwrap_or_default();
    if options.source_workflow {
        options.maximum_chunk_frames = super::super::encoding_preferences::keyframes_at_rate(
            typed_rate,
            settings
                .encoding_preferences
                .map_or(10, |value| value.keyframe_interval_seconds),
        );
        options.minimum_scene_frames = 24;
        options.scene_detection = Av1anSceneDetection::Standard;
        options.scene_downscale_height = (options.split_method == Av1anSplitMethod::SceneDetection)
            .then(|| source_scene_height(source_height));
    }
    options
}

fn source_scene_height(height: u32) -> u16 {
    let multiplier = match height {
        4320.. => 0.1667f32,
        2160.. => 0.3333,
        1440.. => 0.5,
        1080.. => 0.6667,
        900.. => 0.7083,
        720.. => 0.75,
        _ => 1.0,
    };
    ((height as f32 * multiplier).round_ties_even() as u32).clamp(360, 2160) as u16 / 2 * 2
}

pub(in crate::jobs) fn source_resamples(settings: &EncodeSettings, source: (u32, u32)) -> bool {
    if !settings
        .av1an_options
        .is_some_and(|options| options.source_workflow)
    {
        return false;
    }
    let Some(temporal) = settings.temporal else {
        return false;
    };
    let Some(output) = temporal.frame_rate else {
        return false;
    };
    let mut rate = f64::from(source.0) / f64::from(source.1);
    if temporal
        .deinterlace
        .map(|value| value.mode)
        .or_else(|| temporal.qtgmc.map(|value| value.mode))
        == Some(media_core::DeinterlaceMode::Bob)
    {
        rate *= 2.0;
    }
    let target = f64::from(output.numerator) / f64::from(output.denominator);
    target > 0.01 && (rate - target).abs() > rate.max(target) * 0.0001
}

pub(super) fn effective_target(
    options: Av1anOptions,
    encoder: media_core::VideoEncoder,
    source_dimensions: (u32, u32),
) -> Option<media_core::Av1anTargetQuality> {
    let mut target = options.target_quality?;
    if options.source_target_defaults {
        if matches!(
            target.metric,
            media_core::Av1anTargetMetric::Vmaf | media_core::Av1anTargetMetric::Ssimulacra2
        ) {
            target.minimum_score_tenths = (target.minimum_score_tenths / 10) * 10;
        }
        target.maximum_score_tenths = target.minimum_score_tenths;
        target.minimum_crf = 15;
        target.maximum_crf = match encoder {
            media_core::VideoEncoder::X264 | media_core::VideoEncoder::X265Standalone => 35,
            media_core::VideoEncoder::AomAv1 | media_core::VideoEncoder::VpxStandalone => 55,
            _ => 50,
        };
        target.probes = 4;
        target.probing_rate = 1;
        let dimensions = if matches!(
            target.metric,
            media_core::Av1anTargetMetric::Vmaf
                | media_core::Av1anTargetMetric::Xpsnr
                | media_core::Av1anTargetMetric::XpsnrWeighted
        ) {
            (1920, 1080)
        } else {
            source_dimensions
        };
        target.probe_width = u16::try_from(dimensions.0).expect("validated source dimensions");
        target.probe_height = u16::try_from(dimensions.1).expect("validated source dimensions");
    }
    Some(target)
}

/// Qualified engine's scalar parser uses a 1% score tolerance (and replaces
/// zero with 0.001). Keep its actual floating range in recovery validation.
pub(super) fn source_score_range(target: media_core::Av1anTargetQuality) -> (f64, f64) {
    let value = if matches!(
        target.metric,
        media_core::Av1anTargetMetric::Vmaf | media_core::Av1anTargetMetric::Ssimulacra2
    ) {
        f64::from(target.minimum_score_tenths / 10)
    } else {
        f64::from(target.minimum_score_tenths) / 10.0
    };
    let value = if value == 0.0 { 0.001 } else { value };
    let tolerance = value * 0.01;
    (value - tolerance, value + tolerance)
}

fn dependency_target(options: Av1anOptions) -> Option<media_core::Av1anTargetQuality> {
    let mut target = options.target_quality?;
    if options.source_target_defaults {
        target.probing_rate = 1;
    }
    Some(target)
}

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
    if (!options.source_workflow
        && (options.maximum_chunk_frames > 100_000
            || !(1..=100_000).contains(&options.minimum_scene_frames)
            || (options.maximum_chunk_frames > 0
                && options.minimum_scene_frames > options.maximum_chunk_frames)
            || options
                .scene_downscale_height
                .is_some_and(|height| !(64..=4320).contains(&height) || height % 2 != 0)))
        || options.encoder_threads.is_some_and(|threads| threads > 64)
        || !(1..=10).contains(&options.max_tries)
        || !(1..=16).contains(&options.scene_detection_slices)
    {
        return Err(invalid(
            "Use encoder threads 0–64, chunk attempts 1–10, scene slices 1–16, maximum chunk length 0–100000 (0 disables), a minimum scene length of 1–100000 not exceeding an enabled chunk limit, and an even scene height of 64–4320 or original resolution.",
        ));
    }
    if options.pixel_format.is_some_and(|format| {
        (settings.encoder.is_svt()
            && !matches!(
                format,
                media_core::Av1anPixelFormat::Yuv420p | media_core::Av1anPixelFormat::Yuv420p10le
            ))
            || (settings.encoder == media_core::VideoEncoder::VpxStandalone
                && matches!(
                    format,
                    media_core::Av1anPixelFormat::Yuv422p
                        | media_core::Av1anPixelFormat::Yuv422p10le
                ))
    }) {
        return Err(invalid(
            "SVT-AV1 accepts 4:2:0 output, VPX accepts 4:2:0 or 4:4:4, and AOM, x264, and x265 also accept 4:2:2; use 8-bit or 10-bit output.",
        ));
    }
    let maximum_crf = if matches!(
        settings.encoder,
        media_core::VideoEncoder::X264 | media_core::VideoEncoder::X265Standalone
    ) {
        51
    } else {
        63
    };
    let minimum_crf = if matches!(
        settings.encoder,
        media_core::VideoEncoder::X264
            | media_core::VideoEncoder::X265Standalone
            | media_core::VideoEncoder::AomAv1
            | media_core::VideoEncoder::VpxStandalone
    ) {
        0
    } else {
        1
    };
    if let Some(target) = effective_target(options, settings.encoder, (1920, 1080))
        && (settings.lossless
            || target.minimum_score_tenths > target.maximum_score_tenths
            || target.maximum_score_tenths > 1000
            || !(minimum_crf..=maximum_crf).contains(&target.minimum_crf)
            || !(target.minimum_crf..=maximum_crf).contains(&target.maximum_crf)
            || !(1..=10).contains(&target.probes)
            || !(1..=4).contains(&target.probing_rate)
            || [target.probe_width, target.probe_height]
                .into_iter()
                .any(|size| !(128..=8192).contains(&size) || size % 2 != 0))
    {
        return Err(invalid(
            "Quality targets require an ordered score range from 0–100, encoder-specific ordered CRF bounds, 1–10 probes, frame sampling from 1–4, and even evaluation dimensions from 128–8192.",
        ));
    }
    if dependency_target(options).is_some_and(metrics::needs_vapoursynth)
        && plugin(options).is_none()
    {
        return Err(invalid(
            "SSIMULACRA2, Butteraugli, and sampled XPSNR (including weighted XPSNR) require a VapourSynth source reader (L-SMASH, FFMS2, or BestSource). Every-frame XPSNR can use FFmpeg select or hybrid.",
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
        Av1anChunkMethod::Segment => "segment",
    }
}

pub(super) fn plugin(options: Av1anOptions) -> Option<(&'static str, &'static str)> {
    match options.chunk_method {
        Av1anChunkMethod::Lsmash => Some(("systems.innocent.lsmas", "L-SMASH Works")),
        Av1anChunkMethod::Ffms2 => Some(("com.vapoursynth.ffms2", "FFMS2")),
        Av1anChunkMethod::Bestsource => Some(("com.vapoursynth.bestsource", "BestSource")),
        Av1anChunkMethod::Select | Av1anChunkMethod::Hybrid | Av1anChunkMethod::Segment => None,
    }
}

pub(super) fn validate_plugin(
    version: &str,
    options: Av1anOptions,
    requires_probe_filter: bool,
) -> Result<(), String> {
    if options.chunk_method == Av1anChunkMethod::Segment && !version.contains("segment-ffmpeg9-v1")
    {
        return Err("Segment reading requires av1an with the segment-ffmpeg9-v1 compatibility fix. Older engines use FFmpeg's removed -vsync option and cannot create source segments.".into());
    }
    if let Some(target) = dependency_target(options) {
        if options.source_target_defaults && !version.contains("source-unfiltered-probes-v1") {
            return Err("Source target defaults require av1an with source-unfiltered-probes-v1. This engine cannot preserve the source application's unfiltered probe/reference behavior while final chunk filters run.".into());
        }
        metrics::validate_plugin(
            version,
            target,
            requires_probe_filter && !options.source_target_defaults,
        )?;
    }
    if (matches!(
        options.chunk_method,
        Av1anChunkMethod::Select | Av1anChunkMethod::Hybrid
    ) || dependency_target(options).is_some_and(|target| target.probing_rate > 1))
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

pub(super) fn append(
    args: &mut Vec<OsString>,
    settings: &EncodeSettings,
    plan: &Plan,
    params: &str,
) {
    let options = effective_chunks(
        settings,
        plan.source_chunk_rate(),
        plan.frame_dimensions(false).1,
    );
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
        ("--extra-split", options.maximum_chunk_frames.to_string()),
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
    if !options.source_workflow {
        args.extend([
            "--sc-method".into(),
            if options.scene_detection == Av1anSceneDetection::Standard {
                "standard"
            } else {
                "fast"
            }
            .into(),
            "--min-scene-len".into(),
            options.minimum_scene_frames.to_string().into(),
        ]);
    }
    if (!options.source_workflow || options.split_method == Av1anSplitMethod::SceneDetection)
        && let Some(height) = options.scene_downscale_height
    {
        args.extend(["--sc-downscale-height".into(), height.to_string().into()]);
    }
    if let Some(target) = options.target_quality {
        if options.source_target_defaults {
            let score = if matches!(
                target.metric,
                media_core::Av1anTargetMetric::Vmaf | media_core::Av1anTargetMetric::Ssimulacra2
            ) {
                (target.minimum_score_tenths / 10).to_string()
            } else {
                format!("{:.1}", f64::from(target.minimum_score_tenths) / 10.0)
            };
            args.extend([
                "--source-unfiltered-probes".into(),
                "--target-metric".into(),
                metrics::cli(target.metric).into(),
                "--target-quality".into(),
                score.into(),
            ]);
            if target.metric == media_core::Av1anTargetMetric::Vmaf {
                args.extend(["--vmaf-threads".into(), "2".into()]);
            }
            return;
        }
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
        required.extend(["--target-metric", "--target-quality"]);
        if options.source_target_defaults {
            required.push("--source-unfiltered-probes");
        }
        if !options.source_target_defaults {
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
        } else if options
            .target_quality
            .is_some_and(|target| target.metric == media_core::Av1anTargetMetric::Vmaf)
        {
            required.push("--vmaf-threads");
        }
    }
    if !help.status.success()
        || required
            .iter()
            .any(|option| !text.split_whitespace().any(|word| word == *option))
        || options.target_quality.is_some_and(|target| {
            target.metric == media_core::Av1anTargetMetric::XpsnrWeighted
                && !text.contains("xpsnr-weighted")
        })
    {
        return Err(files::error(
            "AV1AN_CAPABILITY_UNSUPPORTED",
            "This av1an does not advertise every required scene/chunk or quality-target option. Install a compatible av1an version.",
            executable,
        ));
    }
    if let Some(target) = dependency_target(options) {
        metrics::check(ffmpeg, environment, work, target, cancel).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_chunk_interval_uses_fork_rounding_bounds_and_ignores_hidden_drafts() {
        let settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            av1an_options: Some(Av1anOptions {
                source_workflow: true,
                maximum_chunk_frames: 100001,
                minimum_scene_frames: 0,
                scene_downscale_height: Some(1),
                scene_detection: Av1anSceneDetection::Fast,
                ..Default::default()
            }),
            ..Default::default()
        };
        validate_settings(&settings).unwrap();
        for (rate, expected) in [
            ((24000, 1001), 240),
            ((30, 1), 300),
            ((25, 20), 12),
            ((27, 20), 14),
            ((60, 1), 480),
        ] {
            let effective = effective_chunks(&settings, rate, 1080);
            assert_eq!(effective.maximum_chunk_frames, expected, "{rate:?}");
            assert_eq!(effective.minimum_scene_frames, 24);
            assert_eq!(effective.scene_detection, Av1anSceneDetection::Standard);
            assert_eq!(effective.scene_downscale_height, Some(720));
        }
        let mut explicit = settings.clone();
        explicit.av1an_options.as_mut().unwrap().source_workflow = false;
        assert!(validate_settings(&explicit).is_err());
        explicit
            .av1an_options
            .as_mut()
            .unwrap()
            .maximum_chunk_frames = 0;
        explicit
            .av1an_options
            .as_mut()
            .unwrap()
            .minimum_scene_frames = 1;
        explicit
            .av1an_options
            .as_mut()
            .unwrap()
            .scene_downscale_height = None;
        assert_eq!(
            effective_chunks(&explicit, (60, 1), 1080),
            explicit.av1an_options.unwrap()
        );
    }

    #[test]
    fn source_scene_height_matches_adaptive_even_source_thresholds() {
        for (height, expected) in [
            (64, 360),
            (480, 480),
            (720, 540),
            (900, 636),
            (1080, 720),
            (1440, 720),
            (2160, 720),
            (4320, 720),
            (16384, 2160),
        ] {
            assert_eq!(source_scene_height(height), expected, "{height}");
        }
    }

    #[test]
    fn svt_rejects_explicit_non_420_output() {
        let settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            av1an_options: Some(Av1anOptions {
                pixel_format: Some(media_core::Av1anPixelFormat::Yuv444p10le),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(
            validate_settings(&settings)
                .unwrap_err()
                .message
                .contains("4:2:0")
        );
    }

    #[test]
    fn segment_reader_requires_patched_engine() {
        let options = Av1anOptions {
            chunk_method: Av1anChunkMethod::Segment,
            ..Default::default()
        };
        assert_eq!(chunk_method(options), "segment");
        assert!(plugin(options).is_none());
        assert!(
            validate_plugin("av1an 0.5.2", options, false)
                .unwrap_err()
                .contains("segment-ffmpeg9-v1")
        );
        validate_plugin("av1an 0.5.2 [segment-ffmpeg9-v1]", options, false).unwrap();
    }

    #[test]
    fn x264_metric_search_uses_its_crf_scale() {
        let mut settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            encoder: media_core::VideoEncoder::X264,
            ..Default::default()
        };
        let mut options = Av1anOptions::default();
        let mut target = media_core::Av1anTargetQuality {
            metric: Default::default(),
            minimum_score_tenths: 900,
            maximum_score_tenths: 950,
            minimum_crf: 0,
            maximum_crf: 51,
            probes: 3,
            probing_rate: 1,
            probe_width: 640,
            probe_height: 360,
        };
        options.target_quality = Some(target);
        settings.av1an_options = Some(options);
        assert!(validate_settings(&settings).is_ok());
        target.maximum_crf = 52;
        options.target_quality = Some(target);
        settings.av1an_options = Some(options);
        assert!(validate_settings(&settings).is_err());
        settings.encoder = media_core::VideoEncoder::SvtAv1;
        target.maximum_crf = 51;
        options.target_quality = Some(target);
        settings.av1an_options = Some(options);
        assert!(validate_settings(&settings).is_err());
    }
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
            "target-probe-filter-v1\n",
            "ffmpeg9-passthrough-v1\n",
            "ffmpeg-metric-matrix-v1\nxpsnr-signed-stats-v1\n",
            "lsmash-software-probes-v1\n",
            "systems.innocent.lsmas : Found",
        );
        validate_plugin(supported_version, options, true).unwrap();
        validate_plugin(
            &supported_version.replace("target-probe-filter-v1", ""),
            options,
            false,
        )
        .unwrap();
        for missing_fix in [
            "target-probe-filter-v1",
            "ffmpeg9-passthrough-v1",
            "ffmpeg-metric-matrix-v1",
        ] {
            let version = supported_version.replace(missing_fix, "");
            assert!(
                validate_plugin(&version, options, true)
                    .unwrap_err()
                    .contains(missing_fix)
            );
        }
        assert!(
            validate_plugin(
                &supported_version.replace("lsmas : Found", "lsmas : Not found"),
                options,
                true,
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
                "target-probe-filter-v1\nffmpeg9-passthrough-v1\nffmpeg-metric-matrix-v1\n{identifier} : Not found"
            );
            let error = validate_plugin(&version, reader, true).unwrap_err();
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
