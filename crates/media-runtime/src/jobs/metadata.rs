use std::collections::BTreeMap;

use media_core::AppError;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct Document {
    #[serde(default)]
    pub streams: Vec<Stream>,
    #[serde(default)]
    pub chapters: Vec<Chapter>,
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Format {
    pub duration: Option<String>,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Stream {
    pub index: u32,
    pub codec_type: Option<String>,
    pub codec_name: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sample_rate: Option<String>,
    pub channels: Option<u32>,
    pub duration: Option<String>,
    pub nb_read_packets: Option<String>,
    pub extradata_hash: Option<String>,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    #[serde(default)]
    pub disposition: BTreeMap<String, u32>,
}

#[derive(Debug, Deserialize)]
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
                Some("video" | "audio" | "subtitle") if !attachment_seen => {}
                Some("video" | "audio" | "subtitle") => {
                    return Err(AppError::new(
                        "STREAM_ORDER_UNSUPPORTED",
                        "Matroska places attachments after media tracks. Put selected attachments last.",
                        None,
                    ));
                }
                _ => {
                    return Err(AppError::new(
                        "STREAM_UNSUPPORTED",
                        format!("Stream {index} cannot be copied by this Matroska workflow."),
                        None,
                    ));
                }
            }
            streams.push(stream);
        }
        if !streams
            .iter()
            .any(|s| matches!(s.codec_type.as_deref(), Some("video" | "audio")))
        {
            return Err(AppError::new(
                "STREAM_SELECTION_INVALID",
                "Select at least one video or audio stream.",
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
                    Some("video" | "audio" | "subtitle")
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
}

fn stable_tags(tags: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    tags.iter()
        .filter_map(|(key, value)| {
            let key = key.to_ascii_lowercase();
            if matches!(
                key.as_str(),
                "encoder" | "duration" | "bps" | "number_of_frames" | "number_of_bytes"
            ) || key.starts_with("_statistics_")
                || key.starts_with("bps-")
                || key.starts_with("number_of_frames-")
                || key.starts_with("number_of_bytes-")
                || (key == "language" && value == "und")
            {
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

pub(super) fn verify(
    source: &Document,
    selected: &[&Stream],
    output: &Document,
) -> Result<(), AppError> {
    let fail = |message: &str| AppError::new("OUTPUT_VALIDATION_FAILED", message, None);
    if selected.len() != output.streams.len() {
        return Err(fail(
            "The output does not contain exactly the selected streams.",
        ));
    }
    for (expected, actual) in selected.iter().zip(&output.streams) {
        if expected.codec_type != actual.codec_type
            || expected.codec_name != actual.codec_name
            || expected.width != actual.width
            || expected.height != actual.height
            || expected.sample_rate != actual.sample_rate
            || expected.channels != actual.channels
        {
            return Err(fail(
                "The output stream order, codecs, or media properties changed.",
            ));
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
        if (media_track && !expected_packets.is_some_and(|count| count > 0))
            || expected_packets != actual_packets
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
            .is_some_and(|f| tags_preserved(&source_format.tags, &f.tags))
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
    if let Some(expected) = source.selected_duration(selected).filter(|v| *v > 0.0) {
        let actual = output
            .duration()
            .ok_or_else(|| fail("The output duration could not be validated."))?;
        // Matroska timestamp rounding and packet boundaries can differ slightly.
        if (expected - actual).abs() > 0.1_f64.max(expected * 0.0001) {
            return Err(fail(
                "The output duration differs from the selected source tracks.",
            ));
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
