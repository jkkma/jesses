//! The planned MOV_TEXT cue timeline comes directly from the input packets.
//! Verify the actual staged publication, excluding empty clearing samples.
use super::{files, metadata::Stream, mux, probe_headers, process_error};
use crate::supervisor::{self, CommandSpec, SupervisorError};
use media_core::AppError;
use std::{ffi::OsString, io::Read, path::Path, sync::Arc, time::Duration};
use tokio::sync::watch;

const MICROS: i128 = 1_000_000;
const CLEAR: &str = "SHA256:96a296d224f285c67bee93c30f8a309157f0daa35dc5b87e410b78630a09cfc7";

#[derive(Clone, Copy)]
struct TimeBase {
    num: i128,
    den: i128,
}

#[derive(Clone, Copy)]
struct PictureTime {
    ticks: i64,
    base: TimeBase,
}

#[derive(Debug)]
pub(super) struct Cue {
    pub start_us: i64,
    pub end_us: i64,
    pub size: u64,
    hash: [u8; 32],
}

#[derive(Debug)]
pub(super) struct TrackProof {
    pub cues: Arc<Vec<Cue>>,
}

fn invalid(path: &Path, message: impl Into<String>) -> AppError {
    files::error("TRIM_VALIDATION_FAILED", message, path)
}

fn packet_error(error: SupervisorError, path: &Path) -> AppError {
    match error {
        SupervisorError::OutputParse(message) => invalid(path, message),
        error => process_error(error, path),
    }
}

fn round_ratio(numerator: i128, denominator: i128) -> i128 {
    let whole = numerator / denominator;
    let remainder = numerator % denominator;
    let half_rounded_up = denominator / 2 + denominator % 2;
    whole
        + if remainder.abs() >= half_rounded_up {
            remainder.signum()
        } else {
            0
        }
}

impl TimeBase {
    fn read(stream: &Stream) -> Option<Self> {
        let (num, den) = stream.time_base.as_deref()?.split_once('/')?;
        let (num, den) = (num.parse::<u32>().ok()?, den.parse::<u32>().ok()?);
        (num > 0 && den > 0).then_some(Self {
            num: i128::from(num),
            den: i128::from(den),
        })
    }

    fn micros(self, ticks: i64) -> Result<i64, String> {
        i64::try_from(round_ratio(i128::from(ticks) * self.num * MICROS, self.den))
            .map_err(|_| "A subtitle clock exceeds the supported timeline.".into())
    }

    fn ticks_after(self, origin: PictureTime, micros: i64) -> Result<i64, String> {
        let offset = i128::from(micros) * origin.base.den;
        let start = i128::from(origin.ticks) * origin.base.num * MICROS;
        let numerator = (start + offset)
            .checked_mul(self.den)
            .ok_or("A subtitle clock exceeds the supported timeline.")?;
        let denominator = origin.base.den * self.num * MICROS;
        i64::try_from(round_ratio(numerator, denominator))
            .map_err(|_| "A subtitle clock exceeds the supported timeline.".into())
    }
}

fn clear(size: u64, hash: &str) -> Result<bool, String> {
    if size > 2 {
        Ok(false)
    } else if size == 2 && hash.eq_ignore_ascii_case(CLEAR) {
        Ok(true)
    } else {
        Err("Invalid MOV_TEXT clearing sample.".into())
    }
}

fn end(pts: i64, duration: Option<i64>) -> Result<i64, String> {
    duration
        .filter(|duration| *duration > 0)
        .and_then(|duration| pts.checked_add(duration))
        .ok_or_else(|| "A visible subtitle cue has no valid positive duration.".into())
}

fn payload_hash(hash: &str) -> [u8; 32] {
    // The shared packet reader already validated the entire SHA-256 field.
    std::array::from_fn(|index| {
        let hex = |offset| {
            char::from(hash.as_bytes()[offset])
                .to_digit(16)
                .expect("validated hex digit") as u8
        };
        hex(7 + 2 * index) * 16 + hex(8 + 2 * index)
    })
}

fn read_source(
    reader: &mut dyn Read,
    base: TimeBase,
    start_us: i64,
    end_us: i64,
) -> Result<TrackProof, String> {
    let mut cues = Vec::new();
    mux::tick_packets(reader, |pts, duration, size, hash| {
        let source_start = base.micros(pts)?;
        if source_start >= end_us {
            return Ok(());
        }
        if clear(size, hash)? {
            return Ok(());
        }
        let cue_start = source_start.max(start_us);
        let cue_end = base.micros(end(pts, duration)?)?.min(end_us);
        if cue_end > cue_start {
            cues.try_reserve(1)
                .map_err(|_| "Could not reserve memory for the selected MOV_TEXT cues.")?;
            cues.push(Cue {
                start_us: cue_start - start_us,
                end_us: cue_end - start_us,
                size,
                hash: payload_hash(hash),
            });
        }
        Ok(())
    })?;
    Ok(TrackProof {
        cues: Arc::new(cues),
    })
}

