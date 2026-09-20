//! Matroska copy jobs with stable source/stream mapping and per-track overrides.
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    io::{BufRead, BufReader, Read},
    path::Path,
    sync::mpsc as blocking_channel,
    time::Duration,
};

use media_core::{AppError, JobSnapshot, JobState, MuxRequest, MuxTrack, RemuxRequest};
use tokio::sync::{mpsc, watch};

use super::{
    JobManager, append_log, check_cancel, discover,
    files::{self, Source, Temporary},
    metadata::{self, Document, Stream},
    probe, process_error, progress_seconds,
};
use crate::supervisor::{self, CommandSpec, ProcessEvent};

fn invalid(message: &str) -> AppError {
    AppError::new("MUX_SETTINGS_INVALID", message, None)
}

pub(super) fn summary(request: &MuxRequest) -> Result<RemuxRequest, AppError> {
    if request.sources.is_empty()
        || request.sources.len() > 32
        || request.tracks.is_empty()
        || request.tracks.len() > 256
    {
        return Err(invalid(
            "Select 1–32 source files and 1–256 distinct output tracks.",
        ));
    }
    let mut ids = HashSet::new();
    for source in &request.sources {
        if source.id.is_empty()
            || source.id.len() > 128
            || !ids.insert(source.id.as_str())
            || source.input_path.contains('\0')
            || !Path::new(&source.input_path).is_absolute()
        {
            return Err(invalid(
                "Each source needs a distinct stable identifier and an absolute local path.",
            ));
        }
    }
    if !ids.contains(request.metadata_source_id.as_str())
        || request
            .chapters_source_id
            .as_ref()
            .is_some_and(|id| !ids.contains(id.as_str()))
    {
        return Err(invalid(
            "Choose an existing source for container metadata and chapters.",
        ));
    }
    let mut tracks = HashSet::new();
    for track in &request.tracks {
        if !ids.contains(track.source_id.as_str())
            || !tracks.insert((track.source_id.as_str(), track.stream_index))
        {
            return Err(invalid(
                "Each selected track must refer to an existing source and occur only once.",
            ));
        }
        if track
            .title
            .as_ref()
            .is_some_and(|value| value.len() > 4096 || value.contains('\0'))
        {
            return Err(invalid(
                "Track titles must be at most 4096 bytes and contain no null characters.",
            ));
        }
        if track.language.as_ref().is_some_and(|value| {
            !value.is_empty()
                && (value.len() != 3 || !value.bytes().all(|b| b.is_ascii_alphabetic()))
        }) {
            return Err(invalid(
                "Use a three-letter language code, or an empty value to clear the language tag.",
            ));
        }
    }
    let first = &request.tracks[0].source_id;
    let source = request
        .sources
        .iter()
        .find(|source| &source.id == first)
        .expect("validated source");
    let summary = RemuxRequest {
        input_path: source.input_path.clone(),
        output_path: request.output_path.clone(),
        stream_indices: request
            .tracks
            .iter()
            .filter(|track| &track.source_id == first)
            .map(|track| track.stream_index)
            .collect(),
    };
    files::validate_request(&summary)?;
    Ok(summary)
}

struct Input {
    id: String,
    source: Source,
    document: Document,
}

fn mapped_stream(stream: &Stream, track: &MuxTrack, index: u32) -> Stream {
    let mut stream = stream.clone();
    stream.index = index;
    for (key, value) in [("title", &track.title), ("language", &track.language)] {
        if let Some(value) = value {
            stream
                .tags
                .retain(|existing, _| !existing.eq_ignore_ascii_case(key));
            if !value.is_empty() {
                stream.tags.insert(key.into(), value.clone());
            }
        }
    }
    for (flag, value) in [("default", track.default), ("forced", track.forced)] {
        if let Some(value) = value {
            stream.disposition.insert(flag.into(), u32::from(value));
        }
    }
    stream
}

