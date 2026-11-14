use media_core::{AppError, EncodeBackend, EncodeSettings, VideoEncoder};
use serde::Deserialize;

use super::metadata::{Document, Stream};

mod aspect;
mod framing;
pub(super) mod hdr;
mod temporal;
mod tone_map;
use hdr::{Hdr10, StaticMetadata, validate_side_data};
mod validation;
pub(super) use validation::Validation;

fn display_matrix(value: &serde_json::Value) -> bool {
    matches!(
        value
            .get("side_data_type")
            .and_then(serde_json::Value::as_str),
        Some("Display Matrix" | "3x3 displaymatrix")
    )
}

pub(super) fn preserves_dolby_source_clock(video: &Stream, settings: &EncodeSettings) -> bool {
    let source = settings.tone_map.is_some_and(|tone| tone.source_workflow);
    let legacy = settings.trim.is_none()
        && !settings
            .subtitles
            .iter()
            .any(|track| track.mode == media_core::SubtitleMode::BurnIn || track.burn_in)
        && !settings.external_tracks.iter().any(|track| {
            track.subtitle_burn_in || track.subtitle_mode == Some(media_core::SubtitleMode::BurnIn)
        });
    settings.backend == EncodeBackend::Standalone
        && (source || legacy)
        && !settings.hdr_metadata.as_ref().is_some_and(|metadata| {
            metadata.dolby_vision_rpu_path.is_some() || metadata.hdr10_plus_json_path.is_some()
        })
        && tone_map::Transform::build(video, settings)
            .ok()
            .flatten()
            .is_some_and(|tone| tone.permits_original_clock())
}

pub(super) fn source_av1an_base_clock(video: &Stream, settings: &EncodeSettings) -> bool {
    settings.backend == EncodeBackend::Av1an
        && settings
            .av1an_options
            .is_some_and(|options| options.source_workflow)
        && settings.tone_map.is_some_and(|tone| {
            tone.source_workflow
                && tone.backend != media_core::ToneMapBackend::Gpu
                && tone.algorithm != Some(media_core::ToneMapAlgorithm::Spline)
        })
        && !settings.hdr_metadata.as_ref().is_some_and(|metadata| {
            metadata.dolby_vision_rpu_path.is_some() || metadata.hdr10_plus_json_path.is_some()
        })
        && !video.side_data_list.iter().any(display_matrix)
        && !video
            .tags
            .keys()
            .any(|key| key.eq_ignore_ascii_case("rotate"))
        && tone_map::compatible_dolby_base(video)
}

pub(super) struct ImageFilterPlan {
    pub filter: Option<String>,
    pub prefix: String,
    pub suffix: String,
    pub device_args: Vec<std::ffi::OsString>,
    pub input_seek: Option<String>,
    pub output_trim: Vec<std::ffi::OsString>,
    pub source_clock: bool,
    pub width: u32,
    pub height: u32,
}
pub(super) async fn image_filter_plan(
    video: &Stream,
    processing: &media_core::ImageProcessingSettings,
    text: Option<&str>,
    ffmpeg: &std::path::Path,
    input: &std::path::Path,
    cancel: &tokio::sync::watch::Receiver<bool>,
    defer_vulkan: bool,
) -> Result<ImageFilterPlan, AppError> {
    let mut video = video.clone();
    let mut processing = processing.clone();
    let angle = video
        .side_data_list
        .iter()
        .find_map(|value| value.get("rotation").and_then(serde_json::Value::as_f64))
        .unwrap_or(0.0)
        .round() as i32;
    let oriented = super::source_normalization::oriented_settings(
        &EncodeSettings {
            framing: processing.framing,
            ..Default::default()
        },
        angle,
    );
    processing.framing = oriented.framing;
    if matches!(angle.rem_euclid(360), 90 | 270) {
        std::mem::swap(&mut video.width, &mut video.height);
        if let Some((n, d)) = video
            .sample_aspect_ratio
            .as_deref()
            .and_then(|value| value.split_once(':'))
            .map(|(n, d)| (n.to_owned(), d.to_owned()))
        {
            video.sample_aspect_ratio = Some(format!("{d}:{n}"));
        }
    }
    let settings = EncodeSettings {
        encoder: VideoEncoder::X264,
        framing: processing.framing,
        temporal: processing.temporal,
        trim: processing.trim,
        tone_map: processing.tone_map,
        av1an_filters: processing.filters.clone(),
        source_filters: processing.source_filters,
        ..Default::default()
    };
    framing::validate(&settings)?;
    temporal::validate(&settings)?;
    tone_map::validate_settings(&settings)?;
    super::av1an::validate_filters(&settings)?;
    super::trim::validate_settings(&settings)?;
    let source_clock =
        processing.source_filters || processing.tone_map.is_some_and(|tone| tone.source_workflow);
    let mut filters = if source_clock {
        Vec::new()
    } else {
        vec!["setpts=PTS-STARTPTS".to_owned()]
    };
    if !source_clock && let Some(trim) = processing.trim {
        if let Some(time) = trim.time {
            if time.end_milliseconds <= time.start_milliseconds {
                return Err(unsupported("Image trim end must follow its start."));
            }
            filters.push(format!(
                "trim=start={:.3}:end={:.3},setpts=PTS-STARTPTS",
                f64::from(time.start_milliseconds) / 1000.0,
                f64::from(time.end_milliseconds) / 1000.0
            ));
        } else {
            if trim.end_frame_exclusive <= trim.start_frame {
                return Err(unsupported("Image trim end must follow its start."));
            }
            filters.push(format!(
                "trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS",
                trim.start_frame, trim.end_frame_exclusive
            ));
        }
    }
    if let Some(temporal) = processing.temporal {
        if let Some(deint) = temporal.deinterlace {
            filters.push(format!(
                "{}=mode={}:parity={}:deint=all",
                deint.engine.ffmpeg(),
                if deint.mode == media_core::DeinterlaceMode::Bob {
                    "send_field"
                } else {
                    "send_frame"
                },
                if deint.field_order == media_core::FieldOrder::TopFirst {
                    "tff"
                } else {
                    "bff"
                }
            ));
        }
        if let Some(cadence) = temporal.cadence_repair {
            if cadence.kind == media_core::CadenceRepairKind::ExactDuplicates {
                filters.push("mpdecimate=max=0:keep=0:hi=0:lo=0:frac=1".into());
            } else {
                filters.push(format!(
                    "fieldmatch=order={}:combmatch=full",
                    if cadence.field_order == media_core::FieldOrder::TopFirst {
                        "tff"
                    } else {
                        "bff"
                    }
                ));
                if cadence.combed_fallback {
                    filters.push("bwdif=mode=send_frame:parity=auto:deint=interlaced".into());
                }
                filters.push("decimate=cycle=5".into());
            }
        }
    }
    let mut device_args = Vec::new();
    if let Some(mut tone) = tone_map::Transform::build(&video, &settings)? {
        if !(defer_vulkan && tone.requires_vulkan()) {
            tone.resolve(ffmpeg, input, video.index, None, None, cancel)
                .await?;
        }
        device_args = tone.device_args();
        filters.push(tone.filter());
    }
    if let Some(rate) = processing.temporal.and_then(|value| value.frame_rate) {
        filters.push(format!(
            "fps=fps={}/{}:start_time=0:round=near:eof_action=round",
            rate.numerator, rate.denominator
        ));
    }
    let prefix = filters.join(",");
    filters.clear();
    let (width, height) = (
        video
            .width
            .ok_or_else(|| unsupported("Image source width is absent."))?,
        video
            .height
            .ok_or_else(|| unsupported("Image source height is absent."))?,
    );
    let (width, height) = if processing.framing != media_core::VideoFraming::default() {
        let sar = aspect::parse_source(video.sample_aspect_ratio.as_deref())?;
        let (n, d) = sar.split_once(':').expect("validated SAR");
        let mut geometry = framing::Geometry::build_with_sar(
            width,
            height,
            processing.framing,
            (n.parse().unwrap(), d.parse().unwrap()),
        )?;
        geometry.resize_filter = processing.temporal.unwrap_or_default().resize_filter;
        if let Some(filter) = geometry.image_filter_with_text(text) {
            filters.push(filter)
        }
        (geometry.width, geometry.height)
    } else {
        if let Some(text) = text {
            filters.push(text.to_owned());
        }
        (width, height)
    };
    if let Some(aspect) = processing.temporal.and_then(|value| value.aspect_ratio) {
        filters.push(aspect::Transform::build(aspect, width, height)?.filter());
    }
    filters.extend(super::av1an::filter_rows(&settings));
    let suffix = filters.join(",");
    let filter = [prefix.as_str(), suffix.as_str()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(",");
    let input_seek = source_clock
        .then_some(processing.trim)
        .flatten()
        .and_then(|trim| trim.time)
        .filter(|time| time.seek_strategy == media_core::VideoSeekStrategy::InputSeek)
        .map(|time| format!("{:.3}", f64::from(time.start_milliseconds) / 1000.0));
    let output_trim = if source_clock {
        let rate = [
            video.avg_frame_rate.as_deref(),
            video.r_frame_rate.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|value| value.split_once('/'))
        .filter_map(|(n, d)| Some((n.parse::<u32>().ok()?, d.parse::<u32>().ok()?)))
        .find(|(n, d)| *n > 0 && *d > 0);
        if rate.is_none() && processing.trim.is_some_and(|trim| trim.time.is_none()) {
            return Err(unsupported(
                "Source image frame trim requires a valid nominal frame rate.",
            ));
        }
        let rate = rate.unwrap_or((1, 1));
        source_trim_arguments(
            processing.trim,
            rate,
            source_keeps_frame_count(processing.temporal, rate),
        )
    } else {
        Vec::new()
    };
    Ok(ImageFilterPlan {
        filter: Some(if filter.is_empty() {
            "null".into()
        } else {
            filter
        }),
        prefix: if prefix.is_empty() {
            "null".into()
        } else {
            prefix
        },
        suffix,
        device_args,
        input_seek,
        output_trim,
        source_clock,
        width,
        height,
    })
}

fn source_keeps_frame_count(
    temporal: Option<media_core::TemporalSettings>,
    rate: (u32, u32),
) -> bool {
    temporal.is_none_or(|temporal| {
        !temporal
            .deinterlace
            .is_some_and(|deinterlace| deinterlace.mode == media_core::DeinterlaceMode::Bob)
            && !temporal
                .qtgmc
                .is_some_and(|qtgmc| qtgmc.mode == media_core::DeinterlaceMode::Bob)
            && temporal.frame_rate.is_none_or(|value| {
                (f64::from(value.numerator) / f64::from(value.denominator)
                    - f64::from(rate.0) / f64::from(rate.1))
                .abs()
                    < 0.01
            })
    })
}

fn source_trim_arguments(
    trim: Option<media_core::VideoTrim>,
    rate: (u32, u32),
    keep_count: bool,
) -> Vec<std::ffi::OsString> {
    let Some(trim) = trim else {
        return Vec::new();
    };
    let mut args = Vec::new();
    if let Some(time) = trim.time {
        if time.seek_strategy == media_core::VideoSeekStrategy::DecodeFromStart {
            args.extend([
                "-ss".into(),
                format!("{:.3}", f64::from(time.start_milliseconds) / 1000.0).into(),
            ]);
        }
        args.extend([
            "-t".into(),
            format!(
                "{:.3}",
                f64::from(time.end_milliseconds - time.start_milliseconds) / 1000.0
            )
            .into(),
        ]);
    } else {
        let (n, d) = rate;
        let frame = f64::from(d) / f64::from(n);
        let start = ((f64::from(trim.start_frame) * frame - frame / 2.0).max(0.0) * 1000.0).floor()
            / 1000.0;
        let duration =
            ((f64::from(trim.end_frame_exclusive - trim.start_frame) + 0.5) * frame * 1000.0)
                .floor()
                / 1000.0;
        args.extend([
            "-ss".into(),
            format!("{start:.3}").into(),
            "-t".into(),
            format!("{duration:.3}").into(),
        ]);
        if keep_count {
            args.extend([
                "-frames:v".into(),
                (trim.end_frame_exclusive - trim.start_frame)
                    .to_string()
                    .into(),
            ]);
        }
    }
    args
}

#[derive(Clone, Debug)]
pub(super) struct Plan {
    pub trim: Option<media_core::VideoTrim>,
    /// Verified common mux clock for source packet-granular trim.
    pub output_origin: f64,
    /// Validated source origin. Copied AV1AN cuts additionally permit ordinal
    /// clocks; source Dolby rendering additionally validates monotonic clocks
    /// before the actual producer/engine establishes its encoded CFR contract.
    pub source_origin: f64,
    /// Set only after the source AV1AN copied-cut receipt is fully verified.
    /// That reader consumes every decoded frame by ordinal, retaining copied
    /// GOP boundary gaps in the input clock rather than inventing frames.
    pub source_cut_ordinals: bool,
    /// A container edit marked video packets for decode only. Stage the
    /// presented ordinals at the validated source rate before chunk reading.
    pub(super) av1an_presented_rate: Option<(u32, u32)>,
    source_dolby_clock: bool,
    pub(super) source_dolby_render: bool,
    source_av1an_base_clock: bool,
    source_rotation: i32,
    source_coded_size: (u32, u32),
    // Only an internally verified copied cut may carry its original matrix.
    source_cut_transform: Option<&'static str>,
    pub source_trim: bool,
    tone_map: Option<tone_map::Transform>,
    grain_prefilter: Option<u8>,
    prepare_metric_reference: bool,
    custom_filters: Vec<String>,
    source_filters: bool,
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
    /// Frozen typed processing rate before arbitrary source filter output is
    /// observed. The fork captures its chunk interval at this same boundary.
    source_chunk_rate: (u32, u32),
    source_av1an_input_rate: (u32, u32),
    source_av1an_workflow: bool,
    source_queue: Option<std::sync::Arc<super::source_queue::Regions>>,
    basic_source_queue: bool,
    pub cadence_reconciled: bool,
    pub tolerance: f64,
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
    source_full_range: bool,
    pub chroma: &'static str,
    hdr10: Option<Hdr10>,
    source_hdr10: Option<Hdr10>,
    authored_hdr: Option<StaticMetadata>,
}

pub(super) fn unsupported(message: &str) -> AppError {
    AppError::new("ENCODE_INPUT_UNSUPPORTED", message, None)
}

pub(super) fn validate_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    super::encoding_preferences::validate(settings)?;
    if settings.video_mode == media_core::VideoMode::Encode {
        settings.encoder.require_cpu_encoding()?;
        if settings.source_nvenc_quality {
            return Err(AppError::new(
                "HARDWARE_ENCODING_UNSUPPORTED",
                "This historical hardware encoding profile is unsupported in CPU-only Jesses.",
                None,
            ));
        }
    }
    if settings.video_mode != media_core::VideoMode::Encode {
        return super::conversion::validate_settings(settings);
    }
    validate_hdr_metadata_settings(settings)?;
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
            settings
                .svt_crf_quarter_steps
                .map_or_else(|| settings.crf <= 63, |value| value <= 280)
                && settings
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
                && (settings.constant_quantizer.is_some() || settings.crf <= 51)
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

/// How far an encoded frame's timestamp may sit from the planned clock. The
/// output container may quantize a fine time base, Matroska to milliseconds.
pub(in crate::jobs) fn encoded_timestamp_tolerance(stream: &Stream) -> f64 {
    rational(stream.time_base.as_deref())
        .map(|(n, d)| f64::from(n) / f64::from(d))
        .unwrap_or(0.000001)
        .clamp(0.000001, 0.001)
        + 0.000002
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

fn unspecified(text: Option<&str>) -> bool {
    matches!(text, None | Some("unknown" | "unspecified"))
}
fn source_sdr_color(text: Option<&str>, fallback: u8) -> Result<u8, AppError> {
    if unspecified(text) {
        Ok(fallback)
    } else {
        sdr_color(text)
    }
}
fn color_name(code: u8) -> &'static str {
    match code {
        1 => "bt709",
        5 => "bt470bg",
        6 => "smpte170m",
        9 => "bt2020",
        16 => "smpte2084",
        18 => "arib-std-b67",
        _ => unreachable!("validated color code"),
    }
}
fn matrix_name(code: u8) -> &'static str {
    if code == 9 {
        "bt2020nc"
    } else {
        color_name(code)
    }
}

