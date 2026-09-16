use media_core::{AppError, BorderSettings, CropSettings, EncodeSettings, VideoFraming};

fn invalid(message: &str) -> AppError {
    AppError::new("ENCODE_SETTINGS_INVALID", message, None)
}

fn valid_dimension(value: u32) -> bool {
    (64..=8192).contains(&value) && value.is_multiple_of(2)
}

pub(super) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    let crop = settings.framing.crop;
    if [crop.top, crop.right, crop.bottom, crop.left]
        .iter()
        .any(|edge| !edge.is_multiple_of(2))
    {
        return Err(invalid("Crop edges must be nonnegative even pixel counts."));
    }
    let borders = settings.framing.borders;
    if [borders.top, borders.right, borders.bottom, borders.left]
        .iter()
        .any(|edge| !edge.is_multiple_of(2))
    {
        return Err(invalid(
            "Border edges must be nonnegative even pixel counts.",
        ));
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
    pub resize_filter: media_core::ResizeFilter,
    pub source_width: u32,
    pub source_height: u32,
    pub width: u32,
    pub height: u32,
    content_width: u32,
    content_height: u32,
    borders: BorderSettings,
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
        let content_width = framing.resize_width.unwrap_or(crop_width);
        // Nearest even height, with a halfway result rounded upward. Integer
        // arithmetic keeps the preview and encoder geometry deterministic.
        let content_height = u64::from(crop_height)
            .checked_mul(u64::from(content_width))
            .and_then(|area| area.checked_add(u64::from(crop_width)))
            .and_then(|numerator| numerator.checked_div(u64::from(crop_width).checked_mul(2)?))
            .and_then(|half_height| half_height.checked_mul(2))
            .and_then(|height| u32::try_from(height).ok())
            .ok_or_else(|| invalid("The requested resize exceeds the supported dimensions."))?;
        if !valid_dimension(content_width) || !valid_dimension(content_height) {
            return Err(invalid(
                "The resized dimensions must be even and between 64 and 8192 pixels.",
            ));
        }
        let borders = framing.borders;
        let padded =
            |content: u32, first: u32, second: u32| content.checked_add(first)?.checked_add(second);
        let (Some(output_width), Some(output_height)) = (
            padded(content_width, borders.left, borders.right),
            padded(content_height, borders.top, borders.bottom),
        ) else {
            return Err(invalid(
                "The requested borders exceed the supported dimensions.",
            ));
        };
        if !valid_dimension(output_width) || !valid_dimension(output_height) {
            return Err(invalid(
                "The dimensions including borders must be even and between 64 and 8192 pixels.",
            ));
        }
        Ok(Self {
            resize_filter: media_core::ResizeFilter::Lanczos,
            source_width: width,
            source_height: height,
            width: output_width,
            height: output_height,
            content_width,
            content_height,
            borders,
            crop,
            crop_width,
            crop_height,
        })
    }

    #[cfg(test)]
    pub(super) fn filter(
        &self,
        matrix: u8,
        full_range: bool,
        chroma: &str,
        pixel_format: &str,
    ) -> Option<String> {
        self.filter_with_text(matrix, full_range, chroma, pixel_format, None)
    }

    pub(super) fn filter_with_text(
        &self,
        matrix: u8,
        full_range: bool,
        chroma: &str,
        pixel_format: &str,
        text_filter: Option<&str>,
    ) -> Option<String> {
        let mut filters = Vec::new();
        if self.crop != CropSettings::default() {
            filters.push(format!(
                "crop={}:{}:{}:{}:exact=1",
                self.crop_width, self.crop_height, self.crop.left, self.crop.top,
            ));
        }
        if self.content_width != self.crop_width || self.content_height != self.crop_height {
            let matrix = match matrix {
                1 => "bt709",
                5 => "bt470bg",
                6 => "smpte170m",
                9 => "bt2020",
                _ => unreachable!("Plan validates supported color matrices"),
            };
            let range = if full_range { "full" } else { "limited" };
            let kernel = match self.resize_filter {
                media_core::ResizeFilter::Nearest => "neighbor",
                media_core::ResizeFilter::Bilinear => "bilinear",
                media_core::ResizeFilter::Bicubic => "bicubic",
                media_core::ResizeFilter::Lanczos => "lanczos",
            };
            let mut scale = format!(
                "scale={}:{}:flags={kernel}:in_color_matrix={matrix}:out_color_matrix={matrix}:in_range={range}:out_range={range}",
                self.content_width, self.content_height,
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
        if let Some(text_filter) = text_filter {
            filters.push(text_filter.to_owned());
        }
        if self.borders != BorderSettings::default() {
            // Lock depth before assigning planar sample values. FFmpeg pad's
            // RGB black conversion varies with range/version, and can produce
            // 514 instead of neutral 512 chroma in limited-range 10-bit video.
            // Fill a duplicate frame with exact planar black, then copy the
            // untouched content onto it. LUT filling avoids a slow per-pixel
            // expression. Both branches retain identical frame timestamps.
            filters.push(format!("format={pixel_format}"));
            let depth_shift = match pixel_format {
                "yuv420p" => 0,
                "yuv420p10le" => 2,
                _ => unreachable!("Plan validates supported output formats"),
            };
            let black = if full_range { 0 } else { 16 << depth_shift };
            let neutral = 128 << depth_shift;
            filters.push(format!(
                "split[jesses_content][jesses_canvas];[jesses_canvas]pad={}:{}:{}:{}:color=black,lutyuv=y={black}:u={neutral}:v={neutral}[jesses_background];[jesses_background][jesses_content]overlay=x={}:y={}:format=auto:shortest=1",
                self.width, self.height, self.borders.left, self.borders.top,
                self.borders.left, self.borders.top,
            ));
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
    use media_core::EncodeBackend;

    #[test]
    fn borders_follow_content_resize_and_keep_source_and_output_dimensions_separate() {
        let geometry = Geometry::build(
            192,
            128,
            VideoFraming {
                crop: CropSettings {
                    left: 32,
                    ..Default::default()
                },
                resize_width: Some(80),
                borders: BorderSettings {
                    top: 8,
                    right: 24,
                    bottom: 16,
                    left: 32,
                },
            },
        )
        .unwrap();
        assert_eq!((geometry.source_width, geometry.source_height), (192, 128));
        assert_eq!((geometry.crop_width, geometry.crop_height), (160, 128));
        assert_eq!((geometry.content_width, geometry.content_height), (80, 64));
        assert_eq!((geometry.width, geometry.height), (136, 88));
        let filter = geometry.filter(1, false, "left", "yuv420p10le").unwrap();
        assert!(filter.starts_with("crop=160:128:32:0:exact=1,scale=80:64:flags=lanczos:"));
        assert!(filter.contains(",format=yuv420p10le,split[jesses_content][jesses_canvas];"));
        assert!(filter.contains(
            "[jesses_canvas]pad=136:88:32:8:color=black,lutyuv=y=64:u=512:v=512[jesses_background];"
        ));
        assert!(filter.ends_with(
            "[jesses_background][jesses_content]overlay=x=32:y=8:format=auto:shortest=1,setsar=1"
        ));
    }

    #[test]
    fn border_black_values_are_explicit_for_each_depth_and_range_without_resizing_content() {
        let geometry = Geometry::build(
            128,
            96,
            VideoFraming {
                borders: BorderSettings {
                    top: 16,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        for (pixel_format, full, luma, chroma) in [
            ("yuv420p", false, 16, 128),
            ("yuv420p", true, 0, 128),
            ("yuv420p10le", false, 64, 512),
            ("yuv420p10le", true, 0, 512),
        ] {
            let filter = geometry.filter(1, full, "left", pixel_format).unwrap();
            assert!(!filter.contains("scale="));
            assert!(filter.starts_with(&format!(
                "format={pixel_format},split[jesses_content][jesses_canvas];[jesses_canvas]pad=128:112:0:16:color=black,"
            )));
            assert!(filter.contains(&format!("lutyuv=y={luma}:u={chroma}:v={chroma}")));
        }
    }

    #[test]
    fn borders_reject_odd_edges_oversize_and_checked_addition_overflow() {
        for borders in [
            BorderSettings {
                left: 1,
                right: 1,
                ..Default::default()
            },
            BorderSettings {
                top: 1,
                bottom: 1,
                ..Default::default()
            },
        ] {
            let settings = EncodeSettings {
                framing: VideoFraming {
                    borders,
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(
                validate(&settings)
                    .unwrap_err()
                    .message
                    .contains("Border edges")
            );
        }
        for borders in [
            BorderSettings {
                left: 8066,
                ..Default::default()
            },
            BorderSettings {
                bottom: 8098,
                ..Default::default()
            },
            BorderSettings {
                left: u32::MAX - 1,
                right: 2,
                ..Default::default()
            },
            BorderSettings {
                top: u32::MAX - 1,
                bottom: u32::MAX - 1,
                ..Default::default()
            },
        ] {
            let framing = VideoFraming {
                borders,
                ..Default::default()
            };
            assert_eq!(
                Geometry::build(128, 96, framing).unwrap_err().code,
                "ENCODE_SETTINGS_INVALID"
            );
        }
        let exact_limit = Geometry::build(
            128,
            96,
            VideoFraming {
                borders: BorderSettings {
                    left: 8064,
                    bottom: 8096,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((exact_limit.width, exact_limit.height), (8192, 8192));
        assert!(
            validate(&EncodeSettings {
                backend: EncodeBackend::Av1an,
                framing: VideoFraming {
                    borders: BorderSettings {
                        left: 2,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            })
            .is_ok()
        );
    }

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
                ..Default::default()
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
                ..Default::default()
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
    fn invalid_edges_dimensions_and_overflow_are_rejected_for_both_backends() {
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
                        resize_width: None,
                        ..Default::default()
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
            validate(&EncodeSettings {
                backend: EncodeBackend::Av1an,
                framing,
                ..Default::default()
            })
            .unwrap();
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
            assert_eq!(geometry.filter(1, false, "left", "yuv420p10le"), None);
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
                ..Default::default()
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
                    let filter = geometry
                        .filter(matrix_id, full, chroma, "yuv420p10le")
                        .unwrap();
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
