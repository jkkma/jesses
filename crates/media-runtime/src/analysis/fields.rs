//! Bounded, cancellable counterparts of the fork's trim and field analysis.
use super::*;
use media_core::{
    DetectedFieldOrder as Order, FieldDetectionRequest, FieldDetectionResult, KeyframeSnapRequest,
    KeyframeSnapResult,
};

pub async fn snap_keyframe(
    request: KeyframeSnapRequest,
    cancel: watch::Receiver<bool>,
) -> Result<KeyframeSnapResult, AppError> {
    if !request.position_seconds.is_finite() || request.position_seconds < 0.0 {
        return Err(AppError::new(
            "ANALYSIS_SETTINGS_INVALID",
            "The requested keyframe position must be a nonnegative finite number.",
            None,
        ));
    }
    let _permit = permit(&cancel).await?;
    let input = inspect(request.input_path, request.video_stream_index, &cancel).await?;
    let probe = tool("ffprobe", &cancel).await?;
    let mut position = None;
    for window in [20.0, 120.0, f64::INFINITY] {
        let from = (request.position_seconds - window).max(0.0);
        let interval = format!("{from:.6}%{:.6}", request.position_seconds + 0.001);
        let args = vec![
            "-v".into(),
            "error".into(),
            "-protocol_whitelist".into(),
            "file".into(),
            "-select_streams".into(),
            request.video_stream_index.to_string().into(),
            "-skip_frame".into(),
            "nokey".into(),
            "-read_intervals".into(),
            interval.into(),
            "-show_entries".into(),
            "frame=best_effort_timestamp_time".into(),
            "-of".into(),
            "csv=p=0".into(),
            "-i".into(),
            input.source.path.as_os_str().to_owned(),
        ];
        let result = capture(probe.clone(), args, &input.source, &cancel, 120).await?;
        position = String::from_utf8_lossy(&result.stdout)
            .lines()
            .filter_map(|line| line.split(',').next()?.trim().parse::<f64>().ok())
            .filter(|value| {
                value.is_finite() && *value >= 0.0 && *value <= request.position_seconds + 0.000001
            })
            .max_by(f64::total_cmp);
        if position.is_some() || from == 0.0 {
            break;
        }
    }
    input.finish(&cancel)?;
    Ok(KeyframeSnapResult {
        position_seconds: position,
        source_fingerprint: input.fingerprint,
    })
}