fn expected_document(request: &MuxRequest, inputs: &[Input]) -> Result<Document, AppError> {
    let by_id: BTreeMap<_, _> = inputs
        .iter()
        .map(|input| (input.id.as_str(), input))
        .collect();
    let mut streams = Vec::new();
    let mut durations = Vec::new();
    for (index, track) in request.tracks.iter().enumerate() {
        let input = by_id[track.source_id.as_str()];
        let stream = input
            .document
            .streams
            .iter()
            .find(|stream| stream.index == track.stream_index)
            .ok_or_else(|| {
                invalid("A selected stream no longer exists. Import the source again.")
            })?;
        if stream.codec_type.as_deref() != Some("attachment")
            && let Some(duration) = input.document.selected_duration(&[stream])
        {
            durations.push(duration);
        }
        streams.push(mapped_stream(stream, track, index as u32));
    }
    let mut format = by_id[request.metadata_source_id.as_str()]
        .document
        .format
        .clone();
    if let Some(format) = &mut format {
        format.duration = durations
            .into_iter()
            .reduce(f64::max)
            .map(|n| n.to_string());
    }
    let document = Document {
        streams,
        format,
        chapters: request
            .chapters_source_id
            .as_ref()
            .map(|id| by_id[id.as_str()].document.chapters.clone())
            .unwrap_or_default(),
    };
    document.selected(&(0..request.tracks.len() as u32).collect::<Vec<_>>())?;
    Ok(document)
}

fn mux_arguments(
    request: &MuxRequest,
    inputs: &[Input],
    expected: &Document,
    output: &Path,
) -> Vec<OsString> {
    let indices: BTreeMap<_, _> = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| (input.id.as_str(), index))
        .collect();
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "warning",
        "-nostats",
        "-progress",
        "pipe:1",
        "-copyts",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    for input in inputs {
        args.extend([
            "-protocol_whitelist".into(),
            "file".into(),
            "-i".into(),
            input.source.path.as_os_str().to_owned(),
        ]);
    }
    for track in &request.tracks {
        args.extend([
            "-map".into(),
            format!(
                "{}:{}",
                indices[track.source_id.as_str()],
                track.stream_index
            )
            .into(),
        ]);
    }
    args.extend([
        "-map_metadata".into(),
        indices[request.metadata_source_id.as_str()]
            .to_string()
            .into(),
        "-map_chapters".into(),
        request
            .chapters_source_id
            .as_ref()
            .map(|id| indices[id.as_str()].to_string())
            .unwrap_or_else(|| "-1".into())
            .into(),
        "-c".into(),
        "copy".into(),
        "-avoid_negative_ts".into(),
        "disabled".into(),
    ]);
    for (index, (track, stream)) in request.tracks.iter().zip(&expected.streams).enumerate() {
        for (key, value) in [("title", &track.title), ("language", &track.language)] {
            if let Some(value) = value {
                args.extend([
                    format!("-metadata:s:{index}").into(),
                    format!("{key}={value}").into(),
                ]);
            }
        }
        let flags = stream
            .disposition
            .iter()
            .filter(|(_, value)| **value != 0)
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>()
            .join("+");
        args.extend([
            format!("-disposition:{index}").into(),
            if flags.is_empty() {
                "0".into()
            } else {
                flags.into()
            },
        ]);
    }
    args.extend([
        "-f".into(),
        "matroska".into(),
        "-y".into(),
        output.as_os_str().to_owned(),
    ]);
    args
}

impl JobManager {
    pub async fn start_mux(&self, request: MuxRequest) -> Result<JobSnapshot, AppError> {
        let summary = summary(&request)?;
        self.admit_job(summary, None, Some(request), false).await
    }

