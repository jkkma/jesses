//! Near-lossless, source-preserving temporal utility exports.
use super::*;
use crate::{jobs::qtgmc, supervisor};
use media_core::{DeinterlaceExportMethod, DeinterlaceExportRequest, DeinterlaceMode, FieldOrder};

const CRF: &str = "12";
const PRESET: &str = "medium";

fn rate(stream: &ProbeStream, path: &Path) -> Result<(u32, u32), AppError> {
    let value = stream
        .r_frame_rate
        .as_deref()
        .or(stream.avg_frame_rate.as_deref())
        .ok_or_else(|| {
            error(
                "UTILITY_SOURCE_INVALID",
                "The source has no usable frame rate.",
                Some(path),
            )
        })?;
    let (num, den) = value.split_once('/').ok_or_else(|| {
        error(
            "UTILITY_SOURCE_INVALID",
            "The source frame rate is invalid.",
            Some(path),
        )
    })?;
    let num: u32 = num.parse().map_err(|_| {
        error(
            "UTILITY_SOURCE_INVALID",
            "The source frame rate is invalid.",
            Some(path),
        )
    })?;
    let den: u32 = den.parse().map_err(|_| {
        error(
            "UTILITY_SOURCE_INVALID",
            "The source frame rate is invalid.",
            Some(path),
        )
    })?;
    if num == 0 || den == 0 || num > 240_000 || den > 100_000 {
        return Err(error(
            "UTILITY_SOURCE_INVALID",
            "The source frame rate is out of range.",
            Some(path),
        ));
    }
    Ok((num, den))
}

fn video<'a>(
    document: &'a ProbeDocument,
    index: u32,
    path: &Path,
) -> Result<&'a ProbeStream, AppError> {
    let selected = selected_stream(document, index, "video", path)?;
    if document
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .count()
        != 1
    {
        return Err(error(
            "UTILITY_SOURCE_INVALID",
            "Temporal export currently requires one video stream; choose a file with one picture track.",
            Some(path),
        ));
    }
    if document.streams.iter().any(|stream| {
        !matches!(
            stream.codec_type.as_deref(),
            Some("video" | "audio" | "subtitle" | "attachment")
        )
    }) {
        return Err(error(
            "UTILITY_SOURCE_INVALID",
            "The source contains a stream that Matroska temporal export cannot preserve.",
            Some(path),
        ));
    }
    if !matches!(
        selected.pix_fmt.as_deref(),
        Some("yuv420p" | "yuv420p10le" | "yuv422p" | "yuv422p10le" | "yuv444p" | "yuv444p10le")
    ) {
        return Err(error(
            "UTILITY_SOURCE_INVALID",
            "Temporal export requires planar 8-bit or 10-bit YUV input.",
            Some(path),
        ));
    }
    if matches!(
        selected.color_transfer.as_deref(),
        Some("smpte2084" | "arib-std-b67")
    ) || (selected.color_primaries.as_deref() == Some("bt2020")
        && !matches!(
            selected.color_transfer.as_deref(),
            Some("bt709" | "smpte170m")
        ))
    {
        return Err(error(
            "UTILITY_HDR_UNSUPPORTED",
            "Near-lossless temporal export cannot preserve HDR static metadata. Use the HDR-capable encode workflow or create a deliberate SDR version first.",
            Some(path),
        ));
    }
    rate(selected, path)?;
    Ok(selected)
}

fn source_frame_count(stream: &ProbeStream, path: &Path) -> Result<u64, AppError> {
    json_u64(stream.nb_read_frames.as_ref())
        .filter(|count| *count > 0)
        .ok_or_else(|| {
            error(
                "UTILITY_SOURCE_INVALID",
                "The complete source frame count could not be decoded.",
                Some(path),
            )
        })
}

fn allowed_tag(value: Option<&str>) -> Option<&str> {
    value.filter(|value| {
        matches!(
            *value,
            "bt709"
                | "bt470m"
                | "bt470bg"
                | "smpte170m"
                | "smpte240m"
                | "bt2020"
                | "bt2020nc"
                | "smpte2084"
                | "arib-std-b67"
                | "iec61966-2-1"
        )
    })
}

