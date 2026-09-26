use media_core::{AppError, EncodeBackend, EncodeSettings, VideoEncoder};
use serde::Deserialize;

use super::metadata::{Document, Stream};

mod aspect;
mod framing;
mod hdr;
mod temporal;
mod tone_map;
use hdr::{Hdr10, StaticMetadata, validate_side_data};
mod validation;
pub(super) use validation::Validation;

#[derive(Clone, Debug)]
pub(super) struct Plan {
    pub trim: Option<media_core::VideoTrim>,
    tone_map: Option<tone_map::Transform>,
    grain_prefilter: Option<u8>,
    prepare_metric_reference: bool,
    custom_filters: Vec<String>,
    temporal: Option<temporal::Transform>,
    pub encoder: VideoEncoder,
    source_pixel_format: &'static str,
    source_is_vp9_alpha: bool,
    pub output_pixel_format: &'static str,
    pub video_index: u32,
    pub width: u32,
    pub height: u32,
    geometry: framing::Geometry,
    aspect: Option<aspect::Transform>,
    source_sar: String,
    output_sar: String,
    pub fps_num: u32,
    pub fps_den: u32,
    pub cadence_reconciled: bool,
    pub tolerance: f64,
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
    pub chroma: &'static str,
    hdr10: Option<Hdr10>,
}

pub(super) fn unsupported(message: &str) -> AppError {
    AppError::new("ENCODE_INPUT_UNSUPPORTED", message, None)
}

pub(super) fn validate_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    super::external_tracks::validate(settings)?;
    super::parameters::validate(settings)?;
    temporal::validate(settings)?;
    super::av1an::validate_settings(settings)?;
    super::av1an::validate_grain(settings)?;
    super::av1an::validate_filters(settings)?;
    let av1an_format = settings
        .av1an_options
        .and_then(|options| options.pixel_format);
    if let (Some(shared), Some(av1an)) = (settings.output_pixel_format, av1an_format)
        && shared != av1an
    {
        return Err(AppError::new(
            "ENCODE_SETTINGS_INVALID",
            "The shared output pixel format conflicts with av1an's selected pixel format.",
            None,
        ));
    }
    if settings.backend == EncodeBackend::Av1an && settings.output_pixel_format.is_some() {
        return Err(AppError::new(
            "ENCODE_SETTINGS_INVALID",
            "Set av1an's output pixel format in av1an options; the shared output pixel format belongs to standalone encoding.",
            None,
        ));
    }
    if let Some(format) = settings.output_pixel_format.or(av1an_format) {
        use media_core::Av1anPixelFormat as Pixel;
        if format == Pixel::Yuva420p
            && !(settings.backend == EncodeBackend::Standalone
                && settings.encoder == VideoEncoder::Vp9)
        {
            return Err(AppError::new(
                "ENCODE_SETTINGS_INVALID",
                "Alpha-bearing YUVA 4:2:0 output requires standalone FFmpeg VP9.",
                None,
            ));
        }
        let allowed = match settings.encoder {
            VideoEncoder::SvtAv1
            | VideoEncoder::SvtAv1FiveFish
            | VideoEncoder::SvtAv1Hdr
            | VideoEncoder::H264Nvenc
            | VideoEncoder::HevcNvenc => matches!(format, Pixel::Yuv420p | Pixel::Yuv420p10le),
            VideoEncoder::VpxStandalone => !matches!(format, Pixel::Yuv422p | Pixel::Yuv422p10le),
            VideoEncoder::Vp9 => matches!(
                format,
                Pixel::Yuv420p
                    | Pixel::Yuv420p10le
                    | Pixel::Yuv444p
                    | Pixel::Yuv444p10le
                    | Pixel::Yuva420p
            ),
            VideoEncoder::X264
            | VideoEncoder::X265
            | VideoEncoder::X265Standalone
            | VideoEncoder::AomAv1 => format != Pixel::Yuva420p,
        };
        if !allowed {
            return Err(AppError::new(
                "ENCODE_SETTINGS_INVALID",
                "The selected encoder does not support that output chroma and bit depth. Choose a supported pixel format.",
                None,
            ));
        }
    }
    super::rate_control::validate(settings)?;
    tone_map::validate_settings(settings)?;
    super::trim::validate_settings(settings)?;
    super::subtitles::validate_settings(settings)?;
    super::audio::validate_settings(settings)?;
    framing::validate(settings)?;
    let fork_options_valid = (if settings.encoder == VideoEncoder::SvtAv1FiveFish {
        settings.lineart_psy_bias <= 7 && settings.texture_psy_bias <= 7
    } else {
        settings.lineart_psy_bias == 0 && settings.texture_psy_bias == 0
    }) && (settings.encoder == VideoEncoder::SvtAv1Hdr
        || settings.hdr_tune == media_core::HdrTune::VisualQuality);
    if !fork_options_valid {
        return Err(AppError::new(
            "ENCODE_SETTINGS_INVALID",
            "Line-art and texture bias (0–7) belong to SVT-AV1 5fish. The film-grain tune belongs to SVT-AV1-HDR. Reset settings that belong to another build.",
            None,
        ));
    }
    if !settings.encoder.is_svt()
        && (settings.svt_crf_quarter_steps.is_some() || settings.svt_preset.is_some())
    {
        return Err(AppError::new(
            "ENCODE_SETTINGS_INVALID",
            "Fractional/extended CRF and research presets belong to SVT encoders. Reset settings that belong to another encoder.",
            None,
        ));
    }
    let standalone = settings.backend == EncodeBackend::Standalone;
    let valid = match settings.encoder {
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
            settings.svt_crf_quarter_steps.map_or_else(
                || (1..=63).contains(&settings.crf),
                |value| (4..=280).contains(&value),
            ) && settings
                .svt_preset
                .map_or(settings.preset <= 13, |value| (-3..=13).contains(&value))
                && settings.film_grain <= 50
        }
        VideoEncoder::X264 => {
            settings.crf <= 51
                && settings.preset <= 9
                && settings.film_grain == 0
                && !settings.hdr10_fallback
        }
        VideoEncoder::X265 => {
            standalone
                && settings.crf <= 51
                && settings.preset <= 9
                && settings.film_grain == 0
                && !settings.hdr10_fallback
        }
        VideoEncoder::X265Standalone => {
            settings.crf <= 51
                && settings.preset <= 9
                && settings.film_grain == 0
                && !settings.hdr10_fallback
        }
        VideoEncoder::Vp9 => {
            standalone
                && settings.crf <= 63
                && settings.preset <= 5
                && settings.film_grain == 0
                && !settings.hdr10_fallback
        }
        VideoEncoder::AomAv1 => {
            settings.crf <= 63
                && settings.preset <= 8
                && settings.film_grain <= 50
                && !settings.hdr10_fallback
        }
        VideoEncoder::VpxStandalone => {
            settings.crf <= 63
                && settings.preset <= 8
                && settings.film_grain == 0
                && !settings.hdr10_fallback
        }
        VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc => {
            standalone
                && settings.crf <= 51
                && settings.preset <= 6
                && settings.film_grain == 0
                && !settings.hdr10_fallback
        }
    };
    if !valid || !(1..=32).contains(&settings.workers) {
        return Err(AppError::new(
            "ENCODE_SETTINGS_INVALID",
            match settings.encoder {
                VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
                    "Use SVT CRF 1–70 in 0.25 steps, an advertised preset from -3–13, film grain synthesis from 0–50, and 1–32 workers."
                }
                VideoEncoder::X264 => {
                    "x264 requires CRF 0–51, preset 0–9, grain 0, HDR10 fallback off, and 1–32 workers."
                }
                VideoEncoder::X265 | VideoEncoder::X265Standalone => {
                    "x265 requires CRF 0–51, preset 0–9, grain 0, HDR10 fallback off, and 1–32 workers."
                }
                VideoEncoder::Vp9 => {
                    "VP9 requires standalone mode, CRF 0–63, speed preset 0–5, grain 0, HDR10 fallback off, and 1–32 workers."
                }
                VideoEncoder::AomAv1 => {
                    "AOM requires CRF 0–63, speed preset 0–8, film grain synthesis 0–50, HDR10 fallback off, and 1–32 workers."
                }
                VideoEncoder::VpxStandalone => {
                    "VPX requires CRF 0–63, speed preset 0–8, grain 0, HDR10 fallback off, and 1–32 workers."
                }
                VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc => {
                    "NVENC requires standalone mode, quality 0–51, preset P1–P7, grain 0, HDR10 fallback off, and 1–32 workers."
                }
            },
            None,
        ));
    }
    Ok(())
}

fn rational(text: Option<&str>) -> Option<(u32, u32)> {
    let (a, b) = text?.split_once('/')?;
    let (a, b) = (a.parse().ok()?, b.parse().ok()?);
    (a > 0 && b > 0).then_some((a, b))
}

