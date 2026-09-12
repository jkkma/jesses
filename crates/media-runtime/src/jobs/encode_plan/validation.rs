//! Constant-memory validation of every decoded frame. The declared cadence and
//! the one permitted decimal alternative are checked independently in one pass.
use super::*;

pub(in crate::jobs) struct Validation {
    declared: Cadence,
    alternative: Option<Cadence>,
    source: bool,
    first: bool,
    discovery_error: Option<AppError>,
}

impl Validation {
    pub(in crate::jobs) fn new(plan: Plan, stream: Stream, encoded: bool) -> Self {
        Self {
            declared: Cadence::new(plan, stream, encoded),
            alternative: None,
            source: !encoded,
            first: true,
            discovery_error: None,
        }
    }

    #[cfg(test)]
    pub(super) fn exact(plan: Plan, stream: Stream, encoded: bool) -> Self {
        Self {
            source: false,
            ..Self::new(plan, stream, encoded)
        }
    }

    pub(in crate::jobs) fn push(&mut self, frame: &Frame) {
        if self.first {
            self.first = false;
            if self.source {
                self.discovery_error = self.discover(frame).err();
                if self.discovery_error.is_none()
                    && u64::from(self.declared.plan.fps_num) * 1001
                        == u64::from(self.declared.plan.fps_den) * 24000
                {
                    let mut candidate = self.declared.plan.clone();
                    candidate.fps_num = 2997;
                    candidate.fps_den = 125;
                    candidate.cadence_reconciled = true;
                    self.alternative =
                        Some(Cadence::new(candidate, self.declared.stream.clone(), false));
                }
            }
        }
        self.declared.push(frame);
        if let Some(alternative) = &mut self.alternative {
            alternative.push(frame);
        }
    }

    fn discover(&mut self, first: &Frame) -> Result<(), AppError> {
        if let Some(hdr) = &mut self.declared.plan.hdr10 {
            hdr.metadata
                .absorb_first_frame(&StaticMetadata::parse(&first.side_data_list)?)?;
            if hdr.metadata.mastering.is_none() {
                return Err(unsupported(
                    "HDR10 encoding requires valid mastering display metadata in the stream or first decoded frame.",
                ));
            }
        }
        Ok(())
    }

    pub(in crate::jobs) fn progress_seconds(&self) -> f64 {
        self.declared.count as f64 * self.declared.plan.frame_seconds()
    }

    pub(in crate::jobs) fn finish(self) -> Result<(Plan, usize), AppError> {
        if let Some(error) = self.discovery_error {
            return Err(error);
        }
        match self.declared.finish() {
            Ok(result) => Ok(result),
            Err(declared_error) => self
                .alternative
                .and_then(|candidate| candidate.finish().ok())
                .ok_or(declared_error),
        }
    }
}

struct Cadence {
    plan: Plan,
    stream: Stream,
    encoded: bool,
    tolerance: f64,
    count: usize,
    observed_hdr: StaticMetadata,
    error: Option<AppError>,
}

impl Cadence {
    fn new(plan: Plan, stream: Stream, encoded: bool) -> Self {
        // Output containers may quantize a fine source time base to milliseconds.
        let tolerance = if encoded {
            rational(stream.time_base.as_deref())
                .map(|(n, d)| f64::from(n) / f64::from(d))
                .unwrap_or(0.000001)
                .clamp(0.000001, 0.001)
                + 0.000002
        } else {
            plan.tolerance
        };
        Self {
            plan,
            stream,
            encoded,
            tolerance,
            count: 0,
            observed_hdr: StaticMetadata::default(),
            error: None,
        }
    }

    fn push(&mut self, frame: &Frame) {
        if self.error.is_none() {
            self.error = self.validate(frame).err();
        }
        // Continue parsing the complete document after a semantic failure. This
        // preserves decode/JSON failure precedence and never trusts a valid prefix.
        match self.count.checked_add(1) {
            Some(count) => self.count = count,
            None => {
                self.error = Some(unsupported(
                    "The decoded frame count exceeds the supported range.",
                ))
            }
        }
    }

    fn validate(&mut self, frame: &Frame) -> Result<(), AppError> {
        if self.count == 0 {
            self.observed_hdr = StaticMetadata::parse(&self.stream.side_data_list)?;
            validate_side_data(
                &self.stream.side_data_list,
                self.plan.hdr10.as_ref(),
                self.encoded,
            )?;
        }
        let time = seconds(frame.best_effort_timestamp_time.as_deref())
            .ok_or_else(|| unsupported("A decoded frame has no usable timestamp."))?;
        if (time - self.count as f64 * self.plan.frame_seconds()).abs() > self.tolerance {
            return Err(unsupported(
                "Decoded frame timestamps are not constant-rate starting at zero. VFR and timestamp gaps require a later workflow.",
            ));
        }
        let (width, height) = self.plan.frame_dimensions(self.encoded);
        if frame.interlaced_frame != Some(0)
            || frame.width != Some(width)
            || frame.height != Some(height)
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
            != normalize_chroma(self.stream.chroma_location.as_deref())
        {
            return Err(unsupported(
                "Decoded frame chroma placement differs from the selected source.",
            ));
        }
        if if self.encoded {
            !self.plan.matches_output_format(frame.pix_fmt.as_deref())
        } else {
            frame.pix_fmt != self.stream.pix_fmt
        } {
            return Err(unsupported(
                "The decoded bit depth, pixel format, or frame side data changed unexpectedly.",
            ));
        }
        validate_side_data(
            &frame.side_data_list,
            self.plan.hdr10.as_ref(),
            self.encoded,
        )?;
        if let Some(hdr) = &self.plan.hdr10 {
            let actual = StaticMetadata::parse(&frame.side_data_list)?;
            hdr.metadata.validate_present(&actual, self.encoded)?;
            if actual.mastering.is_some() {
                self.observed_hdr.mastering = actual.mastering;
            }
            if actual.light.is_some() {
                self.observed_hdr.light = actual.light;
            }
        }
        for (actual, expected) in [
            (&frame.color_space, &self.stream.color_space),
            (&frame.color_transfer, &self.stream.color_transfer),
            (&frame.color_primaries, &self.stream.color_primaries),
            (&frame.color_range, &self.stream.color_range),
        ] {
            if actual != expected {
                return Err(unsupported(
                    "Decoded frame color metadata differs from the selected source.",
                ));
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<(Plan, usize), AppError> {
        if self.count == 0 {
            return Err(unsupported("The video did not decode to any frames."));
        }
        if let Some(error) = self.error {
            return Err(error);
        }
        if let Some(hdr) = &self.plan.hdr10 {
            hdr.metadata
                .validate_present(&self.observed_hdr, self.encoded)?;
            if self.encoded
                && (self.observed_hdr.mastering.is_none()
                    || (hdr.metadata.light.is_some() && self.observed_hdr.light.is_none()))
            {
                return Err(unsupported(
                    "The encoded video is missing planned HDR10 mastering or content light metadata.",
                ));
            }
        }
        Ok((self.plan, self.count))
    }
}