fn metadata_filter(video: &ProbeStream, interlaced: Option<FieldOrder>) -> String {
    let mut filters = Vec::new();
    if let Some(sar) = video.sample_aspect_ratio.as_deref().filter(|sar| {
        sar.split_once(':').is_some_and(|(n, d)| {
            n.parse::<u32>().is_ok_and(|n| n > 0) && d.parse::<u32>().is_ok_and(|d| d > 0)
        })
    }) {
        // FFprobe reports SAR as `num:den`, but FFmpeg uses ':' to separate
        // filter options. A slash keeps the ratio in one setsar expression.
        filters.push(format!("setsar={}", sar.replace(':', "/")));
    }
    let mut params = Vec::new();
    if let Some(field) = interlaced {
        params.push(format!(
            "field_mode={}",
            if field == FieldOrder::TopFirst {
                "tff"
            } else {
                "bff"
            }
        ));
    }
    if let Some(range) = video.color_range.as_deref() {
        match range {
            "tv" => params.push("range=limited".into()),
            "pc" => params.push("range=full".into()),
            _ => {}
        }
    }
    if let Some(primaries) = allowed_tag(video.color_primaries.as_deref()) {
        params.push(format!("color_primaries={primaries}"));
    }
    if let Some(transfer) = allowed_tag(video.color_transfer.as_deref()) {
        params.push(format!("color_trc={transfer}"));
    }
    if let Some(matrix) = allowed_tag(video.color_space.as_deref()) {
        params.push(format!("colorspace={matrix}"));
    }
    if !params.is_empty() {
        filters.push(format!("setparams={}", params.join(":")));
    }
    filters.join(",")
}

// Keep the selected stream, copied tracks and output timing explicit together.
#[allow(clippy::too_many_arguments)]
fn mapped_args(
    source: &Path,
    stream: &ProbeStream,
    document: &ProbeDocument,
    pipe: bool,
    fps: (u32, u32),
    filter: &str,
    interlaced: Option<FieldOrder>,
    output: &Path,
) -> Vec<OsString> {
    let mut args = vec![
        os("-hide_banner"),
        os("-nostdin"),
        os("-v"),
        os("warning"),
        os("-xerror"),
        os("-i"),
        os(source),
    ];
    if pipe {
        args.extend([os("-f"), os("yuv4mpegpipe"), os("-i"), os("pipe:0")]);
    }
    args.extend([
        os("-map"),
        os(if pipe {
            "1:v:0".to_string()
        } else {
            format!("0:{}", stream.index)
        }),
        os("-map"),
        os("0:a?"),
        os("-map"),
        os("0:s?"),
        os("-map"),
        os("0:t?"),
    ]);
    args.extend([
        os("-c:v"),
        os("libx264"),
        os("-crf"),
        os(CRF),
        os("-preset"),
        os(PRESET),
    ]);
    if !filter.is_empty() {
        args.extend([os("-vf"), os(filter)]);
    }
    if let Some(field) = interlaced {
        args.extend([
            os("-x264-params"),
            os(if field == FieldOrder::TopFirst {
                "tff=1"
            } else {
                "bff=1"
            }),
        ]);
    }
    args.extend([
        os("-pix_fmt"),
        os(stream.pix_fmt.as_deref().unwrap()),
        os("-r"),
        os(format!("{}/{}", fps.0, fps.1)),
        os("-fps_mode"),
        os("cfr"),
        os("-c:a"),
        os("copy"),
        os("-c:s"),
        os("copy"),
        os("-c:t"),
        os("copy"),
    ]);
    for (ordinal, subtitle) in document
        .streams
        .iter()
        .filter(|item| item.codec_type.as_deref() == Some("subtitle"))
        .enumerate()
    {
        if subtitle.codec_name.as_deref() == Some("mov_text") {
            args.extend([os(format!("-c:s:{ordinal}")), os("srt")]);
        }
    }
    args.extend([
        os("-map_metadata"),
        os("0"),
        os("-map_chapters"),
        os("0"),
        os("-f"),
        os("matroska"),
        os(output),
    ]);
    args
}

