//! Source resume retains encoded chunks. These internal contracts bind every
//! decoded frame property to the already hashed chunk, independently of new UI
//! settings and of mutable engine completion counters.
use super::{
    check_cancel,
    encode::{self, frame_scan},
    encode_plan::Frame,
    files::{self, Source},
    metadata::{Document, Stream},
    process_error,
};
use crate::supervisor::{self, CommandSpec};
use media_core::{AppError, VideoEncoder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path, sync::Arc, time::Duration};
use tokio::sync::watch;

fn invalid(path: &Path, message: impl Into<String>) -> AppError {
    files::error("RECOVERY_INVALID", message, path)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::jobs) struct ChunkContract {
    pub source_frames: u64,
    pub frames: u64,
    pub bytes: u64,
    pub file_sha256: [u8; 32],
    shape_sha256: [u8; 32],
    first: Frame,
    header: Header,
}

#[derive(Clone, Copy)]
pub(in crate::jobs) enum CountPolicy {
    ExactSourceFrames,
    /// Activated only by the independently verified frozen engine queue flag.
    #[allow(dead_code)]
    SourceTypedResample,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Header {
    codec: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pixel: Option<String>,
    sar: Option<String>,
    primaries: Option<String>,
    transfer: Option<String>,
    matrix: Option<String>,
    range: Option<String>,
    chroma: Option<String>,
    side_data: Vec<serde_json::Value>,
    rate: Option<(u32, u32)>,
    duration: Option<String>,
}
fn normalized(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !matches!(*value, "unknown" | "unspecified" | "N/A"))
}
pub(super) fn stream_rate(stream: &Stream) -> Option<(u32, u32)> {
    stream
        .avg_frame_rate
        .as_deref()
        .or(stream.r_frame_rate.as_deref())
        .and_then(|rate| rate.split_once('/'))
        .and_then(|(n, d)| Some((n.parse::<u32>().ok()?, d.parse::<u32>().ok()?)))
        .filter(|(n, d)| *n > 0 && *d > 0)
}
impl Header {
    fn from_stream(stream: &Stream) -> Self {
        Self {
            codec: stream.codec_name.clone(),
            width: stream.width,
            height: stream.height,
            pixel: stream.pix_fmt.clone(),
            sar: Some(
                stream
                    .sample_aspect_ratio
                    .as_deref()
                    .filter(|value| !matches!(*value, "0:1" | "N/A"))
                    .unwrap_or("1:1")
                    .into(),
            ),
            primaries: stream.color_primaries.clone(),
            transfer: stream.color_transfer.clone(),
            matrix: stream.color_space.clone(),
            range: stream.color_range.clone(),
            chroma: stream.chroma_location.clone(),
            side_data: stream.side_data_list.clone(),
            rate: stream_rate(stream),
            duration: stream.duration.clone().or_else(|| {
                stream
                    .tags
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case("duration"))
                    .map(|(_, value)| value.clone())
            }),
        }
    }
    fn verify(&self, stream: &Stream, allow_absent_vp9: bool) -> Result<(), AppError> {
        let actual = Self::from_stream(stream);
        let optional = |actual: &Option<String>, expected: &Option<String>| {
            normalized(actual.as_deref()) == normalized(expected.as_deref())
                || (allow_absent_vp9 && actual.is_none())
        };
        if actual.codec != self.codec
            || actual.width != self.width
            || actual.height != self.height
            || actual.pixel != self.pixel
            || actual.sar != self.sar
            || !optional(&actual.primaries, &self.primaries)
            || !optional(&actual.transfer, &self.transfer)
            || !optional(&actual.matrix, &self.matrix)
            || !optional(&actual.range, &self.range)
            || !optional(&actual.chroma, &self.chroma)
        {
            return Err(invalid(
                Path::new(""),
                "The video container header differs from the first sealed source chunk.",
            ));
        }
        for expected in self.side_data.iter().filter(|value| {
            matches!(
                value
                    .get("side_data_type")
                    .and_then(serde_json::Value::as_str),
                Some(
                    "Mastering display metadata"
                        | "Content light level metadata"
                        | "DOVI configuration record"
                )
            )
        }) {
            if !actual.side_data.contains(expected) {
                return Err(invalid(
                    Path::new(""),
                    "The video header lost or changed sealed source HDR metadata.",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(in crate::jobs) struct Regions {
    chunks: Vec<ChunkContract>,
    frames: u64,
    source_frames: u64,
    reference_header: Option<Header>,
    reference_clock: Option<super::av1an::source_queue_concat::Clock>,
}
impl Regions {
    pub(in crate::jobs) fn new(chunks: Vec<ChunkContract>) -> Result<Arc<Self>, AppError> {
        if chunks.is_empty()
            || chunks.len() > 100000
            || chunks.iter().any(|chunk| {
                chunk.source_frames == 0
                    || chunk.frames == 0
                    || chunk.first.source_queue_dimensions().is_none()
            })
        {
            return Err(invalid(
                Path::new(""),
                "A source queue requires complete nonempty decoded chunk contracts.",
            ));
        }
        let frames = chunks
            .iter()
            .try_fold(0u64, |sum, chunk| sum.checked_add(chunk.frames))
            .ok_or_else(|| invalid(Path::new(""), "Source output frame count overflow."))?;
        let source_frames = chunks
            .iter()
            .try_fold(0u64, |sum, chunk| sum.checked_add(chunk.source_frames))
            .ok_or_else(|| invalid(Path::new(""), "Source ordinal count overflow."))?;
        usize::try_from(frames)
            .map_err(|_| invalid(Path::new(""), "Source output count exceeds this platform."))?;
        Ok(Arc::new(Self {
            chunks,
            frames,
            source_frames,
            reference_header: None,
            reference_clock: None,
        }))
    }
    pub(in crate::jobs) fn frames(&self) -> u64 {
        self.frames
    }
    pub(in crate::jobs) fn source_frames(&self) -> u64 {
        self.source_frames
    }
    pub(in crate::jobs) fn first_dimensions(&self) -> (u32, u32) {
        self.chunks[0]
            .first
            .source_queue_dimensions()
            .expect("validated contract")
    }
    pub(in crate::jobs) fn first_sar(&self) -> &str {
        self.chunks[0].header.sar.as_deref().unwrap_or("1:1")
    }
    pub(in crate::jobs) fn reference_rate(&self) -> Option<(u32, u32)> {
        self.reference_header
            .as_ref()
            .and_then(|header| header.rate)
    }
    pub(in crate::jobs) fn reference_duration(&self) -> Option<f64> {
        let value = self.reference_header.as_ref()?.duration.as_deref()?;
        let seconds = value.parse::<f64>().ok().or_else(|| {
            let mut parts = value.split(':');
            let (h, m, s) = (
                parts.next()?.parse::<f64>().ok()?,
                parts.next()?.parse::<f64>().ok()?,
                parts.next()?.parse::<f64>().ok()?,
            );
            parts.next().is_none().then_some(h * 3600.0 + m * 60.0 + s)
        })?;
        (seconds.is_finite() && seconds > 0.0).then_some(seconds)
    }
    pub(in crate::jobs) fn with_reference_header(
        mut self: Arc<Self>,
        stream: &Stream,
        clock: super::av1an::source_queue_concat::Clock,
    ) -> Result<Arc<Self>, AppError> {
        if self.first_dimensions()
            != stream.width.zip(stream.height).ok_or_else(|| {
                invalid(
                    Path::new(""),
                    "The independent join has no first-frame dimensions.",
                )
            })?
        {
            return Err(invalid(
                Path::new(""),
                "The independent join header differs from the sealed first decoded chunk.",
            ));
        }
        if clock.frames != self.frames || clock.time_base.0 == 0 || clock.time_base.1 == 0 {
            return Err(invalid(
                Path::new(""),
                "The independent decoded join clock does not cover every sealed output frame.",
            ));
        }
        let regions = Arc::make_mut(&mut self);
        regions.reference_header = Some(Header::from_stream(stream));
        regions.reference_clock = Some(clock);
        Ok(self)
    }
    pub(in crate::jobs) fn verify_header(
        &self,
        stream: &Stream,
        allow_absent_vp9: bool,
    ) -> Result<(), AppError> {
        self.reference_header
            .as_ref()
            .unwrap_or(&self.chunks[0].header)
            .verify(stream, allow_absent_vp9)
    }
    pub(in crate::jobs) fn cursor(self: &Arc<Self>, origin_shift: f64) -> Cursor {
        Cursor {
            regions: self.clone(),
            chunk: 0,
            count: 0,
            digest: [0; 32],
            clock_digest: [0; 32],
            clock_count: 0,
            previous_time: None,
            origin_shift,
        }
    }
}

#[derive(Clone)]
pub(in crate::jobs) struct Cursor {
    regions: Arc<Regions>,
    chunk: usize,
    count: u64,
    digest: [u8; 32],
    clock_digest: [u8; 32],
    clock_count: u64,
    previous_time: Option<f64>,
    origin_shift: f64,
}
impl Cursor {
    pub(in crate::jobs) fn has_reference_clock(&self) -> bool {
        self.regions.reference_clock.is_some()
    }
    pub(in crate::jobs) fn push(&mut self, frame: &Frame) -> Result<(), AppError> {
        if let Some(clock) = &self.regions.reference_clock {
            let time = frame.source_queue_time().ok_or_else(|| {
                invalid(
                    Path::new(""),
                    "The source join frame has no finite presentation timestamp.",
                )
            })?;
            if !self.origin_shift.is_finite()
                || self.previous_time.is_some_and(|previous| time <= previous)
            {
                return Err(invalid(
                    Path::new(""),
                    "The source join decoded frame clock is not strictly increasing.",
                ));
            }
            self.previous_time = Some(time);
            let tick = ((time - self.origin_shift) * f64::from(clock.time_base.1)
                / f64::from(clock.time_base.0))
            .round();
            if !tick.is_finite() || tick < i64::MIN as f64 || tick > i64::MAX as f64 {
                return Err(invalid(
                    Path::new(""),
                    "The source join timestamp exceeds its verified timebase.",
                ));
            }
            let relative = (tick as i64)
                .checked_sub(clock.origin_ticks)
                .ok_or_else(|| {
                    invalid(Path::new(""), "Source join relative timestamp overflow.")
                })?;
            let mut digest = Sha256::new();
            digest.update(self.clock_digest);
            digest.update(relative.to_le_bytes());
            self.clock_digest = digest.finalize().into();
            self.clock_count += 1;
        }
        let expected = self.regions.chunks.get(self.chunk).ok_or_else(|| {
            invalid(
                Path::new(""),
                "The final video contains frames outside the sealed source queue.",
            )
        })?;
        self.digest = shape_hash(self.digest, frame)?;
        self.count += 1;
        if self.count == expected.frames {
            if self.digest != expected.shape_sha256 {
                return Err(invalid(
                    Path::new(""),
                    "Final decoded frame properties differ from the sealed source chunk sequence.",
                ));
            }
            self.chunk += 1;
            self.count = 0;
            self.digest = [0; 32];
        }
        Ok(())
    }
    pub(in crate::jobs) fn finish(&self) -> Result<(), AppError> {
        if self.regions.reference_clock.as_ref().is_some_and(|clock| {
            self.clock_count != clock.frames || self.clock_digest != clock.relative_pts_sha256
        }) {
            return Err(invalid(
                Path::new(""),
                "The complete decoded source join clock differs from its independent reference.",
            ));
        }
        if self.chunk != self.regions.chunks.len() || self.count != 0 {
            return Err(invalid(
                Path::new(""),
                "The complete video does not cover every sealed source chunk.",
            ));
        }
        Ok(())
    }
}

fn shape_hash(previous: [u8; 32], frame: &Frame) -> Result<[u8; 32], AppError> {
    let bytes = serde_json::to_vec(&frame.source_queue_shape())
        .map_err(|cause| invalid(Path::new(""), cause.to_string()))?;
    let mut hash = Sha256::new();
    hash.update(previous);
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
    Ok(hash.finalize().into())
}

async fn file_digest(
    source: &Source,
    cancel: &watch::Receiver<bool>,
) -> Result<(u64, [u8; 32]), AppError> {
    source.verify()?;
    let path = source.path.clone();
    let cancel = cancel.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut file =
            std::fs::File::open(&path).map_err(|cause| invalid(&path, cause.to_string()))?;
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        let mut count = 0u64;
        loop {
            check_cancel(&cancel)?;
            let bytes = file
                .read(&mut buffer)
                .map_err(|cause| invalid(&path, cause.to_string()))?;
            if bytes == 0 {
                break;
            }
            hash.update(&buffer[..bytes]);
            count += bytes as u64;
        }
        Ok::<_, AppError>((count, hash.finalize().into()))
    })
    .await
    .map_err(|cause| invalid(&source.path, cause.to_string()))??;
    source.verify()?;
    Ok(result)
}

pub(in crate::jobs) async fn seal(
    ffmpeg: &Path,
    ffprobe: &Path,
    path: &Path,
    source_frames: u64,
    encoder: VideoEncoder,
    policy: CountPolicy,
    cancel: &watch::Receiver<bool>,
) -> Result<ChunkContract, AppError> {
    #[cfg(test)]
    super::av1an::tests::CHUNK_PROOF_CALLS.with(|calls| calls.set(calls.get() + 1));
    let source = Source::open(path)?;
    let (bytes, file_sha256) = file_digest(&source, cancel).await?;
    // Elementary H.264/HEVC chunks have no mux timestamps. Their immutable
    // source span and decoded count are separate from the final joined clock.
    let inspected = supervisor::run_capture(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args: [
                "-v",
                "error",
                "-show_streams",
                "-show_format",
                "-show_data_hash",
                "sha256",
                "-of",
                "json",
                "-i",
            ]
            .into_iter()
            .map(Into::into)
            .chain([path.as_os_str().to_owned()])
            .collect(),
            cwd: None,
        },
        cancel.clone(),
        512 * 1024,
        Duration::from_secs(60),
    )
    .await
    .map_err(|cause| process_error(cause, path))?;
    if !inspected.status.success() || !inspected.stderr.is_empty() {
        return Err(invalid(
            path,
            "The source chunk elementary header could not be inspected.",
        ));
    }
    let document: Document = serde_json::from_slice(&inspected.stdout)
        .map_err(|cause| invalid(path, cause.to_string()))?;
    let stream = document
        .streams
        .first()
        .ok_or_else(|| invalid(path, "The source chunk has no video header."))?;
    let header = Header::from_stream(stream);
    let codec = match encoder {
        VideoEncoder::SvtAv1
        | VideoEncoder::SvtAv1FiveFish
        | VideoEncoder::SvtAv1Hdr
        | VideoEncoder::AomAv1 => "av1",
        VideoEncoder::X264 => "h264",
        VideoEncoder::X265Standalone => "hevc",
        VideoEncoder::VpxStandalone => "vp9",
        _ => return Err(invalid(path, "This source queue encoder is not supported.")),
    };
    if stream.codec_type.as_deref() != Some("video") || stream.codec_name.as_deref() != Some(codec)
    {
        return Err(invalid(
            path,
            "The sealed chunk codec differs from its original queue encoder.",
        ));
    }
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args: encode::frame_scan_args(path, 0, 1),
            cwd: None,
        },
        cancel.clone(),
        65536,
        Duration::from_secs(24 * 60 * 60),
        move |reader| {
            let mut count = 0u64;
            let mut digest = [0; 32];
            let mut first = None;
            let mut error = None;
            frame_scan::parse(reader, |frame: Frame| {
                if first.is_none() {
                    first = Some(frame.source_queue_shape());
                }
                match shape_hash(digest, &frame) {
                    Ok(value) => digest = value,
                    Err(cause) => error = Some(cause.message),
                }
                count += 1;
            })?;
            if let Some(error) = error {
                return Err(error);
            }
            Ok((count, digest, first))
        },
    )
    .await
    .map_err(|cause| process_error(cause, path))?;
    if !result.status.success()
        || !result.stderr.is_empty()
        || result.value.0 == 0
        || (matches!(policy, CountPolicy::ExactSourceFrames) && result.value.0 != source_frames)
        || result
            .value
            .2
            .as_ref()
            .and_then(Frame::source_queue_dimensions)
            .is_none()
    {
        return Err(invalid(
            path,
            format!(
                "A source chunk failed complete frame/property proof: source{source_frames}, actual{}, {}",
                result.value.0,
                String::from_utf8_lossy(&result.stderr)
            ),
        ));
    }
    let decoded = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args: [
                "-v",
                "error",
                "-nostdin",
                "-xerror",
                "-err_detect",
                "explode",
                "-i",
            ]
            .into_iter()
            .map(Into::into)
            .chain([path.as_os_str().to_owned()])
            .chain(
                [
                    "-map",
                    "0:v:0",
                    "-fps_mode",
                    "passthrough",
                    "-f",
                    "null",
                    "-",
                ]
                .into_iter()
                .map(Into::into),
            )
            .collect(),
            cwd: None,
        },
        cancel.clone(),
        65536,
        Duration::from_secs(24 * 60 * 60),
    )
    .await
    .map_err(|cause| process_error(cause, path))?;
    if !decoded.status.success() || !decoded.stderr.is_empty() {
        return Err(invalid(
            path,
            format!(
                "A source chunk did not decode completely: {}",
                String::from_utf8_lossy(&decoded.stderr)
            ),
        ));
    }
    if file_digest(&source, cancel).await? != (bytes, file_sha256) {
        return Err(invalid(
            path,
            "A source chunk changed during decoded-contract sealing.",
        ));
    }
    Ok(ChunkContract {
        source_frames,
        frames: result.value.0,
        bytes,
        file_sha256,
        shape_sha256: result.value.1,
        first: result.value.2.expect("checked"),
        header,
    })
}
