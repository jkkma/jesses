//! External VapourSynth providers for Linux and macOS. No av1an installation is required.
use super::*;

fn configured(name: &str, directory: bool) -> Result<Option<PathBuf>, AppError> {
    let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if !path.is_absolute()
        || if directory {
            !path.is_dir()
        } else {
            !path.is_file()
        }
    {
        return Err(scorer_error(
            "UTILITY_SCORER_UNAVAILABLE",
            format!(
                "{name} must name an existing absolute {}.",
                if directory { "directory" } else { "file" }
            ),
            &path,
        ));
    }
    Ok(Some(path))
}

pub(super) async fn prepare(
    metric: LadderMetric,
    work: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<Scorer, AppError> {
    let executable = if let Some(path) = configured("JESSES_VAPOURSYNTH_PYTHON", false)? {
        // Preserve a virtual environment's interpreter pathname; canonicalizing
        // its executable symlink would lose that environment's module prefix.
        path
    } else {
        crate::discovery::find_executable(&["python3", "python"]).await.map_err(|detail| error("UTILITY_SCORER_UNAVAILABLE", detail, None))?.ok_or_else(|| error("UTILITY_SCORER_UNAVAILABLE", "Install a Python/VapourSynth environment with L-SMASH, vszip and julek, or set JESSES_VAPOURSYNTH_PYTHON to its absolute interpreter path.", None))?
    };
    let mut environment = ChildEnvironment::default();
    if let Some(plugins) = configured("JESSES_VAPOURSYNTH_PLUGIN_PATH", true)? {
        environment.variables.push((
            "VAPOURSYNTH_EXTRA_PLUGIN_PATH",
            Some(plugins.into_os_string()),
        ));
    }
    probe_cpu(&executable, &environment, metric, work, cancel).await?;
    let gpu_plugin = if let Some(plugin) = configured("JESSES_VSHIP_PLUGIN", false)? {
        let tested = run_capture_with_environment(
            &CommandSpec {
                executable: executable.clone(),
                args: vec!["-B".into(), "-c".into(), gpu_probe(metric, &plugin).into()],
                cwd: Some(work.to_owned()),
            },
            cancel.clone(),
            128 * 1024,
            Duration::from_secs(45),
            Some(&environment),
        )
        .await;
        check_cancel(cancel)?;
        tested
            .ok()
            .filter(|result| {
                result.status.success()
                    && String::from_utf8_lossy(&result.stdout).contains("JESSES_VSHIP_OK")
            })
            .map(|_| plugin)
    } else {
        None
    };
    Ok(Scorer::Python {
        executable,
        environment,
        cwd: work.to_owned(),
        metric,
        gpu_plugin,
    })
}

async fn probe_cpu(
    executable: &Path,
    environment: &ChildEnvironment,
    metric: LadderMetric,
    work: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let program = cpu_probe(metric);
    let result = run_capture_with_environment(
        &CommandSpec {
            executable: executable.to_owned(),
            args: vec!["-B".into(), "-c".into(), program.into()],
            cwd: Some(work.to_owned()),
        },
        cancel.clone(),
        128 * 1024,
        Duration::from_secs(30),
        Some(environment),
    )
    .await
    .map_err(|cause| process_error(cause, Some(executable)))?;
    if !result.status.success()
        || !String::from_utf8_lossy(&result.stdout).contains("JESSES_SCORER_READY")
    {
        return Err(scorer_error(
            "UTILITY_SCORER_UNAVAILABLE",
            format!(
                "The selected Python/VapourSynth environment could not run the CPU {} scorer and L-SMASH reader: {}",
                metric_name(metric),
                diagnostic_tail(&result.stderr)
            ),
            executable,
        ));
    }
    Ok(())
}

fn cpu_probe(metric: LadderMetric) -> String {
    let metric = match metric {
        LadderMetric::Ssimulacra2 => {
            "assert hasattr(core, 'vszip')\nif hasattr(core.vszip, 'SSIMULACRA2'):\n    scored = core.vszip.SSIMULACRA2(reference=ref, distorted=dist)\nelse:\n    scored = core.vszip.Metrics(reference=ref, distorted=dist, mode=0)\nnames = ('SSIMULACRA2', '_SSIMULACRA2')\n"
        }
        LadderMetric::ButteraugliInf => {
            "assert hasattr(core, 'julek') and hasattr(core.julek, 'Butteraugli')\nref = core.resize.Bicubic(ref, format=vs.RGBS, matrix_in_s='709')\ndist = core.resize.Bicubic(dist, format=vs.RGBS, matrix_in_s='709')\nscored = core.julek.Butteraugli(reference=ref, distorted=dist, distmap=1, intensity_target=203.0)\nnames = ('_FrameButteraugli',)\n"
        }
        _ => unreachable!(),
    };
    format!(
        "import math\nimport vapoursynth as vs\ncore = vs.core\nassert hasattr(core, 'lsmas') and hasattr(core.lsmas, 'LWLibavSource')\nref = core.std.BlankClip(width=192, height=128, format=vs.YUV420P8, length=2, color=[32,128,128])\ndist = core.std.BlankClip(ref, color=[34,128,128])\n{metric}for i in range(2):\n    props = scored.get_frame(i).props\n    value = next((float(props[name]) for name in names if name in props), None)\n    assert value is not None and math.isfinite(value), value\nprint('JESSES_SCORER_READY')\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_probe_executes_the_requested_metric_and_checks_finite_properties() {
        for (metric, function, property) in [
            (LadderMetric::Ssimulacra2, "vszip", "_SSIMULACRA2"),
            (
                LadderMetric::ButteraugliInf,
                "julek.Butteraugli",
                "_FrameButteraugli",
            ),
        ] {
            let program = cpu_probe(metric);
            assert!(program.contains("LWLibavSource"));
            assert!(program.contains(function));
            assert!(program.contains(property));
            assert!(program.contains("math.isfinite(value)"));
            assert!(program.contains("for i in range(2)"));
        }
    }
    #[tokio::test]
    async fn failed_provider_probe_is_actionable_and_creates_no_media() {
        let scratch = Scratch::create("failed-scorer-provider").unwrap();
        let (_owner, cancel) = watch::channel(false);
        let result = probe_cpu(
            Path::new("/usr/bin/false"),
            &ChildEnvironment::default(),
            LadderMetric::Ssimulacra2,
            &scratch.path,
            &cancel,
        )
        .await
        .unwrap_err();
        assert_eq!(result.code, "UTILITY_SCORER_UNAVAILABLE");
        assert!(result.message.contains("CPU SSIMULACRA2"));
        assert_eq!(fs::read_dir(&scratch.path).unwrap().count(), 0);
    }
}