impl TrackProof {
    pub async fn read(
        ffprobe: &Path,
        input: &Path,
        stream_index: u32,
        start_us: i64,
        end_us: i64,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Self, AppError> {
        if end_us
            .checked_sub(start_us)
            .is_none_or(|length| length <= 0)
        {
            return Err(invalid(input, "The subtitle trim interval is invalid."));
        }
        let headers = probe_headers(ffprobe, input, cancel, Some(&[stream_index])).await?;
        let base = headers
            .streams
            .iter()
            .find(|stream| stream.index == stream_index && is_mov_text(stream))
            .and_then(TimeBase::read)
            .ok_or_else(|| invalid(input, "The original MOV_TEXT track has no usable clock."))?;
        let mut spec = mux::tick_packet_spec(ffprobe, input, stream_index);
        if end_us > 0 {
            // Scan from the beginning so a cue already active at the cut is
            // always considered, but do not demux beyond the trim endpoint.
            spec.args.extend([
                "-read_intervals".into(),
                format!("%{}.{:06}", end_us / 1_000_000, end_us % 1_000_000).into(),
            ]);
        }
        let result = supervisor::run_streaming_stdout(
            &spec,
            cancel.clone(),
            8192,
            Duration::from_secs(24 * 3600),
            move |reader| read_source(reader, base, start_us, end_us),
        )
        .await
        .map_err(|error| packet_error(error, input))?;
        if !result.status.success() || !result.stderr.is_empty() {
            return Err(invalid(
                input,
                "Could not inspect the original MOV_TEXT cues.",
            ));
        }
        Ok(result.value)
    }
}

fn is_mov_text(stream: &Stream) -> bool {
    stream.codec_type.as_deref() == Some("subtitle")
        && stream.codec_name.as_deref() == Some("mov_text")
}

fn compare(
    reader: &mut dyn Read,
    base: TimeBase,
    origin: PictureTime,
    cues: &[Cue],
) -> Result<(), String> {
    let mut position = 0;
    let mut previous_end = None;
    let mut clear_end = None;
    mux::tick_packets(reader, |pts, duration, size, hash| {
        if previous_end.is_some_and(|end| pts < end) {
            return Err("MOV_TEXT samples overlap a visible cue.".into());
        }
        if clear(size, hash)? {
            clear_end = duration
                .filter(|duration| *duration > 0)
                .and_then(|duration| pts.checked_add(duration));
            return Ok(());
        }
        if clear_end.is_some_and(|end| pts < end) {
            return Err("A MOV_TEXT clearing sample overlaps a visible cue.".into());
        }
        let cue = cues
            .get(position)
            .ok_or("The trimmed MOV_TEXT track has extra visible cues.")?;
        let actual_end = end(pts, duration)?;
        let start = base.ticks_after(origin, cue.start_us)?;
        let end = base.ticks_after(origin, cue.end_us)?;
        if size != cue.size || payload_hash(hash) != cue.hash {
            return Err(format!(
                "Trimmed MOV_TEXT cue #{} changed its original payload or order.",
                position + 1
            ));
        }
        if pts != start || actual_end != end {
            return Err(format!(
                "Trimmed MOV_TEXT cue #{} changed its playback interval: expected {start}–{end} ticks, found {pts}–{actual_end} ticks.",
                position + 1,
            ));
        }
        previous_end = Some(actual_end);
        clear_end = None;
        position += 1;
        Ok(())
    })?;
    if position != cues.len() {
        return Err("The trimmed MOV_TEXT track is missing visible cues.".into());
    }
    Ok(())
}

async fn first_picture(
    ffprobe: &Path,
    path: &Path,
    stream: &Stream,
    cancel: &watch::Receiver<bool>,
) -> Result<PictureTime, AppError> {
    #[derive(serde::Deserialize)]
    struct Frame {
        best_effort_timestamp: Option<i64>,
    }
    #[derive(serde::Deserialize)]
    struct Frames {
        frames: Vec<Frame>,
    }
    let base = TimeBase::read(stream)
        .ok_or_else(|| invalid(path, "The encoded picture has no usable time base."))?;
    let index = stream.index.to_string();
    let args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-select_streams",
        &index,
        "-read_intervals",
        "%+#64",
        "-show_entries",
        "frame=best_effort_timestamp",
        "-of",
        "json",
        "-i",
    ]
    .into_iter()
    .map(Into::into)
    .chain([path.as_os_str().to_owned()])
    .collect();
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        256 * 1024,
        Duration::from_secs(120),
    )
    .await
    .map_err(|error| process_error(error, path))?;
    let read_start = || {
        let decoded = serde_json::from_slice::<Frames>(&result.stdout).ok()?;
        let ticks = decoded.frames.first()?.best_effort_timestamp?;
        Some(PictureTime { ticks, base })
    };
    if result.status.success()
        && result.stderr.is_empty()
        && let Some(origin) = read_start()
    {
        return Ok(origin);
    }
    Err(invalid(
        path,
        "Could not establish the first displayed picture's clock for subtitle verification.",
    ))
}

