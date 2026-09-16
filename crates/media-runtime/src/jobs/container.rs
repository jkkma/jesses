//! The final container is built only from the already verified Matroska stage.
//! It receives a separate owned sibling and cannot publish an unverified result.
use std::{collections::HashSet, ffi::OsString, path::Path, time::Duration};

use media_core::{AppError, ContainerFormat, EncodeSettings, SubtitleMode};
use tokio::sync::watch;

use super::{
    files::Temporary,
    metadata::{self, Document, Stream},
    trim,
};
use crate::supervisor::{self, CommandSpec};

#[derive(Clone, Copy)]
pub(super) struct Cadence {
    pub index: u32,
    pub numerator: u32,
    pub denominator: u32,
    pub frames: usize,
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("CONTAINER_INCOMPATIBLE", message, None)
}

pub(super) fn format(path: &Path) -> Result<ContainerFormat, AppError> {
    path.extension()
        .and_then(|value| value.to_str())
        .and_then(ContainerFormat::from_extension)
        .ok_or_else(|| invalid("Choose a .mkv, .mp4, .mov or .webm destination."))
}

fn tag<'a>(stream: &'a Stream, key: &str) -> Option<&'a str> {
    stream
        .tags
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

fn text_codec(codec: &str) -> bool {
    matches!(codec, "ass" | "subrip" | "webvtt" | "mov_text")
}

fn compatible(container: ContainerFormat, kind: &str, codec: &str) -> bool {
    match container {
        ContainerFormat::Matroska => codec != "mov_text",
        ContainerFormat::Mp4 => match kind {
            "video" => matches!(codec, "h264" | "hevc" | "av1" | "vp9" | "mpeg4"),
            "audio" => matches!(
                codec,
                "aac" | "mp3" | "ac3" | "eac3" | "alac" | "opus" | "flac"
            ),
            "subtitle" => text_codec(codec),
            _ => false,
        },
        ContainerFormat::Mov => match kind {
            "video" => matches!(codec, "h264" | "hevc" | "mpeg4" | "prores" | "mjpeg"),
            "audio" => matches!(
                codec,
                "aac" | "mp3" | "ac3" | "eac3" | "alac" | "pcm_s16le" | "pcm_s24le" | "pcm_s32le"
            ),
            "subtitle" => text_codec(codec),
            _ => false,
        },
        ContainerFormat::Webm => match kind {
            "video" => matches!(codec, "vp8" | "vp9" | "av1"),
            "audio" => matches!(codec, "opus" | "vorbis"),
            "subtitle" => text_codec(codec),
            _ => false,
        },
    }
}

