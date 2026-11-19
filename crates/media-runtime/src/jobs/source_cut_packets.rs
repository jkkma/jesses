//! Complete original/copy access-unit and metadata proof for source AV1AN cuts.
use super::*;
use crate::jobs::{metadata, mux};
use std::io::{BufRead, BufReader};
use std::sync::mpsc;

/// SHA-256 of FFmpeg's 2-byte empty `mov_text` leading gap-filler sample (`[0x00, 0x00]`).
const EMPTY_MOV_TEXT_SHA256: &str =
    "SHA256:96a296d224f285c67bee93c30f8a309157f0daa35dc5b87e410b78630a09cfc7";

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct TrackProof {
    pub source_index: u32,
    pub cut_index: u32,
    pub kind: String,
    pub codec: String,
    pub packets: u64,
    pub first: Option<f64>,
    pub end: Option<f64>,
}

fn is_matroska_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("mkv")
                || ext.eq_ignore_ascii_case("webm")
                || ext.eq_ignore_ascii_case("mka")
                || ext.eq_ignore_ascii_case("mks")
        })
}

fn is_iso_bmff_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("mp4")
                || ext.eq_ignore_ascii_case("m4v")
                || ext.eq_ignore_ascii_case("m4a")
                || ext.eq_ignore_ascii_case("mov")
        })
}

fn packet_spec_bounded(
    ffprobe: &Path,
    path: &Path,
    index: u32,
    read_interval: Option<&str>,
) -> CommandSpec {
    let mut spec = mux::packet_spec(ffprobe, path, index);
    if let Some(interval) = read_interval {
        // Insert `-read_intervals <interval>` immediately before `-i`.
        let insert_at = spec.args.len().saturating_sub(2);
        spec.args.splice(
            insert_at..insert_at,
            ["-read_intervals".into(), interval.into()],
        );
    }
    spec
}

/// AV1 streams remuxed by FFmpeg into MP4/Matroska strip redundant 2-byte
/// Temporal Delimiter OBUs (`0x12 0x00`) per container spec. Hash packets after
/// `av1_metadata=td=remove` so original and stream-copied AV1 frames compare
/// cryptographically bit-for-bit.
fn av1_packet_spec(
    ffmpeg: &Path,
    path: &Path,
    index: u32,
    window: Option<(f64, f64)>,
) -> CommandSpec {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-copyts",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    if let Some((start, _)) = window
        && start > 0.0
    {
        args.extend(["-ss".into(), format!("{start:.3}").into()]);
    }
    args.extend([
        "-i".into(),
        path.as_os_str().to_owned(),
        "-map".into(),
        format!("0:{index}").into(),
        "-c:0".into(),
        "copy".into(),
        "-bsf:0".into(),
        "av1_metadata=td=remove".into(),
    ]);
    if let Some((start, end)) = window {
        let duration = (end - start).max(0.1);
        args.extend(["-t".into(), format!("{duration:.3}").into()]);
    }
    args.extend([
        "-f".into(),
        "framehash".into(),
        "-hash".into(),
        "sha256".into(),
        "-".into(),
    ]);
    CommandSpec {
        executable: ffmpeg.to_owned(),
        args,
        cwd: None,
    }
}

