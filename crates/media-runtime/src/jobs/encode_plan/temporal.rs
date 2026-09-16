//! Explicit temporal processing preserves source time. BWDIF, QTGMC and inverse
//! telecine have distinct dependency and cadence contracts.
use super::{Frame, Plan};
use crate::jobs::metadata;
use media_core::{
    AppError, CadenceRepairKind, DeinterlaceMode, EncodeSettings, FieldOrder, FrameRate,
    TemporalSettings,
};
use std::{ffi::OsString, path::Path, time::Duration};
use tokio::sync::watch;

use crate::supervisor::{self, CommandSpec};

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
        let workflows = usize::from(temporal.deinterlace.is_some())
            + usize::from(temporal.qtgmc.is_some())
            + usize::from(temporal.cadence_repair.is_some());
        if workflows > 1 {
            return Err(invalid(
                "Choose one source reconstruction workflow: BWDIF, QTGMC, or inverse telecine.",
            ));
        }
        if temporal.cadence_repair.is_some_and(|repair| {
            repair.kind == CadenceRepairKind::ExactDuplicates && temporal.frame_rate.is_none()
        }) {
            return Err(invalid(
                "Exact-duplicate cadence repair requires the intended constant output frame rate.",
            ));
        }
        if let Some(aspect) = temporal.aspect_ratio
            && (aspect.numerator == 0
                || aspect.denominator == 0
                || aspect.numerator > 65_535
                || aspect.denominator > 65_535)
        {
            return Err(invalid(
                "Aspect-ratio terms must be positive integers no greater than 65535.",
            ));
        }
    }
    Ok(())
}

fn reduced(numerator: u64, denominator: u64) -> Result<FrameRate, AppError> {
    if numerator == 0 || denominator == 0 {
        return Err(invalid("The resulting frame rate is zero."));
    }
    let (mut a, mut b) = (numerator, denominator);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let numerator = u32::try_from(numerator / a).map_err(|_| invalid("Output rate overflow."))?;
    let denominator =
        u32::try_from(denominator / a).map_err(|_| invalid("Output rate overflow."))?;
    Ok(FrameRate {
        numerator,
        denominator,
    })
}

#[derive(Clone, Debug)]
pub(super) struct Transform {
    pub settings: TemporalSettings,
    source_rate: Option<FrameRate>,
    reconstructed_rate: Option<FrameRate>,
    output_rate: Option<FrameRate>,
    frames: Option<usize>,
    exact_unique_frames: Option<usize>,
}