fn count(line: &str, label: &str) -> u32 {
    line.split_once(label)
        .and_then(|(_, value)| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}
fn idet_counts(text: &str) -> ([u32; 4], u32, u32) {
    let mut counts = [0; 4];
    let mut repeated = 0;
    let mut neither = 0;
    for line in text.lines() {
        if line.contains("Multi frame detection:") {
            counts = [
                count(line, "TFF:"),
                count(line, "BFF:"),
                count(line, "Progressive:"),
                count(line, "Undetermined:"),
            ];
        }
        if line.contains("Repeated Fields:") {
            repeated = count(line, "Top:") + count(line, "Bottom:");
            neither = count(line, "Neither:");
        }
    }
    (counts, repeated, neither)
}
async fn sample(
    input: &Input,
    at: f64,
    filter: &str,
    frames: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<String, AppError> {
    let args = vec![
        "-hide_banner".into(),
        "-v".into(),
        "info".into(),
        "-nostdin".into(),
        "-ss".into(),
        format!("{at:.6}").into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-i".into(),
        input.source.path.as_os_str().to_owned(),
        "-map".into(),
        format!("0:{}", input.stream.index).into(),
        "-an".into(),
        "-sn".into(),
        "-dn".into(),
        "-vf".into(),
        filter.into(),
        "-frames:v".into(),
        frames.to_string().into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ];
    let result = capture(input.ffmpeg.clone(), args, &input.source, cancel, 120).await?;
    Ok(String::from_utf8_lossy(&result.stderr).into_owned())
}
fn gaps(text: &str) -> Vec<f64> {
    text.lines()
        .filter_map(|line| {
            line.split_once("lavfi.signalstats.YDIF=")?
                .1
                .trim()
                .parse::<f64>()
                .ok()
        })
        .filter(|v| v.is_finite() && *v >= 0.0)
        .collect()
}
fn verdict(counts: [u32; 4], gap: Option<f64>) -> Order {
    let [tff, bff, progressive, undetermined] = counts;
    let interlaced = tff + bff;
    let dominant = tff.max(bff);
    let total = interlaced + progressive + undetermined;
    if total == 0 {
        return Order::Undetermined;
    }
    let combed = dominant > 0
        && u64::from(dominant) * 4 >= u64::from(interlaced) * 3
        && (interlaced > progressive || u64::from(interlaced) * 5 > u64::from(total));
    if combed && !gap.is_some_and(|g| g >= 4.0) {
        if bff > tff {
            Order::BottomFirst
        } else {
            Order::TopFirst
        }
    } else {
        Order::Progressive
    }
}

pub async fn detect_fields(
    request: FieldDetectionRequest,
    cancel: watch::Receiver<bool>,
) -> Result<FieldDetectionResult, AppError> {
    let _permit = permit(&cancel).await?;
    let input = inspect(request.input_path, request.video_stream_index, &cancel).await?;
    let mut result = FieldDetectionResult {
        order: Order::Undetermined,
        telecined: false,
        sampled_frames: 0,
        top_first_frames: 0,
        bottom_first_frames: 0,
        progressive_frames: 0,
        undetermined_frames: 0,
        field_gap_ratio: None,
        source_fingerprint: input.fingerprint.clone(),
        evidence: String::new(),
    };
    let declared = match input.stream.field_order.as_deref() {
        Some("progressive") => Some(Order::Progressive),
        Some("tt" | "tb") => Some(Order::TopFirst),
        Some("bb" | "bt") => Some(Order::BottomFirst),
        _ => None,
    };
    if let Some(order) = declared {
        result.order = order;
        result.evidence =
            "The source declares its field order; no frame sampling was needed.".into();
    } else {
        let offsets = match input.duration {
            Some(d) if d > 8.0 => vec![d / 4.0, d / 2.0, d * 3.0 / 4.0],
            _ => vec![0.0],
        };
        let mut counts = [0_u32; 4];
        let mut repeated = 0;
        let mut neither = 0;
        for &at in &offsets {
            let text = sample(&input, at, "idet", 240, &cancel).await?;
            let (sampled, r, n) = idet_counts(&text);
            for i in 0..4 {
                counts[i] += sampled[i];
            }
            repeated += r;
            neither += n;
        }
        let preliminary = verdict(counts, None);
        if matches!(preliminary, Order::TopFirst | Order::BottomFirst) {
            let parity = if preliminary == Order::TopFirst {
                "tff"
            } else {
                "bff"
            };
            let filter = format!(
                "scale=256:ih,setfield={parity},separatefields,signalstats,metadata=print:key=lavfi.signalstats.YDIF"
            );
            let mut sums = [0.0; 2];
            let mut samples = [0_u32; 2];
            for &at in &offsets {
                // Unsupported field-gap filters do not silently erase idet evidence;
                // cancellation/source mutation still propagate as errors.
                let text = sample(&input, at, &filter, 480, &cancel).await?;
                for (i, value) in gaps(&text).into_iter().enumerate().skip(1) {
                    sums[i % 2] += value;
                    samples[i % 2] += 1;
                }
            }
            if samples[0] > 0 && samples[1] > 0 {
                let a = sums[0] / samples[0] as f64;
                let b = sums[1] / samples[1] as f64;
                if a.max(b) >= 0.5 {
                    result.field_gap_ratio = Some(if a.min(b) < 0.0001 {
                        1e12
                    } else {
                        a.max(b) / a.min(b)
                    });
                }
            }
        }
        result.order = verdict(counts, result.field_gap_ratio);
        result.sampled_frames = counts.iter().sum();
        result.top_first_frames = counts[0];
        result.bottom_first_frames = counts[1];
        result.progressive_frames = counts[2];
        result.undetermined_frames = counts[3];
        result.telecined = matches!(result.order, Order::TopFirst | Order::BottomFirst)
            && repeated > 0
            && u64::from(repeated) * 5 > u64::from(repeated + neither);
        result.evidence = format!(
            "Sampled {} frames: {} TFF, {} BFF, {} progressive, {} undetermined.{}{}",
            result.sampled_frames,
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            if result.field_gap_ratio.is_some_and(|gap| gap >= 4.0) {
                " Uneven field timing indicates progressive motion rather than interlacing."
            } else {
                ""
            },
            if result.telecined {
                " Repeated fields suggest telecine; consider inverse telecine."
            } else {
                ""
            }
        );
    }
    input.finish(&cancel)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_idet_and_rejects_balanced_false_combing() {
        let (counts, r, n) = idet_counts(
            "[idet] Repeated Fields: Neither: 100 Top: 7 Bottom: 8\n[idet] Multi frame detection: TFF: 79 BFF: 96 Progressive: 27 Undetermined: 0",
        );
        assert_eq!(counts, [79, 96, 27, 0]);
        assert_eq!((r, n), (15, 100));
        assert_eq!(verdict(counts, None), Order::Progressive);
        assert_eq!(verdict([202, 0, 0, 0], Some(1.02)), Order::TopFirst);
        assert_eq!(verdict([0, 202, 0, 0], Some(1.1)), Order::BottomFirst);
        assert_eq!(verdict([202, 0, 0, 0], Some(6.9)), Order::Progressive);
        assert_eq!(verdict([0; 4], None), Order::Undetermined);
    }
}