fn parse_framehash_packets(
    reader: &mut dyn std::io::Read,
    mut accept: impl FnMut(mux::Packet) -> Result<(), String>,
) -> Result<u64, String> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let mut tb: Option<(f64, f64)> = None;
    let mut count = 0_u64;
    loop {
        line.clear();
        let len = reader
            .by_ref()
            .take(8193)
            .read_until(b'\n', &mut line)
            .map_err(|cause| cause.to_string())?;
        if len == 0 {
            return Ok(count);
        }
        if len > 8192 {
            return Err("A packet hash record exceeds its bounded size.".into());
        }
        let text = std::str::from_utf8(&line)
            .map_err(|cause| cause.to_string())?
            .trim();
        if text.is_empty() {
            continue;
        }
        if let Some(rest) = text.strip_prefix("#tb 0:") {
            let (num, den) = rest
                .trim()
                .split_once('/')
                .ok_or_else(|| "Invalid framehash timebase.".to_owned())?;
            let num: f64 = num
                .trim()
                .parse()
                .map_err(|_| "Invalid framehash timebase numerator.".to_owned())?;
            let den: f64 = den
                .trim()
                .parse()
                .map_err(|_| "Invalid framehash timebase denominator.".to_owned())?;
            if !num.is_finite() || !den.is_finite() || den <= 0.0 {
                return Err("Non-positive framehash timebase.".into());
            }
            tb = Some((num, den));
            continue;
        }
        if text.starts_with('#') {
            continue;
        }
        let (num, den) =
            tb.ok_or_else(|| "Framehash packet appeared before timebase.".to_owned())?;
        let mut parts = text.split(',').map(str::trim);
        let _stream = parts.next();
        let dts_str = parts
            .next()
            .ok_or_else(|| "Missing framehash DTS.".to_owned())?;
        let pts_str = parts
            .next()
            .ok_or_else(|| "Missing framehash PTS.".to_owned())?;
        let dur_str = parts
            .next()
            .ok_or_else(|| "Missing framehash duration.".to_owned())?;
        let size_str = parts
            .next()
            .ok_or_else(|| "Missing framehash size.".to_owned())?;
        let hash_hex = parts
            .next()
            .ok_or_else(|| "Missing framehash digest.".to_owned())?;
        let parse_ts = |value: &str| -> Result<Option<f64>, String> {
            if value == "NOPTS" {
                return Ok(None);
            }
            let ticks: i64 = value
                .parse()
                .map_err(|_| "Invalid framehash timestamp.".to_owned())?;
            Ok(Some(ticks as f64 * num / den))
        };
        let size: u64 = size_str
            .parse()
            .map_err(|_| "Invalid framehash packet size.".to_owned())?;
        let packet = mux::Packet {
            pts: parse_ts(pts_str)?,
            dts: parse_ts(dts_str)?,
            duration: parse_ts(dur_str)?,
            size,
            hash: format!("SHA256:{hash_hex}"),
        };
        accept(packet)?;
        count += 1;
    }
}

fn stream_packets(
    reader: &mut dyn std::io::Read,
    av1: bool,
    accept: impl FnMut(mux::Packet) -> Result<(), String>,
) -> Result<u64, String> {
    if av1 {
        parse_framehash_packets(reader, accept)
    } else {
        mux::packets(reader, accept)
    }
}

async fn first_packet(
    ffmpeg: &Path,
    ffprobe: &Path,
    path: &Path,
    index: u32,
    av1: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<mux::Packet, AppError> {
    let spec = if av1 {
        av1_packet_spec(ffmpeg, path, index, None)
    } else {
        mux::packet_spec(ffprobe, path, index)
    };
    let result =
        supervisor::run_streaming_stdout(&spec, cancel.clone(), 8192, TIME, move |reader| {
            let mut first = None;
            stream_packets(reader, av1, |packet| {
                if first.is_none() {
                    first = Some(packet);
                }
                Ok(())
            })?;
            first.ok_or_else(|| "The copied selected video has no packets.".to_owned())
        })
        .await
        .map_err(|error| process_error(error, path))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(invalid(
            path,
            "The copied selected video packet clock is unreadable.",
        ));
    }
    Ok(result.value)
}

