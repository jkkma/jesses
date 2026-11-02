//! Source Quick Copy uses one original-input FFmpeg mux with input-side seeks.
//! It retains all selected streams/order/offsets and does not first cut a
//! different video or normalize the sources into a second media timeline.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Window {
    pub start: f64,
    pub end: f64,
}
impl Window {
    pub fn new(start: f64, end: f64) -> Result<Self, AppError> {
        let start = (start * 1000.).round_ties_even() / 1000.;
        let end = (end * 1000.).round_ties_even() / 1000.;
        if !start.is_finite() || !end.is_finite() || start < 0. || end <= start {
            return Err(AppError::new(
                "TRIM_UNSUPPORTED",
                "The copied video section must have finite, increasing time boundaries.",
                None,
            ));
        }
        Ok(Self { start, end })
    }
}

pub(super) fn arguments(
    input: &Path,
    output: &Path,
    selected: &[&metadata::Stream],
    settings: &EncodeSettings,
    external: &external_tracks::ExternalTracks,
    window: Window,
    format: media_core::ContainerFormat,
) -> Vec<OsString> {
    let mut args = conversion::source_arguments(input, output, selected, settings, external);
    apply_seek(&mut args, window);
    // Replace the ordinary private-stage format with the actual destination
    // format. Publication owns this sibling, and validation sees final bytes.
    for key in ["-f", "-movflags", "-write_tmcd"] {
        while let Some(index) = args.iter().position(|value| value == key) {
            args.drain(index..index + 2);
        }
    }
    let output = args.pop().expect("source-copy output");
    container::append_format_options(&mut args, format, selected);
    if format == media_core::ContainerFormat::Mp4
        && settings
            .encoding_preferences
            .is_some_and(|preferences| preferences.mp4_faststart)
    {
        let index = args
            .iter()
            .position(|value| value == "-movflags")
            .expect("ISO options");
        args[index + 1] = format!("{}+faststart", args[index + 1].to_string_lossy()).into();
    }
    args.extend(["-f".into(), format.ffmpeg_format().into(), output]);
    args
}

pub(super) fn apply_seek(args: &mut Vec<OsString>, window: Window) {
    args.retain(|value| value != "-copyts");
    while let Some(index) = args.iter().position(|value| value == "-avoid_negative_ts") {
        args.drain(index..index + 2);
    }
    let positions = args
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (value == "-i").then_some(index))
        .collect::<Vec<_>>();
    for index in positions.into_iter().rev() {
        args.splice(
            index..index,
            ["-ss".into(), format!("{:.3}", window.start).into()],
        );
    }
    let last = args.len() - 1;
    args.splice(
        last..last,
        [
            "-t".into(),
            format!("{:.3}", window.end - window.start).into(),
        ],
    );
}

pub(super) fn append_subtitle_codecs(
    args: &mut Vec<OsString>,
    selected: &[&metadata::Stream],
    settings: &EncodeSettings,
) {
    let output = args.pop().expect("source-copy output");
    for (position, stream) in selected.iter().enumerate() {
        if let Some(track) = settings
            .subtitles
            .iter()
            .find(|track| track.stream_index == stream.index)
        {
            let codec = match track.mode {
                media_core::SubtitleMode::Copy => continue,
                media_core::SubtitleMode::Ass => "ass",
                media_core::SubtitleMode::SubRip => "subrip",
                media_core::SubtitleMode::WebVtt => "webvtt",
                media_core::SubtitleMode::MovText => "mov_text",
                media_core::SubtitleMode::BurnIn => unreachable!("copy-video cannot burn captions"),
            };
            args.extend([format!("-c:{position}").into(), codec.into()]);
            for key in stream
                .tags
                .keys()
                .filter(|key| metadata::is_derived_stream_tag(key))
            {
                args.extend([
                    format!("-metadata:s:{position}").into(),
                    format!("{key}=").into(),
                ]);
            }
        }
    }
    args.push(output);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_inputs_keep_offsets_and_only_receive_source_input_seek() {
        let mut args = [
            "-copyts",
            "-i",
            "main.mkv",
            "-itsoffset",
            "0.250",
            "-i",
            "other.mkv",
            "-map",
            "0:2",
            "-map",
            "1:0",
            "-avoid_negative_ts",
            "disabled",
            "-c",
            "copy",
            "out.mov",
        ]
        .map(OsString::from)
        .to_vec();
        apply_seek(&mut args, Window::new(1.234, 3.567).unwrap());
        assert_eq!(
            args,
            [
                "-ss",
                "1.234",
                "-i",
                "main.mkv",
                "-itsoffset",
                "0.250",
                "-ss",
                "1.234",
                "-i",
                "other.mkv",
                "-map",
                "0:2",
                "-map",
                "1:0",
                "-c",
                "copy",
                "-t",
                "2.333",
                "out.mov"
            ]
            .map(OsString::from)
        );
        assert!(Window::new(f64::NAN, 1.).is_err());
        assert!(Window::new(1., 1.).is_err());
    }
}
