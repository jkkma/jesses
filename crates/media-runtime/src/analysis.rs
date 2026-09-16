//! Read-only, bounded source images and crop proposals. Every child uses the
//! same process-tree supervisor as encoding, and never writes beside a source.
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use media_core::{
    AppError, AutoCropRequest, AutoCropResult, CropSettings, FramePreviewRequest,
    FramePreviewResult,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, watch};

use crate::{
    discovery::find_executable,
    jobs::files::Source,
    supervisor::{CapturedOutput, CommandSpec, SupervisorError, run_capture},
};

const CAPTURE_BYTES: usize = 2 * 1024 * 1024;
const SAMPLE_POINTS: u32 = 10;
static ANALYSES: Semaphore = Semaphore::const_new(2);

fn error(code: &str, message: impl Into<String>, path: &Path) -> AppError {
    AppError::new(code, message, Some(path.to_string_lossy().into_owned()))
}

pub(crate) fn check_cancel(cancel: &watch::Receiver<bool>) -> Result<(), AppError> {
    if *cancel.borrow() || cancel.has_changed().is_err() {
        Err(AppError::new(
            "ANALYSIS_CANCELLED",
            "Source analysis was canceled.",
            None,
        ))
    } else {
        Ok(())
    }
}

async fn cancelled(mut cancel: watch::Receiver<bool>) {
    loop {
        if *cancel.borrow_and_update() || cancel.changed().await.is_err() {
            return;
        }
    }
}

pub(crate) async fn permit(
    cancel: &watch::Receiver<bool>,
) -> Result<tokio::sync::SemaphorePermit<'static>, AppError> {
    check_cancel(cancel)?;
    tokio::select! {
        biased;
        _ = cancelled(cancel.clone()) => Err(AppError::new("ANALYSIS_CANCELLED", "Source analysis was canceled.", None)),
        result = ANALYSES.acquire() => Ok(result.expect("analysis semaphore stays open")),
    }
}

pub(crate) async fn tool(name: &str, cancel: &watch::Receiver<bool>) -> Result<PathBuf, AppError> {
    check_cancel(cancel)?;
    let found = find_executable(&[name])
        .await
        .map_err(|detail| AppError::new("TOOL_DISCOVERY_FAILED", detail, None))?;
    check_cancel(cancel)?;
    found.ok_or_else(|| {
        AppError::new(
            "TOOL_MISSING",
            format!("{name} is required for source analysis. Install it and refresh Tools."),
            None,
        )
    })
}

async fn capture(
    executable: PathBuf,
    args: Vec<OsString>,
    source: &Source,
    cancel: &watch::Receiver<bool>,
    seconds: u64,
) -> Result<CapturedOutput, AppError> {
    check_cancel(cancel)?;
    source.verify()?;
    let result = run_capture(
        &CommandSpec {
            executable,
            args,
            cwd: None,
        },
        cancel.clone(),
        CAPTURE_BYTES,
        Duration::from_secs(seconds),
    )
    .await
    .map_err(|cause| match cause {
        SupervisorError::Cancelled => error(
            "ANALYSIS_CANCELLED",
            "Source analysis was canceled.",
            &source.path,
        ),
        SupervisorError::Timeout => error(
            "ANALYSIS_TIMEOUT",
            "Source analysis exceeded its time limit. Try another position or a shorter source.",
            &source.path,
        ),
        SupervisorError::OutputLimit => error(
            "ANALYSIS_OUTPUT_LIMIT",
            "Source analysis exceeded its bounded output limit.",
            &source.path,
        ),
        cause => error("ANALYSIS_FAILED", cause.to_string(), &source.path),
    })?;
    source.verify()?;
    check_cancel(cancel)?;
    if !result.status.success() {
        let diagnostic = String::from_utf8_lossy(&result.stderr);
        let diagnostic: String = diagnostic
            .chars()
            .rev()
            .take(1500)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(error(
            "ANALYSIS_FAILED",
            format!(
                "The media tool could not analyze this source: {}",
                diagnostic.trim()
            ),
            &source.path,
        ));
    }
    Ok(result)
}

