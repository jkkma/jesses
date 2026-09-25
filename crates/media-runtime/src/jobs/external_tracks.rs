//! External tracks keep their own input identities and timestamps.
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::Path,
};

use media_core::{
    AppError, AudioCodec, AudioTrackSettings, EncodeSettings, EncodeTrackOverride,
    ExternalAudioSettings, ExternalTrack, RemuxRequest, SubtitleMode, SubtitleTrackSettings,
};
use tokio::sync::watch;

use super::{
    audio, check_cancel, container,
    files::{self, Source},
    metadata::{Document, Stream},
    mux, probe,
};

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("EXTERNAL_TRACKS_INVALID", message, None)
}

fn audio_settings(audio: &ExternalAudioSettings, index: u32) -> AudioTrackSettings {
    AudioTrackSettings {
        stream_index: index,
        codec: audio.codec,
        bitrate_kbps: audio.bitrate_kbps,
        channels: audio.channels,
        gain: audio.gain.clone(),
    }
}

fn validate_metadata(title: &Option<String>, language: &Option<String>) -> Result<(), AppError> {
    if title
        .as_ref()
        .is_some_and(|value| value.len() > 4096 || value.contains('\0'))
    {
        return Err(invalid(
            "Track titles must be at most 4096 bytes and contain no null characters.",
        ));
    }
    if language.as_ref().is_some_and(|value| {
        !value.is_empty()
            && (value.len() != 3 || !value.bytes().all(|byte| byte.is_ascii_alphabetic()))
    }) {
        return Err(invalid(
            "Use a three-letter language code, or an empty value to clear it.",
        ));
    }
    Ok(())
}

fn validate_donor(path: &str) -> Result<(), AppError> {
    if path.contains('\0') || !Path::new(path).is_absolute() {
        return Err(invalid(
            "Metadata and chapter donors require absolute local file paths.",
        ));
    }
    Ok(())
}

/// Paths bound by the recovery manifest, including sources used only for tags or chapters.
pub(super) fn additional_source_paths(settings: &EncodeSettings) -> Vec<&Path> {
    settings
        .external_tracks
        .iter()
        .map(|track| Path::new(track.input_path.as_str()))
        .chain(settings.metadata_source_path.as_deref().map(Path::new))
        .chain(settings.chapters_source_path.as_deref().map(Path::new))
        .chain(
            settings
                .mov_timecode_track
                .as_ref()
                .and_then(|track| track.input_path.as_deref())
                .map(Path::new),
        )
        .collect()
}

pub(super) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    if settings.external_tracks.len() > 100 {
        return Err(invalid("Select at most 100 tracks from other files."));
    }
    let mut tracks = HashSet::new();
    let mut paths = HashSet::new();
    for track in &settings.external_tracks {
        validate_metadata(&track.title, &track.language)?;
        if i64::from(track.offset_milliseconds).abs() > 86_400_000 {
            return Err(invalid("Track timing offsets must be within 24 hours."));
        }
        if let Some(audio) = &track.audio {
            audio::validate_settings(&EncodeSettings {
                audio: vec![audio_settings(audio, track.stream_index)],
                ..Default::default()
            })?;
        }
        let path = Path::new(&track.input_path);
        if track.input_path.contains('\0') || !path.is_absolute() {
            return Err(invalid(
                "External tracks require absolute local file paths.",
            ));
        }
        let key = crate::batch::path_key(path);
        if !tracks.insert((key.clone(), track.stream_index)) {
            return Err(invalid("An external track can be selected only once."));
        }
        paths.insert(key);
    }
    let mut primary_overrides = HashSet::new();
    for track in &settings.track_overrides {
        validate_metadata(&track.title, &track.language)?;
        if !primary_overrides.insert(track.stream_index) {
            return Err(invalid(
                "Override each selected primary track at most once.",
            ));
        }
    }
    if settings.track_order.len() > 256 {
        return Err(invalid("Order at most 256 selected tracks."));
    }
    for track in &settings.track_order {
        if let Some(path) = &track.input_path {
            validate_donor(path)?;
        }
    }
    for path in [
        settings.metadata_source_path.as_deref(),
        settings.chapters_source_path.as_deref(),
        settings
            .mov_timecode_track
            .as_ref()
            .and_then(|track| track.input_path.as_deref()),
    ]
    .into_iter()
    .flatten()
    {
        validate_donor(path)?;
    }
    if settings.mov_timecode_track.is_some()
        && (settings.trim.is_some()
            || settings.temporal.is_some_and(|temporal| {
                temporal.frame_rate.is_some()
                    || temporal.deinterlace.is_some()
                    || temporal.qtgmc.is_some()
                    || temporal.cadence_repair.is_some()
            }))
    {
        return Err(invalid(
            "MOV timecode copying requires the complete original frame interval and cadence.",
        ));
    }
    if paths.len() > 32 {
        return Err(invalid("Select tracks from at most 32 other files."));
    }
    Ok(())
}

