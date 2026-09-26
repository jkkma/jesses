//! Metric scorers for aligned, frame-counted ladder samples. The two image
//! metrics use the installed CPU VapourSynth plugins; XPSNR uses FFmpeg.
use super::*;

pub(super) enum Scorer {
    Python {
        executable: PathBuf,
        environment: ChildEnvironment,
        cwd: PathBuf,
        metric: LadderMetric,
    },
    Xpsnr {
        ffmpeg: PathBuf,
    },
}

fn scorer_error(code: &str, detail: impl Into<String>, path: &Path) -> AppError {
    error(code, detail, Some(path))
}

impl Scorer {
    pub(super) async fn prepare(
        metric: LadderMetric,
        ffmpeg: &Path,
        work: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Option<Self>, AppError> {
        if !matches!(
            metric,
            LadderMetric::Ssimulacra2 | LadderMetric::ButteraugliInf | LadderMetric::XpsnrWeighted
        ) {
            return Ok(None);
        }
        if metric == LadderMetric::XpsnrWeighted {
            let result = run_capture(
                &CommandSpec {
                    executable: ffmpeg.to_owned(),
                    args: ["-hide_banner", "-h", "filter=xpsnr"]
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                    cwd: None,
                },
                cancel.clone(),
                128 * 1024,
                Duration::from_secs(15),
            )
            .await
            .map_err(|cause| process_error(cause, Some(ffmpeg)))?;
            if !result.status.success()
                || !String::from_utf8_lossy(&result.stdout).contains("Filter xpsnr")
            {
                return Err(scorer_error(
                    "UTILITY_SCORER_UNAVAILABLE",
                    "Weighted XPSNR requires FFmpeg's xpsnr filter.",
                    ffmpeg,
                ));
            }
            return Ok(Some(Self::Xpsnr {
                ffmpeg: ffmpeg.to_owned(),
            }));
        }
        #[cfg(not(windows))]
        {
            let _ = work;
            return Err(scorer_error(
                "UTILITY_SCORER_UNAVAILABLE",
                "This scorer currently requires the validated Windows VapourSynth runtime.",
                ffmpeg,
            ));
        }
        #[cfg(windows)]
        {
            let av1an = tool("av1an", "VapourSynth ladder scorer discovery").await?;
            let packaged = crate::bundled_tools::av1an_runtime(&av1an)
                .map_err(|detail| scorer_error("UTILITY_SCORER_UNAVAILABLE", detail, &av1an))?;
            let (cwd, plugins) = if let Some(plugins) = packaged {
                let cwd = plugins.ancestors().nth(3).ok_or_else(|| {
                    scorer_error(
                        "UTILITY_SCORER_UNAVAILABLE",
                        "Invalid packaged VapourSynth layout.",
                        &plugins,
                    )
                })?;
                (cwd.to_owned(), plugins)
            } else {
                let cwd = av1an
                    .parent()
                    .ok_or_else(|| {
                        scorer_error(
                            "UTILITY_SCORER_UNAVAILABLE",
                            "Invalid av1an executable path.",
                            &av1an,
                        )
                    })?
                    .join("vsynth");
                (cwd.clone(), cwd)
            };
            let executable = cwd.join("python.exe");
            if !executable.is_file() || !plugins.join("vsscript.dll").is_file() {
                return Err(scorer_error(
                    "UTILITY_SCORER_UNAVAILABLE",
                    "The selected av1an installation has no usable portable VapourSynth Python runtime. Install its scorer plugins or select another metric.",
                    &av1an,
                ));
            }
            let mut path = std::env::join_paths([cwd.as_path(), av1an.parent().unwrap()]).map_err(
                |cause| scorer_error("UTILITY_SCORER_UNAVAILABLE", cause.to_string(), &av1an),
            )?;
            if let Some(inherited) = std::env::var_os("PATH") {
                path.push(";");
                path.push(inherited);
            }
            let environment = crate::bundled_tools::frameserver_environment(&plugins, &path);
            let probe = match metric {
                LadderMetric::Ssimulacra2 => {
                    "import vapoursynth as vs; c=vs.core; assert hasattr(c,'lsmas') and hasattr(c,'vszip') and (hasattr(c.vszip,'SSIMULACRA2') or hasattr(c.vszip,'Metrics')); print('JESSES_SCORER_READY')"
                }
                LadderMetric::ButteraugliInf => {
                    "import vapoursynth as vs; c=vs.core; assert hasattr(c,'lsmas') and hasattr(c,'julek') and hasattr(c.julek,'Butteraugli'); print('JESSES_SCORER_READY')"
                }
                _ => unreachable!(),
            };
            let result = run_capture_with_environment(
                &CommandSpec {
                    executable: executable.clone(),
                    args: vec!["-c".into(), probe.into()],
                    cwd: Some(cwd.clone()),
                },
                cancel.clone(),
                128 * 1024,
                Duration::from_secs(30),
                Some(&environment),
            )
            .await
            .map_err(|cause| process_error(cause, Some(&executable)))?;
            if !result.status.success()
                || !String::from_utf8_lossy(&result.stdout).contains("JESSES_SCORER_READY")
            {
                let diagnostic = String::from_utf8_lossy(&result.stderr);
                return Err(scorer_error(
                    "UTILITY_SCORER_UNAVAILABLE",
                    format!(
                        "The selected VapourSynth runtime cannot load the CPU {} scorer and L-SMASH source reader: {}",
                        if metric == LadderMetric::Ssimulacra2 {
                            "SSIMULACRA2"
                        } else {
                            "Butteraugli"
                        },
                        diagnostic.trim()
                    ),
                    &executable,
                ));
            }
            let _ = work;
            Ok(Some(Self::Python {
                executable,
                environment,
                cwd,
                metric,
            }))
        }
    }

    pub(super) async fn score(
        &self,
        reference: &Path,
        candidate: &Path,
        expected_frames: u64,
        work: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Option<f64>, AppError> {
        match self {
            Self::Xpsnr { ffmpeg } => {
                score_xpsnr(ffmpeg, reference, candidate, expected_frames, cancel).await
            }
            Self::Python {
                executable,
                environment,
                cwd,
                metric,
            } => {
                let script = work.join(format!(
                    "scorer-{}.py",
                    NEXT_ID.fetch_add(1, Ordering::Relaxed)
                ));
                // L-SMASH writes index files to cachedir. Keep them at the
                // scratch root so Scratch's owned-file cleanup removes them.
                let source = python_script(*metric, reference, candidate, work, expected_frames)?;
                fs::write(&script, source).map_err(|cause| {
                    scorer_error("UTILITY_SCORER_FAILED", cause.to_string(), &script)
                })?;
                let result = run_capture_with_environment(
                    &CommandSpec {
                        executable: executable.clone(),
                        args: vec![os(&script)],
                        cwd: Some(cwd.clone()),
                    },
                    cancel.clone(),
                    256 * 1024,
                    MEDIA_LIMIT,
                    Some(environment),
                )
                .await
                .map_err(|cause| process_error(cause, Some(&script)))?;
                if !result.status.success() {
                    return Err(scorer_error(
                        "UTILITY_SCORER_FAILED",
                        format!(
                            "The {} scorer failed: {}",
                            metric_name(*metric),
                            String::from_utf8_lossy(&result.stderr).trim()
                        ),
                        candidate,
                    ));
                }
                let text = std::str::from_utf8(&result.stdout).map_err(|_| {
                    scorer_error(
                        "UTILITY_SCORER_FAILED",
                        "The scorer returned non-UTF-8 output.",
                        candidate,
                    )
                })?;
                let line = text
                    .lines()
                    .find(|line| line.starts_with("JESSES_LADDER_SCORE "))
                    .ok_or_else(|| {
                        scorer_error(
                            "UTILITY_SCORER_FAILED",
                            "The scorer returned no complete frame score.",
                            candidate,
                        )
                    })?;
                let fields = line.split_whitespace().collect::<Vec<_>>();
                let (Some(score), Some(frames)) = (
                    fields.get(1).and_then(|s| s.parse::<f64>().ok()),
                    fields.get(2).and_then(|s| s.parse::<u64>().ok()),
                ) else {
                    return Err(scorer_error(
                        "UTILITY_SCORER_FAILED",
                        "The scorer returned an invalid score record.",
                        candidate,
                    ));
                };
                if fields.len() != 3
                    || frames != expected_frames
                    || !score.is_finite()
                    || (*metric == LadderMetric::ButteraugliInf && score < 0.0)
                    // Very poor SSIMULACRA2 scores have no fixed lower bound.
                    || (*metric == LadderMetric::Ssimulacra2 && score > 100.0)
                {
                    return Err(scorer_error(
                        "UTILITY_SCORER_FAILED",
                        "The scorer returned an incomplete or out-of-range frame score.",
                        candidate,
                    ));
                }
                Ok(Some(score))
            }
        }
    }
}

fn metric_name(metric: LadderMetric) -> &'static str {
    match metric {
        LadderMetric::Ssimulacra2 => "SSIMULACRA2",
        LadderMetric::ButteraugliInf => "Butteraugli INF",
        _ => "metric",
    }
}

fn python_script(
    metric: LadderMetric,
    reference: &Path,
    candidate: &Path,
    cache: &Path,
    expected: u64,
) -> Result<String, AppError> {
    let literal = |path: &Path| {
        serde_json::to_string(&path.to_string_lossy().to_string())
            .map_err(|cause| scorer_error("UTILITY_SCORER_FAILED", cause.to_string(), path))
    };
    let mut script = format!(
        "import math\nimport vapoursynth as vs\ncore = vs.core\nREF = {}\nDIST = {}\nCACHE = {}\nEXPECTED = {expected}\n",
        literal(reference)?,
        literal(candidate)?,
        literal(cache)?
    );
    script.push_str("\ndef open_video(path):\n    return core.lsmas.LWLibavSource(source=path, cachedir=CACHE)\nref = open_video(REF)\ndist = open_video(DIST)\nif ref.num_frames != EXPECTED or dist.num_frames != EXPECTED:\n    raise RuntimeError('scorer inputs changed frame count')\n");
    match metric {
        LadderMetric::Ssimulacra2 => script.push_str("\nif hasattr(core.vszip, 'SSIMULACRA2'):\n    scored = core.vszip.SSIMULACRA2(reference=ref, distorted=dist)\nelse:\n    scored = core.vszip.Metrics(reference=ref, distorted=dist, mode=0)\nNAMES = ('SSIMULACRA2', '_SSIMULACRA2')\n"),
        LadderMetric::ButteraugliInf => script.push_str("\ndef rgb(clip):\n    matrix = {1:'709',5:'470bg',6:'170m'}.get(int(clip.get_frame(0).props.get('_Matrix', 1)), '709')\n    return core.resize.Bicubic(clip, format=vs.RGBS, matrix_in_s=matrix)\nscored = core.julek.Butteraugli(reference=rgb(ref), distorted=rgb(dist), distmap=1, intensity_target=203.0)\nNAMES = ('_FrameButteraugli',)\n"),
        _ => unreachable!(),
    }
    script.push_str("\ntotal = 0.0\nfor i in range(EXPECTED):\n    props = scored.get_frame(i).props\n    value = next((float(props[name]) for name in NAMES if name in props), None)\n    if value is None or not math.isfinite(value):\n        raise RuntimeError('missing or nonfinite scorer frame property')\n    total += value\nprint('JESSES_LADDER_SCORE %.9f %d' % (total / EXPECTED, EXPECTED))\n");
    Ok(script)
}

async fn score_xpsnr(
    ffmpeg: &Path,
    reference: &Path,
    candidate: &Path,
    expected: u64,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<f64>, AppError> {
    let result = run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args: ["-hide_banner", "-v", "error", "-nostdin", "-i"]
                .into_iter()
                .map(Into::into)
                .chain([
                    os(candidate),
                    "-i".into(),
                    os(reference),
                    "-filter_complex".into(),
                    "[0:v:0][1:v:0]xpsnr=stats_file=-".into(),
                    "-frames:v".into(),
                    expected.to_string().into(),
                    "-f".into(),
                    "null".into(),
                    "-".into(),
                ])
                .collect(),
            cwd: None,
        },
        cancel.clone(),
        4 * 1024 * 1024,
        MEDIA_LIMIT,
    )
    .await
    .map_err(|cause| process_error(cause, Some(candidate)))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(scorer_error(
            "UTILITY_SCORER_FAILED",
            format!(
                "FFmpeg XPSNR failed: {}",
                String::from_utf8_lossy(&result.stderr)
            ),
            candidate,
        ));
    }
    parse_xpsnr(&result.stdout, expected)
        .map_err(|detail| scorer_error("UTILITY_SCORER_FAILED", detail, candidate))
}

