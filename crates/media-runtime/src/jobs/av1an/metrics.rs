//! Exercise the actual scorer API before admitting a target-quality job.
use super::*;
use media_core::{Av1anTargetMetric, Av1anTargetQuality};
use sha2::{Digest, Sha256};
use std::io::Write;

pub(super) fn cli(metric: Av1anTargetMetric) -> &'static str {
    match metric {
        Av1anTargetMetric::Vmaf => "vmaf",
        Av1anTargetMetric::Ssimulacra2 => "ssimulacra2",
        Av1anTargetMetric::Butteraugli => "butteraugli-inf",
        Av1anTargetMetric::Xpsnr => "xpsnr",
        Av1anTargetMetric::XpsnrWeighted => "xpsnr-weighted",
    }
}

pub(super) fn receipt(metric: Av1anTargetMetric) -> &'static str {
    match metric {
        Av1anTargetMetric::Vmaf => "VMAF",
        Av1anTargetMetric::Ssimulacra2 => "SSIMULACRA2",
        Av1anTargetMetric::Butteraugli => "ButteraugliINF",
        Av1anTargetMetric::Xpsnr => "XPSNR",
        Av1anTargetMetric::XpsnrWeighted => "XPSNRWeighted",
    }
}

pub(super) fn label(metric: Av1anTargetMetric) -> &'static str {
    match metric {
        Av1anTargetMetric::Vmaf => "VMAF v0.6.1 (higher is better)",
        Av1anTargetMetric::Ssimulacra2 => "SSIMULACRA2 (higher is better)",
        Av1anTargetMetric::Butteraugli => "Butteraugli INF (lower is better)",
        Av1anTargetMetric::Xpsnr => "XPSNR minimum Y/U/V in dB (higher is better)",
        Av1anTargetMetric::XpsnrWeighted => "Weighted XPSNR in dB (higher is better)",
    }
}

pub(super) fn needs_vapoursynth(target: Av1anTargetQuality) -> bool {
    matches!(
        target.metric,
        Av1anTargetMetric::Ssimulacra2 | Av1anTargetMetric::Butteraugli
    ) || (matches!(
        target.metric,
        Av1anTargetMetric::Xpsnr | Av1anTargetMetric::XpsnrWeighted
    ) && target.probing_rate > 1)
}

fn found(version: &str, identifier: &str) -> bool {
    version.lines().any(|line| {
        line.split_once(':')
            .is_some_and(|(key, value)| key.trim() == identifier && value.trim() == "Found")
    })
}

pub(super) fn validate_plugin(
    version: &str,
    target: Av1anTargetQuality,
    requires_probe_filter: bool,
) -> Result<(), String> {
    if requires_probe_filter && !version.contains("target-probe-filter-v1") {
        return Err("Quality targeting requires av1an with the target-probe-filter-v1 compatibility fix. Older engines omit final-chunk FFmpeg transforms from probe encodes, so transformed references can be scored against different pixels or geometry.".into());
    }
    if requires_probe_filter && target.metric != Av1anTargetMetric::Vmaf {
        return Err("Quality targeting with direct crop, scale, borders, tone-map, or frame-mode deinterlace transforms is qualified only for VMAF. Other metrics require a verified lossless transformed source before av1an starts; Jesses normally prepares it automatically.".into());
    }
    if (target.metric == Av1anTargetMetric::Vmaf
        || (matches!(
            target.metric,
            Av1anTargetMetric::Xpsnr | Av1anTargetMetric::XpsnrWeighted
        ) && target.probing_rate == 1))
        && !version.contains("ffmpeg-metric-matrix-v1")
    {
        return Err("VMAF and every-frame XPSNR targeting require av1an with the ffmpeg-metric-matrix-v1 compatibility fix. The older engine lets FFmpeg change the untagged Y4M reference's color matrix, producing incorrect probe scores.".into());
    }
    if needs_vapoursynth(target)
        && found(version, "systems.innocent.lsmas")
        && !found(version, "com.vapoursynth.ffms2")
        && !found(version, "com.vapoursynth.bestsource")
        && !version.contains("lsmash-software-probes-v1")
    {
        return Err("Quality targeting with L-SMASH as the only probe reader requires av1an with the lsmash-software-probes-v1 compatibility fix. The older engine forces hardware decoding for probe IVF files, whose software fallback can fail.".into());
    }
    let available = match target.metric {
        Av1anTargetMetric::Ssimulacra2 => {
            found(version, "com.julek.vszip") || found(version, "com.lumen.vship")
        }
        Av1anTargetMetric::Butteraugli => {
            found(version, "com.julek.plugin") || found(version, "com.lumen.vship")
        }
        Av1anTargetMetric::Xpsnr | Av1anTargetMetric::XpsnrWeighted if target.probing_rate > 1 => {
            found(version, "com.julek.vszip")
        }
        _ => true,
    };
    if !available {
        return Err(format!(
            "{} targeting requires its VapourSynth scorer plugin: SSIMULACRA2 uses vszip or Vship, Butteraugli uses Julek or Vship, and sampled XPSNR uses vszip R7 or newer. av1an did not report the selected scorer as available.",
            label(target.metric)
        ));
    }
    if target.metric == Av1anTargetMetric::Butteraugli
        && !found(version, "com.lumen.vship")
        && !version.contains("julek-butteraugli-v1")
    {
        return Err("Butteraugli with the Julek plugin requires av1an with the julek-butteraugli-v1 compatibility fix. The older engine invokes a nonexistent lowercase plugin function.".into());
    }
    Ok(())
}

