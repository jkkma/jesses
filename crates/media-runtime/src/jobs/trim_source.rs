//! Source direct-encoder copied audio/subtitle trim. Each copied source is
//! input-seeked; the encoded picture remains at its already cut clock.
use super::super::{
    external_tracks::ExternalTracks,
    files::Temporary,
    metadata::{Document, Stream},
    process_error,
};
use super::{Interval, unsupported};
use crate::supervisor::{self, CommandSpec};
use media_core::{AppError, EncodeSettings, SubtitleMode};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::watch;
#[path = "trim_packets.rs"]
mod packets;
struct Track {
    index: u32,
    path: PathBuf,
    source_index: u32,
    clock: f64,
    offset: f64,
    subtitle: bool,
    conversion: Option<&'static str>,
}
pub(super) struct SourceTrim {
    tracks: Vec<Track>,
    reference: PathBuf,
    seek: f64,
    duration: f64,
    origin_hint: f64,
}
impl SourceTrim {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn build(
        settings: &EncodeSettings,
        interval: Interval,
        source: &Document,
        selected: &[&Stream],
        external: &ExternalTracks,
        ffmpeg: &Path,
        input: &Path,
        output: &Path,
        id: &str,
        scratch: &mut Vec<Temporary>,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Option<Self>, AppError> {
        if !settings.source_trim || settings.backend != media_core::EncodeBackend::Standalone {
            return Ok(None);
        }
        let (seek, duration) = window(settings, interval, source)?;
        let mut tracks = Vec::new();
        for stream in selected {
            let mode = settings
                .subtitles
                .iter()
                .find(|track| track.stream_index == stream.index)
                .map_or(SubtitleMode::Copy, |track| track.mode);
            let copy = match stream.codec_type.as_deref() {
                Some("audio") => !super::super::audio::converted(settings)
                    .any(|track| track.stream_index == stream.index),
                Some("subtitle") => mode != SubtitleMode::BurnIn,
                _ => false,
            };
            if !copy {
                continue;
            }
            let (path, document, source_index) = external
                .source_track(stream.index)
                .map_or((input, source, stream.index), |(path, document, index)| {
                    (path, document, index)
                });
            let start = document
                .format
                .as_ref()
                .and_then(|format| format.start_time.as_deref())
                .and_then(|time| time.parse::<f64>().ok())
                .unwrap_or(0.0);
            let offset = external.offset_seconds(stream.index);
            tracks.push(Track {
                index: stream.index,
                path: path.to_owned(),
                source_index,
                clock: start + seek - offset,
                offset,
                subtitle: stream.codec_type.as_deref() == Some("subtitle"),
                conversion: if stream.codec_type.as_deref() == Some("subtitle") {
                    match mode {
                        SubtitleMode::SubRip => Some("subrip"),
                        SubtitleMode::WebVtt => Some("webvtt"),
                        SubtitleMode::MovText => Some("mov_text"),
                        SubtitleMode::Ass => Some("ass"),
                        _ => None,
                    }
                } else {
                    None
                },
            });
        }
        if tracks.is_empty() {
            return Ok(None);
        }
        let extension = if tracks.iter().any(|track| {
            selected.iter().any(|stream| {
                stream.index == track.index
                    && (track.conversion == Some("mov_text")
                        || (track.conversion.is_none()
                            && stream.codec_name.as_deref() == Some("mov_text")))
            })
        }) {
            if super::super::container::source_iso_format(output, selected, settings)
                .unwrap_or_else(|| {
                    super::super::container::staging_format(selected, Some(settings))
                })
                == media_core::ContainerFormat::Mp4
            {
                "mp4"
            } else {
                "mov"
            }
        } else {
            "mkv"
        };
        let asset =
            Temporary::create_extension(output, &format!("{id}-source-trim-reference"), extension)?;
        let reference = asset.path.clone();
        scratch.push(asset);
        let mut result = Self {
            tracks,
            reference,
            seek,
            duration,
            origin_hint: 0.0,
        };
        let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-v", "error"]
            .into_iter()
            .map(Into::into)
            .collect();
        result.append_inputs(&mut args);
        for (position, track) in result.tracks.iter().enumerate() {
            args.extend([
                "-map".into(),
                format!("{position}:{}", track.source_index).into(),
            ]);
        }
        args.extend(["-c".into(), "copy".into()]);
        for (index, track) in result.tracks.iter().enumerate() {
            if let Some(codec) = track.conversion {
                args.extend([format!("-c:{index}").into(), codec.into()]);
            }
        }
        args.extend([
            "-map_metadata".into(),
            "-1".into(),
            "-map_chapters".into(),
            "-1".into(),
            "-t".into(),
            format!("{duration:.3}").into(),
            "-y".into(),
            result.reference.as_os_str().to_owned(),
        ]);
        let run = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            1024 * 1024,
            Duration::from_secs(3600),
        )
        .await
        .map_err(|error| process_error(error, input))?;
        if !run.status.success() {
            return Err(unsupported(format!(
                "The source copied-track trim reference failed: {}",
                String::from_utf8_lossy(&run.stderr)
            )));
        }
        scratch
            .last()
            .expect("owned trim reference")
            .flush_nonempty_async()
            .await?;
        if let Some((index, track)) = result
            .tracks
            .iter()
            .enumerate()
            .find(|(_, track)| !track.subtitle)
        {
            let ffprobe = super::super::discover("ffprobe", cancel).await?;
            result.origin_hint = packets::infer_origin(
                &ffprobe,
                &track.path,
                track.source_index,
                &result.reference,
                index as u32,
                track.clock,
                cancel,
            )
            .await?;
        }
        Ok(Some(result))
    }
    pub(super) fn origin_hint(&self) -> f64 {
        self.origin_hint
    }
    fn append_inputs(&self, args: &mut Vec<OsString>) {
        for track in &self.tracks {
            args.extend(["-protocol_whitelist".into(), "file".into()]);
            args.extend(["-ss".into(), format!("{:.3}", self.seek).into()]);
            if track.offset != 0.0 {
                args.extend(["-itsoffset".into(), format!("{:.9}", track.offset).into()]);
            }
            args.extend(["-i".into(), track.path.as_os_str().to_owned()]);
        }
    }
    pub(super) fn audio_origin(&self, video_origin: f64) -> f64 {
        video_origin
    }
    pub(super) fn indices(&self) -> Vec<u32> {
        self.tracks.iter().map(|track| track.index).collect()
    }
    pub(super) fn apply(&self, args: &mut Vec<OsString>, selected: &[&Stream]) {
        let input = args.iter().filter(|argument| *argument == "-i").count();
        let map = args
            .iter()
            .position(|argument| argument == "-map")
            .expect("mapped output");
        let mut inputs = Vec::new();
        self.append_inputs(&mut inputs);
        args.splice(map..map, inputs);
        let mut position = 0;
        for index in 0..args.len() - 1 {
            if args[index] == "-map" {
                if let Some((track_index, track)) =
                    self.tracks.iter().enumerate().find(|(_, track)| {
                        selected
                            .get(position)
                            .is_some_and(|stream| stream.index == track.index)
                    })
                {
                    args[index + 1] =
                        format!("{}:{}", input + track_index, track.source_index).into();
                }
                position += 1;
            }
        }
        args.retain(|argument| argument != "-copyts");
        while let Some(index) = args
            .iter()
            .position(|argument| argument == "-avoid_negative_ts")
        {
            args.drain(index..index + 2);
        }
        let last = args.len() - 1;
        for track in &self.tracks {
            if let Some(codec) = track.conversion {
                let position = selected
                    .iter()
                    .position(|stream| stream.index == track.index)
                    .expect("selected converted source trim");
                args.splice(last..last, [format!("-c:{position}").into(), codec.into()]);
            }
        }
        let last = args.len() - 1;
        args.splice(
            last..last,
            ["-t".into(), format!("{:.3}", self.duration).into()],
        );
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn verify(
        &self,
        ffmpeg: &Path,
        ffprobe: &Path,
        output: &Path,
        selected: &[&Stream],
        origin: f64,
        expected: &mut Document,
        encoded_video: Option<&Path>,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        if !origin.is_finite() || origin < -0.002 {
            return Err(unsupported(
                "The source trim produced an invalid common mux clock.",
            ));
        }
        let joint = if self.tracks.iter().any(|track| track.conversion.is_some()) {
            if let Some(video) = encoded_video {
                Some(
                    Box::pin(self.reference_with_video(ffmpeg, ffprobe, video, output, cancel))
                        .await?,
                )
            } else {
                None
            }
        } else {
            None
        };
        let reference = joint
            .as_ref()
            .map_or(self.reference.as_path(), |(temporary, _)| {
                temporary.path.as_path()
            });
        let reference_origin = joint
            .as_ref()
            .map_or(self.origin_hint, |(_, origin)| *origin);
        let mux_origin = origin;
        let mut max_end = 0.0_f64;
        for (reference_index, track) in self.tracks.iter().enumerate() {
            let output_index = selected
                .iter()
                .position(|stream| stream.index == track.index)
                .expect("selected copied trim");
            let evidence = if track.conversion.is_some() {
                packets::verify_converted_reference(
                    ffprobe,
                    reference,
                    reference_index as u32,
                    output,
                    output_index as u32,
                    cancel,
                )
                .await?
            } else {
                packets::verify(
                    ffprobe,
                    &track.path,
                    track.source_index,
                    reference,
                    reference_index as u32,
                    output,
                    output_index as u32,
                    track.clock,
                    mux_origin,
                    track.subtitle,
                    cancel,
                )
                .await?
            };
            if let Some(end) = evidence
                .end
                .filter(|_| track.conversion != Some("mov_text"))
            {
                max_end = max_end.max(end);
            }
            let stream = expected
                .streams
                .iter_mut()
                .find(|stream| stream.index == track.index)
                .expect("expected trim stream");
            stream.packet_start_time = evidence.first;
            stream.start_time = evidence.first.map(|time| time.to_string());
            stream.nb_read_packets = (evidence.packets > 0).then(|| evidence.packets.to_string());
            stream.duration = if track.conversion == Some("mov_text") {
                None
            } else {
                evidence.end.map(|time| time.to_string())
            };
            if let Some(codec) = track.conversion {
                stream.codec_name = Some(codec.into());
                stream.extradata_hash = None;
                stream
                    .tags
                    .retain(|key, _| !super::super::metadata::is_derived_stream_tag(key));
            }
        }
        // The independent all-copied reference captures FFmpeg's mux policy,
        // including AAC CodecDelay. The earliest packet DTS alone is not that
        // policy: a priming packet may legitimately remain negative.
        if (mux_origin - reference_origin).abs() > 0.003 {
            return Err(unsupported(format!(
                "The source trim common mux clock differs from the source-equivalent seek reference: observed {mux_origin:.6}, expected {:.6}.",
                reference_origin
            )));
        }
        for stream in &mut expected.streams {
            if stream.codec_type.as_deref() == Some("video") {
                stream.packet_start_time = Some(origin);
                stream.start_time = Some(origin.to_string());
                if let Some(end) = stream
                    .duration
                    .as_deref()
                    .and_then(|time| time.parse::<f64>().ok())
                {
                    stream.duration = Some((end + origin).to_string());
                    max_end = max_end.max(end + origin);
                }
            }
        }
        if let Some(format) = &mut expected.format {
            format.duration = Some(max_end.to_string());
        }
        Ok(())
    }

    /// A subtitle encoder can itself determine the muxer's negative-timestamp
    /// shift. Include the actual already encoded zero-origin picture, so this
    /// reference has exactly one shared clock even without a copied audio track.
    async fn reference_with_video(
        &self,
        ffmpeg: &Path,
        ffprobe: &Path,
        video: &Path,
        output: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(Temporary, f64), AppError> {
        let extension = self
            .reference
            .extension()
            .and_then(|extension| extension.to_str())
            .expect("reference extension");
        let mut temporary = Temporary::create_extension(
            output,
            &format!(
                "source-subtitle-joint-reference-{}-{}",
                std::process::id(),
                crate::jobs::NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ),
            extension,
        )?;
        let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-v", "error"]
            .into_iter()
            .map(Into::into)
            .collect();
        self.append_inputs(&mut args);
        args.extend(["-i".into(), video.as_os_str().to_owned()]);
        for (position, track) in self.tracks.iter().enumerate() {
            args.extend([
                "-map".into(),
                format!("{position}:{}", track.source_index).into(),
            ]);
        }
        args.extend([
            "-map".into(),
            format!("{}:0", self.tracks.len()).into(),
            "-c".into(),
            "copy".into(),
        ]);
        for (position, track) in self.tracks.iter().enumerate() {
            if let Some(codec) = track.conversion {
                args.extend([format!("-c:{position}").into(), codec.into()]);
            }
        }
        args.extend([
            "-map_metadata".into(),
            "-1".into(),
            "-map_chapters".into(),
            "-1".into(),
            "-t".into(),
            format!("{:.3}", self.duration).into(),
            "-max_interleave_delta".into(),
            "0".into(),
            "-y".into(),
            temporary.path.as_os_str().to_owned(),
        ]);
        temporary.close_for_path_writer()?;
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            1024 * 1024,
            Duration::from_secs(3600),
        )
        .await;
        let rebound = temporary.reopen_after_path_writer();
        let result = result.map_err(|error| process_error(error, output))?;
        rebound?;
        if !result.status.success() {
            return Err(unsupported(format!(
                "The source subtitle/video reference mux failed: {}",
                String::from_utf8_lossy(&result.stderr)
            )));
        }
        temporary.flush_nonempty_async().await?;
        let document = super::super::probe(ffprobe, &temporary.path, cancel, None).await?;
        let video = document.streams.get(self.tracks.len()).ok_or_else(|| {
            unsupported("The joint subtitle reference omitted its encoded picture.")
        })?;
        let origin = video
            .packet_start_time
            .or_else(|| {
                video
                    .start_time
                    .as_deref()
                    .and_then(|time| time.parse().ok())
            })
            .filter(|time| time.is_finite())
            .ok_or_else(|| {
                unsupported("The joint subtitle reference has no usable picture clock.")
            })?;
        Ok((temporary, origin))
    }
}
fn window(
    settings: &EncodeSettings,
    interval: Interval,
    source: &Document,
) -> Result<(f64, f64), AppError> {
    if settings.trim.and_then(|trim| trim.time).is_some() {
        return Ok((interval.start, interval.duration()));
    }
    let frame = source_frame_seconds(settings, source)?;
    // TrimSettings.TimeSpan + GetTimeString emits only whole milliseconds.
    Ok((
        ((interval.start - frame / 2.0).max(0.0) * 1000.0).floor() / 1000.0,
        ((interval.duration() + frame / 2.0) * 1000.0).floor() / 1000.0,
    ))
}

fn source_frame_seconds(settings: &EncodeSettings, source: &Document) -> Result<f64, AppError> {
    let video = source
        .streams
        .iter()
        .find(|stream| {
            stream.index == settings.video_stream_index
                && stream.codec_type.as_deref() == Some("video")
        })
        .ok_or_else(|| unsupported("Source frame trim needs the selected video rate."))?;
    let rate = video
        .avg_frame_rate
        .as_deref()
        .or(video.r_frame_rate.as_deref())
        .and_then(|rate| rate.split_once('/'))
        .and_then(|(n, d)| Some((n.parse::<f64>().ok()?, d.parse::<f64>().ok()?)))
        .filter(|(n, d)| *n > 0.0 && *d > 0.0)
        .ok_or_else(|| unsupported("Source frame trim rate is unavailable."))?;
    Ok(rate.1 / rate.0)
}