fn parse_xpsnr(bytes: &[u8], expected: u64) -> Result<Option<f64>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "XPSNR output was not UTF-8.")?;
    let mut count = 0_u64;
    let mut total_error = 0.0;
    for line in text.lines().filter(|line| line.starts_with("n:")) {
        let words = line.split_whitespace().collect::<Vec<_>>();
        if words.len() != 11
            || words[0] != "n:"
            || words[2] != "XPSNR"
            || words[3] != "y:"
            || words[5] != "XPSNR"
            || words[6] != "u:"
            || words[8] != "XPSNR"
            || words[9] != "v:"
        {
            return Err("XPSNR returned an invalid per-frame record.".into());
        }
        let ordinal = words[1]
            .parse::<u64>()
            .map_err(|_| "Invalid XPSNR frame ordinal.")?;
        if ordinal != count + 1 {
            return Err("XPSNR skipped or repeated a frame.".into());
        }
        let values = [words[4], words[7], words[10]];
        let mut errors = [0.0; 3];
        for (index, value) in values.into_iter().enumerate() {
            let decibels = value
                .parse::<f64>()
                .map_err(|_| "Invalid XPSNR channel value.")?;
            if decibels.is_nan() || decibels < 0.0 {
                return Err("Invalid XPSNR channel value.".into());
            }
            errors[index] = if decibels.is_infinite() {
                0.0
            } else {
                10_f64.powf(-decibels / 10.0)
            };
        }
        total_error += (4.0 * errors[0] + errors[1] + errors[2]) / 6.0;
        count += 1;
    }
    if count != expected || count == 0 {
        return Err("XPSNR did not score every expected frame.".into());
    }
    let error = total_error / expected as f64;
    if error == 0.0 {
        Ok(None)
    } else {
        let score = -10.0 * error.log10();
        score
            .is_finite()
            .then_some(Some(score))
            .ok_or_else(|| "Invalid pooled XPSNR score.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_xpsnr_uses_all_channels_and_requires_complete_frames() {
        let data = b"n: 1 XPSNR y: 40 XPSNR u: 30 XPSNR v: 30\nn: 2 XPSNR y: 40 XPSNR u: 30 XPSNR v: 30\n\nXPSNR average, 2 frames y: 40\n";
        let score = parse_xpsnr(data, 2).unwrap().unwrap();
        assert!((score - 33.9794).abs() < 0.001);
        assert!(parse_xpsnr(data, 3).is_err());
        assert!(parse_xpsnr(b"n: 2 XPSNR y: 40 XPSNR u: 30 XPSNR v: 30", 1).is_err());
        assert_eq!(
            parse_xpsnr(b"n: 1 XPSNR y: inf XPSNR u: inf XPSNR v: inf", 1).unwrap(),
            None
        );
    }
}
