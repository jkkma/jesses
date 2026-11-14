//! Complete original/copy access-unit and metadata proof for source AV1AN cuts.
use super::*;
use crate::jobs::{metadata, mux};
use std::sync::mpsc;
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
async fn first_packet(
    ffprobe: &Path,
    path: &Path,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<mux::Packet, AppError> {
    let spec = mux::packet_spec(ffprobe, path, index);
    let result = supervisor::run_streaming_stdout(&spec, cancel.clone(), 8192, TIME, |reader| {
        let mut first = None;
        mux::packets(reader, |packet| {
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
async fn original_first(
    ffprobe: &Path,
    path: &Path,
    index: u32,
    first: mux::Packet,
    bound: f64,
    cancel: &watch::Receiver<bool>,
) -> Result<f64, AppError> {
    let spec = mux::packet_spec(ffprobe, path, index);
    let result=supervisor::run_streaming_stdout(&spec,cancel.clone(),8192,TIME,move|reader|{let mut time=None;mux::packets(reader,|packet|{if packet.hash==first.hash&&packet.size==first.size&&packet.pts.is_some_and(|pts|pts<=bound+0.002){time=packet.pts;}Ok(())})?;time.ok_or_else(||"The copied first video packet has no matching keyframe at/before the requested source seek.".to_owned())}).await.map_err(|error|process_error(error,path))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(invalid(
            path,
            "The original keyframe packet could not be proven.",
        ));
    }
    Ok(result.value)
}
fn same(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 0.002,
        (None, None) => true,
        _ => false,
    }
}
#[allow(clippy::too_many_arguments)]
async fn sequence(
    ffprobe: &Path,
    source: &Path,
    source_index: u32,
    cut: &Path,
    cut_index: u32,
    shift: f64,
    subtitle: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<(u64, Option<f64>, Option<f64>), AppError> {
    let (sender, receiver) = mpsc::sync_channel(16);
    let cut_spec = mux::packet_spec(ffprobe, cut, cut_index);
    let source_spec = mux::packet_spec(ffprobe, source, source_index);
    let actual =
        supervisor::run_streaming_stdout(&cut_spec, cancel.clone(), 8192, TIME, move |reader| {
            mux::packets(reader, |packet| {
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
            let mut started = false;
            let mut count = 0;
            let mut first = None;
            let mut end = None;
            mux::packets(reader, |packet| {
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
                let matches = packet.hash == actual.hash
                    && packet.size == actual.size
                    && same(packet.duration, actual.duration)
                    && same(shifted(packet.pts), actual.pts)
                    && same(shifted(packet.dts), actual.dts);
                if !started {
                    if !matches {
                        return Ok(());
                    }
                    started = true;
                    first = shifted(packet.pts);
                }
                if !matches {
                    return Err("A source-copy packet changed payload/order/duration or its common keyframe clock.".into());
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
    let actual = actual.map_err(|error| process_error(error, cut))?;
    let expected = expected.map_err(|error| process_error(error, source))?;
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
    let first = first_packet(ffprobe, cut, cut_video.index, cancel).await?;
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
        ffprobe,
        source,
        original_video.index,
        first,
        start + source_origin,
        cancel,
    )
    .await?;
    let shift = first_pts - original_first;
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
        let (count, first, end) = if kind == "attachment" {
            (0, None, None)
        } else {
            sequence(
                ffprobe,
                source,
                original.index,
                cut,
                stream.index,
                shift,
                kind == "subtitle",
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
}
