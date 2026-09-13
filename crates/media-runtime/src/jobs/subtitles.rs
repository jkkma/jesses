//! Subtitle assets live in an owned directory. Only generated relative names
//! enter libass filter strings; arbitrary source/output paths remain OS arguments.
use std::{
    collections::HashSet,
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use media_core::{AppError, EncodeBackend, EncodeSettings, SubtitleMode};
use tokio::sync::watch;

use super::{
    files::Temporary,
    metadata::{Document, Stream},
    trim,
};
use crate::supervisor::{self, CommandSpec};

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("SUBTITLE_SETTINGS_INVALID", message, None)
}

fn failed(message: impl Into<String>) -> AppError {
    AppError::new("SUBTITLE_VALIDATION_FAILED", message, None)
}

fn format(codec: &str) -> Option<&'static str> {
    match codec {
        "ass" => Some("ass"),
        "subrip" | "mov_text" => Some("srt"),
        "webvtt" => Some("webvtt"),
        _ => None,
    }
}

fn target(mode: SubtitleMode) -> Option<(&'static str, &'static str)> {
    match mode {
        SubtitleMode::SubRip => Some(("subrip", "srt")),
        SubtitleMode::Ass => Some(("ass", "ass")),
        SubtitleMode::WebVtt => Some(("webvtt", "webvtt")),
        _ => None,
    }
}

pub(super) fn validate_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    let mut seen = HashSet::new();
    let mut burns = 0;
    for track in &settings.subtitles {
        if !seen.insert(track.stream_index) {
            return Err(invalid(
                "Subtitle settings contain duplicate source stream indices.",
            ));
        }
        if track.mode != SubtitleMode::Copy && settings.backend != EncodeBackend::Standalone {
            return Err(invalid(
                "Subtitle conversion and burn-in currently require standalone encoding.",
            ));
        }
        burns += usize::from(track.mode == SubtitleMode::BurnIn);
    }
    if burns > 1 {
        return Err(invalid(
            "Choose at most one subtitle track to burn into the video.",
        ));
    }
    Ok(())
}

