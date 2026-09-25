use std::{ffi::OsString, path::Path, time::Duration};

use media_core::AppError;
use tokio::sync::watch;

use super::{Interval, unsupported};
use crate::supervisor::{self, CommandSpec};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cue {
    pub start: f64,
    pub end: f64,
    pub(crate) body: String,
    pub(crate) fields: Vec<String>,
    pub(crate) settings: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Text {
    pub format: &'static str,
    pub(crate) header: String,
    pub cues: Vec<Cue>,
}

fn timestamp(value: &str) -> Result<f64, AppError> {
    let parts: Vec<_> = value.trim().split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(unsupported("Unsupported subtitle timestamp."));
    }
    let mut total = 0.0;
    for part in parts {
        let value: f64 = part
            .replace(',', ".")
            .parse()
            .map_err(|_| unsupported("Invalid subtitle timestamp."))?;
        if !value.is_finite() || value < 0.0 {
            return Err(unsupported("Invalid subtitle timestamp."));
        }
        total = total * 60.0 + value;
    }
    Ok(total)
}

fn clock(seconds: f64, format: &str) -> String {
    let factor = if format == "ass" { 100 } else { 1000 };
    let ticks = (seconds * f64::from(factor)).round().max(0.0) as u64;
    let fraction = ticks % factor as u64;
    let seconds = ticks / factor as u64;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    match format {
        "ass" => format!("{hours}:{minutes:02}:{seconds:02}.{fraction:02}"),
        "srt" => format!("{hours:02}:{minutes:02}:{seconds:02},{fraction:03}"),
        _ => format!("{hours:02}:{minutes:02}:{seconds:02}.{fraction:03}"),
    }
}

impl Text {
    pub fn shifted(&self, seconds: f64) -> Result<Self, AppError> {
        if !seconds.is_finite() || seconds.abs() > 86_400.0 {
            return Err(unsupported("Subtitle timing offset exceeds 24 hours."));
        }
        let mut result = self.clone();
        let precision = if self.format == "ass" { 100.0 } else { 1000.0 };
        for cue in &mut result.cues {
            if seconds != 0.0 && self.format == "webvtt" && has_inline_timestamp(&cue.body) {
                return Err(unsupported(
                    "WebVTT inline timestamp cues cannot be offset safely. Use zero offset or a track without timed inline markup.",
                ));
            }
            cue.start = ((cue.start + seconds) * precision).round() / precision;
            cue.end = ((cue.end + seconds) * precision).round() / precision;
            if self.format == "ass" {
                cue.fields[1] = clock(cue.start, self.format);
                cue.fields[2] = clock(cue.end, self.format);
            }
        }
        Ok(result)
    }

    /// Text formats cannot represent negative timestamps. Retain the visible
    /// part of shifted cues, using the same guarded clipping rules as trimming.
    pub fn visible(&self) -> Result<Self, AppError> {
        if self.cues.iter().all(|cue| cue.start >= 0.0) {
            return Ok(self.clone());
        }
        self.clipped(Interval {
            frames: 0,
            source_start_frame: 0,
            source_end_frame: 0,
            start: 0.0,
            end: f64::MAX,
        })
    }

    pub(crate) fn parse(format: &'static str, text: &str) -> Result<Self, AppError> {
        let text = text.replace("\r\n", "\n");
        let mut result = Self {
            format,
            header: String::new(),
            cues: Vec::new(),
        };
        if format == "ass" {
            let mut events = false;
            for line in text.lines() {
                if line.trim() == "[Events]" {
                    events = true;
                }
                if let Some(event) = line.strip_prefix("Dialogue: ") {
                    let fields: Vec<String> = event.splitn(10, ',').map(str::to_owned).collect();
                    if !events || fields.len() != 10 {
                        return Err(unsupported("Unsupported ASS event format."));
                    }
                    result.cues.push(Cue {
                        start: timestamp(&fields[1])?,
                        end: timestamp(&fields[2])?,
                        body: fields[9].clone(),
                        fields,
                        settings: String::new(),
                    });
                } else {
                    if events
                        && line.starts_with("Format:")
                        && line.trim()
                            != "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text"
                    {
                        return Err(unsupported(
                            "ASS trimming requires the standard event field order.",
                        ));
                    }
                    result.header.push_str(line);
                    result.header.push('\n');
                }
            }
            if !events {
                return Err(unsupported("ASS subtitle header has no Events section."));
            }
        } else {
            if format == "webvtt" {
                result.header = "WEBVTT\n\n".into();
            }
            for block in text.split("\n\n").filter(|block| !block.trim().is_empty()) {
                let lines: Vec<_> = block.lines().collect();
                let Some(position) = lines.iter().position(|line| line.contains(" --> ")) else {
                    if format == "webvtt" && block.starts_with("WEBVTT") {
                        continue;
                    }
                    return Err(unsupported(
                        "Unsupported subtitle cue block or header metadata.",
                    ));
                };
                let (start, end) = lines[position]
                    .split_once(" --> ")
                    .ok_or_else(|| unsupported("Unsupported subtitle cue timing."))?;
                let (end, settings) = end.split_once(' ').unwrap_or((end, ""));
                result.cues.push(Cue {
                    start: timestamp(start)?,
                    end: timestamp(end)?,
                    body: lines[position + 1..].join("\n"),
                    fields: Vec::new(),
                    settings: settings.to_owned(),
                });
            }
        }
        if result.cues.iter().any(|cue| cue.end <= cue.start) {
            return Err(unsupported(
                "Subtitle cue has an empty or reversed interval.",
            ));
        }
        Ok(result)
    }

