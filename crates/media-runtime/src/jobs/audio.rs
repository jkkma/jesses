//! Audio overrides are tied to source indices; encoder arguments use output
//! positions. Converted audio is checked using decoded samples, after codec
//! delay/pre-skip has been applied by the decoder, rather than packet counts.
use media_core::{AudioChannels, AudioCodec, AudioTrackSettings, EncodeBackend};
use serde::Deserialize;

use super::*;

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("AUDIO_SETTINGS_INVALID", message, None)
}

pub(super) fn converted(settings: &EncodeSettings) -> impl Iterator<Item = &AudioTrackSettings> {
    settings
        .audio
        .iter()
        .filter(|track| track.codec != AudioCodec::Copy)
}

pub(super) fn validate_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    let mut indices = HashSet::new();
    for track in &settings.audio {
        if !indices.insert(track.stream_index) {
            return Err(invalid(
                "Audio settings contain duplicate source stream indices.",
            ));
        }
        if !(32..=512).contains(&track.bitrate_kbps) {
            return Err(invalid(
                "Audio bitrate must be an integer from 32 to 512 kb/s.",
            ));
        }
        if track.codec == AudioCodec::Copy && track.channels != AudioChannels::Preserve {
            return Err(invalid("Copy audio requires the original channel layout."));
        }
        if settings.backend == EncodeBackend::Av1an && track.codec != AudioCodec::Copy {
            return Err(invalid(
                "Audio conversion is available in the standalone workflow. av1an currently copies selected audio tracks.",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_selection(
    selected: &[&metadata::Stream],
    settings: &EncodeSettings,
) -> Result<(), AppError> {
    for track in &settings.audio {
        let source = selected
            .iter()
            .find(|stream| stream.index == track.stream_index)
            .ok_or_else(|| invalid("Audio settings refer to a stream that is not selected."))?;
        if source.codec_type.as_deref() != Some("audio") {
            return Err(invalid(
                "Audio settings must refer to selected audio streams.",
            ));
        }
        if track.codec == AudioCodec::Copy {
            continue;
        }
        let rate = sample_rate(source)?;
        if track.codec == AudioCodec::Aac
            && ![
                7350, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200,
                96000,
            ]
            .contains(&rate)
        {
            return Err(invalid(
                "AAC requires a supported source sample rate between 7,350 and 96,000 Hz. Use Opus for conversion to 48,000 Hz.",
            ));
        }
        if !source
            .channels
            .is_some_and(|channels| (1..=8).contains(&channels))
        {
            return Err(invalid(
                "Audio conversion requires a known source channel count from 1 to 8.",
            ));
        }
        let output_channels = expected_channels(source, track).expect("known source channel count");
        let maximum_bitrate = match track.codec {
            AudioCodec::Aac => (rate * output_channels * 6 / 1000).min(512),
            AudioCodec::Opus => (output_channels * 256).min(512),
            AudioCodec::Copy => unreachable!(),
        };
        if u32::from(track.bitrate_kbps) > maximum_bitrate {
            return Err(invalid(format!(
                "Audio stream {} supports at most {maximum_bitrate} kb/s with the selected codec, sample rate, and output channel count.",
                track.stream_index
            )));
        }
        if source.channels.is_some_and(|channels| channels > 2) {
            let layout_channels = match source.channel_layout.as_deref() {
                Some("3.0") => Some(3),
                Some("quad" | "4.0") => Some(4),
                Some("5.0" | "5.0(side)") => Some(5),
                Some("5.1" | "5.1(side)") => Some(6),
                Some("6.1") => Some(7),
                Some("7.1") => Some(8),
                _ => None,
            };
            if layout_channels != source.channels {
                return Err(invalid(
                    "Converting multichannel audio requires a recognized speaker layout matching its channel count.",
                ));
            }
            if track.codec == AudioCodec::Opus
                && track.channels == AudioChannels::Preserve
                && matches!(
                    source.channel_layout.as_deref(),
                    Some("4.0" | "5.0(side)" | "5.1(side)")
                )
            {
                return Err(invalid(format!(
                    "Opus cannot preserve the speaker layout of audio stream {}. Choose mono or stereo, AAC, or copy.",
                    track.stream_index
                )));
            }
        }
    }
    Ok(())
}

fn sample_rate(stream: &metadata::Stream) -> Result<u32, AppError> {
    stream
        .sample_rate
        .as_deref()
        .and_then(|rate| rate.parse::<u32>().ok())
        .filter(|rate| (1..=384_000).contains(rate))
        .ok_or_else(|| invalid("Audio conversion requires a known source sample rate."))
}

pub(super) fn expected_rate(
    source: &metadata::Stream,
    track: &AudioTrackSettings,
) -> Option<String> {
    if track.codec == AudioCodec::Opus {
        Some("48000".into())
    } else {
        source.sample_rate.clone()
    }
}

pub(super) fn expected_channels(
    source: &metadata::Stream,
    track: &AudioTrackSettings,
) -> Option<u32> {
    match track.channels {
        AudioChannels::Preserve => source.channels,
        AudioChannels::Mono => Some(1),
        AudioChannels::Stereo => Some(2),
    }
}

pub(super) fn append_arguments(
    args: &mut Vec<OsString>,
    position: usize,
    track: &AudioTrackSettings,
    source: &metadata::Stream,
) {
    let encoder = match track.codec {
        AudioCodec::Copy => return,
        AudioCodec::Opus => "libopus",
        AudioCodec::Aac => "aac",
    };
    args.extend([
        format!("-c:{position}").into(),
        encoder.into(),
        format!("-b:{position}").into(),
        format!("{}k", track.bitrate_kbps).into(),
        format!("-ar:{position}").into(),
        expected_rate(source, track)
            .expect("validated audio rate")
            .into(),
        // The source scan has bounded authored timestamp quantization. Anchor
        // every converted frame to its decoded sample clock, preserving the
        // first decoded timestamp and every sample. No async resampling,
        // silence insertion, or dropping is used to conceal timestamp gaps.
        format!("-filter:{position}").into(),
        "asetpts=N/SR/TB+STARTPTS".into(),
    ]);
    if track.codec == AudioCodec::Aac {
        args.extend([format!("-profile:{position}").into(), "aac_low".into()]);
    }
    if track.channels != AudioChannels::Preserve {
        args.extend([
            format!("-ac:{position}").into(),
            expected_channels(source, track).unwrap().to_string().into(),
        ]);
    }
}

pub(super) async fn check_encoders(
    ffmpeg: &Path,
    settings: &EncodeSettings,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    if converted(settings).next().is_none() {
        return Ok(());
    }
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args: ["-hide_banner", "-encoders"]
                .into_iter()
                .map(OsString::from)
                .collect(),
            cwd: None,
        },
        cancel.clone(),
        1024 * 1024,
        Duration::from_secs(10),
    )
    .await
    .map_err(|error| process_error(error, ffmpeg))?;
    let listing = String::from_utf8_lossy(&result.stdout);
    for track in converted(settings) {
        let name = if track.codec == AudioCodec::Opus {
            "libopus"
        } else {
            "aac"
        };
        if !result.status.success()
            || !listing
                .lines()
                .any(|line| line.split_whitespace().nth(1) == Some(name))
        {
            return Err(AppError::new(
                "AUDIO_ENCODER_MISSING",
                format!("This FFmpeg build does not provide the {name} audio encoder."),
                None,
            ));
        }
    }
    Ok(())
}

/// Version strings are not sufficient: older FFprobe builds report Matroska
/// CodecDelay but still emit the AAC priming frame as decoded audio. Exercise
/// the actual encoder/muxer/decoder pair before decoding user audio or video.
pub(super) async fn check_delay_support(
    ffmpeg: &Path,
    ffprobe: &Path,
    temporary: &Temporary,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let unsupported = || {
        AppError::new(
            "AUDIO_TOOL_UNSUPPORTED",
            "This FFmpeg/FFprobe pair does not correctly preserve and decode AAC priming in Matroska. Install a matching FFmpeg and FFprobe 8.1 or newer build for audio conversion; copying audio remains available.",
            None,
        )
    };
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=48000:cl=mono",
        "-t",
        "0.05",
        "-c:a",
        "aac",
        "-profile:a",
        "aac_low",
        "-b:a",
        "32k",
        "-copyts",
        "-avoid_negative_ts",
        "disabled",
        "-f",
        "matroska",
        "-y",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(temporary.path.as_os_str().to_owned());
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(10),
    )
    .await
    .map_err(|error| process_error(error, ffmpeg))?;
    if !result.status.success() {
        return Err(unsupported());
    }
    temporary.flush_nonempty_async().await?;
    let document = probe(ffprobe, &temporary.path, cancel, None).await?;
    let stream = document.streams.first().ok_or_else(unsupported)?;
    let decoded = scan(ffprobe, &temporary.path, stream, true, cancel).await?;
    if decoded.start.abs() > 0.002
        || !(2400..=3423).contains(&decoded.samples)
        || decoded.rate != 48_000
    {
        return Err(unsupported());
    }
    // FFmpeg decodes the user's source during muxing, while FFprobe validates
    // it. Check both tools: a new probe paired with an older FFmpeg can still
    // encode priming as audible samples. The fixture is mono 16-bit PCM, so
    // byte count is independent evidence of FFmpeg's decoded sample count.
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-err_detect",
        "explode",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(temporary.path.as_os_str().to_owned());
    args.extend(
        [
            "-map",
            "0:a:0",
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
            "pipe:1",
        ]
        .into_iter()
        .map(OsString::from),
    );
    let pcm = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        16 * 1024,
        Duration::from_secs(10),
    )
    .await
    .map_err(|error| process_error(error, ffmpeg))?;
    if !pcm.status.success()
        || !pcm.stderr.is_empty()
        || pcm.stdout.len() as u64 != decoded.samples * 2
    {
        return Err(unsupported());
    }
    Ok(())
}

