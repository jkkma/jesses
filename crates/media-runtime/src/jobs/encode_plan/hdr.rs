//! Bounded HDR10 metadata handling. HEVC and AV1 use different fixed-point
//! precision for mastering display values; comparison accounts for one AV1 step.
use media_core::AppError;
use serde_json::Value;
use std::ffi::OsString;

use super::unsupported;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::jobs) struct Mastering {
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

    /// x265's HEVC SEI parser takes integer chromaticities in 0.00002 units
    /// and integer luminances in 0.0001 cd/m² units.
    pub fn x265_argument(&self) -> String {
        let v = self.values;
        let xy = |index: usize| (v[index] * 50_000.0).round() as u32;
        let luminance = |index: usize| (v[index] * 10_000.0).round() as u32;
        format!(
            "G({},{})B({},{})R({},{})WP({},{})L({},{})",
            xy(2),
            xy(3),
            xy(4),
            xy(5),
            xy(0),
            xy(1),
            xy(6),
            xy(7),
            luminance(8),
            luminance(9)
        )
    }

    pub fn mkvmerge_arguments(&self) -> Vec<OsString> {
        let v = self.values;
        vec![
            "--chromaticity-coordinates".into(),
            format!("0:{},{},{},{},{},{}", v[0], v[1], v[2], v[3], v[4], v[5]).into(),
            "--white-color-coordinates".into(),
            format!("0:{},{}", v[6], v[7]).into(),
            "--max-luminance".into(),
            format!("0:{}", v[8]).into(),
            "--min-luminance".into(),
            format!("0:{}", v[9]).into(),
        ]
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

#[cfg(test)]
mod formatting_tests {
    use super::*;

    #[test]
    fn x265_uses_hevc_fixed_point_mastering_units() {
        let mastering = Mastering::parse(&serde_json::json!({
            "red_x":"34000/50000", "red_y":"16000/50000",
            "green_x":"13250/50000", "green_y":"34500/50000",
            "blue_x":"7500/50000", "blue_y":"3000/50000",
            "white_point_x":"15635/50000", "white_point_y":"16450/50000",
            "max_luminance":"10000000/10000", "min_luminance":"1/10000"
        }))
        .unwrap();
        assert_eq!(
            mastering.x265_argument(),
            "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)"
        );
    }
}

#[derive(Clone, Debug, Default)]
pub(in crate::jobs) struct StaticMetadata {
    pub mastering: Option<Mastering>,
    pub light: Option<(u16, u16)>,
}

impl StaticMetadata {
    /// MaxCLL is the first declared ceiling; a 10,000-nit placeholder is not
    /// a measurement, so mastering luminance is used when available.
    pub fn declared_peak_nits(&self) -> Option<f64> {
        self.light
            .map(|light| f64::from(light.0))
            .filter(|value| *value > 0.0 && *value < 10_000.0)
            .or_else(|| {
                self.mastering
                    .as_ref()
                    .map(|mastering| mastering.values[8])
                    .filter(|value| *value > 0.0 && *value < 10_000.0)
            })
    }

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

#[cfg(test)]
mod rpu_tests {
    use super::*;
    #[test]
    fn profile8_gpu_requires_complete_single_layer_rpu_on_each_frame() {
        let record = serde_json::json!({"side_data_type":"DOVI configuration record","dv_profile":8,"dv_bl_signal_compatibility_id":1,"bl_present_flag":1,"rpu_present_flag":1,"el_present_flag":0});
        let hdr = Hdr10::build(std::slice::from_ref(&record), true, false).unwrap();
        let data = serde_json::json!({"side_data_type":"Dolby Vision RPU Data"});
        let metadata = serde_json::json!({"side_data_type":"Dolby Vision Metadata","bl_bit_depth":10,"bl_video_full_range_flag":0,"disable_residual_flag":1});
        assert!(
            hdr.require_gpu_rpu(&[data.clone(), metadata.clone()])
                .is_ok()
        );
        assert!(hdr.require_gpu_rpu(std::slice::from_ref(&data)).is_err());
        assert!(
            hdr.require_gpu_rpu(std::slice::from_ref(&metadata))
                .is_err()
        );
        for (field, value) in [
            ("bl_bit_depth", 8),
            ("bl_video_full_range_flag", 1),
            ("disable_residual_flag", 0),
        ] {
            let mut malformed = metadata.clone();
            malformed[field] = value.into();
            assert!(hdr.require_gpu_rpu(&[data.clone(), malformed]).is_err());
        }
        let mut dual = record;
        dual["el_present_flag"] = 1.into();
        assert!(Hdr10::build(&[dual], true, false).is_err());
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
    fn parse(value: &Value, allow_profile5: bool) -> Result<Self, AppError> {
        let number = |key| value.get(key).and_then(Value::as_u64);
        let profile = number("dv_profile");
        let compatibility = number("dv_bl_signal_compatibility_id");
        if !matches!(
            (profile, compatibility),
            (Some(7), Some(6)) | (Some(8), Some(1))
        ) && !(allow_profile5 && (profile, compatibility) == (Some(5), Some(0)))
            || number("bl_present_flag") != Some(1)
            || number("rpu_present_flag") != Some(1)
            || (profile == Some(7) && number("el_present_flag") != Some(1))
            || (matches!(profile, Some(5 | 8)) && number("el_present_flag") != Some(0))
        {
            return Err(unsupported(if allow_profile5 {
                "Dolby Vision rendering requires profile 5/compatibility 0 with an intact single-layer RPU, or a confirmed profile 7/8 HDR10 base layer with its required layer flags."
            } else {
                "HDR10 fallback requires a confirmed Dolby Vision profile 7/compatibility 6 or profile 8/compatibility 1 HDR10 base layer. Profile 5 requires GPU/RPU rendering instead."
            }));
        }
        Ok(Self {
            profile: profile.unwrap(),
            compatibility: compatibility.unwrap(),
        })
    }

    fn is_profile5(&self) -> bool {
        self.profile == 5
    }
}

#[derive(Clone, Debug)]
pub(super) struct Hdr10 {
    pub metadata: StaticMetadata,
    pub discard_dynamic: bool,
    pub dolby: Option<DolbyVision>,
    pub allow_profile5: bool,
    pub imported_dolby: bool,
    pub imported_hdr10plus: bool,
}

impl Hdr10 {
    pub fn build(
        values: &[Value],
        discard_dynamic: bool,
        allow_profile5: bool,
    ) -> Result<Self, AppError> {
        let mut dolby = None;
        for value in values
            .iter()
            .filter(|v| kind(v) == "DOVI configuration record")
        {
            if dolby
                .replace(DolbyVision::parse(value, allow_profile5)?)
                .is_some()
            {
                return Err(unsupported(
                    "Duplicate Dolby Vision configuration records are ambiguous.",
                ));
            }
        }
        let hdr = Self {
            metadata: StaticMetadata::parse(values)?,
            discard_dynamic,
            dolby,
            allow_profile5,
            imported_dolby: false,
            imported_hdr10plus: false,
        };
        validate_side_data(values, Some(&hdr), false)?;
        Ok(hdr)
    }

    pub fn require_profile5_rpu(&self, values: &[Value]) -> Result<(), AppError> {
        if self.dolby.as_ref().is_some_and(DolbyVision::is_profile5) {
            let has_rpu = values
                .iter()
                .any(|value| kind(value) == "Dolby Vision RPU Data");
            let has_parsed_metadata = values.iter().any(|value| {
                kind(value) == "Dolby Vision Metadata"
                    && value.get("bl_bit_depth").and_then(Value::as_u64) == Some(10)
                    && value
                        .get("bl_video_full_range_flag")
                        .and_then(Value::as_u64)
                        == Some(1)
            });
            if !has_rpu || !has_parsed_metadata {
                return Err(unsupported(
                    "Dolby Vision profile 5 needs both intact RPU data and parsed full-range 10-bit Dolby metadata on every decoded frame. Its IPT picture cannot be rendered safely without both.",
                ));
            }
        }
        Ok(())
    }

    pub fn require_gpu_rpu(&self, values: &[Value]) -> Result<(), AppError> {
        if self.dolby.as_ref().is_some_and(DolbyVision::is_profile5) {
            return self.require_profile5_rpu(values);
        }
        if !self
            .dolby
            .as_ref()
            .is_some_and(|dolby| dolby.profile == 8 && dolby.compatibility == 1)
            || !values
                .iter()
                .any(|value| kind(value) == "Dolby Vision RPU Data")
            || !values.iter().any(|value| {
                kind(value) == "Dolby Vision Metadata"
                    && value.get("bl_bit_depth").and_then(Value::as_u64) == Some(10)
                    && value
                        .get("bl_video_full_range_flag")
                        .and_then(Value::as_u64)
                        == Some(0)
                    && value.get("disable_residual_flag").and_then(Value::as_u64) == Some(1)
            })
        {
            return Err(unsupported(
                "Single-layer Dolby Vision8.1 GPU rendering requires intact RPU data and parsed limited-range10-bit metadata without an enhancement-layer residual on every decoded frame.",
            ));
        }
        Ok(())
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
            "DOVI configuration record" | "Dolby Vision RPU Data" | "Dolby Vision Metadata"
                if encoded && hdr.is_some_and(|hdr| hdr.imported_dolby) => {}
            "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"
                if encoded && hdr.is_some_and(|hdr| hdr.imported_hdr10plus) => {}
            "Mastering display metadata" | "Content light level metadata" if hdr.is_some() => {}
            "DOVI configuration record"
            | "Dolby Vision RPU Data"
            | "Dolby Vision Metadata"
            | "HEVC enhancement-layer decoder configuration" => {
                let hdr = hdr.filter(|hdr| hdr.discard_dynamic && hdr.dolby.is_some() && !encoded).ok_or_else(|| unsupported("Dolby Vision requires supported single-layer GPU/RPU rendering or explicit HDR10 fallback using a compatible profile7/8 base layer."))?;
                if kind(value) == "DOVI configuration record"
                    && Some(DolbyVision::parse(value, hdr.allow_profile5)?) != hdr.dolby
                {
                    return Err(unsupported(
                        "Dolby Vision configuration changed during decoding.",
                    ));
                }
                if kind(value) == "Dolby Vision Metadata" {
                    for (key, expected) in [
                        ("bl_bit_depth", 10),
                        (
                            "bl_video_full_range_flag",
                            u64::from(hdr.dolby.as_ref().is_some_and(DolbyVision::is_profile5)),
                        ),
                    ] {
                        if value.get(key).and_then(Value::as_u64) != Some(expected) {
                            return Err(unsupported(
                                if hdr.dolby.as_ref().is_some_and(DolbyVision::is_profile5) {
                                    "Dolby Vision profile 5 decoded metadata does not confirm a full-range 10-bit IPT base layer."
                                } else {
                                    "Dolby Vision decoded metadata does not confirm a limited-range 10-bit HDR10 base layer."
                                },
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
                return Err(unsupported(&format!(
                    "Unsupported video side data '{}' was found. HDR10+ requires explicit HDR10 fallback; display transforms and unknown rendering metadata are unsupported.",
                    kind(value)
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn authored(
    value: &media_core::HdrMetadataSettings,
) -> Result<StaticMetadata, AppError> {
    let mut fields = Vec::new();
    if let Some(mastering) = value.mastering_display {
        fields.push(serde_json::json!({"side_data_type":"Mastering display metadata",
            "red_x":format!("{}/50000",mastering.red.x),"red_y":format!("{}/50000",mastering.red.y),
            "green_x":format!("{}/50000",mastering.green.x),"green_y":format!("{}/50000",mastering.green.y),
            "blue_x":format!("{}/50000",mastering.blue.x),"blue_y":format!("{}/50000",mastering.blue.y),
            "white_point_x":format!("{}/50000",mastering.white_point.x),"white_point_y":format!("{}/50000",mastering.white_point.y),
            "max_luminance":format!("{}/10000",mastering.max_luminance_ten_thousandths),"min_luminance":format!("{}/10000",mastering.min_luminance_ten_thousandths)}));
    }
    if let Some(light) = value.content_light {
        fields.push(serde_json::json!({"side_data_type":"Content light level metadata","max_content":light.max_content,"max_average":light.max_average}));
    }
    StaticMetadata::parse(&fields)
}
impl StaticMetadata {
    pub(super) fn apply_authored(&mut self, authored: &Self) {
        if authored.mastering.is_some() {
            self.mastering = authored.mastering.clone();
        }
        if let Some(light) = authored.light {
            self.light = if light == (0, 0) { None } else { Some(light) };
        }
    }
}
