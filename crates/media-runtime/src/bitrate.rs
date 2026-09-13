//! Bounded packet-payload bitrate inspection, with no decoded media or writes.
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{BufRead, BufReader, Read},
    path::Path,
    time::Duration,
};

use media_core::{AppError, BitratePoint, BitrateRequest, BitrateResult};
use tokio::sync::watch;

use crate::{
    analysis::{check_cancel, fingerprint, permit, tool},
    jobs::files::Source,
    supervisor::{CommandSpec, SupervisorError, run_streaming_stdout},
};

const MAX_POINTS: i64 = 7200;
const MAX_PACKETS: u64 = 50_000_000;
const MAX_RECORD: usize = 8192;

fn add(total: &mut u64, value: u64) -> Result<(), String> {
    *total = total
        .checked_add(value)
        .ok_or("Packet totals overflowed.")?;
    Ok(())
}

fn seconds(value: &str) -> Result<Option<f64>, String> {
    if value == "N/A" {
        return Ok(None);
    }
    let value: f64 = value.parse().map_err(|_| "Invalid packet timestamp.")?;
    if !value.is_finite() || value.abs() > 1_000_000_000.0 {
        return Err("Packet timestamp is outside the supported range.".into());
    }
    Ok(Some(value))
}

fn parse_packets(
    reader: &mut dyn Read,
    stream_index: u32,
    window: f64,
) -> Result<BitrateResult, String> {
    let mut reader = BufReader::with_capacity(MAX_RECORD, reader);
    let mut record = Vec::with_capacity(MAX_RECORD);
    let mut bins = BTreeMap::<i64, u64>::new();
    let mut total = 0;
    let mut packets = 0;
    let mut untimed = 0;
    let mut untimed_packets = 0;
    let mut fallback = 0;
    let mut start: Option<f64> = None;
    let mut end: Option<f64> = None;
    let mut latest_timestamp: Option<f64> = None;
    let mut latest_has_duration = false;
    loop {
        record.clear();
        // `read_until` alone can grow without bound on a corrupt tool record.
        let count = (&mut reader)
            .take((MAX_RECORD + 1) as u64)
            .read_until(b'\n', &mut record)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        if record.len() > MAX_RECORD {
            return Err("Packet record exceeded its size limit.".into());
        }
        let line = std::str::from_utf8(&record)
            .map_err(|_| "Packet record is not UTF-8.")?
            .trim();
        if line.is_empty() {
            continue;
        }
        let mut index = None;
        let mut pts = None;
        let mut dts = None;
        let mut duration = None;
        let mut size = None;
        let mut seen = 0_u8;
        for field in line.split('|').filter(|field| !field.is_empty()) {
            let (key, value) = field.split_once('=').ok_or("Malformed packet record.")?;
            let bit = match key {
                "stream_index" => 1,
                "pts_time" => 2,
                "dts_time" => 4,
                "duration_time" => 8,
                "size" => 16,
                _ => return Err("Unexpected field in packet record.".into()),
            };
            if seen & bit != 0 {
                return Err("Duplicate field in packet record.".into());
            }
            seen |= bit;
            match key {
                "stream_index" => {
                    index = Some(value.parse::<u32>().map_err(|_| "Invalid stream index.")?)
                }
                "pts_time" => pts = seconds(value)?,
                "dts_time" => dts = seconds(value)?,
                "duration_time" => {
                    duration = seconds(value)?;
                    if duration.is_some_and(|value| value < 0.0) {
                        return Err("Negative packet duration.".into());
                    }
                }
                "size" => size = Some(value.parse::<u64>().map_err(|_| "Invalid packet size.")?),
                _ => unreachable!(),
            }
        }
        if index != Some(stream_index) {
            return Err("The packet scan returned a different stream.".into());
        }
        let size = size.ok_or("Packet size is missing.")?;
        add(&mut total, size)?;
        packets += 1;
        if packets > MAX_PACKETS {
            return Err("The packet scan exceeded its packet limit.".into());
        }
        let Some(timestamp) = pts.or(dts) else {
            add(&mut untimed, size)?;
            untimed_packets += 1;
            continue;
        };
        if pts.is_none() {
            fallback += 1;
        }
        let bucket = (timestamp / window).floor() as i64;
        let first = bins
            .first_key_value()
            .map_or(bucket, |(&key, _)| key.min(bucket));
        let last = bins
            .last_key_value()
            .map_or(bucket, |(&key, _)| key.max(bucket));
        if last - first >= MAX_POINTS {
            return Err(
                "The timeline needs more than 7,200 windows. Choose a larger window.".into(),
            );
        }
        add(bins.entry(bucket).or_default(), size)?;
        start = Some(start.map_or(timestamp, |value| value.min(timestamp)));
        let packet_end = timestamp + duration.unwrap_or(0.0);
        end = Some(end.map_or(packet_end, |value| value.max(packet_end)));
        if latest_timestamp.is_none_or(|value| timestamp > value) {
            latest_timestamp = Some(timestamp);
            latest_has_duration = duration.is_some_and(|value| value > 0.0);
        } else if latest_timestamp == Some(timestamp) {
            latest_has_duration |= duration.is_some_and(|value| value > 0.0);
        }
    }
    if packets == 0 {
        return Err("No packets were found for this stream.".into());
    }
    let points: Vec<_> = match (bins.first_key_value(), bins.last_key_value()) {
        (Some((&first, _)), Some((&last, _))) => (first..=last)
            .map(|bucket| {
                let bytes = bins.get(&bucket).copied().unwrap_or(0);
                BitratePoint {
                    start_seconds: bucket as f64 * window,
                    megabits_per_second: bytes as f64 * 8.0 / window / 1_000_000.0,
                    packet_bytes: bytes.to_string(),
                }
            })
            .collect(),
        _ => vec![],
    };
    let peak = points
        .iter()
        .map(|point| point.megabits_per_second)
        .fold(0.0_f64, f64::max);
    let average = start
        .zip(end)
        .filter(|(start, end)| latest_has_duration && end > start)
        .map(|(start, end)| (total - untimed) as f64 * 8.0 / (end - start) / 1_000_000.0);
    Ok(BitrateResult {
        stream_index,
        window_seconds: window,
        packet_bytes: total.to_string(),
        packet_count: packets.to_string(),
        untimed_packet_bytes: untimed.to_string(),
        untimed_packet_count: untimed_packets.to_string(),
        dts_fallback_count: fallback.to_string(),
        start_seconds: start,
        end_seconds: end.filter(|_| latest_has_duration),
        average_megabits_per_second: average,
        peak_window_megabits_per_second: peak,
        points,
        source_fingerprint: String::new(),
    })
}