#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    index: u32,
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<String>,
    sample_aspect_ratio: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_space: Option<String>,
    color_range: Option<String>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
    #[serde(default)]
    side_data_list: Vec<serde_json::Value>,
}

struct Input {
    source: Source,
    stream: Stream,
    width: u32,
    height: u32,
    duration: Option<f64>,
    fingerprint: String,
    ffmpeg: PathBuf,
}

/// Fixed work: metadata plus at most three 64 KiB regions. This is an ephemeral
/// inspection identity, never a substitute for an encoder's full validation.
pub(crate) fn fingerprint(source: &Source) -> Result<String, AppError> {
    let inspect = || -> std::io::Result<String> {
        let mut file = File::open(&source.path)?;
        let metadata = file.metadata()?;
        let mut hash = Sha256::new();
        hash.update(source.path.to_string_lossy().as_bytes());
        hash.update(metadata.len().to_le_bytes());
        hash.update(
            metadata
                .modified()?
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_le_bytes(),
        );
        let mut bytes = [0_u8; 65536];
        for offset in [
            0,
            metadata.len().saturating_sub(65536) / 2,
            metadata.len().saturating_sub(65536),
        ] {
            file.seek(SeekFrom::Start(offset))?;
            let count = usize::try_from((metadata.len() - offset).min(65536)).unwrap();
            file.read_exact(&mut bytes[..count])?;
            hash.update(offset.to_le_bytes());
            hash.update(&bytes[..count]);
        }
        Ok(format!("{:x}", hash.finalize()))
    };
    let result =
        inspect().map_err(|cause| error("FILE_UNREADABLE", cause.to_string(), &source.path))?;
    source.verify()?;
    Ok(result)
}

impl Input {
    fn finish(&self, cancel: &watch::Receiver<bool>) -> Result<(), AppError> {
        self.source.verify()?;
        if fingerprint(&self.source)? != self.fingerprint {
            return Err(error(
                "SOURCE_CHANGED",
                "The source changed during analysis. Import it again before using a preview or crop proposal.",
                &self.source.path,
            ));
        }
        check_cancel(cancel)
    }
}

fn seconds(text: Option<&str>) -> Option<f64> {
    text?
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n > 0.0)
}