pub(super) async fn check(
    ffmpeg: &Path,
    environment: &supervisor::ChildEnvironment,
    work: &Path,
    target: Av1anTargetQuality,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    if needs_vapoursynth(target) {
        return check_vapoursynth(environment, work, target.metric, cancel).await;
    }
    let (graph, marker) = if target.metric == Av1anTargetMetric::Vmaf {
        (
            "testsrc2=s=192x128:r=24,format=yuv420p10le,split=2[reference][distorted];[distorted][reference]libvmaf=model=version=vmaf_v0.6.1:n_threads=2",
            "VMAF score:",
        )
    } else {
        (
            "testsrc2=s=192x128:r=24,format=yuv420p10le,split=2[reference][a];[a]noise=alls=2:allf=t[distorted];[distorted][reference]xpsnr",
            "XPSNR  y:",
        )
    };
    let result = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args: [
                "-v",
                "info",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                graph,
                "-frames:v",
                "2",
                "-f",
                "null",
                "-",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            cwd: Some(work.to_owned()),
        },
        cancel.clone(),
        128 * 1024,
        Duration::from_secs(30),
        Some(environment),
    )
    .await
    .map_err(|e| process_error(e, ffmpeg))?;
    let diagnostic = String::from_utf8_lossy(&result.stderr);
    if !result.status.success() || !diagnostic.contains(marker) {
        return Err(dependency_error(target.metric, &diagnostic, ffmpeg));
    }
    Ok(())
}

fn dependency_error(metric: Av1anTargetMetric, diagnostic: &str, executable: &Path) -> AppError {
    files::error(
        "AV1AN_DEPENDENCY_MISSING",
        format!(
            "The actual {} scorer check failed. Install the matching working filter/plugin in the selected tool environment. {}",
            label(metric),
            diagnostic
                .lines()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" ")
        ),
        executable,
    )
}

// The program is fixed application code; only validated probe dimensions and
// the pinned optional plugin path enter it.
fn script_prefix(width: u16, height: u16) -> String {
    format!(
        "import math
import vapoursynth as vs
core = vs.core
reference = core.std.BlankClip(width={width}, height={height}, format=vs.YUV444P10, length=2, color=[256, 512, 512])
reference = core.std.SetFrameProps(reference, _Matrix=1, _Transfer=1, _Primaries=1, _ColorRange=1)
distorted = core.std.BlankClip(reference, color=[264, 520, 520])
"
    )
}

fn script(metric: Av1anTargetMetric) -> String {
    script_with_plugin(metric, None, 192, 128)
}