async fn packet_payloads(
    ffprobe: &Path,
    path: &Path,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<(u64, String), AppError> {
    let command = CommandSpec {
        executable: ffprobe.to_owned(),
        args: vec![
            os("-v"),
            os("error"),
            os("-select_streams"),
            os(index.to_string()),
            os("-show_packets"),
            os("-show_data_hash"),
            os("sha256"),
            os("-show_entries"),
            os("packet=data_hash"),
            os("-of"),
            os("csv=p=0"),
            os("-i"),
            os(path),
        ],
        cwd: None,
    };
    let captured = run_streaming_stdout(
        &command,
        cancel.clone(),
        OUTPUT_LIMIT,
        MEDIA_LIMIT,
        |reader| {
            let mut lines = BufReader::new(reader);
            let mut line = String::new();
            let mut hash = Sha256::new();
            let mut count = 0_u64;
            while lines
                .read_line(&mut line)
                .map_err(|cause| cause.to_string())?
                > 0
            {
                let value = line.trim().trim_end_matches(',');
                let hex = value
                    .strip_prefix("SHA256:")
                    .ok_or("A copied packet has no SHA-256 payload hash.")?;
                if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err("A copied packet has an invalid SHA-256 payload hash.".into());
                }
                hash.update(hex.as_bytes());
                hash.update([0]);
                count += 1;
                if count > 10_000_000 {
                    return Err("The copied stream exceeds the packet validation limit.".into());
                }
                line.clear();
            }
            Ok((count, format!("{:x}", hash.finalize())))
        },
    )
    .await
    .map_err(|cause| process_error(cause, Some(path)))?;
    if !captured.status.success() {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "Copied-packet inspection failed.",
            Some(path),
        ));
    }
    Ok(captured.value)
}

fn stable_tags(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> bool {
    before
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "DURATION" | "ENCODER" | "MUXINGAPP" | "WRITING_APP"
            )
        })
        .all(|(key, value)| after.get(key) == Some(value))
}

