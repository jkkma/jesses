//! Exercise the actual scorer API before admitting a target-quality job.
use super::*;
use media_core::{Av1anTargetMetric, Av1anTargetQuality};
use std::io::Write;

pub(super) fn cli(metric: Av1anTargetMetric) -> &'static str {
    match metric {
        Av1anTargetMetric::Vmaf => "vmaf",
        Av1anTargetMetric::Ssimulacra2 => "ssimulacra2",
        Av1anTargetMetric::Butteraugli => "butteraugli-inf",
        Av1anTargetMetric::Xpsnr => "xpsnr",
    }
}

pub(super) fn receipt(metric: Av1anTargetMetric) -> &'static str {
    match metric {
        Av1anTargetMetric::Vmaf => "VMAF",
        Av1anTargetMetric::Ssimulacra2 => "SSIMULACRA2",
        Av1anTargetMetric::Butteraugli => "ButteraugliINF",
        Av1anTargetMetric::Xpsnr => "XPSNR",
    }
}

pub(super) fn label(metric: Av1anTargetMetric) -> &'static str {
    match metric {
        Av1anTargetMetric::Vmaf => "VMAF v0.6.1 (higher is better)",
        Av1anTargetMetric::Ssimulacra2 => "SSIMULACRA2 (higher is better)",
        Av1anTargetMetric::Butteraugli => "Butteraugli INF (lower is better)",
        Av1anTargetMetric::Xpsnr => "XPSNR minimum Y/U/V in dB (higher is better)",
    }
}

pub(super) fn needs_vapoursynth(target: Av1anTargetQuality) -> bool {
    matches!(
        target.metric,
        Av1anTargetMetric::Ssimulacra2 | Av1anTargetMetric::Butteraugli
    ) || (target.metric == Av1anTargetMetric::Xpsnr && target.probing_rate > 1)
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
        return Err("Quality targeting with direct crop, scale, borders, tone-map, or frame-mode deinterlace transforms is currently qualified only for VMAF. Use VMAF, remove those transforms, or first create a lossless transformed source and target that file without another transform.".into());
    }
    if (target.metric == Av1anTargetMetric::Vmaf
        || (target.metric == Av1anTargetMetric::Xpsnr && target.probing_rate == 1))
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
        Av1anTargetMetric::Xpsnr if target.probing_rate > 1 => found(version, "com.julek.vszip"),
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

// The program is fixed application code; no user paths or expressions enter it.
const SCRIPT_PREFIX: &str = r#"import math
import vapoursynth as vs
core = vs.core
reference = core.std.BlankClip(width=192, height=128, format=vs.YUV444P10, length=2, color=[256, 512, 512])
reference = core.std.SetFrameProps(reference, _Matrix=1, _Transfer=1, _Primaries=1, _ColorRange=1)
distorted = core.std.BlankClip(reference, color=[264, 520, 520])
"#;

fn script(metric: Av1anTargetMetric) -> String {
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
        Av1anTargetMetric::Xpsnr => {
            "result = core.vszip.XPSNR(reference, distorted)\nprops = ['XPSNR_Y', 'XPSNR_U', 'XPSNR_V']\n"
        }
        Av1anTargetMetric::Vmaf => unreachable!("VMAF uses FFmpeg"),
    };
    format!(
        "{SCRIPT_PREFIX}{scorer}with result.get_frame(0) as frame:\n    values = [float(frame.props[key]) for key in props]\n    assert all(math.isfinite(value) for value in values), values\n    print('JESSES_SCORER_OK', values)\nreference.set_output()\n"
    )
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
        for metric in [Av1anTargetMetric::Vmaf, Av1anTargetMetric::Xpsnr] {
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
    }
}
