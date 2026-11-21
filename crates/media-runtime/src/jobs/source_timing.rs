//! Source planning uses headers when possible and the actual producer or AV1AN
//! queue supplies the exact frame count. Workflows needing exact boundaries
//! inspect packet timestamps and the first decoded frames. Missing timestamps,
//! discarded packets, or ambiguous field coding require a decoded scan.

use super::encode_plan::{Frame, Plan, Validation};
use super::*;
/// Packets decoded for the first-frame checks. References precede dependents
/// in decode order, so a prefix decodes completely; a few packets also cover
/// leading pictures a decoder legitimately drops.
const FIRST_PACKETS: u32 = 16;

/// Why packets cannot stand in for decoded frames; the caller then decodes
/// the complete source before encoding, as before.
#[derive(Debug)]
pub(in crate::jobs) struct Fallback {
    reason: String,
    pub discarded: bool,
}

impl From<String> for Fallback {
    fn from(reason: String) -> Self {
        Self {
            reason,
            discarded: false,
        }
    }
}

impl From<&str> for Fallback {
    fn from(reason: &str) -> Self {
        reason.to_owned().into()
    }
}

impl std::fmt::Display for Fallback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reason.fmt(formatter)
    }
}

/// Plan ordinary encodes from headers and a bounded first-frame inspection.
/// The count is provisional until the producer runs or AV1AN writes its queue.
pub(in crate::jobs) async fn plan_from_headers(
    ffprobe: &Path,
    input: &Path,
    plan: &Plan,
    stream: &metadata::Stream,
    duration: Option<f64>,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<(Plan, usize)>, AppError> {
    if !headers_declare_constant_rate(stream) {
        return Ok(None);
    }
    let Some(count) = header_frame_count(stream, duration, (plan.fps_num, plan.fps_den)) else {
        return Ok(None);
    };
    let first = match first_frame(ffprobe, input, plan, stream.index, cancel).await? {
        Ok(first) => first,
        Err(_) => return Ok(None),
    };
    let mut validation = Validation::new(plan.clone(), stream.clone(), false);
    validation.push(&first);
    let (mut planned, _) = validation.finish()?;
    planned.source_frames_estimated = true;
    Ok(Some((planned, count)))
}

// Equal rates are enough to try the fast path. Some containers reuse their
// default frame duration for both rates even when individual timestamps vary.
fn headers_declare_constant_rate(stream: &metadata::Stream) -> bool {
    let rational = |value: Option<&str>| {
        let (num, den) = value?.split_once('/')?;
        let (num, den) = (num.parse::<u32>().ok()?, den.parse::<u32>().ok()?);
        (num > 0 && den > 0).then_some((u64::from(num), u64::from(den)))
    };
    matches!(
        (rational(stream.avg_frame_rate.as_deref()), rational(stream.r_frame_rate.as_deref())),
        (Some((an, ad)), Some((rn, rd))) if an * rd == rn * ad
    )
}

fn header_frame_count(
    stream: &metadata::Stream,
    duration: Option<f64>,
    rate: (u32, u32),
) -> Option<usize> {
    let parse_count = |value: &str| {
        value
            .parse::<usize>()
            .ok()
            .filter(|&count| count > 0 && count <= u32::MAX as usize)
    };
    let from_duration = metadata::track_duration(stream)
        .or(duration)
        .and_then(|seconds| {
            let frames = (seconds * f64::from(rate.0) / f64::from(rate.1)).round();
            (frames.is_finite() && frames >= 1.0 && frames <= f64::from(u32::MAX))
                .then_some(frames as usize)
        });
    stream
        .nb_frames
        .as_deref()
        .and_then(parse_count)
        .or_else(|| {
            stream
                .tags
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("NUMBER_OF_FRAMES"))
                .and_then(|(_, value)| parse_count(value))
                .filter(|&count| {
                    from_duration
                        .is_none_or(|expected| count.abs_diff(expected) <= (expected / 50).max(2))
                })
        })
        .or(from_duration)
}