// Validation compares both media identities and the complete temporal plan.
#[allow(clippy::too_many_arguments)]
async fn validate_export(
    ffmpeg: &Path,
    ffprobe: &Path,
    source_path: &Path,
    source_document: &ProbeDocument,
    source_video: &ProbeStream,
    temp: &Temporary,
    expected_frames: u64,
    fps: (u32, u32),
    interlaced: Option<FieldOrder>,
    cancel: &watch::Receiver<bool>,
) -> Result<ProbeDocument, AppError> {
    temp.flush_nonempty_async().await?;
    let actual = probe(ffprobe, &temp.path, cancel, true).await?;
    let output_video = actual
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| {
            error(
                "UTILITY_VALIDATION_FAILED",
                "The output has no video stream.",
                Some(&temp.path),
            )
        })?;
    let actual_frames = source_frame_count(output_video, &temp.path)?;
    let output_rate = rate(output_video, &temp.path)?;
    if output_video.codec_name.as_deref() != Some("h264")
        || output_video.width != source_video.width
        || output_video.height != source_video.height
        || output_video.pix_fmt != source_video.pix_fmt
        || actual_frames != expected_frames
        || ((f64::from(output_rate.0) / f64::from(output_rate.1))
            - (f64::from(fps.0) / f64::from(fps.1)))
        .abs()
            > 0.01
        || !match interlaced {
            Some(FieldOrder::TopFirst) => {
                matches!(output_video.field_order.as_deref(), Some("tt" | "tb"))
            }
            Some(FieldOrder::BottomFirst) => {
                matches!(output_video.field_order.as_deref(), Some("bb" | "bt"))
            }
            None => output_video.field_order.as_deref() == Some("progressive"),
        }
    {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            format!(
                "The output video does not match the selected temporal export: {actual_frames}/{expected_frames} frames, {} fps, {:?} field order.",
                output_video.r_frame_rate.as_deref().unwrap_or("unknown"),
                output_video.field_order
            ),
            Some(&temp.path),
        ));
    }
    if source_video
        .sample_aspect_ratio
        .as_deref()
        .filter(|value| *value != "0:1")
        != output_video
            .sample_aspect_ratio
            .as_deref()
            .filter(|value| *value != "0:1")
        || source_video.color_range != output_video.color_range
        || source_video.color_primaries != output_video.color_primaries
        || source_video.color_transfer != output_video.color_transfer
        || source_video.color_space != output_video.color_space
    {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "The output changed source aspect or color declarations.",
            Some(&temp.path),
        ));
    }
    let source_other = source_document
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() != Some("video"))
        .collect::<Vec<_>>();
    let actual_other = actual
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() != Some("video"))
        .collect::<Vec<_>>();
    if source_other.len() != actual_other.len()
        || source_other
            .iter()
            .zip(actual_other.iter())
            .any(|(before, after)| {
                before.codec_type != after.codec_type
                    || (before.codec_name != after.codec_name
                        && !(before.codec_name.as_deref() == Some("mov_text")
                            && after.codec_name.as_deref() == Some("subrip")))
                    || before.extradata_size != after.extradata_size
                        && before.codec_type.as_deref() == Some("attachment")
                    || !stable_tags(&before.tags, &after.tags)
            })
    {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "The output did not preserve every non-video stream.",
            Some(&temp.path),
        ));
    }
    for (before, after) in source_other.iter().zip(actual_other.iter()) {
        if matches!(before.codec_type.as_deref(), Some("audio" | "subtitle"))
            && before.codec_name == after.codec_name
        {
            let expected = packet_payloads(ffprobe, source_path, before.index, cancel).await?;
            let observed = packet_payloads(ffprobe, &temp.path, after.index, cancel).await?;
            if expected != observed {
                return Err(error(
                    "UTILITY_VALIDATION_FAILED",
                    format!(
                        "Copied {} stream {} changed packet payloads or packet order.",
                        before.codec_type.as_deref().unwrap_or("non-video"),
                        before.index
                    ),
                    Some(&temp.path),
                ));
            }
        }
    }
    if source_document.chapters.len() != actual.chapters.len()
        || source_document
            .chapters
            .iter()
            .zip(actual.chapters.iter())
            .any(|(before, after)| {
                before.start_time != after.start_time
                    || before.end_time != after.end_time
                    || !stable_tags(&before.tags, &after.tags)
            })
        || source_document
            .format
            .as_ref()
            .zip(actual.format.as_ref())
            .is_some_and(|(before, after)| !stable_tags(&before.tags, &after.tags))
    {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "The output changed chapter or container metadata.",
            Some(&temp.path),
        ));
    }
    if duration(source_document)
        .zip(duration(&actual))
        .is_some_and(|(before, after)| (before - after).abs() > 0.1)
    {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "The output duration differs from the source by over 100 ms.",
            Some(&temp.path),
        ));
    }
    validate_full_decode(ffmpeg, &temp.path, cancel).await?;
    Ok(actual)
}