/// Called before decoding/encoding. Metadata that the selected container cannot
/// represent receives an actionable error rather than disappearing on publish.
pub(super) fn preflight(
    output: &Path,
    document: &Document,
    selected: &[&Stream],
    settings: Option<&EncodeSettings>,
) -> Result<(), AppError> {
    let container = format(output)?;
    for stream in selected {
        let subtitle = settings.and_then(|settings| {
            settings
                .subtitles
                .iter()
                .find(|track| track.stream_index == stream.index)
        });
        if subtitle.is_some_and(|track| track.mode == SubtitleMode::BurnIn) {
            continue;
        }
        let kind = stream.codec_type.as_deref().unwrap_or_default();
        if container != ContainerFormat::Matroska
            && kind == "subtitle"
            && !stream
                .nb_read_packets
                .as_deref()
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|value| value > 0)
        {
            return Err(invalid(format!(
                "Subtitle stream {} has no cues. Deselect this empty track or use Matroska to retain it.",
                stream.index
            )));
        }
        let codec = if let Some(track) = subtitle.filter(|track| track.mode != SubtitleMode::Copy) {
            match track.mode {
                SubtitleMode::Ass => "ass",
                SubtitleMode::SubRip => "subrip",
                SubtitleMode::WebVtt => "webvtt",
                _ => unreachable!("burned track excluded"),
            }
        } else if let Some(settings) =
            settings.filter(|settings| settings.video_stream_index == stream.index)
        {
            match settings.encoder {
                media_core::VideoEncoder::X264 | media_core::VideoEncoder::H264Nvenc => "h264",
                media_core::VideoEncoder::X265
                | media_core::VideoEncoder::X265Standalone
                | media_core::VideoEncoder::HevcNvenc => "hevc",
                media_core::VideoEncoder::Vp9 | media_core::VideoEncoder::VpxStandalone => "vp9",
                media_core::VideoEncoder::SvtAv1
                | media_core::VideoEncoder::SvtAv1FiveFish
                | media_core::VideoEncoder::SvtAv1Hdr
                | media_core::VideoEncoder::AomAv1 => "av1",
            }
        } else if let Some(track) = settings.and_then(|settings| {
            settings.audio.iter().find(|track| {
                track.stream_index == stream.index && track.codec != media_core::AudioCodec::Copy
            })
        }) {
            super::audio::codec_name(track.codec)
        } else {
            stream.codec_name.as_deref().unwrap_or_default()
        };
        if !compatible(container, kind, codec) {
            return Err(invalid(format!(
                "Stream {} ({kind}, {codec}) is incompatible with {}. Choose Matroska, change its encoder, or deselect that track. Attachments require Matroska.",
                stream.index,
                container.extension().to_uppercase()
            )));
        }
        // The verified staging format cannot carry tx3g. An explicit text
        // conversion in Quick Convert supplies a supported staging asset.
        if stream.codec_name.as_deref() == Some("mov_text")
            && !subtitle.is_some_and(|track| {
                matches!(
                    track.mode,
                    SubtitleMode::Ass | SubtitleMode::SubRip | SubtitleMode::WebVtt
                )
            })
        {
            return Err(invalid(format!(
                "Stream {} uses MP4 text subtitles. In Quick Convert choose SubRip, ASS or WebVTT conversion, or deselect it for stream-copy jobs.",
                stream.index
            )));
        }
        if matches!(container, ContainerFormat::Mp4 | ContainerFormat::Mov) {
            if let Some(language) = tag(stream, "language")
                && (language.len() != 3 || !language.bytes().all(|byte| byte.is_ascii_lowercase()))
            {
                return Err(invalid(format!(
                    "Stream {} needs a lowercase three-letter language code for MP4/MOV. Edit its language in multi-source mux or choose Matroska.",
                    stream.index
                )));
            }
            for key in stream.tags.keys() {
                if !metadata::is_derived_stream_tag(key)
                    && !matches!(
                        key.to_ascii_lowercase().as_str(),
                        "title" | "language" | "handler_name" | "vendor_id" | "name"
                    )
                {
                    return Err(invalid(format!(
                        "Stream {} has metadata '{key}' that this MP4/MOV workflow cannot preserve. Choose Matroska or deselect the track.",
                        stream.index
                    )));
                }
            }
            for (flag, value) in &stream.disposition {
                if *value != 0 && !matches!(flag.as_str(), "default" | "forced") {
                    return Err(invalid(format!(
                        "Stream {} disposition '{flag}' cannot be preserved by this MP4/MOV workflow. Choose Matroska.",
                        stream.index
                    )));
                }
            }
        }
    }
    if matches!(container, ContainerFormat::Mp4 | ContainerFormat::Mov)
        && document.chapters.iter().any(|chapter| {
            chapter
                .tags
                .keys()
                .any(|key| !key.eq_ignore_ascii_case("title"))
        })
    {
        return Err(invalid(
            "MP4/MOV chapters preserve titles and timing. Other chapter metadata requires Matroska.",
        ));
    }
    Ok(())
}