    pub fn clipped(&self, interval: Interval) -> Result<Self, AppError> {
        let mut result = self.clone();
        result.cues.clear();
        for cue in &self.cues {
            if cue.end <= interval.start || cue.start >= interval.end {
                continue;
            }
            let cut = cue.start < interval.start || cue.end > interval.end;
            if self.format == "ass" && cut {
                let lower = cue.body.to_ascii_lowercase();
                if ["\\t(", "\\move(", "\\fad(", "\\fade(", "\\k"]
                    .iter()
                    .any(|tag| lower.contains(tag))
                    || !cue.fields[8].is_empty()
                {
                    return Err(unsupported(
                        "A boundary overlaps an ASS cue with animation, karaoke or effects. Choose boundaries outside that cue or exclude the track; its timing will not be silently changed.",
                    ));
                }
            }
            if self.format == "webvtt" && has_inline_timestamp(&cue.body) {
                return Err(unsupported(
                    "WebVTT inline timestamp cues are not supported by trimming. Exclude this track or use a source without timed inline markup.",
                ));
            }
            let mut clipped = cue.clone();
            // Respect the source text format's timestamp precision. ASS is 10ms;
            // SubRip/WebVTT are 1ms. This cannot alter the video frame interval.
            clipped.start = timestamp(&clock(
                cue.start.max(interval.start) - interval.start,
                self.format,
            ))?;
            clipped.end = timestamp(&clock(
                cue.end.min(interval.end) - interval.start,
                self.format,
            ))?;
            if clipped.end <= clipped.start {
                return Err(unsupported(
                    "A subtitle overlap is shorter than its format's timestamp precision. Move the boundary or exclude the track.",
                ));
            }
            if self.format == "ass" {
                clipped.fields[1] = clock(clipped.start, "ass");
                clipped.fields[2] = clock(clipped.end, "ass");
            }
            result.cues.push(clipped);
        }
        Ok(result)
    }

    pub fn render(&self) -> String {
        let mut output = self.header.clone();
        for (index, cue) in self.cues.iter().enumerate() {
            if self.format == "ass" {
                output.push_str(&format!("Dialogue: {}\n", cue.fields.join(",")));
            } else {
                output.push_str(&format!(
                    "{}\n{} --> {}{}{}\n{}\n\n",
                    index + 1,
                    clock(cue.start, self.format),
                    clock(cue.end, self.format),
                    if cue.settings.is_empty() { "" } else { " " },
                    cue.settings,
                    cue.body
                ));
            }
        }
        output
    }

    pub fn matches(&self, actual: &Self) -> bool {
        self.format == actual.format && self.header == actual.header && self.cues == actual.cues
    }
}

fn has_inline_timestamp(body: &str) -> bool {
    body.split('<')
        .skip(1)
        .any(|tag| tag.starts_with(|c: char| c.is_ascii_digit()) && tag.contains(':'))
}

pub(crate) async fn read(
    ffmpeg: &Path,
    source: &Path,
    index: u32,
    codec: &str,
    cancel: &watch::Receiver<bool>,
) -> Result<Text, AppError> {
    let format = match codec {
        "ass" => "ass",
        "subrip" | "mov_text" => "srt",
        "webvtt" => "webvtt",
        _ => return Err(unsupported("Unsupported subtitle format.")),
    };
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-protocol_whitelist",
        "file",
        "-copyts",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(source.as_os_str().to_owned());
    args.extend([
        "-map".into(),
        format!("0:{index}").into(),
        "-c:s".into(),
        if codec == "mov_text" {
            "srt".into()
        } else {
            "copy".into()
        },
        "-avoid_negative_ts".into(),
        "disabled".into(),
        "-f".into(),
        format.into(),
        "pipe:1".into(),
    ]);
    let capture = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        32 * 1024 * 1024,
        Duration::from_secs(30),
    )
    .await
    .map_err(|error| super::super::process_error(error, source))?;
    if !capture.status.success() {
        return Err(unsupported(format!(
            "Subtitle timing could not be exported: {}",
            String::from_utf8_lossy(&capture.stderr)
        )));
    }
    let text = std::str::from_utf8(&capture.stdout)
        .map_err(|_| unsupported("Trimming requires UTF-8 text subtitles."))?;
    Text::parse(format, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifted_ass_round_trips_its_rendered_event_timestamps() {
        let source = Text::parse("ass", "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.10,0:00:01.20,Default,,0,0,0,,Caption\n").unwrap();
        let shifted = source.shifted(0.255).unwrap().visible().unwrap();
        let actual = Text::parse("ass", &shifted.render()).unwrap();
        assert!(shifted.matches(&actual));
        assert_eq!(actual.cues[0].start, 0.36);
        assert_eq!(actual.cues[0].end, 1.46);
    }

    #[test]
    fn negative_text_offset_keeps_only_the_visible_cue_interval() {
        let source = Text::parse("srt", "1\n00:00:00,100 --> 00:00:01,200\nCaption\n\n2\n00:00:02,000 --> 00:00:03,000\nLater\n\n").unwrap();
        let shifted = source.shifted(-0.5).unwrap().visible().unwrap();
        assert_eq!((shifted.cues[0].start, shifted.cues[0].end), (0.0, 0.7));
        assert_eq!((shifted.cues[1].start, shifted.cues[1].end), (1.5, 2.5));
        assert!(shifted.matches(&Text::parse("srt", &shifted.render()).unwrap()));
    }

    #[test]
    fn offset_rejects_unshifted_absolute_webvtt_inline_timestamps() {
        let source = Text::parse(
            "webvtt",
            "WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nBefore <00:00:02.000>after\n\n",
        )
        .unwrap();
        assert!(source.shifted(0.0).is_ok());
        assert!(source.shifted(0.5).is_err());
        assert!(source.shifted(-0.5).is_err());
    }
}