pub async fn analyze_bitrate(
    request: BitrateRequest,
    cancel: watch::Receiver<bool>,
) -> Result<BitrateResult, AppError> {
    let fail = |code: &str, message: String| {
        AppError::new(code, message, Some(request.input_path.clone()))
    };
    if !request.window_seconds.is_finite() || !(0.1..=3600.0).contains(&request.window_seconds) {
        return Err(fail(
            "INVALID_ANALYSIS",
            "Choose a bitrate window from 0.1 to 3,600 seconds.".into(),
        ));
    }
    let _permit = permit(&cancel).await?;
    let source = Source::open(Path::new(&request.input_path))?;
    let identity = fingerprint(&source)?;
    let executable = tool("ffprobe", &cancel).await?;
    let args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file,pipe",
        "-select_streams",
        &request.stream_index.to_string(),
        "-show_packets",
        "-show_entries",
        "packet=stream_index,pts_time,dts_time,duration_time,size:packet_side_data=",
        "-of",
        "compact=p=0:nk=0",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .chain(std::iter::once(source.path.as_os_str().to_owned()))
    .collect();
    let scan = run_streaming_stdout(
        &CommandSpec {
            executable,
            args,
            cwd: None,
        },
        cancel.clone(),
        8192,
        Duration::from_secs(300),
        move |reader| parse_packets(reader, request.stream_index, request.window_seconds),
    )
    .await
    .map_err(|error| match error {
        SupervisorError::Cancelled => fail(
            "ANALYSIS_CANCELLED",
            "Bitrate analysis was canceled.".into(),
        ),
        SupervisorError::Timeout => fail(
            "ANALYSIS_TIMEOUT",
            "Bitrate analysis exceeded its five-minute limit.".into(),
        ),
        error => fail("ANALYSIS_FAILED", error.to_string()),
    })?;
    source.verify()?;
    if fingerprint(&source)? != identity {
        return Err(fail(
            "SOURCE_CHANGED",
            "The source changed during bitrate analysis. Import it again.".into(),
        ));
    }
    check_cancel(&cancel)?;
    if !scan.status.success() {
        return Err(fail(
            "ANALYSIS_FAILED",
            format!(
                "FFprobe failed: {}",
                String::from_utf8_lossy(&scan.stderr).trim()
            ),
        ));
    }
    let mut result = scan.value;
    result.source_fingerprint = identity;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(input: &str) -> Result<BitrateResult, String> {
        parse_packets(&mut input.as_bytes(), 2, 1.0)
    }

    #[test]
    fn reordered_negative_pts_gaps_and_untimed_payload_are_accounted_separately() {
        let result = scan("stream_index=2|pts_time=1.5|duration_time=0.5|size=200\nstream_index=2|pts_time=-0.5|duration_time=0.5|size=100\nstream_index=2|pts_time=N/A|dts_time=1.0|duration_time=0.5|size=300\nstream_index=2|size=50\n").unwrap();
        assert_eq!(result.packet_bytes, "650");
        assert_eq!(result.untimed_packet_bytes, "50");
        assert_eq!(result.dts_fallback_count, "1");
        assert_eq!(
            result
                .points
                .iter()
                .map(|point| point.packet_bytes.as_str())
                .collect::<Vec<_>>(),
            ["100", "0", "500"]
        );
        assert_eq!(
            result.average_megabits_per_second,
            Some(600.0 * 8.0 / 2.5 / 1_000_000.0)
        );
        assert_eq!(result.start_seconds, Some(-0.5));
        assert_eq!(result.end_seconds, Some(2.0));
    }

    #[test]
    fn missing_terminal_duration_does_not_invent_an_average() {
        let result = scan("stream_index=2|pts_time=0|duration_time=0.5|size=100\nstream_index=2|pts_time=1|size=100\n").unwrap();
        assert!(result.average_megabits_per_second.is_none());
        assert!(result.end_seconds.is_none());
        assert_eq!(result.points.len(), 2);
    }

    #[test]
    fn untrusted_packet_output_is_bounded_and_strict() {
        for invalid in [
            "stream_index=1|size=10",
            "stream_index=2|size=1|size=2",
            "stream_index=2|pts_time=NaN|size=10",
            "stream_index=2|pts_time=inf|size=10",
            "stream_index=2|pts_time=0|duration_time=-1|size=10",
            "stream_index=2|pts_time=0|size=18446744073709551615\nstream_index=2|size=1",
            "stream_index=2|pts_time=0|size=1\nstream_index=2|pts_time=7200|size=1",
            "stream_index=2|pts_time=0",
            "",
            "unrecognized=field",
        ] {
            assert!(scan(invalid).is_err(), "{invalid}");
        }
        assert!(scan(&"x".repeat(MAX_RECORD + 1)).is_err());
        assert!(
            scan("stream_index=2|pts_time=7199|size=1\nstream_index=2|pts_time=0|size=1").is_ok()
        );
    }
}