pub(super) async fn deinterlace(
    request: DeinterlaceExportRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    let source = Source::open(valid_absolute(&request.input_path)?)?;
    let output = destination(&request.output_path, &[&source])?;
    require_mkv(&output, "Deinterlace export")?;
    let ffmpeg = tool("ffmpeg", "Deinterlace export").await?;
    let ffprobe = tool("ffprobe", "Deinterlace export").await?;
    let input = probe(&ffprobe, &source.path, cancel, true).await?;
    let picture = video(&input, request.video_stream_index, &source.path)?.clone();
    let frames = source_frame_count(&picture, &source.path)?;
    let (num, den) = rate(&picture, &source.path)?;
    let multiplier = if request.mode == DeinterlaceMode::Bob {
        2
    } else {
        1
    };
    let expected = frames.checked_mul(multiplier).ok_or_else(|| {
        error(
            "UTILITY_SOURCE_INVALID",
            "Too many frames to deinterlace.",
            Some(&source.path),
        )
    })?;
    let output_fps = (
        num.checked_mul(multiplier as u32).ok_or_else(|| {
            error(
                "UTILITY_SOURCE_INVALID",
                "The doubled frame rate is out of range.",
                Some(&source.path),
            )
        })?,
        den,
    );
    let temporary = Temporary::create(&output, &nonce("deinterlace"))?;
    let scratch = Scratch::create("deinterlace")?;
    match request.method {
        DeinterlaceExportMethod::Qtgmc => {
            let settings = media_core::QtgmcSettings {
                mode: request.mode,
                field_order: request.field_order,
                preset: request.qtgmc_preset,
            };
            let runtime = qtgmc::runtime(cancel).await?;
            qtgmc::check_tools(
                &runtime.vspipe,
                runtime.environment.as_ref(),
                &scratch.path,
                settings,
                cancel,
            )
            .await?;
            let cache = scratch.path.join("source.lwi");
            let script = scratch.path.join("deinterlace.vpy");
            fs::write(
                &script,
                qtgmc::source_script(&source.path, &cache, picture.index, None, settings)?,
            )
            .map_err(|cause| error("UTILITY_SCRIPT_FAILED", cause.to_string(), Some(&script)))?;
            let producer = CommandSpec {
                executable: runtime.vspipe,
                args: vec![os(&script), os("-"), os("-c"), os("y4m"), os("--progress")],
                cwd: Some(scratch.path.clone()),
            };
            let filter = metadata_filter(&picture, None);
            let consumer = CommandSpec {
                executable: ffmpeg.clone(),
                args: mapped_args(
                    &source.path,
                    &picture,
                    &input,
                    true,
                    output_fps,
                    &filter,
                    None,
                    Path::new("pipe:1"),
                ),
                cwd: None,
            };
            let (events, mut receiver) = tokio::sync::mpsc::channel(128);
            let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
            let log = scratch.path.join("deinterlace.log");
            let result = if let Some(environment) = &runtime.environment {
                supervisor::run_pipeline_to_file_with_producer_environment(
                    &producer,
                    &consumer,
                    cancel.clone(),
                    events,
                    &log,
                    MEDIA_LIMIT,
                    temporary.clone_file()?,
                    environment,
                )
                .await
            } else {
                supervisor::run_pipeline_to_file(
                    &producer,
                    &consumer,
                    cancel.clone(),
                    events,
                    &log,
                    MEDIA_LIMIT,
                    temporary.clone_file()?,
                )
                .await
            }
            .map_err(|cause| process_error(cause, Some(&source.path)))?;
            drain.await.map_err(|cause| {
                error("UTILITY_TOOL_FAILED", cause.to_string(), Some(&source.path))
            })?;
            if !result.producer_status.success() || !result.consumer_status.success() {
                return Err(error(
                    "UTILITY_TOOL_FAILED",
                    "VSPipe or FFmpeg failed during QTGMC export.",
                    Some(&source.path),
                ));
            }
        }
        DeinterlaceExportMethod::Bwdif | DeinterlaceExportMethod::Yadif => {
            let name = if request.method == DeinterlaceExportMethod::Bwdif {
                "bwdif"
            } else {
                "yadif"
            };
            let mode = if request.mode == DeinterlaceMode::Bob {
                "send_field"
            } else {
                "send_frame"
            };
            let parity = if request.field_order == FieldOrder::TopFirst {
                "tff"
            } else {
                "bff"
            };
            let metadata = metadata_filter(&picture, None);
            let filter = format!(
                "{name}=mode={mode}:parity={parity}:deint=all{}{}",
                if metadata.is_empty() { "" } else { "," },
                metadata
            );
            let mut args = mapped_args(
                &source.path,
                &picture,
                &input,
                false,
                output_fps,
                &filter,
                None,
                &temporary.path,
            );
            args.splice(0..0, [os("-y")]);
            let mut temporary = temporary;
            temporary.close_for_path_writer()?;
            let encoded = checked(
                ffmpeg.clone(),
                args,
                cancel,
                MEDIA_LIMIT,
                Some(&source.path),
                "Deinterlace export",
            )
            .await;
            temporary.reopen_after_path_writer()?;
            encoded?;
            let actual = validate_export(
                &ffmpeg,
                &ffprobe,
                &source.path,
                &input,
                &picture,
                &temporary,
                expected,
                output_fps,
                None,
                cancel,
            )
            .await?;
            let bytes = publish_media(temporary, &output, &[&source]).await?;
            return Ok(UtilityResult::Artifact(UtilityArtifact {
                operation: "deinterlaceExport".into(),
                output_path: output.to_string_lossy().into_owned(),
                size_bytes: bytes.to_string(),
                duration_seconds: duration(&actual),
                source_fingerprints: vec![fingerprint(&source)?],
                message: format!(
                    "Exported progressive near-lossless H.264 at {}/{} fps using {}.",
                    output_fps.0, output_fps.1, name
                ),
                diagnostics: vec![format!(
                    "Validated {expected} decoded frames, source tracks, and complete output decode."
                )],
            }));
        }
    }
    let actual = validate_export(
        &ffmpeg,
        &ffprobe,
        &source.path,
        &input,
        &picture,
        &temporary,
        expected,
        output_fps,
        None,
        cancel,
    )
    .await?;
    let bytes = publish_media(temporary, &output, &[&source]).await?;
    Ok(UtilityResult::Artifact(UtilityArtifact {
        operation: "deinterlaceExport".into(),
        output_path: output.to_string_lossy().into_owned(),
        size_bytes: bytes.to_string(),
        duration_seconds: duration(&actual),
        source_fingerprints: vec![fingerprint(&source)?],
        message: format!(
            "Exported progressive near-lossless H.264 at {}/{} fps using QTGMC.",
            output_fps.0, output_fps.1
        ),
        diagnostics: vec![format!(
            "Validated {expected} decoded frames, source tracks, and complete output decode."
        )],
    }))
}