struct Input {
    source: Source,
    document: Document,
}

struct Track {
    input: usize,
    mux_input: usize,
    original_index: u32,
    stream: Stream,
    audio: Option<AudioTrackSettings>,
    subtitle_mode: Option<SubtitleMode>,
    offset_milliseconds: i32,
    metadata: TrackMetadata,
}

#[derive(Clone, Default)]
pub(super) struct TrackMetadata {
    pub title: Option<String>,
    pub language: Option<String>,
    pub default: Option<bool>,
    pub forced: Option<bool>,
}

impl TrackMetadata {
    fn external(track: &ExternalTrack) -> Self {
        Self {
            title: track.title.clone(),
            language: track.language.clone(),
            default: track.default,
            forced: track.forced,
        }
    }

    fn primary(track: &EncodeTrackOverride) -> Self {
        Self {
            title: track.title.clone(),
            language: track.language.clone(),
            default: track.default,
            forced: track.forced,
        }
    }

    fn apply(&self, stream: &mut Stream) {
        for (key, value) in [
            ("title", &self.title),
            ("handler_name", &self.title),
            ("language", &self.language),
        ] {
            if let Some(value) = value {
                stream
                    .tags
                    .retain(|existing, _| !existing.eq_ignore_ascii_case(key));
                if !value.is_empty() {
                    stream.tags.insert(key.into(), value.clone());
                }
            }
        }
        for (flag, value) in [("default", self.default), ("forced", self.forced)] {
            if let Some(value) = value {
                stream.disposition.insert(flag.into(), u32::from(value));
            }
        }
    }
}

struct MuxInput {
    input: usize,
    offset_milliseconds: i32,
}

#[derive(Default)]
pub(super) struct ExternalTracks {
    inputs: Vec<Input>,
    mux_inputs: Vec<MuxInput>,
    tracks: Vec<Track>,
    metadata_input: Option<usize>,
    chapters_input: Option<usize>,
    timecode: Option<container::TimecodeTrack>,
}