async fn inspect(
    path: String,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<Input, AppError> {
    check_cancel(cancel)?;
    if path.contains('\0') || !Path::new(&path).is_absolute() {
        return Err(error(
            "INVALID_PATH",
            "Use an absolute local source path.",
            Path::new(&path),
        ));
    }
    let source = tokio::task::spawn_blocking(move || Source::open(Path::new(&path)))
        .await
        .map_err(|cause| AppError::new("ANALYSIS_FAILED", cause.to_string(), None))??;
    let fingerprint = fingerprint(&source)?;
    let ffprobe = tool("ffprobe", cancel).await?;
    let ffmpeg = tool("ffmpeg", cancel).await?;
    let mut args: Vec<OsString> = [
        "-v", "error", "-protocol_whitelist", "file", "-show_streams", "-show_format",
        "-show_entries", "stream=index,codec_type,width,height,duration,sample_aspect_ratio,color_transfer,color_primaries,color_space,color_range:stream_tags=rotate:stream_side_data=rotation:format=duration",
        "-of", "json", "-i",
    ].into_iter().map(OsString::from).collect();
    args.push(source.path.as_os_str().to_owned());
    let result = capture(ffprobe, args, &source, cancel, 15).await?;
    let document: Document = serde_json::from_slice(&result.stdout).map_err(|cause| {
        error(
            "ANALYSIS_FAILED",
            format!("Source metadata could not be read: {cause}"),
            &source.path,
        )
    })?;
    let stream = document
        .streams
        .into_iter()
        .find(|stream| stream.index == index && stream.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| {
            error(
                "STREAM_SELECTION_INVALID",
                "Choose an existing source video stream.",
                &source.path,
            )
        })?;
    let (Some(width), Some(height)) = (stream.width, stream.height) else {
        return Err(error(
            "ANALYSIS_INPUT_UNSUPPORTED",
            "The selected video has no reported dimensions.",
            &source.path,
        ));
    };
    if !(2..=8192).contains(&width) || !(2..=8192).contains(&height) {
        return Err(error(
            "ANALYSIS_INPUT_UNSUPPORTED",
            "Source preview dimensions must be between 2 and 8192 pixels.",
            &source.path,
        ));
    }
    let duration = seconds(stream.duration.as_deref()).or_else(|| {
        seconds(
            document
                .format
                .as_ref()
                .and_then(|format| format.duration.as_deref()),
        )
    });
    Ok(Input {
        source,
        stream,
        width,
        height,
        duration,
        fingerprint,
        ffmpeg,
    })
}

fn decoder_args(input: &Input, position: f64, verbose: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        if verbose { "info" } else { "error" },
        "-xerror",
        "-err_detect",
        "explode",
        "-noautorotate",
        // The transform is applied explicitly for display thumbnails. Do not
        // copy the matrix into PNG EXIF and rotate the image a second time.
        "-display_rotation:v",
        "0",
        "-threads",
        "2",
        "-filter_threads",
        "1",
        "-protocol_whitelist",
        "file",
        "-ss",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.extend([
        format!("{position:.9}").into(),
        "-i".into(),
        input.source.path.as_os_str().to_owned(),
        "-map".into(),
        format!("0:{}", input.stream.index).into(),
    ]);
    args.extend(
        ["-an", "-sn", "-dn", "-map_metadata", "-1"]
            .into_iter()
            .map(OsString::from),
    );
    args
}

fn preview_size(width: u32, height: u32) -> (u32, u32) {
    let scale = (960.0 / f64::from(width))
        .min(540.0 / f64::from(height))
        .min(1.0);
    let rounded = |value: u32| ((f64::from(value) * scale / 2.0).floor() as u32 * 2).max(2);
    (rounded(width), rounded(height))
}

/// Display thumbnails honor the source display matrix and SAR. Editing previews
/// intentionally keep coded coordinates so a crop never targets rotated pixels.
fn display_geometry(input: &Input) -> Result<(u32, u32, &'static str), AppError> {
    let rotation = input
        .stream
        .side_data_list
        .iter()
        .filter_map(|v| v.get("rotation"))
        .find_map(|v| v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok()))
        .or_else(|| {
            input
                .stream
                .tags
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("rotate"))?
                .1
                .parse()
                .ok()
        })
        .unwrap_or(0.0);
    let angle = rotation.rem_euclid(360.0);
    let quarter = (angle / 90.0).round();
    if !angle.is_finite() || (angle - quarter * 90.0).abs() > 0.01 {
        return Err(error(
            "ANALYSIS_INPUT_UNSUPPORTED",
            "Display previews support rotations in 90-degree steps. Use the coded source preview for other display matrices.",
            &input.source.path,
        ));
    }
    let sar = match input.stream.sample_aspect_ratio.as_deref() {
        None | Some("N/A" | "0:1") => 1.0,
        Some(value) => value
            .split_once(':')
            .and_then(|(n, d)| Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok()?))
            .filter(|v| v.is_finite() && *v > 0.0 && *v <= 100.0)
            .ok_or_else(|| {
                error(
                    "ANALYSIS_INPUT_UNSUPPORTED",
                    "The source pixel aspect ratio is invalid.",
                    &input.source.path,
                )
            })?,
    };
    let width = (f64::from(input.width) * sar).round().max(2.0) as u32;
    let (width, height, rotation_filter) = match quarter as u32 % 4 {
        1 => (input.height, width, "transpose=cclock,"),
        2 => (width, input.height, "hflip,vflip,"),
        3 => (input.height, width, "transpose=clock,"),
        _ => (width, input.height, ""),
    };
    let (width, height) = preview_size(width, height);
    Ok((width, height, rotation_filter))
}