const CADENCE_SCRIPT: &str = r#"
import vapoursynth as vs
import bisect
import sys
core = vs.core
clip = core.bs.VideoSource(source=SOURCE, cachepath=CACHE)
n = clip.num_frames
if TARGET < 1 or n / float(TARGET) <= 1.02:
    raise RuntimeError('source has no duplicate-frame padding above the 2% repair threshold')
stats = core.std.PlaneStats(clip, clip[0] + clip[:-1])
diff = [f.props['PlaneStatsDiff'] for f in stats.frames()]
diff[0] = 1e9
times = []
with open(TIMES_FILE, 'r') as input_times:
    for line in input_times:
        if line.strip():
            times.append(float(line))
if abs(len(times) - n) > 10:
    raise RuntimeError('decoded timestamps and BestSource frames differ by more than ten')
times = times[:n]
guess = (times[-1] - times[0]) / max(len(times) - 1, 1)
while len(times) < n:
    times.append(times[-1] + guess)
t0 = times[0]
step = float(OUT_DEN) / OUT_NUM
keep = []
prev = -1
worst = 0.0
for k in range(TARGET):
    want = t0 + k * step
    j = bisect.bisect_left(times, want)
    best, score = None, -1.0
    lo, hi = max(prev + 1, j - 2), min(n, j + 3)
    for candidate in range(lo, hi):
        if abs(times[candidate] - want) <= 0.5 * step and diff[candidate] > score:
            best, score = candidate, diff[candidate]
    if best is None:
        for candidate in range(lo, hi):
            if best is None or abs(times[candidate] - want) < abs(times[best] - want):
                best = candidate
    if best is None:
        best = min(max(prev + 1, j), n - 1)
        if best <= prev:
            raise RuntimeError('source ran out of frames before target duration')
    keep.append(best)
    prev = best
    worst = max(worst, abs(times[best] - want))
kept = set(keep)
drop = [index for index in range(n) if index not in kept]
out = core.std.DeleteFrames(clip, drop)
out = core.std.AssumeFPS(out, fpsnum=OUT_NUM, fpsden=OUT_DEN)
print('Jesses: %d coded, %d target, %d dropped; worst placement %.6fs' % (n, TARGET, len(drop), worst), file=sys.stderr)
out.set_output()
"#;

fn python_literal(path: &Path) -> Result<String, AppError> {
    let value = path.to_str().ok_or_else(|| {
        error(
            "UTILITY_SOURCE_INVALID",
            "VapourSynth paths must be valid Unicode.",
            Some(path),
        )
    })?;
    serde_json::to_string(value)
        .map_err(|cause| error("UTILITY_SCRIPT_FAILED", cause.to_string(), Some(path)))
}