fn arguments(
    input: &Path,
    output: &Path,
    document: &Document,
    container: ContainerFormat,
    cadence: Option<Cadence>,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-copyts",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(input.as_os_str().to_owned());
    for stream in &document.streams {
        args.extend(["-map".into(), format!("0:{}", stream.index).into()]);
    }
    args.extend([
        "-map_metadata".into(),
        "0".into(),
        "-map_chapters".into(),
        "0".into(),
        "-c".into(),
        "copy".into(),
        "-avoid_negative_ts".into(),
        "disabled".into(),
    ]);
    for (index, stream) in document.streams.iter().enumerate() {
        let mut filters = Vec::new();
        // QuickTime's nclc box has no range bit. Retain a known source range in
        // the codec's VUI, while every coded packet remains hash-verified.
        if container == ContainerFormat::Mov
            && matches!(stream.color_range.as_deref(), Some("tv" | "pc"))
            && let Some(filter) = match stream.codec_name.as_deref() {
                Some("h264") => Some("h264_metadata"),
                Some("hevc") => Some("hevc_metadata"),
                _ => None,
            }
        {
            filters.push(format!(
                "{filter}=video_full_range_flag={}",
                u8::from(stream.color_range.as_deref() == Some("pc"))
            ));
        }
        if let Some(clock) = cadence.filter(|clock| clock.index == stream.index) {
            // Restore the independently validated frame clock after Matroska's
            // millisecond quantization. Composition timestamps keep their rank,
            // including B-frame packet reordering; packet number is never PTS.
            filters.push(format!(
                "setts=pts=round(PTS):dts=round(DTS):duration=1:time_base={}/{}:prescale=1",
                clock.denominator, clock.numerator
            ));
        }
        if !filters.is_empty() {
            args.extend([format!("-bsf:{index}").into(), filters.join(",").into()]);
        }
        if stream.codec_type.as_deref() == Some("subtitle") {
            args.extend([
                format!("-c:{index}").into(),
                if container == ContainerFormat::Webm {
                    "webvtt".into()
                } else {
                    "mov_text".into()
                },
            ]);
        }
        if matches!(container, ContainerFormat::Mp4 | ContainerFormat::Mov)
            && let Some(title) = tag(stream, "title").or_else(|| tag(stream, "handler_name"))
        {
            args.extend([
                format!("-metadata:s:{index}").into(),
                format!("handler_name={title}").into(),
            ]);
        }
        let flags = stream
            .disposition
            .iter()
            .filter(|(_, value)| **value != 0)
            .map(|(flag, _)| flag.as_str())
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
    if matches!(container, ContainerFormat::Mp4 | ContainerFormat::Mov) {
        args.extend(["-movflags".into(), "use_metadata_tags+write_colr".into()]);
    }
    args.extend([
        "-f".into(),
        container.ffmpeg_format().into(),
        "-y".into(),
        output.as_os_str().to_owned(),
    ]);
    args
}

fn normalize(
    expected: &mut Document,
    actual: &mut Document,
    container: ContainerFormat,
) -> Result<Vec<u32>, AppError> {
    let mut converted = Vec::new();
    let iso = matches!(container, ContainerFormat::Mp4 | ContainerFormat::Mov);
    if iso && !expected.chapters.is_empty() {
        let auxiliary = actual.streams.last().filter(|stream| {
            stream.codec_type.as_deref() == Some("data")
                && stream.codec_name.as_deref() == Some("bin_data")
                && stream.codec_tag_string.as_deref() == Some("text")
                && tag(stream, "handler_name") == Some("SubtitleHandler")
        });
        if actual.streams.len() != expected.streams.len() + 1 || auxiliary.is_none() {
            return Err(invalid(
                "MP4/MOV chapter track structure could not be verified.",
            ));
        }
        actual.streams.pop();
    }
    let mut first = HashSet::new();
    let kinds_with_default: HashSet<_> = expected
        .streams
        .iter()
        .filter(|stream| stream.disposition.get("default").copied().unwrap_or(0) != 0)
        .map(|stream| stream.codec_type.clone())
        .collect();
    for stream in &mut expected.streams {
        if stream.codec_type.as_deref() == Some("subtitle") {
            converted.push(stream.index);
            stream.codec_name = Some(
                if container == ContainerFormat::Webm {
                    "webvtt"
                } else {
                    "mov_text"
                }
                .into(),
            );
        }
        if iso {
            if first.insert(stream.codec_type.clone())
                && !kinds_with_default.contains(&stream.codec_type)
            {
                stream.disposition.insert("default".into(), 1);
            }
            let title = tag(stream, "title")
                .or_else(|| tag(stream, "handler_name"))
                .map(str::to_owned);
            stream.tags.retain(|key, _| {
                !matches!(
                    key.to_ascii_lowercase().as_str(),
                    "title" | "handler_name" | "vendor_id" | "name"
                )
            });
            if let Some(title) = title {
                stream.tags.insert("handler_name".into(), title);
            }
        }
    }
    Ok(converted)
}

pub(super) async fn prepare(
    input: &Temporary,
    output: &Path,
    id: &str,
    cancel: &watch::Receiver<bool>,
    cadence: Option<Cadence>,
) -> Result<Option<Temporary>, AppError> {
    let container = format(output)?;
    if container == ContainerFormat::Matroska {
        return Ok(None);
    }
    super::check_cancel(cancel)?;
    let ffmpeg = super::discover("ffmpeg", cancel).await?;
    let ffprobe = super::discover("ffprobe", cancel).await?;
    let document = super::probe(&ffprobe, &input.path, cancel, None).await?;
    preflight(
        output,
        &document,
        &document.streams.iter().collect::<Vec<_>>(),
        None,
    )?;
    let artifact =
        Temporary::create_extension(output, &format!("{id}-container"), container.extension())?;
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.clone(),
            args: arguments(&input.path, &artifact.path, &document, container, cadence),
            cwd: None,
        },
        cancel.clone(),
        256 * 1024,
        Duration::from_secs(24 * 60 * 60),
    )
    .await
    .map_err(|error| super::process_error(error, &input.path))?;
    if !result.status.success() {
        return Err(invalid(format!(
            "Final container conversion failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    artifact.flush_nonempty_async().await?;
    let mut actual = super::probe(&ffprobe, &artifact.path, cancel, None).await?;
    let mut expected = document.clone();
    let converted = normalize(&mut expected, &mut actual, container)?;
    metadata::verify_container(
        &expected,
        &expected.streams.iter().collect::<Vec<_>>(),
        &actual,
        &converted,
    )?;
    for (source, target) in document.streams.iter().zip(&actual.streams) {
        if source.codec_type.as_deref() == Some("subtitle") {
            let before = trim::read_subtitles(
                &ffmpeg,
                &input.path,
                source.index,
                source.codec_name.as_deref().unwrap_or_default(),
                cancel,
            )
            .await?;
            let after = trim::read_subtitles(
                &ffmpeg,
                &artifact.path,
                target.index,
                target.codec_name.as_deref().unwrap_or_default(),
                cancel,
            )
            .await?;
            super::subtitles::verify_conversion(&before, &after)?;
        } else {
            let last_duration = super::mux::verify_container_packets(
                &ffprobe,
                &input.path,
                source.index,
                &artifact.path,
                target.index,
                cancel,
            )
            .await?;
            if source.codec_type.as_deref() == Some("audio") {
                let before =
                    super::audio::scan(&ffprobe, &input.path, source, false, cancel).await?;
                let after =
                    super::audio::scan(&ffprobe, &artifact.path, target, true, cancel).await?;
                let tick = source
                    .time_base
                    .as_deref()
                    .and_then(|value| value.split_once('/'))
                    .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
                    .filter(|value| value.is_finite() && *value > 0.0)
                    .unwrap_or(0.001);
                before.verify_container(
                    &after,
                    tick,
                    (source.codec_name.as_deref() == Some("aac"))
                        .then_some(last_duration)
                        .flatten(),
                )?;
            }
            if source.codec_type.as_deref() == Some("video")
                && (source.pix_fmt != target.pix_fmt
                    || source.color_space != target.color_space
                    || source.color_transfer != target.color_transfer
                    || source.color_primaries != target.color_primaries
                    || source.color_range != target.color_range
                    || source.sample_aspect_ratio != target.sample_aspect_ratio)
            {
                return Err(invalid(
                    "The final container changed video pixel format, color signaling or sample aspect ratio.",
                ));
            }
        }
    }
    let decoded = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg,
            args: [
                "-v",
                "error",
                "-nostdin",
                "-xerror",
                "-err_detect",
                "explode",
                "-protocol_whitelist",
                "file",
                "-i",
            ]
            .into_iter()
            .map(OsString::from)
            .chain([artifact.path.as_os_str().to_owned()])
            .chain(
                ["-map", "0:v?", "-map", "0:a?", "-f", "null", "-"]
                    .into_iter()
                    .map(OsString::from),
            )
            .collect(),
            cwd: None,
        },
        cancel.clone(),
        256 * 1024,
        Duration::from_secs(24 * 60 * 60),
    )
    .await
    .map_err(|error| super::process_error(error, &artifact.path))?;
    if !decoded.status.success() || !decoded.stderr.is_empty() {
        return Err(invalid(
            "The complete final container did not decode without errors.",
        ));
    }
    if let Some(clock) = cadence {
        #[derive(serde::Deserialize)]
        struct Frame {
            best_effort_timestamp_time: Option<String>,
        }
        let spec = CommandSpec {
            executable: ffprobe,
            args: ["-v", "error", "-select_streams"]
                .into_iter()
                .map(OsString::from)
                .chain([clock.index.to_string().into()])
                .chain(
                    [
                        "-show_frames",
                        "-show_entries",
                        "frame=best_effort_timestamp_time",
                        "-of",
                        "json",
                        "-i",
                    ]
                    .into_iter()
                    .map(OsString::from),
                )
                .chain([artifact.path.as_os_str().to_owned()])
                .collect(),
            cwd: None,
        };
        let scan = supervisor::run_streaming_stdout(&spec, cancel.clone(), 64*1024, Duration::from_secs(86400), move |reader| {
            let mut index = 0usize;
            let mut valid = true;
            super::encode::frame_scan::parse(reader, |frame: Frame| {
                let expected = index as f64 * f64::from(clock.denominator) / f64::from(clock.numerator);
                let tolerance = if container == ContainerFormat::Webm { 0.001001 } else { 0.000002 };
                valid &= frame.best_effort_timestamp_time.and_then(|value|value.parse::<f64>().ok()).is_some_and(|actual| actual.is_finite() && (actual-expected).abs() <= tolerance);
                index += 1;
            })?;
            if !valid || index != clock.frames { return Err("The final container changed the verified encoded frame count or exact frame clock.".into()); }
            Ok(index)
        }).await.map_err(|error| super::process_error(error, &artifact.path))?;
        if !scan.status.success() || !scan.stderr.is_empty() {
            return Err(invalid("Final encoded frame clock validation failed."));
        }
    }
    input.flush_nonempty_async().await?;
    super::check_cancel(cancel)?;
    Ok(Some(artifact))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document() -> Document {
        serde_json::from_value(serde_json::json!({"streams":[
            {"index":0,"codec_type":"video","codec_name":"h264","nb_read_packets":"48","tags":{"title":"Picture"},"disposition":{"default":0}},
            {"index":2,"codec_type":"subtitle","codec_name":"ass","nb_read_packets":"2","tags":{"language":"eng"},"disposition":{"default":0}}
        ],"chapters":[{"start_time":"0","end_time":"2","tags":{"title":"Chapter"}}]})).unwrap()
    }
    #[test]
    fn container_preflight_rejects_unsupported_codecs_tags_empty_tracks_and_dispositions() {
        let mut doc = document();
        preflight(
            Path::new("output.mp4"),
            &doc,
            &doc.streams.iter().collect::<Vec<_>>(),
            None,
        )
        .unwrap();
        assert!(
            preflight(
                Path::new("output.webm"),
                &doc,
                &doc.streams.iter().collect::<Vec<_>>(),
                None
            )
            .is_err()
        );
        doc.streams[0]
            .tags
            .insert("comment".into(), "Keep me".into());
        assert!(
            preflight(
                Path::new("output.mp4"),
                &doc,
                &doc.streams.iter().collect::<Vec<_>>(),
                None
            )
            .is_err()
        );
        doc.streams[0].tags.remove("comment");
        doc.streams[0]
            .disposition
            .insert("hearing_impaired".into(), 1);
        assert!(
            preflight(
                Path::new("output.mp4"),
                &doc,
                &doc.streams.iter().collect::<Vec<_>>(),
                None
            )
            .is_err()
        );
        doc.streams[0].disposition.remove("hearing_impaired");
        doc.streams[1].nb_read_packets = None;
        assert!(
            preflight(
                Path::new("output.mp4"),
                &doc,
                &doc.streams.iter().collect::<Vec<_>>(),
                None
            )
            .is_err()
        );
        preflight(
            Path::new("output.mkv"),
            &doc,
            &doc.streams.iter().collect::<Vec<_>>(),
            None,
        )
        .unwrap();
    }
    #[test]
    fn only_expected_chapter_auxiliary_track_is_accepted_and_titles_are_normalized() {
        let mut expected = document();
        let mut actual = expected.clone();
        assert!(
            normalize(
                &mut expected.clone(),
                &mut actual.clone(),
                ContainerFormat::Mp4
            )
            .is_err()
        );
        actual.streams.push(serde_json::from_value(serde_json::json!({"index":3,"codec_type":"data","codec_name":"bin_data","codec_tag_string":"text","tags":{"handler_name":"SubtitleHandler"}})).unwrap());
        let indices = normalize(&mut expected, &mut actual, ContainerFormat::Mp4).unwrap();
        assert_eq!(indices, vec![2]);
        assert_eq!(actual.streams.len(), 2);
        assert_eq!(
            expected.streams[0].tags.get("handler_name").unwrap(),
            "Picture"
        );
        assert_eq!(expected.streams[0].disposition["default"], 1);
    }
}