fn preview_filter(input: &Input, width: u32, height: u32) -> Result<(String, bool), AppError> {
    let hdr = matches!(
        input.stream.color_transfer.as_deref(),
        Some("smpte2084" | "arib-std-b67")
    );
    let scale = format!("scale={width}:{height}:flags=lanczos");
    if hdr {
        if input.stream.color_primaries.as_deref() != Some("bt2020")
            || input.stream.color_space.as_deref() != Some("bt2020nc")
            || !matches!(input.stream.color_range.as_deref(), Some("tv" | "pc"))
        {
            return Err(error(
                "ANALYSIS_INPUT_UNSUPPORTED",
                "HDR display previews require reported BT.2020 color, nonconstant luminance and a known range.",
                &input.source.path,
            ));
        }
        // FFmpeg's CPU tone mapper consumes linear floating point RGB. Convert
        // primaries in linear light, then encode an SDR display image. This
        // filter never changes the source or the saved encoder HDR options.
        Ok((
            format!(
                "{scale},zscale=transfer=linear:npl=100,format=gbrpf32le,zscale=primaries=bt709,tonemap=tonemap=hable:desat=0,zscale=transfer=iec61966-2-1:matrix=bt709:range=full,format=rgb24,setsar=1"
            ),
            true,
        ))
    } else {
        Ok((format!("{scale},format=rgb24,setsar=1"), false))
    }
}

