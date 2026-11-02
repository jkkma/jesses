//! ADTS is a transport framing, not part of an AAC access unit. Canonical
//! Matroska stores ASC plus raw access units. Compare explicitly filtered packet
//! hashes and the complete decoded sample clock before accepting that change.
use crate::{
    jobs::{
        audio,
        metadata::{Document, Stream},
        mux, process_error,
    },
    supervisor::{self, CommandSpec},
};
use media_core::{AppError, AudioCodec};
use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    sync::mpsc,
    time::Duration,
};
use tokio::sync::watch;

pub(in crate::jobs) fn needed(document: &Document, stream: &Stream) -> bool {
    stream.codec_name.as_deref() == Some("aac")
        && document
            .format
            .as_ref()
            .and_then(|format| format.format_name.as_deref())
            .is_some_and(|formats| {
                formats
                    .split(',')
                    .any(|format| matches!(format, "mpegts" | "aac"))
            })
}
pub(in crate::jobs) fn filtered_args(
    source: &Path,
    index: u32,
    shift: f64,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-copyts",
        "-itsoffset",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    args.extend([
        format!("{shift:.12}").into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-i".into(),
        source.as_os_str().to_owned(),
        "-map".into(),
        format!("0:{index}").into(),
        "-c:a".into(),
        "copy".into(),
        "-bsf:a".into(),
        "aac_adtstoasc".into(),
        "-avoid_negative_ts".into(),
        "disabled".into(),
        "-f".into(),
        "framehash".into(),
        "-hash".into(),
        "sha256".into(),
        "pipe:1".into(),
    ]);
    args
}
pub(in crate::jobs) fn framehash(
    reader: &mut dyn Read,
    mut accept: impl FnMut(mux::Packet) -> Result<(), String>,
) -> Result<u64, String> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let mut base = None;
    let mut count = 0;
    let mut sha256 = false;
    loop {
        line.clear();
        let n = reader
            .by_ref()
            .take(8193)
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        if n == 0 {
            break;
        }
        if n > 8192 {
            return Err("AAC framehash row exceeds its bound.".into());
        }
        let line = std::str::from_utf8(&line)
            .map_err(|error| error.to_string())?
            .trim();
        if line == "#hash: SHA256" {
            sha256 = true;
            continue;
        }
        if let Some(value) = line.strip_prefix("#tb 0: ") {
            let (n, d) = value
                .split_once('/')
                .ok_or("AAC framehash has no rational time base.")?;
            let n = n
                .parse::<f64>()
                .map_err(|_| "Invalid framehash time base.")?;
            let d = d
                .parse::<f64>()
                .map_err(|_| "Invalid framehash time base.")?;
            if n <= 0.0 || d <= 0.0 {
                return Err("Invalid framehash time base.".into());
            }
            base = Some(n / d);
            continue;
        }
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let columns: Vec<_> = line.split(',').map(str::trim).take(6).collect();
        if !sha256 || columns.len() != 6 || columns[0] != "0" {
            return Err("Unexpected AAC framehash format.".into());
        }
        let base = base.ok_or("AAC framehash lacks its time base.")?;
        let time = |column: &str| -> Result<Option<f64>, String> {
            if column == "N/A" {
                Ok(None)
            } else {
                column
                    .parse::<i64>()
                    .map(|ticks| Some(ticks as f64 * base))
                    .map_err(|_| "Invalid AAC framehash timestamp.".into())
            }
        };
        let hash = columns[5];
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Invalid AAC framehash SHA256.".into());
        }
        accept(mux::Packet {
            dts: time(columns[1])?,
            pts: time(columns[2])?,
            duration: time(columns[3])?,
            size: columns[4]
                .parse()
                .map_err(|_| "Invalid AAC framehash size.")?,
            hash: format!("SHA256:{hash}"),
        })?;
        count += 1;
    }
    if count == 0 {
        return Err("AAC bitstream filter produced no packets.".into());
    }
    Ok(count)
}
fn same(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 0.002,
        (None, None) => true,
        _ => false,
    }
}
#[allow(clippy::too_many_arguments)]
pub(in crate::jobs) async fn verify(
    ffmpeg: &Path,
    ffprobe: &Path,
    source: &Path,
    input: &Stream,
    output: &Path,
    actual: &Stream,
    shift: f64,
    cancel: &watch::Receiver<bool>,
) -> Result<String, AppError> {
    let (sender, receiver) = mpsc::sync_channel(16);
    let reference = CommandSpec {
        executable: ffmpeg.to_owned(),
        args: filtered_args(source, input.index, shift),
        cwd: None,
    };
    let output_spec = mux::packet_spec(ffprobe, output, actual.index);
    let expected = supervisor::run_streaming_stdout(
        &reference,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            framehash(reader, |packet| {
                sender
                    .send(packet)
                    .map_err(|_| "AAC comparison stopped.".into())
            })
        },
    );
    let observed = supervisor::run_streaming_stdout(
        &output_spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            let count = mux::packets(reader, |packet| {
                let expected = receiver
                    .recv()
                    .map_err(|_| "Canonical AAC contains extra packets.")?;
                if expected.hash != packet.hash
                    || expected.size != packet.size
                    || !same(expected.pts, packet.pts)
                    || !same(expected.dts, packet.dts)
                    || !same(expected.duration, packet.duration)
                {
                    return Err("Canonical AAC differs from the explicitly converted access-unit payload or source clock.".into());
                }
                Ok(())
            })?;
            if receiver.recv().is_ok() {
                return Err("Canonical AAC omits source access units.".into());
            }
            Ok(count)
        },
    );
    let (expected, observed) = tokio::join!(Box::pin(expected), Box::pin(observed));
    let expected = expected.map_err(|error| process_error(error, source))?;
    let observed = observed.map_err(|error| process_error(error, output))?;
    if !expected.status.success()
        || !observed.status.success()
        || !expected.stderr.is_empty()
        || !observed.stderr.is_empty()
        || expected.value != observed.value
    {
        return Err(AppError::new(
            "SOURCE_NORMALIZATION_INVALID",
            "Canonical AAC failed its complete ADTS-to-ASC access-unit proof.",
            None,
        ));
    }
    let input_timeline = audio::scan(ffprobe, source, input, false, cancel)
        .await?
        .shifted(shift);
    let output_timeline =
        audio::scan_container_output(ffprobe, output, actual, &input_timeline, cancel).await?;
    let timing = audio::verify_timeline(&input_timeline, &output_timeline, AudioCodec::Copy)?;
    let pcm = |path: &Path, index: u32| {
        let mut args: Vec<std::ffi::OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-err_detect",
            "explode",
            "-protocol_whitelist",
            "file",
            "-i",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.extend([
            path.as_os_str().to_owned(),
            "-map".into(),
            format!("0:{index}").into(),
            "-c:a".into(),
            "pcm_f32le".into(),
            "-f".into(),
            "f32le".into(),
            "pipe:1".into(),
        ]);
        CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: None,
        }
    };
    let original = super::pixels(pcm(source, input.index), cancel, source).await?;
    let converted = super::pixels(pcm(output, actual.index), cancel, output).await?;
    if original != converted {
        return Err(AppError::new(
            "SOURCE_NORMALIZATION_INVALID",
            "Canonical AAC changed decoded sample bytes or count.",
            None,
        ));
    }
    Ok(format!(
        "Verified ADTS-to-ASC AAC: {} exact access-unit hashes, {} decoded PCM bytes, SHA256 {}. {timing}",
        expected.value, original.0, original.1
    ))
}