#[allow(clippy::too_many_arguments)]
async fn original_first(
    ffmpeg: &Path,
    ffprobe: &Path,
    path: &Path,
    index: u32,
    av1: bool,
    first: mux::Packet,
    bound: f64,
    cancel: &watch::Receiver<bool>,
) -> Result<f64, AppError> {
    let read_end = (bound + 5.0).max(5.0);
    for bounded in [true, false] {
        let spec = if av1 {
            av1_packet_spec(ffmpeg, path, index, bounded.then_some((0.0, read_end)))
        } else {
            let interval = bounded.then(|| format!("%{read_end:.3}"));
            packet_spec_bounded(ffprobe, path, index, interval.as_deref())
        };
        let target_hash = first.hash.clone();
        let target_size = first.size;
        let result =
            supervisor::run_streaming_stdout(&spec, cancel.clone(), 8192, TIME, move |reader| {
                let mut time = None;
                stream_packets(reader, av1, |packet| {
                    if packet.hash == target_hash
                        && packet.size == target_size
                        && packet.pts.is_some_and(|pts| pts <= bound + 0.002)
                    {
                        time = packet.pts;
                    }
                    Ok(())
                })?;
                Ok(time)
            })
            .await
            .map_err(|error| process_error(error, path))?;
        if !result.status.success() || !result.stderr.is_empty() {
            return Err(invalid(
                path,
                "The original keyframe packet could not be proven.",
            ));
        }
        if let Some(found) = result.value {
            return Ok(found);
        }
    }
    Err(invalid(
        path,
        "The copied first video packet has no matching keyframe at/before the requested source seek.",
    ))
}

fn same(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 0.002,
        (None, None) => true,
        _ => false,
    }
}

fn is_empty_mov_text_filler(packet: &mux::Packet, shift: f64) -> bool {
    shift > 0.002
        && packet.size == 2
        && packet.hash == EMPTY_MOV_TEXT_SHA256
        && packet.pts.is_some_and(|pts| pts.abs() <= 0.002)
        && packet
            .duration
            .is_some_and(|duration| (duration - shift).abs() <= 0.005)
}

#[allow(clippy::too_many_arguments)]
async fn sequence_pass(
    ffmpeg: &Path,
    ffprobe: &Path,
    source: &Path,
    source_index: u32,
    cut: &Path,
    cut_index: u32,
    shift: f64,
    subtitle: bool,
    mov_text: bool,
    av1: bool,
    matroska: bool,
    window: Option<(f64, f64)>,
    cancel: &watch::Receiver<bool>,
) -> Result<(u64, Option<f64>, Option<f64>), AppError> {
    let (sender, receiver) = mpsc::sync_channel(16);
    let cut_spec = if av1 {
        av1_packet_spec(ffmpeg, cut, cut_index, None)
    } else {
        mux::packet_spec(ffprobe, cut, cut_index)
    };
    let source_spec = if av1 {
        av1_packet_spec(ffmpeg, source, source_index, window)
    } else {
        let interval = window.map(|(start, end)| {
            if start > 0.0 {
                format!("{start:.3}%{end:.3}")
            } else {
                format!("%{end:.3}")
            }
        });
        packet_spec_bounded(ffprobe, source, source_index, interval.as_deref())
    };
    let actual =
        supervisor::run_streaming_stdout(&cut_spec, cancel.clone(), 8192, TIME, move |reader| {
            stream_packets(reader, av1, |packet| {
                sender
                    .send(packet)
                    .map_err(|_| "Source-cut comparison stopped.".into())
            })
        });
    let expected = supervisor::run_streaming_stdout(
        &source_spec,
        cancel.clone(),
        8192,
        TIME,
        move |reader| {
            let mut next = receiver.recv().ok();
            let mut filler_count = 0_u64;
            let mut filler_first = None;
            let mut filler_end = None;
            if mov_text
                && let Some(first_actual) = next.as_ref()
                && is_empty_mov_text_filler(first_actual, shift)
            {
                filler_count = 1;
                filler_first = first_actual.pts;
                filler_end = first_actual
                    .pts
                    .zip(first_actual.duration)
                    .map(|(pts, duration)| pts + duration);
                next = receiver.recv().ok();
            }
            let mut started = false;
            let mut seen_actual_dts = false;
            let mut count = filler_count;
            let mut first = filler_first;
            let mut end = filler_end;
            stream_packets(reader, av1, |packet| {
                let Some(actual) = next.as_ref() else {
                    return Ok(());
                };
                let shifted = |time: Option<f64>| {
                    time.map(|time| {
                        if subtitle {
                            (time + shift).max(0.0)
                        } else {
                            time + shift
                        }
                    })
                };
                // Matroska does not store DTS; FFmpeg leaves leading B-pyramid
                // packets without a DTS at the start of a cut while synthesizing
                // DTS mid-file in the original stream. Require exact payload,
                // size, duration, and PTS on every packet, and exact DTS once
                // present in the cut.
                let dts_matches = same(shifted(packet.dts), actual.dts)
                    || (matroska && !seen_actual_dts && actual.dts.is_none());
                let matches = packet.hash == actual.hash
                    && packet.size == actual.size
                    && same(packet.duration, actual.duration)
                    && same(shifted(packet.pts), actual.pts)
                    && dts_matches;
                if !started {
                    if !matches {
                        return Ok(());
                    }
                    started = true;
                    if first.is_none() {
                        first = shifted(packet.pts);
                    }
                }
                if !matches {
                    return Err("A source-copy packet changed payload/order/duration or its common keyframe clock.".into());
                }
                if actual.dts.is_some() {
                    seen_actual_dts = true;
                }
                if let Some(time) = shifted(packet.pts)
                    .zip(packet.duration)
                    .map(|(pts, duration)| pts + duration)
                {
                    end = Some(end.map_or(time, |old: f64| old.max(time)));
                }
                count += 1;
                next = receiver.recv().ok();
                Ok(())
            })?;
            if next.is_some() {
                return Err("A copied packet has no contiguous original source sequence.".into());
            }
            Ok((count, first, end))
        },
    );
    let (actual, expected) = tokio::join!(Box::pin(actual), Box::pin(expected));
    // Check `expected` before `actual` so the true comparison mismatch is never
    // masked by the channel-closed error on `actual`.
    let expected = expected.map_err(|error| process_error(error, source))?;
    let actual = actual.map_err(|error| process_error(error, cut))?;
    if !actual.status.success()
        || !expected.status.success()
        || !actual.stderr.is_empty()
        || !expected.stderr.is_empty()
        || actual.value != expected.value.0
    {
        return Err(invalid(
            cut,
            "The complete copied source packet sequence failed.",
        ));
    }
    Ok(expected.value)
}

