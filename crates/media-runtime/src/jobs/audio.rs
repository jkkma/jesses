//! Audio overrides are tied to source indices; encoder arguments use output
//! positions. Converted audio is checked using decoded samples, after codec
//! delay/pre-skip has been applied by the decoder, rather than packet counts.
use media_core::{AudioChannels, AudioCodec, AudioTrackSettings};
use serde::Deserialize;

use super::*;

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("AUDIO_SETTINGS_INVALID", message, None)
}

pub(super) fn codec_name(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Copy => "copy",
        AudioCodec::Aac => "aac",
        AudioCodec::Opus => "opus",
        AudioCodec::Flac => "flac",
        AudioCodec::Mp3 => "mp3",
        AudioCodec::Vorbis => "vorbis",
        AudioCodec::Eac3 => "eac3",
    }
}

fn encoder_name(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Opus => "libopus",
        AudioCodec::Mp3 => "libmp3lame",
        AudioCodec::Vorbis => "libvorbis",
        codec => codec_name(codec),
    }
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
        let maximum = if track.codec == AudioCodec::Eac3 {
            6144
        } else {
            512
        };
        if track.codec != AudioCodec::Flac && !(32..=maximum).contains(&track.bitrate_kbps) {
            return Err(invalid(format!(
                "Audio bitrate must be an integer from 32 to {maximum} kb/s."
            )));
        }
        if track.codec == AudioCodec::Copy && track.channels != AudioChannels::Preserve {
            return Err(invalid("Copy audio requires the original channel layout."));
        }
        if let Some(gain) = &track.gain {
            if track.codec == AudioCodec::Copy {
                return Err(invalid(
                    "Gain requires audio conversion. Copy preserves the original audio.",
                ));
            }
            if !(-600..=240).contains(&gain.tenths_db) {
                return Err(invalid(
                    "Audio gain must be between -60 and +24 dB in 0.1 dB steps.",
                ));
            }
            if gain.source_fingerprint.as_ref().is_some_and(|fingerprint| {
                fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
            }) {
                return Err(invalid(
                    "The loudness measurement source identity is invalid.",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_gain_source(
    source: &Source,
    settings: &EncodeSettings,
) -> Result<(), AppError> {
    let measured: Vec<_> = settings
        .audio
        .iter()
        .filter_map(|track| track.gain.as_ref()?.source_fingerprint.as_ref())
        .collect();
    if measured.is_empty() {
        return Ok(());
    }
    let current = crate::analysis::fingerprint(source)?;
    if measured.iter().any(|identity| **identity != current) {
        return Err(AppError::new(
            "SOURCE_CHANGED",
            "The source changed since loudness measurement. Measure again or set an explicit manual gain.",
            Some(source.path.to_string_lossy().into_owned()),
        ));
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
        let valid_rate = match track.codec {
            AudioCodec::Mp3 => {
                [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000].contains(&rate)
            }
            AudioCodec::Eac3 => [32000, 44100, 48000].contains(&rate),
            AudioCodec::Vorbis => (8000..=192000).contains(&rate),
            _ => true,
        };
        if !valid_rate {
            return Err(invalid(format!(
                "{} cannot retain this source sample rate ({rate} Hz). Choose another codec; Opus explicitly converts to 48,000 Hz.",
                codec_name(track.codec)
            )));
        }
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
            AudioCodec::Mp3 => {
                if rate >= 32000 {
                    320
                } else {
                    160
                }
            }
            AudioCodec::Eac3 => 6144 * rate / 48000,
            AudioCodec::Flac | AudioCodec::Vorbis => 512,
            AudioCodec::Copy => unreachable!(),
        };
        if track.codec != AudioCodec::Flac && u32::from(track.bitrate_kbps) > maximum_bitrate {
            return Err(invalid(format!(
                "Audio stream {} supports at most {maximum_bitrate} kb/s with the selected codec, sample rate, and output channel count.",
                track.stream_index
            )));
        }
        if track.codec == AudioCodec::Mp3 {
            if output_channels > 2 {
                return Err(invalid(
                    "MP3 supports mono or stereo. Choose an explicit downmix instead of preserving multichannel audio.",
                ));
            }
            let allowed: &[u16] = if rate >= 32000 {
                &[
                    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
                ]
            } else {
                &[32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]
            };
            if !allowed.contains(&track.bitrate_kbps) {
                return Err(invalid(
                    "MP3 requires one of the displayed standard bitrates for the source sample rate; intermediate values would be rounded by the encoder.",
                ));
            }
        }
        if source.channels.is_some_and(|channels| channels <= 2)
            && source.channel_layout.as_deref().is_some_and(|layout| {
                layout
                    != if source.channels == Some(1) {
                        "mono"
                    } else {
                        "stereo"
                    }
            })
        {
            return Err(invalid(
                "Audio conversion requires a standard mono or stereo layout for sources with one or two channels.",
            ));
        }
        if source.channels.is_some_and(|channels| channels > 2) {
            let layout_channels = match source.channel_layout.as_deref() {
                Some("3.0") => Some(3),
                Some("quad" | "quad(side)" | "4.0") => Some(4),
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
            let unsupported_layout = match track.codec {
                AudioCodec::Opus | AudioCodec::Vorbis => matches!(
                    source.channel_layout.as_deref(),
                    Some("4.0" | "quad(side)" | "5.0(side)" | "5.1(side)")
                ),
                AudioCodec::Flac => {
                    matches!(source.channel_layout.as_deref(), Some("4.0" | "quad(side)"))
                }
                AudioCodec::Eac3 => matches!(
                    source.channel_layout.as_deref(),
                    Some("quad" | "5.0" | "5.1" | "6.1" | "7.1")
                ),
                _ => false,
            };
            if track.channels == AudioChannels::Preserve && unsupported_layout {
                return Err(invalid(format!(
                    "{} cannot preserve the speaker layout of audio stream {}. Choose an explicit mono or stereo downmix, another codec, or copy.",
                    codec_name(track.codec),
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
    if track.codec == AudioCodec::Copy {
        return;
    }
    let encoder = encoder_name(track.codec);
    let mut filter = String::from("asetpts=N/SR/TB+STARTPTS");
    if let Some(gain) = &track.gain {
        filter.push_str(&format!(
            ",volume={:.1}dB:precision=double",
            f64::from(gain.tenths_db) / 10.0
        ));
    }
    args.extend([
        format!("-c:{position}").into(),
        encoder.into(),
        format!("-ar:{position}").into(),
        expected_rate(source, track)
            .expect("validated audio rate")
            .into(),
        // The source scan has bounded authored timestamp quantization. Anchor
        // every converted frame to its decoded sample clock, preserving the
        // first decoded timestamp and every sample. No async resampling,
        // silence insertion, or dropping is used to conceal timestamp gaps.
        format!("-filter:{position}").into(),
        filter.into(),
    ]);
    if track.codec == AudioCodec::Flac {
        args.extend([
            format!("-sample_fmt:{position}").into(),
            "s32".into(),
            format!("-bits_per_raw_sample:{position}").into(),
            "24".into(),
        ]);
    } else {
        args.extend([
            format!("-b:{position}").into(),
            format!("{}k", track.bitrate_kbps).into(),
        ]);
    }
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
        let name = encoder_name(track.codec);
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

/// Exercise the exact additional codec/rate/layout/bitrate combination before
/// touching user audio or starting video encoding. Encoder listings alone do
/// not expose Vorbis rate limits or Matroska delay/discard-padding support.
pub(super) async fn check_conversion_support(
    ffmpeg: &Path,
    ffprobe: &Path,
    temporary: &Temporary,
    source: &metadata::Stream,
    track: &AudioTrackSettings,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let fail = || {
        AppError::new(
            "AUDIO_TOOL_UNSUPPORTED",
            format!(
                "The installed FFmpeg/FFprobe pair cannot preserve {} audio with this sample rate, speaker layout, and bitrate in Matroska. Choose supported settings or install a matching recent tool pair.",
                codec_name(track.codec)
            ),
            None,
        )
    };
    let rate = sample_rate(source)?;
    let layout = match source.channels {
        Some(1) => "mono",
        Some(2) => "stereo",
        _ => source.channel_layout.as_deref().expect("validated layout"),
    };
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(format!("anullsrc=r={rate}:cl={layout}:d=1").into());
    append_arguments(&mut args, 0, track, source);
    args.extend(
        [
            "-copyts",
            "-avoid_negative_ts",
            "disabled",
            "-f",
            "matroska",
            "-y",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args.push(temporary.path.as_os_str().to_owned());
    let encoded = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(15),
    )
    .await
    .map_err(|error| process_error(error, ffmpeg))?;
    if !encoded.status.success() {
        return Err(fail());
    }
    temporary.flush_nonempty_async().await?;
    let document = probe(ffprobe, &temporary.path, cancel, None).await?;
    let stream = document.streams.first().ok_or_else(fail)?;
    if document.streams.len() != 1
        || stream.codec_name.as_deref() != Some(codec_name(track.codec))
        || stream.sample_rate != expected_rate(source, track)
        || stream.channels != expected_channels(source, track)
        || (track.channels == AudioChannels::Preserve
            && source.channels.is_some_and(|channels| channels > 2)
            && stream.channel_layout != source.channel_layout)
        || (track.codec == AudioCodec::Flac && stream.bits_per_raw_sample.as_deref() != Some("24"))
    {
        return Err(fail());
    }
    let decoded = scan(ffprobe, &temporary.path, stream, true, cancel).await?;
    let original = Timeline {
        rate,
        samples: u64::from(rate),
        ..Default::default()
    };
    verify_timeline(&original, &decoded, track.codec).map_err(|_| fail())?;
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
        8 * 1024 * 1024,
        Duration::from_secs(15),
    )
    .await
    .map_err(|error| process_error(error, ffmpeg))?;
    if !pcm.status.success()
        || !pcm.stderr.is_empty()
        || pcm.stdout.len() as u64 != decoded.samples * u64::from(stream.channels.unwrap()) * 2
    {
        return Err(fail());
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

#[derive(Debug, Default, Clone)]
pub(super) struct Timeline {
    start: f64,
    samples: u64,
    rate: u32,
    tolerance: f64,
    minimum_residual: f64,
    maximum_residual: f64,
}

impl Timeline {
    /// Container edit lists quantize codec delay to their source time base. This
    /// bound applies only to copying a verified stage into a different container.
    pub(super) fn verify_container(
        &self,
        output: &Self,
        time_base: f64,
        aac_last_duration: Option<f64>,
    ) -> Result<(), AppError> {
        let tolerance = time_base.max(1.0 / f64::from(self.rate)).min(0.002);
        // Matroska AAC can decode a complete final frame despite a shorter
        // packet duration. MP4 applies that already-declared duration as a
        // discard boundary. Permit only the padding proven by that last packet.
        let declared_padding = aac_last_duration
            .filter(|duration| *duration > 0.0)
            .map(|duration| {
                (1024.0 / f64::from(self.rate) - duration).clamp(0.0, 1023.0 / f64::from(self.rate))
            })
            .unwrap_or(0.0);
        let source_end = self.start + self.samples as f64 / f64::from(self.rate);
        let output_end = output.start + output.samples as f64 / f64::from(output.rate);
        if self.rate != output.rate
            || (self.start - output.start).abs() > tolerance + 0.000001
            || output_end - source_end > tolerance + 0.000001
            || source_end - output_end > declared_padding + tolerance + 0.000001
            || (output.samples as f64 - self.samples as f64) / f64::from(self.rate)
                > tolerance + 0.000001
            || (self.samples as f64 - output.samples as f64) / f64::from(self.rate)
                > declared_padding + tolerance + 0.000001
        {
            return Err(AppError::new(
                "AUDIO_VALIDATION_FAILED",
                format!(
                    "The final container changed decoded audio beyond one source time-base tick (at most 2 ms): start {} -> {}, samples {} -> {}, end {} -> {}.",
                    self.start, output.start, self.samples, output.samples, source_end, output_end
                ),
                None,
            ));
        }
        Ok(())
    }

    /// Pick samples whose source presentation times lie in [start, end).
    /// Fractional video boundaries round upward on the audio sample clock.
    pub(super) fn clipped(&self, start: f64, end: f64) -> Result<(Self, String), AppError> {
        let sample = |time: f64| {
            (((time - self.start) * f64::from(self.rate) - 0.0000001)
                .ceil()
                .max(0.0) as u64)
                .min(self.samples)
        };
        let first = sample(start);
        let last = sample(end);
        if first >= last {
            return Err(AppError::new(
                "TRIM_UNSUPPORTED",
                "The selected audio track has no samples inside this frame interval. Exclude the track or select a wider interval.",
                None,
            ));
        }
        let output_start = (self.start + first as f64 / f64::from(self.rate) - start).max(0.0);
        let mut clipped = self.clone();
        clipped.start = output_start;
        clipped.samples = last - first;
        Ok((
            clipped,
            format!(
                "atrim=start_sample={first}:end_sample={last},asetpts=N/SR/TB+{output_start:.12}/TB"
            ),
        ))
    }

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
            gain: None,
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
        settings.backend = media_core::EncodeBackend::Av1an;
        validate_settings(&settings).unwrap();
        settings.audio[0] = track(AudioCodec::Copy);
        validate_settings(&settings).unwrap();
    }

    #[test]
    fn additional_codecs_reject_implicit_downmix_layout_changes_and_bitrate_rounding() {
        for (codec, channels, layout) in [
            (AudioCodec::Mp3, 6, "5.1"),
            (AudioCodec::Vorbis, 4, "4.0"),
            (AudioCodec::Vorbis, 6, "5.1(side)"),
            (AudioCodec::Flac, 4, "4.0"),
            (AudioCodec::Eac3, 6, "5.1"),
            (AudioCodec::Eac3, 8, "7.1"),
        ] {
            let mut source = source();
            source.streams[1].channels = Some(channels);
            source.streams[1].channel_layout = Some(layout.into());
            let mut settings = EncodeSettings {
                audio: vec![track(codec)],
                ..Default::default()
            };
            let selected = source.selected(&[0, 1]).unwrap();
            assert!(
                validate_selection(&selected, &settings).is_err(),
                "{codec:?} must not silently change {layout}"
            );
            settings.audio[0].channels = AudioChannels::Stereo;
            validate_selection(&selected, &settings).unwrap();
        }
        let mut source = source();
        let mut settings = EncodeSettings {
            audio: vec![track(AudioCodec::Mp3)],
            ..Default::default()
        };
        settings.audio[0].bitrate_kbps = 129;
        assert!(validate_selection(&source.selected(&[0, 1]).unwrap(), &settings).is_err());
        settings.audio[0].bitrate_kbps = 320;
        source.streams[1].sample_rate = Some("22050".into());
        assert!(validate_selection(&source.selected(&[0, 1]).unwrap(), &settings).is_err());
        settings.audio[0].bitrate_kbps = 144;
        validate_selection(&source.selected(&[0, 1]).unwrap(), &settings).unwrap();
        settings.audio[0].codec = AudioCodec::Eac3;
        assert!(validate_selection(&source.selected(&[0, 1]).unwrap(), &settings).is_err());
        source.streams[1].sample_rate = Some("32000".into());
        settings.audio[0].bitrate_kbps = 4096;
        validate_selection(&source.selected(&[0, 1]).unwrap(), &settings).unwrap();
        settings.audio[0].bitrate_kbps = 4097;
        assert!(validate_selection(&source.selected(&[0, 1]).unwrap(), &settings).is_err());
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