fn script_with_plugin(
    metric: Av1anTargetMetric,
    plugin: Option<&Path>,
    width: u16,
    height: u16,
) -> String {
    let scorer = match metric {
        Av1anTargetMetric::Ssimulacra2 => {
            r#"if hasattr(core, 'vship'):
    result = core.vship.SSIMULACRA2(reference, distorted, numStream=4)
    props = ['_SSIMULACRA2']
elif hasattr(core.vszip, 'XPSNR'):
    result = core.vszip.SSIMULACRA2(reference, distorted)
    props = ['SSIMULACRA2']
else:
    result = core.vszip.Metrics(reference, distorted, mode=0)
    props = ['_SSIMULACRA2']
"#
        }
        Av1anTargetMetric::Butteraugli => {
            r#"if hasattr(core, 'vship'):
    result = core.vship.BUTTERAUGLI(reference, distorted, distmap=1, intensity_multiplier=203.0, numStream=4)
    props = ['_BUTTERAUGLI_INFNorm']
else:
    reference = core.resize.Bicubic(reference, format=vs.RGBS, matrix_in_s='709')
    distorted = core.resize.Bicubic(distorted, format=vs.RGBS, matrix_in_s='709')
    result = core.julek.Butteraugli(reference, distorted, distmap=1, intensity_target=203.0)
    props = ['_FrameButteraugli']
"#
        }
        Av1anTargetMetric::Xpsnr | Av1anTargetMetric::XpsnrWeighted => {
            "result = core.vszip.XPSNR(reference, distorted)\nprops = ['XPSNR_Y', 'XPSNR_U', 'XPSNR_V']\n"
        }
        Av1anTargetMetric::Vmaf => unreachable!("VMAF uses FFmpeg"),
    };
    let load = plugin.map_or_else(String::new, |path| {
        let literal = serde_json::to_string(&path.to_string_lossy().to_string())
            .expect("a filesystem path can be serialized as JSON text");
        format!("core.std.LoadPlugin(path={literal})\nassert hasattr(core, 'vship'), 'Vship failed to register'\n")
    });
    format!(
        "{}{load}{scorer}with result.get_frame(0) as frame:\n    values = [float(frame.props[key]) for key in props]\n    assert all(math.isfinite(value) for value in values), values\n    print('JESSES_SCORER_OK', values)\nreference.set_output()\n",
        script_prefix(width, height)
    )
}

/// The package keeps Vship out of the autoload tree. Try it in a disposable
/// child first; only a finite real score and av1an's own discovery permit the
/// final av1an process to inherit the optional plugin directory.
pub(super) struct VshipSelection {
    pub(super) gpu_sha256: Option<String>,
    pub(super) detail: &'static str,
}

fn scorer_binding(
    previous: Option<&recovery::ScorerIdentity>,
    candidate_sha256: Option<&str>,
) -> Result<bool, &'static str> {
    match previous {
        Some(recovery::ScorerIdentity::Cpu) => Ok(false),
        Some(recovery::ScorerIdentity::Vship { sha256 }) => match candidate_sha256 {
            Some(current) if current == sha256 => Ok(true),
            Some(_) => Err(
                "The saved Vship GPU scorer differs from the current bundled DLL. Resume requires the same scorer bytes.",
            ),
            None => Err(
                "The saved Vship GPU scorer is unavailable. Resume cannot switch to CPU scores.",
            ),
        },
        None => Ok(candidate_sha256.is_some()),
    }
}