#[allow(clippy::too_many_arguments)]
async fn sequence(
    ffmpeg: &Path,
    ffprobe: &Path,
    source: &Path,
    source_index: u32,
    cut: &Path,
    cut_index: u32,
    shift: f64,
    subtitle: bool,
    mov_text: bool,
    av1: bool,
    matroska: bool,
    original_first: f64,
    end: f64,
    cancel: &watch::Receiver<bool>,
) -> Result<(u64, Option<f64>, Option<f64>), AppError> {
    let read_end = end.max(original_first) + 15.0;
    let seek_start = if subtitle {
        0.0
    } else {
        let s = (original_first - 10.0).max(0.0);
        if s < 5.0 { 0.0 } else { s }
    };
    let mut windows = vec![Some((seek_start, read_end))];
    if seek_start > 0.0 {
        windows.push(Some((0.0, read_end)));
    }
    windows.push(None);
    let mut last_error = None;
    for window in windows {
        match sequence_pass(
            ffmpeg,
            ffprobe,
            source,
            source_index,
            cut,
            cut_index,
            shift,
            subtitle,
            mov_text,
            av1,
            matroska,
            window,
            cancel,
        )
        .await
        {
            Ok(value) => return Ok(value),
            Err(error) if error.code == "JOB_CANCELED" => return Err(error),
            Err(error) => {
                // If a real packet mismatch occurred after matching the first
                // packet, retrying with a wider window won't fix it.
                if error.message.contains("changed payload/order/duration") {
                    return Err(error);
                }
                last_error = Some(error);
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| invalid(cut, "The complete copied source packet sequence failed.")))
}
fn map_streams<'a>(
    document: &'a Document,
    actual: &'a Document,
    cut: &Path,
) -> Result<Vec<(&'a metadata::Stream, &'a metadata::Stream)>, AppError> {
    // FFprobe reports font attachments without a codec name. FFmpeg still
    // copies their bytes, which metadata::verify binds by extradata hash/tags.
    let copied = |stream: &metadata::Stream| {
        stream.codec_name.is_some() || stream.codec_type.as_deref() == Some("attachment")
    };
    let mut mapping = Vec::new();
    let mut cursor = 0;
    for stream in &actual.streams {
        while document
            .streams
            .get(cursor)
            .is_some_and(|original| !copied(original))
        {
            cursor += 1;
        }
        let original = document
            .streams
            .get(cursor)
            .ok_or_else(|| invalid(cut, "The cut added a source stream."))?;
        if original.codec_type != stream.codec_type || original.codec_name != stream.codec_name {
            return Err(invalid(
                cut,
                "The copied source stream order or codecs changed.",
            ));
        }
        mapping.push((original, stream));
        cursor += 1;
    }
    if document.streams[cursor..].iter().any(copied) {
        return Err(invalid(cut, "The cut omitted a known source stream."));
    }
    Ok(mapping)
}

