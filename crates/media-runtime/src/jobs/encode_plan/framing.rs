use media_core::{AppError, BorderSettings, CropSettings, EncodeSettings, VideoFraming};

fn invalid(message: &str) -> AppError {
    AppError::new("ENCODE_SETTINGS_INVALID", message, None)
}

fn valid_dimension(value: u32) -> bool {
    (2..=16384).contains(&value) && value.is_multiple_of(2)
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
            "Resize width must be an even number from 2 to 16384 pixels.",
        ));
    }
    if let Some(resize) = settings.framing.resize
        && (!(2..=256).contains(&resize.modulus)
            || !resize.modulus.is_multiple_of(2)
            || !(1..=800).contains(&resize.percent)
            || !(2..=16384).contains(&resize.width)
            || !(2..=16384).contains(&resize.height))
    {
        return Err(invalid(
            "Resize targets must be 2–16384 pixels, percentage 1–800, and alignment an even number from 2–256.",
        ));
    }
    if settings.framing.border_ratio.is_some_and(|ratio| {
        ratio.width == 0 || ratio.height == 0 || ratio.width > 10000 || ratio.height > 10000
    }) {
        return Err(invalid("Border ratio terms must be from 1 to 10000."));
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
    post_crop: Option<(u32, u32)>,
    crop: CropSettings,
    crop_width: u32,
    crop_height: u32,
}

impl Geometry {
    /// Image encoders consume RGB/alpha or JPEG YUV directly. Reuse the exact
    /// geometry calculation without forcing video-only planar YUV padding.
    pub(super) fn image_filter_with_text(&self, text: Option<&str>) -> Option<String> {
        let mut filters = Vec::new();
        if self.crop != CropSettings::default() {
            filters.push(format!(
                "crop={}:{}:{}:{}:exact=1",
                self.crop_width, self.crop_height, self.crop.left, self.crop.top
            ));
        }
        if self.content_width != self.crop_width || self.content_height != self.crop_height {
            let kernel = if self.resize_filter == media_core::ResizeFilter::Nearest {
                "neighbor".to_owned()
            } else {
                format!("{:?}", self.resize_filter).to_lowercase()
            };
            filters.push(format!(
                "scale={}:{}:flags={kernel}",
                self.content_width, self.content_height
            ));
        }
        if let Some((w, h)) = self.post_crop {
            filters.push(format!(
                "crop={w}:{h}:{}:{}:exact=1",
                (self.content_width - w) / 4 * 2,
                (self.content_height - h) / 4 * 2
            ));
        }
        if let Some(text) = text {
            filters.push(text.to_owned());
        }
        if self.borders != BorderSettings::default() {
            filters.push(format!(
                "pad={}:{}:{}:{}:color=black",
                self.width, self.height, self.borders.left, self.borders.top
            ));
        }
        if !filters.is_empty() {
            filters.push("setsar=1".into());
            Some(filters.join(","))
        } else {
            None
        }
    }
    #[cfg(test)]
    pub(super) fn build(width: u32, height: u32, framing: VideoFraming) -> Result<Self, AppError> {
        Self::build_with_sar(width, height, framing, (1, 1))
    }

