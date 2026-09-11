//! Bounded HDR10 metadata handling. HEVC and AV1 use different fixed-point
//! precision for mastering display values; comparison accounts for one AV1 step.
use media_core::AppError;
use serde_json::Value;

use super::unsupported;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Mastering {
    // R, G, B, white point x/y; max and min luminance in cd/m2.
    values: [f64; 10],
}

fn rational(value: &Value, key: &str) -> Result<f64, AppError> {
    let parsed = value.get(key).and_then(Value::as_str).and_then(|text| {
        let (num, den) = text.split_once('/')?;
        let num = num.parse::<u32>().ok()?;
        let den = den.parse::<u32>().ok()?;
        (den > 0).then_some(f64::from(num) / f64::from(den))
    });
    parsed
        .filter(|v| v.is_finite() && *v >= 0.0)
        .ok_or_else(|| {
            unsupported("HDR10 mastering metadata contains a missing or invalid rational value.")
        })
}

impl Mastering {
    fn parse(value: &Value) -> Result<Self, AppError> {
        let mut values = [0.0; 10];
        for (index, name) in [
            "red_x",
            "red_y",
            "green_x",
            "green_y",
            "blue_x",
            "blue_y",
            "white_point_x",
            "white_point_y",
            "max_luminance",
            "min_luminance",
        ]
        .iter()
        .enumerate()
        {
            values[index] = rational(value, name)?;
        }
        let triangle = (values[2] - values[0]) * (values[5] - values[1])
            - (values[4] - values[0]) * (values[3] - values[1]);
        if values[..8].iter().any(|v| *v >= 1.0)
            || values[..8]
                .as_chunks::<2>()
                .0
                .iter()
                .any(|xy| xy[0] + xy[1] > 1.000001)
            || triangle.abs() < 1e-8
            || values[6] <= 0.0
            || values[7] <= 0.0
            || values[8] <= 0.0
            || values[8] > 10_000.0
            || values[9] >= values[8]
        {
            return Err(unsupported(
                "HDR10 mastering coordinates or luminance are out of range.",
            ));
        }
        Ok(Self { values })
    }

    pub fn argument(&self) -> String {
        let v = self.values;
        format!(
            "G({:.10},{:.10})B({:.10},{:.10})R({:.10},{:.10})WP({:.10},{:.10})L({:.10},{:.10})",
            v[2], v[3], v[4], v[5], v[0], v[1], v[6], v[7], v[8], v[9]
        )
    }