impl ExternalTracks {
    pub async fn prepare(
        primary: &Source,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        document: &Document,
        ffprobe: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Self, AppError> {
        validate(settings)?;
        let mut result = Self::default();
        let mut by_path = HashMap::new();
        let mut by_mux_input = HashMap::new();
        let primary_key = crate::batch::path_key(&primary.path);
        let mut identities = HashSet::new();
        let mut next_index = document
            .streams
            .iter()
            .map(|stream| stream.index)
            .max()
            .unwrap_or(0);
        for track in &settings.external_tracks {
            check_cancel(cancel)?;
            let key = crate::batch::path_key(Path::new(&track.input_path));
            let input = if let Some(index) = by_path.get(&key) {
                *index
            } else {
                let path = track.input_path.clone();
                let guard = tokio::task::spawn_blocking(move || Source::open(Path::new(&path)))
                    .await
                    .map_err(|cause| invalid(cause.to_string()))??;
                let canonical_key = crate::batch::path_key(&guard.path);
                if canonical_key == primary_key || guard.same_file(primary)? {
                    return Err(invalid(
                        "Select tracks from the video source in its own track list.",
                    ));
                }
                if identities.contains(&canonical_key)
                    || result.inputs.iter().try_fold(false, |found, input| {
                        guard.same_file(&input.source).map(|same| found || same)
                    })?
                {
                    return Err(invalid(
                        "The same external source was added through more than one path. Import it once.",
                    ));
                }
                identities.insert(canonical_key);
                files::output_path(
                    &RemuxRequest {
                        input_path: track.input_path.clone(),
                        output_path: request.output_path.clone(),
                        stream_indices: vec![track.stream_index],
                    },
                    &guard,
                )?;
                let indices = settings
                    .external_tracks
                    .iter()
                    .filter(|candidate| {
                        crate::batch::path_key(Path::new(&candidate.input_path)) == key
                    })
                    .map(|candidate| candidate.stream_index)
                    .collect::<Vec<_>>();
                let document = probe(ffprobe, &guard.path, cancel, Some(&indices)).await?;
                guard.verify()?;
                let index = result.inputs.len();
                result.inputs.push(Input {
                    source: guard,
                    document,
                });
                by_path.insert(key, index);
                index
            };
            let source = &result.inputs[input];
            let stream = source
                .document
                .streams
                .iter()
                .find(|stream| stream.index == track.stream_index)
                .ok_or_else(|| {
                    invalid(format!(
                        "External stream {} no longer exists in {}. Import the file again.",
                        track.stream_index, track.input_path
                    ))
                })?;
            if !matches!(
                stream.codec_type.as_deref(),
                Some("audio" | "subtitle" | "attachment")
            ) {
                return Err(invalid(
                    "Only audio, subtitles and attachments can be added from another file. Select video in the encoding source control.",
                ));
            }
            if track.audio.is_some() && stream.codec_type.as_deref() != Some("audio") {
                return Err(invalid(
                    "Audio conversion settings require an external audio track.",
                ));
            }
            if track.subtitle_mode.is_some() && stream.codec_type.as_deref() != Some("subtitle") {
                return Err(invalid(
                    "Subtitle processing settings require an external subtitle track.",
                ));
            }
            if track.offset_milliseconds != 0 && stream.codec_type.as_deref() == Some("attachment")
            {
                return Err(invalid(
                    "Attachment tracks do not have a playback timeline to shift.",
                ));
            }
            let mut copied = stream.clone();
            next_index = next_index
                .checked_add(1)
                .ok_or_else(|| invalid("External stream indices exceed the supported range."))?;
            copied.index = next_index;
            // Copies need complete packet timing. Converted audio instead uses
            // the complete decoded sample timeline, including codec delay, in
            // encode preparation; some valid PCM inputs omit packet durations.
            if copied.codec_type.as_deref() != Some("attachment")
                && !track
                    .subtitle_mode
                    .is_some_and(|mode| mode != SubtitleMode::Copy)
                && !track
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.codec != AudioCodec::Copy)
            {
                match mux::packet_timeline(ffprobe, &source.source.path, track.stream_index, cancel)
                    .await?
                {
                    Some(timeline) => {
                        let shift = f64::from(track.offset_milliseconds) / 1000.0;
                        copied.packet_start_time = Some(timeline.min_pts + shift);
                        // The output duration check expects an absolute end.
                        // Stream duration headers can instead be a span from a
                        // nonzero start, so use the final copied packet end.
                        copied.duration = Some((timeline.max_end + shift).to_string());
                    }
                    None if copied.codec_type.as_deref() == Some("subtitle") => {
                        // Empty subtitles have no playback timeline. Matroska
                        // may retain them; other containers reject them during preflight.
                        copied.packet_start_time = None;
                        copied.start_time = None;
                        copied.duration = None;
                    }
                    None => {
                        return Err(invalid(format!(
                            "External audio stream {} has no packets to copy. Choose another track.",
                            track.stream_index,
                        )));
                    }
                }
            }
            let audio = track
                .audio
                .as_ref()
                .map(|audio| audio_settings(audio, copied.index));
            if let Some(audio) = &audio {
                audio::validate_gain_source(
                    &source.source,
                    &EncodeSettings {
                        audio: vec![audio.clone()],
                        ..Default::default()
                    },
                )?;
            }
            let mux_input = *by_mux_input
                .entry((input, track.offset_milliseconds))
                .or_insert_with(|| {
                    let index = result.mux_inputs.len();
                    result.mux_inputs.push(MuxInput {
                        input,
                        offset_milliseconds: track.offset_milliseconds,
                    });
                    index
                });
            result.tracks.push(Track {
                input,
                mux_input,
                original_index: track.stream_index,
                audio,
                subtitle_mode: track.subtitle_mode,
                offset_milliseconds: track.offset_milliseconds,
                metadata: TrackMetadata::external(track),
                stream: copied,
            });
        }
        if let Some(path) = settings.metadata_source_path.as_deref() {
            result.metadata_input = result
                .add_donor(
                    path,
                    primary,
                    request,
                    ffprobe,
                    cancel,
                    &mut by_path,
                    &mut by_mux_input,
                )
                .await?;
        }
        if let Some(path) = settings.chapters_source_path.as_deref() {
            result.chapters_input = result
                .add_donor(
                    path,
                    primary,
                    request,
                    ffprobe,
                    cancel,
                    &mut by_path,
                    &mut by_mux_input,
                )
                .await?;
        }
        if let Some(selection) = &settings.mov_timecode_track {
            if container::format(Path::new(&request.output_path))?
                != media_core::ContainerFormat::Mov
            {
                return Err(invalid(
                    "QuickTime timecode data can be copied only into a MOV output.",
                ));
            }
            let (path, source_document) = if let Some(path) = &selection.input_path {
                match result
                    .add_donor(
                        path,
                        primary,
                        request,
                        ffprobe,
                        cancel,
                        &mut by_path,
                        &mut by_mux_input,
                    )
                    .await?
                {
                    Some(slot) => {
                        let input = &result.inputs[result.mux_inputs[slot].input];
                        (input.source.path.clone(), &input.document)
                    }
                    None => (primary.path.clone(), document),
                }
            } else {
                (primary.path.clone(), document)
            };
            let mut stream = source_document
                .streams
                .iter()
                .find(|stream| stream.index == selection.stream_index)
                .ok_or_else(|| invalid("The selected MOV timecode stream no longer exists."))?
                .clone();
            if stream.codec_type.as_deref() != Some("data")
                || stream.codec_tag_string.as_deref() != Some("tmcd")
                || stream.nb_read_packets.as_deref() != Some("1")
                || !stream
                    .tags
                    .keys()
                    .any(|key| key.eq_ignore_ascii_case("timecode"))
            {
                return Err(invalid(
                    "Select a real MOV tmcd stream with one readable timecode packet.",
                ));
            }
            let timeline = mux::packet_timeline(ffprobe, &path, selection.stream_index, cancel)
                .await?
                .ok_or_else(|| invalid("The selected MOV timecode has no timed packet."))?;
            if timeline.min_pts.abs() > 0.002 {
                return Err(invalid(
                    "MOV timecode must start at the beginning of its source.",
                ));
            }
            stream.packet_start_time = Some(timeline.min_pts);
            stream.duration = Some(timeline.max_end.to_string());
            result.timecode = Some(container::TimecodeTrack {
                source: path,
                stream,
            });
        }
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_donor(
        &mut self,
        path: &str,
        primary: &Source,
        request: &RemuxRequest,
        ffprobe: &Path,
        cancel: &watch::Receiver<bool>,
        by_path: &mut HashMap<String, usize>,
        by_mux_input: &mut HashMap<(usize, i32), usize>,
    ) -> Result<Option<usize>, AppError> {
        check_cancel(cancel)?;
        let key = crate::batch::path_key(Path::new(path));
        if key == crate::batch::path_key(&primary.path) {
            return Ok(None);
        }
        let input = if let Some(index) = by_path.get(&key) {
            *index
        } else {
            let guard = Source::open(Path::new(path))?;
            if guard.same_file(primary)? {
                return Ok(None);
            }
            if self.inputs.iter().try_fold(false, |found, input| {
                guard.same_file(&input.source).map(|same| found || same)
            })? {
                return Err(invalid(
                    "The same donor source was added through more than one path. Import it once.",
                ));
            }
            files::output_path(
                &RemuxRequest {
                    input_path: path.into(),
                    output_path: request.output_path.clone(),
                    stream_indices: Vec::new(),
                },
                &guard,
            )?;
            let document = probe(ffprobe, &guard.path, cancel, None).await?;
            guard.verify()?;
            let index = self.inputs.len();
            self.inputs.push(Input {
                source: guard,
                document,
            });
            by_path.insert(key, index);
            index
        };
        Ok(Some(*by_mux_input.entry((input, 0)).or_insert_with(|| {
            let slot = self.mux_inputs.len();
            self.mux_inputs.push(MuxInput {
                input,
                offset_milliseconds: 0,
            });
            slot
        })))
    }

    pub fn is_empty(&self) -> bool {
        self.mux_inputs.is_empty()
    }

    /// Synthetic indices are private to the mux plan, never persisted in requests.
    pub fn mux_settings(&self, settings: &EncodeSettings) -> EncodeSettings {
        let mut settings = settings.clone();
        settings
            .audio
            .extend(self.tracks.iter().filter_map(|track| track.audio.clone()));
        settings
            .subtitles
            .extend(self.tracks.iter().filter_map(|track| {
                track.subtitle_mode.map(|mode| SubtitleTrackSettings {
                    stream_index: track.stream.index,
                    mode,
                })
            }));
        settings
    }

    pub fn offset_seconds(&self, index: u32) -> f64 {
        self.tracks
            .iter()
            .find(|track| track.stream.index == index)
            .map_or(0.0, |track| f64::from(track.offset_milliseconds) / 1000.0)
    }

    pub fn audio_source(&self, index: u32) -> Option<(&Path, &Stream)> {
        let (path, document, original_index) = self.source_track(index)?;
        let stream = document
            .streams
            .iter()
            .find(|stream| stream.index == original_index)
            .expect("prepared external stream");
        Some((path, stream))
    }

    /// Resolve a synthetic output index to its guarded input and original
    /// stream. The full source document retains attachment/font headers.
    pub fn source_track(&self, index: u32) -> Option<(&Path, &Document, u32)> {
        let track = self
            .tracks
            .iter()
            .find(|track| track.stream.index == index)?;
        let input = &self.inputs[track.input];
        Some((&input.source.path, &input.document, track.original_index))
    }

    pub fn verify(&self) -> Result<(), AppError> {
        for input in &self.inputs {
            input.source.verify()?;
        }
        Ok(())
    }

    pub fn extend_document(&self, primary: &Document, settings: &EncodeSettings) -> Document {
        let mut document = primary.clone();
        document.streams.extend(self.tracks.iter().map(|track| {
            let mut stream = track.stream.clone();
            track.metadata.apply(&mut stream);
            stream
        }));
        for track in &settings.track_overrides {
            if let Some(stream) = document
                .streams
                .iter_mut()
                .find(|stream| stream.index == track.stream_index)
            {
                TrackMetadata::primary(track).apply(stream);
            }
        }
        if let Some(slot) = self.metadata_input {
            document.format = self.inputs[self.mux_inputs[slot].input]
                .document
                .format
                .clone();
            if let Some(format) = &mut document.format {
                format.duration = primary
                    .format
                    .as_ref()
                    .and_then(|primary| primary.duration.clone());
            }
        }
        if let Some(slot) = self.chapters_input {
            document.chapters = self.inputs[self.mux_inputs[slot].input]
                .document
                .chapters
                .clone();
        }
        document
    }

    pub fn output_indices(
        &self,
        primary: &Document,
        indices: Vec<u32>,
        settings: &EncodeSettings,
    ) -> Result<Vec<u32>, AppError> {
        for override_track in &settings.track_overrides {
            if !indices.contains(&override_track.stream_index) {
                return Err(invalid(
                    "Primary track overrides must name a selected track.",
                ));
            }
        }
        if !settings.track_order.is_empty() {
            let expected = indices
                .iter()
                .copied()
                .chain(self.tracks.iter().map(|track| track.stream.index))
                .collect::<HashSet<_>>();
            if settings.track_order.len() != expected.len() {
                return Err(invalid(
                    "Explicit track order must list every selected track exactly once.",
                ));
            }
            let mut output = Vec::with_capacity(expected.len());
            for track in &settings.track_order {
                let index = if let Some(path) = &track.input_path {
                    let key = crate::batch::path_key(Path::new(path));
                    self.tracks
                        .iter()
                        .zip(&settings.external_tracks)
                        .find(|(candidate, selected)| {
                            candidate.original_index == track.stream_index
                                && crate::batch::path_key(Path::new(&selected.input_path)) == key
                        })
                        .map(|(candidate, _)| candidate.stream.index)
                } else {
                    indices
                        .contains(&track.stream_index)
                        .then_some(track.stream_index)
                }
                .ok_or_else(|| {
                    invalid("Explicit track order names a track outside the selected sources.")
                })?;
                if !expected.contains(&index) || output.contains(&index) {
                    return Err(invalid("Explicit track order repeats a selected track."));
                }
                output.push(index);
            }
            return Ok(output);
        }
        let attachment = |index: &u32| {
            primary
                .streams
                .iter()
                .find(|stream| stream.index == *index)
                .is_some_and(|stream| stream.codec_type.as_deref() == Some("attachment"))
        };
        Ok(indices
            .iter()
            .copied()
            .filter(|index| !attachment(index))
            .chain(
                self.tracks
                    .iter()
                    .filter(|track| track.stream.codec_type.as_deref() != Some("attachment"))
                    .map(|track| track.stream.index),
            )
            .chain(indices.iter().copied().filter(attachment))
            .chain(
                self.tracks
                    .iter()
                    .filter(|track| track.stream.codec_type.as_deref() == Some("attachment"))
                    .map(|track| track.stream.index),
            )
            .collect())
    }

    pub fn metadata_input_index(&self) -> usize {
        self.metadata_input.map_or(0, |slot| slot + 2)
    }

    pub fn chapters_input_index(&self) -> usize {
        self.chapters_input.map_or(0, |slot| slot + 2)
    }

    pub fn timecode(&self) -> Option<&container::TimecodeTrack> {
        self.timecode.as_ref()
    }

    pub fn validate_timecode_clock(
        &self,
        fps_num: u32,
        fps_den: u32,
        frames: usize,
    ) -> Result<(), AppError> {
        let Some(timecode) = &self.timecode else {
            return Ok(());
        };
        let clock = timecode
            .stream
            .avg_frame_rate
            .as_deref()
            .and_then(|rate| rate.split_once('/'))
            .and_then(|(num, den)| Some((num.parse::<u64>().ok()?, den.parse::<u64>().ok()?)))
            .filter(|(num, den)| *num > 0 && *den > 0)
            .ok_or_else(|| invalid("MOV timecode lacks a valid source frame rate."))?;
        if u128::from(fps_num) * u128::from(clock.1) != u128::from(fps_den) * u128::from(clock.0) {
            return Err(invalid(
                "MOV timecode frame rate differs from the validated video cadence.",
            ));
        }
        let expected = frames as f64 * f64::from(fps_den) / f64::from(fps_num);
        let actual = timecode
            .stream
            .duration
            .as_deref()
            .and_then(|duration| duration.parse::<f64>().ok())
            .ok_or_else(|| invalid("MOV timecode duration could not be verified."))?;
        if (actual - expected).abs() > f64::from(fps_den) / f64::from(fps_num) + 0.002 {
            return Err(invalid(
                "MOV timecode duration differs from the encoded video interval.",
            ));
        }
        Ok(())
    }

    pub fn track_metadata(&self, index: u32, settings: &EncodeSettings) -> Option<TrackMetadata> {
        self.tracks
            .iter()
            .find(|track| track.stream.index == index)
            .map(|track| track.metadata.clone())
            .or_else(|| {
                settings
                    .track_overrides
                    .iter()
                    .find(|track| track.stream_index == index)
                    .map(TrackMetadata::primary)
            })
    }

    pub fn verify_metadata_overrides(
        &self,
        settings: &EncodeSettings,
        selected: &[&Stream],
        output: &Document,
    ) -> Result<(), AppError> {
        for (source, actual) in selected.iter().zip(&output.streams) {
            if let Some(options) = self.track_metadata(source.index, settings) {
                for (key, expected) in [
                    ("title", &options.title),
                    ("handler_name", &options.title),
                    ("language", &options.language),
                ] {
                    if let Some(expected) = expected {
                        let actual = actual
                            .tags
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(key))
                            .map(|(_, value)| value.as_str())
                            .filter(|value| !value.is_empty());
                        if actual != (!expected.is_empty()).then_some(expected.as_str()) {
                            return Err(AppError::new(
                                "OUTPUT_VALIDATION_FAILED",
                                "An explicit track title or language change was not retained.",
                                None,
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Input 0 is the primary source; input 1 is the encoded video.
    pub fn append_inputs(&self, args: &mut Vec<OsString>) {
        for slot in &self.mux_inputs {
            let input = &self.inputs[slot.input];
            args.extend(["-protocol_whitelist".into(), "file".into()]);
            if slot.offset_milliseconds != 0 {
                args.extend([
                    "-itsoffset".into(),
                    format!("{:.3}", f64::from(slot.offset_milliseconds) / 1000.0).into(),
                ]);
            }
            args.extend(["-i".into(), input.source.path.as_os_str().to_owned()]);
        }
    }

    pub fn input_stream(&self, index: u32) -> (usize, u32) {
        self.tracks
            .iter()
            .find(|track| track.stream.index == index)
            .map_or((0, index), |track| {
                (track.mux_input + 2, track.original_index)
            })
    }

    pub async fn verify_packets_excluding(
        &self,
        ffprobe: &Path,
        output: &Path,
        selected: &[&Stream],
        excluded_synthetic_indices: &[u32],
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        for track in &self.tracks {
            check_cancel(cancel)?;
            if track.stream.codec_type.as_deref() == Some("attachment")
                || excluded_synthetic_indices.contains(&track.stream.index)
                || track
                    .subtitle_mode
                    .is_some_and(|mode| mode != SubtitleMode::Copy)
                || track
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.codec != AudioCodec::Copy)
            {
                continue;
            }
            let output_index = selected
                .iter()
                .position(|stream| stream.index == track.stream.index)
                .expect("resolved external output track") as u32;
            mux::verify_shifted_packets(
                ffprobe,
                &self.inputs[track.input].source.path,
                track.original_index,
                output,
                output_index,
                f64::from(track.offset_milliseconds) / 1000.0,
                cancel,
            )
            .await?;
        }
        self.verify()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_settings() -> EncodeSettings {
        EncodeSettings {
            external_tracks: vec![media_core::ExternalTrack {
                audio: None,
                offset_milliseconds: 0,
                subtitle_mode: None,
                title: None,
                language: None,
                default: None,
                forced: None,
                input_path: std::env::temp_dir()
                    .join("external.mka")
                    .to_string_lossy()
                    .into_owned(),
                stream_index: 0,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn external_tracks_reject_ambiguous_or_unbounded_selections_before_execution() {
        let mut settings = base_settings();
        validate(&settings).unwrap();
        settings
            .external_tracks
            .push(settings.external_tracks[0].clone());
        assert!(validate(&settings).is_err());
        settings.external_tracks.pop();
        settings.trim = Some(media_core::VideoTrim {
            start_frame: 0,
            end_frame_exclusive: 24,
            time: None,
        });
        validate(&settings).unwrap();
        settings.trim = None;
        settings.external_tracks[0].input_path = "relative.mka".into();
        assert!(validate(&settings).is_err());
        settings.external_tracks = (0..101)
            .map(|index| media_core::ExternalTrack {
                audio: None,
                offset_milliseconds: 0,
                subtitle_mode: None,
                title: None,
                language: None,
                default: None,
                forced: None,
                input_path: std::env::temp_dir()
                    .join("external.mka")
                    .to_string_lossy()
                    .into_owned(),
                stream_index: index,
            })
            .collect();
        assert!(validate(&settings).is_err());
        settings.external_tracks = (0..33)
            .map(|index| media_core::ExternalTrack {
                audio: None,
                offset_milliseconds: 0,
                subtitle_mode: None,
                title: None,
                language: None,
                default: None,
                forced: None,
                input_path: std::env::temp_dir()
                    .join(format!("external-{index}.mka"))
                    .to_string_lossy()
                    .into_owned(),
                stream_index: 0,
            })
            .collect();
        assert!(validate(&settings).is_err());
        settings = base_settings();
        settings.external_tracks[0].offset_milliseconds = 86_400_001;
        assert!(validate(&settings).is_err());
        settings.external_tracks[0].offset_milliseconds = -86_400_000;
        validate(&settings).unwrap();
    }

    #[test]
    fn old_settings_omit_external_mapping_and_track_objects_reject_unimplemented_controls() {
        let saved = serde_json::to_value(EncodeSettings::default()).unwrap();
        assert!(saved.get("externalTracks").is_none());
        let loaded: EncodeSettings = serde_json::from_value(saved).unwrap();
        assert!(loaded.external_tracks.is_empty());
        let mut track = serde_json::to_value(&base_settings().external_tracks[0]).unwrap();
        track["offsetSeconds"] = 2.into();
        assert!(serde_json::from_value::<media_core::ExternalTrack>(track).is_err());
    }

    #[test]
    fn external_audio_settings_are_bounded_and_cannot_override_source_identity() {
        let mut settings = base_settings();
        let mut serialized = serde_json::to_value(&settings.external_tracks[0]).unwrap();
        assert!(serialized.get("audio").is_none());
        serialized["audio"] = serde_json::json!({"codec":"aac"});
        let track: media_core::ExternalTrack = serde_json::from_value(serialized.clone()).unwrap();
        assert_eq!(track.audio.as_ref().unwrap().bitrate_kbps, 128);
        assert_eq!(
            track.audio.as_ref().unwrap().channels,
            media_core::AudioChannels::Preserve
        );
        settings.external_tracks[0] = track;
        validate(&settings).unwrap();
        settings.external_tracks[0]
            .audio
            .as_mut()
            .unwrap()
            .bitrate_kbps = 0;
        assert_eq!(
            validate(&settings).unwrap_err().code,
            "AUDIO_SETTINGS_INVALID"
        );
        for (field, value) in [
            ("streamIndex", serde_json::json!(1)),
            ("inputPath", serde_json::json!("different.mka")),
            ("offsetMilliseconds", serde_json::json!(2)),
            ("offsetSeconds", serde_json::json!(2)),
        ] {
            let mut invalid = serialized.clone();
            invalid["audio"][field] = value;
            assert!(
                serde_json::from_value::<media_core::ExternalTrack>(invalid).is_err(),
                "{field}"
            );
        }
    }
}
