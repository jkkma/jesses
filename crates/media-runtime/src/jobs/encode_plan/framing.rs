use media_core::{AppError, CropSettings, EncodeBackend, EncodeSettings, VideoFraming};

fn invalid(message: &str) -> AppError {
    AppError::new("ENCODE_SETTINGS_INVALID", message, None)
}

fn valid_dimension(value: u32) -> bool {
    (64..=8192).contains(&value) && value.is_multiple_of(2)
}

pub(super) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    if settings.backend == EncodeBackend::Av1an && settings.framing != VideoFraming::default() {
        return Err(invalid(
            "Crop and resize require a standalone encoder. Reset framing before choosing av1an.",
        ));
    }
    let crop = settings.framing.crop;
    if [crop.top, crop.right, crop.bottom, crop.left]
        .iter()
        .any(|edge| !edge.is_multiple_of(2))
    {
        return Err(invalid("Crop edges must be nonnegative even pixel counts."));
    }
    if settings
        .framing
        .resize_width
        .is_some_and(|width| !valid_dimension(width))
    {
        return Err(invalid(
            "Resize width must be an even number from 64 to 8192 pixels.",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(super) struct Geometry {
    pub source_width: u32,
    pub source_height: u32,
    pub width: u32,
    pub height: u32,
    crop: CropSettings,
    crop_width: u32,
    crop_height: u32,
}

impl Geometry {
    pub(super) fn build(width: u32, height: u32, framing: VideoFraming) -> Result<Self, AppError> {
        let crop = framing.crop;
        let cropped = |source: u32, first: u32, second: u32| {
            first
                .checked_add(second)
                .and_then(|removed| source.checked_sub(removed))
        };
        let crop_width = cropped(width, crop.left, crop.right);
        let crop_height = cropped(height, crop.top, crop.bottom);
        let (Some(crop_width), Some(crop_height)) = (crop_width, crop_height) else {
            return Err(invalid("Crop edges extend beyond the source dimensions."));
        };
        if !valid_dimension(crop_width) || !valid_dimension(crop_height) {
            return Err(invalid(
                "The cropped dimensions must be even and between 64 and 8192 pixels.",
            ));
        }
        let output_width = framing.resize_width.unwrap_or(crop_width);
        // Nearest even height, with a halfway result rounded upward. Integer
        // arithmetic keeps the preview and encoder geometry deterministic.
        let output_height = u64::from(crop_height)
            .checked_mul(u64::from(output_width))
            .and_then(|area| area.checked_add(u64::from(crop_width)))
            .and_then(|numerator| numerator.checked_div(u64::from(crop_width).checked_mul(2)?))
            .and_then(|half_height| half_height.checked_mul(2))
            .and_then(|height| u32::try_from(height).ok())
            .ok_or_else(|| invalid("The requested resize exceeds the supported dimensions."))?;
        if !valid_dimension(output_width) || !valid_dimension(output_height) {
            return Err(invalid(
                "The resized dimensions must be even and between 64 and 8192 pixels.",
            ));
        }
        Ok(Self {
            source_width: width,
            source_height: height,
            width: output_width,
            height: output_height,
            crop,
            crop_width,
            crop_height,
        })
    }

    pub(super) fn filter(&self, matrix: u8, full_range: bool, chroma: &str) -> Option<String> {
        let mut filters = Vec::new();
        if self.crop != CropSettings::default() {
            filters.push(format!(
                "crop={}:{}:{}:{}:exact=1",
                self.crop_width, self.crop_height, self.crop.left, self.crop.top,
            ));
        }
        if self.width != self.crop_width || self.height != self.crop_height {
            let matrix = match matrix {
                1 => "bt709",
                5 => "bt470bg",
                6 => "smpte170m",
                9 => "bt2020",
                _ => unreachable!("Plan validates supported color matrices"),
            };
            let range = if full_range { "full" } else { "limited" };
            let mut scale = format!(
                "scale={}:{}:flags=lanczos:in_color_matrix={matrix}:out_color_matrix={matrix}:in_range={range}:out_range={range}",
                self.width, self.height,
            );
            // Explicit sample positions keep scale from assuming centered chroma
            // for a source tagged left/top-left. These flags also work on older
            // FFmpeg builds that predate the in/out_chroma_loc spelling.
            let position = match chroma {
                "left" => Some((0, 128)),
                "topleft" => Some((0, 0)),
                "center" => Some((128, 128)),
                _ => None,
            };
            if let Some((x, y)) = position {
                scale.push_str(&format!(
                    ":in_h_chr_pos={x}:out_h_chr_pos={x}:in_v_chr_pos={y}:out_v_chr_pos={y}"
                ));
            }
            filters.push(scale);
        }
        if filters.is_empty() {
            None
        } else {
            // Scale may compensate its even-height rounding by changing SAR.
            // The workflow's output contract deliberately retains square pixels.
            filters.push("setsar=1".into());
            Some(filters.join(","))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_then_resize_uses_cropped_aspect_ratio_and_rounds_halfway_up() {
        let crop = CropSettings {
            left: 16,
            right: 16,
            ..Default::default()
        };
        let crop_only = Geometry::build(
            128,
            96,
            VideoFraming {
                crop,
                resize_width: None,
            },
        )
        .unwrap();
        assert_eq!((crop_only.width, crop_only.height), (96, 96));
        let upscaled = Geometry::build(
            128,
            96,
            VideoFraming {
                crop,
                resize_width: Some(192),
            },
        )
        .unwrap();
        assert_eq!((upscaled.width, upscaled.height), (192, 192));
        let tie = Geometry::build(
            192,
            130,
            VideoFraming {
                resize_width: Some(96),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((tie.width, tie.height), (96, 66));
        let rounded_down = Geometry::build(
            192,
            132,
            VideoFraming {
                resize_width: Some(94),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((rounded_down.width, rounded_down.height), (94, 64));
    }

    #[test]
    fn invalid_edges_dimensions_overflow_and_av1an_are_rejected() {
        for framing in [
            VideoFraming {
                crop: CropSettings {
                    left: 1,
                    right: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            VideoFraming {
                crop: CropSettings {
                    top: 3,
                    ..Default::default()
                },
                ..Default::default()
            },
            VideoFraming {
                resize_width: Some(63),
                ..Default::default()
            },
            VideoFraming {
                resize_width: Some(95),
                ..Default::default()
            },
            VideoFraming {
                resize_width: Some(8194),
                ..Default::default()
            },
        ] {
            assert_eq!(
                validate(&EncodeSettings {
                    framing,
                    ..Default::default()
                })
                .unwrap_err()
                .code,
                "ENCODE_SETTINGS_INVALID"
            );
        }
        for crop in [
            CropSettings {
                left: 66,
                ..Default::default()
            }, // Leaves too little width.
            CropSettings {
                top: 34,
                ..Default::default()
            }, // Leaves too little height.
            CropSettings {
                left: 128,
                ..Default::default()
            },
            CropSettings {
                right: 130,
                ..Default::default()
            },
            CropSettings {
                left: u32::MAX - 1,
                right: 2,
                ..Default::default()
            },
            CropSettings {
                top: u32::MAX - 1,
                bottom: u32::MAX - 1,
                ..Default::default()
            },
        ] {
            assert!(
                Geometry::build(
                    128,
                    96,
                    VideoFraming {
                        crop,
                        resize_width: None
                    }
                )
                .is_err()
            );
        }
        for (width, height, output_width) in [(128, 96, 64), (64, 8192, 8192)] {
            assert!(
                Geometry::build(
                    width,
                    height,
                    VideoFraming {
                        resize_width: Some(output_width),
                        ..Default::default()
                    }
                )
                .is_err()
            );
        }
        assert!(
            validate(&EncodeSettings {
                backend: EncodeBackend::Av1an,
                ..Default::default()
            })
            .is_ok()
        );
        for framing in [
            VideoFraming {
                crop: CropSettings {
                    left: 2,
                    ..Default::default()
                },
                ..Default::default()
            },
            VideoFraming {
                resize_width: Some(128),
                ..Default::default()
            },
        ] {
            let error = validate(&EncodeSettings {
                backend: EncodeBackend::Av1an,
                framing,
                ..Default::default()
            })
            .unwrap_err();
            assert!(error.message.contains("standalone"));
        }
    }

    #[test]
    fn no_op_framing_never_adds_a_filter_and_color_placement_is_explicit_when_scaling() {
        for resize_width in [None, Some(128)] {
            let geometry = Geometry::build(
                128,
                96,
                VideoFraming {
                    resize_width,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!((geometry.width, geometry.height), (128, 96));
            assert_eq!(geometry.filter(1, false, "left"), None);
        }
        let geometry = Geometry::build(
            128,
            96,
            VideoFraming {
                crop: CropSettings {
                    left: 16,
                    right: 16,
                    ..Default::default()
                },
                resize_width: Some(64),
            },
        )
        .unwrap();
        for (matrix_id, matrix) in [
            (1, "bt709"),
            (5, "bt470bg"),
            (6, "smpte170m"),
            (9, "bt2020"),
        ] {
            for (full, range) in [(false, "limited"), (true, "full")] {
                for (chroma, x, y) in [("left", 0, 128), ("topleft", 0, 0), ("center", 128, 128)] {
                    let filter = geometry.filter(matrix_id, full, chroma).unwrap();
                    assert!(
                        filter.starts_with("crop=96:96:16:0:exact=1,scale=64:64:flags=lanczos:")
                    );
                    assert!(filter.contains(&format!(
                        "in_color_matrix={matrix}:out_color_matrix={matrix}"
                    )));
                    assert!(filter.contains(&format!("in_range={range}:out_range={range}")));
                    assert!(filter.contains(&format!(
                        "in_h_chr_pos={x}:out_h_chr_pos={x}:in_v_chr_pos={y}:out_v_chr_pos={y}"
                    )));
                    assert!(filter.ends_with(",setsar=1"));
                }
            }
        }
    }
}