pub(super) async fn verify_output(
    ffprobe: &Path,
    path: &Path,
    video: &Stream,
    selected: &[&Stream],
    proofs: &[(usize, &TrackProof)],
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let origin = first_picture(ffprobe, path, video, cancel).await?;
    for &(position, proof) in proofs {
        let stream = selected
            .get(position)
            .copied()
            .filter(|stream| is_mov_text(stream))
            .ok_or_else(|| {
                invalid(
                    path,
                    "The planned MOV_TEXT subtitle track is missing from the finished file.",
                )
            })?;
        let base = TimeBase::read(stream)
            .ok_or_else(|| invalid(path, "The finished MOV_TEXT track has no usable clock."))?;
        let spec = mux::tick_packet_spec(ffprobe, path, stream.index);
        let cues = Arc::clone(&proof.cues);
        let result = supervisor::run_streaming_stdout(
            &spec,
            cancel.clone(),
            8192,
            Duration::from_secs(24 * 3600),
            move |reader| compare(reader, base, origin, &cues),
        )
        .await
        .map_err(|error| packet_error(error, path))?;
        if !result.status.success() || !result.stderr.is_empty() {
            return Err(invalid(
                path,
                "The finished MOV_TEXT packets could not be verified.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MICROSECONDS: TimeBase = TimeBase {
        num: 1,
        den: 1_000_000,
    };
    const FIRST: &str = "SHA256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const SECOND: &str = "SHA256:1023456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn record(pts: i64, duration: i64, size: u64, hash: &str) -> String {
        format!("pts={pts}|duration={duration}|size={size}|data_hash={hash}\n")
    }

    fn original() -> TrackProof {
        // Clip [103.436667,148.148). The initial clearing sample is outside
        // some of the window and is not retained as a visible cue.
        let source = [
            record(103_020_000, 1_459_000, 2, CLEAR),
            record(104_479_000, 3_420_000, 36, FIRST),
            record(147_001_000, 3_313_000, 32, SECOND),
            record(151_000_000, 1_000_000, 36, FIRST),
        ]
        .concat();
        read_source(
            &mut source.as_bytes(),
            MICROSECONDS,
            103_436_667,
            148_148_000,
        )
        .unwrap()
    }

    #[test]
    fn verifies_original_timing_payload_order_and_semantic_clear_samples() {
        let expected = original();
        assert_eq!(expected.cues.len(), 2);
        assert_eq!(expected.cues[1].end_us, 44_711_333);
        let origin = PictureTime {
            ticks: 0,
            base: MICROSECONDS,
        };
        let lead_in = record(0, 1_042_333, 2, CLEAR);
        let good = [
            lead_in.clone(),
            record(1_042_333, 3_420_000, 36, FIRST),
            record(4_462_333, 43_564_333 - 4_462_333, 2, CLEAR),
            record(43_564_333, 1_147_000, 32, SECOND),
        ]
        .concat();
        compare(&mut good.as_bytes(), MICROSECONDS, origin, &expected.cues).unwrap();

        // Shape of the broken real MP4: all visible cues kept the original
        // payload, but the mux moved them and reduced their duration to 1 us.
        let damaged = [
            record(0, 1, 2, CLEAR),
            record(1, 1, 36, FIRST),
            record(104_040_033, 1, 32, SECOND),
        ]
        .concat();
        let error = compare(
            &mut damaged.as_bytes(),
            MICROSECONDS,
            origin,
            &expected.cues,
        )
        .unwrap_err();
        assert!(
            error.contains("cue #1 changed its playback interval"),
            "{error}"
        );
        let shortened = [lead_in, record(1_042_333, 3_420_000, 36, FIRST)].concat();
        assert!(
            compare(
                &mut shortened.as_bytes(),
                MICROSECONDS,
                origin,
                &expected.cues
            )
            .unwrap_err()
            .contains("missing visible cues")
        );
        let cleared_early = format!(
            "{}{}",
            record(1_042_333, 3_420_000, 36, FIRST),
            record(1_900_000, 0, 2, CLEAR),
        );
        assert!(
            compare(
                &mut cleared_early.as_bytes(),
                MICROSECONDS,
                origin,
                &expected.cues
            )
            .unwrap_err()
            .contains("overlap")
        );
    }

    #[test]
    fn rounds_boundaries_once_at_the_actual_mixed_time_base() {
        let base = TimeBase { num: 1, den: 1_000 };
        let origin = PictureTime {
            ticks: 1,
            base: TimeBase {
                num: 1,
                den: 24_000,
            },
        };
        // 0.0416667 ms picture origin + 1.458 ms is 1.4996667 ms.
        // Rounding the picture origin to 42 us first would yield 2 ticks.
        assert_eq!(base.ticks_after(origin, 1_458).unwrap(), 1);
        assert_eq!(base.ticks_after(origin, 1_459).unwrap(), 2);
        assert_eq!(base.ticks_after(origin, -542).unwrap(), -1);
    }
}