fn seconds(text: Option<&str>) -> Option<f64> {
    text?.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn sdr_color(text: Option<&str>) -> Result<u8, AppError> {
    match text {
        Some("bt709") => Ok(1),
        Some("bt470bg") => Ok(5),
        Some("smpte170m") => Ok(6),
        _ => Err(unsupported(
            "Encoding requires explicitly tagged BT.709, BT.470BG, or SMPTE 170M SDR, or limited-range 10-bit BT.2020/PQ HDR10.",
        )),
    }
}

fn source_pixel_format(
    format: Option<&str>,
    full_range: bool,
) -> Result<(&'static str, u8), AppError> {
    match format {
        Some("yuv420p") => Ok(("yuv420p", 8)),
        Some("yuv422p") => Ok(("yuv422p", 8)),
        Some("yuv444p") => Ok(("yuv444p", 8)),
        Some("yuv420p10le") => Ok(("yuv420p10le", 10)),
        Some("yuv422p10le") => Ok(("yuv422p10le", 10)),
        Some("yuv444p10le") => Ok(("yuv444p10le", 10)),
        Some("yuv420p12le") => Ok(("yuv420p12le", 12)),
        Some("yuv422p12le") => Ok(("yuv422p12le", 12)),
        Some("yuv444p12le") => Ok(("yuv444p12le", 12)),
        Some("yuyv422") => Ok(("yuyv422", 8)),
        Some("uyvy422") => Ok(("uyvy422", 8)),
        Some("rgb24") if full_range => Ok(("rgb24", 8)),
        Some("bgr24") if full_range => Ok(("bgr24", 8)),
        Some("gbrp") if full_range => Ok(("gbrp", 8)),
        Some("gbrp10le") if full_range => Ok(("gbrp10le", 10)),
        Some("gbrp12le") if full_range => Ok(("gbrp12le", 12)),
        Some("gbrp16le") if full_range => Ok(("gbrp16le", 16)),
        Some("rgb48le") if full_range => Ok(("rgb48le", 16)),
        Some("rgb48be") if full_range => Ok(("rgb48be", 16)),
        Some("rgba") if full_range => Ok(("rgba", 8)),
        Some("yuva420p") => Ok(("yuva420p", 8)),
        Some("yuvj420p") if full_range => Ok(("yuvj420p", 8)),
        Some("yuvj422p") if full_range => Ok(("yuvj422p", 8)),
        Some("yuvj444p") if full_range => Ok(("yuvj444p", 8)),
        _ => Err(unsupported(
            "Encoding requires explicitly tagged planar YUV 4:2:0/4:2:2/4:4:4 (8/10/12-bit), packed YUYV/UYVY 4:2:2, full-range opaque RGB/GBR (8–16-bit), or supported 8-bit RGBA/YUVA420P for standalone VP9 alpha output. Other alpha-bearing, packed, and hardware-only formats need a separate conversion workflow.",
        )),
    }
}

fn rgb_source_format(format: &str) -> bool {
    matches!(
        format,
        "rgb24"
            | "bgr24"
            | "rgba"
            | "gbrp"
            | "gbrp10le"
            | "gbrp12le"
            | "gbrp16le"
            | "rgb48le"
            | "rgb48be"
    )
}

impl Plan {
    pub fn build(
        document: &Document,
        selected: &[&Stream],
        settings: &EncodeSettings,
    ) -> Result<Self, AppError> {
        validate_settings(settings)?;
        super::audio::validate_selection(selected, settings)?;
        super::trim::validate_selection(selected, settings)?;
        super::subtitles::validate_selection(selected, settings)?;
        let videos: Vec<_> = selected
            .iter()
            .filter(|s| s.codec_type.as_deref() == Some("video"))
            .collect();
        if videos.len() != 1 || videos[0].index != settings.video_stream_index {
            return Err(unsupported(
                "Select exactly one video stream and make it the video chosen for encoding.",
            ));
        }
        let video = videos[0];
        let tone_map = tone_map::Transform::build(video, settings)?;
        let source_has_alpha_tag = video
            .tags
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("alpha_mode") && value == "1");
        let source_is_vp9_alpha = source_has_alpha_tag
            && video.codec_name.as_deref() == Some("vp9")
            && video.pix_fmt.as_deref() == Some("yuv420p");
        if source_has_alpha_tag && !source_is_vp9_alpha {
            return Err(unsupported(
                "An alpha_mode source needs supported VP9 8-bit 4:2:0 alpha metadata and an explicit alpha-preserving output.",
            ));
        }
        let (source_pixel_format, source_depth) = source_pixel_format(
            if source_is_vp9_alpha {
                Some("yuva420p")
            } else {
                video.pix_fmt.as_deref()
            },
            video.color_range.as_deref() == Some("pc"),
        )?;
        let rgb_source = rgb_source_format(source_pixel_format);
        let alpha_source = matches!(source_pixel_format, "rgba" | "yuva420p");
        if settings.encoder.is_svt()
            && video.color_range.as_deref() == Some("pc")
            && !rgb_source
            && tone_map.is_none()
        {
            return Err(unsupported(
                "SVT requires limited-range YUV. Full-range opaque RGB can be explicitly converted to limited-range BT.709 output.",
            ));
        }
        if rgb_source
            && (video.color_space.as_deref() != Some("gbr")
                || video.color_primaries.as_deref() != Some("bt709")
                || video.color_transfer.as_deref() != Some("bt709"))
        {
            return Err(unsupported(
                "RGB/GBR input requires full-range BT.709 primaries and transfer with an explicit GBR identity matrix. RGBA alpha requires standalone VP9 YUVA output.",
            ));
        }
        if settings.temporal.is_none_or(|temporal| {
            temporal.deinterlace.is_none()
                && temporal.qtgmc.is_none()
                && temporal.cadence_repair.is_none()
        }) && !matches!(
            video.field_order.as_deref(),
            None | Some("progressive" | "unknown")
        ) {
            return Err(unsupported(
                "Interlaced video requires explicit BWDIF, QTGMC, or inverse telecine processing and the matching TFF/BFF source field order.",
            ));
        }
        let source_sar = aspect::parse_source(video.sample_aspect_ratio.as_deref())?;
        if source_sar != "1:1"
            && settings
                .temporal
                .and_then(|temporal| temporal.aspect_ratio)
                .is_none()
        {
            return Err(unsupported(
                "A non-square source requires an explicit output SAR or DAR conversion.",
            ));
        }
        let is_hdr10 = video.color_primaries.as_deref() == Some("bt2020")
            && video.color_transfer.as_deref() == Some("smpte2084")
            && video.color_space.as_deref() == Some("bt2020nc")
            && video.pix_fmt.as_deref() == Some("yuv420p10le")
            && video.color_range.as_deref() == Some("tv");
        let explicit_pixel_format = settings
            .output_pixel_format
            .or_else(|| {
                settings
                    .av1an_options
                    .and_then(|options| options.pixel_format)
            })
            .map(media_core::Av1anPixelFormat::ffmpeg);
        if alpha_source != (explicit_pixel_format == Some("yuva420p")) {
            return Err(unsupported(
                "Alpha-bearing RGBA/YUVA420P input requires explicit standalone VP9 YUVA420P output; YUVA output requires a source with alpha.",
            ));
        }
        if alpha_source
            && (settings.temporal.is_some()
                || !settings.av1an_filters.is_empty()
                || settings
                    .subtitles
                    .iter()
                    .any(|subtitle| subtitle.mode == media_core::SubtitleMode::BurnIn))
        {
            return Err(unsupported(
                "VP9 alpha preservation currently supports trim and framing, but not temporal filters, custom pixel filters, or subtitle burn-in.",
            ));
        }
        if is_hdr10
            && tone_map.is_none()
            && explicit_pixel_format.is_some_and(|format| format != "yuv420p10le")
        {
            return Err(unsupported(
                "HDR10 output requires 10-bit 4:2:0. Choose 10-bit output or enable SDR tone mapping.",
            ));
        }
        if is_hdr10
            && !matches!(
                settings.encoder,
                VideoEncoder::SvtAv1
                    | VideoEncoder::SvtAv1FiveFish
                    | VideoEncoder::SvtAv1Hdr
                    | VideoEncoder::AomAv1
                    | VideoEncoder::X265Standalone
            )
            && tone_map.is_none()
        {
            return Err(unsupported(
                "The selected encoder supports SDR output only. Select SVT-AV1, AOM, or standalone x265 for preserved HDR10, or enable explicit HDR/HLG-to-SDR tone mapping.",
            ));
        }
        let hdr10 = if is_hdr10 || tone_map.as_ref().is_some_and(|tone| tone.hlg || tone.dv5) {
            Some(Hdr10::build(
                &video.side_data_list,
                settings.hdr10_fallback
                    || tone_map
                        .as_ref()
                        .is_some_and(|tone| tone.settings.hdr10_base_layer || tone.dv5),
                tone_map.as_ref().is_some_and(|tone| tone.dv5),
            )?)
        } else {
            None
        };
        if hdr10.as_ref().is_some_and(|hdr| hdr.dolby.is_some())
            && video.codec_name.as_deref() != Some("hevc")
        {
            return Err(unsupported(
                "Dolby Vision HDR10 fallback supports HEVC profile 7/8 base layers only.",
            ));
        }
        validate_side_data(&video.side_data_list, hdr10.as_ref(), false)?;
        if video
            .tags
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("rotate") && value != "0")
        {
            return Err(unsupported(
                "Rotation and display transforms are not supported by this encoding workflow.",
            ));
        }
        if video
            .disposition
            .get("attached_pic")
            .is_some_and(|v| *v != 0)
        {
            return Err(unsupported(
                "Choose the movie video track, not an attached cover image.",
            ));
        }
        let (width, height) = match (video.width, video.height) {
            (Some(w), Some(h))
                if (64..=8192).contains(&w)
                    && (64..=8192).contains(&h)
                    && w % 2 == 0
                    && h % 2 == 0 =>
            {
                (w, h)
            }
            _ => {
                return Err(unsupported(
                    "Encoding requires even dimensions between 64 and 8192 pixels.",
                ));
            }
        };
        let mut geometry = framing::Geometry::build(width, height, settings.framing)?;
        geometry.resize_filter = settings
            .temporal
            .map_or_default(|temporal| temporal.resize_filter);
        let aspect = settings
            .temporal
            .and_then(|temporal| temporal.aspect_ratio)
            .map(|aspect| aspect::Transform::build(aspect, geometry.width, geometry.height))
            .transpose()?;
        let output_sar = aspect.map_or_else(|| "1:1".into(), aspect::Transform::sar);
        let (fps_num, fps_den) = rational(video.avg_frame_rate.as_deref())
            .or_else(|| rational(video.r_frame_rate.as_deref()))
            .ok_or_else(|| unsupported("The source has no usable rational frame rate."))?;
        let fps = f64::from(fps_num) / f64::from(fps_den);
        if !(1.0..=120.0).contains(&fps) {
            return Err(unsupported(
                "Encoding supports constant frame rates from 1 through 120 fps.",
            ));
        }
        let tick = rational(video.time_base.as_deref())
            .map(|(n, d)| f64::from(n) / f64::from(d))
            .unwrap_or(0.000001);
        let tolerance = tick.clamp(0.000001, 0.001) + 0.000002;
        for start in [
            video.start_time.as_deref(),
            document
                .format
                .as_ref()
                .and_then(|f| f.start_time.as_deref()),
        ] {
            if !seconds(start).is_some_and(|value| value.abs() <= tolerance) {
                return Err(unsupported(
                    "Encoding requires video and container timelines that start at zero. Timestamp offset handling is not available yet.",
                ));
            }
        }
        Ok(Self {
            custom_filters: settings.av1an_filters.clone(),
            grain_prefilter: settings
                .av1an_grain
                .as_ref()
                .filter(|grain| settings.encoder.is_svt() && grain.table.is_some() && grain.denoise)
                .map(|grain| grain.denoise_strength),
            prepare_metric_reference: settings
                .av1an_options
                .and_then(|options| options.target_quality)
                .is_some_and(|target| target.metric != media_core::Av1anTargetMetric::Vmaf),
            temporal: settings.temporal.map(temporal::Transform::new),
            trim: settings.trim,
            encoder: settings.encoder,
            source_pixel_format,
            source_is_vp9_alpha,
            output_pixel_format: explicit_pixel_format.unwrap_or_else(|| {
                if settings.encoder.is_svt() || source_depth >= 10 {
                    "yuv420p10le"
                } else {
                    "yuv420p"
                }
            }),
            video_index: video.index,
            width: geometry.width,
            height: geometry.height,
            geometry,
            aspect,
            source_sar,
            output_sar,
            fps_num,
            fps_den,
            cadence_reconciled: false,
            tolerance,
            primaries: if tone_map.is_some() {
                1
            } else if is_hdr10 {
                9
            } else {
                sdr_color(video.color_primaries.as_deref())?
            },
            transfer: if tone_map.is_some() {
                1
            } else if is_hdr10 {
                16
            } else {
                sdr_color(video.color_transfer.as_deref())?
            },
            matrix: if tone_map.is_some() {
                1
            } else if is_hdr10 {
                9
            } else if rgb_source {
                1 // The encoded YUV uses BT.709; RGB input carries GBR identity.
            } else {
                sdr_color(video.color_space.as_deref())?
            },
            hdr10,
            full_range: if tone_map.is_some() || (settings.encoder.is_svt() && rgb_source) {
                false
            } else {
                match video.color_range.as_deref() {
                    Some("tv") => false,
                    Some("pc") => true,
                    _ => {
                        return Err(unsupported(
                            "Explicit limited or full color range is required.",
                        ));
                    }
                }
            },
            chroma: if tone_map.is_some() {
                "left"
            } else {
                match video.chroma_location.as_deref() {
                    Some("left") => "left",
                    Some("topleft") => "topleft",
                    Some("center") if !settings.encoder.is_svt() => "center",
                    None | Some("unspecified" | "unknown")
                        if source_pixel_format.contains("444") || rgb_source =>
                    {
                        // 4:4:4 has no chroma sample offset. Conversion to a
                        // subsampled output needs a deterministic destination.
                        "left"
                    }
                    None | Some("unspecified" | "unknown") if settings.encoder.is_svt() => {
                        "unknown"
                    }
                    _ => {
                        return Err(unsupported(
                            "SVT supports left, top-left, or unspecified chroma placement; x264, x265, and VP9 require explicit left, center, or top-left placement.",
                        ));
                    }
                }
            },
            tone_map,
        })
    }

    pub fn frame_seconds(&self) -> f64 {
        f64::from(self.fps_den) / f64::from(self.fps_num)
    }

    pub fn output_sar(&self) -> &str {
        &self.output_sar
    }

    pub fn has_alpha(&self) -> bool {
        self.output_pixel_format == "yuva420p"
    }

    pub fn source_is_vp9_alpha(&self) -> bool {
        self.source_is_vp9_alpha
    }

    fn matches_source_stream_format(&self, format: Option<&str>) -> bool {
        format == Some(self.source_pixel_format)
            || (self.source_is_vp9_alpha && format == Some("yuv420p"))
    }

    pub fn requires_qtgmc(&self) -> bool {
        self.temporal
            .as_ref()
            .is_some_and(temporal::Transform::requires_qtgmc)
    }

    /// av1an derives its scene/chunk frame count before applying `--ffmpeg`.
    /// Timeline-changing filters therefore run once into a verified lossless
    /// source which all scene, chunk and quality-reference readers share.
    pub fn requires_av1an_preprocess(&self) -> bool {
        self.trim.is_some()
            || self.grain_prefilter.is_some()
            || !self.custom_filters.is_empty()
            || (self.prepare_metric_reference && self.decoder_filter().is_some())
            || self
                .temporal
                .as_ref()
                .is_some_and(temporal::Transform::changes_av1an_source_timeline)
    }

    pub fn qtgmc_settings(&self) -> Option<media_core::QtgmcSettings> {
        self.temporal
            .as_ref()
            .and_then(temporal::Transform::qtgmc_settings)
    }

    pub fn requires_exact_duplicate_scan(&self) -> bool {
        self.temporal
            .as_ref()
            .is_some_and(temporal::Transform::requires_exact_duplicate_scan)
    }

    pub fn set_exact_duplicate_count(
        &mut self,
        source_frames: usize,
        unique_frames: usize,
    ) -> Result<(), AppError> {
        let source_rate = media_core::FrameRate {
            numerator: self.fps_num,
            denominator: self.fps_den,
        };
        self.temporal
            .as_mut()
            .ok_or_else(|| {
                AppError::new(
                    "TEMPORAL_SETTINGS_INVALID",
                    "Duplicate cadence repair is not active.",
                    None,
                )
            })?
            .set_exact_duplicate_count(source_frames, unique_frames, source_rate)
    }

    pub fn post_qtgmc_filter_with_text(&self, text: Option<&str>) -> Option<String> {
        let filters = [
            self.post_qtgmc_prefix(),
            self.post_qtgmc_suffix_with_text(text),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        (!filters.is_empty()).then(|| filters.join(","))
    }

    /// Processing before bitmap subtitle graphics. Text and geometry follow.
    pub fn post_qtgmc_prefix(&self) -> Option<String> {
        let mut filters = Vec::new();
        if let Some(strength) = self.grain_prefilter {
            filters.push(format!(
                "hqdn3d={strength}:{strength}:{}:{}",
                u16::from(strength) * 3 / 2,
                u16::from(strength) * 3 / 2
            ));
        }
        if let Some(filter) = self
            .temporal
            .as_ref()
            .and_then(temporal::Transform::post_qtgmc_filter)
        {
            filters.push(filter);
        }
        if let Some(tone) = &self.tone_map {
            filters.push(tone.filter());
        }
        if let Some(conversion) = self.pixel_conversion() {
            filters.push(conversion);
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }

    pub fn post_qtgmc_suffix_with_text(&self, text: Option<&str>) -> Option<String> {
        let mut filters = Vec::new();
        if let Some(framing) = self.geometry.filter_with_text(
            self.matrix,
            self.full_range,
            self.chroma,
            self.output_pixel_format,
            text,
        ) {
            filters.push(framing);
        }
        let primaries = match self.primaries {
            1 => "bt709",
            5 => "bt470bg",
            6 => "smpte170m",
            9 => "bt2020",
            _ => unreachable!("validated output primaries"),
        };
        let transfer = match self.transfer {
            1 => "bt709",
            5 => "bt470bg",
            6 => "smpte170m",
            16 => "smpte2084",
            18 => "arib-std-b67",
            _ => unreachable!("validated output transfer"),
        };
        let matrix = match self.matrix {
            1 => "bt709",
            5 => "bt470bg",
            6 => "smpte170m",
            9 => "bt2020nc",
            _ => unreachable!("validated output matrix"),
        };
        filters.push(format!(
            "setparams=field_mode=prog:range={}:color_primaries={primaries}:color_trc={transfer}:colorspace={matrix}",
            if self.full_range { "pc" } else { "tv" }
        ));
        if let Some(aspect) = self.aspect {
            filters.push(aspect.filter());
        } else {
            // VSPipe's Y4M output does not carry source SAR. Re-establish the
            // plan's square-pixel default on the owned QTGMC intermediate.
            filters.push("setsar=1/1:max=65535".into());
        }
        filters.extend(self.custom_filters.iter().cloned());
        (!filters.is_empty()).then(|| filters.join(","))
    }

    pub fn decoder_filter(&self) -> Option<String> {
        self.decoder_filter_with_text(None)
    }

    pub fn decoder_filter_with_text(&self, text: Option<&str>) -> Option<String> {
        let mut filters = Vec::new();
        if let Some(prefix) = self.decoder_prefix() {
            filters.push(prefix);
        }
        if let Some(framing) = self.geometry.filter_with_text(
            self.matrix,
            self.full_range,
            self.chroma,
            self.output_pixel_format,
            text,
        ) {
            filters.push(framing);
        }
        if let Some(aspect) = self.aspect {
            filters.push(aspect.filter());
        }
        filters.extend(self.custom_filters.iter().cloned());
        (!filters.is_empty()).then(|| filters.join(","))
    }

    pub fn decoder_prefix(&self) -> Option<String> {
        let mut filters = Vec::new();
        if let Some(strength) = self.grain_prefilter {
            filters.push(format!(
                "hqdn3d={strength}:{strength}:{}:{}",
                u16::from(strength) * 3 / 2,
                u16::from(strength) * 3 / 2
            ));
        }
        if let Some(trim) = self.trim {
            if let Some(time) = trim.time {
                let seconds = |milliseconds: u32| {
                    format!("{}.{:03}", milliseconds / 1_000, milliseconds % 1_000)
                };
                filters.push(format!(
                    "trim=start={}:end={},setpts=PTS-STARTPTS",
                    seconds(time.start_milliseconds),
                    seconds(time.end_milliseconds)
                ));
            } else {
                filters.push(format!(
                    "trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS",
                    trim.start_frame, trim.end_frame_exclusive
                ));
            }
        }
        if let Some(temporal) = &self.temporal
            && let Some(filter) = temporal.filter()
        {
            filters.push(filter);
        }
        if let Some(tone) = &self.tone_map {
            filters.push(tone.filter());
        }
        if let Some(conversion) = self.pixel_conversion() {
            filters.push(conversion);
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }

    fn pixel_conversion(&self) -> Option<String> {
        let input = if self.tone_map.is_some() {
            "yuv420p10le"
        } else {
            self.source_pixel_format
        };
        if input == self.output_pixel_format {
            return None;
        }
        let matrix = match self.matrix {
            1 => "bt709",
            5 => "bt470bg",
            6 => "smpte170m",
            9 => "bt2020",
            _ => unreachable!("validated color matrix"),
        };
        let input_range = if rgb_source_format(self.source_pixel_format) || self.full_range {
            "full"
        } else {
            "limited"
        };
        let output_range = if self.full_range { "full" } else { "limited" };
        let (x, y) = match self.chroma {
            "left" => (0, 128),
            "topleft" => (0, 0),
            "center" => (128, 128),
            // An unspecified 4:2:0 input remains unspecified. The scale
            // operation uses its conventional left sample position.
            _ => (0, 128),
        };
        Some(format!(
            "scale=iw:ih:flags=bilinear+accurate_rnd:in_color_matrix={matrix}:out_color_matrix={matrix}:in_range={input_range}:out_range={output_range}:in_h_chr_pos={x}:out_h_chr_pos={x}:in_v_chr_pos={y}:out_v_chr_pos={y}:sws_dither=ed,format={}",
            self.output_pixel_format
        ))
    }

    pub fn geometry_filter_with_text(&self, text: Option<&str>) -> Option<String> {
        let mut filters = Vec::new();
        if let Some(filter) = self.geometry.filter_with_text(
            self.matrix,
            self.full_range,
            self.chroma,
            self.output_pixel_format,
            text,
        ) {
            filters.push(filter);
        }
        if let Some(aspect) = self.aspect {
            filters.push(aspect.filter());
        }
        filters.extend(self.custom_filters.iter().cloned());
        (!filters.is_empty()).then(|| filters.join(","))
    }

    pub async fn resolve_tone_map(
        &mut self,
        ffmpeg: &std::path::Path,
        source: &std::path::Path,
        duration_seconds: Option<f64>,
        cancel: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        let declared_peak = self
            .hdr10
            .as_ref()
            .and_then(|hdr| hdr.metadata.declared_peak_nits());
        if let Some(tone) = &mut self.tone_map {
            tone.resolve(
                ffmpeg,
                source,
                self.video_index,
                duration_seconds,
                declared_peak,
                cancel,
            )
            .await?;
        }
        Ok(())
    }

    pub fn tone_map_device_args(&self) -> Vec<std::ffi::OsString> {
        self.tone_map
            .as_ref()
            .map_or_else(Vec::new, tone_map::Transform::device_args)
    }

    pub fn tone_map_description(&self) -> Option<String> {
        self.tone_map.as_ref().map(tone_map::Transform::description)
    }

    pub async fn check_temporal_tools(
        &self,
        ffmpeg: &std::path::Path,
        cancel: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        if let Some(temporal) = &self.temporal {
            temporal.check_tools(ffmpeg, cancel).await?;
        }
        Ok(())
    }

    pub fn is_tone_mapped(&self) -> bool {
        self.tone_map.is_some()
    }

    fn validation_hdr(&self, encoded: bool) -> Option<&Hdr10> {
        if encoded && self.is_tone_mapped() {
            None
        } else {
            self.hdr10.as_ref()
        }
    }

    fn needs_mastering_metadata(&self) -> bool {
        // Preserving HDR10 output requires mastering metadata. Explicit SDR
        // rendering uses the supplied signal peak; any metadata present is
        // still parsed and checked across the complete original source.
        self.tone_map.is_none()
    }

    fn frame_dimensions(&self, encoded: bool) -> (u32, u32) {
        if encoded {
            (self.width, self.height)
        } else {
            (self.geometry.source_width, self.geometry.source_height)
        }
    }

    fn frame_sar(&self, encoded: bool) -> &str {
        if encoded {
            &self.output_sar
        } else {
            &self.source_sar
        }
    }

    pub fn output_codec(&self) -> &'static str {
        match self.encoder {
            VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => "av1",
            VideoEncoder::AomAv1 => "av1",
            VideoEncoder::X264 | VideoEncoder::H264Nvenc => "h264",
            VideoEncoder::X265 | VideoEncoder::X265Standalone | VideoEncoder::HevcNvenc => "hevc",
            VideoEncoder::Vp9 | VideoEncoder::VpxStandalone => "vp9",
        }
    }

    pub fn output_bit_depth(&self) -> u8 {
        if self.output_pixel_format.ends_with("10le") {
            10
        } else {
            8
        }
    }

    fn high_chroma_output(&self) -> bool {
        matches!(
            self.output_pixel_format,
            "yuv422p" | "yuv422p10le" | "yuv444p" | "yuv444p10le"
        )
    }

    pub fn matches_output_format(&self, format: Option<&str>) -> bool {
        format == Some(self.output_pixel_format)
            || (self.output_pixel_format == "yuva420p" && format == Some("yuv420p"))
            || (!self.encoder.is_svt()
                && self.full_range
                && matches!(
                    (self.output_pixel_format, format),
                    ("yuv420p", Some("yuvj420p"))
                        | ("yuv422p", Some("yuvj422p"))
                        | ("yuv444p", Some("yuvj444p"))
                ))
    }

    pub fn is_hdr10(&self) -> bool {
        self.validation_hdr(true).is_some()
    }

    pub fn hdr_arguments(&self) -> Vec<std::ffi::OsString> {
        let mut args = Vec::new();
        if let Some(hdr) = self.validation_hdr(true) {
            if let Some(mastering) = &hdr.metadata.mastering {
                args.extend(["--mastering-display".into(), mastering.argument().into()]);
            }
            if let Some((cll, fall)) = hdr.metadata.light {
                args.extend(["--content-light".into(), format!("{cll},{fall}").into()]);
            }
        }
        args
    }

    pub fn x265_hdr_arguments(&self) -> Vec<std::ffi::OsString> {
        let mut args = Vec::new();
        if let Some(hdr) = self.validation_hdr(true) {
            args.push("--hdr10".into());
            if let Some(mastering) = &hdr.metadata.mastering {
                args.extend(["--master-display".into(), mastering.x265_argument().into()]);
            }
            if let Some((cll, fall)) = hdr.metadata.light {
                args.extend(["--max-cll".into(), format!("{cll},{fall}").into()]);
            }
        }
        args
    }

    /// AOM's CLI writes HDR CICP into AV1 but cannot emit mastering or CLL
    /// metadata OBUs. Matroska carries those verified values through the
    /// subsequent FFmpeg copy mux, including when the destination is MP4.
    pub fn aom_hdr_mkvmerge_arguments(&self) -> Vec<std::ffi::OsString> {
        let mut args = vec![
            "--color-matrix-coefficients".into(),
            "0:9".into(),
            "--color-transfer-characteristics".into(),
            "0:16".into(),
            "--color-primaries".into(),
            "0:9".into(),
            "--color-bits-per-channel".into(),
            "0:10".into(),
            "--color-range".into(),
            "0:1".into(),
        ];
        if let Some(hdr) = self.validation_hdr(true) {
            if let Some(mastering) = &hdr.metadata.mastering {
                args.extend(mastering.mkvmerge_arguments());
            }
            if let Some((cll, fall)) = hdr.metadata.light {
                args.extend([
                    "--max-content-light".into(),
                    format!("0:{cll}").into(),
                    "--max-frame-light".into(),
                    format!("0:{fall}").into(),
                ]);
            }
        }
        args
    }

    #[cfg(test)]
    pub fn validate_source_frames(
        &mut self,
        frames: &Frames,
        stream: &Stream,
    ) -> Result<usize, AppError> {
        let mut validation = Validation::new(self.clone(), stream.clone(), false);
        for frame in &frames.frames {
            validation.push(frame);
        }
        let actual = validation.finish();
        let mut reference = self.clone();
        let expected = reference.validate_source_frames_buffered(frames, stream);
        assert_eq!(
            actual.as_ref().map(|(_, count)| *count),
            expected.as_ref().copied()
        );
        if let Ok((plan, _)) = &actual {
            assert_eq!(plan.hdr_arguments(), reference.hdr_arguments());
            assert_eq!(
                (plan.fps_num, plan.fps_den, plan.cadence_reconciled),
                (
                    reference.fps_num,
                    reference.fps_den,
                    reference.cadence_reconciled
                )
            );
            assert_eq!(plan.tolerance, reference.tolerance);
        }
        actual.map(|(plan, count)| {
            *self = plan;
            count
        })
    }

    // Retain the prior buffered implementation only as a test oracle. Every
    // existing metadata/cadence fixture exercises the production online validator.
    #[cfg(test)]
    fn validate_source_frames_buffered(
        &mut self,
        frames: &Frames,
        stream: &Stream,
    ) -> Result<usize, AppError> {
        let needs_mastering = self.needs_mastering_metadata();
        if let Some(hdr) = &mut self.hdr10 {
            let first = frames
                .frames
                .first()
                .ok_or_else(|| unsupported("The video did not decode to any frames."))?;
            hdr.metadata
                .absorb_first_frame(&StaticMetadata::parse(&first.side_data_list)?)?;
            if needs_mastering && hdr.metadata.mastering.is_none() {
                return Err(unsupported(
                    "HDR10 encoding requires valid mastering display metadata in the stream or first decoded frame.",
                ));
            }
        }
        let declared_error = match self.validate_frames_buffered(frames, stream, false) {
            Ok(count) => return Ok(count),
            Err(error) => error,
        };
        // Some sources declare NTSC 24000/1001 but author their timestamps at
        // decimal 23.976. This fixed alternative differs by one part per million.
        // Never estimate an arbitrary cadence or widen the per-frame tolerance:
        // every timestamp AND every frame's metadata must validate at one rate.
        if u64::from(self.fps_num) * 1001 != u64::from(self.fps_den) * 24000 {
            return Err(declared_error);
        }
        let mut candidate = self.clone();
        candidate.fps_num = 2997;
        candidate.fps_den = 125;
        candidate.cadence_reconciled = true;
        match candidate.validate_frames_buffered(frames, stream, false) {
            Ok(count) => {
                *self = candidate;
                Ok(count)
            }
            Err(_) => Err(declared_error),
        }
    }

    #[cfg(test)]
    pub fn validate_frames(
        &self,
        frames: &Frames,
        stream: &Stream,
        encoded: bool,
    ) -> Result<usize, AppError> {
        let mut validation = Validation::exact(self.clone(), stream.clone(), encoded);
        for frame in &frames.frames {
            validation.push(frame);
        }
        let actual = validation.finish().map(|(_, count)| count);
        assert_eq!(
            actual,
            self.validate_frames_buffered(frames, stream, encoded)
        );
        actual
    }

    #[cfg(test)]
    fn validate_frames_buffered(
        &self,
        frames: &Frames,
        stream: &Stream,
        encoded: bool,
    ) -> Result<usize, AppError> {
        // Matroska commonly quantizes timestamps to milliseconds even when the
        // source time base is much finer. Use each scanned file's own time base.
        let tolerance = if encoded {
            rational(stream.time_base.as_deref())
                .map(|(n, d)| f64::from(n) / f64::from(d))
                .unwrap_or(0.000001)
                .clamp(0.000001, 0.001)
                + 0.000002
        } else {
            self.tolerance
        };
        if frames.frames.is_empty() {
            return Err(unsupported("The video did not decode to any frames."));
        }
        if !encoded && !self.matches_source_stream_format(stream.pix_fmt.as_deref()) {
            return Err(unsupported(
                "The decoded source pixel format differs from the selected stream.",
            ));
        }
        let mut observed_hdr = StaticMetadata::parse(&stream.side_data_list)?;
        validate_side_data(
            &stream.side_data_list,
            self.validation_hdr(encoded),
            encoded,
        )?;
        for (index, frame) in frames.frames.iter().enumerate() {
            let time = seconds(frame.best_effort_timestamp_time.as_deref())
                .ok_or_else(|| unsupported("A decoded frame has no usable timestamp."))?;
            if (time - index as f64 * self.frame_seconds()).abs() > tolerance {
                return Err(unsupported(
                    "Decoded frame timestamps are not constant-rate starting at zero. VFR and timestamp gaps require a later workflow.",
                ));
            }
            let (width, height) = self.frame_dimensions(encoded);
            self.validate_fields(frame, encoded)?;
            if frame.width != Some(width)
                || frame.height != Some(height)
                || frame.sample_aspect_ratio.as_deref() != Some(self.frame_sar(encoded))
            {
                return Err(unsupported(
                    "Decoded frame dimensions or sample aspect ratio differ from the processing plan.",
                ));
            }
            let normalize_chroma = |value: Option<&str>| match value {
                None | Some("unspecified" | "unknown") => "unknown",
                Some("left") => "left",
                Some("topleft") => "topleft",
                Some("center") => "center",
                _ => "unsupported",
            };
            if normalize_chroma(frame.chroma_location.as_deref())
                != normalize_chroma(stream.chroma_location.as_deref())
                && !(encoded && self.high_chroma_output() && frame.chroma_location.is_none())
            {
                return Err(unsupported(
                    "Decoded frame chroma placement differs from the selected source.",
                ));
            }
            if if encoded {
                !self.matches_output_format(frame.pix_fmt.as_deref())
            } else {
                frame.pix_fmt.as_deref() != Some(self.source_pixel_format)
            } {
                return Err(unsupported(
                    "The decoded bit depth, pixel format, or frame side data changed unexpectedly.",
                ));
            }
            validate_side_data(&frame.side_data_list, self.validation_hdr(encoded), encoded)?;
            if !encoded && let Some(hdr) = self.hdr10.as_ref() {
                hdr.require_profile5_rpu(&frame.side_data_list)?;
            }
            if let Some(hdr) = self.validation_hdr(encoded) {
                let actual = StaticMetadata::parse(&frame.side_data_list)?;
                hdr.metadata.validate_present(&actual, encoded)?;
                // Metadata need not be repeated on every frame, but output must
                // contain all planned static metadata somewhere in the stream.
                if actual.mastering.is_some() {
                    observed_hdr.mastering = actual.mastering;
                }
                if actual.light.is_some() {
                    observed_hdr.light = actual.light;
                }
            }
            for (actual, expected) in [
                (&frame.color_space, &stream.color_space),
                (&frame.color_transfer, &stream.color_transfer),
                (&frame.color_primaries, &stream.color_primaries),
                (&frame.color_range, &stream.color_range),
            ] {
                if actual != expected {
                    return Err(unsupported(
                        "Decoded frame color metadata differs from the selected source.",
                    ));
                }
            }
        }
        if let Some(hdr) = self.validation_hdr(encoded) {
            hdr.metadata.validate_present(&observed_hdr, encoded)?;
            if encoded
                && (observed_hdr.mastering.is_none()
                    || (hdr.metadata.light.is_some() && observed_hdr.light.is_none()))
            {
                return Err(unsupported(
                    "The encoded video is missing planned HDR10 mastering or content light metadata.",
                ));
            }
        }
        Ok(frames.frames.len())
    }

    pub fn validate_encoded_stream(
        &self,
        source: &Stream,
        output: &Stream,
    ) -> Result<(), AppError> {
        self.validate_encoded_stream_fields(source, output, false)
    }

    /// An elementary VP9/IVF checkpoint cannot represent display aspect,
    /// chroma location, color primaries, or transfer characteristics. The
    /// final Matroska stage writes and strictly validates those plan fields.
    /// At this phase, absent container-level values are accepted while an
    /// explicitly wrong value still rejects the checkpoint.
    pub fn validate_elementary_checkpoint(
        &self,
        source: &Stream,
        output: &Stream,
    ) -> Result<(), AppError> {
        self.validate_encoded_stream_fields(source, output, self.output_codec() == "vp9")
    }

    fn validate_encoded_stream_fields(
        &self,
        source: &Stream,
        output: &Stream,
        allow_absent_vp9_container_fields: bool,
    ) -> Result<(), AppError> {
        let expected_primaries = if self.is_tone_mapped() {
            Some("bt709")
        } else {
            source.color_primaries.as_deref()
        };
        let expected_transfer = if self.is_tone_mapped() {
            Some("bt709")
        } else {
            source.color_transfer.as_deref()
        };
        let optional_container_field_matches = |actual: Option<&str>, expected: Option<&str>| {
            actual == expected || (allow_absent_vp9_container_fields && actual.is_none())
        };
        let chroma_matches = if self.high_chroma_output() && output.chroma_location.is_none() {
            // HEVC and some other high-chroma bitstreams do not expose a
            // chroma-location field through FFprobe. The producer's explicit
            // conversion and decoded pixel format remain independently checked.
            true
        } else {
            match self.chroma {
                "left" => optional_container_field_matches(
                    output.chroma_location.as_deref(),
                    Some("left"),
                ),
                "topleft" => optional_container_field_matches(
                    output.chroma_location.as_deref(),
                    Some("topleft"),
                ),
                "center" => optional_container_field_matches(
                    output.chroma_location.as_deref(),
                    Some("center"),
                ),
                _ => matches!(
                    output.chroma_location.as_deref(),
                    None | Some("unspecified" | "unknown")
                ),
            }
        };
        if (self.output_pixel_format == "yuva420p"
            && !allow_absent_vp9_container_fields
            && !output
                .tags
                .iter()
                .any(|(key, value)| key.eq_ignore_ascii_case("alpha_mode") && value == "1"))
            || output.codec_name.as_deref() != Some(self.output_codec())
            || !self.matches_output_format(output.pix_fmt.as_deref())
            || output.width != Some(self.width)
            || output.height != Some(self.height)
            || !optional_container_field_matches(
                output.sample_aspect_ratio.as_deref(),
                Some(self.output_sar.as_str()),
            )
            || if self.is_tone_mapped() {
                output.color_space.as_deref() != Some("bt709")
                    || !optional_container_field_matches(
                        output.color_transfer.as_deref(),
                        expected_transfer,
                    )
                    || !optional_container_field_matches(
                        output.color_primaries.as_deref(),
                        expected_primaries,
                    )
                    || output.color_range.as_deref() != Some("tv")
            } else {
                (if rgb_source_format(self.source_pixel_format) {
                    output.color_space.as_deref() != Some("bt709")
                } else {
                    source.color_space != output.color_space
                }) || !optional_container_field_matches(
                    output.color_transfer.as_deref(),
                    expected_transfer,
                ) || !optional_container_field_matches(
                    output.color_primaries.as_deref(),
                    expected_primaries,
                ) || output.color_range.as_deref()
                    != Some(if self.full_range { "pc" } else { "tv" })
            }
            || !chroma_matches
        {
            return Err(AppError::new(
                "ENCODE_VALIDATION_FAILED",
                "The encoded output codec, dimensions, bit depth, aspect ratio, or color metadata differ from the plan.",
                None,
            ));
        }
        validate_side_data(&output.side_data_list, self.validation_hdr(true), true)?;
        if let Some(hdr) = self.validation_hdr(true) {
            hdr.metadata
                .validate_present(&StaticMetadata::parse(&output.side_data_list)?, true)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
pub(super) struct Frames {
    pub frames: Vec<Frame>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Frame {
    best_effort_timestamp_time: Option<String>,
    interlaced_frame: Option<u8>,
    top_field_first: Option<u8>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    sample_aspect_ratio: Option<String>,
    chroma_location: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_range: Option<String>,
    #[serde(default)]
    side_data_list: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vp9_elementary_checkpoint_allows_only_absent_container_color_fields() {
        let mut source = source();
        source.streams[0].chroma_location = Some("left".into());
        let settings = EncodeSettings {
            encoder: VideoEncoder::VpxStandalone,
            ..Default::default()
        };
        let plan = Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).unwrap();
        let mut checkpoint = source.streams[0].clone();
        checkpoint.codec_name = Some("vp9".into());
        checkpoint.sample_aspect_ratio = None;
        checkpoint.chroma_location = None;
        checkpoint.color_primaries = None;
        checkpoint.color_transfer = None;

        plan.validate_elementary_checkpoint(&source.streams[0], &checkpoint)
            .unwrap();
        assert!(
            plan.validate_encoded_stream(&source.streams[0], &checkpoint)
                .is_err()
        );

        checkpoint.color_primaries = Some("bt470bg".into());
        assert!(
            plan.validate_elementary_checkpoint(&source.streams[0], &checkpoint)
                .is_err()
        );
    }

    #[test]
    fn framing_validates_original_source_frames_and_transformed_output_frames_separately() {
        for hdr in [false, true] {
            let (source, decoded) = if hdr {
                hdr_source_and_frames()
            } else {
                (source(), frames(&["0", "0.042", "0.083"]))
            };
            let settings = EncodeSettings {
                framing: media_core::VideoFraming {
                    crop: media_core::CropSettings {
                        left: 16,
                        right: 16,
                        ..Default::default()
                    },
                    resize_width: Some(64),
                    borders: media_core::BorderSettings {
                        top: 8,
                        right: 24,
                        bottom: 16,
                        left: 32,
                    },
                },
                ..Default::default()
            };
            let selected = source.selected(&[0]).unwrap();
            let mut plan = Plan::build(&source, &selected, &settings).unwrap();
            assert_eq!((plan.width, plan.height), (120, 88));
            assert_eq!(plan.frame_dimensions(false), (128, 96));
            assert_eq!(
                plan.validate_source_frames(&decoded, &source.streams[0])
                    .unwrap(),
                3
            );
            let mut unchanged_plan =
                Plan::build(&source, &selected, &EncodeSettings::default()).unwrap();
            unchanged_plan
                .validate_source_frames(&decoded, &source.streams[0])
                .unwrap();
            assert_eq!(plan.hdr_arguments(), unchanged_plan.hdr_arguments());
            let mut encoded = source.streams[0].clone();
            encoded.codec_name = Some("av1".into());
            encoded.pix_fmt = Some("yuv420p10le".into());
            assert!(
                plan.validate_encoded_stream(&source.streams[0], &encoded)
                    .is_err()
            );
            encoded.width = Some(64);
            encoded.height = Some(64);
            assert!(
                plan.validate_encoded_stream(&source.streams[0], &encoded)
                    .is_err()
            );
            encoded.width = Some(120);
            encoded.height = Some(88);
            plan.validate_encoded_stream(&source.streams[0], &encoded)
                .unwrap();
            let mut output = Frames {
                frames: decoded.frames.clone(),
            };
            for frame in &mut output.frames {
                frame.width = Some(120);
                frame.height = Some(88);
                frame.pix_fmt = Some("yuv420p10le".into());
            }
            assert_eq!(plan.validate_frames(&output, &encoded, true).unwrap(), 3);
            assert!(
                plan.validate_source_frames(&output, &source.streams[0])
                    .is_err()
            );
            assert!(plan.validate_frames(&decoded, &encoded, true).is_err());
            for defect in ["geometry", "depth", "sar", "chroma", "color", "time"] {
                let mut wrong = Frames {
                    frames: output.frames.clone(),
                };
                let last = wrong.frames.last_mut().unwrap();
                match defect {
                    "geometry" => last.width = Some(128),
                    "depth" => last.pix_fmt = Some("yuv420p".into()),
                    "sar" => last.sample_aspect_ratio = Some("4:3".into()),
                    "chroma" => last.chroma_location = Some("center".into()),
                    "color" => last.color_primaries = Some("bt470bg".into()),
                    "time" => last.best_effort_timestamp_time = Some("0.100".into()),
                    _ => unreachable!(),
                }
                assert!(
                    plan.validate_frames(&wrong, &encoded, true).is_err(),
                    "{defect}, HDR {hdr}"
                );
            }
        }
    }

    fn mastering() -> serde_json::Value {
        serde_json::json!({"side_data_type":"Mastering display metadata", "red_x":"34000/50000", "red_y":"16000/50000", "green_x":"13250/50000", "green_y":"34500/50000", "blue_x":"7500/50000", "blue_y":"3000/50000", "white_point_x":"15635/50000", "white_point_y":"16450/50000", "max_luminance":"10000000/10000", "min_luminance":"1/10000"})
    }

    fn hdr_source_and_frames() -> (Document, Frames) {
        let mut source = source();
        let stream = &mut source.streams[0];
        stream.codec_name = Some("hevc".into());
        stream.pix_fmt = Some("yuv420p10le".into());
        stream.color_primaries = Some("bt2020".into());
        stream.color_transfer = Some("smpte2084".into());
        stream.color_space = Some("bt2020nc".into());
        let mut decoded = frames(&["0", "0.042", "0.083"]);
        for frame in &mut decoded.frames {
            frame.pix_fmt = stream.pix_fmt.clone();
            frame.color_primaries = stream.color_primaries.clone();
            frame.color_transfer = stream.color_transfer.clone();
            frame.color_space = stream.color_space.clone();
        }
        decoded.frames[0].side_data_list = vec![
            mastering(),
            serde_json::json!({"side_data_type":"Content light level metadata", "max_content":200, "max_average":142}),
        ];
        (source, decoded)
    }

    #[test]
    fn hdr10_uses_first_frame_metadata_and_verifies_av1_precision() {
        let (source, mut decoded) = hdr_source_and_frames();
        let mut plan = Plan::build(
            &source,
            &source.selected(&[0]).unwrap(),
            &EncodeSettings::default(),
        )
        .unwrap();
        assert_eq!(
            plan.validate_source_frames(&decoded, &source.streams[0])
                .unwrap(),
            3
        );
        assert_eq!((plan.primaries, plan.transfer, plan.matrix), (9, 16, 9));
        let args = plan.hdr_arguments();
        assert_eq!(args[2], "--content-light");
        assert_eq!(args[3], "200,142");
        assert!(
            args[1]
                .to_string_lossy()
                .starts_with("G(0.2650000000,0.6900000000)")
        );
        let mut encoded = source.streams[0].clone();
        encoded.codec_name = Some("av1".into());
        assert!(
            plan.validate_encoded_stream(&source.streams[0], &encoded)
                .is_ok()
        );
        // AV1 stores coordinates /65536, peak luminance /256, minimum /16384.
        decoded.frames[0].side_data_list[0]["red_x"] = "44564/65536".into();
        decoded.frames[0].side_data_list[0]["min_luminance"] = "2/16384".into();
        assert!(plan.validate_frames(&decoded, &encoded, true).is_ok());
        assert!(
            plan.validate_frames(&decoded, &source.streams[0], false)
                .is_err()
        );
        decoded.frames[0].side_data_list[0]["red_x"] = "44567/65536".into();
        assert!(plan.validate_frames(&decoded, &encoded, true).is_err());
    }

    #[test]
    fn hdr10_rejects_missing_changed_malformed_or_new_static_metadata() {
        for defect in [
            "missing",
            "zero-denominator",
            "missing-coordinate",
            "changed",
            "stream-mismatch",
            "late-light",
            "missing-output",
            "wrong-light",
            "output-dynamic",
        ] {
            let (mut source, mut decoded) = hdr_source_and_frames();
            match defect {
                "missing" => decoded.frames[0].side_data_list.clear(),
                "zero-denominator" => decoded.frames[0].side_data_list[0]["red_x"] = "1/0".into(),
                "missing-coordinate" => {
                    decoded.frames[0].side_data_list[0]
                        .as_object_mut()
                        .unwrap()
                        .remove("red_y");
                }
                "changed" => {
                    let mut changed = mastering();
                    changed["max_luminance"] = "2000/1".into();
                    decoded.frames[1].side_data_list.push(changed);
                }
                "stream-mismatch" => {
                    let mut changed = mastering();
                    changed["max_luminance"] = "2000/1".into();
                    source.streams[0].side_data_list.push(changed);
                }
                "late-light" => {
                    let light = decoded.frames[0].side_data_list.pop().unwrap();
                    decoded.frames[1].side_data_list.push(light);
                }
                _ => {}
            }
            let mut plan = Plan::build(
                &source,
                &source.selected(&[0]).unwrap(),
                &EncodeSettings::default(),
            )
            .unwrap();
            if matches!(defect, "missing-output" | "wrong-light" | "output-dynamic") {
                plan.validate_source_frames(&decoded, &source.streams[0])
                    .unwrap();
                match defect {
                    "missing-output" => decoded.frames[0].side_data_list.clear(),
                    "wrong-light" => decoded.frames[0].side_data_list[1]["max_content"] = 201.into(),
                    _ => decoded.frames[0].side_data_list.push(serde_json::json!({"side_data_type":"HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"}))
                }
                assert!(
                    plan.validate_frames(&decoded, &source.streams[0], true)
                        .is_err(),
                    "{defect}"
                );
            } else {
                assert!(
                    plan.validate_source_frames(&decoded, &source.streams[0])
                        .is_err(),
                    "{defect}"
                );
            }
        }
    }

    #[test]
    fn dynamic_hdr_requires_opt_in_and_confirmed_compatible_dolby_base_layer() {
        for (profile, compatibility, allowed) in [
            (7, 6, true),
            (8, 1, true),
            (5, 0, false),
            (5, 1, false),
            (8, 4, false),
            (8, 2, false),
            (7, 0, false),
            (9, 1, false),
        ] {
            for fallback in [false, true] {
                let (mut source, mut decoded) = hdr_source_and_frames();
                source.streams[0].side_data_list.push(serde_json::json!({"side_data_type":"DOVI configuration record", "dv_profile":profile, "dv_bl_signal_compatibility_id":compatibility,"bl_present_flag":1,"rpu_present_flag":1,"el_present_flag":u8::from(profile==7)}));
                decoded.frames[0]
                    .side_data_list
                    .push(serde_json::json!({"side_data_type":"Dolby Vision RPU Data"}));
                decoded.frames[0].side_data_list.push(serde_json::json!({"side_data_type":"Dolby Vision Metadata", "bl_bit_depth":10, "bl_video_full_range_flag":0}));
                decoded.frames[0].side_data_list.push(serde_json::json!({"side_data_type":"HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"}));
                let settings = EncodeSettings {
                    hdr10_fallback: fallback,
                    ..EncodeSettings::default()
                };
                let plan = Plan::build(&source, &source.selected(&[0]).unwrap(), &settings);
                if allowed && fallback {
                    assert!(
                        plan.unwrap()
                            .validate_source_frames(&decoded, &source.streams[0])
                            .is_ok()
                    );
                } else {
                    assert!(
                        plan.is_err(),
                        "profile{profile} compat{compatibility} fallback{fallback}"
                    );
                }
            }
        }
        let (source, mut decoded) = hdr_source_and_frames();
        decoded.frames[1]
            .side_data_list
            .push(serde_json::json!({"side_data_type":"Dolby Vision RPU Data"}));
        let mut plan = Plan::build(
            &source,
            &source.selected(&[0]).unwrap(),
            &EncodeSettings {
                hdr10_fallback: true,
                ..EncodeSettings::default()
            },
        )
        .unwrap();
        assert!(
            plan.validate_source_frames(&decoded, &source.streams[0])
                .is_err(),
            "No stream configuration to confirm Dolby compatibility"
        );
    }

    #[test]
    fn profile5_requires_gpu_route_and_rpu_on_every_decoded_frame() {
        let (mut source, mut decoded) = hdr_source_and_frames();
        let stream = &mut source.streams[0];
        stream.color_range = Some("pc".into());
        stream.color_primaries = None;
        stream.color_transfer = None;
        stream.color_space = None;
        stream.chroma_location = Some("left".into());
        stream.side_data_list.push(serde_json::json!({
            "side_data_type":"DOVI configuration record",
            "dv_profile":5,
            "dv_bl_signal_compatibility_id":0,
            "bl_present_flag":1,
            "rpu_present_flag":1,
            "el_present_flag":0
        }));
        for frame in &mut decoded.frames {
            frame.color_range = Some("pc".into());
            frame.color_primaries = None;
            frame.color_transfer = None;
            frame.color_space = None;
            frame.chroma_location = Some("left".into());
            frame.side_data_list.push(serde_json::json!({
                "side_data_type":"Dolby Vision RPU Data"
            }));
            frame.side_data_list.push(serde_json::json!({
                "side_data_type":"Dolby Vision Metadata",
                "bl_bit_depth":10,
                "bl_video_full_range_flag":1
            }));
        }
        let mut settings = EncodeSettings {
            tone_map: Some(
                serde_json::from_value(serde_json::json!({
                    "sourcePeakNits":1000,
                    "backend":"auto",
                    "peakMode":"measured"
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let mut plan = Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).unwrap();
        assert!(plan.tone_map.as_ref().unwrap().dv5);
        assert!(
            !plan.full_range,
            "SDR output must be limited even from a full-range IPT source"
        );
        plan.validate_source_frames(&decoded, &source.streams[0])
            .unwrap();
        decoded.frames[1].side_data_list.retain(|value| {
            value
                .get("side_data_type")
                .and_then(serde_json::Value::as_str)
                != Some("Dolby Vision RPU Data")
        });
        assert!(
            plan.validate_source_frames(&decoded, &source.streams[0])
                .is_err()
        );
        decoded.frames[1].side_data_list.push(serde_json::json!({
            "side_data_type":"Dolby Vision RPU Data"
        }));
        decoded.frames[1].side_data_list.retain(|value| {
            value
                .get("side_data_type")
                .and_then(serde_json::Value::as_str)
                != Some("Dolby Vision Metadata")
        });
        assert!(
            plan.validate_source_frames(&decoded, &source.streams[0])
                .is_err(),
            "Raw RPU without parsed metadata must not qualify profile 5 rendering"
        );
        settings.tone_map.as_mut().unwrap().backend = media_core::ToneMapBackend::Cpu;
        assert!(Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).is_err());
        settings.tone_map.as_mut().unwrap().backend = media_core::ToneMapBackend::Auto;
        settings.tone_map.as_mut().unwrap().hdr10_base_layer = true;
        assert!(Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).is_err());
    }

    #[test]
    fn grain_and_worker_settings_are_bounded() {
        for grain in [0, 1, 50] {
            assert!(
                validate_settings(&EncodeSettings {
                    film_grain: grain,
                    ..EncodeSettings::default()
                })
                .is_ok()
            );
        }
        for grain in [51, 255] {
            assert!(
                validate_settings(&EncodeSettings {
                    film_grain: grain,
                    ..EncodeSettings::default()
                })
                .is_err()
            );
        }
        for workers in [0, 33, 255] {
            assert!(
                validate_settings(&EncodeSettings {
                    workers,
                    ..EncodeSettings::default()
                })
                .is_err()
            );
        }
    }

    #[test]
    fn hdr10plus_alone_needs_fallback_and_unknown_rendering_data_is_rejected() {
        let (source, mut decoded) = hdr_source_and_frames();
        decoded.frames[1].side_data_list.push(serde_json::json!({
            "side_data_type": "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"
        }));
        for fallback in [false, true] {
            let mut plan = Plan::build(
                &source,
                &source.selected(&[0]).unwrap(),
                &EncodeSettings {
                    hdr10_fallback: fallback,
                    ..EncodeSettings::default()
                },
            )
            .unwrap();
            assert_eq!(
                plan.validate_source_frames(&decoded, &source.streams[0])
                    .is_ok(),
                fallback
            );
        }
        for kind in [
            "Display Matrix",
            "ICC profile",
            "HDR Vivid",
            "future rendering metadata",
        ] {
            decoded.frames[1].side_data_list = vec![serde_json::json!({"side_data_type": kind})];
            let mut plan = Plan::build(
                &source,
                &source.selected(&[0]).unwrap(),
                &EncodeSettings {
                    hdr10_fallback: true,
                    ..EncodeSettings::default()
                },
            )
            .unwrap();
            assert!(
                plan.validate_source_frames(&decoded, &source.streams[0])
                    .is_err(),
                "{kind}"
            );
        }
        let mut full_range = source.clone();
        full_range.streams[0].color_range = Some("pc".into());
        assert!(
            Plan::build(
                &full_range,
                &full_range.selected(&[0]).unwrap(),
                &EncodeSettings::default()
            )
            .is_err()
        );
    }
    fn source() -> Document {
        serde_json::from_str(r#"{"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":128,"height":96,"pix_fmt":"yuv420p","field_order":"progressive","sample_aspect_ratio":"1:1","avg_frame_rate":"24000/1001","time_base":"1/1000","start_time":"0","color_space":"bt709","color_primaries":"bt709","color_transfer":"bt709","color_range":"tv"}],"format":{"start_time":"0","duration":"1"}}"#).unwrap()
    }

    #[test]
    fn fork_options_are_validated_without_leaking_to_other_encoders() {
        for encoder in [
            VideoEncoder::SvtAv1,
            VideoEncoder::SvtAv1FiveFish,
            VideoEncoder::SvtAv1Hdr,
        ] {
            let settings = EncodeSettings {
                encoder,
                ..Default::default()
            };
            let document = source();
            let plan =
                Plan::build(&document, &document.selected(&[0]).unwrap(), &settings).unwrap();
            assert_eq!(plan.output_codec(), "av1");
            assert_eq!(plan.output_pixel_format, "yuv420p10le");
            for bias in [0, 5, 7, 8] {
                let result = validate_settings(&EncodeSettings {
                    lineart_psy_bias: bias,
                    texture_psy_bias: bias,
                    ..settings.clone()
                });
                assert_eq!(
                    result.is_ok(),
                    bias == 0 || (encoder == VideoEncoder::SvtAv1FiveFish && bias <= 7)
                );
            }
            assert_eq!(
                validate_settings(&EncodeSettings {
                    hdr_tune: media_core::HdrTune::FilmGrain,
                    ..settings
                })
                .is_ok(),
                encoder == VideoEncoder::SvtAv1Hdr
            );
        }
    }

    #[test]
    fn fork_parameters_and_hdr_fallback_remain_independent() {
        for encoder in [
            VideoEncoder::SvtAv1,
            VideoEncoder::SvtAv1FiveFish,
            VideoEncoder::SvtAv1Hdr,
        ] {
            let (mut document, _) = hdr_source_and_frames();
            document.streams[0].side_data_list.push(
                serde_json::json!({"side_data_type":"HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"}),
            );
            for fallback in [false, true] {
                let settings = EncodeSettings {
                    encoder,
                    hdr10_fallback: fallback,
                    lineart_psy_bias: if encoder == VideoEncoder::SvtAv1FiveFish {
                        5
                    } else {
                        0
                    },
                    texture_psy_bias: if encoder == VideoEncoder::SvtAv1FiveFish {
                        4
                    } else {
                        0
                    },
                    hdr_tune: if encoder == VideoEncoder::SvtAv1Hdr {
                        media_core::HdrTune::FilmGrain
                    } else {
                        Default::default()
                    },
                    ..Default::default()
                };
                let result = Plan::build(&document, &document.selected(&[0]).unwrap(), &settings);
                assert_eq!(result.is_ok(), fallback);
                if let Ok(plan) = result {
                    let args = super::super::encode::encoder_parameters(&plan, &settings);
                    assert_eq!(
                        args.iter().any(|arg| arg == "--lineart-psy-bias"),
                        encoder == VideoEncoder::SvtAv1FiveFish
                    );
                    assert_eq!(
                        args.iter().any(|arg| arg == "--texture-psy-bias"),
                        encoder == VideoEncoder::SvtAv1FiveFish
                    );
                    assert_eq!(
                        args.iter().any(|arg| arg == "--tune"),
                        encoder == VideoEncoder::SvtAv1Hdr
                    );
                    assert!(
                        args.windows(2)
                            .any(|pair| pair == ["--film-grain-denoise", "0"])
                    );
                    if encoder == VideoEncoder::SvtAv1Hdr {
                        assert!(args.windows(2).any(|pair| pair == ["--tune", "5"]));
                        let mut overridden = settings.clone();
                        overridden.parameters.push(media_core::EncoderParameter {
                            name: "tune".into(),
                            value: "0".into(),
                        });
                        let overridden_args =
                            super::super::encode::encoder_parameters(&plan, &overridden);
                        assert_eq!(
                            overridden_args
                                .iter()
                                .filter(|value| *value == "--tune")
                                .count(),
                            1
                        );
                        assert!(
                            overridden_args
                                .windows(2)
                                .any(|pair| pair == ["--tune", "0"])
                        );
                    }
                }
            }
        }
    }
    fn frames(times: &[&str]) -> Frames {
        serde_json::from_value(serde_json::json!({"frames": times.iter().map(|time| serde_json::json!({"best_effort_timestamp_time":time,"interlaced_frame":0,"width":128,"height":96,"pix_fmt":"yuv420p","sample_aspect_ratio":"1:1","color_space":"bt709","color_primaries":"bt709","color_transfer":"bt709","color_range":"tv"})).collect::<Vec<_>>()})).unwrap()
    }

    fn x264_settings() -> EncodeSettings {
        EncodeSettings {
            encoder: VideoEncoder::X264,
            crf: 23,
            preset: 5,
            ..Default::default()
        }
    }

    #[test]
    fn av1an_x264_explicit_output_formats_preserve_depth_and_chroma() {
        use media_core::{Av1anOptions, Av1anPixelFormat};
        let mut source = source();
        source.streams[0].chroma_location = Some("left".into());
        for (format, expected, depth) in [
            (Av1anPixelFormat::Yuv420p, "yuv420p", 8),
            (Av1anPixelFormat::Yuv420p10le, "yuv420p10le", 10),
            (Av1anPixelFormat::Yuv422p, "yuv422p", 8),
            (Av1anPixelFormat::Yuv422p10le, "yuv422p10le", 10),
            (Av1anPixelFormat::Yuv444p, "yuv444p", 8),
            (Av1anPixelFormat::Yuv444p10le, "yuv444p10le", 10),
        ] {
            let settings = EncodeSettings {
                backend: EncodeBackend::Av1an,
                av1an_options: Some(Av1anOptions {
                    pixel_format: Some(format),
                    ..Default::default()
                }),
                ..x264_settings()
            };
            let plan = Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).unwrap();
            assert_eq!(plan.output_pixel_format, expected);
            assert_eq!(plan.output_bit_depth(), depth);
            assert!(plan.matches_output_format(Some(expected)));
        }
    }

    #[test]
    fn av1an_svt_eight_bit_output_sets_matching_encoder_depth() {
        let mut source = source();
        source.streams[0].chroma_location = Some("left".into());
        let settings = EncodeSettings {
            backend: EncodeBackend::Av1an,
            av1an_options: Some(media_core::Av1anOptions {
                pixel_format: Some(media_core::Av1anPixelFormat::Yuv420p),
                ..Default::default()
            }),
            ..Default::default()
        };
        let plan = Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).unwrap();
        assert_eq!(plan.output_pixel_format, "yuv420p");
        assert_eq!(plan.output_bit_depth(), 8);
        let args = super::super::encode::encoder_parameters(&plan, &settings);
        assert!(args.windows(2).any(|pair| pair == ["--input-depth", "8"]));
    }

    #[test]
    fn ffmpeg_encoders_enforce_independent_quality_speed_and_backend_limits() {
        for (encoder, max_crf, max_preset) in
            [(VideoEncoder::X265, 51, 9), (VideoEncoder::Vp9, 63, 5)]
        {
            let settings = EncodeSettings {
                encoder,
                crf: max_crf,
                preset: max_preset,
                ..Default::default()
            };
            assert!(validate_settings(&settings).is_ok());
            assert!(
                validate_settings(&EncodeSettings {
                    crf: 0,
                    preset: 0,
                    ..settings.clone()
                })
                .is_ok()
            );
            for invalid in [
                EncodeSettings {
                    crf: max_crf + 1,
                    ..settings.clone()
                },
                EncodeSettings {
                    preset: max_preset + 1,
                    ..settings.clone()
                },
                EncodeSettings {
                    backend: EncodeBackend::Av1an,
                    ..settings.clone()
                },
                EncodeSettings {
                    film_grain: 1,
                    ..settings.clone()
                },
                EncodeSettings {
                    hdr10_fallback: true,
                    ..settings.clone()
                },
                EncodeSettings {
                    lineart_psy_bias: 1,
                    ..settings.clone()
                },
            ] {
                assert!(validate_settings(&invalid).is_err(), "{invalid:?}");
            }
        }
    }

    #[test]
    fn x264_accepts_both_backends_and_rejects_hdr_or_unknown_chroma() {
        assert!(
            validate_settings(&EncodeSettings {
                backend: EncodeBackend::Av1an,
                ..x264_settings()
            })
            .is_ok()
        );
        for crf in [0, 23, 51] {
            for preset in [0, 5, 9] {
                assert!(
                    validate_settings(&EncodeSettings {
                        crf,
                        preset,
                        ..x264_settings()
                    })
                    .is_ok()
                );
            }
        }
        for invalid in [
            EncodeSettings {
                crf: 52,
                ..x264_settings()
            },
            EncodeSettings {
                preset: 10,
                ..x264_settings()
            },
            EncodeSettings {
                film_grain: 1,
                ..x264_settings()
            },
            EncodeSettings {
                hdr10_fallback: true,
                ..x264_settings()
            },
        ] {
            assert!(validate_settings(&invalid).is_err());
        }
        let unknown_chroma = source();
        assert!(
            Plan::build(
                &unknown_chroma,
                &unknown_chroma.selected(&[0]).unwrap(),
                &x264_settings()
            )
            .is_err()
        );
        let (hdr, _) = hdr_source_and_frames();
        let error = Plan::build(&hdr, &hdr.selected(&[0]).unwrap(), &x264_settings()).unwrap_err();
        assert!(error.message.contains("SDR output only"));
    }

    #[test]
    fn x264_preserves_sdr_depth_range_chroma_and_fractional_timing() {
        for depth in [8, 10] {
            for range in ["tv", "pc"] {
                for chroma in ["left", "center", "topleft"] {
                    let mut source = source();
                    let pixel = if depth == 10 {
                        "yuv420p10le"
                    } else if range == "pc" {
                        "yuvj420p"
                    } else {
                        "yuv420p"
                    };
                    source.streams[0].pix_fmt = Some(pixel.into());
                    source.streams[0].color_range = Some(range.into());
                    source.streams[0].chroma_location = Some(chroma.into());
                    let mut plan =
                        Plan::build(&source, &source.selected(&[0]).unwrap(), &x264_settings())
                            .unwrap();
                    assert_eq!(plan.output_bit_depth(), depth);
                    assert_eq!(plan.output_codec(), "h264");
                    let mut decoded = frames(&["0", "0.042", "0.083", "0.125"]);
                    for frame in &mut decoded.frames {
                        frame.pix_fmt = Some(pixel.into());
                        frame.color_range = Some(range.into());
                        frame.chroma_location = Some(chroma.into());
                    }
                    assert_eq!(
                        plan.validate_source_frames(&decoded, &source.streams[0])
                            .unwrap(),
                        4
                    );
                    plan.validate_encoded_stream(&source.streams[0], &source.streams[0])
                        .unwrap();
                    assert_eq!(
                        plan.validate_frames(&decoded, &source.streams[0], true)
                            .unwrap(),
                        4
                    );
                    let mut wrong_codec = source.streams[0].clone();
                    wrong_codec.codec_name = Some("av1".into());
                    assert!(
                        plan.validate_encoded_stream(&source.streams[0], &wrong_codec)
                            .is_err()
                    );
                    decoded.frames[1].pix_fmt = Some(
                        if depth == 10 {
                            "yuv420p"
                        } else {
                            "yuv420p10le"
                        }
                        .into(),
                    );
                    assert!(
                        plan.validate_frames(&decoded, &source.streams[0], true)
                            .is_err()
                    );
                }
            }
        }
    }

    #[test]
    fn planar_source_formats_are_validated_before_explicit_output_conversion() {
        for (source_format, source_range, output_format) in [
            ("yuv422p", "tv", "yuv420p"),
            ("yuv444p", "tv", "yuv420p"),
            ("yuv422p10le", "tv", "yuv420p10le"),
            ("yuv444p10le", "tv", "yuv420p10le"),
            ("yuv420p12le", "tv", "yuv420p10le"),
            ("yuv422p12le", "tv", "yuv420p10le"),
            ("yuv444p12le", "tv", "yuv420p10le"),
            ("yuvj422p", "pc", "yuv420p"),
            ("yuvj444p", "pc", "yuv420p"),
            ("yuyv422", "tv", "yuv420p"),
            ("uyvy422", "tv", "yuv420p"),
        ] {
            let mut source = source();
            source.streams[0].pix_fmt = Some(source_format.into());
            source.streams[0].color_range = Some(source_range.into());
            source.streams[0].chroma_location = Some("left".into());
            let mut plan =
                Plan::build(&source, &source.selected(&[0]).unwrap(), &x264_settings()).unwrap();
            assert_eq!(plan.source_pixel_format, source_format);
            assert_eq!(plan.output_pixel_format, output_format);
            let filter = plan.decoder_filter().unwrap();
            assert!(filter.contains("scale=iw:ih:"), "{source_format}: {filter}");
            assert!(filter.contains(&format!("format={output_format}")));
            assert!(filter.contains("in_color_matrix=bt709:out_color_matrix=bt709"));
            let mut decoded = frames(&["0", "0.042"]);
            for frame in &mut decoded.frames {
                frame.pix_fmt = Some(source_format.into());
                frame.color_range = Some(source_range.into());
                frame.chroma_location = Some("left".into());
            }
            assert_eq!(
                plan.validate_source_frames(&decoded, &source.streams[0])
                    .unwrap(),
                2
            );
            let mut wrong = Frames {
                frames: decoded.frames.clone(),
            };
            wrong.frames[1].pix_fmt = Some(output_format.into());
            assert!(
                plan.validate_source_frames(&wrong, &source.streams[0])
                    .is_err()
            );
            let mut encoded = source.streams[0].clone();
            encoded.pix_fmt = Some(output_format.into());
            assert!(plan.matches_output_format(encoded.pix_fmt.as_deref()));
            assert!(!plan.matches_output_format(Some(source_format)));
        }
    }

    #[test]
    fn opaque_rgb_has_an_explicit_gbr_to_bt709_conversion_and_alpha_is_rejected() {
        for source_format in ["rgb24", "bgr24"] {
            let mut source = source();
            let stream = &mut source.streams[0];
            stream.pix_fmt = Some(source_format.into());
            stream.color_range = Some("pc".into());
            stream.color_space = Some("gbr".into());
            stream.chroma_location = None;
            let mut plan =
                Plan::build(&source, &source.selected(&[0]).unwrap(), &x264_settings()).unwrap();
            assert_eq!(plan.source_pixel_format, source_format);
            assert_eq!(plan.output_pixel_format, "yuv420p");
            assert_eq!(plan.matrix, 1);
            assert_eq!(plan.chroma, "left");
            assert!(
                plan.decoder_filter()
                    .unwrap()
                    .contains("out_color_matrix=bt709:in_range=full:out_range=full")
            );
            let mut decoded = frames(&["0", "0.042"]);
            for frame in &mut decoded.frames {
                frame.pix_fmt = Some(source_format.into());
                frame.color_range = Some("pc".into());
                frame.color_space = Some("gbr".into());
                frame.chroma_location = None;
            }
            assert_eq!(
                plan.validate_source_frames(&decoded, &source.streams[0])
                    .unwrap(),
                2
            );
            let mut encoded = source.streams[0].clone();
            encoded.codec_name = Some("h264".into());
            encoded.pix_fmt = Some("yuv420p".into());
            encoded.color_space = Some("bt709".into());
            encoded.chroma_location = Some("left".into());
            assert!(
                plan.validate_encoded_stream(&source.streams[0], &encoded)
                    .is_ok()
            );
            source.streams[0].pix_fmt = Some("rgba".into());
            assert!(
                Plan::build(&source, &source.selected(&[0]).unwrap(), &x264_settings()).is_err()
            );
            source.streams[0].pix_fmt = Some(source_format.into());
            source.streams[0].color_space = Some("bt709".into());
            assert!(
                Plan::build(&source, &source.selected(&[0]).unwrap(), &x264_settings()).is_err()
            );
        }
    }

    #[test]
    fn svt_converts_full_range_opaque_rgb_to_limited_range_yuv() {
        let mut source = source();
        let stream = &mut source.streams[0];
        stream.pix_fmt = Some("rgb24".into());
        stream.color_range = Some("pc".into());
        stream.color_space = Some("gbr".into());
        stream.chroma_location = None;
        let settings = EncodeSettings {
            encoder: VideoEncoder::SvtAv1,
            ..Default::default()
        };
        let plan = Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).unwrap();
        assert_eq!(plan.source_pixel_format, "rgb24");
        assert_eq!(plan.output_pixel_format, "yuv420p10le");
        assert!(!plan.full_range);
        assert!(
            plan.decoder_filter()
                .unwrap()
                .contains("in_range=full:out_range=limited")
        );
        let mut output = source.streams[0].clone();
        output.codec_name = Some("av1".into());
        output.pix_fmt = Some("yuv420p10le".into());
        output.color_range = Some("tv".into());
        output.color_space = Some("bt709".into());
        output.chroma_location = Some("left".into());
        assert!(
            plan.validate_encoded_stream(&source.streams[0], &output)
                .is_ok()
        );
        source.streams[0].pix_fmt = Some("yuvj420p".into());
        assert!(Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).is_err());
    }

    #[test]
    fn shared_output_format_cannot_bypass_av1an_options_or_encoder_chroma_limits() {
        let source = source();
        let shared = EncodeSettings {
            backend: EncodeBackend::Av1an,
            encoder: VideoEncoder::X264,
            output_pixel_format: Some(media_core::Av1anPixelFormat::Yuv444p),
            ..x264_settings()
        };
        assert!(Plan::build(&source, &source.selected(&[0]).unwrap(), &shared).is_err());
        let conflict = EncodeSettings {
            av1an_options: Some(media_core::Av1anOptions {
                pixel_format: Some(media_core::Av1anPixelFormat::Yuv420p),
                ..Default::default()
            }),
            ..shared.clone()
        };
        assert!(Plan::build(&source, &source.selected(&[0]).unwrap(), &conflict).is_err());
        let unsupported = EncodeSettings {
            backend: EncodeBackend::Standalone,
            encoder: VideoEncoder::VpxStandalone,
            output_pixel_format: Some(media_core::Av1anPixelFormat::Yuv422p),
            ..Default::default()
        };
        assert!(Plan::build(&source, &source.selected(&[0]).unwrap(), &unsupported).is_err());
    }

    #[test]
    fn vp9_alpha_requires_explicit_standalone_route_and_alpha_source() {
        let opaque = source();
        let alpha_settings = EncodeSettings {
            encoder: VideoEncoder::Vp9,
            output_pixel_format: Some(media_core::Av1anPixelFormat::Yuva420p),
            ..Default::default()
        };
        assert!(Plan::build(&opaque, &opaque.selected(&[0]).unwrap(), &alpha_settings).is_err());
        let mut transparent = source();
        transparent.streams[0].pix_fmt = Some("yuva420p".into());
        transparent.streams[0].chroma_location = Some("left".into());
        assert!(
            Plan::build(
                &transparent,
                &transparent.selected(&[0]).unwrap(),
                &EncodeSettings {
                    encoder: VideoEncoder::Vp9,
                    ..Default::default()
                }
            )
            .is_err()
        );
        let plan = Plan::build(
            &transparent,
            &transparent.selected(&[0]).unwrap(),
            &alpha_settings,
        )
        .unwrap();
        assert!(plan.has_alpha());
        let wrong_encoder = EncodeSettings {
            encoder: VideoEncoder::X264,
            ..alpha_settings.clone()
        };
        assert!(
            Plan::build(
                &transparent,
                &transparent.selected(&[0]).unwrap(),
                &wrong_encoder
            )
            .is_err()
        );
        let av1an = EncodeSettings {
            backend: EncodeBackend::Av1an,
            encoder: VideoEncoder::Vp9,
            av1an_options: Some(media_core::Av1anOptions {
                pixel_format: Some(media_core::Av1anPixelFormat::Yuva420p),
                ..Default::default()
            }),
            output_pixel_format: None,
            ..alpha_settings.clone()
        };
        assert!(Plan::build(&transparent, &transparent.selected(&[0]).unwrap(), &av1an).is_err());
        let mut tagged = source();
        tagged.streams[0].codec_name = Some("vp9".into());
        tagged.streams[0].chroma_location = Some("left".into());
        tagged.streams[0]
            .tags
            .insert("ALPHA_MODE".into(), "1".into());
        assert!(
            Plan::build(
                &tagged,
                &tagged.selected(&[0]).unwrap(),
                &EncodeSettings {
                    encoder: VideoEncoder::Vp9,
                    ..Default::default()
                }
            )
            .is_err()
        );
        let plan = Plan::build(&tagged, &tagged.selected(&[0]).unwrap(), &alpha_settings).unwrap();
        assert!(plan.source_is_vp9_alpha());
    }

    #[test]
    fn full_chroma_output_and_unsupported_source_formats_remain_distinct() {
        let mut source = source();
        source.streams[0].pix_fmt = Some("yuv444p10le".into());
        source.streams[0].chroma_location = None;
        let settings = EncodeSettings {
            backend: EncodeBackend::Av1an,
            av1an_options: Some(media_core::Av1anOptions {
                pixel_format: Some(media_core::Av1anPixelFormat::Yuv444p10le),
                ..Default::default()
            }),
            ..x264_settings()
        };
        let plan = Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).unwrap();
        assert_eq!(plan.output_pixel_format, "yuv444p10le");
        assert_eq!(plan.chroma, "left");
        assert!(plan.pixel_conversion().is_none());
        for unsupported_format in ["rgb24", "gbrp", "yuva420p", "nv12", "p010le"] {
            source.streams[0].pix_fmt = Some(unsupported_format.into());
            assert!(
                Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).is_err(),
                "{unsupported_format}"
            );
        }
        source.streams[0].pix_fmt = Some("yuvj444p".into());
        source.streams[0].color_range = Some("tv".into());
        assert!(Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).is_err());
    }
    #[test]
    fn fractional_cfr_accepts_timestamp_rounding_but_rejects_vfr() {
        let source = source();
        let settings = EncodeSettings::default();
        let plan = Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).unwrap();
        assert_eq!(
            plan.validate_frames(
                &frames(&["0", "0.042", "0.083", "0.125"]),
                &source.streams[0],
                false
            )
            .unwrap(),
            4
        );
        assert!(
            plan.validate_frames(&frames(&["0", "0.042", "0.100"]), &source.streams[0], false)
                .is_err()
        );
    }

    fn long_cfr_frames(num: u64, den: u64) -> Frames {
        let template = frames(&["0"]).frames.pop().unwrap();
        Frames {
            frames: (0..40_000)
                .map(|index| {
                    let mut frame = template.clone();
                    let millis = (index * den * 1000 + num / 2) / num;
                    frame.best_effort_timestamp_time =
                        Some(format!("{}.{:03}", millis / 1000, millis % 1000));
                    frame
                })
                .collect(),
        }
    }

    #[test]
    fn long_ntsc_and_decimal_cadences_validate_without_widening_tolerance() {
        let source = source();
        for (num, den) in [(24000, 1001), (2997, 125)] {
            let mut plan = Plan::build(
                &source,
                &source.selected(&[0]).unwrap(),
                &EncodeSettings::default(),
            )
            .unwrap();
            let frames = long_cfr_frames(num, den);
            let tolerance = plan.tolerance;
            if num == 2997 {
                assert!(
                    plan.validate_frames(&frames, &source.streams[0], false)
                        .is_err()
                );
            }
            assert_eq!(
                plan.validate_source_frames(&frames, &source.streams[0])
                    .unwrap(),
                40_000
            );
            assert_eq!((plan.fps_num, plan.fps_den), (num as u32, den as u32));
            assert_eq!(plan.tolerance, tolerance);
            let mut output = frames;
            for frame in &mut output.frames {
                frame.pix_fmt = Some("yuv420p10le".into());
            }
            assert!(
                plan.validate_frames(&output, &source.streams[0], true)
                    .is_ok()
            );
            if num == 2997 {
                // Output must preserve the selected cadence, not revert to the
                // declared rate after reconciliation.
                let mut wrong_rate = long_cfr_frames(24000, 1001);
                for frame in &mut wrong_rate.frames {
                    frame.pix_fmt = Some("yuv420p10le".into());
                }
                assert!(
                    plan.validate_frames(&wrong_rate, &source.streams[0], true)
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn online_validation_rejects_empty_video_and_late_hdr_changes() {
        let (source, hdr_frames) = hdr_source_and_frames();
        let mut plan =
            Plan::build(&source, &[&source.streams[0]], &EncodeSettings::default()).unwrap();
        let empty = Frames { frames: Vec::new() };
        assert!(
            plan.validate_source_frames(&empty, &source.streams[0])
                .is_err()
        );
        assert!(
            plan.validate_frames(&empty, &source.streams[0], true)
                .is_err()
        );
        let mut decoded = long_cfr_frames(24000, 1001);
        for frame in &mut decoded.frames {
            frame.pix_fmt = hdr_frames.frames[0].pix_fmt.clone();
            frame.color_primaries = hdr_frames.frames[0].color_primaries.clone();
            frame.color_space = hdr_frames.frames[0].color_space.clone();
            frame.color_transfer = hdr_frames.frames[0].color_transfer.clone();
        }
        decoded.frames[0].side_data_list = hdr_frames.frames[0].side_data_list.clone();
        let mut valid = plan.clone();
        assert_eq!(
            valid
                .validate_source_frames(&decoded, &source.streams[0])
                .unwrap(),
            40_000
        );
        let mut changed = mastering();
        changed["max_luminance"] = "2000/1".into();
        decoded
            .frames
            .last_mut()
            .unwrap()
            .side_data_list
            .push(changed);
        assert!(
            plan.validate_source_frames(&decoded, &source.streams[0])
                .is_err()
        );
        assert!(
            valid
                .validate_frames(&decoded, &source.streams[0], true)
                .is_err()
        );
    }

    #[test]
    fn reconciliation_rejects_gaps_duplicates_nonlinear_drift_and_changed_frames() {
        let source = source();
        for defect in [
            "gap",
            "duplicate",
            "nonlinear",
            "offset",
            "geometry",
            "color",
            "hdr",
        ] {
            let mut plan = Plan::build(
                &source,
                &source.selected(&[0]).unwrap(),
                &EncodeSettings::default(),
            )
            .unwrap();
            let mut frames = long_cfr_frames(2997, 125);
            match defect {
                "gap" => {
                    for frame in &mut frames.frames[20_000..] {
                        let time = seconds(frame.best_effort_timestamp_time.as_deref()).unwrap();
                        frame.best_effort_timestamp_time = Some(format!("{:.6}", time + 0.042));
                    }
                }
                "duplicate" => {
                    frames.frames[20_000].best_effort_timestamp_time =
                        frames.frames[19_999].best_effort_timestamp_time.clone()
                }
                "nonlinear" => {
                    for (index, frame) in frames.frames.iter_mut().enumerate() {
                        let time = seconds(frame.best_effort_timestamp_time.as_deref()).unwrap();
                        let drift = 0.005 * (index as f64 / 40_000.0).powi(2);
                        frame.best_effort_timestamp_time = Some(format!("{:.6}", time + drift));
                    }
                }
                "offset" => frames.frames[0].best_effort_timestamp_time = Some("0.010".into()),
                "geometry" => frames.frames[20_000].sample_aspect_ratio = Some("4:3".into()),
                "color" => frames.frames[20_000].color_primaries = Some("bt2020".into()),
                "hdr" => frames.frames[20_000]
                    .side_data_list
                    .push(serde_json::json!({"side_data_type": "Mastering display metadata"})),
                _ => unreachable!(),
            }
            assert!(
                plan.validate_source_frames(&frames, &source.streams[0])
                    .is_err(),
                "{defect}"
            );
            assert_eq!((plan.fps_num, plan.fps_den), (24000, 1001));
        }
    }

    #[test]
    fn fine_source_timebase_uses_matroska_timebase_only_for_encoded_output() {
        let mut source = source();
        source.streams[0].avg_frame_rate = Some("24/1".into());
        source.streams[0].time_base = Some("1/12288".into());
        let plan = Plan::build(
            &source,
            &source.selected(&[0]).unwrap(),
            &EncodeSettings::default(),
        )
        .unwrap();
        assert!(
            plan.validate_frames(
                &frames(&["0", "0.041667", "0.083333"]),
                &source.streams[0],
                false
            )
            .is_ok()
        );
        assert!(
            plan.validate_frames(&frames(&["0", "0.042", "0.083"]), &source.streams[0], false)
                .is_err()
        );
        let mut encoded = source.streams[0].clone();
        encoded.time_base = Some("1/1000".into());
        encoded.pix_fmt = Some("yuv420p10le".into());
        let mut output = frames(&["0", "0.042", "0.083"]);
        for frame in &mut output.frames {
            frame.pix_fmt = Some("yuv420p10le".into());
        }
        assert!(plan.validate_frames(&output, &encoded, true).is_ok());
    }
    #[test]
    fn rejects_unsupported_color_geometry_interlace_and_settings() {
        for (field, value) in [
            ("color_transfer", "smpte2084"),
            ("pix_fmt", "rgb24"),
            ("sample_aspect_ratio", "4:3"),
            ("field_order", "tt"),
            ("start_time", "1"),
        ] {
            let mut source = serde_json::to_value(serde_json::from_str::<serde_json::Value>(r#"{"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":128,"height":96,"pix_fmt":"yuv420p","field_order":"progressive","sample_aspect_ratio":"1:1","avg_frame_rate":"24/1","start_time":"0","color_space":"bt709","color_primaries":"bt709","color_transfer":"bt709","color_range":"tv"}],"format":{"start_time":"0"}}"#).unwrap()).unwrap();
            source["streams"][0][field] = value.into();
            let source: Document = serde_json::from_value(source).unwrap();
            assert!(
                Plan::build(
                    &source,
                    &source.selected(&[0]).unwrap(),
                    &EncodeSettings::default()
                )
                .is_err(),
                "{field}"
            );
        }
        assert!(
            validate_settings(&EncodeSettings {
                crf: 0,
                ..EncodeSettings::default()
            })
            .is_err()
        );
        assert!(
            validate_settings(&EncodeSettings {
                preset: 14,
                ..EncodeSettings::default()
            })
            .is_err()
        );
    }

    #[test]
    fn rejects_frame_level_interlace_and_hdr_and_multiple_video_selection() {
        let source = source();
        let plan = Plan::build(
            &source,
            &source.selected(&[0]).unwrap(),
            &EncodeSettings::default(),
        )
        .unwrap();
        let mut interlaced = frames(&["0"]);
        interlaced.frames[0].interlaced_frame = Some(1);
        assert!(
            plan.validate_frames(&interlaced, &source.streams[0], false)
                .is_err()
        );
        let mut changed_geometry = frames(&["0", "0.042"]);
        changed_geometry.frames[1].sample_aspect_ratio = Some("4:3".into());
        assert!(
            plan.validate_frames(&changed_geometry, &source.streams[0], false)
                .is_err()
        );
        let mut changed_chroma = frames(&["0", "0.042"]);
        changed_chroma.frames[1].chroma_location = Some("left".into());
        assert!(
            plan.validate_frames(&changed_chroma, &source.streams[0], false)
                .is_err()
        );
        let mut hdr = frames(&["0"]);
        hdr.frames[0].side_data_list.push(
            serde_json::json!({"side_data_type":"HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"}),
        );
        assert!(
            plan.validate_frames(&hdr, &source.streams[0], false)
                .is_err()
        );
        let mut source = source.clone();
        let mut second = source.streams[0].clone();
        second.index = 1;
        source.streams.push(second);
        assert!(
            Plan::build(
                &source,
                &source.selected(&[0, 1]).unwrap(),
                &EncodeSettings::default()
            )
            .is_err()
        );
        assert!(
            Plan::build(
                &source,
                &source.selected(&[0]).unwrap(),
                &EncodeSettings {
                    video_stream_index: 1,
                    ..EncodeSettings::default()
                }
            )
            .is_err()
        );
    }
}
