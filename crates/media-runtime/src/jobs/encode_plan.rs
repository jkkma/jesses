use media_core::{AppError, EncodeSettings};
use serde::Deserialize;

use super::metadata::{Document, Stream};

#[derive(Clone, Debug)]
pub(super) struct Plan {
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
}

pub(super) fn unsupported(message: &str) -> AppError {
    AppError::new("ENCODE_INPUT_UNSUPPORTED", message, None)
}

pub(super) fn validate_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    if !(1..=63).contains(&settings.crf) || settings.preset > 13 {
        return Err(AppError::new(
            "ENCODE_SETTINGS_INVALID",
            "Use CRF 1–63 and an SVT preset from 0–13.",
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
            "This first encoder supports explicitly tagged BT.709, BT.470BG, or SMPTE 170M SDR color. HDR and unknown color metadata require a later workflow.",
        )),
    }
}

fn unsupported_side_data(values: &[serde_json::Value]) -> bool {
    values.iter().any(|value| {
        let kind = value
            .get("side_data_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        [
            "mastering",
            "content light",
            "dovi",
            "dolby",
            "hdr",
            "display matrix",
        ]
        .iter()
        .any(|term| kind.contains(term))
    })
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
        if !matches!(video.pix_fmt.as_deref(), Some("yuv420p" | "yuv420p10le")) {
            return Err(unsupported(
                "This first encoder supports 8-bit or 10-bit planar 4:2:0 video only.",
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
                "This first encoder requires explicit square pixels (sample aspect ratio 1:1).",
            ));
        }
        if unsupported_side_data(&video.side_data_list)
            || video
                .tags
                .iter()
                .any(|(key, value)| key.eq_ignore_ascii_case("rotate") && value != "0")
        {
            return Err(unsupported(
                "Rotation, display transforms, and HDR side data are not supported by this encoding workflow.",
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
                    "The first encoder requires even dimensions between 64 and 8192 pixels.",
                ));
            }
        };
        let (fps_num, fps_den) = rational(video.avg_frame_rate.as_deref())
            .or_else(|| rational(video.r_frame_rate.as_deref()))
            .ok_or_else(|| unsupported("The source has no usable rational frame rate."))?;
        let fps = f64::from(fps_num) / f64::from(fps_den);
        if !(1.0..=120.0).contains(&fps) {
            return Err(unsupported(
                "The first encoder supports constant frame rates from 1 through 120 fps.",
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
                    "The first encoder requires video and container timelines that start at zero. Timestamp offset handling is not available yet.",
                ));
            }
        }
        Ok(Self {
            video_index: video.index,
            width,
            height,
            fps_num,
            fps_den,
            cadence_reconciled: false,
            tolerance,
            primaries: sdr_color(video.color_primaries.as_deref())?,
            transfer: sdr_color(video.color_transfer.as_deref())?,
            matrix: sdr_color(video.color_space.as_deref())?,
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
                None | Some("unspecified" | "unknown") => "unknown",
                _ => {
                    return Err(unsupported(
                        "This first encoder supports left, top-left, or unspecified chroma placement only.",
                    ));
                }
            },
        })
    }

    pub fn frame_seconds(&self) -> f64 {
        f64::from(self.fps_den) / f64::from(self.fps_num)
    }

    pub fn validate_source_frames(
        &mut self,
        frames: &Frames,
        stream: &Stream,
    ) -> Result<usize, AppError> {
        let declared_error = match self.validate_frames(frames, stream, false) {
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
        match candidate.validate_frames(frames, stream, false) {
            Ok(count) => {
                *self = candidate;
                Ok(count)
            }
            Err(_) => Err(declared_error),
        }
    }

    pub fn validate_frames(
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
                _ => "unsupported",
            };
            if normalize_chroma(frame.chroma_location.as_deref())
                != normalize_chroma(stream.chroma_location.as_deref())
            {
                return Err(unsupported(
                    "Decoded frame chroma placement differs from the selected source.",
                ));
            }
            let expected_format = if encoded {
                Some("yuv420p10le")
            } else {
                stream.pix_fmt.as_deref()
            };
            if frame.pix_fmt.as_deref() != expected_format
                || unsupported_side_data(&frame.side_data_list)
            {
                return Err(unsupported(
                    "The decoded bit depth, pixel format, or frame side data changed unexpectedly.",
                ));
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
            _ => matches!(
                output.chroma_location.as_deref(),
                None | Some("unspecified" | "unknown")
            ),
        };
        if output.codec_name.as_deref() != Some("av1")
            || output.pix_fmt.as_deref() != Some("yuv420p10le")
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
                "The AV1 output dimensions, 10-bit format, aspect ratio, or SDR color metadata differ from the plan.",
                None,
            ));
        }
        Ok(())
    }
}

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
    fn source() -> Document {
        serde_json::from_str(r#"{"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":128,"height":96,"pix_fmt":"yuv420p","field_order":"progressive","sample_aspect_ratio":"1:1","avg_frame_rate":"24000/1001","time_base":"1/1000","start_time":"0","color_space":"bt709","color_primaries":"bt709","color_transfer":"bt709","color_range":"tv"}],"format":{"start_time":"0","duration":"1"}}"#).unwrap()
    }
    fn frames(times: &[&str]) -> Frames {
        serde_json::from_value(serde_json::json!({"frames": times.iter().map(|time| serde_json::json!({"best_effort_timestamp_time":time,"interlaced_frame":0,"width":128,"height":96,"pix_fmt":"yuv420p","sample_aspect_ratio":"1:1","color_space":"bt709","color_primaries":"bt709","color_transfer":"bt709","color_range":"tv"})).collect::<Vec<_>>()})).unwrap()
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
