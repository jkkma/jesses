//! Frame intervals are derived only after the full source has passed its CFR
//! scan. Output validation then compares the exact selected interval.
use media_core::{AppError, AudioCodec, EncodeBackend, EncodeSettings, VideoTrim};

use super::files::Temporary;
use super::metadata::{Document, Stream};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};
use tokio::sync::watch;
#[path = "trim_subtitles.rs"]
mod subtitles;
pub(super) use subtitles::{Text, read as read_subtitles};

fn unsupported(message: impl Into<String>) -> AppError {
    AppError::new("TRIM_UNSUPPORTED", message, None)
}

pub(super) fn validate_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    if let Some(trim) = settings.trim {
        if trim.end_frame_exclusive <= trim.start_frame {
            return Err(unsupported(
                "The trim end frame must be greater than its zero-based start frame; the end frame is excluded.",
            ));
        }
        if settings.backend != EncodeBackend::Standalone {
            return Err(unsupported(
                "Frame intervals currently require standalone encoding. av1an chunks cannot apply a source interval independently.",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_selection(
    selected: &[&Stream],
    settings: &EncodeSettings,
) -> Result<(), AppError> {
    if settings.trim.is_none() {
        return Ok(());
    }
    for stream in selected {
        match stream.codec_type.as_deref() {
            Some("audio")
                if !super::audio::converted(settings).any(|track| {
                    track.stream_index == stream.index && track.codec != AudioCodec::Copy
                }) =>
            {
                return Err(unsupported(format!(
                    "Audio stream #{} requires an explicit conversion when trimming. Copy cannot guarantee sample-accurate boundaries through compressed audio packets.",
                    stream.index
                )));
            }
            Some("subtitle")
                if !matches!(
                    stream.codec_name.as_deref(),
                    Some("ass" | "subrip" | "webvtt")
                ) =>
            {
                return Err(unsupported(format!(
                    "Subtitle stream #{} uses an unsupported trim format. Select ASS, SubRip or WebVTT text subtitles, or exclude this track.",
                    stream.index
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Interval {
    pub frames: usize,
    pub start: f64,
    pub end: f64,
}

impl Interval {
    pub fn build(
        trim: VideoTrim,
        source_frames: usize,
        fps_num: u32,
        fps_den: u32,
    ) -> Result<Self, AppError> {
        if trim.end_frame_exclusive <= trim.start_frame
            || u64::from(trim.end_frame_exclusive) > source_frames as u64
        {
            return Err(unsupported(format!(
                "The selected interval [{}, {}) exceeds the {} validated source frames or is empty.",
                trim.start_frame, trim.end_frame_exclusive, source_frames
            )));
        }
        let seconds = |frames: u32| f64::from(frames) * f64::from(fps_den) / f64::from(fps_num);
        Ok(Self {
            frames: (trim.end_frame_exclusive - trim.start_frame) as usize,
            start: seconds(trim.start_frame),
            end: seconds(trim.end_frame_exclusive),
        })
    }

    pub fn duration(self) -> f64 {
        self.end - self.start
    }

    pub fn expected_document(self, source: &Document) -> Result<Document, AppError> {
        let mut expected = source.clone();
        if let Some(format) = &mut expected.format {
            format.start_time = Some("0".into());
            format.duration = Some(self.duration().to_string());
        }
        expected.chapters.clear();
        for chapter in &source.chapters {
            let start = chapter
                .start_time
                .parse::<f64>()
                .map_err(|_| unsupported("Invalid source chapter start."))?;
            let end = chapter
                .end_time
                .parse::<f64>()
                .map_err(|_| unsupported("Invalid source chapter end."))?;
            if !start.is_finite() || !end.is_finite() || start < 0.0 || end < start {
                return Err(unsupported("Invalid source chapter interval."));
            }
            if end > self.start && start < self.end {
                let mut result = chapter.clone();
                result.start_time = (start.max(self.start) - self.start).to_string();
                result.end_time = (end.min(self.end) - self.start).to_string();
                expected.chapters.push(result);
            }
        }
        for stream in &mut expected.streams {
            if matches!(
                stream.codec_type.as_deref(),
                Some("video" | "audio" | "subtitle")
            ) {
                stream.start_time = Some("0".into());
                stream.duration = Some(self.duration().to_string());
                stream
                    .tags
                    .retain(|key, _| !super::metadata::is_derived_stream_tag(key));
            }
        }
        Ok(expected)
    }
}

struct Subtitle {
    source_index: u32,
    codec: String,
    path: PathBuf,
    text: subtitles::Text,
}

pub(super) struct Prepared {
    pub expected: Document,
    subtitles: Vec<Subtitle>,
    chapters: PathBuf,
}

async fn write_asset(
    output: &Path,
    id: &str,
    extension: &str,
    contents: String,
    scratch: &mut Vec<Temporary>,
) -> Result<PathBuf, AppError> {
    let temporary = Temporary::create_extension(output, id, extension)?;
    let path = temporary.path.clone();
    let mut file = temporary.clone_file()?;
    scratch.push(temporary);
    tokio::task::spawn_blocking(move || {
        file.write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
    })
    .await
    .map_err(|error| unsupported(error.to_string()))?
    .map_err(|error| unsupported(error.to_string()))?;
    Ok(path)
}

impl Prepared {
    pub(super) fn asset(&self, source_index: u32) -> Option<(&Path, &str)> {
        self.subtitles
            .iter()
            .find(|subtitle| subtitle.source_index == source_index)
            .map(|subtitle| (subtitle.path.as_path(), subtitle.text.format))
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn build(
        interval: Interval,
        source: &Document,
        selected: &[&Stream],
        ffmpeg: &Path,
        input: &Path,
        output: &Path,
        id: &str,
        scratch: &mut Vec<Temporary>,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Self, AppError> {
        let mut expected = interval.expected_document(source)?;
        let mut subtitles = Vec::new();
        for stream in selected
            .iter()
            .filter(|stream| stream.codec_type.as_deref() == Some("subtitle"))
        {
            let codec = stream
                .codec_name
                .as_deref()
                .ok_or_else(|| unsupported("Subtitle codec is missing."))?;
            let text = subtitles::read(ffmpeg, input, stream.index, codec, cancel)
                .await?
                .clipped(interval)?;
            let path = write_asset(
                output,
                &format!("{id}-trim-subtitle-{}", stream.index),
                text.format,
                text.render(),
                scratch,
            )
            .await?;
            let expected_stream = expected
                .streams
                .iter_mut()
                .find(|value| value.index == stream.index)
                .expect("source subtitle");
            expected_stream.packet_start_time = text.cues.first().map(|cue| cue.start);
            expected_stream.start_time = expected_stream
                .packet_start_time
                .map(|start| start.to_string());
            expected_stream.nb_read_packets =
                (!text.cues.is_empty()).then(|| text.cues.len().to_string());
            subtitles.push(Subtitle {
                source_index: stream.index,
                codec: codec.to_owned(),
                path,
                text,
            });
        }
        let escape = |text: &str| {
            text.replace('\\', "\\\\")
                .replace('=', "\\=")
                .replace(';', "\\;")
                .replace('#', "\\#")
                .replace('\n', "\\\n")
                .replace('\r', "")
        };
        let mut chapters = ";FFMETADATA1\n".to_owned();
        for chapter in &expected.chapters {
            let start = (chapter.start_time.parse::<f64>().unwrap() * 1_000_000.0).round() as u64;
            let end = (chapter.end_time.parse::<f64>().unwrap() * 1_000_000.0).round() as u64;
            chapters.push_str(&format!(
                "[CHAPTER]\nTIMEBASE=1/1000000\nSTART={start}\nEND={end}\n"
            ));
            for (key, value) in &chapter.tags {
                chapters.push_str(&format!("{}={}\n", escape(key), escape(value)));
            }
        }
        let chapters = write_asset(
            output,
            &format!("{id}-trim-chapters"),
            "ffmeta",
            chapters,
            scratch,
        )
        .await?;
        Ok(Self {
            expected,
            subtitles,
            chapters,
        })
    }

    pub fn apply_mux(
        &self,
        args: &mut Vec<OsString>,
        selected: &[&Stream],
        audio_filters: &BTreeMap<u32, String>,
    ) -> Result<(), AppError> {
        let mut inputs = Vec::<OsString>::new();
        for subtitle in &self.subtitles {
            inputs.extend([
                "-f".into(),
                subtitle.text.format.into(),
                "-i".into(),
                subtitle.path.as_os_str().to_owned(),
            ]);
        }
        inputs.extend([
            "-f".into(),
            "ffmetadata".into(),
            "-i".into(),
            self.chapters.as_os_str().to_owned(),
        ]);
        let first_map = args
            .iter()
            .position(|arg| arg == "-map")
            .expect("mapped encoder output");
        args.splice(first_map..first_map, inputs);
        for index in 0..args.len() - 1 {
            if args[index] == "-map" {
                if let Some(position) = self.subtitles.iter().position(|subtitle| {
                    args[index + 1] == format!("0:{}", subtitle.source_index).as_str()
                }) {
                    args[index + 1] = format!("{}:0", position + 2).into();
                }
            } else if args[index] == "-map_chapters" {
                args[index + 1] = (2 + self.subtitles.len()).to_string().into();
            }
        }
        for (stream_index, prefix) in audio_filters {
            let position = selected
                .iter()
                .position(|stream| stream.index == *stream_index)
                .expect("selected clipped audio");
            let key = format!("-filter:{position}");
            let index = args
                .iter()
                .position(|arg| arg == key.as_str())
                .ok_or_else(|| unsupported("Trim audio filter was not configured."))?;
            let filter = args[index + 1]
                .to_str()
                .filter(|value| value.starts_with("asetpts=N/SR/TB+STARTPTS"))
                .ok_or_else(|| unsupported("Unexpected audio filter order."))?;
            args[index + 1] = filter
                .replacen("asetpts=N/SR/TB+STARTPTS", prefix, 1)
                .into();
        }
        Ok(())
    }

    pub async fn verify_subtitles(
        &self,
        ffmpeg: &Path,
        output: &Path,
        selected: &[&Stream],
        overridden: &[u32],
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        for subtitle in &self.subtitles {
            if overridden.contains(&subtitle.source_index) {
                continue;
            }
            let index = selected
                .iter()
                .position(|stream| stream.index == subtitle.source_index)
                .expect("selected subtitle") as u32;
            let actual = subtitles::read(ffmpeg, output, index, &subtitle.codec, cancel).await?;
            if !subtitle.text.matches(&actual) {
                return Err(AppError::new(
                    "TRIM_VALIDATION_FAILED",
                    "Trimmed subtitle cue timing, order, text or styles changed unexpectedly.",
                    None,
                ));
            }
        }
        Ok(())
    }
}