pub(super) async fn select_bundled_vship(
    av1an: &Path,
    launched_av1an: &Path,
    environment: &mut supervisor::ChildEnvironment,
    work: &Path,
    target: Av1anTargetQuality,
    previous: Option<&recovery::ScorerIdentity>,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<VshipSelection>, AppError> {
    let metric = target.metric;
    if !matches!(
        metric,
        Av1anTargetMetric::Ssimulacra2 | Av1anTargetMetric::Butteraugli
    ) {
        return Ok(None);
    }
    if matches!(previous, Some(recovery::ScorerIdentity::Cpu)) {
        return Ok(Some(VshipSelection {
            gpu_sha256: None,
            detail: "CPU vszip/Julek (resuming CPU scorer history)",
        }));
    }
    let fallback = |detail| {
        scorer_binding(previous, None)
            .map_err(|message| files::error("RECOVERY_INVALID", message, work))?;
        Ok(Some(VshipSelection {
            gpu_sha256: None,
            detail,
        }))
    };
    let Some(frameserver) = crate::bundled_tools::av1an_runtime(av1an)
        .map_err(|detail| files::error("BUNDLED_TOOL_INVALID", detail, av1an))?
    else {
        return if previous.is_some() {
            fallback("CPU vszip/Julek (Vship bundle unavailable)")
        } else {
            Ok(None)
        };
    };
    let plugin = frameserver.join("optional-plugins/libvship_VULKAN.dll");
    if !plugin.is_file() {
        return if previous.is_some() {
            fallback("CPU vszip/Julek (Vship bundle unavailable)")
        } else {
            Ok(None)
        };
    }
    let bytes = std::fs::read(&plugin)
        .map_err(|error| files::error("BUNDLED_TOOL_INVALID", error.to_string(), &plugin))?;
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    scorer_binding(previous, Some(&sha256))
        .map_err(|message| files::error("RECOVERY_INVALID", message, work))?;
    let executable = frameserver.join(if cfg!(windows) {
        "vspipe.exe"
    } else {
        "vspipe"
    });
    let path = work.join(format!(
        "metric-gpu-check-{}.vpy",
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut open = std::fs::OpenOptions::new();
    open.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        open.share_mode(1);
    }
    let file = open
        .open(&path)
        .map_err(|error| files::error("AV1AN_PREPARE_FAILED", error.to_string(), &path))?;
    let mut probe = Script {
        path,
        file: Some(file),
    };
    probe
        .file
        .as_mut()
        .unwrap()
        .write_all(
            script_with_plugin(
                metric,
                Some(&plugin),
                target.probe_width,
                target.probe_height,
            )
            .as_bytes(),
        )
        .map_err(|error| files::error("AV1AN_PREPARE_FAILED", error.to_string(), &probe.path))?;
    let tested = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable,
            args: vec![
                "--info".into(),
                probe.path.as_os_str().to_owned(),
                "-".into(),
            ],
            cwd: Some(work.to_owned()),
        },
        cancel.clone(),
        128 * 1024,
        Duration::from_secs(45),
        Some(environment),
    )
    .await;
    check_cancel(cancel)?;
    if !tested.is_ok_and(|result| {
        result.status.success()
            && (String::from_utf8_lossy(&result.stdout).contains("JESSES_SCORER_OK")
                || String::from_utf8_lossy(&result.stderr).contains("JESSES_SCORER_OK"))
    }) {
        return fallback("CPU vszip/Julek (Vship capability probe unavailable)");
    }
    let optional = plugin
        .parent()
        .expect("Vship has an optional plugin parent");
    let setting = environment
        .variables
        .iter_mut()
        .find(|(name, _)| *name == "VAPOURSYNTH_EXTRA_PLUGIN_PATH");
    if let Some((_, value)) = setting {
        *value = Some(optional.as_os_str().to_owned());
    } else {
        environment.variables.push((
            "VAPOURSYNTH_EXTRA_PLUGIN_PATH",
            Some(optional.as_os_str().to_owned()),
        ));
    }
    let discovered = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable: launched_av1an.to_owned(),
            args: vec!["--version".into()],
            cwd: Some(work.to_owned()),
        },
        cancel.clone(),
        128 * 1024,
        Duration::from_secs(30),
        Some(environment),
    )
    .await;
    check_cancel(cancel)?;
    let selected = discovered.is_ok_and(|result| {
        result.status.success()
            && found(&String::from_utf8_lossy(&result.stdout), "com.lumen.vship")
    });
    if !selected {
        if let Some((_, value)) = environment
            .variables
            .iter_mut()
            .find(|(name, _)| *name == "VAPOURSYNTH_EXTRA_PLUGIN_PATH")
        {
            *value = None;
        }
        return fallback("CPU vszip/Julek (av1an could not activate Vship)");
    }
    Ok(Some(VshipSelection {
        gpu_sha256: Some(sha256),
        detail: "Vship Vulkan GPU (finite frame probe and av1an discovery passed)",
    }))
}

