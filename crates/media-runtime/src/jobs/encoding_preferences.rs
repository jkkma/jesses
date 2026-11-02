use super::encode_plan::Plan;
use media_core::{AppError, EncodeSettings};

pub(super) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    if settings
        .encoding_preferences
        .is_some_and(|value| !(1..=120).contains(&value.keyframe_interval_seconds))
    {
        return Err(AppError::new(
            "ENCODE_SETTINGS_INVALID",
            "The keyframe interval must be between 1 and 120 seconds.",
            None,
        ));
    }
    Ok(())
}

/// Match CodecUtils.GetKeyIntArg: post-filter frame rate, nearest whole frame,
/// and 12–480 bounds. An explicit advanced encoder override wins later.
pub(super) fn keyframes(plan: &Plan, settings: &EncodeSettings) -> Option<u32> {
    settings.encoding_preferences.map(|value| {
        keyframes_at_rate(
            (plan.fps_num, plan.fps_den),
            value.keyframe_interval_seconds,
        )
    })
}

pub(super) fn keyframes_at_rate(rate: (u32, u32), seconds: u16) -> u32 {
    ((rate.0 as f32 / rate.1 as f32 * f32::from(seconds)).round_ties_even() as u32).clamp(12, 480)
}