fn matches_dolby_vision_configuration(mapping: &[(&metadata::Stream, &metadata::Stream)]) -> bool {
    mapping.iter().all(|(original, copied)| {
        original
            .dolby_vision_configuration()
            .eq(copied.dolby_vision_configuration())
    })
}

pub(super) fn copied_dolby_vision_configuration_matches(
    original: &Document,
    copied: &Document,
    cut: &Path,
) -> Result<bool, AppError> {
    Ok(matches_dolby_vision_configuration(&map_streams(
        original, copied, cut,
    )?))
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify(
    ffmpeg: &Path,
    ffprobe: &Path,
    source: &Path,
    document: &Document,
    cut: &Path,
    actual: &Document,
    video_index: u32,
    start: f64,
    end: f64,
    cancel: &watch::Receiver<bool>,
) -> Result<(Vec<TrackProof>, f64), AppError> {
    let mapping = map_streams(document, actual, cut)?;
    if !matches_dolby_vision_configuration(&mapping) {
        return Err(invalid(
            cut,
            "The copied stream's Dolby Vision configuration changed.",
        ));
    }
    let (original_video, cut_video) = mapping
        .iter()
        .find(|(original, _)| original.index == video_index)
        .ok_or_else(|| invalid(cut, "The selected video did not survive the source copy."))?;
    let video_av1 = original_video.codec_name.as_deref() == Some("av1");
    let first = first_packet(ffmpeg, ffprobe, cut, cut_video.index, video_av1, cancel).await?;
    let first_pts = first
        .pts
        .ok_or_else(|| invalid(cut, "The copied first video has no PTS."))?;
    let source_origin = document
        .format
        .as_ref()
        .and_then(|format| format.start_time.as_deref())
        .and_then(|time| time.parse::<f64>().ok())
        .unwrap_or(0.0);
    let original_first = original_first(
        ffmpeg,
        ffprobe,
        source,
        original_video.index,
        video_av1,
        first,
        start + source_origin,
        cancel,
    )
    .await?;
    let shift = first_pts - original_first;
    let matroska = is_matroska_path(cut);
    let interval = crate::jobs::trim::Interval {
        frames: 0,
        source_start_frame: 0,
        source_end_frame: 0,
        start: start + source_origin,
        end: end + source_origin,
    };
    let mut expected = interval.expected_document(document)?;
    expected.streams.clear();
    let mut proof = Vec::new();
    let mut max_end = 0.0_f64;
    let mut observed = actual.clone();
    for (original, stream) in mapping {
        let mut expected_stream = original.clone();
        expected_stream.index = stream.index;
        let kind = original.codec_type.as_deref().unwrap_or_default();
        let codec = original.codec_name.as_deref().unwrap_or_default();
        let attached_pic = original
            .disposition
            .get("attached_pic")
            .copied()
            .unwrap_or(0)
            != 0;
        let (count, first, end) = if kind == "attachment" {
            (0, None, None)
        } else if attached_pic {
            sequence_pass(
                ffmpeg,
                ffprobe,
                source,
                original.index,
                cut,
                stream.index,
                shift,
                false,
                false,
                false,
                matroska,
                None,
                cancel,
            )
            .await?
        } else {
            sequence(
                ffmpeg,
                ffprobe,
                source,
                original.index,
                cut,
                stream.index,
                shift,
                kind == "subtitle",
                codec == "mov_text",
                codec == "av1",
                matroska,
                original_first,
                end + source_origin,
                cancel,
            )
            .await?
        };
        if let Some(end) = end {
            max_end = max_end.max(end);
        }
        expected_stream.packet_start_time = first;
        expected_stream.start_time = first.map(|time| time.to_string());
        expected_stream.nb_read_packets = (count > 0).then(|| count.to_string());
        expected_stream.duration = Some(end.unwrap_or(0.0).to_string());
        expected_stream
            .tags
            .retain(|key, _| !metadata::is_derived_stream_tag(key));
        if is_iso_bmff_path(cut) {
            // FFmpeg's ISO BMFF muxer cannot write trak/udta `name` or `vendor_id`
            // stream tags when stream-copying a source cut (matching container::expected_document).
            expected_stream.tags.retain(|key, value| {
                !matches!(key.to_ascii_lowercase().as_str(), "name" | "vendor_id")
                    && !(key.eq_ignore_ascii_case("rotate") && value.trim() == "0")
            });
        }
        if let Some(actual) = observed
            .streams
            .iter_mut()
            .find(|actual| actual.index == stream.index)
        {
            actual.packet_start_time = first;
        }
        proof.push(TrackProof {
            source_index: original.index,
            cut_index: stream.index,
            kind: kind.into(),
            codec: original.codec_name.clone().unwrap_or_default(),
            packets: count,
            first,
            end,
        });
        expected.streams.push(expected_stream);
    }
    if let Some(format) = &mut expected.format {
        format.duration = Some(max_end.to_string());
    }
    metadata::verify(
        &expected,
        &expected.streams.iter().collect::<Vec<_>>(),
        &observed,
    )?;
    // Every stream is decoded in addition to the complete packet proof. A copied
    // GOP that needs an absent reference is rejected before any encoder receives it.
    let run = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args: [
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
            .chain([cut.as_os_str().to_owned()])
            .chain(
                ["-map", "0:v?", "-map", "0:a?", "-f", "null", "-"]
                    .into_iter()
                    .map(Into::into),
            )
            .collect(),
            cwd: None,
        },
        cancel.clone(),
        256 * 1024,
        TIME,
    )
    .await
    .map_err(|error| process_error(error, cut))?;
    if !run.status.success() || !run.stderr.is_empty() {
        return Err(invalid(
            cut,
            "The copied AV1AN source did not fully decode without errors.",
        ));
    }
    Ok((proof, first_pts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn document(streams: Value) -> Document {
        serde_json::from_value(json!({"streams": streams})).unwrap()
    }

    fn with_font() -> Document {
        document(json!([
            {"index": 0, "codec_type": "video", "codec_name": "av1", "nb_read_packets": "1"},
            {"index": 1, "codec_type": "audio", "codec_name": "aac", "nb_read_packets": "1"},
            {"index": 2, "codec_type": "subtitle", "codec_name": "ass"},
            {"index": 3, "codec_type": "attachment", "extradata_hash": "SHA256:font",
             "tags": {"filename": "font.ttf", "mimetype": "application/x-truetype-font"}}
        ]))
    }

    #[test]
    fn copied_codec_less_font_remains_in_mixed_stream_mapping() {
        let original = with_font();
        let actual = original.clone();
        let mapped = map_streams(&original, &actual, Path::new("cut.mkv")).unwrap();
        assert_eq!(mapped.len(), 4);
        assert_eq!(mapped[3].0.extradata_hash, mapped[3].1.extradata_hash);
        assert_eq!(mapped[3].0.tags["filename"], "font.ttf");
        metadata::verify(
            &original,
            &original.streams.iter().collect::<Vec<_>>(),
            &actual,
        )
        .unwrap();
        let mut changed = actual;
        changed.streams[3].extradata_hash = Some("SHA256:changed".into());
        assert!(
            metadata::verify(
                &original,
                &original.streams.iter().collect::<Vec<_>>(),
                &changed,
            )
            .is_err()
        );
    }

    #[test]
    fn source_cut_cannot_drop_or_change_dolby_vision_configuration() {
        let original = document(json!([{
            "index": 0, "codec_type": "video", "codec_name": "hevc",
            "side_data_list": [{"side_data_type": "DOVI configuration record", "dv_profile": 8}]
        }]));
        let cut = Path::new("cut.mp4");
        assert!(copied_dolby_vision_configuration_matches(&original, &original, cut).unwrap());
        let mut changed = original.clone();
        changed.streams[0].side_data_list[0]["dv_profile"] = json!(5);
        assert!(!copied_dolby_vision_configuration_matches(&original, &changed, cut).unwrap());
        changed.streams[0].side_data_list.clear();
        assert!(!copied_dolby_vision_configuration_matches(&original, &changed, cut).unwrap());
    }

    #[test]
    fn source_cut_cannot_omit_or_add_codec_less_attachments() {
        let original = with_font();
        let mut omitted = original.clone();
        omitted.streams.pop();
        assert!(map_streams(&original, &omitted, Path::new("cut.mkv")).is_err());
        let mut added = original.clone();
        let mut extra = added.streams[3].clone();
        extra.index = 4;
        added.streams.push(extra);
        assert!(map_streams(&original, &added, Path::new("cut.mkv")).is_err());
    }

    #[test]
    fn source_cut_still_rejects_added_media_and_changed_codec_or_order() {
        let original = with_font();
        let mut actual = original.clone();
        actual.streams.push(actual.streams[1].clone());
        assert!(map_streams(&original, &actual, Path::new("cut.mkv")).is_err());
        let mut actual = original.clone();
        actual.streams[1].codec_name = None;
        assert!(map_streams(&original, &actual, Path::new("cut.mkv")).is_err());
        let mut actual = original.clone();
        actual.streams.swap(1, 2);
        assert!(map_streams(&original, &actual, Path::new("cut.mkv")).is_err());
    }

    #[test]
    fn framehash_parser_and_mov_text_filler_detection() {
        let sample = b"#format: frame checksums\n#tb 0: 1/90000\n0,          0,          0,     2191,   606574, b2b845d40eb8fee0cb74fe04a52934ab9999647ee4f55529b28040d37c6d14ca\n0,      NOPTS,       2191,     1498,   191415, f0ba45c9a65e6b82ffd04e7f7a7ec86c2b2bb35082210356d228615aa4997a8f\n";
        let mut cursor = std::io::Cursor::new(&sample[..]);
        let mut parsed = Vec::new();
        let count = parse_framehash_packets(&mut cursor, |p| {
            parsed.push(p);
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(parsed[0].size, 606574);
        assert_eq!(parsed[0].pts, Some(0.0));
        assert_eq!(parsed[0].dts, Some(0.0));
        assert_eq!(parsed[1].dts, None);
        assert!(parsed[1].hash.starts_with("SHA256:f0ba45"));

        let filler = mux::Packet {
            pts: Some(0.0),
            dts: Some(0.0),
            duration: Some(0.041709),
            size: 2,
            hash: EMPTY_MOV_TEXT_SHA256.into(),
        };
        assert!(is_empty_mov_text_filler(&filler, 0.041708));
        assert!(!is_empty_mov_text_filler(&filler, 0.0));
        assert!(!is_empty_mov_text_filler(&filler, 0.5));
        let non_empty = mux::Packet { size: 4, ..filler };
        assert!(!is_empty_mov_text_filler(&non_empty, 0.041708));
    }
}
