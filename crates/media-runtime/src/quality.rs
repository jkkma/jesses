//! Explicit frame-paired quality measurements with bounded, owned diagnostics.
use crate::{
    analysis::{check_cancel, fingerprint, permit, tool},
    jobs::files::Source,
    supervisor::{CommandSpec, SupervisorError, run_capture, run_streaming_stdout},
};
use media_core::{
    AppError, QualityAlignment, QualityMetric, QualityPoint, QualityRequest, QualityResult,
    QualityVmafModel,
};
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
            "Quality analysis exceeded its 24-hour limit.",
            None,
        ),
        error => fail(error.to_string()),
    }
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
struct FrameScan {
    count: u32,
    geometry: BTreeMap<String, String>,
    width: u32,
    height: u32,
    sar: (u32, u32),
    vp9_decoder: bool,
}
fn parse_sar(value: &str) -> Result<(u32, u32), String> {
    if value == "N/A" {
        return Ok((1, 1));
    }
    let (n, d) = value
        .split_once(':')
        .ok_or("Invalid sample aspect ratio.")?;
    let (n, d) = (
        n.parse::<u32>()
            .map_err(|_| "Invalid sample aspect ratio.")?,
        d.parse::<u32>()
            .map_err(|_| "Invalid sample aspect ratio.")?,
    );
    if n == 0 || d == 0 || f64::from(n) / f64::from(d) > 100.0 {
        return Err("Invalid sample aspect ratio.".into());
    }
    Ok((n, d))
}
fn scan_frames(
    reader: &mut dyn Read,
    start: u32,
    count: Option<u32>,
    mut visit: impl FnMut(f64) -> Result<(), String>,
) -> Result<FrameScan, String> {
    let mut reader = BufReader::new(reader);
    let mut record = Vec::new();
    let mut selected_count = 0u32;
    let mut last_timestamp = None;
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
        if count.is_some() && ordinal >= MAX_SCAN {
            return Err("Source exceeds one million decoded frames.".into());
        }
        if ordinal >= start && count.is_none_or(|count| ordinal < start + count) {
            let timestamp: f64 = fields
                .remove("best_effort_timestamp_time")
                .unwrap()
                .parse()
                .map_err(|_| "Invalid frame timestamp.")?;
            if !timestamp.is_finite()
                || timestamp.abs() > 1e9
                || last_timestamp.is_some_and(|last| timestamp <= last)
            {
                return Err("Quality inputs need strictly increasing timestamps.".into());
            }
            if fields.get("interlaced_frame").map(String::as_str) != Some("0") {
                return Err("Quality inputs need progressive video.".into());
            }
            parse_sar(
                fields
                    .get("sample_aspect_ratio")
                    .map(String::as_str)
                    .ok_or("Missing sample aspect ratio.")?,
            )?;
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
                Some(
                    "yuv420p"
                        | "yuv422p"
                        | "yuv444p"
                        | "yuv420p10le"
                        | "yuv422p10le"
                        | "yuv444p10le"
                        | "yuva420p"
                )
            ) {
                return Err("Quality analysis supports matched 8/10-bit planar YUV 4:2:0/4:2:2/4:4:4 and 8-bit 4:2:0 alpha inputs.".into());
            }
            if !matches!(
                fields.get("color_transfer").map(String::as_str),
                Some("bt709" | "bt470bg" | "smpte170m")
            ) {
                return Err(
                    "Quality analysis currently requires explicitly tagged SDR inputs.".into(),
                );
            }
            // HEVC does not signal variable chroma location for 4:2:2; use its
            // nominal left position when the decoder omits it. Explicit siting
            // remains authoritative and still has to match the paired input.
            if fields
                .get("pix_fmt")
                .is_some_and(|format| format.starts_with("yuv422p"))
                && fields
                    .get("chroma_location")
                    .is_none_or(|value| matches!(value.as_str(), "unknown" | "unspecified"))
            {
                fields.insert("chroma_location".into(), "left".into());
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
            ) || (!fields
                .get("pix_fmt")
                .is_some_and(|format| format.starts_with("yuv444p"))
                && !matches!(
                    fields.get("chroma_location").map(String::as_str),
                    Some("left" | "center" | "topleft")
                ))
            {
                return Err(
                    "Explicit color metadata is required for a meaningful comparison.".into(),
                );
            }
            // Full-resolution chroma has no subsampling/siting coordinate.
            if fields
                .get("pix_fmt")
                .is_some_and(|format| format.starts_with("yuv444p"))
            {
                fields.remove("chroma_location");
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
            visit(timestamp)?;
            last_timestamp = Some(timestamp);
            selected_count = selected_count
                .checked_add(1)
                .ok_or("Decoded frame count exceeds the contract range.")?;
        }
        ordinal = ordinal
            .checked_add(1)
            .ok_or("Decoded frame count exceeds the contract range.")?;
    }
    if count.is_some_and(|count| selected_count != count) {
        return Err("The selected interval extends beyond the decoded video.".into());
    }
    let mut geometry = geometry.ok_or("No decoded video frames.")?;
    let width = geometry
        .remove("width")
        .unwrap()
        .parse()
        .map_err(|_| "Invalid width.")?;
    let height = geometry
        .remove("height")
        .unwrap()
        .parse()
        .map_err(|_| "Invalid height.")?;
    let sar = parse_sar(&geometry.remove("sample_aspect_ratio").unwrap())?;
    Ok(FrameScan {
        count: selected_count,
        geometry,
        width,
        height,
        sar,
        vp9_decoder: false,
    })
}
async fn vp9_decoder(
    source: &Source,
    index: u32,
    ffprobe: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<bool, AppError> {
    let mut arguments = args(&[
        "-v",
        "error",
        "-select_streams",
        &index.to_string(),
        "-show_entries",
        "stream=codec_name:stream_tags=alpha_mode",
        "-of",
        "json",
        "-i",
    ]);
    arguments.push(source.path.as_os_str().into());
    let result = run_capture(
        &CommandSpec {
            executable: ffprobe.into(),
            args: arguments,
            cwd: None,
        },
        cancel.clone(),
        65536,
        Duration::from_secs(30),
    )
    .await
    .map_err(process_error)?;
    if !result.status.success() {
        return Err(fail(
            "The selected quality input codec could not be inspected.",
        ));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&result.stdout).map_err(|cause| fail(cause.to_string()))?;
    Ok(value["streams"].as_array().is_some_and(|streams| {
        streams.len() == 1
            && streams[0]["codec_name"] == "vp9"
            && streams[0]["tags"].as_object().is_some_and(|tags| {
                tags.iter().any(|(key, value)| {
                    key.eq_ignore_ascii_case("alpha_mode") && (value == "1" || value == 1)
                })
            })
    }))
}

fn inspect_spec(source: &Source, index: u32, ffprobe: &Path, vp9: bool) -> CommandSpec {
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
    if vp9 {
        let input = arguments
            .iter()
            .position(|value| value == "-i")
            .expect("input");
        arguments.splice(input..input, ["-c:v".into(), "libvpx-vp9".into()]);
    }
    arguments.push(source.path.as_os_str().into());
    CommandSpec {
        executable: ffprobe.into(),
        args: arguments,
        cwd: None,
    }
}
async fn inspect_pair(
    reference: &Source,
    candidate: &Source,
    request: &QualityRequest,
    ffprobe: &Path,
    cancel: &watch::Receiver<bool>,
    fixed_rate: bool,
) -> Result<(FrameScan, FrameScan), AppError> {
    let left_vp9 = vp9_decoder(reference, request.reference_stream_index, ffprobe, cancel).await?;
    let right_vp9 = vp9_decoder(candidate, request.candidate_stream_index, ffprobe, cancel).await?;
    let reference_spec = inspect_spec(reference, request.reference_stream_index, ffprobe, left_vp9);
    let candidate_spec = inspect_spec(
        candidate,
        request.candidate_stream_index,
        ffprobe,
        right_vp9,
    );
    let count = (!request.full_source).then_some(request.frame_count);
    let left_start = request.reference_start_frame;
    let right_start = request.candidate_start_frame;
    let (sender, receiver) = std::sync::mpsc::sync_channel(32);
    let left = run_streaming_stdout(
        &reference_spec,
        cancel.clone(),
        65536,
        Duration::from_secs(86400),
        move |reader| {
            scan_frames(reader, left_start, count, |pts| {
                sender
                    .send(pts)
                    .map_err(|_| "Paired source inspection stopped.".into())
            })
        },
    );
    let right = run_streaming_stdout(
        &candidate_spec,
        cancel.clone(),
        65536,
        Duration::from_secs(86400),
        move |reader| {
            let mut first = None;
            let scan = scan_frames(reader, right_start, count, |pts| {
                let expected = receiver.recv().map_err(|_| {
                    "Candidate has more decoded frames than the reference.".to_owned()
                })?;
                let (left_first, right_first) = *first.get_or_insert((expected, pts));
                if !fixed_rate && ((expected - left_first) - (pts - right_first)).abs() > 0.0021 {
                    return Err("Decoded frame timing differs. Select corresponding frame intervals or enable fixed-rate frame pairing.".into());
                }
                Ok(())
            })?;
            if receiver.recv().is_ok() {
                return Err("Reference has more decoded frames than the candidate.".into());
            }
            Ok(scan)
        },
    );
    let (left, right) = tokio::join!(Box::pin(left), Box::pin(right));
    let right = right.map_err(process_error)?;
    let left = left.map_err(process_error)?;
    if !left.status.success()
        || !right.status.success()
        || !left.stderr.is_empty()
        || !right.stderr.is_empty()
    {
        return Err(fail(format!(
            "Complete paired frame inspection failed: {} {}",
            String::from_utf8_lossy(&left.stderr),
            String::from_utf8_lossy(&right.stderr)
        )));
    }
    if left.value.count != right.value.count {
        return Err(fail("Reference and candidate decoded frame counts differ."));
    }
    let mut left = left.value;
    let mut right = right.value;
    left.vp9_decoder = left_vp9;
    right.vp9_decoder = right_vp9;
    Ok((left, right))
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
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("quality.log"));
        let _ = std::fs::remove_dir(&self.0);
    }
}
fn parse_report_reader(
    reader: &mut dyn Read,
    metric: QualityMetric,
    count: u32,
    point_limit: u32,
) -> Result<(Option<f64>, Vec<QualityPoint>), AppError> {
    let mut points = Vec::new();
    let mut total = 0.0;
    let mut seen = 0u32;
    let stride = count.div_ceil(point_limit.max(1)).max(1);
    let mut consume = |ordinal: u32, score: Option<f64>| -> Result<(), String> {
        if ordinal != seen {
            return Err("Unexpected metric frame sequence.".into());
        }
        if let Some(value) = score {
            if !value.is_finite()
                || (metric == QualityMetric::Ssim && !(-1.0..=1.0).contains(&value))
                || (metric == QualityMetric::Psnr && value < 0.0)
                || (metric == QualityMetric::Vmaf && !(-1000.0..=1000.0).contains(&value))
            {
                return Err("Metric score outside its valid range.".into());
            }
            total += value;
        }
        if (ordinal.is_multiple_of(stride) && points.len() < (point_limit.max(1) - 1) as usize)
            || ordinal == count - 1
        {
            points.push(QualityPoint {
                frame: ordinal,
                score,
            });
        }
        seen = seen.checked_add(1).ok_or("Metric frame count overflow.")?;
        Ok(())
    };
    if metric == QualityMetric::Vmaf {
        #[derive(serde::Deserialize)]
        struct Frame {
            #[serde(rename = "frameNum")]
            frame: u32,
            metrics: BTreeMap<String, f64>,
        }
        let mut failure = None;
        crate::jobs::parse_frame_records(reader, |frame: Frame| {
            if failure.is_none() {
                failure = frame
                    .metrics
                    .get("vmaf")
                    .copied()
                    .ok_or("Missing VMAF value.".to_owned())
                    .and_then(|value| consume(frame.frame, Some(value)))
                    .err();
            }
        })
        .map_err(fail)?;
        if let Some(error) = failure {
            return Err(fail(error));
        }
    } else {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            let size = (&mut reader)
                .take(8193)
                .read_until(b'\n', &mut line)
                .map_err(|e| fail(e.to_string()))?;
            if size == 0 {
                break;
            }
            if size > 8192 {
                return Err(fail("Metric record exceeds 8192 bytes."));
            }
            let line = std::str::from_utf8(&line).map_err(|e| fail(e.to_string()))?;
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
                .and_then(|value| value.parse::<u32>().ok())
                .and_then(|value| value.checked_sub(1))
                .ok_or_else(|| fail("Missing metric frame number."))?;
            let raw = fields
                .get(if metric == QualityMetric::Ssim {
                    "All"
                } else {
                    "psnr_avg"
                })
                .ok_or_else(|| fail("Missing metric value."))?;
            let value = if metric == QualityMetric::Psnr && *raw == "inf" {
                None
            } else {
                Some(raw.parse::<f64>().map_err(|e| fail(e.to_string()))?)
            };
            consume(ordinal, value).map_err(fail)?;
        }
    }
    if seen != count {
        return Err(fail("The metric did not compare every requested frame."));
    }
    Ok((
        if metric == QualityMetric::Psnr {
            None
        } else {
            Some(total / f64::from(count))
        },
        points,
    ))
}
#[cfg(test)]
fn parse_report(
    bytes: &[u8],
    metric: QualityMetric,
    count: u32,
) -> Result<(Option<f64>, Vec<QualityPoint>), AppError> {
    parse_report_reader(&mut &*bytes, metric, count, count)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Crop {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
}

fn parse_crop(line: &str, width: u32, height: u32) -> Option<Crop> {
    let value = line.split("crop=").nth(1)?.split_whitespace().next()?;
    let values: Vec<_> = value.split(':').map(str::parse::<u32>).collect();
    let [Ok(w), Ok(h), Ok(x), Ok(y)] = values.as_slice() else {
        return None;
    };
    if *w < 32
        || *h < 32
        || *x > width.saturating_sub(*w)
        || *y > height.saturating_sub(*h)
        || *w > width
        || *h > height
        || *w % 2 != 0
        || *h % 2 != 0
    {
        return None;
    }
    Some(Crop {
        width: *w,
        height: *h,
        x: *x,
        y: *y,
    })
}

fn choose_crop(stderr: &[u8], width: u32, height: u32) -> Option<Crop> {
    let text = String::from_utf8_lossy(stderr);
    let crops: Vec<_> = text
        .lines()
        .filter_map(|line| parse_crop(line, width, height))
        .collect();
    if crops.is_empty() {
        return None;
    }
    let mut counts = BTreeMap::<(u32, u32, u32, u32), usize>::new();
    for c in &crops {
        *counts.entry((c.width, c.height, c.x, c.y)).or_default() += 1;
    }
    let (common, common_count) = counts.into_iter().max_by_key(|(c, n)| (*n, c.0 * c.1))?;
    let selected = if common_count * 100 > crops.len() * 80 {
        Crop {
            width: common.0,
            height: common.1,
            x: common.2,
            y: common.3,
        }
    } else {
        // Disagreeing samples must never discard more of the picture than a
        // sampled frame permits. Retain their union, rounded out to even edges.
        let left = crops.iter().map(|c| c.x).min()? & !1;
        let top = crops.iter().map(|c| c.y).min()? & !1;
        let right = crops.iter().map(|c| c.x + c.width).max()?.div_ceil(2) * 2;
        let bottom = crops.iter().map(|c| c.y + c.height).max()?.div_ceil(2) * 2;
        let right = right.min(width);
        let bottom = bottom.min(height);
        Crop {
            width: right - left,
            height: bottom - top,
            x: left,
            y: top,
        }
    };
    Some(selected)
}

async fn detect_reference_crop(
    reference: &Source,
    stream: u32,
    interval: (u32, u32),
    size: (u32, u32),
    ffmpeg: &Path,
    vp9: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<Crop, AppError> {
    let (start, count) = interval;
    let (width, height) = size;
    let step = count.div_ceil(60).max(1);
    let filter = format!(
        "[0:{stream}]trim=start_frame={start}:end_frame={},select=not(mod(n\\,{step})),cropdetect=limit=0.094117647:round=2:reset=0:skip=0[scan]",
        start + count
    );
    let mut arguments = args(&[
        "-hide_banner",
        "-nostdin",
        "-v",
        "info",
        "-threads",
        "4",
        "-noautorotate",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]);
    if vp9 {
        let index = arguments.len() - 1;
        arguments.splice(index..index, ["-c:v".into(), "libvpx-vp9".into()]);
    }
    arguments.push(reference.path.as_os_str().into());
    arguments.extend(args(&[
        "-filter_complex",
        &filter,
        "-map",
        "[scan]",
        "-frames:v",
        "60",
        "-fps_mode",
        "passthrough",
        "-f",
        "null",
        "-",
    ]));
    let result = run_capture(
        &CommandSpec {
            executable: ffmpeg.into(),
            args: arguments,
            cwd: None,
        },
        cancel.clone(),
        256 * 1024,
        Duration::from_secs(86400),
    )
    .await
    .map_err(process_error)?;
    if !result.status.success() {
        return Err(fail(format!(
            "Reference crop detection failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    choose_crop(&result.stderr,width,height).ok_or_else(|| fail("Reference crop detection found no usable sampled picture. Choose resize only or another interval."))
}

fn display_width(width: u32, sar: (u32, u32)) -> Result<u32, AppError> {
    let scaled = (f64::from(width) * f64::from(sar.0) / f64::from(sar.1) / 2.0).round() * 2.0;
    if !(32.0..=8192.0).contains(&scaled) {
        return Err(fail(
            "Displayed frame width is outside the supported 32–8192 pixel range.",
        ));
    }
    Ok(scaled as u32)
}

struct Comparison {
    reference: Vec<String>,
    candidate: Vec<String>,
    width: u32,
    height: u32,
}
fn comparison(
    left: &FrameScan,
    right: &FrameScan,
    alignment: QualityAlignment,
    crop: Option<Crop>,
) -> Result<Comparison, AppError> {
    let mut reference = Vec::new();
    let mut candidate = Vec::new();
    let mut ref_size = (left.width, left.height);
    if let Some(crop) = crop {
        ref_size = (crop.width, crop.height);
        reference.push(format!(
            "crop={}:{}:{}:{}",
            crop.width, crop.height, crop.x, crop.y
        ));
    }
    let matching_storage = (right.width, right.height) == ref_size
        && u64::from(left.sar.0) * u64::from(right.sar.1)
            == u64::from(right.sar.0) * u64::from(left.sar.1);
    let desqueeze = !matching_storage && (left.sar.0 != left.sar.1 || right.sar.0 != right.sar.1);
    let candidate_size = if desqueeze {
        (display_width(right.width, right.sar)?, right.height)
    } else {
        (right.width, right.height)
    };
    if desqueeze && (candidate_size != (right.width, right.height) || right.sar.0 != right.sar.1) {
        candidate.push(format!(
            "scale={}:{}:flags=bicubic,setsar=1",
            candidate_size.0, candidate_size.1
        ));
    }
    let resize = matches!(
        alignment,
        QualityAlignment::ResizeReference | QualityAlignment::CropAndResizeReference
    );
    let target = if resize {
        candidate_size
    } else if desqueeze {
        (display_width(ref_size.0, left.sar)?, ref_size.1)
    } else {
        ref_size
    };
    if target != ref_size || (desqueeze && left.sar.0 != left.sar.1) {
        reference.push(format!(
            "scale={}:{}:flags=bicubic,setsar=1",
            target.0, target.1
        ));
    }
    if target != candidate_size {
        return Err(fail(format!(
            "Aligned reference is {}×{} and candidate is {}×{}. Choose Resize reference or a different crop.",
            target.0, target.1, candidate_size.0, candidate_size.1
        )));
    }
    Ok(Comparison {
        reference,
        candidate,
        width: target.0,
        height: target.1,
    })
}

fn model_name(model: QualityVmafModel) -> &'static str {
    match model {
        QualityVmafModel::Standard => "vmaf_v0.6.1",
        QualityVmafModel::Negative => "vmaf_v0.6.1neg",
        QualityVmafModel::FourK => "vmaf_4k_v0.6.1",
    }
}
pub async fn analyze_quality(
    mut request: QualityRequest,
    cancel: watch::Receiver<bool>,
) -> Result<QualityResult, AppError> {
    let _permit = permit(&cancel).await?;
    let options = request.options.clone().unwrap_or_default();
    if !(1..=1000).contains(&options.subsample) {
        return Err(fail("Score every 1 to 1,000 frames."));
    }
    if !request.full_source
        && (request.frame_count == 0
            || request.frame_count > MAX_FRAMES
            || request.reference_start_frame > MAX_SCAN - request.frame_count
            || request.candidate_start_frame > MAX_SCAN - request.frame_count)
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
    if request.full_source
        && (request.reference_start_frame != 0 || request.candidate_start_frame != 0)
    {
        return Err(fail(
            "Full-source comparison starts at the beginning of both files.",
        ));
    }
    let (left, right) = inspect_pair(
        &reference,
        &candidate,
        &request,
        &ffprobe,
        &cancel,
        options.fix_frame_rate,
    )
    .await?;
    if request.full_source {
        request.frame_count = left.count;
    }
    if left.geometry != right.geometry {
        return Err(fail(
            "Reference and candidate must have matching pixel format and color metadata. Prepare an explicitly matched reference first.",
        ));
    }
    let crop_requested = matches!(
        options.alignment,
        QualityAlignment::CropReference | QualityAlignment::CropAndResizeReference
    );
    let crop = if crop_requested {
        Some(
            detect_reference_crop(
                &reference,
                request.reference_stream_index,
                (request.reference_start_frame, request.frame_count),
                (left.width, left.height),
                &ffmpeg,
                left.vp9_decoder,
                &cancel,
            )
            .await?,
        )
    } else {
        None
    };
    let aligned = comparison(&left, &right, options.alignment, crop)?;
    let workspace = Workspace::create()?;
    let required = u64::from(request.frame_count.div_ceil(options.subsample))
        .saturating_mul(16 * 1024)
        .saturating_add(16 * 1024 * 1024);
    let available =
        crate::jobs::available_space(&workspace.0).map_err(|error| fail(error.message))?;
    if required > available {
        return Err(fail(format!(
            "Complete metric reporting needs up to {} MiB of temporary space; {} MiB is available.",
            required.div_ceil(1024 * 1024),
            available / (1024 * 1024)
        )));
    }
    let metric = match request.metric {
        QualityMetric::Psnr => "psnr=stats_file=quality.log:shortest=1:repeatlast=0".to_owned(),
        QualityMetric::Ssim => "ssim=stats_file=quality.log:shortest=1:repeatlast=0".to_owned(),
        QualityMetric::Vmaf => format!(
            "libvmaf=log_fmt=json:log_path=quality.log:model=version\\={}:n_threads=4:shortest=1:repeatlast=0",
            model_name(options.vmaf_model)
        ),
    };
    let sample = if options.subsample > 1 {
        format!("select=not(mod(n\\,{}))", options.subsample)
    } else {
        String::new()
    };
    let chain = |filters: &[String]| {
        let mut parts = filters.to_vec();
        if request.metric == QualityMetric::Vmaf
            && left
                .geometry
                .get("pix_fmt")
                .is_some_and(|format| format == "yuva420p")
        {
            parts.push("format=yuv420p".into());
        }
        if !sample.is_empty() {
            parts.push(sample.clone());
        }
        parts.push("settb=1/1".into());
        parts.push("setpts=N".into());
        parts.join(",")
    };
    // Input timestamps are checked unless fixed-rate pairing was requested;
    // both chains then receive identical ordinal timestamps after sampling.
    let filter = format!(
        "[0:{}]trim=start_frame={}:end_frame={},{}[ref];[1:{}]trim=start_frame={}:end_frame={},{}[dist];[dist][ref]{}[scored]",
        request.reference_stream_index,
        request.reference_start_frame,
        request.reference_start_frame + request.frame_count,
        chain(&aligned.reference),
        request.candidate_stream_index,
        request.candidate_start_frame,
        request.candidate_start_frame + request.frame_count,
        chain(&aligned.candidate),
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
    if left.vp9_decoder {
        let index = arguments.len() - 1;
        arguments.splice(index..index, ["-c:v".into(), "libvpx-vp9".into()]);
    }
    arguments.push(reference.path.as_os_str().into());
    arguments.extend(args(&[
        "-threads",
        "4",
        "-noautorotate",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]));
    if right.vp9_decoder {
        let index = arguments.len() - 1;
        arguments.splice(index..index, ["-c:v".into(), "libvpx-vp9".into()]);
    }
    arguments.push(candidate.path.as_os_str().into());
    arguments.extend(args(&[
        "-filter_complex_threads",
        "4",
        "-filter_complex",
        &filter,
        "-map",
        "[scored]",
        "-frames:v",
        &request.frame_count.div_ceil(options.subsample).to_string(),
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
        Duration::from_secs(86400),
    )
    .await
    .map_err(process_error)?;
    if !result.status.success() {
        return Err(fail(format!(
            "Quality measurement failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    let scored_count = request.frame_count.div_ceil(options.subsample);
    let report = workspace.0.join("quality.log");
    let metric = request.metric;
    let limit = if request.full_source {
        10_000
    } else {
        scored_count
    };
    let (mut score, mut points) = tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(report).map_err(|e| fail(e.to_string()))?;
        parse_report_reader(&mut file, metric, scored_count, limit)
    })
    .await
    .map_err(|e| fail(e.to_string()))??;
    for point in &mut points {
        point.frame *= options.subsample;
    }
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
    let crop_text = crop.map_or_else(
        || "no crop".into(),
        |c| format!("reference crop {}×{} at {},{}", c.width, c.height, c.x, c.y),
    );
    let pairing = if options.fix_frame_rate {
        "paired by frame number at fixed rate"
    } else {
        "verified matching source timing"
    };
    Ok(QualityResult {
        metric: request.metric,
        frame_count: scored_count,
        score,
        points,
        reference_fingerprint: ref_identity,
        candidate_fingerprint: candidate_identity,
        model: (request.metric == QualityMetric::Vmaf)
            .then(|| model_name(options.vmaf_model).into()),
        message: format!(
            "Scored {scored_count} of {} selected frames at {}×{} ({crop_text}; {pairing}). Higher scores indicate closer agreement. PSNR uses pooled mean-square error; infinite PSNR means identical decoded pixels. Metrics do not replace visual review.",
            request.frame_count, aligned.width, aligned.height
        ),
    })
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
    #[test]
    fn crop_consensus_keeps_full_picture_when_samples_disagree() {
        let samples = b"crop=192:112:8:8\ncrop=190:112:6:8\ncrop=192:112:8:8\n";
        assert_eq!(
            choose_crop(samples, 208, 128),
            Some(Crop {
                width: 194,
                height: 112,
                x: 6,
                y: 8
            })
        );
        assert_eq!(parse_crop("crop=300:112:8:8", 208, 128), None);
        assert_eq!(
            choose_crop(b"crop=192:112:7:8\ncrop=190:112:8:8\n", 208, 128),
            Some(Crop {
                width: 194,
                height: 112,
                x: 6,
                y: 8
            })
        );
    }
    #[test]
    fn anamorphic_width_rounds_to_even_display_frame() {
        assert_eq!(display_width(720, (32, 27)).unwrap(), 854);
        assert_eq!(display_width(144, (4, 3)).unwrap(), 192);
    }
}