/// Full timing inspection for explicitly requested verification or operations
/// that need exact source frame boundaries before encoding.
pub(in crate::jobs) async fn plan_from_packets(
    ffprobe: &Path,
    input: &Path,
    plan: &Plan,
    stream: &metadata::Stream,
    check_discard: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<Result<(Plan, usize), Fallback>, AppError> {
    // Field-coded video can store each field as its own packet, so packets
    // would not count frames. Such sources keep the blocking decoded scan.
    let may_be_field_coded = plan.accepts_field_coded_source()
        || matches!(
            stream.field_order.as_deref(),
            Some("tt" | "bb" | "tb" | "bt")
        );
    if may_be_field_coded && !check_discard {
        return Ok(Err("the source may be field-coded".into()));
    }
    let times = match packet_times(ffprobe, input, stream.index, cancel).await? {
        Ok(times) => times,
        Err(reason) => return Ok(Err(reason)),
    };
    if may_be_field_coded {
        return Ok(Err("the source may be field-coded".into()));
    }
    let first = match first_frame(ffprobe, input, plan, stream.index, cancel).await? {
        Ok(first) => first,
        Err(reason) => return Ok(Err(reason)),
    };
    let first_time = first
        .timestamp()
        .and_then(|value| value.parse::<f64>().ok());
    if !first_time.is_some_and(|time| (time - times[0]).abs() <= 0.000_001) {
        return Ok(Err(
            "the first decoded frame is not the first packet time".into()
        ));
    }
    let mut validation = Validation::timing(plan.clone(), stream.clone());
    validation.push(&first);
    for (position, time) in times[1..].iter().enumerate() {
        if position % 8192 == 0 {
            check_cancel(cancel)?;
        }
        validation.push(&Frame::timestamp_only(format!("{time:.6}")));
    }
    validation.finish().map(Ok)
}

/// Every packet's presentation time in presentation order, without decoding.
async fn packet_times(
    ffprobe: &Path,
    input: &Path,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<Result<Vec<f64>, Fallback>, AppError> {
    check_cancel(cancel)?;
    let args = packet_args(input, index);
    let output = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(60 * 60),
        |reader| Ok(parse_packets(std::io::BufReader::new(reader))),
    )
    .await
    .map_err(|error| process_error(error, input))?;
    check_cancel(cancel)?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Ok(Err("the packet listing reported errors".into()));
    }
    Ok(output.value)
}

fn packet_args(input: &Path, index: u32) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-select_streams",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(index.to_string().into());
    args.extend(
        [
            "-show_entries",
            "packet=pts_time,flags",
            "-of",
            "csv=p=0",
            "-i",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args.push(input.as_os_str().to_owned());
    args
}

/// Read the complete listing even after a fallback reason: the supervisor
/// rejects a parser that leaves stdout unread.
fn parse_packets(reader: impl std::io::BufRead) -> Result<Vec<f64>, Fallback> {
    let mut times = Vec::new();
    let mut fallback = None;
    let mut discarded = false;
    for (position, line) in reader.lines().enumerate() {
        let line = line.map_err(|cause| cause.to_string())?;
        let line = line.trim();
        discarded |= line
            .split_once(',')
            .is_some_and(|(_, flags)| flags.contains('D'));
        if line.is_empty() || fallback.is_some() {
            continue;
        }
        let packet = (|| {
            let (time, flags) = line
                .split_once(',')
                .ok_or_else(|| format!("packet {position} has an unreadable listing"))?;
            let time = time
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("packet {position} has no presentation time"))?;
            if flags.contains('D') || flags.contains('C') {
                return Err(format!("packet {position} is discarded or corrupt"));
            }
            if times.is_empty() && !flags.contains('K') {
                return Err("the first packet is not a keyframe".to_owned());
            }
            Ok(time)
        })();
        match packet {
            Ok(time) => times.push(time),
            Err(reason) => fallback = Some(reason),
        }
    }
    if let Some(reason) = fallback {
        return Err(Fallback { reason, discarded });
    }
    if times.is_empty() {
        return Err("the stream has no packets".into());
    }
    times.sort_by(f64::total_cmp);
    Ok(times)
}

