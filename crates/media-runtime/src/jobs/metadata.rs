use std::collections::BTreeMap;

use media_core::AppError;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Document {
    #[serde(default)]
    pub streams: Vec<Stream>,
    #[serde(default)]
    pub chapters: Vec<Chapter>,
    pub format: Option<Format>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Format {
    pub format_name: Option<String>,
    pub duration: Option<String>,
    pub start_time: Option<String>,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Stream {
    pub index: u32,
    pub codec_type: Option<String>,
    pub codec_name: Option<String>,
    pub codec_tag_string: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sample_rate: Option<String>,
    pub bits_per_raw_sample: Option<String>,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    pub duration: Option<String>,
    pub nb_read_packets: Option<String>,
    pub extradata_hash: Option<String>,
    pub pix_fmt: Option<String>,
    pub field_order: Option<String>,
    pub sample_aspect_ratio: Option<String>,
    pub avg_frame_rate: Option<String>,
    pub r_frame_rate: Option<String>,
    pub time_base: Option<String>,
    pub start_time: Option<String>,
    pub color_space: Option<String>,
    // Subtitle headers may inherit the wrong start and some video headers omit
    // it. Filled from a bounded actual-packet probe, never guessed from headers.
    #[serde(skip)]
    pub packet_start_time: Option<f64>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub color_range: Option<String>,
    pub chroma_location: Option<String>,
    #[serde(default)]
    pub side_data_list: Vec<serde_json::Value>,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    #[serde(default)]
    pub disposition: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Chapter {
    pub start_time: String,
    pub end_time: String,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

fn seconds(text: &str) -> Option<f64> {
    text.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

fn clock_seconds(text: &str) -> Option<f64> {
    let mut parts = text.split(':');
    let hours = seconds(parts.next()?)?;
    let minutes = seconds(parts.next()?)?;
    let seconds = seconds(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

impl Document {
    pub fn duration(&self) -> Option<f64> {
        self.format
            .as_ref()
            .and_then(|f| f.duration.as_deref())
            .and_then(seconds)
    }

    pub fn selected<'a>(&'a self, indices: &[u32]) -> Result<Vec<&'a Stream>, AppError> {
        let mut streams = Vec::new();
        let mut attachment_seen = false;
        for index in indices {
            let stream = self
                .streams
                .iter()
                .find(|s| s.index == *index)
                .ok_or_else(|| {
                    AppError::new(
                        "STREAM_NOT_FOUND",
                        format!("Source stream {index} no longer exists. Import the file again."),
                        None,
                    )
                })?;
            match stream.codec_type.as_deref() {
                Some("attachment") => attachment_seen = true,
                Some("video" | "audio" | "subtitle" | "data") if !attachment_seen => {}
                Some("video" | "audio" | "subtitle" | "data") => {
                    return Err(AppError::new(
                        "STREAM_ORDER_UNSUPPORTED",
                        "Matroska places attachments after media tracks. Put selected attachments last.",
                        None,
                    ));
                }
                _ => {
                    return Err(AppError::new(
                        "STREAM_UNSUPPORTED",
                        format!("Stream {index} has an unsupported type for this workflow."),
                        None,
                    ));
                }
            }
            streams.push(stream);
        }
        if !streams.is_empty()
            && !streams.iter().any(|s| {
                matches!(
                    s.codec_type.as_deref(),
                    Some("video" | "audio" | "subtitle" | "data")
                )
            })
        {
            return Err(AppError::new(
                "STREAM_SELECTION_INVALID",
                "Select at least one media stream.",
                None,
            ));
        }
        Ok(streams)
    }

    pub fn selected_duration(&self, streams: &[&Stream]) -> Option<f64> {
        streams
            .iter()
            .filter(|s| {
                matches!(
                    s.codec_type.as_deref(),
                    Some("video" | "audio" | "subtitle" | "data")
                )
            })
            .filter_map(|s| {
                s.duration.as_deref().and_then(seconds).or_else(|| {
                    s.tags
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("duration"))
                        .and_then(|(_, v)| clock_seconds(v))
                })
            })
            .reduce(f64::max)
            .or_else(|| self.duration())
    }

    /// The end every output must reach: the longest selected video, audio or
    /// data track. Text subtitle tracks may end with an empty "clear" cue whose
    /// duration runs past the picture and sound (MP4 mov_text does this), and
    /// converting such a track drops that empty cue. Subtitle tails therefore
    /// only bound the permitted end; they set the required end only when the
    /// selection has no other media, and then only for copied subtitles.
    /// Converted cues are verified separately against every readable cue.
    pub fn required_duration(
        &self,
        streams: &[&Stream],
        converted_subtitles: &[u32],
    ) -> Option<f64> {
        let media = streams
            .iter()
            .copied()
            .filter(|s| matches!(s.codec_type.as_deref(), Some("video" | "audio" | "data")))
            .collect::<Vec<_>>();
        if !media.is_empty() {
            return self.selected_duration(&media);
        }
        let copied = streams
            .iter()
            .copied()
            .filter(|s| {
                s.codec_type.as_deref() == Some("subtitle")
                    && !converted_subtitles.contains(&s.index)
            })
            .collect::<Vec<_>>();
        (!copied.is_empty())
            .then(|| self.selected_duration(&copied))
            .flatten()
    }
}

/// Computed stream statistics and encoder provenance describe the old bitstream
/// after transcoding. Copied streams keep these tags; encodes must clear them.
pub(super) fn is_derived_stream_tag(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    matches!(
        key.as_str(),
        "encoder" | "duration" | "bps" | "number_of_frames" | "number_of_bytes"
    ) || key.starts_with("_statistics_")
        || ["duration-", "encoder-"].iter().any(|prefix| {
            key.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.len() == 3 && suffix.bytes().all(|byte| byte.is_ascii_alphabetic())
            })
        })
        || key.starts_with("bps-")
        || key.starts_with("number_of_frames-")
        || key.starts_with("number_of_bytes-")
}

fn stable_tags(tags: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    tags.iter()
        .filter_map(|(key, value)| {
            let key = key.to_ascii_lowercase();
            if is_derived_stream_tag(&key) || (key == "language" && value == "und") {
                return None;
            }
            Some((key, value.clone()))
        })
        .collect()
}

fn tags_preserved(source: &BTreeMap<String, String>, output: &BTreeMap<String, String>) -> bool {
    let output = stable_tags(output);
    stable_tags(source)
        .iter()
        .all(|(k, v)| output.get(k) == Some(v))
}

fn format_tags_preserved(
    source: &BTreeMap<String, String>,
    output: &BTreeMap<String, String>,
) -> bool {
    let output = stable_tags(output);
    stable_tags(source).iter().all(|(key, value)| {
        // These describe the source container's file structure. FFmpeg writes
        // the destination brand itself; carrying MOV values into the verified
        // Matroska staging file creates duplicate, semicolon-joined tags.
        matches!(
            key.as_str(),
            "major_brand" | "minor_version" | "compatible_brands"
        ) || output.get(key) == Some(value)
    })
}

pub(super) fn verify(
    source: &Document,
    selected: &[&Stream],
    output: &Document,
) -> Result<(), AppError> {
    verify_inner(source, selected, output, &[], &[], &[])
}

/// Container text conversion is verified separately against every readable cue.
/// Native text containers may add empty gap packets, so their packet counts and
/// header start times do not describe the source cue sequence.
pub(super) fn verify_container(
    source: &Document,
    selected: &[&Stream],
    output: &Document,
    converted_subtitles: &[u32],
) -> Result<(), AppError> {
    if converted_subtitles.is_empty() {
        return verify(source, selected, output);
    }
    verify_inner(source, selected, output, &[], &[], converted_subtitles)
}

pub(super) fn verify_encoded(
    source: &Document,
    selected: &[&Stream],
    output: &Document,
    video_index: u32,
    expected_codec: &str,
    expected_dimensions: (u32, u32),
    audio: &[media_core::AudioTrackSettings],
) -> Result<(), AppError> {
    verify_inner(
        source,
        selected,
        output,
        &[EncodedVideo {
            index: video_index,
            codec: expected_codec,
            dimensions: expected_dimensions,
        }],
        audio,
        &[],
    )
}

pub(super) fn verify_audio_conversion(
    source: &Document,
    selected: &[&Stream],
    output: &Document,
    audio: &[media_core::AudioTrackSettings],
) -> Result<(), AppError> {
    verify_inner(source, selected, output, &[], audio, &[])
}

pub(super) fn verify_audio_conversion_with_subtitles(
    source: &Document,
    selected: &[&Stream],
    output: &Document,
    audio: &[media_core::AudioTrackSettings],
    converted_subtitles: &[u32],
) -> Result<(), AppError> {
    if converted_subtitles.is_empty() {
        return verify_audio_conversion(source, selected, output, audio);
    }
    verify_inner(source, selected, output, &[], audio, converted_subtitles)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn verify_encoded_with_subtitles(
    source: &Document,
    selected: &[&Stream],
    output: &Document,
    video_index: u32,
    expected_codec: &str,
    expected_dimensions: (u32, u32),
    audio: &[media_core::AudioTrackSettings],
    converted_subtitles: &[u32],
) -> Result<(), AppError> {
    if converted_subtitles.is_empty() {
        return verify_encoded(
            source,
            selected,
            output,
            video_index,
            expected_codec,
            expected_dimensions,
            audio,
        );
    }
    verify_inner(
        source,
        selected,
        output,
        &[EncodedVideo {
            index: video_index,
            codec: expected_codec,
            dimensions: expected_dimensions,
        }],
        audio,
        converted_subtitles,
    )
}

pub(super) struct EncodedVideo<'a> {
    pub index: u32,
    pub codec: &'a str,
    pub dimensions: (u32, u32),
}

fn verify_inner(
    source: &Document,
    selected: &[&Stream],
    output: &Document,
    encoded_videos: &[EncodedVideo<'_>],
    audio: &[media_core::AudioTrackSettings],
    converted_subtitles: &[u32],
) -> Result<(), AppError> {
    let fail = |message: &str| AppError::new("OUTPUT_VALIDATION_FAILED", message, None);
    if selected.len() != output.streams.len() {
        return Err(fail(
            "The output does not contain exactly the selected streams.",
        ));
    }
    for (expected, actual) in selected.iter().zip(&output.streams) {
        let encoded_video = encoded_videos
            .iter()
            .find(|video| video.index == expected.index);
        let encoded = encoded_video.is_some();
        let (expected_width, expected_height) = if encoded {
            let (width, height) = encoded_video.expect("encoded video plan").dimensions;
            (Some(width), Some(height))
        } else {
            (expected.width, expected.height)
        };
        let converted_audio = audio.iter().find(|track| {
            track.stream_index == expected.index && track.codec != media_core::AudioCodec::Copy
        });
        let track_start = |stream: &Stream| {
            stream.packet_start_time.or_else(|| {
                stream
                    .start_time
                    .as_deref()
                    .and_then(|v| v.parse::<f64>().ok())
            })
        };
        let converted_subtitle = expected.codec_type.as_deref() == Some("subtitle")
            && converted_subtitles.contains(&expected.index);
        if let Some(source_start) = track_start(expected).filter(|_| {
            converted_audio.is_none()
                    && !converted_subtitle
                    // Attachments have no playback timeline. FFprobe may give
                    // them the container start, which belongs to another file
                    // when an attachment is copied into a mixed-source output.
                    && expected.codec_type.as_deref() != Some("attachment")
        }) {
            let output_start = track_start(actual);
            if !output_start
                .is_some_and(|start| start.is_finite() && (start - source_start).abs() <= 0.002)
            {
                return Err(fail(&format!(
                    "The output track start time changed, which could affect synchronization (source stream {}, output stream {}: {source_start:.6} s -> {output_start:?} s).",
                    expected.index, actual.index,
                )));
            }
        }
        if expected.codec_type != actual.codec_type
            || (expected.codec_type.as_deref() == Some("data")
                && expected.codec_tag_string != actual.codec_tag_string)
            || if encoded {
                actual.codec_name.as_deref() != encoded_video.map(|video| video.codec)
            } else if let Some(track) = converted_audio {
                actual.codec_name.as_deref() != Some(super::audio::codec_name(track.codec))
            } else {
                expected.codec_name != actual.codec_name
            }
            || expected_width != actual.width
            || expected_height != actual.height
            || converted_audio.map_or_else(
                || expected.sample_rate.clone(),
                |track| super::audio::expected_rate(expected, track),
            ) != actual.sample_rate
            || converted_audio.map_or(expected.channels, |track| {
                super::audio::expected_channels(expected, track)
            }) != actual.channels
            || converted_audio.is_some_and(|track| {
                super::audio::expected_channels(expected, track)
                    .is_some_and(|channels| channels > 2)
                    && super::audio::expected_layout(expected, track)
                        != actual.channel_layout.as_deref()
            })
            || converted_audio.is_some_and(|track| {
                track.codec == media_core::AudioCodec::Flac
                    && actual.bits_per_raw_sample.as_deref() != Some("24")
            })
        {
            return Err(fail(&format!(
                "The output stream order, codecs, or media properties changed (source stream {}, output {}: expected {:?}/{:?} {:?}x{:?}, audio {:?}/{:?}; actual {:?}/{:?} {:?}x{:?}, audio {:?}/{:?}).",
                expected.index,
                actual.index,
                expected.codec_type,
                encoded_video
                    .map(|video| video.codec)
                    .or_else(|| converted_audio.map(|track| super::audio::codec_name(track.codec)))
                    .or(expected.codec_name.as_deref()),
                expected_width,
                expected_height,
                converted_audio.map_or_else(
                    || expected.sample_rate.clone(),
                    |track| super::audio::expected_rate(expected, track)
                ),
                converted_audio.map_or(expected.channels, |track| super::audio::expected_channels(
                    expected, track
                )),
                actual.codec_type,
                actual.codec_name,
                actual.width,
                actual.height,
                actual.sample_rate,
                actual.channels,
            )));
        }
        let media_track = matches!(expected.codec_type.as_deref(), Some("video" | "audio"));
        let expected_packets = expected
            .nb_read_packets
            .as_deref()
            .and_then(|v| v.parse::<u64>().ok());
        let actual_packets = actual
            .nb_read_packets
            .as_deref()
            .and_then(|v| v.parse::<u64>().ok());
        if !encoded
            && converted_audio.is_none()
            && !converted_subtitle
            && ((media_track && !expected_packets.is_some_and(|count| count > 0))
                || expected_packets != actual_packets)
        {
            return Err(fail(
                "The output packet count differs from the selected source track.",
            ));
        }
        if expected.codec_type.as_deref() == Some("attachment")
            && (expected.extradata_hash.is_none()
                || expected.extradata_hash != actual.extradata_hash)
        {
            return Err(fail("The output attachment contents changed."));
        }
        if !tags_preserved(&expected.tags, &actual.tags) {
            return Err(fail(
                "The output did not preserve the selected stream metadata.",
            ));
        }
        for (flag, value) in &expected.disposition {
            if actual.disposition.get(flag).copied().unwrap_or(0) != *value {
                return Err(fail("The output did not preserve stream dispositions."));
            }
        }
    }
    if let Some(source_format) = &source.format
        && !output
            .format
            .as_ref()
            .is_some_and(|f| format_tags_preserved(&source_format.tags, &f.tags))
    {
        return Err(fail("The output did not preserve container metadata."));
    }
    if source.chapters.len() != output.chapters.len() {
        return Err(fail("The output chapter count changed."));
    }
    for (expected, actual) in source.chapters.iter().zip(&output.chapters) {
        for (a, b) in [
            (&expected.start_time, &actual.start_time),
            (&expected.end_time, &actual.end_time),
        ] {
            if !matches!((seconds(a), seconds(b)), (Some(a), Some(b)) if (a-b).abs() <= 0.002) {
                return Err(fail("The output chapter timing changed."));
            }
        }
        if !tags_preserved(&expected.tags, &actual.tags) {
            return Err(fail("The output chapter metadata changed."));
        }
    }
    let permitted = source.selected_duration(selected).filter(|v| *v > 0.0);
    let required = source
        .required_duration(selected, converted_subtitles)
        .filter(|v| *v > 0.0);
    if let Some(permitted) = permitted.or(required) {
        let actual = output
            .duration()
            .ok_or_else(|| fail("The output duration could not be validated."))?;
        // Keep the original lower bound and copied-track tolerance. A converted
        // AAC track can extend the container's reported end by its last frame
        // and codec delay. MP3 retains LAME's 1105-sample delay in its reported
        // duration (138 ms at 8 kHz), even after decoded samples are trimmed.
        // Decoded start and sample count remain separately checked, including
        // final padding; neither codec permits arbitrary audible extension.
        // The lower bound comes from picture, sound and data only; a longer
        // subtitle tail may raise the upper bound but never the required end.
        let tolerance = 0.1_f64.max(permitted * 0.0001);
        let mut latest_end = permitted.max(required.unwrap_or(0.0)) + tolerance;
        for track in audio.iter().filter(|track| {
            matches!(
                track.codec,
                media_core::AudioCodec::Aac | media_core::AudioCodec::Mp3
            )
        }) {
            if let Some(stream) = selected
                .iter()
                .find(|stream| stream.index == track.stream_index)
                && let Some(rate) = stream
                    .sample_rate
                    .as_deref()
                    .and_then(|rate| rate.parse::<f64>().ok())
                    .filter(|rate| *rate > 0.0)
                && let Some(end) = source.selected_duration(&[stream])
            {
                let samples = if track.codec == media_core::AudioCodec::Aac {
                    2047.0
                } else {
                    1105.0
                };
                latest_end = latest_end.max(end + samples / rate + 0.002);
            }
        }
        let lower = required.map(|required| required - tolerance);
        if lower.is_some_and(|lower| actual < lower) || actual > latest_end {
            return Err(fail(&format!(
                "The output duration differs from the selected source tracks: observed {actual:.6}s, expected {expected:.6}s (allowed {lower:.6}..{latest_end:.6}s).",
                expected = required.unwrap_or(permitted),
                lower = lower.unwrap_or(0.0),
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Document {
        serde_json::from_value(json!({"streams":[
            {"index":0,"codec_type":"video","codec_name":"h264","width":64,"height":64,"nb_read_packets":"48","tags":{"title":"Picture"}},
            {"index":2,"codec_type":"audio","codec_name":"flac","channels":2,"nb_read_packets":"96","tags":{"LANGUAGE":"jpn","title":"Original"}},
            {"index":5,"codec_type":"attachment","extradata_hash":"SHA256:fixture","tags":{"filename":"font.ttf","mimetype":"application/x-truetype-font"}}
        ],"chapters":[{"start_time":"0","end_time":"2","tags":{"title":"Opening"}}],"format":{"duration":"2","tags":{"title":"Sample","ENCODER":"source tool"}}})).unwrap()
    }

    /// MP4 mov_text tracks end with an empty two-byte "clear" sample whose
    /// duration can run seconds past the picture. Converting to SRT drops it.
    fn subtitle_tail(output_duration: &str) -> (Document, Document) {
        let source: Document = serde_json::from_value(json!({"streams":[
            {"index":0,"codec_type":"video","codec_name":"hevc","width":64,"height":64,"duration":"600.058000","nb_read_packets":"14387"},
            {"index":1,"codec_type":"audio","codec_name":"eac3","channels":6,"sample_rate":"48000","duration":"600.032000","nb_read_packets":"18751"},
            {"index":3,"codec_type":"subtitle","codec_name":"mov_text","duration":"607.983000","nb_read_packets":"240"}
        ],"format":{"duration":"607.983000"}}))
        .unwrap();
        let mut output = source.clone();
        output.format.as_mut().unwrap().duration = Some(output_duration.into());
        (source, output)
    }

    #[test]
    fn converted_subtitle_tails_do_not_set_the_required_duration() {
        let (source, output) = subtitle_tail("600.058000");
        let selected = source.selected(&[0, 1, 3]).unwrap();
        verify_container(&source, &selected, &output, &[3]).unwrap();
        // A copied track keeps its packets, but its tail still only bounds the end.
        verify(&source, &selected, &output).unwrap();
        // Picture and sound remain strictly required.
        let (source, short) = subtitle_tail("599.500000");
        let error = verify_container(&source, &selected, &short, &[3]).unwrap_err();
        assert_eq!(error.code, "OUTPUT_VALIDATION_FAILED");
        assert!(
            error.message.contains("expected 600.058000s"),
            "{}",
            error.message
        );
        // The subtitle tail is the most any output may extend to.
        let (source, long) = subtitle_tail("608.200000");
        assert!(verify_container(&source, &selected, &long, &[3]).is_err());
        let (source, tail) = subtitle_tail("607.983000");
        verify_container(&source, &selected, &tail, &[3]).unwrap();
    }

    #[test]
    fn subtitle_only_selections_keep_copied_ends_and_skip_converted_tails() {
        let (source, output) = subtitle_tail("600.058000");
        let selected = source.selected(&[3]).unwrap();
        let mut output = output;
        output.streams = vec![source.streams[2].clone()];
        verify_container(&source, &selected, &output, &[3]).unwrap();
        assert!(verify(&source, &selected, &output).is_err());
        assert_eq!(source.required_duration(&selected, &[3]), None);
        assert_eq!(source.required_duration(&selected, &[]), Some(607.983));
    }

    #[test]
    fn destination_container_brands_do_not_replace_descriptive_metadata() {
        let source = BTreeMap::from([
            ("major_brand".into(), "qt  ".into()),
            ("minor_version".into(), "512".into()),
            ("compatible_brands".into(), "qt  ".into()),
            ("title".into(), "Original".into()),
        ]);
        let mut output = BTreeMap::from([("TITLE".into(), "Original".into())]);
        assert!(format_tags_preserved(&source, &output));
        assert!(!tags_preserved(&source, &output));
        output.insert("TITLE".into(), "Changed".into());
        assert!(!format_tags_preserved(&source, &output));
    }

    #[test]
    fn attachment_container_starts_are_not_playback_timestamps() {
        let mut source = fixture();
        source.streams[2].start_time = Some("0.250".into());
        let mut output = source.clone();
        output.streams[2].start_time = Some("0.000".into());
        let selected = source.selected(&[0, 2, 5]).unwrap();
        verify(&source, &selected, &output).unwrap();
        output.streams[2].extradata_hash = Some("SHA256:changed".into());
        assert!(verify(&source, &selected, &output).is_err());
    }

    #[test]
    fn validates_selected_order_chapters_and_attachments() {
        let source = fixture();
        let selected = source.selected(&[2, 0, 5]).unwrap();
        let mut output = fixture();
        output.streams.swap(0, 1);
        verify(&source, &selected, &output).unwrap();
        output.streams[2]
            .tags
            .insert("filename".into(), "lost.ttf".into());
        assert_eq!(
            verify(&source, &selected, &output).unwrap_err().code,
            "OUTPUT_VALIDATION_FAILED"
        );
    }

    #[test]
    fn encoded_codec_is_checked_without_relaxing_copied_tracks() {
        let source = fixture();
        let selected = source.selected(&[2, 0, 5]).unwrap();
        let mut output = fixture();
        output.streams.swap(0, 1);
        verify_encoded(&source, &selected, &output, 0, "h264", (64, 64), &[]).unwrap();
        assert!(verify_encoded(&source, &selected, &output, 0, "av1", (64, 64), &[]).is_err());
        output.streams[1].codec_name = Some("av1".into());
        verify_encoded(&source, &selected, &output, 0, "av1", (64, 64), &[]).unwrap();
        assert!(verify_encoded(&source, &selected, &output, 0, "h264", (64, 64), &[]).is_err());
        output.streams[0].codec_name = Some("aac".into());
        assert!(verify_encoded(&source, &selected, &output, 0, "av1", (64, 64), &[]).is_err());
    }

    #[test]
    fn framing_uses_planned_dimensions_only_for_the_encoded_video() {
        let mut source = fixture();
        let mut copied_video = source.streams[0].clone();
        copied_video.index = 9;
        source.streams.insert(2, copied_video);
        let selected = source.selected(&[2, 0, 9, 5]).unwrap();
        let mut output = source.clone();
        output.streams.swap(0, 1);
        assert!(verify_encoded(&source, &selected, &output, 0, "h264", (96, 128), &[]).is_err());
        output.streams[1].width = Some(96);
        output.streams[1].height = Some(128);
        verify_encoded(&source, &selected, &output, 0, "h264", (96, 128), &[]).unwrap();
        assert!(verify(&source, &selected, &output).is_err());
        output.streams[2].width = Some(96);
        assert!(verify_encoded(&source, &selected, &output, 0, "h264", (96, 128), &[]).is_err());
    }

    #[test]
    fn audio_conversion_leaves_copied_packets_and_video_timestamps_strict() {
        let mut source = fixture();
        source.streams[0].start_time = Some("0".into());
        let mut copied = source.streams[1].clone();
        copied.index = 7;
        source.streams.insert(2, copied);
        let selected = source.selected(&[2, 0, 7, 5]).unwrap();
        let mut output = source.clone();
        output.streams.swap(0, 1);
        output.streams[0].codec_name = Some("opus".into());
        output.streams[0].sample_rate = Some("48000".into());
        output.streams[0].nb_read_packets = Some("12".into());
        let tracks = [media_core::AudioTrackSettings {
            normalization: None,
            stream_index: 2,
            codec: media_core::AudioCodec::Opus,
            bitrate_kbps: 128,
            channels: media_core::AudioChannels::Preserve,
            gain: None,
        }];
        verify_encoded(&source, &selected, &output, 0, "h264", (64, 64), &tracks).unwrap();
        output.streams[1].start_time = Some("0.006".into());
        assert!(verify_encoded(&source, &selected, &output, 0, "h264", (64, 64), &tracks).is_err());
        output.streams[1].start_time = Some("0".into());
        output.streams[2].nb_read_packets = Some("1".into());
        assert!(verify_encoded(&source, &selected, &output, 0, "h264", (64, 64), &tracks).is_err());
    }

    #[test]
    fn rejects_reordered_or_short_outputs_and_lost_chapters() {
        let source = fixture();
        let selected = source.selected(&[0, 2, 5]).unwrap();
        let mut output = fixture();
        output.streams.swap(0, 1);
        assert!(verify(&source, &selected, &output).is_err());
        let mut output = fixture();
        output.format.as_mut().unwrap().duration = Some("0.1".into());
        assert!(verify(&source, &selected, &output).is_err());
        let mut output = fixture();
        output.chapters.clear();
        assert!(verify(&source, &selected, &output).is_err());
        assert!(source.selected(&[5, 0]).is_err());
        assert!(source.selected(&[5]).is_err());
    }

    #[test]
    fn subtitle_packet_start_overrides_estimated_header_but_rejects_shifted_cues() {
        let mut source = fixture();
        source.streams[1].codec_type = Some("subtitle".into());
        source.streams[1].codec_name = Some("ass".into());
        source.streams[1].start_time = Some("0".into());
        source.streams[1].packet_start_time = Some(0.740);
        let selected = source.selected(&[0, 2, 5]).unwrap();
        let mut output = source.clone();
        output.streams[1].start_time = Some("0.740".into());
        verify(&source, &selected, &output).unwrap();
        output.streams[1].packet_start_time = Some(0.750);
        assert!(verify(&source, &selected, &output).is_err());
        output.streams[1].packet_start_time = Some(f64::NAN);
        assert!(verify(&source, &selected, &output).is_err());
    }

    #[test]
    fn missing_or_mismatched_packet_and_attachment_evidence_cannot_succeed() {
        let source = fixture();
        let selected = source.selected(&[0, 2, 5]).unwrap();
        let mut output = fixture();
        output.streams[0].nb_read_packets = Some("47".into());
        assert!(verify(&source, &selected, &output).is_err());
        let mut no_evidence = fixture();
        no_evidence.streams[0].nb_read_packets = None;
        assert!(
            verify(
                &no_evidence,
                &no_evidence.selected(&[0, 2, 5]).unwrap(),
                &no_evidence
            )
            .is_err()
        );
        let mut no_hash = fixture();
        no_hash.streams[2].extradata_hash = None;
        assert!(verify(&no_hash, &no_hash.selected(&[0, 2, 5]).unwrap(), &no_hash).is_err());
    }
}
