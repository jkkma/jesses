//! The final container uses the verified Matroska stage, plus an optional guarded
//! QuickTime timecode source. Its owned sibling is verified before publication.
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsString,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Duration,
};

use media_core::{AppError, ContainerFormat, EncodeSettings, SubtitleMode};
use tokio::sync::watch;

use super::{
    files::Temporary,
    metadata::{self, Document, Stream},
    trim,
};
use crate::supervisor::{self, CommandSpec};

mod tx3g;

#[derive(Clone, Copy)]
pub(super) struct Cadence<'a> {
    pub index: u32,
    pub numerator: u32,
    pub denominator: u32,
    pub frames: usize,
    pub source_iso: Option<SourceIsoReceipt>,
    /// Thorough verification: re-check every converted track and decode the
    /// encoded video of the final file again whenever its copy is not proven
    /// identical to the verified staging video. Off, the final file is checked
    /// only by its streams and its video frame count.
    pub full_verification: bool,
    pub basic_video_check: Option<super::output_counts::VideoCheck>,
    /// Original stream plan when a default AV1AN count is deferred to the
    /// rewritten container. The staging header cannot define expectations.
    pub expected_kinds: Option<&'a [&'a str]>,
}

/// Internal only. Sealed before/after full source proof, bound to this owned
/// pathname and every byte; no settings/IPC value can construct this receipt.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceIsoReceipt {
    digest: [u8; 32],
    length: u64,
    format: ContainerFormat,
}
pub(super) async fn seal_source_iso(
    input: &Temporary,
    format: ContainerFormat,
    cancel: &watch::Receiver<bool>,
) -> Result<SourceIsoReceipt, AppError> {
    input.flush_nonempty_async().await?;
    let mut file = input.clone_file()?;
    let path = input.path.clone();
    let cancellation = cancel.clone();
    let receipt = tokio::task::spawn_blocking(move || {
        file.seek(SeekFrom::Start(0))
            .map_err(|error| invalid(error.to_string()))?;
        let mut hash = Sha256::new();
        let bytes = path.as_os_str().as_encoded_bytes();
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
        hash.update(format.ffmpeg_format().as_bytes());
        let mut block = [0u8; 65536];
        let mut length = 0u64;
        loop {
            super::check_cancel(&cancellation)?;
            let read = file
                .read(&mut block)
                .map_err(|error| invalid(error.to_string()))?;
            if read == 0 {
                break;
            }
            length += read as u64;
            hash.update(&block[..read]);
        }
        Ok::<_, AppError>(SourceIsoReceipt {
            digest: hash.finalize().into(),
            length,
            format,
        })
    })
    .await
    .map_err(|error| invalid(error.to_string()))??;
    input.flush_nonempty_async().await?;
    Ok(receipt)
}
impl SourceIsoReceipt {
    pub(super) async fn verify(
        &self,
        input: &Temporary,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        if seal_source_iso(input, self.format, cancel).await? != *self {
            return Err(invalid(
                "The fully verified source ISO output changed before publication.",
            ));
        }
        Ok(())
    }
}

pub(super) fn source_iso_format(
    output: &Path,
    selected: &[&Stream],
    settings: &EncodeSettings,
) -> Option<ContainerFormat> {
    if !settings.source_trim
        || settings.trim.is_none()
        || settings.backend != media_core::EncodeBackend::Standalone
        || settings.video_mode != media_core::VideoMode::Encode
    {
        return None;
    }
    let format = format(output).ok()?;
    if !matches!(format, ContainerFormat::Mp4 | ContainerFormat::Mov) {
        return None;
    }
    selected
        .iter()
        .any(|stream| {
            stream.codec_type.as_deref() == Some("subtitle")
                && settings
                    .subtitles
                    .iter()
                    .find(|track| track.stream_index == stream.index)
                    .map_or(stream.codec_name.as_deref() == Some("mov_text"), |track| {
                        track.mode == SubtitleMode::MovText
                            || (track.mode == SubtitleMode::Copy
                                && stream.codec_name.as_deref() == Some("mov_text"))
                    })
        })
        .then_some(format)
}

pub(super) struct TimecodeTrack {
    pub source: PathBuf,
    pub stream: Stream,
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("CONTAINER_INCOMPATIBLE", message, None)
}

pub(super) fn format(path: &Path) -> Result<ContainerFormat, AppError> {
    path.extension()
        .and_then(|value| value.to_str())
        .and_then(ContainerFormat::from_extension)
        .ok_or_else(|| invalid("Choose a .mkv, .mp4, .mov, .webm, .m4a or .ogg destination."))
}

/// Header probes carry no packet counts. Only a selected subtitle bound for a
/// container other than Matroska needs one: `preflight` rejects it when empty.
pub(super) fn needs_cue_counts(output: &Path, document: &Document, selected: &[u32]) -> bool {
    format(output).is_ok_and(|container| container != ContainerFormat::Matroska)
        && document.streams.iter().any(|stream| {
            selected.contains(&stream.index) && stream.codec_type.as_deref() == Some("subtitle")
        })
}