pub(super) fn validate_selection(
    selected: &[&Stream],
    settings: &EncodeSettings,
) -> Result<(), AppError> {
    validate_settings(settings)?;
    for track in &settings.subtitles {
        let stream = selected
            .iter()
            .find(|stream| {
                stream.index == track.stream_index
                    && stream.codec_type.as_deref() == Some("subtitle")
            })
            .ok_or_else(|| invalid("Subtitle settings must refer to selected subtitle tracks."))?;
        if track.mode == SubtitleMode::Copy {
            continue;
        }
        let codec = stream.codec_name.as_deref().unwrap_or_default();
        let text = format(codec).is_some();
        if !text
            && (track.mode != SubtitleMode::BurnIn
                || !matches!(
                    codec,
                    "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvb_subtitle" | "xsub"
                ))
        {
            return Err(invalid(
                "Text conversion supports ASS, SubRip and WebVTT. Bitmap subtitles can be copied or burned; OCR is not applied.",
            ));
        }
        if track.mode == SubtitleMode::BurnIn
            && settings.tone_map.is_none()
            && selected.iter().any(|stream| {
                stream.codec_type.as_deref() == Some("video")
                    && matches!(
                        stream.color_transfer.as_deref(),
                        Some("smpte2084" | "arib-std-b67")
                    )
            })
        {
            return Err(invalid(
                "Subtitle burn-in currently requires SDR video. HDR graphics need an explicit tone-map workflow before rendering.",
            ));
        }
    }
    Ok(())
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

struct Conversion {
    index: u32,
    codec: &'static str,
    text: trim::Text,
    path: PathBuf,
    derived_tags: Vec<String>,
}

pub(super) struct Prepared {
    // Rust drops fields in declaration order: files disappear before directory.
    assets: Vec<Temporary>,
    directory: Option<Directory>,
    conversions: Vec<Conversion>,
    overridden: Vec<u32>,
    burned: Option<u32>,
    bitmap: Option<u32>,
    text_filter: Option<String>,
}

async fn capture(
    ffmpeg: &Path,
    args: Vec<OsString>,
    limit: usize,
    input: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<Vec<u8>, AppError> {
    let output = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        limit,
        Duration::from_secs(60),
    )
    .await
    .map_err(|error| super::process_error(error, input))?;
    if !output.status.success() {
        return Err(invalid(format!(
            "Subtitle processing failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output.stdout)
}

impl Prepared {
    #[allow(clippy::too_many_arguments)]
    pub async fn build(
        settings: &EncodeSettings,
        document: &Document,
        selected: &[&Stream],
        trimmed: Option<&trim::Prepared>,
        ffmpeg: &Path,
        input: &Path,
        output: &Path,
        id: &str,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Self, AppError> {
        validate_selection(selected, settings)?;
        let mut result = Self {
            assets: Vec::new(),
            directory: None,
            conversions: Vec::new(),
            overridden: Vec::new(),
            burned: None,
            bitmap: None,
            text_filter: None,
        };
        for track in settings
            .subtitles
            .iter()
            .filter(|track| track.mode != SubtitleMode::Copy)
        {
            if *cancel.borrow() {
                return Err(super::canceled());
            }
            result.overridden.push(track.stream_index);
            let stream = selected
                .iter()
                .find(|stream| stream.index == track.stream_index)
                .expect("validated selected subtitle");
            let codec = stream.codec_name.as_deref().unwrap_or_default();
            let Some(source_format) = format(codec) else {
                result.burned = Some(track.stream_index);
                result.bitmap = Some(track.stream_index);
                continue;
            };
            if result.directory.is_none() {
                let path = output
                    .parent()
                    .expect("validated output parent")
                    .join(format!(".jesses-{id}-subtitles"));
                std::fs::create_dir(&path).map_err(|error| {
                    invalid(format!("Could not reserve subtitle workspace: {error}"))
                })?;
                result.directory = Some(Directory(path));
            }
            let source =
                if let Some((asset, _)) = trimmed.and_then(|trim| trim.asset(track.stream_index)) {
                    trim::read_subtitles(ffmpeg, asset, 0, codec, cancel).await?
                } else {
                    trim::read_subtitles(ffmpeg, input, track.stream_index, codec, cancel).await?
                };
            let source_path = result.write_asset(
                &format!("source-{}", track.stream_index),
                source_format,
                source.render().as_bytes(),
            )?;
            if track.mode == SubtitleMode::BurnIn {
                result.burned = Some(track.stream_index);
                result
                    .extract_fonts(document, ffmpeg, input, cancel)
                    .await?;
                // The process cwd is the private directory. User-controlled paths
                // and attachment names never enter the filter language.
                let name = source_path
                    .file_name()
                    .expect("generated name")
                    .to_str()
                    .expect("ASCII generated name");
                result.text_filter = Some(format!("subtitles=filename={name}:fontsdir=."));
            } else {
                let (target_codec, target_format) = target(track.mode).expect("conversion mode");
                let args = [
                    "-v".into(),
                    "error".into(),
                    "-nostdin".into(),
                    "-protocol_whitelist".into(),
                    "file".into(),
                    "-f".into(),
                    source_format.into(),
                    "-i".into(),
                    source_path.into_os_string(),
                    "-map".into(),
                    "0:0".into(),
                    "-c:s".into(),
                    target_codec.into(),
                    "-f".into(),
                    target_format.into(),
                    "pipe:1".into(),
                ]
                .into();
                let bytes = capture(ffmpeg, args, 32 * 1024 * 1024, input, cancel).await?;
                let text = trim::Text::parse(
                    target_format,
                    std::str::from_utf8(&bytes)
                        .map_err(|_| failed("Subtitle conversion returned invalid UTF-8."))?,
                )?;
                verify_conversion(&source, &text)?;
                let path = result.write_asset(
                    &format!("converted-{}", track.stream_index),
                    target_format,
                    text.render().as_bytes(),
                )?;
                result.conversions.push(Conversion {
                    index: track.stream_index,
                    codec: target_codec,
                    text,
                    path,
                    derived_tags: stream
                        .tags
                        .keys()
                        .filter(|key| super::metadata::is_derived_stream_tag(key))
                        .cloned()
                        .collect(),
                });
            }
        }
        Ok(result)
    }

    fn write_asset(
        &mut self,
        id: &str,
        extension: &str,
        bytes: &[u8],
    ) -> Result<PathBuf, AppError> {
        let output = self
            .directory
            .as_ref()
            .expect("reserved workspace")
            .0
            .join("assets");
        let file = Temporary::create_extension(&output, id, extension)?;
        let path = file.path.clone();
        let mut writer = file.clone_file()?;
        self.assets.push(file);
        writer
            .write_all(bytes)
            .and_then(|()| writer.sync_all())
            .map_err(|error| invalid(format!("Could not write subtitle asset: {error}")))?;
        Ok(path)
    }

    async fn extract_fonts(
        &mut self,
        document: &Document,
        ffmpeg: &Path,
        input: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        let fonts: Vec<_> = document
            .streams
            .iter()
            .filter(|stream| stream.codec_type.as_deref() == Some("attachment") && font(stream))
            .collect();
        if fonts.len() > 128 {
            return Err(invalid(
                "The source contains more than 128 font attachments.",
            ));
        }
        let video = document
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("video"))
            .ok_or_else(|| invalid("Source video is missing."))?;
        let mut total = 0usize;
        for stream in fonts {
            let args = [
                "-v".into(),
                "error".into(),
                "-nostdin".into(),
                format!("-dump_attachment:{}", stream.index).into(),
                "pipe:1".into(),
                "-protocol_whitelist".into(),
                "file".into(),
                "-i".into(),
                input.as_os_str().to_owned(),
                "-map".into(),
                format!("0:{}", video.index).into(),
                "-frames:v".into(),
                "0".into(),
                "-an".into(),
                "-sn".into(),
                "-f".into(),
                "null".into(),
                "-".into(),
            ]
            .into();
            let bytes = capture(ffmpeg, args, 32 * 1024 * 1024, input, cancel).await?;
            total += bytes.len();
            if total > 128 * 1024 * 1024 || bytes.is_empty() {
                return Err(invalid(
                    "Font attachments are empty or exceed the 128 MiB total limit.",
                ));
            }
            let font = Temporary::create_font(
                &self.directory.as_ref().expect("reserved workspace").0,
                stream.index,
            )?;
            let mut writer = font.clone_file()?;
            self.assets.push(font);
            writer
                .write_all(&bytes)
                .and_then(|()| writer.sync_all())
                .map_err(|error| invalid(format!("Could not write subtitle font: {error}")))?;
        }
        Ok(())
    }

    pub fn effective_indices(&self, selected: &[&Stream]) -> Vec<u32> {
        selected
            .iter()
            .filter(|stream| Some(stream.index) != self.burned)
            .map(|stream| stream.index)
            .collect()
    }
    pub fn overridden_indices(&self) -> &[u32] {
        &self.overridden
    }
    pub fn text_filter(&self) -> Option<&str> {
        self.text_filter.as_deref()
    }
    pub fn bitmap_index(&self) -> Option<u32> {
        self.bitmap
    }
    pub fn decoder_cwd(&self) -> Option<&Path> {
        self.text_filter
            .as_ref()
            .and(self.directory.as_ref())
            .map(|directory| directory.0.as_path())
    }

    pub fn expected_document(&self, base: &Document) -> Document {
        let mut expected = base.clone();
        for converted in &self.conversions {
            let stream = expected
                .streams
                .iter_mut()
                .find(|stream| stream.index == converted.index)
                .expect("original subtitle identity");
            stream.codec_name = Some(converted.codec.into());
            stream.extradata_hash = None;
            stream
                .tags
                .retain(|key, _| !super::metadata::is_derived_stream_tag(key));
            stream.packet_start_time = converted.text.cues.first().map(|cue| cue.start);
            stream.start_time = stream.packet_start_time.map(|value| value.to_string());
            stream.nb_read_packets =
                (!converted.text.cues.is_empty()).then(|| converted.text.cues.len().to_string());
        }
        expected
    }

    /// Called after trim has supplied rebased assets. Maps use effective output
    /// positions, independent of the input number assigned by either module.
    pub fn apply_mux(&self, args: &mut Vec<OsString>, selected: &[&Stream]) {
        let first_input = args.iter().filter(|arg| *arg == "-i").count();
        for (offset, converted) in self.conversions.iter().enumerate() {
            let next_input = first_input + offset;
            let output_index = selected
                .iter()
                .position(|stream| stream.index == converted.index)
                .expect("effective selected subtitle");
            let first_map = args
                .iter()
                .position(|arg| arg == "-map")
                .expect("mapped output");
            args.splice(
                first_map..first_map,
                [
                    "-f".into(),
                    converted.text.format.into(),
                    "-i".into(),
                    converted.path.as_os_str().to_owned(),
                ],
            );
            let map = args
                .iter()
                .enumerate()
                .filter(|(_, arg)| *arg == "-map")
                .nth(output_index)
                .expect("output map")
                .0;
            args[map + 1] = format!("{next_input}:0").into();
            for key in [
                "ENCODER",
                "DURATION",
                "BPS",
                "NUMBER_OF_FRAMES",
                "NUMBER_OF_BYTES",
                "_STATISTICS_WRITING_APP",
                "_STATISTICS_WRITING_DATE_UTC",
                "_STATISTICS_TAGS",
            ]
            .into_iter()
            .chain(converted.derived_tags.iter().map(String::as_str))
            {
                let last = args.len() - 1;
                args.splice(
                    last..last,
                    [
                        format!("-metadata:s:{output_index}").into(),
                        format!("{key}=").into(),
                    ],
                );
            }
        }
    }

    pub async fn verify(
        &self,
        ffmpeg: &Path,
        output: &Path,
        selected: &[&Stream],
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        for converted in &self.conversions {
            let index = selected
                .iter()
                .position(|stream| stream.index == converted.index)
                .expect("effective subtitle") as u32;
            let actual =
                trim::read_subtitles(ffmpeg, output, index, converted.codec, cancel).await?;
            if !converted.text.matches(&actual) {
                return Err(failed(
                    "Converted subtitle timing, text, styles or order changed during muxing.",
                ));
            }
        }
        Ok(())
    }
}

fn font(stream: &Stream) -> bool {
    stream.tags.iter().any(|(key, value)| {
        let value = value.to_ascii_lowercase();
        if key.eq_ignore_ascii_case("filename") {
            [".ttf", ".otf", ".ttc", ".otc", ".woff", ".woff2"]
                .iter()
                .any(|extension| value.ends_with(extension))
        } else {
            key.eq_ignore_ascii_case("mimetype")
                && matches!(
                    value.as_str(),
                    "font/ttf"
                        | "font/otf"
                        | "font/sfnt"
                        | "font/woff"
                        | "font/woff2"
                        | "application/font-sfnt"
                        | "application/font-woff"
                        | "application/x-truetype-font"
                        | "application/vnd.ms-opentype"
                        | "application/x-font-ttf"
                )
        }
    })
}

fn readable(text: &str, ass: bool) -> Result<String, AppError> {
    let mut result = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if ass && character == '{' {
            let mut tag = String::new();
            for character in characters.by_ref() {
                if character == '}' {
                    break;
                }
                tag.push(character);
            }
            if tag.to_ascii_lowercase().contains("\\p") {
                return Err(invalid(
                    "ASS drawing or perspective tags cannot be converted safely to another text format. Copy or burn this track.",
                ));
            }
        } else if !ass && character == '<' {
            let mut tag = String::new();
            for character in characters.by_ref() {
                if character == '>' {
                    break;
                }
                tag.push(character);
            }
            if tag == "br" || tag == "br/" || tag == "br /" {
                result.push('\n');
            } else if !(matches!(
                tag.as_str(),
                "b" | "/b"
                    | "i"
                    | "/i"
                    | "u"
                    | "/u"
                    | "/font"
                    | "ruby"
                    | "/ruby"
                    | "rt"
                    | "/rt"
                    | "/c"
                    | "/v"
                    | "/lang"
            ) || tag.starts_with("font ")
                || tag.starts_with("c.")
                || tag.starts_with("v ")
                || tag.starts_with("lang "))
            {
                return Err(invalid(
                    "Subtitle markup cannot be converted with verified readable text. Copy or burn this track.",
                ));
            }
        } else if ass && character == '\\' {
            match characters.peek() {
                Some('N' | 'n') => {
                    characters.next();
                    result.push('\n');
                }
                Some('h') => {
                    characters.next();
                    result.push(' ');
                }
                _ => result.push(character),
            }
        } else if !ass && character == '&' {
            // SRT also permits a literal ampersand. Only a terminated entity
            // consumes subsequent characters; a bare '&' is ordinary text.
            let lookahead: String = characters
                .clone()
                .take(18)
                .take_while(|c| *c != ';' && !c.is_whitespace())
                .collect();
            if characters.clone().nth(lookahead.chars().count()) != Some(';') {
                result.push('&');
                continue;
            }
            let mut entity = String::new();
            for character in characters.by_ref() {
                if character == ';' {
                    break;
                }
                entity.push(character);
                if entity.len() > 16 {
                    return Err(invalid("Unsupported subtitle text entity."));
                }
            }
            let value = match entity.as_str() {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                value if value.starts_with("#x") => u32::from_str_radix(&value[2..], 16)
                    .ok()
                    .and_then(char::from_u32),
                value if value.starts_with('#') => {
                    value[1..].parse::<u32>().ok().and_then(char::from_u32)
                }
                _ => None,
            }
            .ok_or_else(|| invalid("Unsupported subtitle text entity."))?;
            result.push(value);
        } else {
            result.push(character);
        }
    }
    // Text muxers discard invisible spaces before line breaks. Keep line breaks,
    // leading spaces and every visible character significant.
    Ok(result
        .replace('\u{a0}', " ")
        .replace("\r\n", "\n")
        .split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n"))
}

pub(super) fn verify_conversion(source: &trim::Text, target: &trim::Text) -> Result<(), AppError> {
    if source.cues.len() != target.cues.len() {
        return Err(failed("Subtitle conversion changed the cue count."));
    }
    let tolerance = if source.format == "ass" || target.format == "ass" {
        0.010_001
    } else {
        0.001_001
    };
    for (source_cue, target_cue) in source.cues.iter().zip(&target.cues) {
        if (source_cue.start - target_cue.start).abs() > tolerance
            || (source_cue.end - target_cue.end).abs() > tolerance
            || readable(&source_cue.body, source.format == "ass")?
                != readable(&target_cue.body, target.format == "ass")?
        {
            return Err(failed(
                "Subtitle conversion changed readable text, cue order or timing beyond ASS's 10 ms precision.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn conversion_rejects_lost_text_entities_timing_and_cues() {
        let srt =
            trim::Text::parse("srt", "1\n00:00:00,000 --> 00:00:01,000\nA &amp; B\n\n").unwrap();
        let ass = trim::Text::parse("ass", "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,A &amp; B\n").unwrap();
        assert!(
            verify_conversion(&srt, &ass).is_err(),
            "FFmpeg can retain an entity as literal ASS characters; do not publish changed readable text"
        );
        let valid = trim::Text::parse(
            "webvtt",
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nA &amp; B\n\n",
        )
        .unwrap();
        verify_conversion(&srt, &valid).unwrap();
        let mut damaged = valid.clone();
        damaged.cues[0].start += 0.005;
        assert!(
            verify_conversion(&srt, &damaged).is_err(),
            "SRT/WebVTT precision is one millisecond, not ASS precision"
        );
        damaged = valid.clone();
        damaged.cues[0].body = "A &amp; C".into();
        assert!(verify_conversion(&srt, &damaged).is_err());
        damaged.cues.clear();
        assert!(verify_conversion(&srt, &damaged).is_err());
    }

    #[test]
    fn source_identity_hdr_and_multiple_burn_rules_are_validated() {
        let document: Document = serde_json::from_value(json!({"streams":[{"index":0,"codec_type":"video","codec_name":"h264","color_transfer":"smpte2084"},{"index":5,"codec_type":"subtitle","codec_name":"ass"},{"index":8,"codec_type":"subtitle","codec_name":"subrip"}]})).unwrap();
        let selected = document.selected(&[0, 5, 8]).unwrap();
        let mut settings = EncodeSettings {
            subtitles: serde_json::from_value(json!([{"streamIndex":5,"mode":"burnIn"}])).unwrap(),
            ..Default::default()
        };
        assert!(validate_selection(&selected, &settings).is_err());
        settings.subtitles[0].mode = SubtitleMode::Ass;
        validate_selection(&selected, &settings).unwrap();
        settings.subtitles[0].stream_index = 0;
        assert!(validate_selection(&selected, &settings).is_err());
        settings.subtitles = serde_json::from_value(
            json!([{"streamIndex":5,"mode":"burnIn"},{"streamIndex":8,"mode":"burnIn"}]),
        )
        .unwrap();
        assert!(validate_settings(&settings).is_err());
    }

    #[test]
    fn font_fallback_recognizes_extension_without_trusting_attachment_path() {
        let stream: Stream = serde_json::from_value(json!({"index":7,"codec_type":"attachment","tags":{"filename":"../../escaped.ttf","mimetype":"application/octet-stream"}})).unwrap();
        assert!(font(&stream));
        assert_eq!(
            readable("literal & value", false).unwrap(),
            "literal & value"
        );
        assert_eq!(
            readable(" leading space  \nnext line ", false).unwrap(),
            " leading space\nnext line"
        );
        assert_eq!(
            readable("a {\\i1}bold{\\i0}\\Nline", true).unwrap(),
            "a bold\nline"
        );
        assert!(readable("{\\p1}m 0 0 l 10 10", true).is_err());
    }
}