/// The first decoded frame, with exactly the fields of the complete scan.
async fn first_frame(
    ffprobe: &Path,
    input: &Path,
    plan: &Plan,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<Result<Frame, Fallback>, AppError> {
    check_cancel(cancel)?;
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    let mut args = super::encode::source_scan_args(input, index, threads, plan);
    let show = args
        .iter()
        .position(|arg| arg == "-show_frames")
        .expect("frame scan arguments");
    args.splice(
        show..show,
        [
            "-read_intervals".into(),
            format!("%+#{FIRST_PACKETS}").into(),
        ],
    );
    let output = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(10 * 60),
        |reader| {
            let mut first = None;
            super::encode::frame_scan::parse(reader, |frame: Frame| {
                first.get_or_insert(frame);
            })?;
            Ok(first)
        },
    )
    .await
    .map_err(|error| process_error(error, input))?;
    check_cancel(cancel)?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Ok(Err(
            "the first frames could not be decoded without errors".into()
        ));
    }
    Ok(output
        .value
        .ok_or_else(|| "no frame decoded from the first packets".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_counts_use_video_metadata_and_reject_unusable_estimates() {
        let stream = |value| serde_json::from_value::<metadata::Stream>(value).unwrap();
        let video = stream(serde_json::json!({"index":0,"duration":"2.002"}));
        assert_eq!(
            header_frame_count(&video, Some(30.0), (24000, 1001)),
            Some(48)
        );
        let tagged = stream(serde_json::json!({"index":0,"tags":{"NUMBER_OF_FRAMES":"73"}}));
        assert_eq!(header_frame_count(&tagged, Some(2.0), (24, 1)), Some(48));
        assert_eq!(header_frame_count(&tagged, Some(3.0), (24, 1)), Some(73));
        let exact = stream(
            serde_json::json!({"index":0,"nb_frames":"48","tags":{"NUMBER_OF_FRAMES":"73"}}),
        );
        assert_eq!(header_frame_count(&exact, Some(2.0), (24, 1)), Some(48));
        let unknown = stream(serde_json::json!({"index":0,"nb_frames":"N/A"}));
        for duration in [None, Some(f64::NAN), Some(-1.0), Some(f64::INFINITY)] {
            assert_eq!(header_frame_count(&unknown, duration, (24, 1)), None);
        }
    }

    #[test]
    fn constant_rate_headers_require_two_equal_positive_rationals() {
        for (base, average, expected) in [
            (Some("24/1"), Some("48/2"), true),
            (Some("24/1"), Some("24000/1001"), false),
            (Some("0/1"), Some("0/1"), false),
            (Some("24/0"), Some("24/0"), false),
            (None, Some("24/1"), false),
            (Some("24/1"), None, false),
        ] {
            let video: metadata::Stream = serde_json::from_value(serde_json::json!({
                "index": 0, "r_frame_rate": base, "avg_frame_rate": average,
            }))
            .unwrap();
            assert_eq!(headers_declare_constant_rate(&video), expected);
        }
    }

    #[test]
    fn packets_give_presentation_order_and_refuse_what_decoding_would_change() {
        let listing = "0.000000,K__\n0.083417,___\n0.041708,___\n\n0.125125,___\n";
        assert_eq!(
            parse_packets(listing.as_bytes()).unwrap(),
            [0.0, 0.041708, 0.083417, 0.125125]
        );
        for (listing, reason) in [
            ("", "no packets"),
            ("0.041708,___\n0.000000,K__\n", "keyframe"),
            ("0.000000,K__\nN/A,___\n", "presentation time"),
            ("-0.041708,KD_\n0.000000,K__\n", "discarded"),
            ("0.000000,K__\n0.041708,_C_\n", "corrupt"),
            ("0.000000\n", "unreadable"),
        ] {
            let error = parse_packets(listing.as_bytes()).unwrap_err();
            assert!(error.reason.contains(reason), "{listing:?}: {error}");
            assert_eq!(error.discarded, listing.contains('D'));
        }
    }

    #[test]
    fn packet_listing_demuxes_one_stream_without_decoding() {
        let args = packet_args(Path::new("movie.mp4"), 2)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "-v",
                "error",
                "-protocol_whitelist",
                "file",
                "-select_streams",
                "2",
                "-show_entries",
                "packet=pts_time,flags",
                "-of",
                "csv=p=0",
                "-i",
                "movie.mp4"
            ]
        );
    }
}