/// A single source frame, bounded to 960x540 and 2 MiB. Cancellation awaits the
/// owned tool tree; dropping the future also terminates it through the supervisor.
pub async fn preview_frame(
    request: FramePreviewRequest,
    cancel: watch::Receiver<bool>,
) -> Result<FramePreviewResult, AppError> {
    let _permit = permit(&cancel).await?;
    if !request.position_seconds.is_finite() || !(0.0..=86400.0).contains(&request.position_seconds)
    {
        return Err(AppError::new(
            "ANALYSIS_SETTINGS_INVALID",
            "Choose a preview position between zero and 24 hours.",
            None,
        ));
    }
    let input = inspect(request.input_path, request.video_stream_index, &cancel).await?;
    if input
        .duration
        .is_some_and(|duration| request.position_seconds >= duration)
    {
        return Err(error(
            "ANALYSIS_SETTINGS_INVALID",
            "Choose a preview position before the end of this source.",
            &input.source.path,
        ));
    }
    let (width, height, orientation) = if request.display_orientation == Some(true) {
        display_geometry(&input)?
    } else {
        let (w, h) = preview_size(input.width, input.height);
        (w, h, "")
    };
    let (filter, tone_mapped) = preview_filter(&input, width, height)?;
    let mut args = decoder_args(&input, request.position_seconds, false);
    args.extend(["-vf".into(), format!("{orientation}{filter}").into()]);
    args.extend(
        [
            "-frames:v",
            "1",
            "-fps_mode",
            "passthrough",
            "-c:v",
            "png",
            "-threads:v",
            "1",
            "-f",
            "image2pipe",
            "pipe:1",
        ]
        .into_iter()
        .map(OsString::from),
    );
    let output = capture(input.ffmpeg.clone(), args, &input.source, &cancel, 30).await?;
    if output.stdout.len() < 45
        || &output.stdout[..8] != b"\x89PNG\r\n\x1a\n"
        || &output.stdout[12..16] != b"IHDR"
        || u32::from_be_bytes(output.stdout[16..20].try_into().unwrap()) != width
        || u32::from_be_bytes(output.stdout[20..24].try_into().unwrap()) != height
        || !output.stdout.ends_with(b"\0\0\0\0IEND\xaeB`\x82")
    {
        return Err(error(
            "ANALYSIS_FAILED",
            "The selected position did not produce a complete bounded preview image.",
            &input.source.path,
        ));
    }
    input.finish(&cancel)?;
    Ok(FramePreviewResult {
        image_data_url: format!("data:image/png;base64,{}", STANDARD.encode(output.stdout)),
        width,
        height,
        source_width: input.width,
        source_height: input.height,
        position_seconds: request.position_seconds,
        source_fingerprint: input.fingerprint,
        tone_mapped,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Rectangle {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

fn rectangles(text: &[u8], width: u32, height: u32) -> Vec<Rectangle> {
    String::from_utf8_lossy(text)
        .lines()
        .filter_map(|line| {
            let value = line.rsplit_once("crop=")?.1.split_whitespace().next()?;
            let fields = value
                .split(':')
                .map(str::parse::<u32>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            let [w, h, x, y] = fields.as_slice() else {
                return None;
            };
            // Align outward to retain visible content. The existing encode contract
            // still requires even edges and a final picture of at least 64 pixels.
            let right = x.checked_add(*w)?;
            let bottom = y.checked_add(*h)?;
            if *w == 0 || *h == 0 || right > width || bottom > height {
                return None;
            }
            let x = x / 2 * 2;
            let y = y / 2 * 2;
            let right = right.div_ceil(2) * 2;
            let bottom = bottom.div_ceil(2) * 2;
            if right > width || bottom > height || right - x < 64 || bottom - y < 64 {
                return None;
            }
            Some(Rectangle {
                x,
                y,
                width: right - x,
                height: bottom - y,
            })
        })
        .take(64)
        .collect()
}

fn proposal(rectangles: &[Rectangle], width: u32, height: u32) -> (Option<CropSettings>, u8) {
    let mut counts = BTreeMap::new();
    for rectangle in rectangles {
        *counts.entry(*rectangle).or_insert(0_u32) += 1;
    }
    let Some((common, count)) = counts
        .iter()
        .max_by_key(|(rectangle, count)| (**count, rectangle.width * rectangle.height))
    else {
        return (None, 0);
    };
    let percent = ((*count * 100 + rectangles.len() as u32 / 2) / rectangles.len() as u32) as u8;
    let chosen = if *count * 100 > rectangles.len() as u32 * 80 {
        *common
    } else {
        // When samples disagree, retain the union of their visible areas. A
        // largest-area rectangle alone can cut content shifted in another scene.
        let x = rectangles.iter().map(|r| r.x).min().unwrap();
        let y = rectangles.iter().map(|r| r.y).min().unwrap();
        let right = rectangles.iter().map(|r| r.x + r.width).max().unwrap();
        let bottom = rectangles.iter().map(|r| r.y + r.height).max().unwrap();
        Rectangle {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    };
    (
        Some(CropSettings {
            left: chosen.x,
            top: chosen.y,
            right: width - chosen.x - chosen.width,
            bottom: height - chosen.y - chosen.height,
        }),
        percent,
    )
}

/// Sample at most 60 decoded frames across ten positions. The result is never
/// applied to a job; callers offer it for review and explicit draft application.
pub async fn detect_crop(
    request: AutoCropRequest,
    cancel: watch::Receiver<bool>,
) -> Result<AutoCropResult, AppError> {
    let _permit = permit(&cancel).await?;
    let input = inspect(request.input_path, request.video_stream_index, &cancel).await?;
    if input.width < 64
        || input.height < 64
        || !input.width.is_multiple_of(2)
        || !input.height.is_multiple_of(2)
    {
        return Err(error(
            "ANALYSIS_INPUT_UNSUPPORTED",
            "Automatic crop requires even source dimensions of at least 64 pixels.",
            &input.source.path,
        ));
    }
    let duration = input.duration.ok_or_else(|| {
        error(
            "ANALYSIS_INPUT_UNSUPPORTED",
            "Automatic crop needs a reported source duration; manual crop remains available.",
            &input.source.path,
        )
    })?;
    let mut found = Vec::new();
    for sample in 0..SAMPLE_POINTS {
        let position = duration * (f64::from(sample) + 0.5) / f64::from(SAMPLE_POINTS);
        let mut args = decoder_args(&input, position, true);
        args.extend(
            [
                "-vf",
                "cropdetect=limit=0.094117647:round=2:reset=0:skip=0",
                "-frames:v",
                "6",
                "-fps_mode",
                "passthrough",
                "-f",
                "null",
                "-",
            ]
            .into_iter()
            .map(OsString::from),
        );
        let output = capture(input.ffmpeg.clone(), args, &input.source, &cancel, 12).await?;
        found.extend(rectangles(&output.stderr, input.width, input.height));
    }
    input.finish(&cancel)?;
    let (crop, agreement_percent) = proposal(&found, input.width, input.height);
    let message = match crop {
        None => "The sampled frames did not establish a usable crop. Keep the manual values and inspect another preview.",
        Some(CropSettings { top: 0, right: 0, bottom: 0, left: 0 }) => "The sampled picture reaches the frame edges. Review the source before applying zero crop.",
        Some(_) if agreement_percent > 80 => "A consistent border was detected in sampled frames. Review the proposal before applying it; unsampled scenes may differ.",
        Some(_) => "Samples disagree. This proposal retains the combined visible area; review several positions before applying it.",
    }.to_owned();
    Ok(AutoCropResult {
        crop,
        source_width: input.width,
        source_height: input.height,
        sample_count: SAMPLE_POINTS,
        sampled_frames: found.len() as u32,
        agreement_percent,
        source_fingerprint: input.fingerprint,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_picture_positions_keep_the_union_not_only_the_largest_area() {
        let first = Rectangle {
            x: 0,
            y: 8,
            width: 160,
            height: 80,
        };
        let second = Rectangle {
            x: 32,
            y: 8,
            width: 160,
            height: 80,
        };
        let (crop, agreement) = proposal(&[first, second], 192, 112);
        assert_eq!(
            crop.unwrap(),
            CropSettings {
                left: 0,
                right: 0,
                top: 8,
                bottom: 24
            }
        );
        assert_eq!(agreement, 50);
        let mut samples = vec![first; 9];
        samples.push(second);
        assert_eq!(proposal(&samples, 192, 112).0.unwrap().right, 32);
    }

    #[test]
    fn malformed_dark_and_out_of_bounds_rectangles_are_not_crop_proposals() {
        let rows = b"crop=-4:8:0:0\ncrop=0:0:0:0\ncrop=64:64:999:0\ncrop=65:65:3:5\ncrop=1:1:0:0\ncrop=64:64:4294967295:0";
        assert_eq!(
            rectangles(rows, 192, 128),
            vec![Rectangle {
                x: 2,
                y: 4,
                width: 66,
                height: 66
            }]
        );
        assert_eq!(proposal(&[], 192, 128), (None, 0));
    }

    #[test]
    fn previews_are_bounded_without_upscaling() {
        assert_eq!(preview_size(3840, 2160), (960, 540));
        assert_eq!(preview_size(2160, 3840), (302, 540));
        assert_eq!(preview_size(128, 72), (128, 72));
        for (width, height) in [(2, 8192), (8192, 2), (8191, 8191)] {
            let (w, h) = preview_size(width, height);
            assert!(w <= 960 && h <= 540 && w >= 2 && h >= 2);
        }
    }

    #[tokio::test]
    async fn canceled_requests_do_not_open_a_source() {
        let (_owner, cancel) = watch::channel(true);
        let error = preview_frame(
            FramePreviewRequest {
                display_orientation: None,
                input_path: "missing".into(),
                video_stream_index: 0,
                position_seconds: 0.0,
            },
            cancel,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "ANALYSIS_CANCELLED");
    }
}
