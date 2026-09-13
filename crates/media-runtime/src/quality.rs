//! Explicit frame-paired quality measurements with bounded, owned diagnostics.
use crate::{
    analysis::{check_cancel, fingerprint, permit, tool},
    jobs::files::Source,
    supervisor::{CommandSpec, SupervisorError, run_capture, run_streaming_stdout},
};
use media_core::{AppError, QualityMetric, QualityPoint, QualityRequest, QualityResult};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
const MAX_FRAMES: u32 = 60_000;
const MAX_SCAN: u32 = 1_000_000;
const MAX_REPORT: u64 = 96 * 1024 * 1024;
fn fail(message: impl Into<String>) -> AppError {
    AppError::new("QUALITY_ANALYSIS_FAILED", message, None)
}
fn process_error(error: SupervisorError) -> AppError {
    match error {
        SupervisorError::Cancelled => {
            AppError::new("ANALYSIS_CANCELLED", "Quality analysis was canceled.", None)
        }
        SupervisorError::Timeout => AppError::new(
            "ANALYSIS_TIMEOUT",
            "Quality analysis exceeded its 30-minute limit.",
            None,
        ),
        error => fail(error.to_string()),
    }
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
struct FrameScan {
    pts: Vec<f64>,
    geometry: BTreeMap<String, String>,
}
fn scan_frames(reader: &mut dyn Read, start: u32, count: u32) -> Result<FrameScan, String> {
    let mut reader = BufReader::new(reader);
    let mut record = Vec::new();
    let mut pts = Vec::new();
    let mut geometry = None;
    let mut ordinal = 0;
    loop {
        record.clear();
        if (&mut reader)
            .take(8193)
            .read_until(b'\n', &mut record)
            .map_err(|e| e.to_string())?
            == 0
        {
            break;
        }
        if record.len() > 8192 {
            return Err("Frame record exceeds its bound.".into());
        }
        let line = std::str::from_utf8(&record)
            .map_err(|_| "Invalid frame encoding.")?
            .trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = BTreeMap::new();
        for field in line.split('|').filter(|s| !s.is_empty()) {
            let (key, value) = field.split_once('=').ok_or("Malformed frame record.")?;
            if fields.insert(key.to_owned(), value.to_owned()).is_some() {
                return Err("Duplicate frame field.".into());
            }
        }
        if !fields.contains_key("best_effort_timestamp_time") {
            return Err("A decoded frame has no timestamp.".into());
        }
        if ordinal >= MAX_SCAN {
            return Err("Source exceeds one million decoded frames.".into());
        }
        if ordinal >= start && ordinal < start + count {
            let timestamp: f64 = fields
                .remove("best_effort_timestamp_time")
                .unwrap()
                .parse()
                .map_err(|_| "Invalid frame timestamp.")?;
            if !timestamp.is_finite()
                || timestamp.abs() > 1e9
                || pts.last().is_some_and(|last| timestamp <= *last)
            {
                return Err("Quality inputs need strictly increasing timestamps.".into());
            }
            if fields.get("interlaced_frame").map(String::as_str) != Some("0")
                || fields.get("sample_aspect_ratio").map(String::as_str) != Some("1:1")
            {
                return Err("Quality inputs need progressive square-pixel video.".into());
            }
            let width = fields
                .get("width")
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or("Missing width")?;
            let height = fields
                .get("height")
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or("Missing height")?;
            if !(32..=8192).contains(&width) || !(32..=8192).contains(&height) {
                return Err("Quality dimensions must be between 32 and 8192 pixels.".into());
            }
            if !matches!(
                fields.get("pix_fmt").map(String::as_str),
                Some("yuv420p" | "yuv420p10le")
            ) {
                return Err("Quality analysis supports 8/10-bit planar 4:2:0 inputs.".into());
            }
            if !matches!(
                fields.get("color_transfer").map(String::as_str),
                Some("bt709" | "bt470bg" | "smpte170m")
            ) {
                return Err(
                    "Quality analysis currently requires explicitly tagged SDR inputs.".into(),
                );
            }
            if !matches!(
                fields.get("color_range").map(String::as_str),
                Some("tv" | "pc")
            ) || !matches!(
                fields.get("color_space").map(String::as_str),
                Some("bt709" | "bt470bg" | "smpte170m")
            ) || !matches!(
                fields.get("color_primaries").map(String::as_str),
                Some("bt709" | "bt470bg" | "smpte170m")
            ) || !matches!(
                fields.get("chroma_location").map(String::as_str),
                Some("left" | "center" | "topleft")
            ) {
                return Err(
                    "Explicit color metadata is required for a meaningful comparison.".into(),
                );
            }
            if geometry
                .as_ref()
                .is_some_and(|expected| expected != &fields)
            {
                return Err(
                    "Decoded geometry or color changes within the selected interval.".into(),
                );
            }
            geometry.get_or_insert(fields);
            pts.push(timestamp);
        }
        ordinal += 1;
    }
    if pts.len() != count as usize {
        return Err("The selected interval extends beyond the decoded video.".into());
    }
    Ok(FrameScan {
        pts,
        geometry: geometry.ok_or("No decoded video frames.")?,
    })
}
async fn inspect(
    source: &Source,
    index: u32,
    start: u32,
    count: u32,
    ffprobe: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<FrameScan, AppError> {
    let mut arguments = args(&[
        "-v",
        "error",
        "-threads",
        "4",
        "-select_streams",
        &index.to_string(),
        "-show_frames",
        "-show_entries",
        "frame=best_effort_timestamp_time,width,height,pix_fmt,sample_aspect_ratio,interlaced_frame,color_range,color_space,color_primaries,color_transfer,chroma_location:frame_side_data=",
        "-of",
        "compact=p=0:nk=0",
        "-i",
    ]);
    arguments.push(source.path.as_os_str().into());
    let result = run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.into(),
            args: arguments,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(1800),
        move |reader| scan_frames(reader, start, count),
    )
    .await
    .map_err(process_error)?;
    if !result.status.success() {
        return Err(fail(format!(
            "Frame inspection failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    Ok(result.value)
}
struct Workspace(PathBuf);
impl Workspace {
    fn create() -> Result<Self, AppError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "jesses-quality-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| fail(e.to_string()))?
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).map_err(|e| fail(e.to_string()))?;
        Ok(Self(path))
    }
    fn read(&self) -> Result<Vec<u8>, AppError> {
        let file =
            std::fs::File::open(self.0.join("quality.log")).map_err(|e| fail(e.to_string()))?;
        if file.metadata().map_err(|e| fail(e.to_string()))?.len() > MAX_REPORT {
            return Err(fail("Quality report exceeded its bound."));
        }
        let mut bytes = Vec::new();
        file.take(MAX_REPORT + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| fail(e.to_string()))?;
        if bytes.len() as u64 > MAX_REPORT {
            return Err(fail("Quality report exceeded its bound."));
        }
        Ok(bytes)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("quality.log"));
        let _ = std::fs::remove_dir(&self.0);
    }
}
fn parse_report(
    bytes: &[u8],
    metric: QualityMetric,
    count: u32,
) -> Result<(Option<f64>, Vec<QualityPoint>), AppError> {
    let mut points = Vec::new();
    let mut total = 0.0;
    if metric == QualityMetric::Vmaf {
        let json: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| fail(e.to_string()))?;
        let frames = json["frames"]
            .as_array()
            .ok_or_else(|| fail("Missing VMAF frame scores."))?;
        for (i, frame) in frames.iter().enumerate() {
            if frame["frameNum"].as_u64() != Some(i as u64) {
                return Err(fail("Unexpected VMAF frame order."));
            }
            let value = frame["metrics"]["vmaf"]
                .as_f64()
                .filter(|v| v.is_finite() && (-1000.0..=1000.0).contains(v))
                .ok_or_else(|| fail("Invalid VMAF value."))?;
            total += value;
            points.push(QualityPoint {
                frame: i as u32,
                score: Some(value),
            });
        }
    } else {
        for line in std::str::from_utf8(bytes)
            .map_err(|_| fail("Invalid metric encoding."))?
            .lines()
        {
            let mut fields = BTreeMap::new();
            for field in line.split_whitespace() {
                if let Some((key, value)) = field.split_once(':')
                    && fields.insert(key, value).is_some()
                {
                    return Err(fail("Duplicate metric field."));
                }
            }
            let ordinal = fields
                .get("n")
                .and_then(|n| n.parse::<usize>().ok())
                .ok_or_else(|| fail("Missing metric frame number."))?;
            if ordinal != points.len() + 1 {
                return Err(fail("Unexpected metric frame sequence."));
            }
            let key = if metric == QualityMetric::Ssim {
                "All"
            } else {
                "psnr_avg"
            };
            let raw = fields
                .get(key)
                .ok_or_else(|| fail("Missing metric value."))?;
            if metric == QualityMetric::Psnr && *raw == "inf" {
                points.push(QualityPoint {
                    frame: (ordinal - 1) as u32,
                    score: None,
                });
                continue;
            }
            let value = Some(raw)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .ok_or_else(|| fail("Invalid metric score."))?;
            if (metric == QualityMetric::Ssim && !(-1.0..=1.0).contains(&value))
                || (metric == QualityMetric::Psnr && value < 0.0)
            {
                return Err(fail("Metric score outside its valid range."));
            }
            total += value;
            let score = Some(value);
            points.push(QualityPoint {
                frame: (ordinal - 1) as u32,
                score,
            });
        }
    }
    if points.len() != count as usize {
        return Err(fail("The metric did not compare every requested frame."));
    }
    let average = total / f64::from(count);
    Ok((
        if metric == QualityMetric::Psnr {
            None // The authoritative pooled PSNR comes from FFmpeg's summary.
        } else {
            Some(average)
        },
        points,
    ))
}
fn psnr_summary(stderr: &[u8]) -> Result<Option<f64>, AppError> {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .rev()
        .find(|line| line.contains("PSNR y:"))
        .ok_or_else(|| fail("Missing pooled PSNR summary."))?;
    let value = line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("average:"))
        .ok_or_else(|| fail("Missing pooled PSNR value."))?;
    if value == "inf" {
        return Ok(None);
    }
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(Some)
        .ok_or_else(|| fail("Invalid pooled PSNR value."))
}
pub async fn analyze_quality(
    request: QualityRequest,
    cancel: watch::Receiver<bool>,
) -> Result<QualityResult, AppError> {
    let _permit = permit(&cancel).await?;
    if request.frame_count == 0
        || request.frame_count > MAX_FRAMES
        || request.reference_start_frame > MAX_SCAN - request.frame_count
        || request.candidate_start_frame > MAX_SCAN - request.frame_count
    {
        return Err(fail(
            "Choose 1 to 60,000 corresponding frames within the first million source frames.",
        ));
    }
    for path in [&request.reference_path, &request.candidate_path] {
        if !Path::new(path).is_absolute() || path.contains('\0') {
            return Err(fail("Use absolute local media paths."));
        }
    }
    let reference = Source::open(Path::new(&request.reference_path))?;
    let candidate = Source::open(Path::new(&request.candidate_path))?;
    let ref_identity = fingerprint(&reference)?;
    let candidate_identity = fingerprint(&candidate)?;
    let ffprobe = tool("ffprobe", &cancel).await?;
    let ffmpeg = tool("ffmpeg", &cancel).await?;
    let left = inspect(
        &reference,
        request.reference_stream_index,
        request.reference_start_frame,
        request.frame_count,
        &ffprobe,
        &cancel,
    )
    .await?;
    let right = inspect(
        &candidate,
        request.candidate_stream_index,
        request.candidate_start_frame,
        request.frame_count,
        &ffprobe,
        &cancel,
    )
    .await?;
    if left.geometry != right.geometry {
        return Err(fail(
            "Reference and candidate must have matching dimensions, pixel format and color metadata. Prepare an explicitly matched reference first.",
        ));
    }
    for (a, b) in left.pts.iter().zip(&right.pts) {
        if ((a - left.pts[0]) - (b - right.pts[0])).abs() > 0.0021 {
            return Err(fail(
                "Decoded frame timing differs. Select corresponding frame intervals without changing cadence.",
            ));
        }
    }
    let workspace = Workspace::create()?;
    let metric = match request.metric {
        QualityMetric::Psnr => "psnr=stats_file=quality.log:shortest=1:repeatlast=0",
        QualityMetric::Ssim => "ssim=stats_file=quality.log:shortest=1:repeatlast=0",
        QualityMetric::Vmaf => {
            "libvmaf=log_fmt=json:log_path=quality.log:model=version=vmaf_v0.6.1:n_threads=4:shortest=1:repeatlast=0"
        }
    };
    // Frame timings have been compared above. Ordinal timestamps prevent a
    // container's sub-millisecond rounding from pairing a neighboring frame.
    let filter = format!(
        "[0:{}]trim=start_frame={}:end_frame={},settb=1/1,setpts=N[ref];[1:{}]trim=start_frame={}:end_frame={},settb=1/1,setpts=N[dist];[dist][ref]{}[scored]",
        request.reference_stream_index,
        request.reference_start_frame,
        request.reference_start_frame + request.frame_count,
        request.candidate_stream_index,
        request.candidate_start_frame,
        request.candidate_start_frame + request.frame_count,
        metric
    );
    let mut arguments = args(&[
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-v",
        "info",
        "-threads",
        "4",
        "-noautorotate",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]);
    arguments.push(reference.path.as_os_str().into());
    arguments.extend(args(&[
        "-threads",
        "4",
        "-noautorotate",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]));
    arguments.push(candidate.path.as_os_str().into());
    arguments.extend(args(&[
        "-filter_complex_threads",
        "4",
        "-filter_complex",
        &filter,
        "-map",
        "[scored]",
        "-frames:v",
        &request.frame_count.to_string(),
        "-fps_mode",
        "passthrough",
        "-f",
        "null",
        "-",
    ]));
    let result = run_capture(
        &CommandSpec {
            executable: ffmpeg,
            args: arguments,
            cwd: Some(workspace.0.clone()),
        },
        cancel.clone(),
        256 * 1024,
        Duration::from_secs(1800),
    )
    .await
    .map_err(process_error)?;
    if !result.status.success() {
        return Err(fail(format!(
            "Quality measurement failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    let (mut score, points) =
        parse_report(&workspace.read()?, request.metric, request.frame_count)?;
    if request.metric == QualityMetric::Psnr {
        score = psnr_summary(&result.stderr)?;
    }
    reference.verify()?;
    candidate.verify()?;
    if fingerprint(&reference)? != ref_identity || fingerprint(&candidate)? != candidate_identity {
        return Err(AppError::new(
            "SOURCE_CHANGED",
            "A comparison source changed. Select it again.",
            None,
        ));
    }
    check_cancel(&cancel)?;
    Ok(QualityResult{metric:request.metric,frame_count:request.frame_count,score,points,reference_fingerprint:ref_identity,candidate_fingerprint:candidate_identity,model:(request.metric==QualityMetric::Vmaf).then(||"vmaf_v0.6.1".into()),message:"Higher scores indicate closer agreement within these selected frames. PSNR uses pooled mean-square error; infinite PSNR means identical decoded pixels. Metrics do not replace visual review.".into()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn psnr_pools_error_and_identical_pixels_have_infinite_score() {
        let (_, points) = parse_report(
            b"n:1 mse_avg:0.00 psnr_avg:inf\nn:2 mse_avg:0.00 psnr_avg:91.02\n",
            QualityMetric::Psnr,
            2,
        )
        .unwrap();
        assert!(points[0].score.is_none());
        assert_eq!(points[1].score, Some(91.02));
        assert_eq!(
            psnr_summary(b"[psnr] PSNR y:1 average:94.03021 min:91.02 max:inf").unwrap(),
            Some(94.03021)
        );
        assert!(parse_report(b"n:2 psnr_avg:1\n", QualityMetric::Psnr, 1).is_err());
        assert!(parse_report(b"n:1 All:NaN\n", QualityMetric::Ssim, 1).is_err());
    }
    #[test]
    fn malformed_or_short_metric_reports_fail() {
        assert!(
            parse_report(
                br#"{"frames":[{"frameNum":1,"metrics":{"vmaf":99}}]}"#,
                QualityMetric::Vmaf,
                1
            )
            .is_err()
        );
        assert!(parse_report(b"n:1 All:1.0\n", QualityMetric::Ssim, 2).is_err());
    }
}