    pub(super) fn build_with_sar(
        width: u32,
        height: u32,
        framing: VideoFraming,
        sar: (u32, u32),
    ) -> Result<Self, AppError> {
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
                "The cropped dimensions must be even and between 2 and 16384 pixels.",
            ));
        }
        let mut borders = BorderSettings::default();
        let mut post_crop = None;
        let (content_width, content_height, frame_width, frame_height) = if let Some(resize) =
            framing.resize
        {
            use media_core::{ResizeFill as Fill, ResizeMode as Mode};
            let modulus = resize.modulus as f64;
            let nearest = |value: f64| ((value / modulus).round().max(1.0) * modulus) as u32;
            let down = |value: f64| ((value / modulus).floor().max(1.0) * modulus) as u32;
            let up = |value: f64| ((value / modulus).ceil().max(1.0) * modulus) as u32;
            let sw = crop_width as f64
                * if resize.correct_aspect {
                    sar.0 as f64 / sar.1 as f64
                } else {
                    1.0
                };
            let sh = crop_height as f64;
            let aspect = sw / sh;
            let from_width = |w: f64| {
                let w = nearest(w);
                (w, nearest(w as f64 / aspect))
            };
            let from_height = |h: f64| {
                let h = nearest(h);
                (nearest(h as f64 * aspect), h)
            };
            let fit = |bw: u32, bh: u32, allow: bool| {
                let mut scale = (bw as f64 / sw).min(bh as f64 / sh);
                if !allow {
                    scale = scale.min(1.0);
                }
                let mut size = if bw as f64 / sw <= bh as f64 / sh {
                    from_width(sw * scale)
                } else {
                    from_height(sh * scale)
                };
                if size.0 > bw {
                    size = from_width(down(bw as f64) as f64);
                }
                if size.1 > bh {
                    size = from_height(down(bh as f64) as f64);
                }
                size
            };
            let (mut bw, mut bh) = (resize.width, resize.height);
            if resize.mode == Mode::Fit && (sh > sw) != (bh > bw) {
                std::mem::swap(&mut bw, &mut bh);
            }
            let (cw, ch) = match resize.mode {
                Mode::Fit => fit(bw, bh, resize.allow_upscale),
                Mode::Width => from_width(if resize.allow_upscale {
                    bw as f64
                } else {
                    (bw as f64).min(sw)
                }),
                Mode::Height => from_height(if resize.allow_upscale {
                    bh as f64
                } else {
                    (bh as f64).min(sh)
                }),
                Mode::Percent => {
                    if sw < sh {
                        from_width(sw * resize.percent as f64 / 100.0)
                    } else {
                        from_height(sh * resize.percent as f64 / 100.0)
                    }
                }
                Mode::Exact => {
                    bw = nearest(bw as f64);
                    bh = nearest(bh as f64);
                    match resize.fill {
                        Fill::Stretch => (bw, bh),
                        Fill::Pad => fit(bw, bh, resize.allow_upscale),
                        Fill::Crop => {
                            if bw as f64 / sw >= bh as f64 / sh {
                                let w = up(bw as f64);
                                (w, up(w as f64 / aspect).max(bh))
                            } else {
                                let h = up(bh as f64);
                                (up(h as f64 * aspect).max(bw), h)
                            }
                        }
                    }
                }
            };
            if resize.mode == Mode::Exact && resize.fill == Fill::Pad {
                let x = (bw - cw) / 4 * 2;
                let y = (bh - ch) / 4 * 2;
                borders.left = borders
                    .left
                    .checked_add(x)
                    .ok_or_else(|| invalid("Border overflow."))?;
                borders.right = borders
                    .right
                    .checked_add(bw - cw - x)
                    .ok_or_else(|| invalid("Border overflow."))?;
                borders.top = borders
                    .top
                    .checked_add(y)
                    .ok_or_else(|| invalid("Border overflow."))?;
                borders.bottom = borders
                    .bottom
                    .checked_add(bh - ch - y)
                    .ok_or_else(|| invalid("Border overflow."))?;
                (cw, ch, cw, ch)
            } else if resize.mode == Mode::Exact && resize.fill == Fill::Crop {
                post_crop = Some((bw, bh));
                (cw, ch, bw, bh)
            } else {
                (cw, ch, cw, ch)
            }
        } else {
            let w = framing.resize_width.unwrap_or(crop_width);
            let h = ((crop_height as u64 * w as u64 + crop_width as u64) / (crop_width as u64 * 2)
                * 2) as u32;
            (w, h, w, h)
        };
        if !valid_dimension(content_width) || !valid_dimension(content_height) {
            return Err(invalid(
                "The resized dimensions must be even and between 2 and 16384 pixels.",
            ));
        }
        if let Some(ratio) = framing.border_ratio {
            let fw = frame_width
                .checked_add(borders.left)
                .and_then(|v| v.checked_add(borders.right))
                .ok_or_else(|| invalid("Border overflow."))?;
            let fh = frame_height
                .checked_add(borders.top)
                .and_then(|v| v.checked_add(borders.bottom))
                .ok_or_else(|| invalid("Border overflow."))?;
            let pixel_aspect = if framing.resize.is_none() && framing.resize_width.is_none() {
                sar.0 as f64 / sar.1 as f64
            } else {
                1.0
            };
            let ideal_w = fh as f64 * ratio.width as f64 / ratio.height as f64 / pixel_aspect;
            let ideal_h = fw as f64 * pixel_aspect * ratio.height as f64 / ratio.width as f64;
            let (horizontal, ideal, current) = if ideal_w > fw as f64 {
                (true, ideal_w, fw)
            } else {
                (false, ideal_h, fh)
            };
            let target = (ideal / 2.0).round() as u32 * 2;
            let growth = target.saturating_sub(current);
            if growth >= 4 {
                let near = growth / 4 * 2;
                let far = growth - near;
                if horizontal {
                    borders.left = borders
                        .left
                        .checked_add(near)
                        .ok_or_else(|| invalid("Border overflow."))?;
                    borders.right = borders
                        .right
                        .checked_add(far)
                        .ok_or_else(|| invalid("Border overflow."))?;
                } else {
                    borders.top = borders
                        .top
                        .checked_add(near)
                        .ok_or_else(|| invalid("Border overflow."))?;
                    borders.bottom = borders
                        .bottom
                        .checked_add(far)
                        .ok_or_else(|| invalid("Border overflow."))?;
                }
            }
        }
        borders.top = borders
            .top
            .checked_add(framing.borders.top)
            .ok_or_else(|| invalid("Border overflow."))?;
        borders.right = borders
            .right
            .checked_add(framing.borders.right)
            .ok_or_else(|| invalid("Border overflow."))?;
        borders.bottom = borders
            .bottom
            .checked_add(framing.borders.bottom)
            .ok_or_else(|| invalid("Border overflow."))?;
        borders.left = borders
            .left
            .checked_add(framing.borders.left)
            .ok_or_else(|| invalid("Border overflow."))?;
        let padded =
            |content: u32, first: u32, second: u32| content.checked_add(first)?.checked_add(second);
        let (Some(output_width), Some(output_height)) = (
            padded(frame_width, borders.left, borders.right),
            padded(frame_height, borders.top, borders.bottom),
        ) else {
            return Err(invalid(
                "The requested borders exceed the supported dimensions.",
            ));
        };
        if !valid_dimension(output_width) || !valid_dimension(output_height) {
            return Err(invalid(
                "The dimensions including borders must be even and between 2 and 16384 pixels.",
            ));
        }
        if u64::from(output_width) * u64::from(output_height) > 260_000_000 {
            return Err(invalid(
                "The output exceeds FFmpeg's safe 260-megapixel frame limit.",
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
            post_crop,
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
                media_core::ResizeFilter::Spline => "spline",
                media_core::ResizeFilter::Area => "area",
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
        if let Some((width, height)) = self.post_crop {
            let left = (self.content_width - width) / 4 * 2;
            let top = (self.content_height - height) / 4 * 2;
            filters.push(format!("crop={width}:{height}:{left}:{top}:exact=1"));
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
            let depth_shift = if pixel_format.ends_with("10le") { 2 } else { 0 };
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
    fn resize_intent_handles_orientation_anamorphic_exact_fill_and_ratio_borders() {
        use media_core::{BorderRatio, ResizeFill as Fill, ResizeMode as Mode, ResizeSettings};
        let geometry = |w, h, resize| {
            Geometry::build(
                w,
                h,
                VideoFraming {
                    resize: Some(resize),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let fit = ResizeSettings {
            allow_upscale: true,
            ..Default::default()
        };
        let g = geometry(3840, 1608, fit);
        assert_eq!((g.width, g.height), (1920, 804));
        let g = geometry(720, 1280, fit);
        assert_eq!((g.width, g.height), (1080, 1920));
        let g = geometry(640, 480, ResizeSettings::default());
        assert_eq!((g.width, g.height), (640, 480));
        let g = geometry(
            1280,
            720,
            ResizeSettings {
                mode: Mode::Percent,
                ..fit
            },
        );
        assert_eq!((g.width, g.height), (640, 360));
        let g = Geometry::build_with_sar(
            720,
            480,
            VideoFraming {
                resize: Some(ResizeSettings {
                    mode: Mode::Height,
                    height: 480,
                    ..fit
                }),
                ..Default::default()
            },
            (32, 27),
        )
        .unwrap();
        assert_eq!((g.width, g.height), (854, 480));
        let g = geometry(
            1280,
            960,
            ResizeSettings {
                mode: Mode::Exact,
                fill: Fill::Pad,
                ..fit
            },
        );
        assert_eq!((g.width, g.height), (1920, 1080));
        assert_eq!((g.content_width, g.content_height), (1440, 1080));
        assert_eq!(g.borders.left, 240);
        let g = geometry(
            1280,
            960,
            ResizeSettings {
                mode: Mode::Exact,
                fill: Fill::Crop,
                ..fit
            },
        );
        assert_eq!((g.width, g.height), (1920, 1080));
        assert_eq!((g.content_width, g.content_height), (1920, 1440));
        assert!(
            g.filter(1, false, "left", "yuv420p")
                .unwrap()
                .contains("crop=1920:1080:0:180:exact=1")
        );
        let g = geometry(
            1280,
            960,
            ResizeSettings {
                mode: Mode::Exact,
                fill: Fill::Stretch,
                ..fit
            },
        );
        assert_eq!((g.width, g.height), (1920, 1080));
        assert!(g.post_crop.is_none());
        assert_eq!(g.borders, BorderSettings::default());
        let g = Geometry::build(
            1920,
            804,
            VideoFraming {
                border_ratio: Some(BorderRatio {
                    width: 16,
                    height: 9,
                }),
                borders: BorderSettings {
                    top: 8,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((g.width, g.height), (1920, 1088));
        assert_eq!((g.borders.top, g.borders.bottom), (146, 138));
        let g = geometry(1920, 1080, ResizeSettings { modulus: 16, ..fit });
        assert!(g.width <= 1920 && g.height <= 1080);
        assert_eq!(g.width % 16, 0);
        assert_eq!(g.height % 16, 0);
    }

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
                resize: None,
                border_ratio: None,
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
                left: 16258,
                ..Default::default()
            },
            BorderSettings {
                bottom: 16290,
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
                resize_width: Some(16386),
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
        {
            let (width, height, output_width) = (64, 16384, 16384);
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