/// FFmpeg writes MP4/MOV chapters as a text track that reads back as a
/// trailing data stream. It is not a mapped stream: every remux writes it again
/// from the chapters.
pub(super) fn is_chapter_track(stream: &Stream) -> bool {
    stream.codec_type.as_deref() == Some("data")
        && stream.codec_name.as_deref() == Some("bin_data")
        && stream.codec_tag_string.as_deref() == Some("text")
        && tag(stream, "handler_name") == Some("SubtitleHandler")
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
        ContainerFormat::Matroska => kind != "data" && codec != "mov_text",
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
            "data" => true,
            "video" => matches!(codec, "h264" | "hevc" | "mpeg4" | "prores" | "mjpeg"),
            "audio" => matches!(
                codec,
                "aac" | "mp3" | "ac3" | "eac3" | "alac" | "pcm_s16le" | "pcm_s24le" | "pcm_s32le"
            ),
            "subtitle" => text_codec(codec),
            _ => false,
        },
        ContainerFormat::M4a => kind == "audio" && matches!(codec, "aac" | "alac"),
        ContainerFormat::Ogg => kind == "audio" && matches!(codec, "opus" | "vorbis" | "flac"),
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
    preflight_with_staging(
        output,
        document,
        selected,
        settings,
        staging_format(selected, settings),
    )
}
/// Source single-command routes already write their chosen final container.
pub(super) fn preflight_final(
    output: &Path,
    document: &Document,
    selected: &[&Stream],
    settings: Option<&EncodeSettings>,
) -> Result<(), AppError> {
    preflight_with_staging(output, document, selected, settings, format(output)?)
}
/// Suggest only changes that can help: a Matroska destination is already the
/// most permissive container, so it never recommends choosing Matroska again.
fn incompatible_remedy(container: ContainerFormat, kind: &str) -> &'static str {
    match (container, kind) {
        (ContainerFormat::Matroska, "subtitle") => {
            "Convert it to SubRip, ASS or WebVTT, or deselect that track."
        }
        (ContainerFormat::Matroska, "data") => {
            "Deselect that track, or choose MOV to keep data tracks."
        }
        (ContainerFormat::Matroska, _) => "Change its encoder, or deselect that track.",
        (_, "attachment") => {
            "Attachments require Matroska. Choose Matroska or deselect that track."
        }
        _ => "Choose Matroska, change its encoder, or deselect that track.",
    }
}
fn preflight_with_staging(
    output: &Path,
    document: &Document,
    selected: &[&Stream],
    settings: Option<&EncodeSettings>,
    staging: ContainerFormat,
) -> Result<(), AppError> {
    let container = format(output)?;
    if container == ContainerFormat::Ogg && !document.chapters.is_empty() {
        return Err(invalid(
            "Ogg audio conversion currently requires chapter-free input. Choose Matroska or M4A to retain chapters.",
        ));
    }
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
        if container == ContainerFormat::Ogg
            && stream
                .disposition
                .iter()
                .any(|(flag, value)| *value != 0 && !(selected.len() == 1 && flag == "default"))
        {
            return Err(invalid(
                "Ogg has no stream disposition flags. Clear explicit track flags or choose Matroska; a single audio track is implicitly the default.",
            ));
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
                SubtitleMode::MovText => "mov_text",
                SubtitleMode::Ass => "ass",
                SubtitleMode::SubRip => "subrip",
                SubtitleMode::WebVtt => "webvtt",
                _ => unreachable!("burned track excluded"),
            }
        } else if let Some(settings) = settings.filter(|settings| {
            settings.video_mode == media_core::VideoMode::Encode
                && settings.video_stream_index == stream.index
        }) {
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
                "Stream {} ({kind}, {codec}) is incompatible with {}. {}",
                stream.index,
                container.extension().to_uppercase(),
                incompatible_remedy(container, kind),
            )));
        }
        // The verified staging format cannot carry tx3g. An explicit text
        // conversion in Quick Convert supplies a supported staging asset.
        if stream.codec_name.as_deref() == Some("mov_text")
            && staging == ContainerFormat::Matroska
            && !subtitle.is_some_and(|track| {
                matches!(
                    track.mode,
                    SubtitleMode::Ass
                        | SubtitleMode::SubRip
                        | SubtitleMode::WebVtt
                        | SubtitleMode::MovText
                )
            })
        {
            return Err(invalid(format!(
                "Stream {} uses MP4 text subtitles. In Quick Convert choose SubRip, ASS or WebVTT conversion, or deselect it for stream-copy jobs.",
                stream.index
            )));
        }
        if matches!(
            container,
            ContainerFormat::Mp4 | ContainerFormat::Mov | ContainerFormat::M4a
        ) {
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
                    && !(container == ContainerFormat::Mov && key.eq_ignore_ascii_case("timecode"))
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
    if matches!(
        container,
        ContainerFormat::Mp4 | ContainerFormat::Mov | ContainerFormat::M4a
    ) && document.chapters.iter().any(|chapter| {
        chapter
            .tags
            .keys()
            .any(|key| !key.eq_ignore_ascii_case("title"))
    }) {
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
    cadence: Option<Cadence<'_>>,
    timecode: Option<&TimecodeTrack>,
    faststart: bool,
) -> Result<Vec<OsString>, AppError> {
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
    if let Some(timecode) = timecode {
        args.extend([
            "-protocol_whitelist".into(),
            "file".into(),
            "-i".into(),
            timecode.source.as_os_str().to_owned(),
        ]);
    }
    for stream in &document.streams {
        args.extend(["-map".into(), format!("0:{}", stream.index).into()]);
    }
    if let Some(timecode) = timecode {
        args.extend(["-map".into(), format!("1:{}", timecode.stream.index).into()]);
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
            let scale = clock.numerator.checked_mul(1000).ok_or_else(|| {
                invalid("The verified frame clock exceeds the final-container timebase.")
            })?;
            let step = u64::from(clock.denominator) * 1000;
            let origin = stream
                .packet_start_time
                .or_else(|| {
                    stream
                        .start_time
                        .as_deref()
                        .and_then(|time| time.parse::<f64>().ok())
                })
                .unwrap_or(0.0);
            if !origin.is_finite() {
                return Err(invalid(
                    "The verified frame clock has no finite mux origin.",
                ));
            }
            let origin_ticks = (origin * f64::from(scale)).round();
            // A frame-sized timebase cannot represent source mux preroll such
            // as0.979s at24fps. Keep the proven origin and restore only each
            // frame's relative ordinal, including reordered composition times.
            filters.push(format!(
                "setts=pts=round((PTS-{origin_ticks:.0})/{step})*{step}+{origin_ticks:.0}:dts=round((DTS-{origin_ticks:.0})/{step})*{step}+{origin_ticks:.0}:duration={step}:time_base=1/{scale}:prescale=1"
            ));
        }
        if !filters.is_empty() {
            args.extend([format!("-bsf:{index}").into(), filters.join(",").into()]);
        }
        if stream.codec_type.as_deref() == Some("subtitle") {
            let target = if container == ContainerFormat::Webm {
                "webvtt"
            } else {
                "mov_text"
            };
            args.extend([
                format!("-c:{index}").into(),
                if stream.codec_name.as_deref() == Some(target) {
                    "copy"
                } else {
                    target
                }
                .into(),
            ]);
        }
        if matches!(
            container,
            ContainerFormat::Mp4 | ContainerFormat::Mov | ContainerFormat::M4a
        ) && let Some(title) = tag(stream, "title").or_else(|| tag(stream, "handler_name"))
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
    if container == ContainerFormat::Ogg {
        // Vorbis comments are per stream. Keep global metadata in an explicit
        // namespace so a movie title cannot overwrite an audio track title.
        if let Some(format) = &document.format {
            for (key, value) in &format.tags {
                if metadata::is_derived_stream_tag(key) {
                    continue;
                }
                for stream in &document.streams {
                    args.extend([
                        format!("-metadata:s:{}", stream.index).into(),
                        format!("JESSES_CONTAINER_{key}={value}").into(),
                    ]);
                }
            }
        }
    }
    if container == ContainerFormat::Mov {
        args.extend(["-write_tmcd".into(), "0".into()]);
    }
    if matches!(
        container,
        ContainerFormat::Mp4 | ContainerFormat::Mov | ContainerFormat::M4a
    ) {
        let mut flags = iso_flags(&document.streams.iter().collect::<Vec<_>>());
        if faststart && container == ContainerFormat::Mp4 {
            flags.push_str("+faststart");
        }
        args.extend(["-movflags".into(), flags.into()]);
        if document.streams.iter().any(|stream| {
            stream
                .side_data_list
                .iter()
                .any(|value| value["side_data_type"] == "DOVI configuration record")
        }) {
            // FFmpeg requires this explicit mux mode to retain dvvC/dvcC.
            // The imported AV1 path independently checks its exact fields.
            args.extend(["-strict".into(), "unofficial".into()]);
        }
    }
    args.extend([
        "-f".into(),
        container.ffmpeg_format().into(),
        "-y".into(),
        output.as_os_str().to_owned(),
    ]);
    Ok(args)
}

pub(super) fn normalize(
    expected: &mut Document,
    actual: &mut Document,
    container: ContainerFormat,
) -> Result<Vec<u32>, AppError> {
    let mut converted = Vec::new();
    let iso = matches!(
        container,
        ContainerFormat::Mp4 | ContainerFormat::Mov | ContainerFormat::M4a
    );
    if container == ContainerFormat::Mov
        && !expected.streams.iter().any(|stream| {
            stream.codec_type.as_deref() == Some("data")
                && stream.codec_tag_string.as_deref() == Some("tmcd")
        })
    {
        // A demuxed video timecode tag is derived from its tmcd association.
        // It must not synthesize an unchecked data track when none is mapped.
        for stream in expected
            .streams
            .iter_mut()
            .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        {
            stream
                .tags
                .retain(|key, _| !key.eq_ignore_ascii_case("timecode"));
        }
    }
    if container == ContainerFormat::Ogg {
        if let (Some(source_format), Some(actual_format)) = (&expected.format, &mut actual.format) {
            for (key, value) in &source_format.tags {
                if metadata::is_derived_stream_tag(key) {
                    continue;
                }
                let comment = format!("JESSES_CONTAINER_{key}");
                if !actual
                    .streams
                    .iter()
                    .all(|stream| tag(stream, &comment) == Some(value.as_str()))
                {
                    return Err(invalid(
                        "Ogg output did not retain the source container metadata in its audio comments.",
                    ));
                }
                actual_format.tags.insert(key.clone(), value.clone());
            }
        }
        if expected.streams.len() == 1
            && let Some(stream) = actual.streams.first_mut()
        {
            // A sole Ogg stream is the playback default without an authored flag.
            if expected.streams[0]
                .disposition
                .get("default")
                .copied()
                .unwrap_or(0)
                != 0
            {
                stream.disposition.insert("default".into(), 1);
            }
        }
    }
    if iso && !expected.chapters.is_empty() {
        let auxiliary = actual
            .streams
            .last()
            .filter(|stream| is_chapter_track(stream));
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
            if stream.codec_type.as_deref() != Some("data")
                && first.insert(stream.codec_type.clone())
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

/// Keep ordinary work in MKV; tx3g needs ISO BMFF and timed data needs MOV.
/// MOV retains PCM; AV1/VP9 pictures instead require the MP4 muxer.
pub(super) fn staging_format(
    selected: &[&Stream],
    settings: Option<&EncodeSettings>,
) -> ContainerFormat {
    if selected
        .iter()
        .any(|stream| stream.codec_type.as_deref() == Some("data"))
    {
        return ContainerFormat::Mov;
    }
    if selected.iter().any(|stream| {
        if stream.codec_type.as_deref() != Some("subtitle") {
            return false;
        }
        let mode = settings
            .and_then(|settings| {
                settings
                    .subtitles
                    .iter()
                    .find(|track| track.stream_index == stream.index)
            })
            .map(|track| track.mode)
            .unwrap_or(SubtitleMode::Copy);
        mode == SubtitleMode::MovText
            || (mode == SubtitleMode::Copy && stream.codec_name.as_deref() == Some("mov_text"))
    }) {
        let needs_mp4 = selected.iter().any(|stream| {
            if stream.codec_type.as_deref() != Some("video") {
                return false;
            }
            if let Some(settings) = settings.filter(|settings| {
                settings.video_mode == media_core::VideoMode::Encode
                    && settings.video_stream_index == stream.index
            }) {
                !matches!(
                    settings.encoder,
                    media_core::VideoEncoder::X264
                        | media_core::VideoEncoder::X265
                        | media_core::VideoEncoder::X265Standalone
                )
            } else {
                matches!(stream.codec_name.as_deref(), Some("av1" | "vp9"))
            }
        });
        if needs_mp4 {
            ContainerFormat::Mp4
        } else {
            ContainerFormat::Mov
        }
    } else {
        ContainerFormat::Matroska
    }
}

fn iso_flags(selected: &[&Stream]) -> String {
    // Forcing a colr box on an entirely unspecified source invents a limited
    // range declaration in MP4. Keep unknown signaling unknown during copy.
    let known = selected.iter().any(|stream| {
        [
            &stream.color_range,
            &stream.color_space,
            &stream.color_primaries,
            &stream.color_transfer,
        ]
        .into_iter()
        .any(|value| {
            value
                .as_deref()
                .is_some_and(|value| !matches!(value, "unknown" | "unspecified"))
        })
    });
    if known {
        "use_metadata_tags+write_colr"
    } else {
        "use_metadata_tags"
    }
    .into()
}

pub(super) fn append_staging_format(
    args: &mut Vec<OsString>,
    selected: &[&Stream],
    settings: Option<&EncodeSettings>,
) {
    let format = staging_format(selected, settings);
    append_format_options(args, format, selected);
    args.extend(["-f".into(), format.ffmpeg_format().into(), "-y".into()]);
}

pub(super) fn append_format_options(
    args: &mut Vec<OsString>,
    format: ContainerFormat,
    selected: &[&Stream],
) {
    if matches!(format, ContainerFormat::Mov | ContainerFormat::Mp4) {
        args.extend([
            "-movflags".into(),
            iso_flags(selected).into(),
            "-write_tmcd".into(),
            "0".into(),
        ]);
        for (position, stream) in selected
            .iter()
            .enumerate()
            .filter(|(_, stream)| stream.codec_type.as_deref() == Some("data"))
        {
            if let Some(tag) = &stream.codec_tag_string {
                args.extend([format!("-tag:{position}").into(), tag.into()]);
            }
        }
    }
}

pub(super) fn normalize_staging(
    expected: &Document,
    indices: &[u32],
    actual: &mut Document,
    settings: Option<&EncodeSettings>,
) -> Result<(Document, Vec<u32>), AppError> {
    let selected = expected.selected(indices)?;
    let mut document = expected.clone();
    let format = staging_format(&selected, settings);
    let converted = if matches!(format, ContainerFormat::Mov | ContainerFormat::Mp4) {
        document.streams = selected.iter().map(|stream| (*stream).clone()).collect();
        normalize(&mut document, actual, format)?
    } else {
        Vec::new()
    };
    Ok((document, converted))
}

/// MP4/MOV assigns the first track of a media kind as default when no track
/// of that kind is marked default. A user-requested `default: false` on that
/// first track must not be silently promoted during final normalization.
pub(super) fn preflight_explicit_defaults(
    output: &Path,
    selected: &[&Stream],
    explicitly_disabled: &[u32],
) -> Result<(), AppError> {
    if !matches!(
        format(output)?,
        ContainerFormat::Mp4 | ContainerFormat::Mov | ContainerFormat::M4a
    ) || explicitly_disabled.is_empty()
    {
        return Ok(());
    }
    let kinds_with_default: HashSet<_> = selected
        .iter()
        .filter(|stream| stream.disposition.get("default").copied().unwrap_or(0) != 0)
        .filter_map(|stream| stream.codec_type.as_deref())
        .collect();
    let mut first = HashSet::new();
    for stream in selected {
        let Some(kind) = stream.codec_type.as_deref() else {
            continue;
        };
        if kind != "data"
            && first.insert(kind)
            && explicitly_disabled.contains(&stream.index)
            && !kinds_with_default.contains(kind)
        {
            return Err(invalid(format!(
                "Stream {} explicitly disables its default flag, but MP4/MOV requires one default {kind} track and would promote this first track. Mark another {kind} track default or choose Matroska.",
                stream.index
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare<'a>(
    input: &Temporary,
    output: &Path,
    id: &str,
    cancel: &watch::Receiver<bool>,
    cadence: Option<Cadence<'_>>,
    timecode: Option<&TimecodeTrack>,
    faststart: bool,
    scratch: &'a mut Vec<Temporary>,
) -> Result<Option<&'a Temporary>, AppError> {
    let container = format(output)?;
    if timecode.is_some() && container != ContainerFormat::Mov {
        return Err(invalid("QuickTime timecode can be copied only into MOV."));
    }
    if container == ContainerFormat::Matroska {
        return Ok(None);
    }
    super::check_cancel(cancel)?;
    let ffmpeg = super::discover("ffmpeg", cancel).await?;
    let ffprobe = super::discover("ffprobe", cancel).await?;
    // Only an encode passes a cadence. Without thorough verification its final
    // file is checked like its staging file: by streams and video frame count.
    let full_verification = cadence.is_none_or(|clock| clock.full_verification);
    if let Some(receipt) = cadence.and_then(|clock| clock.source_iso) {
        if receipt.format != container || format(&input.path)? != container || timecode.is_some() {
            return Err(invalid(
                "The sealed source ISO receipt does not match the final container.",
            ));
        }
        receipt.verify(input, cancel).await?;
        if !full_verification {
            // The sealed file is the staging file, whose counts the encode
            // already checked.
            super::check_cancel(cancel)?;
            return Ok(None);
        }
        let document = super::probe(&ffprobe, &input.path, cancel, None).await?;
        // The sealed file is the staging file, whose encoded video the encode
        // already verified at its chosen level.
        super::encode::boxed(|| {
            verify_final_media(
                &ffmpeg,
                &ffprobe,
                &input.path,
                &document,
                container,
                cadence,
                true,
                cancel,
            )
        })
        .await?;
        receipt.verify(input, cancel).await?;
        super::check_cancel(cancel)?;
        return Ok(None);
    }
    let mut document = if let Some(kinds) = cadence.and_then(|clock| clock.expected_kinds) {
        // Count video once after rewriting the file. Before the rewrite only
        // subtitle compatibility can require counts from the staging file.
        let headers = if kinds.contains(&"subtitle") {
            super::probe(&ffprobe, &input.path, cancel, None).await?
        } else {
            super::probe_headers(&ffprobe, &input.path, cancel, None).await?
        };
        super::output_counts::check_streams(&headers, kinds)?;
        headers
    } else {
        super::probe(&ffprobe, &input.path, cancel, None).await?
    };
    if !document.chapters.is_empty() && document.streams.last().is_some_and(is_chapter_track) {
        document.streams.pop();
    }
    for stream in document
        .streams
        .iter_mut()
        .filter(|stream| stream.codec_type.as_deref() == Some("data"))
    {
        let timeline = Box::pin(super::mux::packet_timeline(
            &ffprobe,
            &input.path,
            stream.index,
            cancel,
        ))
        .await?
        .ok_or_else(|| invalid("Mapped MOV data has no timed packets to preserve."))?;
        stream.packet_start_time = Some(timeline.min_pts);
        stream.duration = Some(timeline.max_end.to_string());
    }
    preflight(
        output,
        &document,
        &document.streams.iter().collect::<Vec<_>>(),
        None,
    )?;
    // Keep ownership in the job even when conversion or validation returns an
    // error. A deny-delete reader must produce a visible cleanup diagnostic,
    // rather than leave a partial file behind through best-effort Drop.
    scratch.push(Temporary::create_extension(
        output,
        &format!("{id}-container"),
        container.extension(),
    )?);
    let artifact = scratch.last().expect("owned final container");
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.clone(),
            args: arguments(
                &input.path,
                &artifact.path,
                &document,
                container,
                cadence,
                timecode,
                faststart,
            )?,
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
    if let Some(clock) = cadence.filter(|_| !full_verification) {
        let mut kinds: Vec<&str> = clock.expected_kinds.map_or_else(
            || {
                document
                    .streams
                    .iter()
                    .map(|stream| stream.codec_type.as_deref().unwrap_or_default())
                    .collect()
            },
            <[&str]>::to_vec,
        );
        kinds.extend(timecode.map(|_| "data"));
        super::output_counts::verify(
            &ffprobe,
            &artifact.path,
            &kinds,
            clock.index as usize,
            clock.frames,
            clock.basic_video_check,
            cancel,
        )
        .await?;
        input.flush_nonempty_async().await?;
        super::check_cancel(cancel)?;
        return Ok(Some(artifact));
    }
    let mut actual = super::probe(&ffprobe, &artifact.path, cancel, None).await?;
    let mut expected = document.clone();
    if let Some(timecode) = timecode {
        let mut stream = timecode.stream.clone();
        stream.index = expected.streams.len() as u32;
        expected.streams.push(stream);
    }
    let converted = normalize(&mut expected, &mut actual, container)?;
    metadata::verify_container(
        &expected,
        &expected.streams.iter().collect::<Vec<_>>(),
        &actual,
        &converted,
    )?;
    if let Some(timecode) = timecode {
        super::mux::verify_shifted_packets(
            &ffprobe,
            &timecode.source,
            timecode.stream.index,
            &artifact.path,
            document.streams.len() as u32,
            0.0,
            cancel,
        )
        .await?;
    }
    for (source, target) in document.streams.iter().zip(&actual.streams) {
        if source.codec_type.as_deref() == Some("subtitle") {
            if source.codec_name == target.codec_name {
                if source.extradata_hash != target.extradata_hash
                    && !(source.codec_name.as_deref() == Some("mov_text")
                        && tx3g::equivalent(
                            &ffprobe,
                            &input.path,
                            source,
                            &artifact.path,
                            target,
                            cancel,
                        )
                        .await?)
                {
                    return Err(invalid(
                        "The final container changed copied subtitle codec headers or styles.",
                    ));
                }
                Box::pin(super::mux::verify_container_packets(
                    &ffprobe,
                    &input.path,
                    source.index,
                    &artifact.path,
                    target.index,
                    false,
                    cancel,
                ))
                .await?;
            }
            let before = Box::pin(trim::read_subtitles(
                &ffmpeg,
                &input.path,
                source.index,
                source.codec_name.as_deref().unwrap_or_default(),
                cancel,
            ))
            .await?;
            let after = Box::pin(trim::read_subtitles(
                &ffmpeg,
                &artifact.path,
                target.index,
                target.codec_name.as_deref().unwrap_or_default(),
                cancel,
            ))
            .await?;
            super::subtitles::verify_conversion(&before, &after)?;
        } else {
            let last_duration = Box::pin(super::mux::verify_container_packets(
                &ffprobe,
                &input.path,
                source.index,
                &artifact.path,
                target.index,
                source.codec_name.as_deref() == Some("aac"),
                cancel,
            ))
            .await?;
            if source.codec_type.as_deref() == Some("audio") {
                let is_aac = source.codec_name.as_deref() == Some("aac");
                let before = if is_aac {
                    super::audio::scan_container_source(&ffprobe, &input.path, source, cancel)
                        .await?
                } else {
                    super::audio::scan(&ffprobe, &input.path, source, false, cancel).await?
                };
                let after = if is_aac {
                    super::audio::scan_container_output(
                        &ffprobe,
                        &artifact.path,
                        target,
                        &before,
                        cancel,
                    )
                    .await?
                } else {
                    super::audio::scan(&ffprobe, &artifact.path, target, true, cancel).await?
                };
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
                return Err(invalid(format!(
                    "The final container changed video pixel/color/aspect declarations for stream {}: {:?} → {:?}.",
                    source.index,
                    (
                        &source.pix_fmt,
                        &source.color_space,
                        &source.color_transfer,
                        &source.color_primaries,
                        &source.color_range,
                        &source.sample_aspect_ratio
                    ),
                    (
                        &target.pix_fmt,
                        &target.color_space,
                        &target.color_transfer,
                        &target.color_primaries,
                        &target.color_range,
                        &target.sample_aspect_ratio
                    )
                )));
            }
        }
    }
    // `cadence` is given only by an encode. With full verification, its staging
    // video was decoded completely and validated frame by frame, or proven a
    // byte-identical copy of a stream that was. Its packets were compared byte
    // for byte above; with the same codec configuration, this copy decodes to
    // the same frames. A rewritten configuration (for example, the MOV range
    // flag) is decoded again. Fast verification never decodes it here.
    let encoded_video_copied = cadence.is_some_and(|clock| {
        !clock.full_verification
            || document
                .streams
                .iter()
                .zip(&actual.streams)
                .any(|(source, target)| {
                    source.index == clock.index
                        && source.codec_type.as_deref() == Some("video")
                        && target.codec_type.as_deref() == Some("video")
                        && source.extradata_hash == target.extradata_hash
                })
    });
    super::encode::boxed(|| {
        verify_final_media(
            &ffmpeg,
            &ffprobe,
            &artifact.path,
            &document,
            container,
            cadence,
            encoded_video_copied,
            cancel,
        )
    })
    .await?;
    input.flush_nonempty_async().await?;
    super::check_cancel(cancel)?;
    Ok(Some(artifact))
}

/// The `ffmpeg -f null` arguments that decode every video and audio stream of
/// the final file except a verified encoded video, or `None` when nothing is
/// left to decode.
fn final_decode_args(
    path: &Path,
    document: &Document,
    verified_video: Option<u32>,
) -> Option<Vec<OsString>> {
    let decoded = |stream: &&Stream| {
        matches!(stream.codec_type.as_deref(), Some("video" | "audio"))
            && Some(stream.index) != verified_video
    };
    let mut maps: Vec<OsString> = Vec::new();
    let mut video = false;
    if verified_video.is_none() {
        if !document.streams.iter().any(|stream| decoded(&stream)) {
            return None;
        }
        maps.extend(["-map", "0:v?", "-map", "0:a?"].map(OsString::from));
        video = true;
    } else {
        for stream in document.streams.iter().filter(decoded) {
            video |= stream.codec_type.as_deref() == Some("video");
            maps.extend(["-map".into(), format!("0:{}", stream.index).into()]);
        }
        if maps.is_empty() {
            return None;
        }
    }
    let mut args: Vec<OsString> = [
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
    .chain([path.as_os_str().to_owned()])
    .chain(maps)
    .collect();
    if video {
        args.extend(
            ["-fps_mode:v", "passthrough", "-enc_time_base:v", "demux"].map(OsString::from),
        );
    }
    args.extend(["-f", "null", "-"].map(OsString::from));
    Some(args)
}

/// Decodes the final file and checks the encoded frame clock. With
/// `encoded_video_copied`, the caller has proven that the encoded video is a
/// byte-identical copy, with the same codec configuration, of a stream that
/// was already decoded completely and validated. That stream is then not
/// decoded again, and its clock is read from packet timestamps.
#[allow(clippy::too_many_arguments)]
async fn verify_final_media(
    ffmpeg: &Path,
    ffprobe: &Path,
    path: &Path,
    document: &Document,
    container: ContainerFormat,
    cadence: Option<Cadence<'_>>,
    encoded_video_copied: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let verified_video = cadence
        .filter(|_| encoded_video_copied)
        .map(|clock| clock.index);
    if let Some(args) = final_decode_args(path, document, verified_video) {
        let decoded = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            256 * 1024,
            Duration::from_secs(24 * 60 * 60),
        )
        .await
        .map_err(|error| super::process_error(error, path))?;
        if !decoded.status.success() || !decoded.stderr.is_empty() {
            return Err(invalid(format!(
                "The complete final container did not decode without errors: {}",
                String::from_utf8_lossy(&decoded.stderr).trim()
            )));
        }
    }
    if let Some(clock) = cadence {
        let origin = document
            .streams
            .iter()
            .find(|stream| stream.index == clock.index)
            .and_then(|stream| {
                stream.packet_start_time.or_else(|| {
                    stream
                        .start_time
                        .as_deref()
                        .and_then(|value| value.parse::<f64>().ok())
                })
            })
            .filter(|value| value.is_finite())
            .ok_or_else(|| invalid("The verified final video has no usable mux origin."))?;
        let tolerance = if container == ContainerFormat::Webm {
            0.001001
        } else {
            0.000002
        };
        if verified_video.is_some() {
            return super::packet_clock::verify(
                ffprobe,
                path,
                clock.index,
                super::packet_clock::Expected {
                    origin,
                    frame_seconds: f64::from(clock.denominator) / f64::from(clock.numerator),
                    tolerance,
                    frames: clock.frames,
                },
                cancel,
            )
            .await;
        }
        #[derive(serde::Deserialize)]
        struct Frame {
            best_effort_timestamp_time: Option<String>,
        }
        let spec = CommandSpec {
            executable: ffprobe.to_owned(),
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
                .chain([path.as_os_str().to_owned()])
                .collect(),
            cwd: None,
        };
        let scan = supervisor::run_streaming_stdout(&spec, cancel.clone(), 64*1024, Duration::from_secs(86400), move |reader| {
            let mut index = 0usize;
            let mut valid = true;
            super::encode::frame_scan::parse(reader, |frame: Frame| {
                let expected = origin + index as f64 * f64::from(clock.denominator) / f64::from(clock.numerator);
                valid &= frame.best_effort_timestamp_time.and_then(|value|value.parse::<f64>().ok()).is_some_and(|actual| actual.is_finite() && (actual-expected).abs() <= tolerance);
                index += 1;
            })?;
            if !valid || index != clock.frames { return Err("The final container changed the verified encoded frame count or exact frame clock.".into()); }
            Ok(index)
        }).await.map_err(|error| super::process_error(error, path))?;
        if !scan.status.success() || !scan.stderr.is_empty() {
            return Err(invalid("Final encoded frame clock validation failed."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_counts_are_needed_only_for_subtitles_outside_matroska() {
        let document: Document = serde_json::from_value(serde_json::json!({"streams": [
            {"index": 0, "codec_type": "video"},
            {"index": 1, "codec_type": "subtitle"}
        ]}))
        .unwrap();
        assert!(!needs_cue_counts(
            Path::new("/out/a.mkv"),
            &document,
            &[0, 1]
        ));
        assert!(needs_cue_counts(
            Path::new("/out/a.mp4"),
            &document,
            &[0, 1]
        ));
        assert!(needs_cue_counts(
            Path::new("/out/a.mov"),
            &document,
            &[0, 1]
        ));
        assert!(!needs_cue_counts(Path::new("/out/a.mp4"), &document, &[0]));
    }

    #[tokio::test]
    async fn source_iso_receipt_rejects_changed_bytes_and_another_owned_path() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "jesses-iso-seal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let output = root.join("out.mp4");
        let mut first = Temporary::create_extension(&output, "seal-first", "mp4").unwrap();
        let mut second = Temporary::create_extension(&output, "seal-second", "mp4").unwrap();
        for file in [&first, &second] {
            let mut writer = file.clone_file().unwrap();
            writer.seek(SeekFrom::Start(0)).unwrap();
            writer.write_all(b"original ISO payload").unwrap();
            writer.sync_all().unwrap();
        }
        let (_owner, cancel) = watch::channel(false);
        let receipt = seal_source_iso(&first, ContainerFormat::Mp4, &cancel)
            .await
            .unwrap();
        receipt.verify(&first, &cancel).await.unwrap();
        assert!(receipt.verify(&second, &cancel).await.is_err());
        let mut writer = first.clone_file().unwrap();
        writer.seek(SeekFrom::Start(0)).unwrap();
        writer.write_all(b"modified ISO payload").unwrap();
        writer.sync_all().unwrap();
        let error = receipt.verify(&first, &cancel).await.unwrap_err();
        assert!(error.message.contains("changed before publication"));
        drop(writer);
        first.cleanup().unwrap();
        second.cleanup().unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    fn document() -> Document {
        serde_json::from_value(serde_json::json!({"streams":[
            {"index":0,"codec_type":"video","codec_name":"h264","nb_read_packets":"48","tags":{"title":"Picture"},"disposition":{"default":0}},
            {"index":2,"codec_type":"subtitle","codec_name":"ass","nb_read_packets":"2","tags":{"language":"eng"},"disposition":{"default":0}}
        ],"chapters":[{"start_time":"0","end_time":"2","tags":{"title":"Chapter"}}]})).unwrap()
    }
    #[test]
    fn final_decode_skips_only_the_verified_encoded_video() {
        let mut document: Document = serde_json::from_value(serde_json::json!({"streams":[
            {"index":0,"codec_type":"video","codec_name":"av1"},
            {"index":1,"codec_type":"audio","codec_name":"opus"},
            {"index":2,"codec_type":"video","codec_name":"mjpeg"},
            {"index":3,"codec_type":"subtitle","codec_name":"mov_text"}
        ]}))
        .unwrap();
        let path = Path::new("final.mp4");
        let text = |args: Vec<OsString>| {
            args.iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let everything = text(final_decode_args(path, &document, None).unwrap());
        assert!(
            everything.contains("-map 0:v? -map 0:a? -fps_mode:v passthrough"),
            "{everything}"
        );
        let others = text(final_decode_args(path, &document, Some(0)).unwrap());
        assert!(
            others.contains("-map 0:1 -map 0:2 -fps_mode:v passthrough"),
            "{others}"
        );
        assert!(!others.contains("-map 0:0"), "{others}");
        document.streams.retain(|stream| stream.index != 2);
        let audio = text(final_decode_args(path, &document, Some(0)).unwrap());
        assert!(audio.ends_with("-map 0:1 -f null -"), "{audio}");
        document.streams.retain(|stream| stream.index != 1);
        assert!(final_decode_args(path, &document, Some(0)).is_none());
        assert!(final_decode_args(path, &document, None).is_some());
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
    fn incompatible_track_advice_never_suggests_the_chosen_container() {
        let mut doc = document();
        doc.streams[1].codec_name = Some("mov_text".into());
        let check = |doc: &Document, output: &str| {
            let selected = doc.streams.iter().collect::<Vec<_>>();
            preflight(Path::new(output), doc, &selected, None)
                .unwrap_err()
                .message
        };
        let message = check(&doc, "output.mkv");
        assert!(message.contains("incompatible with MKV"), "{message}");
        assert!(message.contains("SubRip"), "{message}");
        assert!(!message.contains("Choose Matroska"), "{message}");
        assert!(!message.contains("Attachments"), "{message}");
        doc.streams[1].codec_name = Some("hdmv_pgs_subtitle".into());
        let message = check(&doc, "output.mp4");
        assert!(message.contains("Choose Matroska"), "{message}");
        assert!(!message.contains("Attachments"), "{message}");
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

    #[test]
    fn explicit_false_is_rejected_only_when_iso_would_promote_that_first_track() {
        let mut doc = document();
        let selected = doc.streams.iter().collect::<Vec<_>>();
        assert!(preflight_explicit_defaults(Path::new("out.mp4"), &selected, &[0]).is_err());
        assert!(preflight_explicit_defaults(Path::new("out.mov"), &selected, &[0]).is_err());
        assert!(preflight_explicit_defaults(Path::new("out.mkv"), &selected, &[0]).is_ok());
        assert!(preflight_explicit_defaults(Path::new("out.mp4"), &selected, &[]).is_ok());
        doc.streams.push(
            serde_json::from_value(serde_json::json!({
                "index":3,"codec_type":"video","codec_name":"h264",
                "disposition":{"default":1}
            }))
            .unwrap(),
        );
        let selected = doc.streams.iter().collect::<Vec<_>>();
        assert!(preflight_explicit_defaults(Path::new("out.mp4"), &selected, &[0]).is_ok());
        doc.streams[2].disposition.insert("default".into(), 0);
        let selected = doc.streams.iter().collect::<Vec<_>>();
        assert!(preflight_explicit_defaults(Path::new("out.mp4"), &selected, &[3]).is_ok());
    }
}