#[derive(Default, Deserialize)]
struct Frame {
    best_effort_timestamp_time: Option<String>,
    nb_samples: Option<u32>,
    sample_rate: Option<serde_json::Value>,
    channels: Option<u32>,
    channel_layout: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct Timeline {
    start: f64,
    samples: u64,
    rate: u32,
    tolerance: f64,
    minimum_residual: f64,
    maximum_residual: f64,
}

impl Timeline {
    fn push(&mut self, frame: Frame) -> Result<(), String> {
        if let Some(value) = frame.sample_rate {
            let rate = value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()));
            if rate != Some(u64::from(self.rate)) {
                return Err("Decoded audio changed sample rate.".into());
            }
        }
        let timestamp = frame
            .best_effort_timestamp_time
            .and_then(|time| time.parse::<f64>().ok())
            .filter(|time| time.is_finite())
            .ok_or("A decoded audio frame has no valid timestamp.")?;
        let samples = frame
            .nb_samples
            .filter(|samples| *samples > 0)
            .ok_or("A decoded audio frame has no samples.")?;
        if self.samples == 0 {
            self.start = timestamp;
        }
        let residual = timestamp - self.start - self.samples as f64 / self.rate as f64;
        self.minimum_residual = self.minimum_residual.min(residual);
        self.maximum_residual = self.maximum_residual.max(residual);
        if residual.abs() > self.tolerance {
            return Err(
                "Decoded audio contains a timestamp gap, overlap, or changing sample rate.".into(),
            );
        }
        self.samples = self
            .samples
            .checked_add(u64::from(samples))
            .ok_or("Decoded audio sample count overflowed.")?;
        Ok(())
    }
}

