//! Explicit temporal processing preserves source time. BWDIF is not QTGMC or
//! cadence repair; frame-rate conversion duplicates/drops frames without speedup.
use super::{Frame, Plan};
use crate::jobs::metadata;
use media_core::{
    AppError, DeinterlaceMode, EncodeBackend, EncodeSettings, FieldOrder, FrameRate,
    TemporalSettings,
};

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("TEMPORAL_SETTINGS_INVALID", message, None)
}

fn valid_rate(rate: FrameRate) -> bool {
    rate.numerator > 0
        && rate.numerator <= 12_000_000
        && rate.denominator > 0
        && rate.denominator <= 100_000
        && u64::from(rate.numerator) >= u64::from(rate.denominator)
        && u64::from(rate.numerator) <= u64::from(rate.denominator) * 120
}

pub(super) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    if let Some(temporal) = settings.temporal {
        if temporal.frame_rate.is_some_and(|rate| !valid_rate(rate)) {
            return Err(invalid(
                "Use a positive rational output rate from 1 through 120 fps (numerator at most 12000000; denominator at most 100000).",
            ));
        }
        if settings.backend == EncodeBackend::Av1an
            && (temporal.deinterlace.is_some() || temporal.frame_rate.is_some())
        {
            return Err(invalid(
                "Deinterlacing and frame-rate conversion currently require standalone encoding. Av1an target probes and chunks must share the same temporal processing before this can be enabled there.",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(super) struct Transform {
    pub settings: TemporalSettings,
    source_rate: Option<FrameRate>,
    output_rate: Option<FrameRate>,
    frames: Option<usize>,
}

impl Transform {
    pub fn new(settings: TemporalSettings) -> Self {
        Self {
            settings,
            source_rate: None,
            output_rate: None,
            frames: None,
        }
    }
    fn activate(
        &mut self,
        frames: usize,
        source: FrameRate,
    ) -> Result<(usize, FrameRate), AppError> {
        if self.settings.deinterlace.is_none() && self.settings.frame_rate.is_none() {
            return Ok((frames, source));
        }
        let bob = self
            .settings
            .deinterlace
            .is_some_and(|setting| setting.mode == DeinterlaceMode::Bob);
        let output = self.settings.frame_rate.unwrap_or(FrameRate {
            numerator: source
                .numerator
                .checked_mul(if bob { 2 } else { 1 })
                .ok_or_else(|| invalid("Output rate overflow."))?,
            denominator: source.denominator,
        });
        if !valid_rate(output) {
            return Err(invalid(
                "The resulting output rate must be from 1 through 120 fps. Choose a lower explicit rate for bob deinterlacing.",
            ));
        }
        let denominator = u128::from(source.numerator) * u128::from(output.denominator);
        let numerator =
            frames as u128 * u128::from(source.denominator) * u128::from(output.numerator);
        let count = usize::try_from((numerator + denominator / 2) / denominator)
            .map_err(|_| invalid("Output frame count overflow."))?;
        if count == 0 {
            return Err(invalid(
                "The chosen rate would produce no frames for this interval. Choose a longer interval or higher rate.",
            ));
        }
        self.source_rate = Some(source);
        self.output_rate = Some(output);
        self.frames = Some(count);
        Ok((count, output))
    }
    pub fn field_matches(&self, frame: &Frame) -> bool {
        self.settings
            .deinterlace
            .map_or(frame.interlaced_frame == Some(0), |setting| {
                frame.interlaced_frame == Some(1)
                    && frame.top_field_first
                        == Some(u8::from(setting.field_order == FieldOrder::TopFirst))
            })
    }
    pub fn filter(&self) -> Option<String> {
        let source = self.source_rate?;
        let output = self.output_rate.expect("active output rate");
        let mut filters = vec![format!(
            "settb=expr={}/{},setpts=N",
            source.denominator, source.numerator
        )];
        if let Some(setting) = self.settings.deinterlace {
            filters.push(format!(
                "bwdif=mode={}:parity={}:deint=all",
                if setting.mode == DeinterlaceMode::Bob {
                    "send_field"
                } else {
                    "send_frame"
                },
                if setting.field_order == FieldOrder::TopFirst {
                    "tff"
                } else {
                    "bff"
                }
            ));
        }
        if self.settings.frame_rate.is_some() {
            filters.push(format!(
                "fps=fps={}/{}:start_time=0:round=near:eof_action=round",
                output.numerator, output.denominator
            ));
        }
        filters.push(format!(
            "settb=expr={}/{},setpts=N,setfield=prog",
            output.denominator, output.numerator
        ));
        Some(filters.join(","))
    }
}

impl Plan {
    pub fn activate_temporal(&mut self, frames: usize) -> Result<usize, AppError> {
        if let Some(transform) = &mut self.temporal {
            let (frames, rate) = transform.activate(
                frames,
                FrameRate {
                    numerator: self.fps_num,
                    denominator: self.fps_den,
                },
            )?;
            self.fps_num = rate.numerator;
            self.fps_den = rate.denominator;
            return Ok(frames);
        }
        Ok(frames)
    }
    pub(super) fn fields_match(&self, frame: &Frame, encoded: bool) -> bool {
        if encoded {
            frame.interlaced_frame == Some(0)
        } else {
            self.temporal
                .as_ref()
                .map_or(frame.interlaced_frame == Some(0), |transform| {
                    transform.field_matches(frame)
                })
        }
    }
    pub(super) fn validate_fields(&self, frame: &Frame, encoded: bool) -> Result<(), AppError> {
        if self.fields_match(frame, encoded) {
            return Ok(());
        }
        Err(invalid(if encoded {
            "The encoded output contains an interlaced frame; progressive output is required."
        } else if self
            .temporal
            .as_ref()
            .is_some_and(|transform| transform.settings.deinterlace.is_some())
        {
            "A source frame is progressive, has unknown field order, or disagrees with the selected TFF/BFF order. Choose the matching field order for a uniformly interlaced source; mixed cadence needs a separate workflow."
        } else {
            "An interlaced source frame requires explicit BWDIF deinterlacing and the matching source field order."
        }))
    }
    pub fn temporal_document(&self, mut document: metadata::Document) -> metadata::Document {
        if let Some(frames) = self
            .temporal
            .as_ref()
            .and_then(|transform| transform.frames)
            && let Some(video) = document
                .streams
                .iter_mut()
                .find(|stream| stream.index == self.video_index)
        {
            video.duration = Some((frames as f64 * self.frame_seconds()).to_string());
        }
        document
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_output_clock_is_integer_and_count_rounds_once() {
        let mut transform = Transform::new(TemporalSettings {
            frame_rate: Some(FrameRate {
                numerator: 30000,
                denominator: 1001,
            }),
            ..Default::default()
        });
        assert_eq!(
            transform
                .activate(
                    48,
                    FrameRate {
                        numerator: 24,
                        denominator: 1
                    }
                )
                .unwrap()
                .0,
            60
        );
        let filter = transform.filter().unwrap();
        assert!(filter.starts_with("settb=expr=1/24,setpts=N,"));
        assert!(filter.ends_with("settb=expr=1001/30000,setpts=N,setfield=prog"));
        assert!(
            !filter.contains("/TB"),
            "floating timestamps can truncate a whole tick"
        );
        assert!(
            transform
                .activate(
                    1,
                    FrameRate {
                        numerator: 120,
                        denominator: 1
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn temporal_order_and_rate_limits_are_explicit() {
        let mut transform = Transform::new(TemporalSettings {
            deinterlace: Some(media_core::DeinterlaceSettings {
                mode: DeinterlaceMode::Bob,
                field_order: FieldOrder::BottomFirst,
            }),
            frame_rate: Some(FrameRate {
                numerator: 24,
                denominator: 1,
            }),
            ..Default::default()
        });
        assert_eq!(
            transform
                .activate(
                    30,
                    FrameRate {
                        numerator: 30,
                        denominator: 1
                    }
                )
                .unwrap()
                .0,
            24
        );
        let filter = transform.filter().unwrap();
        assert!(
            filter.find("bwdif=mode=send_field:parity=bff").unwrap()
                < filter.find("fps=fps=24/1").unwrap()
        );
        for rate in [
            FrameRate {
                numerator: 0,
                denominator: 1,
            },
            FrameRate {
                numerator: 24,
                denominator: 0,
            },
            FrameRate {
                numerator: 121,
                denominator: 1,
            },
            FrameRate {
                numerator: 1,
                denominator: 2,
            },
        ] {
            assert!(!valid_rate(rate));
        }
        transform.settings.frame_rate = None;
        assert!(
            transform
                .activate(
                    60,
                    FrameRate {
                        numerator: 60,
                        denominator: 1
                    }
                )
                .is_ok()
        );
        assert!(
            transform
                .activate(
                    61,
                    FrameRate {
                        numerator: 61,
                        denominator: 1
                    }
                )
                .is_err()
        );
    }
}
