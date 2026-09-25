//! Explicit linear-light HDR/HLG rendering, with a separate SDR output contract.
use media_core::{
    AppError, EncodeBackend, EncodeSettings, ToneMapAlgorithm, ToneMapBackend, ToneMapPeakMode,
    ToneMapSettings,
};
use std::{ffi::OsString, path::Path, time::Duration};
use tokio::sync::watch;

use super::{Stream, unsupported};
use crate::supervisor::{self, CommandSpec};

pub(super) fn validate_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    if let Some(tone) = settings.tone_map {
        if !(100..=10_000).contains(&tone.source_peak_nits) || settings.hdr10_fallback {
            return Err(unsupported(
                "Tone mapping requires a signal peak from 100 to 10000 nits. Use its separate HDR10 base-layer option instead of HDR10 output fallback.",
            ));
        }
        if tone.backend == ToneMapBackend::Gpu && tone.peak_mode == ToneMapPeakMode::Manual {
            return Err(unsupported(
                "GPU tone mapping measures each frame; choose measured peak mode or use the CPU backend for a fixed manual peak.",
            ));
        }
        if tone.algorithm == Some(ToneMapAlgorithm::Spline)
            && (tone.backend == ToneMapBackend::Cpu || tone.peak_mode == ToneMapPeakMode::Manual)
        {
            return Err(unsupported(
                "Spline requires Auto or GPU with measured peak mode. The CPU filter has no Spline curve.",
            ));
        }
        if settings.backend == EncodeBackend::Av1an
            && (tone.backend == ToneMapBackend::Gpu
                || tone.algorithm == Some(ToneMapAlgorithm::Spline))
        {
            return Err(unsupported(
                "AV1AN tone mapping runs on the CPU per chunk. Use Hable, Mobius or Reinhard with CPU or Auto, or choose standalone encoding for GPU/Spline.",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolvedBackend {
    Cpu,
    Gpu,
}

enum GpuProbeError {
    Cancelled,
    Unavailable(String),
}

#[derive(Clone, Debug)]
pub(super) struct Transform {
    pub settings: ToneMapSettings,
    pub hlg: bool,
    pub dv5: bool,
    chroma: &'static str,
    workflow: EncodeBackend,
    resolved: ResolvedBackend,
    effective_peak_nits: f64,
    measured_peak_nits: Option<f64>,
    fallback_note: Option<String>,
}

impl Transform {
    pub fn build(video: &Stream, settings: &EncodeSettings) -> Result<Option<Self>, AppError> {
        let Some(tone) = settings.tone_map else {
            return Ok(None);
        };
        let dv5 = video.side_data_list.iter().any(|value| {
            value
                .get("side_data_type")
                .and_then(serde_json::Value::as_str)
                == Some("DOVI configuration record")
                && value.get("dv_profile").and_then(serde_json::Value::as_u64) == Some(5)
        });
        let hlg = video.color_transfer.as_deref() == Some("arib-std-b67");
        if dv5 {
            if tone.backend == ToneMapBackend::Cpu
                || tone.peak_mode != ToneMapPeakMode::Measured
                || settings.backend != EncodeBackend::Standalone
                || video.codec_name.as_deref() != Some("hevc")
                || video.pix_fmt.as_deref() != Some("yuv420p10le")
                || video.color_range.as_deref() != Some("pc")
                || !matches!(video.chroma_location.as_deref(), Some("left" | "center"))
                || settings.temporal.is_some_and(|temporal| {
                    temporal.deinterlace.is_some()
                        || temporal.qtgmc.is_some()
                        || temporal.cadence_repair.is_some()
                })
                || settings.av1an_grain.is_some()
                || !settings.av1an_filters.is_empty()
            {
                return Err(unsupported(
                    "Dolby Vision profile 5 requires standalone Auto/GPU measured rendering of a full-range 10-bit HEVC source with its RPU intact. CPU, AV1AN and pre-tone frame-changing filters cannot safely render its IPT picture.",
                ));
            }
            if tone.hdr10_base_layer {
                return Err(unsupported(
                    "Dolby Vision profile 5 has no HDR10 base layer. Render its RPU on a usable GPU instead of selecting HDR10 base-layer fallback.",
                ));
            }
        } else if !matches!(
            video.color_transfer.as_deref(),
            Some("smpte2084" | "arib-std-b67")
        ) || video.color_primaries.as_deref() != Some("bt2020")
            || video.color_space.as_deref() != Some("bt2020nc")
            || video.color_range.as_deref() != Some("tv")
            || video.pix_fmt.as_deref() != Some("yuv420p10le")
        {
            return Err(unsupported(
                "Tone mapping requires explicitly tagged limited-range 10-bit 4:2:0 BT.2020 PQ/HDR10 or HLG video. Dolby Vision profile 5 uses a separately validated GPU/RPU route.",
            ));
        }
        let chroma = match video.chroma_location.as_deref() {
            Some("left") => "left",
            Some("topleft") if !dv5 => "topleft",
            Some("center") => "center",
            _ => {
                return Err(unsupported(
                    "Tone mapping requires explicit left, center or top-left source chroma placement.",
                ));
            }
        };
        if hlg && tone.hdr10_base_layer {
            return Err(unsupported(
                "HDR10 base-layer fallback cannot be applied to HLG.",
            ));
        }
        Ok(Some(Self {
            settings: tone,
            hlg,
            dv5,
            chroma,
            workflow: settings.backend,
            resolved: ResolvedBackend::Cpu,
            effective_peak_nits: f64::from(tone.source_peak_nits),
            measured_peak_nits: None,
            fallback_note: None,
        }))
    }

    pub fn filter(&self) -> String {
        if self.resolved == ResolvedBackend::Gpu {
            let algorithm = match self.settings.algorithm.unwrap_or_default() {
                ToneMapAlgorithm::Hable => "hable",
                ToneMapAlgorithm::Mobius => "mobius",
                ToneMapAlgorithm::Reinhard => "reinhard",
                ToneMapAlgorithm::Spline => "spline",
            };
            let input_tags = if self.dv5 {
                // Profile 5 is IPT-PQ with reshaping. BT.2020 setparams here
                // would reinterpret the base samples before RPU application.
                String::new()
            } else {
                format!(
                    "setparams=field_mode=prog:range=tv:color_primaries=bt2020:color_trc={}:colorspace=bt2020nc,",
                    if self.hlg {
                        "arib-std-b67"
                    } else {
                        "smpte2084"
                    }
                )
            };
            let discard_dynamic = if self.settings.hdr10_base_layer && !self.dv5 {
                // The user selected the HDR10 base layer. Keep mastering and
                // content-light data for peak handling, but do not let GPU
                // processing consume Dolby RPU or HDR10+ frame metadata.
                "sidedata=mode=delete:type=DOVI_RPU_BUFFER,sidedata=mode=delete:type=DOVI_METADATA,sidedata=mode=delete:type=DYNAMIC_HDR_PLUS,"
            } else {
                ""
            };
            return format!(
                "{input_tags}{discard_dynamic}libplacebo=tonemapping={algorithm}:peak_detect=1:apply_dolbyvision={}:format=yuv420p10le:colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv,limiter=min=64:max=940:planes=1,limiter=min=64:max=960:planes=6,sidedata=mode=delete,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
                u8::from(self.dv5)
            );
        }
        let algorithm = match self.cpu_algorithm() {
            media_core::ToneMapAlgorithm::Hable => "hable",
            media_core::ToneMapAlgorithm::Mobius => "mobius",
            media_core::ToneMapAlgorithm::Reinhard => "reinhard",
            media_core::ToneMapAlgorithm::Spline => unreachable!("Spline substitutes Hable on CPU"),
        };
        let transfer = if self.hlg {
            "arib-std-b67"
        } else {
            "smpte2084"
        };
        // zimg maps PQ absolute luminance (and its 1000-nit reference HLG EOTF)
        // to linear values relative to npl=100. Both tonemap input and its
        // explicit signal peak therefore use the same 100-nit units.
        // tonemap reads matrix tags for desaturation even on RGB planes. GBR's
        // coefficients sum all channels, so tag the physical BT.709 primaries
        // for its luma calculation; tell final zscale explicitly that the
        // samples are still linear RGB. Neutral-ramp arithmetic tests guard it.
        format!(
            "zscale=pin=bt2020:tin={transfer}:min=bt2020nc:rin=limited:cin={}:p=bt2020:t=linear:m=gbr:r=full:npl=100:agamma=0,format=gbrpf32le,zscale=p=bt709,setparams=colorspace=bt709,tonemap=tonemap={algorithm}:desat=2:peak={:.2},zscale=pin=bt709:tin=linear:min=gbr:rin=full:p=bt709:t=bt709:m=bt709:r=limited:c=left:dither=error_diffusion:agamma=0,format=yuv420p10le,limiter=min=64:max=940:planes=1,limiter=min=64:max=960:planes=6,sidedata=mode=delete,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
            self.chroma,
            self.effective_peak_nits / 100.0
        )
    }

    fn cpu_algorithm(&self) -> ToneMapAlgorithm {
        match self.settings.algorithm.unwrap_or_default() {
            ToneMapAlgorithm::Spline => ToneMapAlgorithm::Hable,
            other => other,
        }
    }

    pub fn device_args(&self) -> Vec<OsString> {
        if self.resolved == ResolvedBackend::Gpu {
            vec!["-init_hw_device".into(), "vulkan".into()]
        } else {
            Vec::new()
        }
    }

    pub fn description(&self) -> String {
        match self.resolved {
            ResolvedBackend::Gpu => format!(
                "GPU libplacebo tone mapping with per-frame peak detection and {:?} curve{}.",
                self.settings.algorithm.unwrap_or_default(),
                if self.dv5 {
                    "; Dolby Vision RPU applied"
                } else {
                    ""
                }
            ),
            ResolvedBackend::Cpu => format!(
                "CPU zscale tone mapping with {:?} curve and {:.0}-nit peak{}{}.",
                self.cpu_algorithm(),
                self.effective_peak_nits,
                self.measured_peak_nits
                    .map_or(String::new(), |value| format!(
                        " (sampled {:.0} nits)",
                        value
                    )),
                self.fallback_note
                    .as_ref()
                    .map_or(String::new(), |note| format!(" {note}"))
            ),
        }
    }

    pub async fn check_tools(
        &self,
        ffmpeg: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        let filter = format!("format=yuv420p10le,{}", self.filter());
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=64x64:r=1,format=yuv420p10le",
            "-frames:v",
            "1",
            "-vf",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        args.extend([
            filter.into(),
            "-pix_fmt".into(),
            "+yuv420p10le".into(),
            "-f".into(),
            "rawvideo".into(),
            "pipe:1".into(),
        ]);
        let captured = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            128 * 1024,
            Duration::from_secs(10),
        )
        .await
        .map_err(|error| crate::jobs::process_error(error, ffmpeg))?;
        if !captured.status.success() || captured.stdout.len() != 64 * 64 * 3 {
            return Err(AppError::new(
                "TONE_MAP_TOOL_UNSUPPORTED",
                format!(
                    "The installed FFmpeg could not run the required linear-light zscale/Hable pipeline: {}",
                    String::from_utf8_lossy(&captured.stderr)
                ),
                None,
            ));
        }
        Ok(())
    }

    pub async fn resolve(
        &mut self,
        ffmpeg: &Path,
        source: &Path,
        video_index: u32,
        duration_seconds: Option<f64>,
        declared_peak_nits: Option<f64>,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        if *cancel.borrow() {
            return Err(crate::jobs::canceled());
        }
        let try_gpu = self.workflow == EncodeBackend::Standalone
            && self.settings.peak_mode == ToneMapPeakMode::Measured
            && matches!(
                self.settings.backend,
                ToneMapBackend::Auto | ToneMapBackend::Gpu
            );
        if try_gpu {
            match self.probe_gpu(ffmpeg, cancel).await {
                Ok(()) => {
                    if *cancel.borrow() {
                        return Err(crate::jobs::canceled());
                    }
                    self.resolved = ResolvedBackend::Gpu;
                    return Ok(());
                }
                Err(GpuProbeError::Cancelled) => return Err(crate::jobs::canceled()),
                Err(GpuProbeError::Unavailable(problem)) => {
                    if *cancel.borrow() {
                        return Err(crate::jobs::canceled());
                    }
                    if self.dv5 || self.settings.backend == ToneMapBackend::Gpu {
                        return Err(AppError::new(
                            "TONE_MAP_GPU_UNAVAILABLE",
                            format!(
                                "A real GPU with working Vulkan/libplacebo is required for this tone map: {problem}"
                            ),
                            None,
                        ));
                    }
                    self.fallback_note = Some(format!(
                        "GPU unavailable ({problem}); CPU fallback selected."
                    ));
                }
            }
        }
        if self.dv5 {
            return Err(unsupported(
                "Dolby Vision profile 5 cannot be rendered by the CPU tone-map chain; its RPU must be applied by a real GPU.",
            ));
        }
        if self.settings.algorithm == Some(ToneMapAlgorithm::Spline) {
            self.fallback_note = Some(format!(
                "{} Spline is unavailable on CPU; Hable was selected.",
                self.fallback_note.as_deref().unwrap_or_default()
            ));
        }
        self.check_tools(ffmpeg, cancel).await?;
        if !self.hlg && self.settings.peak_mode == ToneMapPeakMode::Measured {
            self.measured_peak_nits =
                measure_peak(ffmpeg, source, video_index, duration_seconds, cancel).await?;
            let declared = declared_peak_nits.filter(|value| *value > 0.0 && *value < 10_000.0);
            if let Some(measured) = self.measured_peak_nits {
                let headroom = measured * 2.0;
                self.effective_peak_nits = declared
                    .map_or(headroom, |value| headroom.min(value))
                    .max(measured)
                    .clamp(100.0, 10_000.0);
            } else if let Some(declared) = declared {
                self.effective_peak_nits = declared.clamp(100.0, 10_000.0);
            }
        }
        Ok(())
    }

    async fn probe_gpu(
        &self,
        ffmpeg: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), GpuProbeError> {
        if *cancel.borrow() {
            return Err(GpuProbeError::Cancelled);
        }
        let mut candidate = self.clone();
        candidate.resolved = ResolvedBackend::Gpu;
        let filter = format!(
            "format=yuv420p10le,setparams=field_mode=prog:range=tv:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc,{}",
            candidate.filter()
        );
        let args: Vec<OsString> = [
            OsString::from("-hide_banner"),
            OsString::from("-v"),
            OsString::from("verbose"),
            OsString::from("-nostdin"),
            OsString::from("-f"),
            OsString::from("lavfi"),
            OsString::from("-i"),
            OsString::from("color=black:s=64x64:r=1"),
            OsString::from("-init_hw_device"),
            OsString::from("vulkan"),
            OsString::from("-vf"),
            OsString::from(filter),
            OsString::from("-frames:v"),
            OsString::from("1"),
            OsString::from("-f"),
            OsString::from("md5"),
            OsString::from("pipe:1"),
        ]
        .into();
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            1024 * 1024,
            Duration::from_secs(30),
        )
        .await
        .map_err(|error| match error {
            supervisor::SupervisorError::Cancelled => GpuProbeError::Cancelled,
            other => GpuProbeError::Unavailable(other.to_string()),
        })?;
        let stderr = String::from_utf8_lossy(&result.stderr);
        let device = stderr
            .lines()
            .find(|line| line.contains(" selected: "))
            .ok_or_else(|| {
                GpuProbeError::Unavailable(
                    "FFmpeg did not identify the selected Vulkan device".into(),
                )
            })?;
        if !device.contains("(discrete)") && !device.contains("(integrated)") {
            return Err(GpuProbeError::Unavailable(format!(
                "selected Vulkan device is not a confirmed hardware GPU: {device}"
            )));
        }
        if !result.status.success() || !result.stdout.starts_with(b"MD5=") {
            return Err(GpuProbeError::Unavailable(format!(
                "libplacebo could not render a complete test frame: {}",
                stderr.lines().rev().take(4).collect::<Vec<_>>().join(" ")
            )));
        }
        Ok(())
    }
}

fn pq_code_to_nits(code: u16) -> f64 {
    let signal = ((f64::from(code) - 64.0) / 876.0).clamp(0.0, 1.0);
    let m1 = 2610.0 / 16384.0;
    let m2 = 2523.0 / 32.0;
    let c1 = 3424.0 / 4096.0;
    let c2 = 2413.0 / 128.0;
    let c3 = 2392.0 / 128.0;
    let power = signal.powf(1.0 / m2);
    (10_000.0 * ((power - c1).max(0.0) / (c2 - c3 * power)).powf(1.0 / m1)).clamp(0.0, 10_000.0)
}

async fn measure_peak(
    ffmpeg: &Path,
    source: &Path,
    video_index: u32,
    duration_seconds: Option<f64>,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<f64>, AppError> {
    let duration = duration_seconds
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(0.0);
    let points = if duration > 1.0 { 12 } else { 1 };
    let frames = if points == 1 { 60 } else { 5 };
    let mut maximum = None::<u16>;
    for point in 0..points {
        if *cancel.borrow() {
            return Err(crate::jobs::canceled());
        }
        let position = if points == 1 {
            0.0
        } else {
            duration * (f64::from(point) + 0.5) / f64::from(points)
        };
        let args: Vec<OsString> = [
            OsString::from("-hide_banner"),
            OsString::from("-v"),
            OsString::from("info"),
            OsString::from("-nostdin"),
            OsString::from("-ss"),
            OsString::from(format!("{position:.3}")),
            OsString::from("-protocol_whitelist"),
            OsString::from("file"),
            OsString::from("-i"),
            source.as_os_str().to_owned(),
            OsString::from("-map"),
            OsString::from(format!("0:{video_index}")),
            OsString::from("-an"),
            OsString::from("-sn"),
            OsString::from("-frames:v"),
            OsString::from(frames.to_string()),
            OsString::from("-vf"),
            OsString::from("signalstats,metadata=mode=print:key=lavfi.signalstats.YMAX"),
            OsString::from("-f"),
            OsString::from("null"),
            OsString::from("-"),
        ]
        .into();
        let captured = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            512 * 1024,
            Duration::from_secs(120),
        )
        .await
        .map_err(|error| crate::jobs::process_error(error, source))?;
        if !captured.status.success() {
            return Ok(None);
        }
        for line in String::from_utf8_lossy(&captured.stderr).lines() {
            if let Some((_, value)) = line.split_once("lavfi.signalstats.YMAX=")
                && let Ok(code) = value.trim().parse::<u16>()
                && code <= 1023
            {
                maximum = Some(maximum.map_or(code, |prior| prior.max(code)));
            }
        }
    }
    Ok(maximum.map(pq_code_to_nits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pq_eotf_and_declared_peak_headroom_are_bounded() {
        assert_eq!(pq_code_to_nits(64), 0.0);
        assert!((pq_code_to_nits(940) - 10_000.0).abs() < 0.1);
        assert!((pq_code_to_nits(508) - 100.0).abs() < 20.0);
    }

    #[test]
    fn gpu_profile5_applies_rpu_but_hdr10_base_layer_strips_it_first() {
        let normal: Stream = serde_json::from_value(serde_json::json!({
            "index":0,"codec_type":"video","codec_name":"hevc",
            "pix_fmt":"yuv420p10le","color_range":"tv",
            "color_primaries":"bt2020","color_transfer":"smpte2084",
            "color_space":"bt2020nc","chroma_location":"left"
        }))
        .unwrap();
        let dv5: Stream = serde_json::from_value(serde_json::json!({
            "index":0,"codec_type":"video","codec_name":"hevc",
            "pix_fmt":"yuv420p10le","color_range":"pc",
            "chroma_location":"left",
            "side_data_list":[{"side_data_type":"DOVI configuration record","dv_profile":5}]
        }))
        .unwrap();
        let tone: ToneMapSettings = serde_json::from_value(serde_json::json!({
            "sourcePeakNits":1000,"backend":"gpu","peakMode":"measured"
        }))
        .unwrap();
        let mut base = Transform::build(
            &normal,
            &EncodeSettings {
                tone_map: Some(ToneMapSettings {
                    hdr10_base_layer: true,
                    ..tone
                }),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        base.resolved = ResolvedBackend::Gpu;
        let base_filter = base.filter();
        assert!(base_filter.contains("apply_dolbyvision=0"));
        assert!(base_filter.contains("sidedata=mode=delete:type=DOVI_RPU_BUFFER"));
        assert!(base_filter.contains("sidedata=mode=delete:type=DYNAMIC_HDR_PLUS"));
        let mut rendered = Transform::build(
            &dv5,
            &EncodeSettings {
                tone_map: Some(tone),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        rendered.resolved = ResolvedBackend::Gpu;
        let dv5_filter = rendered.filter();
        assert!(dv5_filter.contains("apply_dolbyvision=1"));
        assert!(!dv5_filter.contains("sidedata=mode=delete:type=DOVI_RPU_BUFFER"));
        assert!(!dv5_filter.starts_with("setparams=color_primaries=bt2020"));
        assert_eq!(rendered.device_args(), ["-init_hw_device", "vulkan"]);
    }

    #[test]
    fn gpu_cannot_discard_a_manual_peak_and_cpu_cannot_offer_spline() {
        let settings = |backend, peak_mode, algorithm| EncodeSettings {
            tone_map: Some(ToneMapSettings {
                backend,
                peak_mode,
                algorithm,
                source_peak_nits: 1000,
                hdr10_base_layer: false,
            }),
            ..Default::default()
        };
        assert!(
            validate_settings(&settings(
                ToneMapBackend::Gpu,
                ToneMapPeakMode::Manual,
                None
            ))
            .is_err()
        );
        assert!(
            validate_settings(&settings(
                ToneMapBackend::Cpu,
                ToneMapPeakMode::Measured,
                Some(ToneMapAlgorithm::Spline)
            ))
            .is_err()
        );
        assert!(
            validate_settings(&settings(
                ToneMapBackend::Auto,
                ToneMapPeakMode::Measured,
                Some(ToneMapAlgorithm::Spline)
            ))
            .is_ok()
        );
    }

    #[tokio::test]
    async fn canceled_gpu_probe_does_not_fall_back_or_report_gpu_unavailable() {
        let video: Stream = serde_json::from_value(serde_json::json!({
            "index":0,"codec_type":"video","codec_name":"hevc",
            "pix_fmt":"yuv420p10le","color_range":"tv",
            "color_primaries":"bt2020","color_transfer":"smpte2084",
            "color_space":"bt2020nc","chroma_location":"left"
        }))
        .unwrap();
        let (_owner, cancel) = watch::channel(true);
        for backend in [ToneMapBackend::Auto, ToneMapBackend::Gpu] {
            let mut transform = Transform::build(
                &video,
                &EncodeSettings {
                    tone_map: Some(ToneMapSettings {
                        backend,
                        peak_mode: ToneMapPeakMode::Measured,
                        source_peak_nits: 1000,
                        algorithm: None,
                        hdr10_base_layer: false,
                    }),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
            assert!(matches!(
                transform
                    .probe_gpu(Path::new("missing-ffmpeg"), &cancel)
                    .await,
                Err(GpuProbeError::Cancelled)
            ));
            let error = transform
                .resolve(
                    Path::new("missing-ffmpeg"),
                    Path::new("missing-source"),
                    0,
                    Some(1.0),
                    None,
                    &cancel,
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, "JOB_CANCELED");
        }
    }
}