pub(super) fn validate_hdr_metadata_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    let Some(metadata) = &settings.hdr_metadata else {
        return Ok(());
    };
    hdr::authored(metadata)?;
    if settings.tone_map.is_some()
        || !(settings.encoder.is_svt()
            || matches!(
                settings.encoder,
                VideoEncoder::AomAv1 | VideoEncoder::X265 | VideoEncoder::X265Standalone
            ))
    {
        return Err(unsupported(
            "Authored HDR metadata requires HDR output with SVT-AV1, AOM or x265. Disable SDR tone mapping or choose an HDR-capable encoder.",
        ));
    }
    if metadata.dolby_vision_rpu_path.is_some() || metadata.hdr10_plus_json_path.is_some() {
        if !matches!(
            settings.encoder,
            VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr
        ) {
            return Err(unsupported(
                "External Dolby Vision/HDR10+ metadata requires a compatible SVT provider with the matching metadata library.",
            ));
        }
        if settings.trim.is_some()
            || settings.temporal.is_some()
            || settings.framing != media_core::VideoFraming::default()
            || !settings.av1an_filters.is_empty()
            || settings
                .av1an_grain
                .as_ref()
                .is_some_and(|grain| grain.denoise)
        {
            return Err(unsupported(
                "External per-frame HDR metadata must match unchanged complete source frames. Clear trim, geometry, frame-rate, deinterlace and custom filters before importing it.",
            ));
        }
        if settings.backend == EncodeBackend::Av1an
            && !settings.av1an_options.is_some_and(|options| {
                options.split_method == media_core::Av1anSplitMethod::FixedChunks
                    && options.maximum_chunk_frames == 0
            })
        {
            return Err(unsupported(
                "External per-frame HDR metadata requires standalone encoding or a single AV1AN chunk (fixed splitting, maximum chunk frames 0). Parallel chunks require metadata sliced to their source frame intervals.",
            ));
        }
        for path in [
            metadata.dolby_vision_rpu_path.as_deref(),
            metadata.hdr10_plus_json_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if path.contains('\0') || !std::path::Path::new(path).is_absolute() {
                return Err(unsupported(
                    "HDR metadata resources require absolute local file paths.",
                ));
            }
        }
    }
    Ok(())
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
        Some("bgr0") if full_range => Ok(("bgr0", 8)),
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
            | "bgr0"
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
    /// The caller has independently sealed a source AV1AN stream-copy cut.
    /// Plan its display raster while retaining its untouched coded-frame contract.
    pub(super) fn build_source_cut(
        document: &Document,
        selected: &[&Stream],
        settings: &EncodeSettings,
    ) -> Result<Self, AppError> {
        let original = selected
            .iter()
            .copied()
            .find(|video| {
                video.index == settings.video_stream_index
                    && video.codec_type.as_deref() == Some("video")
            })
            .ok_or_else(|| unsupported("The verified cut has no selected video."))?;
        let (swaps, transform) =
            crate::analysis::display_transform_parts(&original.side_data_list, &original.tags)
                .map_err(unsupported)?;
        let mut display = document.clone();
        let video = display
            .streams
            .iter_mut()
            .find(|video| video.index == original.index)
            .expect("selected cut stream");
        video.side_data_list.retain(|value| !display_matrix(value));
        video
            .tags
            .retain(|key, _| !key.eq_ignore_ascii_case("rotate"));
        let raw_sar = aspect::parse_source(original.sample_aspect_ratio.as_deref())?;
        if swaps {
            std::mem::swap(&mut video.width, &mut video.height);
            let (n, d) = raw_sar.split_once(':').expect("validated source SAR");
            video.sample_aspect_ratio = Some(format!("{d}:{n}"));
        }
        let indices = selected
            .iter()
            .map(|stream| stream.index)
            .collect::<Vec<_>>();
        let mut plan = Self::build(&display, &display.selected(&indices)?, settings)?;
        plan.source_coded_size = (
            original
                .width
                .ok_or_else(|| unsupported("The copied source width is absent."))?,
            original
                .height
                .ok_or_else(|| unsupported("The copied source height is absent."))?,
        );
        plan.source_sar = raw_sar;
        plan.source_cut_transform = Some(transform);
        Ok(plan)
    }
    pub fn build(
        _document: &Document,
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
        if videos.len() != 1
            || !videos
                .iter()
                .any(|video| video.index == settings.video_stream_index)
        {
            return Err(unsupported(
                "Select exactly one video stream and make it the video chosen for encoding.",
            ));
        }
        let video = videos
            .iter()
            .find(|video| video.index == settings.video_stream_index)
            .expect("selected video");
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
        let inherently_full = video.pix_fmt.as_deref().is_some_and(|format| {
            format.starts_with("rgb")
                || format.starts_with("bgr")
                || format.starts_with("gbr")
                || format.starts_with("yuvj")
        });
        let source_full_range = match video.color_range.as_deref() {
            Some("pc") => true,
            Some("tv") => false,
            value if unspecified(value) => inherently_full,
            _ => return Err(unsupported("The source color range is unsupported.")),
        };
        let (source_pixel_format, source_depth) = source_pixel_format(
            if source_is_vp9_alpha {
                Some("yuva420p")
            } else {
                video.pix_fmt.as_deref()
            },
            source_full_range,
        )?;
        let rgb_source = rgb_source_format(source_pixel_format);
        let alpha_source = matches!(source_pixel_format, "rgba" | "yuva420p");
        if rgb_source
            && (!unspecified(video.color_space.as_deref())
                && video.color_space.as_deref() != Some("gbr")
                || !unspecified(video.color_primaries.as_deref())
                    && video.color_primaries.as_deref() != Some("bt709")
                || !unspecified(video.color_transfer.as_deref())
                    && video.color_transfer.as_deref() != Some("bt709"))
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
                "Interlaced video requires explicit BWDIF, YADIF, QTGMC, or inverse telecine processing and the matching TFF/BFF source field order.",
            ));
        }
        let source_sar = aspect::parse_source(video.sample_aspect_ratio.as_deref())?;
        let is_hdr10 = video.color_primaries.as_deref() == Some("bt2020")
            && video.color_transfer.as_deref() == Some("smpte2084")
            && video.color_space.as_deref() == Some("bt2020nc")
            && video.pix_fmt.as_deref() == Some("yuv420p10le")
            && !source_full_range;
        if !is_hdr10
            && tone_map.is_none()
            && video.side_data_list.iter().any(|value| {
                matches!(
                    hdr::kind(value),
                    "Mastering display metadata" | "Content light level metadata"
                )
            })
        {
            return Err(unsupported(&format!(
                "Static HDR metadata requires a supported HDR color contract. Observed {:?}, {:?}, {:?}, {:?}, {:?}.",
                video.pix_fmt,
                video.color_primaries,
                video.color_transfer,
                video.color_space,
                video.color_range
            )));
        }
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
                || settings.subtitles.iter().any(|subtitle| {
                    subtitle.mode == media_core::SubtitleMode::BurnIn || subtitle.burn_in
                }))
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
                    | VideoEncoder::X265
            )
            && tone_map.is_none()
        {
            return Err(unsupported(
                "The selected encoder supports SDR output only. Select SVT-AV1, AOM, or standalone x265 for preserved HDR10, or enable explicit HDR/HLG-to-SDR tone mapping.",
            ));
        }
        let source_hdr10 = if is_hdr10 || tone_map.as_ref().is_some_and(|tone| tone.hlg || tone.dv5)
        {
            Some(Hdr10::build(
                &video
                    .side_data_list
                    .iter()
                    .filter(|value| {
                        !(settings.tone_map.is_some_and(|tone| tone.source_workflow)
                            && preserves_dolby_source_clock(video, settings)
                            && display_matrix(value))
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
                settings.hdr10_fallback
                    || tone_map.as_ref().is_some_and(|tone| {
                        tone.settings.hdr10_base_layer
                            || tone.dolby_rpu
                            || (tone.settings.source_workflow && tone.permits_original_clock())
                    }),
                tone_map.as_ref().is_some_and(|tone| tone.dv5),
            )?)
        } else {
            None
        };
        let authored_hdr = settings
            .hdr_metadata
            .as_ref()
            .map(hdr::authored)
            .transpose()?;
        if authored_hdr.is_some() && !is_hdr10 {
            return Err(unsupported(
                "Authored HDR metadata requires a BT.2020/PQ 10-bit HDR10 source/output. Static metadata cannot turn SDR pixels into HDR.",
            ));
        }
        let mut hdr10 = source_hdr10.clone();
        if let (Some(hdr), Some(metadata)) = (&mut hdr10, &settings.hdr_metadata) {
            hdr.imported_dolby = metadata.dolby_vision_rpu_path.is_some();
            hdr.imported_hdr10plus = metadata.hdr10_plus_json_path.is_some();
        }
        if let (Some(hdr), Some(authored)) = (&mut hdr10, &authored_hdr) {
            hdr.metadata.apply_authored(authored);
        }
        if hdr10.as_ref().is_some_and(|hdr| hdr.dolby.is_some())
            && video.codec_name.as_deref() != Some("hevc")
        {
            return Err(unsupported(
                "Dolby Vision HDR10 fallback supports HEVC profile 7/8 base layers only.",
            ));
        }
        let validation_side_data = video
            .side_data_list
            .iter()
            .filter(|value| {
                !(settings.tone_map.is_some_and(|tone| tone.source_workflow)
                    && preserves_dolby_source_clock(video, settings)
                    && display_matrix(value))
            })
            .cloned()
            .collect::<Vec<_>>();
        validate_side_data(&validation_side_data, hdr10.as_ref(), false)?;
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
                if (2..=16384).contains(&w)
                    && (2..=16384).contains(&h)
                    && w % 2 == 0
                    && h % 2 == 0 =>
            {
                (w, h)
            }
            _ => {
                return Err(unsupported(
                    "Encoding requires even dimensions between 2 and 16384 pixels.",
                ));
            }
        };
        let source_dolby_render = settings.tone_map.is_some_and(|tone| tone.source_workflow)
            && preserves_dolby_source_clock(video, settings);
        let rotation = if source_dolby_render {
            video
                .side_data_list
                .iter()
                .find_map(|value| value.get("rotation").and_then(serde_json::Value::as_f64))
                .unwrap_or(0.0)
        } else {
            0.0
        };
        let source_rotation = rotation.round() as i32;
        if !rotation.is_finite()
            || (rotation - f64::from(source_rotation)).abs() > 0.001
            || source_rotation % 90 != 0
        {
            return Err(unsupported(
                "Intact Dolby rendering supports right-angle display rotations only.",
            ));
        }
        let source_coded_size = (width, height);
        let (width, height) = if source_rotation.rem_euclid(180) == 90 {
            (height, width)
        } else {
            (width, height)
        };
        let (sar_n, sar_d) = source_sar.split_once(':').expect("validated SAR");
        let mut geometry = framing::Geometry::build_with_sar(
            width,
            height,
            settings.framing,
            (
                sar_n.parse().expect("SAR numerator"),
                sar_d.parse().expect("SAR denominator"),
            ),
        )?;
        geometry.resize_filter = settings
            .temporal
            .map_or_default(|temporal| temporal.resize_filter);
        let aspect = settings
            .temporal
            .and_then(|temporal| temporal.aspect_ratio)
            .map(|aspect| aspect::Transform::build(aspect, geometry.width, geometry.height))
            .transpose()?;
        let output_sar = aspect.map_or_else(
            || {
                if settings.framing.resize.is_some() || settings.framing.resize_width.is_some() {
                    "1:1".into()
                } else {
                    source_sar.clone()
                }
            },
            aspect::Transform::sar,
        );
        let (fps_num, fps_den) = rational(video.avg_frame_rate.as_deref())
            .or_else(|| rational(video.r_frame_rate.as_deref()))
            .ok_or_else(|| unsupported("The source has no usable rational frame rate."))?;
        let fps = f64::from(fps_num) / f64::from(fps_den);
        if !(0.001..=1000.0).contains(&fps) {
            return Err(unsupported(
                "Encoding supports positive constant frame rates from 0.001 through 1000 fps; the selected encoder may impose a narrower limit.",
            ));
        }
        let tick = rational(video.time_base.as_deref())
            .map(|(n, d)| f64::from(n) / f64::from(d))
            .unwrap_or(0.000001);
        let tolerance = tick.clamp(0.000001, 0.001) + 0.000002;
        let declared_origin = seconds(video.start_time.as_deref())
            .filter(|value| value.is_finite())
            .unwrap_or(0.0);
        if !seconds(video.start_time.as_deref()).is_some_and(|value| value.abs() <= tolerance)
            && !preserves_dolby_source_clock(video, settings)
            && !source_av1an_base_clock(video, settings)
        {
            return Err(unsupported(
                "Encoding requires video timelines that start at zero. Timestamp offset handling requires canonical source preparation.",
            ));
        }
        let conventional_sdr = if rgb_source
            || video.height.is_some_and(|height| height > 576)
            || video.width.is_some_and(|width| width >= 1280)
        {
            1
        } else if video.height == Some(576) {
            5
        } else {
            6
        };
        let fallback_sdr = [
            video.color_space.as_deref(),
            video.color_primaries.as_deref(),
            video.color_transfer.as_deref(),
        ]
        .into_iter()
        .find_map(|value| sdr_color(value).ok())
        .unwrap_or(conventional_sdr);
        Ok(Self {
            output_origin: 0.0,
            source_origin: declared_origin,
            source_cut_ordinals: false,
            av1an_presented_rate: None,
            source_dolby_clock: declared_origin.abs() > tolerance
                && preserves_dolby_source_clock(video, settings),
            source_dolby_render,
            source_rotation,
            source_coded_size,
            source_cut_transform: None,
            source_av1an_base_clock: source_av1an_base_clock(video, settings),
            source_trim: settings.source_trim,
            custom_filters: super::av1an::filter_rows(settings),
            source_filters: settings.source_filters,
            grain_prefilter: settings
                .av1an_grain
                .as_ref()
                .filter(|grain| settings.encoder.is_svt() && grain.table.is_some() && grain.denoise)
                .map(|grain| grain.denoise_strength),
            prepare_metric_reference: settings
                .av1an_options
                .filter(|options| !options.source_target_defaults)
                .and_then(|options| options.target_quality)
                .is_some_and(|target| target.metric != media_core::Av1anTargetMetric::Vmaf),
            temporal: settings.temporal.map(|mut value| {
                if settings
                    .av1an_options
                    .is_some_and(|options| options.source_workflow)
                    && !super::av1an::source_resamples(settings, (fps_num, fps_den))
                {
                    value.frame_rate = None;
                }
                temporal::Transform::new(value)
            }),
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
            source_chunk_rate: (fps_num, fps_den),
            source_av1an_input_rate: (fps_num, fps_den),
            source_av1an_workflow: settings.backend == EncodeBackend::Av1an
                && settings
                    .av1an_options
                    .is_some_and(|options| options.source_workflow),
            source_queue: None,
            basic_source_queue: false,
            cadence_reconciled: false,
            tolerance,
            primaries: if tone_map.is_some() {
                1
            } else if is_hdr10 {
                9
            } else {
                source_sdr_color(video.color_primaries.as_deref(), fallback_sdr)?
            },
            transfer: if tone_map.is_some() {
                1
            } else if is_hdr10 {
                16
            } else {
                source_sdr_color(video.color_transfer.as_deref(), fallback_sdr)?
            },
            matrix: if tone_map.is_some() {
                1
            } else if is_hdr10 {
                9
            } else if rgb_source {
                1 // The encoded YUV uses BT.709; RGB input carries GBR identity.
            } else {
                source_sdr_color(video.color_space.as_deref(), fallback_sdr)?
            },
            hdr10,
            source_hdr10,
            authored_hdr,
            source_full_range,
            full_range: if tone_map.is_some() || settings.encoder.is_svt() {
                false
            } else {
                source_full_range
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
                    None | Some("unspecified" | "unknown") => "left",
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

    pub fn source_interpretation_notes(&self, source: &Stream) -> Vec<String> {
        let mut notes = Vec::new();
        if [
            source.color_primaries.as_deref(),
            source.color_transfer.as_deref(),
            source.color_space.as_deref(),
            source.color_range.as_deref(),
        ]
        .into_iter()
        .any(unspecified)
        {
            notes.push(format!("Unspecified source SDR fields use conventional interpretation: primaries {}, transfer {}, matrix {}, range {}. Explicit source tags remain authoritative; every decoded frame is still checked against the original stream metadata.",color_name(self.primaries),color_name(self.transfer),matrix_name(self.matrix),if self.source_full_range {"full"} else {"limited"}));
        }
        if self.source_full_range != self.full_range {
            notes.push("The selected encoder requires limited-range YUV; the verified decoder converts full-range source samples before encoding.".into());
        }
        notes
    }

    pub fn output_sar(&self) -> &str {
        &self.output_sar
    }
    pub fn encoded_mux_origin(&self) -> f64 {
        if self.source_dolby_clock && !self.source_dolby_render && self.trim.is_none() {
            self.source_origin
        } else {
            self.output_origin
        }
    }
    pub fn restores_source_dolby_clock(&self) -> bool {
        self.source_dolby_clock && !self.source_dolby_render && self.trim.is_none()
    }
    pub fn has_source_filters(&self) -> bool {
        self.source_filters
    }
    pub fn source_chunk_rate(&self) -> (u32, u32) {
        self.source_chunk_rate
    }
    pub(in crate::jobs) fn source_av1an_input_rate(&self) -> (u32, u32) {
        self.source_av1an_input_rate
    }
    pub(in crate::jobs) fn source_queue_regions(
        &mut self,
        regions: std::sync::Arc<super::source_queue::Regions>,
        rate: (u32, u32),
    ) {
        (self.width, self.height) = regions.first_dimensions();
        self.output_sar = regions.first_sar().into();
        (self.fps_num, self.fps_den) = regions.reference_rate().unwrap_or(rate);
        self.source_queue = Some(regions);
    }
    pub(in crate::jobs) fn source_queue_frame_count(&self) -> Option<usize> {
        self.source_queue
            .as_ref()
            .map(|regions| usize::try_from(regions.frames()).expect("validated source queue size"))
    }
    pub(in crate::jobs) fn has_source_queue(&self) -> bool {
        self.source_queue.is_some()
    }
    pub(super) fn has_basic_source_queue(&self) -> bool {
        self.basic_source_queue
    }
    pub(super) fn basic_source_video_check(&self) -> Option<super::output_counts::VideoCheck> {
        self.basic_source_queue
            .then(|| super::output_counts::VideoCheck {
                codec: self.output_codec(),
                dimensions: (self.width, self.height),
                frame_seconds: (!self.source_filters).then(|| self.frame_seconds()),
            })
    }
    pub(super) fn requires_source_queue_clock_proof(&self) -> bool {
        self.source_av1an_base_clock
    }
    pub(super) fn observe_source_queue_header(
        &mut self,
        stream: &Stream,
        queue_rate: (u32, u32),
    ) -> Result<(), AppError> {
        if self.source_filters {
            self.observe_source_filter_stream(stream, None)?;
        } else {
            if stream.codec_name.as_deref() != Some(self.output_codec())
                || !self.matches_output_format(stream.pix_fmt.as_deref())
                || (stream.width, stream.height) != (Some(self.width), Some(self.height))
            {
                return Err(AppError::new(
                    "ENCODE_VALIDATION_FAILED",
                    "The joined AV1AN video does not match the planned codec, pixel format or dimensions.",
                    None,
                ));
            }
            self.output_sar = aspect::parse_source(stream.sample_aspect_ratio.as_deref())?;
        }
        (self.fps_num, self.fps_den) =
            super::source_queue::stream_rate(stream).unwrap_or(queue_rate);
        self.basic_source_queue = true;
        Ok(())
    }
    pub fn without_source_filter_rows(&self) -> Self {
        let mut plan = self.clone();
        plan.custom_filters.clear();
        plan
    }
    pub fn source_filter_rows_only(&self) -> Option<String> {
        (!self.custom_filters.is_empty()).then(|| self.custom_filters.join(","))
    }

    pub fn source_filter_document(&self, mut document: Document, frames: usize) -> Document {
        if (self.source_filters || self.source_queue.is_some() || self.source_av1an_base_clock)
            && let Some(video) = document
                .streams
                .iter_mut()
                .find(|stream| stream.index == self.video_index)
        {
            // Raw source filters can change frame count or nominal Y4M rate.
            // Count/geometry/clock have already been proved against the actual
            // coded checkpoint; copied non-video tracks retain their own ends.
            video.duration = Some(
                self.source_queue
                    .as_ref()
                    .and_then(|regions| regions.reference_duration())
                    .unwrap_or(frames as f64 * self.frame_seconds())
                    .to_string(),
            );
            video.avg_frame_rate = Some(format!("{}/{}", self.fps_num, self.fps_den));
            video.r_frame_rate = video.avg_frame_rate.clone();
            video.sample_aspect_ratio = Some(self.output_sar.clone());
            if (self.source_dolby_render && self.trim.is_none()) || self.source_av1an_base_clock {
                // Source Y4M's default CFR pads a positive first-frame gap;
                // the separately wrapped coded track consequently starts at0.
                // Copied audio/subtitle clocks retain their original authority.
                video.start_time = Some("0".into());
                video.packet_start_time = Some(0.0);
            }
            video
                .tags
                .retain(|key, _| !super::metadata::is_derived_stream_tag(key));
        }
        document
    }
    pub fn source_filter_contract(&self, frames: usize) -> super::source_filters::OutputContract {
        super::source_filters::OutputContract {
            width: self.width,
            height: self.height,
            sar: self.output_sar.clone(),
            numerator: self.fps_num,
            denominator: self.fps_den,
            frames: frames as u64,
        }
    }
    pub fn apply_source_filter_contract(
        &mut self,
        contract: &super::source_filters::OutputContract,
    ) -> Result<(), AppError> {
        if !self.source_filters
            || contract.width == 0
            || contract.height == 0
            || contract.frames == 0
            || contract.numerator == 0
            || contract.denominator == 0
        {
            return Err(unsupported(
                "Invalid observed source-filter output contract.",
            ));
        }
        let sar = aspect::parse_source(Some(&contract.sar))?;
        self.width = contract.width;
        self.height = contract.height;
        self.output_sar = sar;
        self.fps_num = contract.numerator;
        self.fps_den = contract.denominator;
        Ok(())
    }
    pub fn observe_source_filter_reference(
        &mut self,
        stream: &Stream,
        clock: &super::source_filters::ProducerClock,
    ) -> Result<(), AppError> {
        if !self.source_filters
            || stream.codec_name.as_deref() != Some("ffv1")
            || !self.matches_output_format(stream.pix_fmt.as_deref())
        {
            return Err(unsupported(
                "The source filter reference must remain a lossless FFV1 video in the selected pixel format.",
            ));
        }
        self.width = stream
            .width
            .filter(|value| *value > 0)
            .ok_or_else(|| unsupported("The filtered reference has no valid width."))?;
        self.height = stream
            .height
            .filter(|value| *value > 0)
            .ok_or_else(|| unsupported("The filtered reference has no valid height."))?;
        self.output_sar = aspect::parse_source(stream.sample_aspect_ratio.as_deref())?;
        self.fps_num = clock.time_base.1;
        self.fps_den = clock.time_base.0;
        Ok(())
    }
    pub fn observe_source_filter_stream(
        &mut self,
        stream: &Stream,
        producer: Option<&super::source_filters::ProducerClock>,
    ) -> Result<(), AppError> {
        if !self.source_filters
            || stream.codec_name.as_deref() != Some(self.output_codec())
            || !self.matches_output_format(stream.pix_fmt.as_deref())
        {
            return Err(unsupported(
                "The source filter output changed the selected codec or pixel format/depth.",
            ));
        }
        let (width, height) = match (stream.width, stream.height) {
            (Some(width), Some(height)) if width > 0 && height > 0 => (width, height),
            _ => {
                return Err(unsupported(
                    "The source filter output has no valid geometry.",
                ));
            }
        };
        self.width = width;
        self.height = height;
        self.output_sar = aspect::parse_source(stream.sample_aspect_ratio.as_deref())?;
        if let Some(producer) = producer
            && (self.encoder.is_svt()
                || matches!(
                    self.encoder,
                    VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone
                ))
        {
            self.fps_num = producer.time_base.1;
            self.fps_den = producer.time_base.0;
        }
        Ok(())
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

    /// Deinterlacing and cadence repair accept interlaced or telecined
    /// sources, whose fields may be stored as separate packets.
    pub(in crate::jobs) fn accepts_field_coded_source(&self) -> bool {
        self.temporal.as_ref().is_some_and(|transform| {
            transform.settings.deinterlace.is_some()
                || transform.settings.qtgmc.is_some()
                || transform.settings.cadence_repair.is_some()
        })
    }

    /// av1an derives its scene/chunk frame count before applying `--ffmpeg`.
    /// Timeline-changing filters therefore run once into a verified lossless
    /// source which all scene, chunk and quality-reference readers share.
    pub fn requires_av1an_preprocess(&self) -> bool {
        if self.source_av1an_workflow {
            return false;
        }
        if self.source_filters {
            return self.trim.is_some();
        }
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
            filters.push(format!(
                "setsar={}:max=65535",
                self.output_sar.replace(':', "/")
            ));
        }
        filters.extend(self.custom_filters.iter().cloned());
        (!filters.is_empty()).then(|| filters.join(","))
    }

    /// AV1AN's original FFmpeg source reader already applies its display matrix.
    /// The second filtering pipe must not apply that orientation a second time.
    pub fn av1an_decoder_filter(&self) -> Option<String> {
        let mut reader = self.clone();
        reader.source_cut_transform = None;
        reader.decoder_filter()
    }

    pub(super) fn av1an_presented_filter(&self, rate: (u32, u32)) -> String {
        let transform = self.source_cut_transform.unwrap_or_default();
        format!("{transform}settb=expr={}/{},setpts=N", rate.1, rate.0)
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
        } else if self.output_sar != "1:1" {
            filters.push(format!(
                "setsar={}:max=65535",
                self.output_sar.replace(':', "/")
            ));
        }
        filters.extend(self.custom_filters.iter().cloned());
        (!filters.is_empty()).then(|| filters.join(","))
    }

    pub(super) fn validate_video_side_data(
        &self,
        values: &[serde_json::Value],
        encoded: bool,
    ) -> Result<(), AppError> {
        if !encoded && let Some(expected) = self.source_cut_transform {
            if values.iter().any(display_matrix) {
                let (_, actual) =
                    crate::analysis::display_transform_parts(values, &Default::default())
                        .map_err(unsupported)?;
                if actual != expected {
                    return Err(unsupported(
                        "The verified copied-cut display transform changed during decoding.",
                    ));
                }
            }
            let remaining = values
                .iter()
                .filter(|value| !display_matrix(value))
                .cloned()
                .collect::<Vec<_>>();
            validate_side_data(&remaining, self.validation_hdr(encoded), encoded)
        } else if !encoded && self.source_dolby_render && self.source_rotation != 0 {
            for value in values.iter().filter(|value| display_matrix(value)) {
                if value
                    .get("rotation")
                    .and_then(serde_json::Value::as_f64)
                    .is_none_or(|rotation| {
                        (rotation - f64::from(self.source_rotation)).abs() > 0.001
                    })
                {
                    return Err(unsupported(
                        "The original Dolby display rotation changed during decoding.",
                    ));
                }
            }
            let remaining = values
                .iter()
                .filter(|value| !display_matrix(value))
                .cloned()
                .collect::<Vec<_>>();
            validate_side_data(&remaining, self.validation_hdr(encoded), encoded)
        } else {
            validate_side_data(values, self.validation_hdr(encoded), encoded)
        }
    }

    pub(super) fn uses_source_output_trim(&self) -> bool {
        self.source_dolby_render || (self.source_trim && self.source_filters)
    }

    pub(super) fn source_output_trim(&self) -> Vec<std::ffi::OsString> {
        if !self.uses_source_output_trim() {
            return Vec::new();
        }
        source_trim_arguments(
            self.trim,
            self.source_chunk_rate,
            source_keeps_frame_count(
                self.temporal.as_ref().map(|value| value.settings),
                self.source_chunk_rate,
            ),
        )
    }

    pub fn decoder_prefix(&self) -> Option<String> {
        let mut filters = Vec::new();
        if let Some(transform) = self.source_cut_transform.filter(|value| !value.is_empty()) {
            filters.push(transform.trim_end_matches(',').to_owned());
        }
        if self.source_dolby_render {
            match self.source_rotation.rem_euclid(360) {
                90 => filters.push("transpose=cclock".into()),
                180 => filters.push("hflip,vflip".into()),
                270 => filters.push("transpose=clock".into()),
                _ => {}
            }
        }
        if let Some(strength) = self.grain_prefilter {
            filters.push(format!(
                "hqdn3d={strength}:{strength}:{}:{}",
                u16::from(strength) * 3 / 2,
                u16::from(strength) * 3 / 2
            ));
        }
        if !self.uses_source_output_trim()
            && let Some(trim) = self.trim
        {
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
        if !self.source_dolby_render
            && let Some(temporal) = &self.temporal
            && let Some(filter) = temporal.filter()
        {
            filters.push(filter);
        }
        if let Some(tone) = &self.tone_map {
            filters.push(tone.filter());
        }
        if self.source_dolby_render
            && let Some(rate) = self
                .temporal
                .as_ref()
                .and_then(|temporal| temporal.settings.frame_rate)
        {
            filters.push(format!("fps=fps={}/{}", rate.numerator, rate.denominator));
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
        if input == self.output_pixel_format && self.source_full_range == self.full_range {
            return None;
        }
        let matrix = match self.matrix {
            1 => "bt709",
            5 => "bt470bg",
            6 => "smpte170m",
            9 => "bt2020",
            _ => unreachable!("validated color matrix"),
        };
        let input_range = if rgb_source_format(self.source_pixel_format) || self.source_full_range {
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

    pub fn requires_vulkan_tone_map(&self) -> bool {
        self.tone_map
            .as_ref()
            .is_some_and(tone_map::Transform::requires_vulkan)
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
        if encoded {
            if self.is_tone_mapped() {
                None
            } else {
                self.hdr10.as_ref()
            }
        } else {
            self.source_hdr10.as_ref()
        }
    }
    fn absorb_source_metadata(&mut self, first: &Frame) -> Result<(), AppError> {
        let required = self.needs_mastering_metadata();
        let imported_dolby = self.hdr10.as_ref().is_some_and(|hdr| hdr.imported_dolby);
        let imported_hdr10plus = self
            .hdr10
            .as_ref()
            .is_some_and(|hdr| hdr.imported_hdr10plus);
        if let Some(source) = &mut self.source_hdr10 {
            source
                .metadata
                .absorb_first_frame(&StaticMetadata::parse(&first.side_data_list)?)?;
            let mut output = source.clone();
            output.imported_dolby = imported_dolby;
            output.imported_hdr10plus = imported_hdr10plus;
            if let Some(authored) = &self.authored_hdr {
                output.metadata.apply_authored(authored);
            }
            if required && output.metadata.mastering.is_none() {
                return Err(unsupported(
                    "HDR10 output requires mastering display metadata in the source or an authored mastering-display override.",
                ));
            }
            self.hdr10 = Some(output);
        }
        Ok(())
    }

    fn needs_mastering_metadata(&self) -> bool {
        // Preserving HDR10 output requires mastering metadata. Explicit SDR
        // rendering uses the supplied signal peak; any metadata present is
        // still parsed and checked across the complete original source.
        self.tone_map.is_none()
    }

    pub(super) fn frame_dimensions(&self, encoded: bool) -> (u32, u32) {
        if encoded {
            (self.width, self.height)
        } else {
            if self.source_dolby_render || self.source_cut_transform.is_some() {
                self.source_coded_size
            } else {
                (self.geometry.source_width, self.geometry.source_height)
            }
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

    pub fn ffmpeg_x265_hdr_suffix(&self) -> String {
        let args = self.x265_hdr_arguments();
        let mut fields = Vec::new();
        let mut index = 0;
        while index < args.len() {
            let name = args[index]
                .to_str()
                .expect("HDR option")
                .trim_start_matches('-');
            if matches!(name, "hdr10" | "repeat-headers") {
                fields.push(format!(":{name}=1"));
                index += 1;
            } else {
                fields.push(format!(":{name}={}", args[index + 1].to_string_lossy()));
                index += 2;
            }
        }
        fields.join("")
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
        if self.source_hdr10.is_some() {
            let first = frames
                .frames
                .first()
                .ok_or_else(|| unsupported("The video did not decode to any frames."))?;
            self.absorb_source_metadata(first)?;
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
        self.validate_video_side_data(&stream.side_data_list, encoded)?;
        let mut previous_time = None;
        for (index, frame) in frames.frames.iter().enumerate() {
            let time = seconds(frame.best_effort_timestamp_time.as_deref())
                .ok_or_else(|| unsupported("A decoded frame has no usable timestamp."))?;
            if !encoded
                && (self.source_cut_ordinals
                    || self.source_dolby_render
                    || self.source_av1an_base_clock)
            {
                if previous_time.is_some_and(|previous| time <= previous) {
                    return Err(unsupported(
                        "The decoded source timestamps must be strictly increasing.",
                    ));
                }
                previous_time = Some(time);
            } else if (time
                - if encoded {
                    self.output_origin
                } else {
                    self.source_origin
                }
                - index as f64 * self.frame_seconds())
            .abs()
                > tolerance
            {
                return Err(unsupported(
                    "Decoded frame timestamps are not constant-rate starting at zero. VFR and timestamp gaps require a later workflow.",
                ));
            }
            let (width, height) = self.frame_dimensions(encoded);
            self.validate_fields(frame, encoded)?;
            if frame.width != Some(width)
                || frame.height != Some(height)
                || frame
                    .sample_aspect_ratio
                    .as_deref()
                    .filter(|value| !matches!(*value, "0:1" | "N/A"))
                    .unwrap_or("1:1")
                    != self.frame_sar(encoded)
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
            self.validate_video_side_data(&frame.side_data_list, encoded)?;
            if !encoded && let Some(hdr) = self.hdr10.as_ref() {
                hdr.require_profile5_rpu(&frame.side_data_list)?;
                if self.tone_map.as_ref().is_some_and(|tone| tone.dolby_rpu) {
                    hdr.require_gpu_rpu(&frame.side_data_list)?;
                }
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

    /// ISO VP9 does not carry Matroska's chroma-siting declaration. This is
    /// usable only after the caller proves every copied video packet against
    /// the fully validated encoder checkpoint. All explicit declarations and
    /// all remaining planned fields must still match.
    pub fn validate_packet_copied_iso_vp9(
        &self,
        source: &Stream,
        checkpoint: &Stream,
        output: &Stream,
        format: media_core::ContainerFormat,
    ) -> Result<(), AppError> {
        if self.output_codec() != "vp9"
            || !matches!(
                format,
                media_core::ContainerFormat::Mp4 | media_core::ContainerFormat::Mov
            )
        {
            return self.validate_encoded_stream(source, output);
        }
        self.validate_elementary_checkpoint(source, checkpoint)?;
        let mut declared = output.clone();
        if declared.chroma_location.is_none() {
            declared.chroma_location = Some(self.chroma.into());
        }
        self.validate_encoded_stream(source, &declared)
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
        if self.encoder == VideoEncoder::X264 && self.output_sar != "1:1" {
            // x264's native Matroska writer truncates display dimensions to
            // integral pixels. The common mux restores the exact rational
            // aspect; only this owned intermediate admits that known value.
            let (n, d) = self.output_sar.split_once(':').expect("validated SAR");
            let (n, d) = (
                n.parse::<u64>().expect("SAR numerator"),
                d.parse::<u64>().expect("SAR denominator"),
            );
            let (mut numerator, mut denominator) = if n >= d {
                (u64::from(self.width) * n / d, u64::from(self.width))
            } else {
                (u64::from(self.height), u64::from(self.height) * d / n)
            };
            let (mut a, mut b) = (numerator, denominator);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            numerator /= a;
            denominator /= a;
            let mut intermediate = self.clone();
            intermediate.output_sar = format!("{numerator}:{denominator}");
            if output.sample_aspect_ratio.as_deref() == Some(intermediate.output_sar.as_str()) {
                return intermediate.validate_encoded_stream_fields(source, output, false);
            }
        }
        self.validate_encoded_stream_fields(source, output, self.output_codec() == "vp9")
    }

    fn validate_encoded_stream_fields(
        &self,
        _source: &Stream,
        output: &Stream,
        allow_absent_vp9_container_fields: bool,
    ) -> Result<(), AppError> {
        if let Some(regions) = &self.source_queue {
            return regions.verify_header(output, allow_absent_vp9_container_fields);
        }
        let expected_primaries = Some(color_name(self.primaries));
        let expected_transfer = Some(color_name(self.transfer));
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
                    output.color_space.as_deref() != Some(matrix_name(self.matrix))
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
                format!(
                    "The encoded output codec, dimensions, bit depth, aspect ratio, or color metadata differ from the plan. Observed codec {:?}, format {:?}, size {:?}x{:?}, SAR {:?}, primaries {:?}, transfer {:?}, matrix {:?}, range {:?}, chroma {:?}; expected {}, {}, {}x{}, SAR {}, primaries {}, transfer {}, matrix {}, range {}, chroma {}.",
                    output.codec_name,
                    output.pix_fmt,
                    output.width,
                    output.height,
                    output.sample_aspect_ratio,
                    output.color_primaries,
                    output.color_transfer,
                    output.color_space,
                    output.color_range,
                    output.chroma_location,
                    self.output_codec(),
                    self.output_pixel_format,
                    self.width,
                    self.height,
                    self.output_sar,
                    color_name(self.primaries),
                    color_name(self.transfer),
                    matrix_name(self.matrix),
                    if self.full_range { "pc" } else { "tv" },
                    self.chroma
                ),
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
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

impl Frame {
    /// A presentation time without decoded properties. Only the packet-timing
    /// validator accepts it, after its first frame was fully decoded.
    pub(in crate::jobs) fn timestamp_only(time: String) -> Self {
        Self {
            best_effort_timestamp_time: Some(time),
            interlaced_frame: None,
            top_field_first: None,
            width: None,
            height: None,
            pix_fmt: None,
            sample_aspect_ratio: None,
            chroma_location: None,
            color_space: None,
            color_transfer: None,
            color_primaries: None,
            color_range: None,
            side_data_list: Vec::new(),
        }
    }

    pub(in crate::jobs) fn timestamp(&self) -> Option<&str> {
        self.best_effort_timestamp_time.as_deref()
    }

    pub(in crate::jobs) fn source_queue_shape(&self) -> Self {
        let mut frame = self.clone();
        frame.best_effort_timestamp_time = None;
        if matches!(
            frame.sample_aspect_ratio.as_deref(),
            None | Some("0:1" | "N/A")
        ) {
            frame.sample_aspect_ratio = Some("1:1".into());
        }
        for value in [
            &mut frame.chroma_location,
            &mut frame.color_space,
            &mut frame.color_transfer,
            &mut frame.color_primaries,
            &mut frame.color_range,
        ] {
            if matches!(value.as_deref(), Some("unknown" | "unspecified" | "N/A")) {
                *value = None;
            }
        }
        frame
    }
    pub(in crate::jobs) fn source_queue_dimensions(&self) -> Option<(u32, u32)> {
        self.width
            .zip(self.height)
            .filter(|(w, h)| *w > 0 && *h > 0)
    }
    pub(in crate::jobs) fn source_queue_time(&self) -> Option<f64> {
        self.best_effort_timestamp_time
            .as_deref()?
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_dolby_origin_keeps_strict_cfr_and_does_not_enable_trim_or_ordinal_clocks() {
        let document:Document=serde_json::from_value(serde_json::json!({"streams":[{
            "index":0,"codec_type":"video","codec_name":"hevc","width":128,"height":96,
            "pix_fmt":"yuv420p10le","field_order":"progressive","sample_aspect_ratio":"1:1",
            "avg_frame_rate":"4/1","time_base":"1/1000","start_time":"0.05",
            "color_primaries":"bt2020","color_transfer":"smpte2084","color_space":"bt2020nc",
            "color_range":"tv","chroma_location":"topleft","side_data_list":[{
                "side_data_type":"DOVI configuration record","dv_profile":8,"dv_bl_signal_compatibility_id":1,
                "bl_present_flag":1,"rpu_present_flag":1,"el_present_flag":0
            }]}]})).unwrap();
        let settings=EncodeSettings {encoder:VideoEncoder::X264,tone_map:Some(serde_json::from_value(serde_json::json!({"sourcePeakNits":4000,"backend":"auto","peakMode":"measured","algorithm":"spline"})).unwrap()),..Default::default()};
        let selected = document.selected(&[0]).unwrap();
        let stream = selected[0];
        let mut plan = Plan::build(&document, &selected, &settings).unwrap();
        assert_eq!(plan.source_origin, 0.05);
        assert_eq!(plan.encoded_mux_origin(), 0.05);
        assert!(!plan.source_cut_ordinals);
        let frames:Frames=serde_json::from_value(serde_json::json!({"frames":(0..4).map(|index|serde_json::json!({
            "best_effort_timestamp_time":(0.05+f64::from(index)/4.0).to_string(),"interlaced_frame":0,"top_field_first":0,
            "width":128,"height":96,"pix_fmt":"yuv420p10le","sample_aspect_ratio":"1:1","chroma_location":"topleft",
            "color_primaries":"bt2020","color_transfer":"smpte2084","color_space":"bt2020nc","color_range":"tv",
            "side_data_list":[{"side_data_type":"Dolby Vision RPU Data"},{"side_data_type":"Dolby Vision Metadata","bl_bit_depth":10,"bl_video_full_range_flag":0,"disable_residual_flag":1}]
        })).collect::<Vec<_>>()})).unwrap();
        assert_eq!(plan.validate_source_frames(&frames, stream).unwrap(), 4);
        let mut variable = Frames {
            frames: frames.frames.clone(),
        };
        variable.frames[2].best_effort_timestamp_time = Some("0.555".into());
        assert!(plan.validate_source_frames(&variable, stream).is_err());
        let mut missing = Frames {
            frames: frames.frames.clone(),
        };
        missing.frames[1].side_data_list.clear();
        assert!(plan.validate_source_frames(&missing, stream).is_err());
        let mut trimmed = settings.clone();
        trimmed.trim = Some(
            serde_json::from_value(serde_json::json!({"startFrame":0,"endFrameExclusive":4}))
                .unwrap(),
        );
        assert!(!preserves_dolby_source_clock(stream, &trimmed));
        assert!(Plan::build(&document, &selected, &trimmed).is_err());
        let source_cpu=EncodeSettings{backend:EncodeBackend::Av1an,encoder:VideoEncoder::X264,
            av1an_options:Some(media_core::Av1anOptions{source_workflow:true,..Default::default()}),
            tone_map:Some(serde_json::from_value(serde_json::json!({"sourcePeakNits":4000,"backend":"auto","peakMode":"measured","algorithm":"hable","sourceWorkflow":true})).unwrap()),..Default::default()};
        let mut source_plan = Plan::build(&document, &selected, &source_cpu).unwrap();
        assert!(source_plan.source_av1an_base_clock);
        assert!(!source_plan.source_cut_ordinals);
        assert_eq!(source_plan.encoded_mux_origin(), 0.0);
        assert_eq!(
            source_plan
                .validate_source_frames(&variable, stream)
                .unwrap(),
            4
        );
        variable.frames[2].best_effort_timestamp_time =
            variable.frames[1].best_effort_timestamp_time.clone();
        assert!(
            source_plan
                .validate_source_frames(&variable, stream)
                .is_err()
        );
        let mut legacy_cpu = source_cpu.clone();
        legacy_cpu.av1an_options.as_mut().unwrap().source_workflow = false;
        assert!(Plan::build(&document, &selected, &legacy_cpu).is_err());
    }

    #[test]
    fn source_settings_reach_all_encoder_gops_and_resize_filters() {
        let document = source();
        for encoder in [
            VideoEncoder::SvtAv1,
            VideoEncoder::X264,
            VideoEncoder::X265Standalone,
            VideoEncoder::AomAv1,
            VideoEncoder::VpxStandalone,
            VideoEncoder::X265,
            VideoEncoder::Vp9,
        ] {
            let settings = EncodeSettings {
                encoder,
                preset: 0,
                encoding_preferences: Some(media_core::EncodingPreferences {
                    keyframe_interval_seconds: 1,
                    mp4_faststart: true,
                    full_output_verification: true,
                }),
                ..Default::default()
            };
            let plan =
                Plan::build(&document, &document.selected(&[0]).unwrap(), &settings).unwrap();
            let expected = super::super::encoding_preferences::keyframes(&plan, &settings).unwrap();
            let args = super::super::encode::consumer_arguments(&plan, &settings);
            let found = if matches!(encoder, VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone) {
                args.iter()
                    .any(|arg| arg == format!("--kf-max-dist={expected}").as_str())
            } else {
                args.windows(2).any(|pair| {
                    (pair[0] == "--keyint" || pair[0] == "-g")
                        && pair[1] == expected.to_string().as_str()
                })
            };
            assert!(found, "{encoder:?}: {args:?}");
        }
        for (filter, name) in [
            (media_core::ResizeFilter::Spline, "spline"),
            (media_core::ResizeFilter::Area, "area"),
        ] {
            let settings = EncodeSettings {
                framing: media_core::VideoFraming {
                    resize_width: Some(160),
                    ..Default::default()
                },
                temporal: Some(media_core::TemporalSettings {
                    resize_filter: filter,
                    ..Default::default()
                }),
                ..Default::default()
            };
            let plan =
                Plan::build(&document, &document.selected(&[0]).unwrap(), &settings).unwrap();
            assert!(
                plan.decoder_filter()
                    .unwrap()
                    .contains(&format!("flags={name}"))
            );
        }
        let settings = EncodeSettings {
            encoding_preferences: Some(media_core::EncodingPreferences {
                keyframe_interval_seconds: 1,
                mp4_faststart: false,
                full_output_verification: true,
            }),
            ..Default::default()
        };
        let mut plan =
            Plan::build(&document, &document.selected(&[0]).unwrap(), &settings).unwrap();
        plan.fps_num = 25;
        plan.fps_den = 2;
        assert_eq!(
            super::super::encoding_preferences::keyframes(&plan, &settings),
            Some(12),
            "source Math.Round uses ties-to-even"
        );
    }

    #[test]
    fn authored_hdr_output_does_not_replace_source_validation_metadata() {
        use media_core::{
            HdrChromaticity, HdrContentLight, HdrMasteringDisplay, HdrMetadataSettings,
        };
        let (source, decoded) = hdr_source_and_frames();
        let metadata = HdrMetadataSettings {
            mastering_display: Some(HdrMasteringDisplay {
                red: HdrChromaticity { x: 34000, y: 16000 },
                green: HdrChromaticity { x: 13250, y: 34500 },
                blue: HdrChromaticity { x: 7500, y: 3000 },
                white_point: HdrChromaticity { x: 15635, y: 16450 },
                max_luminance_ten_thousandths: 20_000_000,
                min_luminance_ten_thousandths: 1,
            }),
            content_light: Some(HdrContentLight {
                max_content: 500,
                max_average: 250,
            }),
            ..Default::default()
        };
        let mut plan = Plan::build(
            &source,
            &source.selected(&[0]).unwrap(),
            &EncodeSettings {
                hdr_metadata: Some(metadata.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            plan.validate_source_frames(&decoded, &source.streams[0])
                .unwrap(),
            3
        );
        assert_eq!(
            plan.source_hdr10.as_ref().unwrap().metadata.light,
            Some((200, 142))
        );
        assert_eq!(
            plan.hdr10.as_ref().unwrap().metadata.light,
            Some((500, 250))
        );
        assert!(plan.hdr_arguments().iter().any(|value| value == "500,250"));
        assert!(
            plan.x265_hdr_arguments()
                .iter()
                .any(|value| value.to_string_lossy().contains("L(20000000,1)"))
        );
        let mut invalid = metadata.clone();
        invalid
            .mastering_display
            .as_mut()
            .unwrap()
            .max_luminance_ten_thousandths = 0;
        assert!(
            validate_settings(&EncodeSettings {
                hdr_metadata: Some(invalid),
                ..Default::default()
            })
            .is_err()
        );
        let mut invalid = metadata.clone();
        invalid.content_light = Some(HdrContentLight {
            max_content: 100,
            max_average: 101,
        });
        assert!(
            validate_settings(&EncodeSettings {
                hdr_metadata: Some(invalid),
                ..Default::default()
            })
            .is_err()
        );
        let mut unknown = metadata;
        unknown.content_light = Some(HdrContentLight {
            max_content: 0,
            max_average: 0,
        });
        let mut plan = Plan::build(
            &source,
            &source.selected(&[0]).unwrap(),
            &EncodeSettings {
                hdr_metadata: Some(unknown),
                ..Default::default()
            },
        )
        .unwrap();
        plan.validate_source_frames(&decoded, &source.streams[0])
            .unwrap();
        assert!(plan.hdr10.as_ref().unwrap().metadata.light.is_none());
    }

    #[test]
    fn unspecified_sdr_tags_anamorphic_sar_small_frames_and_high_cadence_have_explicit_plans() {
        let mut untagged = source();
        let video = &mut untagged.streams[0];
        video.color_primaries = None;
        video.color_transfer = None;
        video.color_space = None;
        video.color_range = None;
        video.chroma_location = None;
        video.sample_aspect_ratio = None;
        let plan = Plan::build(
            &untagged,
            &untagged.selected(&[0]).unwrap(),
            &x264_settings(),
        )
        .unwrap();
        assert_eq!((plan.primaries, plan.transfer, plan.matrix), (6, 6, 6));
        assert!(!plan.full_range);
        assert_eq!(plan.chroma, "left");
        assert!(
            !plan
                .source_interpretation_notes(&untagged.streams[0])
                .is_empty()
        );
        let mut anamorphic = source();
        anamorphic.streams[0].sample_aspect_ratio = Some("4:3".into());
        let plan = Plan::build(
            &anamorphic,
            &anamorphic.selected(&[0]).unwrap(),
            &x264_settings(),
        )
        .unwrap();
        assert_eq!(plan.output_sar(), "4:3");
        assert!(plan.decoder_filter().unwrap().contains("setsar=4/3"));
        let mut small = source();
        small.streams[0].width = Some(32);
        small.streams[0].height = Some(24);
        small.streams[0].avg_frame_rate = Some("240/1".into());
        let plan = Plan::build(&small, &small.selected(&[0]).unwrap(), &x264_settings()).unwrap();
        assert_eq!((plan.width, plan.height, plan.fps_num), (32, 24, 240));
        let mut full = source();
        full.streams[0].color_range = Some("pc".into());
        let plan = Plan::build(
            &full,
            &full.selected(&[0]).unwrap(),
            &EncodeSettings::default(),
        )
        .unwrap();
        assert!(!plan.full_range);
        assert!(
            plan.decoder_filter()
                .unwrap()
                .contains("in_range=full:out_range=limited")
        );
    }

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
    fn copied_iso_vp9_allows_only_missing_chroma_after_verified_checkpoint() {
        use media_core::ContainerFormat;
        let mut source = source();
        source.streams[0].chroma_location = Some("left".into());
        let settings = EncodeSettings {
            encoder: VideoEncoder::Vp9,
            ..Default::default()
        };
        let plan = Plan::build(&source, &source.selected(&[0]).unwrap(), &settings).unwrap();
        let mut checkpoint = source.streams[0].clone();
        checkpoint.codec_name = Some("vp9".into());
        let mut output = checkpoint.clone();
        output.chroma_location = None;
        assert!(
            plan.validate_encoded_stream(&source.streams[0], &output)
                .is_err()
        );
        plan.validate_packet_copied_iso_vp9(
            &source.streams[0],
            &checkpoint,
            &output,
            ContainerFormat::Mp4,
        )
        .unwrap();
        assert!(
            plan.validate_packet_copied_iso_vp9(
                &source.streams[0],
                &checkpoint,
                &output,
                ContainerFormat::Matroska
            )
            .is_err()
        );
        for field in ["chroma", "pixel", "primaries", "codec", "sar"] {
            let mut changed = output.clone();
            match field {
                "chroma" => changed.chroma_location = Some("center".into()),
                "pixel" => changed.pix_fmt = Some("yuv420p10le".into()),
                "primaries" => changed.color_primaries = Some("bt470bg".into()),
                "codec" => changed.codec_name = Some("h264".into()),
                "sar" => changed.sample_aspect_ratio = Some("4:3".into()),
                _ => unreachable!(),
            }
            assert!(
                plan.validate_packet_copied_iso_vp9(
                    &source.streams[0],
                    &checkpoint,
                    &changed,
                    ContainerFormat::Mp4
                )
                .is_err(),
                "accepted {field}"
            );
        }
        checkpoint.chroma_location = Some("center".into());
        assert!(
            plan.validate_packet_copied_iso_vp9(
                &source.streams[0],
                &checkpoint,
                &output,
                ContainerFormat::Mp4
            )
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
                    resize: None,
                    border_ratio: None,
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
    fn verified_cut_orientation_keeps_coded_scan_and_display_contract_separate() {
        let mut document = source();
        let matrix = serde_json::json!({"side_data_type":"Display Matrix","displaymatrix":"\n00000000: 0 -65536 0\n00000001: 65536 0 0\n00000002: 0 0 1073741824\n","rotation":90});
        document.streams[0].side_data_list.push(matrix.clone());
        document.streams[0].sample_aspect_ratio = Some("4:3".into());
        let settings = EncodeSettings {
            backend: EncodeBackend::Av1an,
            encoder: VideoEncoder::X264,
            ..x264_settings()
        };
        let selected = document.selected(&[0]).unwrap();
        assert!(Plan::build(&document, &selected, &settings).is_err());
        let plan = Plan::build_source_cut(&document, &selected, &settings).unwrap();
        assert_eq!(plan.frame_dimensions(false), (128, 96));
        assert_eq!(plan.frame_dimensions(true), (96, 128));
        assert_eq!(plan.frame_sar(false), "4:3");
        assert_eq!(plan.output_sar(), "3:4");
        assert!(
            plan.decoder_filter()
                .unwrap()
                .starts_with("transpose=cclock")
        );
        assert!(
            !plan
                .av1an_decoder_filter()
                .unwrap_or_default()
                .contains("transpose")
        );
        assert!(
            plan.validate_video_side_data(std::slice::from_ref(&matrix), false)
                .is_ok()
        );
        assert!(
            plan.validate_video_side_data(std::slice::from_ref(&matrix), true)
                .is_err()
        );
        let mut changed = matrix;
        changed["displaymatrix"] = serde_json::json!(
            "\n00000000: 0 65536 0\n00000001: -65536 0 0\n00000002: 0 0 1073741824\n"
        );
        assert!(
            plan.validate_video_side_data(&[changed.clone()], false)
                .is_err()
        );
        changed["displaymatrix"] = serde_json::json!("malformed");
        document.streams[0].side_data_list = vec![changed];
        assert!(
            Plan::build_source_cut(&document, &document.selected(&[0]).unwrap(), &settings)
                .is_err()
        );
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
    fn x264_accepts_both_backends_and_unspecified_chroma_but_rejects_hdr() {
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
            .is_ok()
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
        for source_format in ["rgb24", "bgr24", "bgr0"] {
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

    #[test]
    fn verified_copied_cut_uses_ordinals_but_rejects_bad_source_clocks_and_fields() {
        let source = source();
        let mut plan = Plan::build(
            &source,
            &source.selected(&[0]).unwrap(),
            &EncodeSettings::default(),
        )
        .unwrap();
        let boundary = frames(&["0.083", "0.125", "0.166", "0.250"]);
        assert!(
            plan.validate_frames(&boundary, &source.streams[0], false)
                .is_err()
        );
        plan.source_cut_ordinals = true;
        plan.source_origin = 0.083;
        assert_eq!(
            plan.validate_frames(&boundary, &source.streams[0], false)
                .unwrap(),
            4
        );
        for times in [
            ["0.083", "0.083"],
            ["0.083", "0.042"],
            ["0.083", "NaN"],
            ["0.083", "inf"],
        ] {
            assert!(
                plan.validate_frames(&frames(&times), &source.streams[0], false)
                    .is_err(),
                "{times:?}"
            );
        }
        let mut changed = frames(&["0.083", "0.125", "0.166", "0.250"]);
        changed.frames[3].width = Some(64);
        assert!(
            plan.validate_frames(&changed, &source.streams[0], false)
                .is_err()
        );
        // The source-only policy cannot relax the actual output cadence.
        assert!(
            plan.validate_frames(&boundary, &source.streams[0], true)
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

    /// Packet timing plans the same count and frame clock as the complete
    /// decoded validator and rejects the same timing defects. Properties of
    /// later frames remain the decoded scan's job; the first frame is checked.
    #[test]
    fn packet_timing_matches_the_decoded_validator_for_cadence_and_count() {
        let source = source();
        let new_plan = || {
            Plan::build(
                &source,
                &source.selected(&[0]).unwrap(),
                &EncodeSettings::default(),
            )
            .unwrap()
        };
        let timing = |plan: &Plan, frames: &Frames| {
            let mut validation = Validation::timing(plan.clone(), source.streams[0].clone());
            validation.push(&frames.frames[0]);
            for frame in &frames.frames[1..] {
                validation.push(&Frame::timestamp_only(
                    frame.best_effort_timestamp_time.clone().unwrap(),
                ));
            }
            validation.finish()
        };
        for (num, den) in [(24000, 1001), (2997, 125)] {
            let plan = new_plan();
            let frames = long_cfr_frames(num, den);
            let (planned, count) = timing(&plan, &frames).unwrap();
            let mut decoded = plan.clone();
            assert_eq!(
                decoded
                    .validate_source_frames(&frames, &source.streams[0])
                    .unwrap(),
                count
            );
            assert_eq!(
                (
                    planned.fps_num,
                    planned.fps_den,
                    planned.cadence_reconciled,
                    planned.tolerance
                ),
                (
                    decoded.fps_num,
                    decoded.fps_den,
                    decoded.cadence_reconciled,
                    decoded.tolerance
                )
            );
        }
        for defect in ["gap", "duplicate", "offset"] {
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
                _ => frames.frames[0].best_effort_timestamp_time = Some("0.010".into()),
            }
            assert!(timing(&new_plan(), &frames).is_err(), "{defect}");
        }
        let mut later = long_cfr_frames(24000, 1001);
        later.frames[20_000].color_primaries = Some("bt2020".into());
        assert!(timing(&new_plan(), &later).is_ok());
        assert!(
            new_plan()
                .validate_source_frames(&later, &source.streams[0])
                .is_err()
        );
        let mut first = long_cfr_frames(24000, 1001);
        first.frames[0].color_primaries = Some("bt2020".into());
        assert!(timing(&new_plan(), &first).is_err());
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
            ("sample_aspect_ratio", "4:0"),
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
                crf: 64,
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
