use media_core::{AppError, AspectRatioKind, AspectRatioSettings};

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("TEMPORAL_SETTINGS_INVALID", message, None)
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

fn ratio(numerator: u64, denominator: u64) -> Result<(u32, u32), AppError> {
    if numerator == 0 || denominator == 0 {
        return Err(invalid("Aspect-ratio terms must be positive."));
    }
    let divisor = gcd(numerator, denominator);
    let numerator = u32::try_from(numerator / divisor)
        .map_err(|_| invalid("The resulting sample aspect ratio is too large."))?;
    let denominator = u32::try_from(denominator / divisor)
        .map_err(|_| invalid("The resulting sample aspect ratio is too large."))?;
    Ok((numerator, denominator))
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Transform {
    settings: AspectRatioSettings,
    sar: (u32, u32),
}

impl Transform {
    pub fn build(
        settings: AspectRatioSettings,
        output_width: u32,
        output_height: u32,
    ) -> Result<Self, AppError> {
        let sar = match settings.kind {
            AspectRatioKind::Sample => ratio(
                u64::from(settings.numerator),
                u64::from(settings.denominator),
            )?,
            AspectRatioKind::Display => ratio(
                u64::from(settings.numerator) * u64::from(output_height),
                u64::from(settings.denominator) * u64::from(output_width),
            )?,
        };
        if sar.0 > 65_535 || sar.1 > 65_535 {
            return Err(invalid(
                "The requested display ratio cannot be represented as a bounded sample aspect ratio for this frame size.",
            ));
        }
        Ok(Self { settings, sar })
    }

    pub fn filter(self) -> String {
        match self.settings.kind {
            AspectRatioKind::Sample => format!(
                "setsar={}/{}:max=65535",
                self.settings.numerator, self.settings.denominator
            ),
            AspectRatioKind::Display => format!(
                "setdar={}/{}:max=65535",
                self.settings.numerator, self.settings.denominator
            ),
        }
    }

    pub fn sar(self) -> String {
        format!("{}:{}", self.sar.0, self.sar.1)
    }
}

pub(super) fn parse_source(value: Option<&str>) -> Result<String, AppError> {
    let value = value.ok_or_else(|| invalid("The source sample aspect ratio is missing."))?;
    let (numerator, denominator) = value
        .split_once(':')
        .ok_or_else(|| invalid("The source sample aspect ratio is invalid."))?;
    let numerator = numerator
        .parse::<u64>()
        .map_err(|_| invalid("The source sample aspect ratio is invalid."))?;
    let denominator = denominator
        .parse::<u64>()
        .map_err(|_| invalid("The source sample aspect ratio is invalid."))?;
    let (numerator, denominator) = ratio(numerator, denominator)?;
    Ok(format!("{numerator}:{denominator}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_ratio_derives_exact_reduced_sar() {
        let transform = Transform::build(
            AspectRatioSettings {
                kind: AspectRatioKind::Display,
                numerator: 16,
                denominator: 9,
            },
            720,
            576,
        )
        .unwrap();
        assert_eq!(transform.sar(), "64:45");
        assert_eq!(transform.filter(), "setdar=16/9:max=65535");
        assert_eq!(parse_source(Some("16:12")).unwrap(), "4:3");
    }
}