    pub(super) async fn mux(
        &self,
        id: &str,
        request: &MuxRequest,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        temporary: &mut Option<Temporary>,
        scratch: &mut Vec<Temporary>,
    ) -> Result<(), AppError> {
        summary(request)?;
        check_cancel(cancel)?;
        self.phase(
            id,
            JobState::Preparing,
            "Checking all source files, selected tracks and the output destination.",
        )
        .await;
        let output_key = crate::batch::path_key(Path::new(&request.output_path));
        if request
            .sources
            .iter()
            .any(|source| crate::batch::path_key(Path::new(&source.input_path)) == output_key)
        {
            return Err(files::error(
                "SOURCE_OUTPUT_COLLISION",
                "The destination must differ from every source file.",
                Path::new(&request.output_path),
            ));
        }
        let ffmpeg = discover("ffmpeg", cancel).await?;
        let ffprobe = discover("ffprobe", cancel).await?;
        let mut inputs = Vec::new();
        let mut paths = HashSet::new();
        let mut output = None;
        for source in &request.sources {
            check_cancel(cancel)?;
            let path = source.input_path.clone();
            let guard = tokio::task::spawn_blocking(move || Source::open(Path::new(&path)))
                .await
                .map_err(|cause| AppError::new("PREFLIGHT_FAILED", cause.to_string(), None))??;
            if !paths.insert(crate::batch::path_key(&guard.path)) {
                return Err(invalid(
                    "The same canonical source file was added more than once.",
                ));
            }
            let check = RemuxRequest {
                input_path: source.input_path.clone(),
                output_path: request.output_path.clone(),
                stream_indices: vec![0],
            };
            output = Some(files::output_path(&check, &guard)?);
            let selected = request
                .tracks
                .iter()
                .filter(|track| track.source_id == source.id)
                .map(|track| track.stream_index)
                .collect::<Vec<_>>();
            let document = probe(&ffprobe, &guard.path, cancel, Some(&selected)).await?;
            guard.verify()?;
            inputs.push(Input {
                id: source.id.clone(),
                source: guard,
                document,
            });
        }
        let output = output.expect("validated sources");
        let expected = expected_document(request, &inputs)?;
        let selected = expected.streams.iter().collect::<Vec<_>>();
        super::container::preflight(&output, &expected, &selected, None)?;
        self.change(id, |snapshot| {
            snapshot.duration_seconds = expected.selected_duration(&selected)
        })
        .await;
        for input in &inputs {
            input.source.verify()?;
        }
        tokio::fs::create_dir_all(self.log_dir.as_ref())
            .await
            .map_err(|cause| {
                files::error(
                    "LOG_CREATE_FAILED",
                    cause.to_string(),
                    self.log_dir.as_ref(),
                )
            })?;
        check_cancel(cancel)?;
        *temporary = Some(Temporary::create(&output, id)?);
        let temporary = temporary.as_ref().expect("reserved output");
        let spec = CommandSpec {
            executable: ffmpeg,
            args: mux_arguments(request, &inputs, &expected, &temporary.path),
            cwd: None,
        };
        self.phase(
            id,
            JobState::Running,
            "Copying the ordered tracks from all selected source files.",
        )
        .await;
        let (sender, mut receiver) = mpsc::channel(256);
        let manager = self.clone();
        let event_id = id.to_owned();
        let events = tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                manager
                    .change(&event_id, |snapshot| match event {
                        ProcessEvent::Stdout(line) => {
                            if let Some(seconds) = progress_seconds(&line) {
                                snapshot.progress_seconds = Some(seconds);
                            }
                        }
                        ProcessEvent::Stderr(line) => append_log(snapshot, line),
                    })
                    .await;
            }
        });
        let result = supervisor::run(
            &spec,
            cancel.clone(),
            sender,
            log_path,
            Duration::from_secs(86400),
        )
        .await;
        let _ = events.await;
        let result = result.map_err(|cause| process_error(cause, &output))?;
        check_cancel(cancel)?;
        if !result.status.success() {
            return Err(files::error(
                "REMUX_FAILED",
                format!(
                    "FFmpeg failed ({}). Source files were preserved; see the job log.",
                    result.status
                ),
                &output,
            ));
        }
        self.phase(id, JobState::Finalizing, "Verifying all mapped tracks, packet contents and timing, metadata, chapters and attachments.").await;
        temporary.flush_nonempty_async().await?;
        let artifact = probe(&ffprobe, &temporary.path, cancel, None).await?;
        metadata::verify(&expected, &selected, &artifact)?;
        // The general preservation check accepts additional tags. Explicit
        // clears must also prove that the old title/language is absent.
        for (track, actual) in request.tracks.iter().zip(&artifact.streams) {
            for (key, value) in [("title", &track.title), ("language", &track.language)] {
                if let Some(value) = value {
                    let actual = actual
                        .tags
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case(key))
                        .map(|(_, value)| value.as_str())
                        .filter(|value| !value.is_empty());
                    let expected = (!value.is_empty()).then_some(value.as_str());
                    if actual != expected {
                        return Err(files::error(
                            "OUTPUT_VALIDATION_FAILED",
                            "An explicit track title or language change was not retained.",
                            &temporary.path,
                        ));
                    }
                }
            }
        }
        for (index, track) in request.tracks.iter().enumerate() {
            if expected.streams[index].codec_type.as_deref() == Some("attachment") {
                continue;
            }
            let input = inputs
                .iter()
                .find(|input| input.id == track.source_id)
                .expect("validated source");
            verify_packets(
                &ffprobe,
                &input.source.path,
                track.stream_index,
                &temporary.path,
                artifact.streams[index].index,
                cancel,
            )
            .await?;
        }
        let converted =
            super::container::prepare(temporary, &output, id, cancel, None, scratch).await?;
        let temporary = converted.unwrap_or(temporary);
        let mut state = self.state.lock().await;
        check_cancel(cancel)?;
        for input in &inputs {
            input.source.verify()?;
        }
        temporary.publish(&output)?;
        if let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.snapshot.id == id)
        {
            entry.snapshot.state = JobState::Succeeded;
            entry.snapshot.progress_seconds = entry.snapshot.duration_seconds;
            append_log(
                &mut entry.snapshot,
                "Verified multi-source output published. All source files were preserved.".into(),
            );
        }
        let _ = self.persist(&mut state).await;
        Ok(())
    }
}