pub(super) async fn scan(
    ffprobe: &Path,
    input: &Path,
    stream: &metadata::Stream,
    encoded: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<Timeline, AppError> {
    let mut timeline = Timeline {
        start: 0.0,
        samples: 0,
        rate: sample_rate(stream)?,
        ..Default::default()
    };
    // Imported containers can quantize the original timestamp, decoded
    // timestamp, and first-frame origin separately. Bound this source-only
    // uncertainty to three ticks (at most 1 ms per tick) plus one sample.
    // Our encoder's decoded output keeps the stricter 2 ms continuity bound.
    let tick = stream
        .time_base
        .as_deref()
        .and_then(|base| base.split_once('/'))
        .and_then(|(num, den)| Some((num.parse::<f64>().ok()?, den.parse::<f64>().ok()?)))
        .map(|(num, den)| num / den)
        .filter(|tick| tick.is_finite() && *tick > 0.0)
        .unwrap_or(0.000001)
        .min(0.001);
    timeline.tolerance = if encoded {
        0.002
    } else {
        (3.0 * tick + 1.0 / f64::from(timeline.rate)).max(0.002)
    };
    let channels = stream.channels;
    let layout = stream.channel_layout.clone();
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-err_detect",
        "explode",
        "-select_streams",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(stream.index.to_string().into());
    args.extend(
        [
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp_time,nb_samples,sample_rate,channels,channel_layout",
            "-of",
            "json",
            "-i",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args.push(input.as_os_str().to_owned());
    let output = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(24 * 60 * 60),
        move |reader| {
            let mut failure = None;
            super::encode::frame_scan::parse(reader, |frame: Frame| {
                if failure.is_none() {
                    if frame
                        .channels
                        .is_some_and(|actual| Some(actual) != channels)
                        || frame.channel_layout.as_ref().is_some_and(|actual| {
                            layout.as_ref().is_some_and(|expected| actual != expected)
                        })
                    {
                        failure = Some(
                            "Decoded audio changed its channel count or speaker layout.".into(),
                        );
                    } else {
                        failure = timeline.push(frame).err();
                    }
                }
            })?;
            if let Some(failure) = failure {
                return Err(failure);
            }
            if timeline.samples == 0 {
                return Err("The audio stream decoded to no samples.".into());
            }
            Ok(timeline)
        },
    )
    .await
    .map_err(|error| match error {
        SupervisorError::OutputParse(message) => {
            files::error("AUDIO_TIMELINE_INVALID", message, input)
        }
        error => process_error(error, input),
    })?;
    check_cancel(cancel)?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(files::error(
            "AUDIO_DECODE_FAILED",
            "The complete audio track could not be decoded without errors.",
            input,
        ));
    }
    Ok(output.value)
}

pub(super) fn verify_timeline(
    source: &Timeline,
    output: &Timeline,
    codec: AudioCodec,
) -> Result<String, AppError> {
    let fail = |message| AppError::new("AUDIO_VALIDATION_FAILED", message, None);
    // FFprobe applies codec delay and skip_samples before producing decoded
    // frames. Packet PTS can legitimately precede this audible start.
    if (source.start - output.start).abs() > 0.002 {
        return Err(fail(
            "The decoded audio start changed, which could affect synchronization.",
        ));
    }
    let expected_samples = source.samples as f64 * output.rate as f64 / source.rate as f64;
    let padding = output.samples as f64 - expected_samples;
    // Opus pre-skip and final discard padding preserve the decoded length (plus
    // at most two resampler rounding samples). Native AAC-LC has 1024-sample
    // frames; Matroska can retain padding in its final partial frame only.
    let maximum_padding = if codec == AudioCodec::Aac {
        1023.0
    } else {
        2.0
    };
    if padding < -2.0 || padding > maximum_padding + 2.0 {
        return Err(fail(
            "The decoded audio sample count changed beyond the codec's final-frame padding.",
        ));
    }
    Ok(format!(
        "Validated audio: {} source samples at {} Hz, {} output samples at {} Hz, start {:.6} s, final padding {:.0} samples. Source continuity bound {:.3} ms, observed residual {:.3} to {:.3} ms; output residual {:.3} to {:.3} ms.",
        source.samples,
        source.rate,
        output.samples,
        output.rate,
        output.start,
        padding,
        source.tolerance * 1000.0,
        source.minimum_residual * 1000.0,
        source.maximum_residual * 1000.0,
        output.minimum_residual * 1000.0,
        output.maximum_residual * 1000.0
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track(codec: AudioCodec) -> AudioTrackSettings {
        AudioTrackSettings {
            stream_index: 1,
            codec,
            bitrate_kbps: 128,
            channels: AudioChannels::Preserve,
        }
    }
    fn source() -> Document {
        serde_json::from_value(serde_json::json!({"streams":[
            {"index":0,"codec_type":"video"},
            {"index":1,"codec_type":"audio","codec_name":"pcm_s16le","sample_rate":"44100","channels":2,"channel_layout":"stereo"},
            {"index":2,"codec_type":"audio","sample_rate":"48000","channels":6,"channel_layout":"5.1"}
        ]})).unwrap()
    }
    #[test]
    fn rejects_duplicate_wrong_unselected_and_invalid_audio_options() {
        let document = source();
        let selected = document.selected(&[0, 1]).unwrap();
        let mut settings = EncodeSettings {
            audio: vec![track(AudioCodec::Opus)],
            ..Default::default()
        };
        validate_settings(&settings).unwrap();
        validate_selection(&selected, &settings).unwrap();
        settings.audio.push(settings.audio[0].clone());
        assert!(validate_settings(&settings).is_err());
        settings.audio.pop();
        for index in [0, 2, 999] {
            settings.audio[0].stream_index = index;
            assert!(validate_selection(&selected, &settings).is_err());
        }
        settings.audio[0].stream_index = 1;
        for bitrate in [0, 31, 513, u16::MAX] {
            settings.audio[0].bitrate_kbps = bitrate;
            assert!(validate_settings(&settings).is_err());
        }
        settings.audio[0] = track(AudioCodec::Copy);
        settings.audio[0].channels = AudioChannels::Stereo;
        assert!(validate_settings(&settings).is_err());
        settings.audio[0] = track(AudioCodec::Opus);
        settings.backend = EncodeBackend::Av1an;
        assert!(validate_settings(&settings).is_err());
        settings.audio[0] = track(AudioCodec::Copy);
        validate_settings(&settings).unwrap();
    }
    #[test]
    fn requires_known_preserved_layout_and_supported_aac_sample_rate() {
        let mut document = source();
        document.streams[1].sample_rate = Some("12345".into());
        let mut settings = EncodeSettings {
            audio: vec![track(AudioCodec::Aac)],
            ..Default::default()
        };
        assert!(validate_selection(&document.selected(&[0, 1]).unwrap(), &settings).is_err());
        settings.audio[0].codec = AudioCodec::Opus;
        validate_selection(&document.selected(&[0, 1]).unwrap(), &settings).unwrap();
        document.streams[1].channels = Some(6);
        document.streams[1].channel_layout = None;
        assert!(validate_selection(&document.selected(&[0, 1]).unwrap(), &settings).is_err());
        settings.audio[0].channels = AudioChannels::Stereo;
        assert!(validate_selection(&document.selected(&[0, 1]).unwrap(), &settings).is_err());
        document.streams[1].channel_layout = Some("5.1".into());
        validate_selection(&document.selected(&[0, 1]).unwrap(), &settings).unwrap();
    }
    #[test]
    fn rejects_silently_clamped_bitrates_and_opus_layouts_before_encoding() {
        let mut document = source();
        let mut settings = EncodeSettings {
            audio: vec![track(AudioCodec::Aac)],
            ..Default::default()
        };
        document.streams[1].sample_rate = Some("8000".into());
        document.streams[1].channels = Some(1);
        document.streams[1].channel_layout = Some("mono".into());
        assert!(
            validate_selection(&document.selected(&[0, 1]).unwrap(), &settings)
                .unwrap_err()
                .message
                .contains("48 kb/s")
        );
        settings.audio[0].bitrate_kbps = 48;
        validate_selection(&document.selected(&[0, 1]).unwrap(), &settings).unwrap();
        settings.audio[0].codec = AudioCodec::Opus;
        settings.audio[0].bitrate_kbps = 257;
        assert!(
            validate_selection(&document.selected(&[0, 1]).unwrap(), &settings)
                .unwrap_err()
                .message
                .contains("256 kb/s")
        );
        settings.audio[0].bitrate_kbps = 128;
        settings.audio[0].stream_index = 2;
        document.streams[2].channel_layout = Some("5.1(side)".into());
        assert!(validate_selection(&document.selected(&[0, 2]).unwrap(), &settings).is_err());
        settings.audio[0].channels = AudioChannels::Stereo;
        validate_selection(&document.selected(&[0, 2]).unwrap(), &settings).unwrap();
    }
    #[test]
    fn timing_rejects_shift_truncation_extra_frames_and_internal_gaps() {
        let source = Timeline {
            start: 0.012,
            samples: 44_100,
            rate: 44_100,
            ..Default::default()
        };
        let mut output = Timeline {
            start: 0.012,
            samples: 48_000,
            rate: 48_000,
            ..Default::default()
        };
        verify_timeline(&source, &output, AudioCodec::Opus).unwrap();
        output.start += 0.0065;
        assert!(verify_timeline(&source, &output, AudioCodec::Opus).is_err());
        output.start = source.start;
        output.samples = 47_900;
        assert!(verify_timeline(&source, &output, AudioCodec::Aac).is_err());
        output.samples = 48_700;
        verify_timeline(&source, &output, AudioCodec::Aac).unwrap();
        assert!(verify_timeline(&source, &output, AudioCodec::Opus).is_err());
        output.samples = 49_100;
        assert!(verify_timeline(&source, &output, AudioCodec::Aac).is_err());
        let mut timeline = Timeline {
            start: 0.0,
            samples: 0,
            rate: 48_000,
            ..Default::default()
        };
        timeline
            .push(Frame {
                best_effort_timestamp_time: Some("0.012".into()),
                nb_samples: Some(960),
                ..Default::default()
            })
            .unwrap();
        assert!(
            timeline
                .push(Frame {
                    best_effort_timestamp_time: Some("0.052".into()),
                    nb_samples: Some(960),
                    ..Default::default()
                })
                .is_err()
        );
    }
}