struct Script {
    path: PathBuf,
    file: Option<std::fs::File>,
}
impl Drop for Script {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn check_vapoursynth(
    environment: &supervisor::ChildEnvironment,
    work: &Path,
    metric: Av1anTargetMetric,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let name = if cfg!(windows) {
        "vspipe.exe"
    } else {
        "vspipe"
    };
    let executable = environment
        .path
        .as_ref()
        .and_then(|path| {
            std::env::split_paths(path)
                .map(|directory| directory.join(name))
                .find(|path| path.is_file())
        })
        .ok_or_else(|| {
            dependency_error(
                metric,
                "VSPipe is missing from the selected av1an child PATH.",
                work,
            )
        })?;
    let path = work.join(format!(
        "metric-check-{}.vpy",
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut open = std::fs::OpenOptions::new();
    open.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        open.share_mode(1); // The scorer can read; replacement/writes stay locked.
    }
    let file = open
        .open(&path)
        .map_err(|error| files::error("AV1AN_PREPARE_FAILED", error.to_string(), &path))?;
    let content = script(metric);
    let mut script = Script {
        path,
        file: Some(file),
    };
    script
        .file
        .as_mut()
        .unwrap()
        .write_all(content.as_bytes())
        .map_err(|error| files::error("AV1AN_PREPARE_FAILED", error.to_string(), &script.path))?;
    let result = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable: executable.clone(),
            args: vec![
                "--info".into(),
                script.path.as_os_str().to_owned(),
                "-".into(),
            ],
            cwd: Some(work.to_owned()),
        },
        cancel.clone(),
        128 * 1024,
        Duration::from_secs(45),
        Some(environment),
    )
    .await
    .map_err(|e| process_error(e, &executable))?;
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    if !result.status.success() || !diagnostic.contains("JESSES_SCORER_OK") {
        return Err(dependency_error(metric, &diagnostic, &executable));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_probe_matches_target_frame_shape_and_stream_count() {
        let plugin = Path::new("C:\\Vship\\libvship_VULKAN.dll");
        for metric in [
            Av1anTargetMetric::Ssimulacra2,
            Av1anTargetMetric::Butteraugli,
        ] {
            let gpu = script_with_plugin(metric, Some(plugin), 1920, 1080);
            assert!(gpu.contains("BlankClip(width=1920, height=1080"));
            assert!(gpu.contains("numStream=4"));
            assert!(gpu.contains("core.std.LoadPlugin(path="));
            assert!(gpu.contains("assert hasattr(core, 'vship')"));
            assert!(gpu.contains("JESSES_SCORER_OK"));
        }
        let cpu = script(Av1anTargetMetric::Ssimulacra2);
        assert!(cpu.contains("BlankClip(width=192, height=128"));
        assert!(!cpu.contains("LoadPlugin"));
    }

    #[tokio::test]
    async fn recovery_pins_cpu_and_gpu_scorers_across_attempts() {
        let cpu = recovery::ScorerIdentity::Cpu;
        let old_sha = "a".repeat(64);
        let other_sha = "b".repeat(64);
        let gpu = recovery::ScorerIdentity::Vship {
            sha256: old_sha.clone(),
        };
        // A newly available GPU must not change a CPU-scored recovery.
        assert_eq!(scorer_binding(Some(&cpu), Some(&other_sha)), Ok(false));
        assert_eq!(scorer_binding(Some(&gpu), Some(&old_sha)), Ok(true));
        assert!(scorer_binding(Some(&gpu), Some(&other_sha)).is_err());
        assert!(scorer_binding(Some(&gpu), None).is_err());

        let (_owner, cancel) = watch::channel(false);
        let mut environment = supervisor::ChildEnvironment::default();
        let executable = std::env::current_exe().unwrap();
        let target = Av1anTargetQuality {
            metric: Av1anTargetMetric::Butteraugli,
            minimum_score_tenths: 0,
            maximum_score_tenths: 100,
            minimum_crf: 10,
            maximum_crf: 20,
            probes: 1,
            probing_rate: 1,
            probe_width: 1920,
            probe_height: 1080,
        };
        let selected = select_bundled_vship(
            &executable,
            &executable,
            &mut environment,
            Path::new("."),
            target,
            Some(&cpu),
            &cancel,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(selected.gpu_sha256.is_none());
        assert!(selected.detail.contains("resuming CPU scorer history"));
        assert!(environment.variables.is_empty());
        let unavailable = select_bundled_vship(
            &executable,
            &executable,
            &mut environment,
            Path::new("."),
            target,
            Some(&gpu),
            &cancel,
        )
        .await;
        assert!(
            unavailable
                .err()
                .unwrap()
                .message
                .contains("cannot switch to CPU")
        );
    }

    #[test]
    fn metric_dependencies_and_reader_requirements_match_actual_engine() {
        let mut target = Av1anTargetQuality {
            metric: Av1anTargetMetric::Butteraugli,
            minimum_score_tenths: 8,
            maximum_score_tenths: 12,
            minimum_crf: 15,
            maximum_crf: 50,
            probes: 4,
            probing_rate: 1,
            probe_width: 1920,
            probe_height: 1080,
        };
        assert!(
            validate_plugin(
                "target-probe-filter-v1\ncom.julek.plugin : Found",
                target,
                false,
            )
            .unwrap_err()
            .contains("julek-butteraugli-v1")
        );
        validate_plugin(
            "target-probe-filter-v1\njulek-butteraugli-v1\ncom.julek.plugin : Found",
            target,
            false,
        )
        .unwrap();
        target.metric = Av1anTargetMetric::Ssimulacra2;
        assert!(
            validate_plugin(
                "target-probe-filter-v1\ncom.julek.vszip : Not found",
                target,
                false,
            )
            .is_err()
        );
        validate_plugin(
            "target-probe-filter-v1\ncom.julek.vszip : Found",
            target,
            false,
        )
        .unwrap();
        assert!(
            validate_plugin(
                "target-probe-filter-v1\ncom.julek.vszip : Found\nsystems.innocent.lsmas : Found",
                target,
                false,
            )
            .unwrap_err()
            .contains("lsmash-software-probes-v1")
        );
        validate_plugin(
            "target-probe-filter-v1\nlsmash-software-probes-v1\ncom.julek.vszip : Found\nsystems.innocent.lsmas : Found",
            target,
            false,
        )
        .unwrap();
        for metric in [
            Av1anTargetMetric::Vmaf,
            Av1anTargetMetric::Xpsnr,
            Av1anTargetMetric::XpsnrWeighted,
        ] {
            let checked = Av1anTargetQuality {
                metric,
                probing_rate: 1,
                ..target
            };
            assert!(
                validate_plugin(
                    "target-probe-filter-v1\nffmpeg9-passthrough-v1",
                    checked,
                    false,
                )
                .unwrap_err()
                .contains("ffmpeg-metric-matrix-v1")
            );
            validate_plugin(
                "target-probe-filter-v1\nffmpeg-metric-matrix-v1",
                checked,
                false,
            )
            .unwrap();
        }
        for metric in [
            Av1anTargetMetric::Ssimulacra2,
            Av1anTargetMetric::Butteraugli,
            Av1anTargetMetric::Xpsnr,
            Av1anTargetMetric::XpsnrWeighted,
        ] {
            let filtered = Av1anTargetQuality { metric, ..target };
            assert!(
                validate_plugin(
                    "target-probe-filter-v1\nffmpeg9-passthrough-v1\nffmpeg-metric-matrix-v1\njulek-butteraugli-v1\nlsmash-software-probes-v1\ncom.julek.vszip : Found\ncom.julek.plugin : Found\nsystems.innocent.lsmas : Found",
                    filtered,
                    true,
                )
                .unwrap_err()
                .contains("qualified only for VMAF"),
                "direct-filter {metric:?} must fail before an incomparable reference is scored"
            );
        }
        let mut settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            av1an_options: Some(media_core::Av1anOptions {
                chunk_method: media_core::Av1anChunkMethod::Select,
                target_quality: Some(target),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(options::validate_settings(&settings).is_err());
        settings
            .av1an_options
            .as_mut()
            .unwrap()
            .target_quality
            .as_mut()
            .unwrap()
            .metric = Av1anTargetMetric::Xpsnr;
        options::validate_settings(&settings).unwrap();
        settings
            .av1an_options
            .as_mut()
            .unwrap()
            .target_quality
            .as_mut()
            .unwrap()
            .probing_rate = 2;
        assert!(options::validate_settings(&settings).is_err());
        assert!(script(Av1anTargetMetric::Butteraugli).contains("core.julek.Butteraugli("));
        assert_eq!(cli(Av1anTargetMetric::XpsnrWeighted), "xpsnr-weighted");
        assert_eq!(receipt(Av1anTargetMetric::XpsnrWeighted), "XPSNRWeighted");
        assert!(script(Av1anTargetMetric::XpsnrWeighted).contains("core.vszip.XPSNR("));
    }
}