#[derive(Debug)]
struct Packet {
    pts: Option<f64>,
    dts: Option<f64>,
    duration: Option<f64>,
    size: u64,
    hash: String,
}

fn timing_matches(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 0.002,
        (None, None) => true,
        _ => false,
    }
}

fn packet_duration_matches(
    expected: Option<f64>,
    actual: Option<f64>,
    actual_dts: Option<f64>,
    next_actual_dts: Option<f64>,
    allow_aac_reshape: bool,
) -> bool {
    if expected.is_none() || actual.is_none() || timing_matches(expected, actual) {
        return true;
    }
    // ISO BMFF stores one duration table for consecutive AAC samples. When an
    // imported timeline contains a small gap, FFmpeg represents that gap by
    // extending the preceding sample to the next unchanged DTS. Exact packet
    // bytes and every PTS/DTS still have to pass the normal comparison, and the
    // decoded-audio scan independently bounds the complete sample timeline.
    allow_aac_reshape
        && actual_dts
            .zip(next_actual_dts)
            .filter(|(current, next)| next >= current)
            .map(|(current, next)| next - current)
            .is_some_and(|dts_delta| timing_matches(actual, Some(dts_delta)))
}

fn packets(
    reader: &mut dyn Read,
    mut accept: impl FnMut(Packet) -> Result<(), String>,
) -> Result<u64, String> {
    let mut reader = BufReader::new(reader);
    let mut record = Vec::new();
    let mut count = 0;
    loop {
        record.clear();
        let length = reader
            .by_ref()
            .take(8193)
            .read_until(b'\n', &mut record)
            .map_err(|cause| cause.to_string())?;
        if length == 0 {
            return Ok(count);
        }
        if length > 8192 {
            return Err("A packet record exceeds its bounded size.".into());
        }
        let text = std::str::from_utf8(&record)
            .map_err(|cause| cause.to_string())?
            .trim();
        if text.is_empty() {
            continue;
        }
        let fields = text
            .split('|')
            .filter_map(|field| field.split_once('='))
            .collect::<BTreeMap<_, _>>();
        let timestamp = |key| -> Result<Option<f64>, String> {
            match fields.get(key).copied() {
                None | Some("N/A") => Ok(None),
                Some(text) => text
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .map(Some)
                    .ok_or_else(|| "A packet timestamp is invalid.".into()),
            }
        };
        let size = fields
            .get("size")
            .and_then(|text| text.parse().ok())
            .ok_or("A packet size is missing.")?;
        let hash = fields
            .get("data_hash")
            .filter(|hash| {
                hash.starts_with("SHA256:")
                    && hash.len() == 71
                    && hash[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or("A packet content hash is missing.")?
            .to_string();
        accept(Packet {
            pts: timestamp("pts_time")?,
            dts: timestamp("dts_time")?,
            duration: timestamp("duration_time")?,
            size,
            hash,
        })?;
        count += 1;
    }
}

fn packet_spec(ffprobe: &Path, input: &Path, index: u32) -> CommandSpec {
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
    args.extend([index.to_string().into()]);
    args.extend(
        [
            "-show_packets",
            "-show_data_hash",
            "sha256",
            "-show_entries",
            "packet=pts_time,dts_time,duration_time,size,data_hash:packet_side_data=",
            "-of",
            "compact=p=0:nk=0",
            "-i",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args.push(input.as_os_str().to_owned());
    CommandSpec {
        executable: ffprobe.to_owned(),
        args,
        cwd: None,
    }
}

pub(super) async fn verify_packets(
    ffprobe: &Path,
    source: &Path,
    source_index: u32,
    output: &Path,
    output_index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    compare_packets(
        ffprobe,
        source,
        source_index,
        output,
        output_index,
        cancel,
        false,
        false,
    )
    .await
    .map(|_| ())
}

pub(super) async fn verify_container_packets(
    ffprobe: &Path,
    source: &Path,
    source_index: u32,
    output: &Path,
    output_index: u32,
    allow_aac_duration_reshape: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<f64>, AppError> {
    compare_packets(
        ffprobe,
        source,
        source_index,
        output,
        output_index,
        cancel,
        true,
        allow_aac_duration_reshape,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn compare_packets(
    ffprobe: &Path,
    source: &Path,
    source_index: u32,
    output: &Path,
    output_index: u32,
    cancel: &watch::Receiver<bool>,
    reconstruct_initial_dts: bool,
    allow_aac_duration_reshape: bool,
) -> Result<Option<f64>, AppError> {
    let source_spec = packet_spec(ffprobe, source, source_index);
    let output_spec = packet_spec(ffprobe, output, output_index);
    let (sender, receiver) = blocking_channel::sync_channel(32);
    let original = supervisor::run_streaming_stdout(
        &source_spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            let mut last_duration = None;
            let count = packets(reader, |packet| {
                last_duration = packet.duration;
                sender
                    .send(packet)
                    .map_err(|_| "Output comparison stopped.".into())
            })?;
            Ok((count, last_duration))
        },
    );
    let compare = supervisor::run_streaming_stdout(
        &output_spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            let mut missing_initial_dts = 0;
            let mut source_dts_seen = false;
            let mut previous_dts = None;
            let mut pending_duration = None;
            let count = packets(reader, |actual| {
                let expected = receiver
                    .recv()
                    .map_err(|_| "The output contains extra packets.".to_string())?;
                // Matroska omits leading decode timestamps for reordered video;
                // MP4 must reconstruct them. Only a bounded leading prefix may
                // gain DTS, with monotonic decode time at or before its PTS.
                let reconstructed = reconstruct_initial_dts
                    && expected.dts.is_none()
                    && !source_dts_seen
                    && missing_initial_dts < 16
                    && actual.dts.zip(actual.pts).is_some_and(|(dts, pts)| {
                        dts <= pts && previous_dts.is_none_or(|previous| dts >= previous)
                    });
                source_dts_seen |= expected.dts.is_some();
                if reconstructed {
                    missing_initial_dts += 1;
                }
                if expected.hash != actual.hash
                    || expected.size != actual.size
                    || !timing_matches(expected.pts, actual.pts)
                    || (!reconstructed && !timing_matches(expected.dts, actual.dts))
                {
                    return Err("A copied packet's content or timing changed.".into());
                }
                if let Some((expected_duration, actual_duration, actual_dts)) =
                    pending_duration.take()
                    && !packet_duration_matches(
                        expected_duration,
                        actual_duration,
                        actual_dts,
                        actual.dts,
                        allow_aac_duration_reshape,
                    )
                {
                    return Err("A copied packet's duration changed unexpectedly.".into());
                }
                pending_duration = Some((expected.duration, actual.duration, actual.dts));
                previous_dts = actual.dts;
                Ok(())
            })?;
            if let Some((expected_duration, actual_duration, actual_dts)) = pending_duration
                && !packet_duration_matches(
                    expected_duration,
                    actual_duration,
                    actual_dts,
                    None,
                    allow_aac_duration_reshape,
                )
            {
                return Err("The final copied packet's duration changed.".into());
            }
            if receiver.recv().is_ok() {
                return Err("The output is missing source packets.".into());
            }
            Ok(count)
        },
    );
    let (original, compare) = tokio::join!(original, compare);
    let compare = compare.map_err(|cause| process_error(cause, output))?;
    let original = original.map_err(|cause| process_error(cause, source))?;
    if !original.status.success()
        || !compare.status.success()
        || !original.stderr.is_empty()
        || !compare.stderr.is_empty()
        || original.value.0 != compare.value
    {
        return Err(AppError::new(
            "OUTPUT_VALIDATION_FAILED",
            "A complete copied-packet comparison failed.",
            Some(output.to_string_lossy().into_owned()),
        ));
    }
    check_cancel(cancel)?;
    Ok(original.value.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_core::MuxSource;

    fn request() -> MuxRequest {
        let base = std::env::temp_dir();
        MuxRequest {
            sources: vec![MuxSource {
                id: "a".into(),
                input_path: base.join("source.mkv").to_string_lossy().into_owned(),
            }],
            tracks: vec![MuxTrack {
                source_id: "a".into(),
                stream_index: 7,
                title: None,
                language: None,
                default: None,
                forced: None,
            }],
            metadata_source_id: "a".into(),
            chapters_source_id: Some("a".into()),
            output_path: base.join("output.mkv").to_string_lossy().into_owned(),
        }
    }

    #[test]
    fn source_identity_and_track_mapping_are_validated_before_admission() {
        let mut request = request();
        assert_eq!(summary(&request).unwrap().stream_indices, vec![7]);
        request.sources.push(request.sources[0].clone());
        assert!(summary(&request).is_err());
        request.sources.pop();
        request.tracks.push(request.tracks[0].clone());
        assert!(summary(&request).is_err());
        request.tracks.pop();
        request.tracks[0].source_id = "removed".into();
        assert!(summary(&request).is_err());
        request.tracks[0].source_id = "a".into();
        request.tracks[0].language = Some("spa".into());
        assert!(summary(&request).is_ok());
        request.tracks[0].language = Some("spa\0argument".into());
        assert!(summary(&request).is_err());
    }

    #[test]
    fn track_overrides_preserve_other_tags_and_dispositions() {
        let source: Stream = serde_json::from_value(serde_json::json!({"index":7,"codec_type":"audio","codec_name":"flac","tags":{"TITLE":"Old","language":"eng","comment":"Keep"},"disposition":{"default":1,"forced":0,"hearing_impaired":1}})).unwrap();
        let mut track = request().tracks.remove(0);
        track.title = Some(String::new());
        track.language = Some("spa".into());
        track.default = Some(false);
        track.forced = Some(true);
        let mapped = mapped_stream(&source, &track, 0);
        assert!(
            !mapped
                .tags
                .keys()
                .any(|key| key.eq_ignore_ascii_case("title"))
        );
        assert_eq!(mapped.tags["language"], "spa");
        assert_eq!(mapped.tags["comment"], "Keep");
        assert_eq!(mapped.disposition["default"], 0);
        assert_eq!(mapped.disposition["forced"], 1);
        assert_eq!(mapped.disposition["hearing_impaired"], 1);
        assert_eq!(source.tags["TITLE"], "Old");
    }

    #[test]
    fn packet_parser_rejects_unbounded_and_unverifiable_records() {
        let row = format!(
            "pts_time=-0.002|dts_time=-0.002|duration_time=0.042|size=64|data_hash=SHA256:{}\n",
            "a".repeat(64)
        );
        assert_eq!(
            packets(&mut row.as_bytes(), |packet| {
                assert_eq!(packet.size, 64);
                assert_eq!(packet.pts, Some(-0.002));
                Ok(())
            })
            .unwrap(),
            1
        );
        assert!(packets(&mut b"size=32\n".as_slice(), |_| Ok(())).is_err());
        assert!(packets(&mut vec![b'x'; 8194].as_slice(), |_| Ok(())).is_err());
    }

    #[test]
    fn aac_duration_reshape_requires_the_next_unchanged_decode_timestamp() {
        assert!(packet_duration_matches(
            Some(0.023),
            Some(0.026508),
            Some(44.976485),
            Some(45.002993),
            true,
        ));
        assert!(!packet_duration_matches(
            Some(0.023),
            Some(0.026508),
            Some(44.976485),
            Some(44.999000),
            true,
        ));
        assert!(!packet_duration_matches(
            Some(0.023),
            Some(0.026508),
            Some(44.976485),
            Some(45.002993),
            false,
        ));
        assert!(!packet_duration_matches(
            Some(0.023),
            Some(0.026508),
            Some(44.976485),
            None,
            true,
        ));
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg and FFprobe"]
    async fn actual_packet_comparison_rejects_altered_payload_missing_tail_and_shifted_timing() {
        let path = std::env::temp_dir().join(format!(
            "jesses-packet-compare-{}-{}",
            std::process::id(),
            super::super::NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let (owner, cancel) = watch::channel(false);
        let ffmpeg = discover("ffmpeg", &cancel).await.unwrap();
        let ffprobe = discover("ffprobe", &cancel).await.unwrap();
        let original = path.join("original.mkv");
        let changed = path.join("changed.mkv");
        let shorter = path.join("shorter.mkv");
        let shifted = path.join("shifted.mkv");
        let run = |args: Vec<OsString>| {
            let executable = ffmpeg.clone();
            let cancel = cancel.clone();
            async move {
                let result = supervisor::run_capture(
                    &CommandSpec {
                        executable,
                        args,
                        cwd: None,
                    },
                    cancel,
                    64 * 1024,
                    Duration::from_secs(15),
                )
                .await
                .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
            }
        };
        for (destination, color) in [(&original, "red"), (&changed, "blue")] {
            let mut args = ["-v", "error", "-nostdin", "-f", "lavfi", "-i"]
                .map(OsString::from)
                .to_vec();
            args.push(format!("color=c={color}:s=128x80:r=24:d=2").into());
            args.extend(["-c:v", "ffv1"].map(OsString::from));
            args.push(destination.as_os_str().to_owned());
            run(args).await;
        }
        let mut args = ["-v", "error", "-nostdin", "-i"]
            .map(OsString::from)
            .to_vec();
        args.push(original.as_os_str().to_owned());
        args.extend(["-c", "copy", "-frames:v", "47"].map(OsString::from));
        args.push(shorter.as_os_str().to_owned());
        run(args).await;
        let mut args = [
            "-v",
            "error",
            "-nostdin",
            "-copyts",
            "-itsoffset",
            "0.250",
            "-i",
        ]
        .map(OsString::from)
        .to_vec();
        args.push(original.as_os_str().to_owned());
        args.extend(["-c", "copy", "-avoid_negative_ts", "disabled"].map(OsString::from));
        args.push(shifted.as_os_str().to_owned());
        run(args).await;
        verify_packets(&ffprobe, &original, 0, &original, 0, &cancel)
            .await
            .unwrap();
        for artifact in [&changed, &shorter, &shifted] {
            assert!(
                tokio::time::timeout(
                    Duration::from_secs(10),
                    verify_packets(&ffprobe, &original, 0, artifact, 0, &cancel)
                )
                .await
                .unwrap()
                .is_err(),
                "incorrectly accepted {}",
                artifact.display()
            );
        }
        owner.send_replace(true);
        assert_eq!(
            verify_packets(&ffprobe, &original, 0, &original, 0, &cancel)
                .await
                .unwrap_err()
                .code,
            "JOB_CANCELED"
        );
        std::fs::remove_dir_all(path).unwrap();
    }
}