    fn matches(&self, actual: &Self, encoded: bool) -> bool {
        self.values
            .iter()
            .zip(actual.values)
            .enumerate()
            .all(|(i, (a, b))| {
                let tolerance = if encoded {
                    match i {
                        0..=7 => 1.0 / 65536.0,
                        8 => 1.0 / 256.0,
                        _ => 1.0 / 16384.0,
                    }
                } else {
                    0.0
                };
                (a - b).abs() <= tolerance + 1e-10
            })
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct StaticMetadata {
    pub mastering: Option<Mastering>,
    pub light: Option<(u16, u16)>,
}

impl StaticMetadata {
    pub fn parse(values: &[Value]) -> Result<Self, AppError> {
        let mut metadata = Self::default();
        for value in values {
            match kind(value) {
                "Mastering display metadata" => {
                    if metadata
                        .mastering
                        .replace(Mastering::parse(value)?)
                        .is_some()
                    {
                        return Err(unsupported(
                            "Duplicate HDR10 mastering metadata is ambiguous.",
                        ));
                    }
                }
                "Content light level metadata" => {
                    let field = |name| {
                        value
                            .get(name)
                            .and_then(Value::as_u64)
                            .and_then(|v| u16::try_from(v).ok())
                            .ok_or_else(|| {
                                unsupported(
                                    "HDR10 content light metadata is incomplete or out of range.",
                                )
                            })
                    };
                    let light = (field("max_content")?, field("max_average")?);
                    if (light.0 != 0 && light.1 > light.0)
                        || metadata.light.replace(light).is_some()
                    {
                        return Err(unsupported(
                            "HDR10 content light metadata is inconsistent or duplicated.",
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(metadata)
    }

    pub fn absorb_first_frame(&mut self, frame: &Self) -> Result<(), AppError> {
        if let (Some(a), Some(b)) = (&self.mastering, &frame.mastering)
            && !a.matches(b, false)
        {
            return Err(unsupported(
                "Stream and frame HDR10 mastering metadata disagree.",
            ));
        }
        if self.light.is_some() && frame.light.is_some() && self.light != frame.light {
            return Err(unsupported(
                "Stream and frame HDR10 content light metadata disagree.",
            ));
        }
        if self.mastering.is_none() {
            self.mastering = frame.mastering.clone();
        }
        if self.light.is_none() {
            self.light = frame.light;
        }
        Ok(())
    }

    pub fn validate_present(&self, actual: &Self, encoded: bool) -> Result<(), AppError> {
        if actual.mastering.as_ref().is_some_and(|b| {
            !self
                .mastering
                .as_ref()
                .is_some_and(|a| a.matches(b, encoded))
        }) || (actual.light.is_some() && actual.light != self.light)
        {
            return Err(unsupported(
                "HDR10 mastering or content light metadata changed unexpectedly.",
            ));
        }
        Ok(())
    }
}

pub(super) fn kind(value: &Value) -> &str {
    value
        .get("side_data_type")
        .and_then(Value::as_str)
        .unwrap_or("")
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct DolbyVision {
    profile: u64,
    compatibility: u64,
}

impl DolbyVision {
    fn parse(value: &Value) -> Result<Self, AppError> {
        let number = |key| value.get(key).and_then(Value::as_u64);
        let profile = number("dv_profile");
        let compatibility = number("dv_bl_signal_compatibility_id");
        if !matches!(
            (profile, compatibility),
            (Some(7), Some(6)) | (Some(8), Some(1))
        ) || number("bl_present_flag") != Some(1)
            || number("rpu_present_flag") != Some(1)
            || (profile == Some(7) && number("el_present_flag") != Some(1))
            || (profile == Some(8) && number("el_present_flag") != Some(0))
        {
            return Err(unsupported(
                "HDR10 fallback requires a confirmed Dolby Vision profile 7/compatibility 6 or profile 8/compatibility 1 HDR10 base layer. Other profiles, including profile 5, are unsupported.",
            ));
        }
        Ok(Self {
            profile: profile.unwrap(),
            compatibility: compatibility.unwrap(),
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct Hdr10 {
    pub metadata: StaticMetadata,
    pub discard_dynamic: bool,
    pub dolby: Option<DolbyVision>,
}

impl Hdr10 {
    pub fn build(values: &[Value], discard_dynamic: bool) -> Result<Self, AppError> {
        let mut dolby = None;
        for value in values
            .iter()
            .filter(|v| kind(v) == "DOVI configuration record")
        {
            if dolby.replace(DolbyVision::parse(value)?).is_some() {
                return Err(unsupported(
                    "Duplicate Dolby Vision configuration records are ambiguous.",
                ));
            }
        }
        let hdr = Self {
            metadata: StaticMetadata::parse(values)?,
            discard_dynamic,
            dolby,
        };
        validate_side_data(values, Some(&hdr), false)?;
        Ok(hdr)
    }
}

/// Explicit allowlist: do not silently discard unrecognized rendering metadata.
pub(super) fn validate_side_data(
    values: &[Value],
    hdr: Option<&Hdr10>,
    encoded: bool,
) -> Result<(), AppError> {
    for value in values {
        match kind(value) {
            "Mastering display metadata" | "Content light level metadata" if hdr.is_some() => {}
            "DOVI configuration record"
            | "Dolby Vision RPU Data"
            | "Dolby Vision Metadata"
            | "HEVC enhancement-layer decoder configuration" => {
                let hdr = hdr.filter(|hdr| hdr.discard_dynamic && hdr.dolby.is_some() && !encoded).ok_or_else(|| unsupported("Dolby Vision requires explicit HDR10 fallback with a compatible base layer; its dynamic metadata and enhancement layer will be discarded."))?;
                if kind(value) == "DOVI configuration record"
                    && Some(DolbyVision::parse(value)?) != hdr.dolby
                {
                    return Err(unsupported(
                        "Dolby Vision configuration changed during decoding.",
                    ));
                }
                if kind(value) == "Dolby Vision Metadata" {
                    for (key, expected) in [("bl_bit_depth", 10), ("bl_video_full_range_flag", 0)] {
                        if value.get(key).and_then(Value::as_u64) != Some(expected) {
                            return Err(unsupported(
                                "Dolby Vision decoded metadata does not confirm a limited-range 10-bit base layer.",
                            ));
                        }
                    }
                }
            }
            "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"
                if hdr.is_some_and(|h| h.discard_dynamic) && !encoded => {}
            "H.26[45] User Data Unregistered SEI message"
            | "H.264 User Data Unregistered SEI message"
            | "H.265 User Data Unregistered SEI message"
            | "SMPTE 12-1 timecode" => {}
            _ => {
                return Err(unsupported(
                    "Unsupported video side data was found. HDR10+ requires explicit HDR10 fallback; display transforms and unknown rendering metadata are unsupported.",
                ));
            }
        }
    }
    Ok(())
}