impl Transform {
    pub fn new(settings: TemporalSettings) -> Self {
        Self {
            settings,
            source_rate: None,
            reconstructed_rate: None,
            output_rate: None,
            frames: None,
            exact_unique_frames: None,
        }
    }
    fn activate(
        &mut self,
        frames: usize,
        source: FrameRate,
    ) -> Result<(usize, FrameRate), AppError> {
        if self.settings.deinterlace.is_none()
            && self.settings.qtgmc.is_none()
            && self.settings.cadence_repair.is_none()
            && self.settings.frame_rate.is_none()
        {
            return Ok((frames, source));
        }
        let mut reconstructed_frames = frames;
        let mut reconstructed_rate = source;
        if self
            .settings
            .cadence_repair
            .is_some_and(|repair| repair.kind == CadenceRepairKind::InverseTelecine)
        {
            reconstructed_frames = frames - frames / 5;
            reconstructed_rate = reduced(
                u64::from(source.numerator) * 4,
                u64::from(source.denominator) * 5,
            )?;
        } else if self
            .settings
            .cadence_repair
            .is_some_and(|repair| repair.kind == CadenceRepairKind::ExactDuplicates)
        {
            reconstructed_frames = self.exact_unique_frames.ok_or_else(|| {
                invalid("Exact-duplicate cadence repair was not characterized before activation.")
            })?;
            reconstructed_rate = self
                .settings
                .frame_rate
                .expect("validated exact duplicate output rate");
        } else if self
            .settings
            .deinterlace
            .map(|setting| setting.mode)
            .or_else(|| self.settings.qtgmc.map(|setting| setting.mode))
            == Some(DeinterlaceMode::Bob)
        {
            reconstructed_frames = frames
                .checked_mul(2)
                .ok_or_else(|| invalid("Output frame count overflow."))?;
            reconstructed_rate = reduced(
                u64::from(source.numerator) * 2,
                u64::from(source.denominator),
            )?;
        }
        let output = self.settings.frame_rate.unwrap_or(reconstructed_rate);
        if !valid_rate(output) {
            return Err(invalid(
                "The resulting output rate must be from 1 through 120 fps. Choose a lower explicit rate for bob deinterlacing.",
            ));
        }
        let denominator = u128::from(reconstructed_rate.numerator) * u128::from(output.denominator);
        let numerator = reconstructed_frames as u128
            * u128::from(reconstructed_rate.denominator)
            * u128::from(output.numerator);
        let count = usize::try_from((numerator + denominator / 2) / denominator)
            .map_err(|_| invalid("Output frame count overflow."))?;
        if count == 0 {
            return Err(invalid(
                "The chosen rate would produce no frames for this interval. Choose a longer interval or higher rate.",
            ));
        }
        self.source_rate = Some(source);
        self.reconstructed_rate = Some(reconstructed_rate);
        self.output_rate = Some(output);
        self.frames = Some(count);
        Ok((count, output))
    }
    pub fn field_matches(&self, frame: &Frame) -> bool {
        self.settings
            .deinterlace
            .or_else(|| {
                self.settings
                    .qtgmc
                    .map(|setting| media_core::DeinterlaceSettings {
                        mode: setting.mode,
                        field_order: setting.field_order,
                    })
            })
            .map_or_else(
                || {
                    self.settings.cadence_repair.map_or(
                        frame.interlaced_frame == Some(0),
                        |setting| {
                            if setting.kind == CadenceRepairKind::ExactDuplicates {
                                frame.interlaced_frame == Some(0)
                            } else {
                                frame.interlaced_frame == Some(0)
                                    || (frame.interlaced_frame == Some(1)
                                        && frame.top_field_first
                                            == Some(u8::from(
                                                setting.field_order == FieldOrder::TopFirst,
                                            )))
                            }
                        },
                    )
                },
                |setting| {
                    frame.interlaced_frame == Some(1)
                        && frame.top_field_first
                            == Some(u8::from(setting.field_order == FieldOrder::TopFirst))
                },
            )
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
        } else if let Some(setting) = self.settings.cadence_repair {
            if setting.kind == CadenceRepairKind::ExactDuplicates {
                // The admission scan hashes the same decoded pixels and proves
                // the exact kept-frame count before this guarded path can run.
                filters.push("mpdecimate=max=0:keep=0:hi=0:lo=0:frac=1".into());
            } else {
                filters.push(format!(
                    "fieldmatch=order={}:combmatch=full",
                    if setting.field_order == FieldOrder::TopFirst {
                        "tff"
                    } else {
                        "bff"
                    }
                ));
                if setting.combed_fallback {
                    filters.push(format!(
                        "bwdif=mode=send_frame:parity={}:deint=interlaced",
                        if setting.field_order == FieldOrder::TopFirst {
                            "tff"
                        } else {
                            "bff"
                        }
                    ));
                }
                filters.push("decimate=cycle=5".into());
            }
        }
        if self.settings.frame_rate.is_some()
            && !self
                .settings
                .cadence_repair
                .is_some_and(|repair| repair.kind == CadenceRepairKind::ExactDuplicates)
        {
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

    pub fn requires_qtgmc(&self) -> bool {
        self.settings.qtgmc.is_some()
    }

    pub fn requires_exact_duplicate_scan(&self) -> bool {
        self.settings
            .cadence_repair
            .is_some_and(|repair| repair.kind == CadenceRepairKind::ExactDuplicates)
    }

    pub fn changes_av1an_source_timeline(&self) -> bool {
        self.settings.frame_rate.is_some()
            || self.settings.cadence_repair.is_some()
            || self
                .settings
                .deinterlace
                .is_some_and(|settings| settings.mode == DeinterlaceMode::Bob)
    }

    pub fn set_exact_duplicate_count(
        &mut self,
        source_frames: usize,
        unique_frames: usize,
        source_rate: FrameRate,
    ) -> Result<(), AppError> {
        if !self.requires_exact_duplicate_scan() {
            return Err(invalid(
                "Duplicate cadence evidence was supplied without selecting that repair.",
            ));
        }
        let target = self
            .settings
            .frame_rate
            .expect("validated exact duplicate output rate");
        if u64::from(target.numerator) * u64::from(source_rate.denominator)
            >= u64::from(source_rate.numerator) * u64::from(target.denominator)
        {
            return Err(invalid(
                "Exact-duplicate repair requires an output rate below the padded capture rate.",
            ));
        }
        let denominator = u128::from(source_rate.numerator) * u128::from(target.denominator);
        let numerator = source_frames as u128
            * u128::from(source_rate.denominator)
            * u128::from(target.numerator);
        let expected = usize::try_from((numerator + denominator / 2) / denominator)
            .map_err(|_| invalid("Duplicate cadence frame count overflow."))?;
        if unique_frames != expected {
            return Err(invalid(format!(
                "The decoded duplicate pattern retains {unique_frames} unique frames, but {expected} are required to preserve duration at {}/{} fps.",
                target.numerator, target.denominator
            )));
        }
        self.exact_unique_frames = Some(unique_frames);
        Ok(())
    }

    pub fn qtgmc_settings(&self) -> Option<media_core::QtgmcSettings> {
        self.settings.qtgmc
    }

    /// Run the selected built-in filters against decoded pixels before the
    /// source encode starts. This establishes actual FFmpeg compatibility
    /// instead of assuming that a filter listed by the binary can execute.
    pub async fn check_tools(
        &self,
        ffmpeg: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        let mut filters = Vec::new();
        let mut input = "testsrc2=s=64x64:r=30000/1001:d=0.5,format=yuv420p";
        if let Some(setting) = self.settings.deinterlace {
            filters.push(format!(
                "setfield={},bwdif=mode={}:parity={}:deint=all",
                if setting.field_order == FieldOrder::TopFirst {
                    "tff"
                } else {
                    "bff"
                },
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
        } else if let Some(setting) = self.settings.cadence_repair {
            if setting.kind == CadenceRepairKind::ExactDuplicates {
                input = "color=c=black:s=64x64:r=60:d=0.2,format=yuv420p";
                filters.push("mpdecimate=max=0:keep=0:hi=0:lo=0:frac=1".into());
            } else {
                filters.push(format!(
                    "setfield={},fieldmatch=order={}:combmatch=full",
                    if setting.field_order == FieldOrder::TopFirst {
                        "tff"
                    } else {
                        "bff"
                    },
                    if setting.field_order == FieldOrder::TopFirst {
                        "tff"
                    } else {
                        "bff"
                    }
                ));
                if setting.combed_fallback {
                    filters.push(format!(
                        "bwdif=mode=send_frame:parity={}:deint=interlaced",
                        if setting.field_order == FieldOrder::TopFirst {
                            "tff"
                        } else {
                            "bff"
                        }
                    ));
                }
                filters.push("decimate=cycle=5".into());
            }
        }
        if let Some(rate) = self.settings.frame_rate {
            filters.push(format!(
                "fps=fps={}/{}:round=near",
                rate.numerator, rate.denominator
            ));
        }
        if filters.is_empty() {
            return Ok(());
        }
        let mut args = ["-v", "error", "-nostdin", "-f", "lavfi", "-i", input, "-vf"]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        args.extend([
            filters.join(",").into(),
            "-frames:v".into(),
            "20".into(),
            "-pix_fmt".into(),
            "+yuv420p".into(),
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
            256 * 1024,
            Duration::from_secs(10),
        )
        .await
        .map_err(|error| crate::jobs::process_error(error, ffmpeg))?;
        const FRAME_BYTES: usize = 64 * 64 * 3 / 2;
        if !captured.status.success()
            || captured.stdout.is_empty()
            || !captured.stdout.len().is_multiple_of(FRAME_BYTES)
        {
            return Err(AppError::new(
                "TEMPORAL_TOOL_UNSUPPORTED",
                format!(
                    "The installed FFmpeg could not run the selected temporal filter chain: {}",
                    String::from_utf8_lossy(&captured.stderr)
                ),
                None,
            ));
        }
        Ok(())
    }

    /// QTGMC has already reconstructed the fields and emitted this cadence.
    /// Normalize that Y4M clock, then apply only an explicitly requested final
    /// rate conversion before the remaining pixel filters.
    pub fn post_qtgmc_filter(&self) -> Option<String> {
        self.settings.qtgmc?;
        let source = self.reconstructed_rate?;
        let output = self.output_rate.expect("active output rate");
        let mut filters = vec![format!(
            "settb=expr={}/{},setpts=N",
            source.denominator, source.numerator
        )];
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
        } else if self.temporal.as_ref().is_some_and(|transform| {
            transform.settings.deinterlace.is_some() || transform.settings.qtgmc.is_some()
        }) {
            "A source frame is progressive, has unknown field order, or disagrees with the selected TFF/BFF order. Choose the matching field order for a uniformly interlaced source."
        } else if self.temporal.as_ref().is_some_and(|transform| {
            transform
                .settings
                .cadence_repair
                .is_some_and(|repair| repair.kind == CadenceRepairKind::ExactDuplicates)
        }) {
            "Exact-duplicate cadence repair requires a progressive decoded source."
        } else if self
            .temporal
            .as_ref()
            .is_some_and(|transform| transform.settings.cadence_repair.is_some())
        {
            "Inverse telecine found an interlaced frame with the wrong field order. Choose the matching TFF/BFF order."
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

    #[test]
    fn exact_duplicate_repair_needs_characterized_duration_preserving_count() {
        let settings = TemporalSettings {
            cadence_repair: Some(media_core::CadenceRepairSettings {
                kind: CadenceRepairKind::ExactDuplicates,
                field_order: FieldOrder::TopFirst,
                combed_fallback: false,
            }),
            frame_rate: Some(FrameRate {
                numerator: 24,
                denominator: 1,
            }),
            ..Default::default()
        };
        let mut transform = Transform::new(settings);
        assert!(
            transform
                .activate(
                    60,
                    FrameRate {
                        numerator: 60,
                        denominator: 1
                    }
                )
                .is_err()
        );
        transform
            .set_exact_duplicate_count(
                60,
                24,
                FrameRate {
                    numerator: 60,
                    denominator: 1,
                },
            )
            .unwrap();
        assert_eq!(
            transform
                .activate(
                    60,
                    FrameRate {
                        numerator: 60,
                        denominator: 1,
                    },
                )
                .unwrap(),
            (
                24,
                FrameRate {
                    numerator: 24,
                    denominator: 1,
                }
            )
        );
        let filter = transform.filter().unwrap();
        assert!(filter.contains("mpdecimate=max=0:keep=0:hi=0:lo=0:frac=1"));
        assert!(!filter.contains("fps="));
        assert!(filter.ends_with("settb=expr=1/24,setpts=N,setfield=prog"));

        let mut wrong = Transform::new(settings);
        assert!(
            wrong
                .set_exact_duplicate_count(
                    60,
                    25,
                    FrameRate {
                        numerator: 60,
                        denominator: 1,
                    },
                )
                .is_err()
        );
    }

    #[test]
    fn av1an_prepares_only_timeline_changing_builtin_filters() {
        let transform =
            |mode: Option<DeinterlaceMode>,
             rate: Option<FrameRate>,
             cadence: Option<media_core::CadenceRepairSettings>| {
                Transform::new(TemporalSettings {
                    deinterlace: mode.map(|mode| media_core::DeinterlaceSettings {
                        mode,
                        field_order: FieldOrder::TopFirst,
                    }),
                    frame_rate: rate,
                    cadence_repair: cadence,
                    ..Default::default()
                })
            };
        assert!(
            !transform(Some(DeinterlaceMode::Frame), None, None).changes_av1an_source_timeline()
        );
        assert!(transform(Some(DeinterlaceMode::Bob), None, None).changes_av1an_source_timeline());
        assert!(
            transform(
                None,
                Some(FrameRate {
                    numerator: 24,
                    denominator: 1,
                }),
                None,
            )
            .changes_av1an_source_timeline()
        );
        assert!(
            transform(
                None,
                None,
                Some(media_core::CadenceRepairSettings {
                    kind: CadenceRepairKind::InverseTelecine,
                    field_order: FieldOrder::TopFirst,
                    combed_fallback: false,
                }),
            )
            .changes_av1an_source_timeline()
        );
    }

    #[tokio::test]
    #[ignore = "requires an installed FFmpeg"]
    async fn selected_temporal_filters_execute_on_real_decoded_frames() {
        let ffmpeg = crate::discovery::find_executable(&["ffmpeg"])
            .await
            .unwrap()
            .expect("FFmpeg on PATH");
        let (_sender, cancel) = watch::channel(false);
        let settings = [
            TemporalSettings {
                deinterlace: Some(media_core::DeinterlaceSettings {
                    mode: DeinterlaceMode::Bob,
                    field_order: FieldOrder::BottomFirst,
                }),
                frame_rate: Some(FrameRate {
                    numerator: 24_000,
                    denominator: 1_001,
                }),
                ..Default::default()
            },
            TemporalSettings {
                cadence_repair: Some(media_core::CadenceRepairSettings {
                    kind: CadenceRepairKind::InverseTelecine,
                    field_order: FieldOrder::TopFirst,
                    combed_fallback: true,
                }),
                ..Default::default()
            },
            TemporalSettings {
                cadence_repair: Some(media_core::CadenceRepairSettings {
                    kind: CadenceRepairKind::ExactDuplicates,
                    field_order: FieldOrder::TopFirst,
                    combed_fallback: false,
                }),
                frame_rate: Some(FrameRate {
                    numerator: 24,
                    denominator: 1,
                }),
                ..Default::default()
            },
        ];
        for settings in settings {
            Transform::new(settings)
                .check_tools(&ffmpeg, &cancel)
                .await
                .unwrap();
        }
    }
}