async fn write_frame_times(
    ffprobe: &Path,
    source: &Path,
    stream_index: u32,
    destination: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<u64, AppError> {
    let destination_owned = destination.to_owned();
    let command = CommandSpec {
        executable: ffprobe.to_owned(),
        args: vec![
            os("-v"),
            os("error"),
            os("-select_streams"),
            os(stream_index.to_string()),
            os("-show_entries"),
            os("frame=best_effort_timestamp_time"),
            os("-of"),
            os("csv=p=0"),
            os("-i"),
            os(source),
        ],
        cwd: None,
    };
    let capture = run_streaming_stdout(
        &command,
        cancel.clone(),
        OUTPUT_LIMIT,
        MEDIA_LIMIT,
        move |reader| {
            let mut lines = BufReader::new(reader);
            let mut writer =
                fs::File::create(&destination_owned).map_err(|cause| cause.to_string())?;
            let mut line = String::new();
            let mut count = 0_u64;
            let mut last = 0_f64;
            while lines
                .read_line(&mut line)
                .map_err(|cause| cause.to_string())?
                > 0
            {
                let text = line
                    .trim()
                    .trim_end_matches(',')
                    .split(',')
                    .next()
                    .unwrap_or("");
                if !text.is_empty() {
                    let time = text
                        .parse::<f64>()
                        .ok()
                        .filter(|time| time.is_finite() && *time >= 0.0)
                        .unwrap_or(last);
                    last = last.max(time);
                    writeln!(writer, "{last:.6}").map_err(|cause| cause.to_string())?;
                    count += 1;
                    if count > 5_000_000 {
                        return Err(
                            "The source has too many frames for bounded cadence repair.".into()
                        );
                    }
                }
                line.clear();
            }
            writer.sync_all().map_err(|cause| cause.to_string())?;
            if count < 2 {
                return Err("The source has too few decoded timestamps to repair.".into());
            }
            Ok(count)
        },
    )
    .await
    .map_err(|cause| process_error(cause, Some(source)))?;
    if !capture.status.success() {
        return Err(error(
            "UTILITY_SOURCE_INVALID",
            "FFprobe could not decode source frame timestamps.",
            Some(source),
        ));
    }
    Ok(capture.value)
}

pub(super) async fn repair_cadence(
    request: media_core::CadenceRepairExportRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    let source = Source::open(valid_absolute(&request.input_path)?)?;
    let output = destination(&request.output_path, &[&source])?;
    require_mkv(&output, "Cadence repair export")?;
    let ffmpeg = tool("ffmpeg", "Cadence repair export").await?;
    let ffprobe = tool("ffprobe", "Cadence repair export").await?;
    let input = probe(&ffprobe, &source.path, cancel, true).await?;
    let picture = video(&input, request.video_stream_index, &source.path)?.clone();
    let coded = source_frame_count(&picture, &source.path)?;
    let fps = rate(&picture, &source.path)?;
    let seconds = duration(&input)
        .filter(|duration| *duration > 0.0)
        .ok_or_else(|| {
            error(
                "UTILITY_SOURCE_INVALID",
                "The source has no measured duration to repair against.",
                Some(&source.path),
            )
        })?;
    let target = (seconds * f64::from(fps.0) / f64::from(fps.1)).round();
    if !(1.0..=5_000_000.0).contains(&target) {
        return Err(error(
            "UTILITY_SOURCE_INVALID",
            "The source duration and frame rate yield an invalid target count.",
            Some(&source.path),
        ));
    }
    let target = target as u64;
    if (coded as f64) / (target as f64) <= 1.02 {
        return Err(error(
            "UTILITY_NOT_NEEDED",
            format!(
                "The source has {coded} decoded frames and its duration at the declared rate calls for {target}; duplicate-frame padding does not exceed 2%."
            ),
            Some(&source.path),
        ));
    }
    if !matches!(
        picture.field_order.as_deref(),
        Some("tt" | "tb" | "bb" | "bt")
    ) {
        return Err(error(
            "UTILITY_SOURCE_INVALID",
            "Cadence repair preserves interlaced fields and requires a declared top-first or bottom-first source.",
            Some(&source.path),
        ));
    }
    let field = if matches!(picture.field_order.as_deref(), Some("tt" | "tb")) {
        FieldOrder::TopFirst
    } else {
        FieldOrder::BottomFirst
    };
    let runtime = qtgmc::runtime(cancel).await?;
    let scratch = Scratch::create("cadence")?;
    let times = scratch.path.join("frame-times.txt");
    let timestamp_count =
        write_frame_times(&ffprobe, &source.path, picture.index, &times, cancel).await?;
    if coded.abs_diff(timestamp_count) > 10 {
        return Err(error(
            "UTILITY_SOURCE_INVALID",
            "Decoded timestamps and counted source frames differ by more than ten.",
            Some(&source.path),
        ));
    }
    let script = scratch.path.join("cadence.vpy");
    let content = format!(
        "SOURCE = {}\nCACHE = {}\nTIMES_FILE = {}\nTARGET = {}\nOUT_NUM = {}\nOUT_DEN = {}\n{}",
        python_literal(&source.path)?,
        python_literal(&scratch.path)?,
        python_literal(&times)?,
        target,
        fps.0,
        fps.1,
        CADENCE_SCRIPT
    );
    fs::write(&script, content)
        .map_err(|cause| error("UTILITY_SCRIPT_FAILED", cause.to_string(), Some(&script)))?;
    let temporary = Temporary::create(&output, &nonce("cadence"))?;
    let producer = CommandSpec {
        executable: runtime.vspipe,
        args: vec![os(&script), os("-"), os("-c"), os("y4m"), os("--progress")],
        cwd: Some(scratch.path.clone()),
    };
    let metadata = metadata_filter(&picture, Some(field));
    let consumer = CommandSpec {
        executable: ffmpeg.clone(),
        args: mapped_args(
            &source.path,
            &picture,
            &input,
            true,
            fps,
            &metadata,
            Some(field),
            Path::new("pipe:1"),
        ),
        cwd: None,
    };
    let (events, mut receiver) = tokio::sync::mpsc::channel(128);
    let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    let log = scratch.path.join("cadence.log");
    let result = if let Some(environment) = &runtime.environment {
        supervisor::run_pipeline_to_file_with_producer_environment(
            &producer,
            &consumer,
            cancel.clone(),
            events,
            &log,
            MEDIA_LIMIT,
            temporary.clone_file()?,
            environment,
        )
        .await
    } else {
        supervisor::run_pipeline_to_file(
            &producer,
            &consumer,
            cancel.clone(),
            events,
            &log,
            MEDIA_LIMIT,
            temporary.clone_file()?,
        )
        .await
    }
    .map_err(|cause| process_error(cause, Some(&source.path)))?;
    drain
        .await
        .map_err(|cause| error("UTILITY_TOOL_FAILED", cause.to_string(), Some(&source.path)))?;
    if !result.producer_status.success() || !result.consumer_status.success() {
        return Err(error(
            "UTILITY_TOOL_FAILED",
            "BestSource, VSPipe, or FFmpeg failed during cadence repair.",
            Some(&source.path),
        ));
    }
    let actual = validate_export(
        &ffmpeg,
        &ffprobe,
        &source.path,
        &input,
        &picture,
        &temporary,
        target,
        fps,
        Some(field),
        cancel,
    )
    .await?;
    let bytes = publish_media(temporary, &output, &[&source]).await?;
    Ok(UtilityResult::Artifact(UtilityArtifact {
        operation: "cadenceRepairExport".into(),
        output_path: output.to_string_lossy().into_owned(),
        size_bytes: bytes.to_string(),
        duration_seconds: duration(&actual),
        source_fingerprints: vec![fingerprint(&source)?],
        message: format!(
            "Removed {} padded frames into a near-lossless interlaced H.264 Matroska file.",
            coded - target
        ),
        diagnostics: vec![format!(
            "Validated {target} decoded frames at {}/{} fps with original field order and source tracks.",
            fps.0, fps.1
        )],
    }))
}
