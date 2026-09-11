use media_core::{AppError, EncodeBackend, EncodeSettings, VideoEncoder};
use serde::Deserialize;

use super::metadata::{Document, Stream};

mod hdr;
use hdr::{Hdr10, StaticMetadata, validate_side_data};
mod validation;
pub(super) use validation::Validation;

#[derive(Clone, Debug)]
pub(super) struct Plan {
    pub encoder: VideoEncoder,
    pub output_pixel_format: &'static str,
    pub video_index: u32,
    pub width: u32,
    pub height: u32,
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
    let valid = match settings.encoder {
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
            (1..=63).contains(&settings.crf) && settings.preset <= 13 && settings.film_grain <= 50
        }
        VideoEncoder::X264 => {
            settings.backend == EncodeBackend::Standalone
                && settings.crf <= 51
                && settings.preset <= 9
                && settings.film_grain == 0
                && !settings.hdr10_fallback
        }
    };
    if !valid || !(1..=32).contains(&settings.workers) {
        return Err(AppError::new(
            "ENCODE_SETTINGS_INVALID",
            match settings.encoder {
                VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
                    "Use CRF 1–63, an SVT preset from 0–13, film grain synthesis from 0–50, and 1–32 workers."
                }
                VideoEncoder::X264 => {
                    "Standalone x264 requires CRF 0–51, preset 0–9, grain 0, HDR10 fallback off, and 1–32 workers. av1an with x264 is not supported yet."
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

impl Plan {
    pub fn build(
        document: &Document,
        selected: &[&Stream],
        settings: &EncodeSettings,
    ) -> Result<Self, AppError> {
        validate_settings(settings)?;
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
        let full_range_8bit = settings.encoder == VideoEncoder::X264
            && video.pix_fmt.as_deref() == Some("yuvj420p")
            && video.color_range.as_deref() == Some("pc");
        if !matches!(video.pix_fmt.as_deref(), Some("yuv420p" | "yuv420p10le")) && !full_range_8bit
        {
            return Err(unsupported(
                "Encoding supports 8-bit or 10-bit planar 4:2:0 video only.",
            ));
        }
        if !matches!(
            video.field_order.as_deref(),
            None | Some("progressive" | "unknown")
        ) {
            return Err(unsupported(
                "Interlaced video is not supported by this progressive encoding workflow.",
            ));
        }
        if video.sample_aspect_ratio.as_deref() != Some("1:1") {
            return Err(unsupported(
                "Encoding requires explicit square pixels (sample aspect ratio 1:1).",
            ));
        }
        let is_hdr10 = video.color_primaries.as_deref() == Some("bt2020")
            && video.color_transfer.as_deref() == Some("smpte2084")
            && video.color_space.as_deref() == Some("bt2020nc")
            && video.pix_fmt.as_deref() == Some("yuv420p10le")
            && video.color_range.as_deref() == Some("tv");
        if is_hdr10 && settings.encoder == VideoEncoder::X264 {
            return Err(unsupported(
                "Standalone x264 currently supports SDR only. Select SVT-AV1 for HDR10 output; no automatic tone mapping or HDR metadata removal is performed.",
            ));
        }
        let hdr10 = if is_hdr10 {
            Some(Hdr10::build(
                &video.side_data_list,
                settings.hdr10_fallback,
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
            encoder: settings.encoder,
            output_pixel_format: if settings.encoder.is_svt()
                || video.pix_fmt.as_deref() == Some("yuv420p10le")
            {
                "yuv420p10le"
            } else {
                "yuv420p"
            },
            video_index: video.index,
            width,
            height,
            fps_num,
            fps_den,
            cadence_reconciled: false,
            tolerance,
            primaries: if is_hdr10 {
                9
            } else {
                sdr_color(video.color_primaries.as_deref())?
            },
            transfer: if is_hdr10 {
                16
            } else {
                sdr_color(video.color_transfer.as_deref())?
            },
            matrix: if is_hdr10 {
                9
            } else {
                sdr_color(video.color_space.as_deref())?
            },
            hdr10,
            full_range: match video.color_range.as_deref() {
                Some("tv") => false,
                Some("pc") => true,
                _ => {
                    return Err(unsupported(
                        "Explicit limited or full color range is required.",
                    ));
                }
            },
            chroma: match video.chroma_location.as_deref() {
                Some("left") => "left",
                Some("topleft") => "topleft",
                Some("center") if settings.encoder == VideoEncoder::X264 => "center",
                None | Some("unspecified" | "unknown") if settings.encoder.is_svt() => "unknown",
                _ => {
                    return Err(unsupported(
                        "SVT supports left, top-left, or unspecified chroma placement; x264 requires explicit left, center, or top-left placement.",
                    ));
                }
            },
        })
    }

    pub fn frame_seconds(&self) -> f64 {
        f64::from(self.fps_den) / f64::from(self.fps_num)
    }

    pub fn output_codec(&self) -> &'static str {
        match self.encoder {
            VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => "av1",
            VideoEncoder::X264 => "h264",
        }
    }

    pub fn output_bit_depth(&self) -> u8 {
        if self.output_pixel_format == "yuv420p10le" {
            10
        } else {
            8
        }
    }

    pub fn matches_output_format(&self, format: Option<&str>) -> bool {
        format == Some(self.output_pixel_format)
            || (self.encoder == VideoEncoder::X264
                && self.full_range
                && self.output_pixel_format == "yuv420p"
                && format == Some("yuvj420p"))
    }

    pub fn is_hdr10(&self) -> bool {
        self.hdr10.is_some()
    }

    pub fn hdr_arguments(&self) -> Vec<std::ffi::OsString> {
        let mut args = Vec::new();
        if let Some(hdr) = &self.hdr10 {
            if let Some(mastering) = &hdr.metadata.mastering {
                args.extend(["--mastering-display".into(), mastering.argument().into()]);
            }
            if let Some((cll, fall)) = hdr.metadata.light {
                args.extend(["--content-light".into(), format!("{cll},{fall}").into()]);
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
        if let Some(hdr) = &mut self.hdr10 {
            let first = frames
                .frames
                .first()
                .ok_or_else(|| unsupported("The video did not decode to any frames."))?;
            hdr.metadata
                .absorb_first_frame(&StaticMetadata::parse(&first.side_data_list)?)?;
            if hdr.metadata.mastering.is_none() {
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
        let mut observed_hdr = StaticMetadata::parse(&stream.side_data_list)?;
        validate_side_data(&stream.side_data_list, self.hdr10.as_ref(), encoded)?;
        for (index, frame) in frames.frames.iter().enumerate() {
            let time = seconds(frame.best_effort_timestamp_time.as_deref())
                .ok_or_else(|| unsupported("A decoded frame has no usable timestamp."))?;
            if (time - index as f64 * self.frame_seconds()).abs() > tolerance {
                return Err(unsupported(
                    "Decoded frame timestamps are not constant-rate starting at zero. VFR and timestamp gaps require a later workflow.",
                ));
            }
            if frame.interlaced_frame != Some(0)
                || frame.width != Some(self.width)
                || frame.height != Some(self.height)
                || frame.sample_aspect_ratio.as_deref() != Some("1:1")
            {
                return Err(unsupported(
                    "Interlaced frames or changing frame dimensions/pixel aspect ratios are not supported.",
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
            {
                return Err(unsupported(
                    "Decoded frame chroma placement differs from the selected source.",
                ));
            }
            if if encoded {
                !self.matches_output_format(frame.pix_fmt.as_deref())
            } else {
                frame.pix_fmt != stream.pix_fmt
            } {
                return Err(unsupported(
                    "The decoded bit depth, pixel format, or frame side data changed unexpectedly.",
                ));
            }
            validate_side_data(&frame.side_data_list, self.hdr10.as_ref(), encoded)?;
            if let Some(hdr) = &self.hdr10 {
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
        if let Some(hdr) = &self.hdr10 {
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
        let chroma_matches = match self.chroma {
            "left" => output.chroma_location.as_deref() == Some("left"),
            "topleft" => output.chroma_location.as_deref() == Some("topleft"),
            "center" => output.chroma_location.as_deref() == Some("center"),
            _ => matches!(
                output.chroma_location.as_deref(),
                None | Some("unspecified" | "unknown")
            ),
        };
        if output.codec_name.as_deref() != Some(self.output_codec())
            || !self.matches_output_format(output.pix_fmt.as_deref())
            || output.width != Some(self.width)
            || output.height != Some(self.height)
            || output.sample_aspect_ratio.as_deref() != Some("1:1")
            || source.color_space != output.color_space
            || source.color_transfer != output.color_transfer
            || source.color_primaries != output.color_primaries
            || source.color_range != output.color_range
            || !chroma_matches
        {
            return Err(AppError::new(
                "ENCODE_VALIDATION_FAILED",
                "The encoded output codec, dimensions, bit depth, aspect ratio, or color metadata differ from the plan.",
                None,
            ));
        }
        validate_side_data(&output.side_data_list, self.hdr10.as_ref(), true)?;
        if let Some(hdr) = &self.hdr10 {
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
    fn x264_requires_standalone_settings_and_rejects_hdr_or_unknown_chroma() {
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
            EncodeSettings {
                backend: EncodeBackend::Av1an,
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
        assert!(error.message.contains("SDR only"));
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
            ("pix_fmt", "yuv444p"),
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
