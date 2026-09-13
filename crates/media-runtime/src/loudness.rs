//! Read-only full-track loudness measurement. Applying gain is a separate edit.
use crate::{
    analysis::{check_cancel, fingerprint, permit, tool},
    jobs::files::Source,
    supervisor::{CommandSpec, SupervisorError, run_capture},
};
use media_core::{AppError, AudioChannels, LoudnessRequest, LoudnessResult};
use serde::Deserialize;
use std::{ffi::OsString, path::Path, time::Duration};
use tokio::sync::watch;

#[derive(Deserialize)]
struct Measurement {
    input_i: String,
    input_tp: String,
    input_lra: String,
}

fn measurement(value: &str) -> Result<Option<f64>, String> {
    if value == "-inf" {
        return Ok(None);
    }
    let value: f64 = value.parse().map_err(|_| "Invalid loudness measurement.")?;
    if !value.is_finite() || !(-200.0..=200.0).contains(&value) {
        return Err("Loudness measurement is outside the supported range.".into());
    }
    Ok(Some(value))
}

fn parse(stderr: &[u8], request: &LoudnessRequest) -> Result<LoudnessResult, String> {
    let text = std::str::from_utf8(stderr).map_err(|_| "Invalid loudness output encoding.")?;
    let start = text
        .rfind('{')
        .ok_or("FFmpeg did not report loudness measurements.")?;
    let end = text[start..]
        .find('}')
        .ok_or("Incomplete loudness measurements.")?
        + start
        + 1;
    if end - start > 8192 {
        return Err("Loudness record exceeded its size limit.".into());
    }
    let record: Measurement =
        serde_json::from_str(&text[start..end]).map_err(|error| error.to_string())?;
    let integrated = measurement(&record.input_i)?;
    let peak = measurement(&record.input_tp)?;
    let range = measurement(&record.input_lra)?;
    if range.is_some_and(|value| value < 0.0) {
        return Err("Negative loudness range.".into());
    }
    let proposed = integrated.zip(peak).map(|(integrated, peak)| {
        let wanted = request.target_lufs - integrated;
        let headroom = request.peak_limit_dbfs - peak;
        // Do not silently clamp an unsafe attenuation requirement upward.
        let safe = wanted.min(headroom).min(24.0);
        (safe, headroom < wanted)
    });
    let gain = proposed
        .filter(|(gain, _)| *gain >= -60.0)
        .map(|(gain, _)| (gain * 10.0).floor() as i16);
    Ok(LoudnessResult {
        integrated_lufs: integrated, true_peak_dbfs: peak, loudness_range_lu: range,
        suggested_gain_tenths_db: gain,
        target_limited_by_peak: proposed.is_some_and(|(_, limited)| limited),
        source_fingerprint: String::new(),
        message: if gain.is_none() {
            "No usable gain proposal: this track is silent, too short for integrated loudness, or requires more than 60 dB attenuation."
        } else {
            "Review the flat gain before applying it. Dynamics are preserved; the peak limit can keep loudness below your target. Lossy encoding can change true peaks."
        }.into(),
    })
}

pub async fn measure_loudness(
    request: LoudnessRequest,
    cancel: watch::Receiver<bool>,
) -> Result<LoudnessResult, AppError> {
    let fail = |code: &str, message: String| {
        AppError::new(code, message, Some(request.input_path.clone()))
    };
    if !request.target_lufs.is_finite()
        || !(-70.0..=-5.0).contains(&request.target_lufs)
        || !request.peak_limit_dbfs.is_finite()
        || !(-9.0..=0.0).contains(&request.peak_limit_dbfs)
    {
        return Err(fail(
            "INVALID_ANALYSIS",
            "Choose a target from -70 to -5 LUFS and a peak limit from -9 to 0 dBTP.".into(),
        ));
    }
    let _permit = permit(&cancel).await?;
    let source = Source::open(Path::new(&request.input_path))?;
    let identity = fingerprint(&source)?;
    let executable = tool("ffmpeg", &cancel).await?;
    let mut filter = match request.channels {
        AudioChannels::Preserve => String::new(),
        AudioChannels::Mono => "aformat=channel_layouts=mono,".into(),
        AudioChannels::Stereo => "aformat=channel_layouts=stereo,".into(),
    };
    // Only the measured input values are used. The filter's processed samples
    // go to the null muxer, never to a media output or the source directory.
    filter.push_str("loudnorm=I=-23:TP=-1:LRA=7:print_format=json");
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-v",
        "info",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(source.path.as_os_str().to_owned());
    args.extend([
        "-map".into(),
        format!("0:{}", request.stream_index).into(),
        "-vn".into(),
        "-sn".into(),
        "-dn".into(),
        "-af".into(),
        filter.into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]);
    let output = run_capture(
        &CommandSpec {
            executable,
            args,
            cwd: None,
        },
        cancel.clone(),
        256 * 1024,
        Duration::from_secs(1800),
    )
    .await
    .map_err(|error| match error {
        SupervisorError::Cancelled => fail(
            "ANALYSIS_CANCELLED",
            "Loudness measurement was canceled.".into(),
        ),
        SupervisorError::Timeout => fail(
            "ANALYSIS_TIMEOUT",
            "Loudness measurement exceeded its 30-minute limit.".into(),
        ),
        error => fail("ANALYSIS_FAILED", error.to_string()),
    })?;
    source.verify()?;
    if fingerprint(&source)? != identity {
        return Err(fail(
            "SOURCE_CHANGED",
            "The source changed during loudness measurement. Import it again.".into(),
        ));
    }
    check_cancel(&cancel)?;
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr);
        return Err(fail(
            "ANALYSIS_FAILED",
            format!(
                "FFmpeg could not measure this audio stream: {}",
                text.chars()
                    .rev()
                    .take(1500)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
                    .trim()
            ),
        ));
    }
    let mut result =
        parse(&output.stderr, &request).map_err(|message| fail("ANALYSIS_FAILED", message))?;
    result.source_fingerprint = identity;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> LoudnessRequest {
        LoudnessRequest {
            input_path: "sample.mkv".into(),
            stream_index: 1,
            channels: AudioChannels::Preserve,
            target_lufs: -14.0,
            peak_limit_dbfs: -1.0,
        }
    }
    #[test]
    fn flat_gain_respects_peak_headroom_and_rounds_down() {
        let result = parse(
            br#"{"input_i":"-20.11","input_tp":"-4.24","input_lra":"5.5"}"#,
            &request(),
        )
        .unwrap();
        assert_eq!(result.suggested_gain_tenths_db, Some(32));
        assert!(result.target_limited_by_peak);
        let result = parse(
            br#"{"input_i":"-10.01","input_tp":"-0.1","input_lra":"5.5"}"#,
            &request(),
        )
        .unwrap();
        assert_eq!(result.suggested_gain_tenths_db, Some(-40));
        assert!(!result.target_limited_by_peak);
    }
    #[test]
    fn silence_is_unknown_and_malformed_values_are_rejected() {
        let result = parse(
            br#"{"input_i":"-inf","input_tp":"-inf","input_lra":"0.0"}"#,
            &request(),
        )
        .unwrap();
        assert!(result.suggested_gain_tenths_db.is_none());
        assert!(result.integrated_lufs.is_none());
        for value in ["NaN", "inf", "garbage", "999"] {
            assert!(measurement(value).is_err());
        }
        assert!(parse(b"incomplete {", &request()).is_err());
    }
}
