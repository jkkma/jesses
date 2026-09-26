//! Read-only conversion of historical AV1AN sidecars into typed encode requests.
//! Saved commands are parsed as data; unknown semantics reject the entire import.
use media_core::{
    AudioChannels, AudioCodec, AudioTrackSettings, Av1anChunkMethod, Av1anChunkOrder,
    Av1anConcatMethod, Av1anGrainSettings, Av1anOptions, Av1anPixelFormat, Av1anSceneDetection,
    Av1anSplitMethod, EncodeBackend, EncodeRequest, EncodeSettings, EncoderParameter, MediaFile,
    RemuxRequest, SubtitleMode, SubtitleTrackSettings, VideoEncoder,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Sidecar {
    pub file_path: String,
    file_name: String,
    temp_folder_name: String,
    args: String,
    creation_timestamp: String,
    last_run_timestamp: String,
}

pub(super) fn parse(bytes: &[u8]) -> Option<Sidecar> {
    let sidecar: Sidecar = serde_json::from_slice(bytes).ok()?;
    if sidecar.file_path.is_empty()
        || sidecar.file_name.is_empty()
        || sidecar.temp_folder_name.is_empty()
        || sidecar.creation_timestamp.is_empty()
        || sidecar.last_run_timestamp.is_empty()
        || sidecar.args.len() > 65536
    {
        return None;
    }
    Some(sidecar)
}

fn tokens(text: &str) -> Result<Vec<String>, String> {
    if text.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return Err("The saved arguments contain control characters.".into());
    }
    let mut result = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && quote.is_some() && chars.peek().copied() == quote {
            token.push(chars.next().unwrap());
            started = true;
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                token.push(c);
            }
        } else if matches!(c, '\'' | '"') {
            quote = Some(c);
            started = true;
        } else if c.is_whitespace() {
            if started {
                result.push(std::mem::take(&mut token));
            }
            started = false;
        } else {
            token.push(c);
            started = true;
        }
    }
    if quote.is_some() {
        return Err("The saved arguments have an unclosed quote.".into());
    }
    if started {
        result.push(token);
    }
    if result.len() > 512 {
        return Err("The saved argument list is too large.".into());
    }
    Ok(result)
}

fn options(text: &str, switches: &[&str]) -> Result<BTreeMap<String, String>, String> {
    let args = tokens(text)?;
    options_tokens(&args, switches)
}

fn options_tokens(args: &[String], switches: &[&str]) -> Result<BTreeMap<String, String>, String> {
    let mut output = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let (key, value) = if switches.contains(&args[i].as_str()) {
            (args[i].clone(), String::new())
        } else if let Some((key, value)) = args[i].split_once('=') {
            (key.to_owned(), value.to_owned())
        } else {
            let value = args
                .get(i + 1)
                .ok_or_else(|| format!("Missing value for {}.", args[i]))?;
            i += 1;
            (args[i - 1].clone(), value.clone())
        };
        if !key.starts_with('-') || output.insert(key.clone(), value).is_some() {
            return Err(format!("Unrecognized or repeated argument: {key}."));
        }
        i += 1;
    }
    Ok(output)
}

fn audio_options(text: &str) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let args = tokens(text)?;
    let mut ordinary = Vec::new();
    let mut maps = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-map" {
            let value = args
                .get(i + 1)
                .ok_or("The saved audio mapping has no value.")?;
            maps.push(value.clone());
            i += 2;
        } else {
            ordinary.push(args[i].clone());
            i += 1;
        }
    }
    Ok((options_tokens(&ordinary, &["-an", "-sn", "-dn"])?, maps))
}

fn take(map: &mut BTreeMap<String, String>, names: &[&str]) -> Result<Option<String>, String> {
    let values: Vec<_> = names.iter().filter_map(|name| map.remove(*name)).collect();
    if values.len() > 1 {
        return Err(format!("Conflicting aliases for {}.", names[0]));
    }
    Ok(values.into_iter().next())
}

fn number<T: std::str::FromStr>(text: String, label: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("Invalid {label}: {text}."))
}

fn same_path(left: &str, right: &str) -> bool {
    // Canonical paths also resolve case and 8.3 aliases on Windows.
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn color_matches(saved: &str, observed: Option<&str>, kind: &str) -> bool {
    let normalized = match (kind, saved) {
        (_, "1") => "bt709",
        (_, "5") => "bt470bg",
        (_, "6") => "smpte170m",
        ("primaries", "9") => "bt2020",
        ("transfer", "16") => "smpte2084",
        ("transfer", "18") => "arib-std-b67",
        ("matrix", "9") => "bt2020nc",
        (_, "bt601") => "smpte170m",
        (_, value) => value,
    };
    observed == Some(normalized)
}

impl Sidecar {
    pub(super) fn request(
        self,
        media: &MediaFile,
        svt_build: Option<VideoEncoder>,
    ) -> Result<EncodeRequest, String> {
        let mut args = options(
            &self.args,
            &["--force", "--keep", "--verbose", "-y", "-r", "--resume"],
        )?;
        // These paths describe the old process, not encode settings. Never open
        // them or grant them authority over the new job's workspace and logs.
        for flag in ["--temp", "--log-file"] {
            if args.remove(flag).is_some_and(|value| value.is_empty()) {
                return Err(format!("The saved {flag} path is empty."));
            }
        }
        let input =
            take(&mut args, &["-i", "--input"])?.ok_or("The saved command has no input path.")?;
        if !same_path(&input, &self.file_path) || !same_path(&media.path, &self.file_path) {
            return Err("The command uses a different or trimmed intermediate. Its trim cannot be inferred from this sidecar; recreate the selection from the original source. The original source and recovery files were left unchanged.".into());
        }
        let output =
            take(&mut args, &["-o", "--output"])?.ok_or("The saved command has no output path.")?;
        let encoder = match take(&mut args, &["-e", "--encoder"])?.as_deref() {
            Some("svt-av1") => svt_build.filter(|encoder| encoder.is_svt()).ok_or("Choose the original SVT build before importing this sidecar: mainline, 5fish or HDR. The sidecar itself does not identify the build.")?,
            Some("x264") => VideoEncoder::X264,
            Some("x265") => VideoEncoder::X265Standalone,
            Some("aom") => VideoEncoder::AomAv1,
            Some("vpx") => VideoEncoder::VpxStandalone,
            _ => return Err("The saved encoder is not recognized.".into()),
        };
        let video = media
            .streams
            .iter()
            .find(|stream| stream.kind == "video")
            .ok_or("The original source has no video stream.")?;
        if media
            .streams
            .iter()
            .filter(|stream| stream.kind == "video")
            .count()
            != 1
        {
            return Err(
                "This sidecar does not identify which of the source's video streams was encoded."
                    .into(),
            );
        }
        let mut settings = EncodeSettings {
            backend: EncodeBackend::Av1an,
            encoder,
            video_stream_index: video.index,
            ..Default::default()
        };
        if let Some(value) = take(&mut args, &["-w", "--workers"])? {
            settings.workers = number(value, "worker count")?;
        }
        let mut av1an = Av1anOptions::default();
        if let Some(value) = take(&mut args, &["-m", "--chunk-method"])? {
            av1an.chunk_method = match value.as_str() {
                "lsmash" => Av1anChunkMethod::Lsmash,
                "ffms2" => Av1anChunkMethod::Ffms2,
                "bestsource" => Av1anChunkMethod::Bestsource,
                "select" => Av1anChunkMethod::Select,
                "hybrid" => Av1anChunkMethod::Hybrid,
                "segment" => Av1anChunkMethod::Segment,
                _ => return Err(format!("Unsupported saved chunk method: {value}.")),
            };
        }
        if let Some(value) = take(&mut args, &["--split-method"])? {
            av1an.split_method = match value.as_str() {
                "av-scenechange" => Av1anSplitMethod::SceneDetection,
                "none" => Av1anSplitMethod::FixedChunks,
                _ => return Err(format!("Unsupported saved split method: {value}.")),
            };
        }
        if let Some(value) = take(&mut args, &["--sc-method"])? {
            av1an.scene_detection = match value.as_str() {
                "standard" => Av1anSceneDetection::Standard,
                "fast" => Av1anSceneDetection::Fast,
                _ => return Err(format!("Unsupported scene detection: {value}.")),
            };
        }
        if let Some(value) = take(&mut args, &["--chunk-order"])? {
            av1an.chunk_order = match value.as_str() {
                "long-to-short" => Av1anChunkOrder::LongToShort,
                "short-to-long" => Av1anChunkOrder::ShortToLong,
                "sequential" => Av1anChunkOrder::Sequential,
                "random" => Av1anChunkOrder::Random,
                _ => return Err(format!("Unsupported chunk order: {value}.")),
            };
        }
        if let Some(value) = take(&mut args, &["-c", "--concat"])? {
            av1an.concat_method = match value.as_str() {
                "mkvmerge" => Av1anConcatMethod::Mkvmerge,
                "ffmpeg" => Av1anConcatMethod::Ffmpeg,
                _ => return Err(format!("Unsupported concatenation: {value}.")),
            };
        }
        if let Some(value) = take(&mut args, &["--pix-format"])? {
            av1an.pixel_format = Some(match value.as_str() {
                "yuv420p" => Av1anPixelFormat::Yuv420p,
                "yuv420p10le" => Av1anPixelFormat::Yuv420p10le,
                "yuv422p" => Av1anPixelFormat::Yuv422p,
                "yuv422p10le" => Av1anPixelFormat::Yuv422p10le,
                "yuv444p" => Av1anPixelFormat::Yuv444p,
                "yuv444p10le" => Av1anPixelFormat::Yuv444p10le,
                _ => return Err(format!("Unsupported pixel format: {value}.")),
            });
        }
        if let Some(value) = take(&mut args, &["-x", "--extra-split"])? {
            av1an.maximum_chunk_frames = number(value, "maximum chunk length")?;
        }
        if let Some(value) = take(&mut args, &["--min-scene-len"])? {
            av1an.minimum_scene_frames = number(value, "minimum scene length")?;
        }
        if let Some(value) = take(&mut args, &["--sc-downscale-height"])? {
            av1an.scene_downscale_height = Some(number(value, "scene detection height")?);
        }
        if let Some(value) = take(&mut args, &["--max-tries"])? {
            av1an.max_tries = number(value, "chunk retry count")?;
        }
        let video_args = take(&mut args, &["-v", "--video-params"])?
            .ok_or("The sidecar does not record explicit encoder settings.")?;
        let mut video_args = options(
            &video_args,
            &["--no-progress", "--disable-kf", "--disable-warning-prompt"],
        )?;
        let quality = take(&mut video_args, &["--crf", "--cq-level"])?
            .ok_or("Quality targeting or unspecified quality needs manual recreation.")?;
        if encoder.is_svt() {
            let quality: f64 = number(quality, "CRF")?;
            if !quality.is_finite()
                || !(1.0..=70.0).contains(&quality)
                || (quality * 4.0).fract() != 0.0
            {
                return Err("The saved SVT CRF must use quarter steps from 1 to 70.".into());
            }
            settings.svt_crf_quarter_steps = Some((quality * 4.0) as u16);
            settings.crf = quality.floor() as u8;
        } else {
            settings.crf = number(quality, "CRF")?;
        }
        if let Some(value) = take(&mut video_args, &["--preset", "--cpu-used"])? {
            if matches!(encoder, VideoEncoder::X264 | VideoEncoder::X265Standalone) {
                settings.preset = [
                    "ultrafast",
                    "superfast",
                    "veryfast",
                    "faster",
                    "fast",
                    "medium",
                    "slow",
                    "slower",
                    "veryslow",
                    "placebo",
                ]
                .iter()
                .position(|v| *v == value)
                .ok_or("Unknown saved speed preset.")? as u8;
            } else if encoder.is_svt() {
                let preset: i8 = number(value, "preset")?;
                settings.svt_preset = Some(preset);
                settings.preset = preset.max(0) as u8;
            } else {
                settings.preset = number(value, "preset")?;
            }
        }
        if let Some(value) = take(&mut video_args, &["--lp", "--threads", "--pools"])? {
            av1an.encoder_threads = Some(number(value, "encoder threads")?);
        }
        if let Some(value) = take(&mut video_args, &["--film-grain"])? {
            if !encoder.is_svt() {
                return Err("The saved film-grain option belongs to a different encoder.".into());
            }
            settings.film_grain = number(value, "film grain")?;
            if settings.film_grain > 50 {
                return Err("The saved film grain exceeds the supported strength.".into());
            }
        }
        if let Some(value) = take(&mut video_args, &["--film-grain-denoise"])? {
            if !encoder.is_svt() || !matches!(value.as_str(), "0" | "1") {
                return Err("The saved SVT denoising mode is not supported.".into());
            }
            if value == "1" && settings.film_grain > 0 {
                settings.av1an_grain = Some(Av1anGrainSettings {
                    table: None,
                    denoise: true,
                    denoise_strength: 1,
                });
            }
        }
        let aom_denoise = take(&mut video_args, &["--enable-dnl-denoising"])?;
        let aom_grain = take(&mut video_args, &["--denoise-noise-level"])?;
        let aom_table = video_args.remove("--film-grain-table");
        let svt_table = video_args.remove("--fgs-table");
        if aom_table.is_some() && encoder != VideoEncoder::AomAv1
            || svt_table.is_some() && !encoder.is_svt()
            || aom_table.is_some() && svt_table.is_some()
        {
            return Err("The saved grain-table option belongs to a different encoder.".into());
        }
        let grain_table = aom_table.or(svt_table);
        if let Some(path) = grain_table {
            if !matches!(
                encoder,
                VideoEncoder::AomAv1
                    | VideoEncoder::SvtAv1
                    | VideoEncoder::SvtAv1FiveFish
                    | VideoEncoder::SvtAv1Hdr
            ) || settings.film_grain != 0
                || settings.av1an_grain.is_some()
                || aom_denoise.is_some()
                || aom_grain.is_some()
            {
                return Err("The saved grain table conflicts with another grain setting.".into());
            }
            let path = std::path::Path::new(&path);
            if !path.is_absolute() {
                return Err(
                    "The saved grain-table path is relative to an unknown working directory."
                        .into(),
                );
            }
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .and_then(|file| file.take(262145).read_to_end(&mut bytes))
                .map_err(|_| "The saved grain table is missing or unreadable.")?;
            if bytes.len() > 262144 {
                return Err("The saved grain table exceeds the immutable request limit.".into());
            }
            crate::utilities::validate_grain_table(&bytes)?;
            let table =
                String::from_utf8(bytes).map_err(|_| "The saved grain table is not UTF-8 text.")?;
            if table.lines().next() != Some("filmgrn1") {
                return Err("The saved grain table has no AV1 grain header.".into());
            }
            settings.av1an_grain = Some(Av1anGrainSettings {
                table: Some(table),
                denoise: false,
                denoise_strength: 1,
            });
        }
        if aom_denoise.is_some() || aom_grain.is_some() {
            if encoder != VideoEncoder::AomAv1 || aom_denoise.is_none() || aom_grain.is_none() {
                return Err(
                    "The saved AOM grain settings are incomplete or belong to another encoder."
                        .into(),
                );
            }
            let denoise = match aom_denoise.as_deref() {
                Some("0") => false,
                Some("1") => true,
                _ => return Err("The saved AOM denoising mode is not supported.".into()),
            };
            let grain: u8 = number(aom_grain.unwrap(), "AOM grain strength")?;
            if grain > 50 {
                return Err("The saved AOM grain exceeds the supported strength.".into());
            }
            settings.film_grain = grain;
            if denoise && grain > 0 {
                settings.av1an_grain = Some(Av1anGrainSettings {
                    table: None,
                    denoise,
                    denoise_strength: grain.clamp(1, 16),
                });
            }
        }
        if video_args.remove("--disable-warning-prompt").is_some()
            && !matches!(encoder, VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone)
        {
            return Err("The saved prompt switch belongs to a different encoder.".into());
        }
        if video_args.remove("--disable-kf").is_some() {
            if encoder != VideoEncoder::AomAv1 {
                return Err("The saved keyframe switch belongs to a different encoder.".into());
            }
            settings.parameters.push(EncoderParameter {
                name: "disable-kf".into(),
                value: "1".into(),
            });
        }
        if let Some(value) = take(&mut video_args, &["--row-mt"])?
            && (!matches!(encoder, VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone)
                || value != "1")
        {
            return Err("The saved row threading mode cannot be replayed.".into());
        }
        video_args.remove("--no-progress");
        // These declarations were generated from the source by older clients.
        // Retain them only when they match the freshly probed source; Jesses
        // regenerates its own metadata arguments when the new job is planned.
        for (flags, observed, kind) in [
            (
                &["--colorprim", "--color-primaries"][..],
                video.color_primaries.as_deref(),
                "primaries",
            ),
            (
                &["--transfer", "--transfer-characteristics"][..],
                video.color_transfer.as_deref(),
                "transfer",
            ),
            (
                &["--colormatrix", "--matrix-coefficients"][..],
                video.color_space.as_deref(),
                "matrix",
            ),
        ] {
            if let Some(value) = take(&mut video_args, flags)?
                && !color_matches(&value, observed, kind)
            {
                return Err(format!(
                    "The saved {kind} declaration differs from the original source."
                ));
            }
        }
        if let Some(value) = take(&mut video_args, &["--range", "--color-range"])? {
            let range = match value.as_str() {
                "tv" | "limited" | "0" => "tv",
                "pc" | "full" | "1" => "pc",
                _ => return Err("The saved color range is not recognized.".into()),
            };
            if video.color_range.as_deref() != Some(range) {
                return Err("The saved color range differs from the original source.".into());
            }
        }
        if let Some(depth) = take(&mut video_args, &["--output-depth", "--bit-depth"])? {
            let expected = av1an
                .pixel_format
                .ok_or("An explicit saved output depth needs its pixel format.")?
                .ffmpeg();
            if depth
                != if expected.ends_with("10le") {
                    "10"
                } else {
                    "8"
                }
            {
                return Err("The saved pixel format and bit depth conflict.".into());
            }
        }
        if let Some(profile) = take(&mut video_args, &["--profile"])? {
            let format = av1an
                .pixel_format
                .ok_or("The saved profile needs an explicit pixel format.")?;
            let expected = match (encoder, format) {
                (
                    VideoEncoder::AomAv1,
                    Av1anPixelFormat::Yuv420p | Av1anPixelFormat::Yuv420p10le,
                ) => "0",
                (
                    VideoEncoder::AomAv1,
                    Av1anPixelFormat::Yuv444p | Av1anPixelFormat::Yuv444p10le,
                ) => "1",
                (
                    VideoEncoder::AomAv1,
                    Av1anPixelFormat::Yuv422p | Av1anPixelFormat::Yuv422p10le,
                ) => "2",
                (VideoEncoder::VpxStandalone, Av1anPixelFormat::Yuv420p) => "0",
                (VideoEncoder::VpxStandalone, Av1anPixelFormat::Yuv444p) => "1",
                (VideoEncoder::VpxStandalone, Av1anPixelFormat::Yuv420p10le) => "2",
                (VideoEncoder::VpxStandalone, Av1anPixelFormat::Yuv444p10le) => "3",
                _ => return Err("The saved profile and pixel format cannot be replayed.".into()),
            };
            if profile != expected {
                return Err("The saved profile conflicts with the pixel format.".into());
            }
        }
        if let Some(value) = take(&mut video_args, &["--end-usage"])?
            && (value != "q"
                || !matches!(encoder, VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone))
        {
            return Err("The saved rate control mode needs manual recreation.".into());
        }
        if let Some(value) = take(&mut video_args, &["--codec"])?
            && (value != "vp9" || encoder != VideoEncoder::VpxStandalone)
        {
            return Err("The saved VPX codec is not supported.".into());
        }
        for (flag, value) in video_args {
            if !flag.starts_with("--") || value.is_empty() {
                return Err(format!("Unsupported saved encoder argument: {flag}."));
            }
            settings.parameters.push(EncoderParameter {
                name: flag[2..].into(),
                value,
            });
        }
        crate::jobs::parameters::validate_values(encoder, settings.backend, &settings.parameters)
            .map_err(|error| {
            format!(
                "The saved encoder settings cannot be replayed: {}",
                error.message
            )
        })?;
        let audio = take(&mut args, &["-a", "--audio-params"])?
            .ok_or("The sidecar does not record explicit audio handling.")?;
        let mut selected = vec![video.index];
        let (mut audio_args, maps) = audio_options(&audio)?;
        let no_audio = audio_args.remove("-an").is_some();
        let no_subtitles = audio_args.remove("-sn").is_some();
        let mut no_attachments = false;
        let mut dropped_subtitle_ordinals = BTreeSet::new();
        for mapping in maps {
            if mapping == "-0:t?" && !no_attachments {
                no_attachments = true;
            } else if let Some(ordinal) = mapping.strip_prefix("-0:s:") {
                let ordinal: usize = number(ordinal.to_owned(), "subtitle mapping index")?;
                if !dropped_subtitle_ordinals.insert(ordinal) {
                    return Err("The saved subtitle mapping repeats a track.".into());
                }
            } else {
                return Err("The saved stream mapping needs manual recreation.".into());
            }
        }
        let subtitle_streams = media
            .streams
            .iter()
            .filter(|stream| stream.kind == "subtitle")
            .collect::<Vec<_>>();
        if dropped_subtitle_ordinals
            .iter()
            .any(|ordinal| *ordinal >= subtitle_streams.len())
        {
            return Err("The saved subtitle mapping names a missing track.".into());
        }
        if no_subtitles
            && std::path::Path::new(&output)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
            && !subtitle_streams.is_empty()
        {
            return Err("The MP4 sidecar does not record whether text subtitles were restored after encoding. Choose them manually.".into());
        }
        audio_args.remove("-dn"); // Ordinary data is not supported by the intermediate container.
        let codec = match take(&mut audio_args, &["-c:a", "-acodec"])?
            .as_deref()
            .unwrap_or("copy")
        {
            "copy" => AudioCodec::Copy,
            "aac" => AudioCodec::Aac,
            "libopus" => AudioCodec::Opus,
            "flac" => AudioCodec::Flac,
            "libmp3lame" => AudioCodec::Mp3,
            "libvorbis" => AudioCodec::Vorbis,
            "eac3" => AudioCodec::Eac3,
            _ => return Err("The saved audio codec needs manual recreation.".into()),
        };
        let bitrate = if let Some(value) = take(&mut audio_args, &["-b:a"])? {
            number(
                value
                    .strip_suffix('k')
                    .ok_or("Audio bitrate must be expressed in kbps.")?
                    .into(),
                "audio bitrate",
            )?
        } else {
            128
        };
        let channels = match take(&mut audio_args, &["-ac"])?.as_deref() {
            None => AudioChannels::Preserve,
            Some("1") => AudioChannels::Mono,
            Some("2") => AudioChannels::Stereo,
            Some("6") => AudioChannels::Surround51,
            Some("8") => AudioChannels::Surround71,
            _ => return Err("The saved audio channel count needs manual recreation.".into()),
        };
        let subtitle_mode = match audio_args.remove("-c:s").as_deref() {
            None | Some("copy") => SubtitleMode::Copy,
            Some("webvtt") if !no_subtitles => SubtitleMode::WebVtt,
            _ => return Err("The saved subtitle conversion needs manual recreation.".into()),
        };
        for flag in ["-c:t", "-c:d"] {
            if audio_args.remove(flag).is_some_and(|v| v != "copy") {
                return Err(format!(
                    "The saved {flag} conversion needs manual recreation."
                ));
            }
        }
        if !audio_args.is_empty() {
            return Err(format!(
                "Unsupported saved audio/mux arguments: {}.",
                audio_args.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        let mut subtitle_ordinal = 0;
        for stream in &media.streams {
            match stream.kind.as_str() {
                "audio" if !no_audio => {
                    selected.push(stream.index);
                    if codec != AudioCodec::Copy || channels != AudioChannels::Preserve {
                        settings.audio.push(AudioTrackSettings {
                            stream_index: stream.index,
                            codec,
                            bitrate_kbps: bitrate,
                            channels,
                            gain: None,
                        });
                    }
                }
                "subtitle" => {
                    let dropped = dropped_subtitle_ordinals.contains(&subtitle_ordinal);
                    subtitle_ordinal += 1;
                    if !no_subtitles && !dropped {
                        if subtitle_mode == SubtitleMode::WebVtt {
                            let text_codec = matches!(
                                stream.codec.as_deref(),
                                Some(
                                    "subrip"
                                        | "srt"
                                        | "ass"
                                        | "ssa"
                                        | "webvtt"
                                        | "mov_text"
                                        | "text"
                                )
                            );
                            if !text_codec {
                                return Err("The saved WebVTT conversion names a subtitle whose text format cannot be verified.".into());
                            }
                            settings.subtitles.push(SubtitleTrackSettings {
                                stream_index: stream.index,
                                mode: SubtitleMode::WebVtt,
                            });
                        }
                        selected.push(stream.index);
                    }
                }
                "attachment" if !no_attachments => selected.push(stream.index),
                _ => {}
            }
        }
        for flag in ["--force", "--keep", "--verbose", "-y", "-r", "--resume"] {
            args.remove(flag);
        }
        if !args.is_empty() {
            return Err(format!(
                "Unsupported saved processing arguments: {}. No settings or recovery files were changed.",
                args.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        settings.av1an_options = Some(av1an);
        Ok(EncodeRequest {
            source: RemuxRequest {
                input_path: media.path.clone(),
                output_path: output,
                stream_indices: selected,
            },
            settings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokenization_preserves_windows_paths_and_does_not_expand_shell_text() {
        assert_eq!(
            tokens(r#"-i "C:\media\a $name & test.mkv" -v "--crf 24 --tune film""#).unwrap(),
            [
                "-i",
                "C:\\media\\a $name & test.mkv",
                "-v",
                "--crf 24 --tune film"
            ]
        );
        assert!(tokens("-i \"unfinished").is_err());
        assert!(options("--crf 20 --crf=30", &[]).is_err());
        assert!(options("--crf", &[]).is_err());
        assert!(options("&& erase", &[]).is_err());
    }

    #[test]
    fn sidecar_preserves_original_indices_and_rejects_ambiguous_semantics() {
        let root = std::env::temp_dir().join(format!(
            "jesses-sidecar-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let input = root.join("source $name & film.mkv");
        std::fs::write(&input, b"unchanged source").unwrap();
        let media: MediaFile = serde_json::from_value(serde_json::json!({
            "id":"source", "path":input, "name":"source.mkv", "sizeBytes":"16",
            "durationSeconds":1.0, "format":"matroska",
            "streams":[{"index":4,"kind":"video"},{"index":7,"kind":"audio"},{"index":9,"kind":"subtitle"},{"index":11,"kind":"attachment"}]
        })).unwrap();
        let command = format!(
            r#"-i "{}" -e x264 -v "--crf 24 --preset medium --tune film" -a "-c:a libopus -b:a 160k -ac 2 -c:s copy -c:t copy -dn" -w 2 -o "out.mkv""#,
            input.display()
        );
        let make = |args: String| {
            serde_json::to_vec(&serde_json::json!({
                "filePath":input, "fileName":"source.mkv", "tempFolderName":"saved", "args":args,
                "creationTimestamp":"1", "lastRunTimestamp":"2"
            }))
            .unwrap()
        };
        let request = parse(&make(command.clone()))
            .unwrap()
            .request(&media, None)
            .unwrap();
        assert_eq!(request.source.stream_indices, vec![4, 7, 9, 11]);
        assert_eq!(request.settings.video_stream_index, 4);
        assert_eq!(request.settings.encoder, VideoEncoder::X264);
        assert_eq!(request.settings.crf, 24);
        assert_eq!(request.settings.audio[0].stream_index, 7);
        assert_eq!(request.settings.audio[0].codec, AudioCodec::Opus);
        assert_eq!(request.settings.audio[0].bitrate_kbps, 160);
        assert_eq!(request.settings.audio[0].channels, AudioChannels::Stereo);
        let without_attachments = command.replace("-c:t copy -dn", "-c:t copy -dn -map -0:t?");
        let imported = parse(&make(without_attachments))
            .unwrap()
            .request(&media, None)
            .unwrap();
        assert_eq!(imported.source.stream_indices, vec![4, 7, 9]);
        let mut webm_media = media.clone();
        webm_media.streams = serde_json::from_value(serde_json::json!([
            {"index":4,"kind":"video"}, {"index":7,"kind":"audio"},
            {"index":9,"kind":"subtitle","codec":"subrip"},
            {"index":10,"kind":"subtitle","codec":"hdmv_pgs_subtitle"},
            {"index":11,"kind":"subtitle","codec":"dvd_subtitle"},
            {"index":12,"kind":"attachment"}
        ]))
        .unwrap();
        let webm = format!(
            r#"-i "{}" -e vpx -v "--cq-level 32 --cpu-used 8 --codec vp9" -a "-c:a copy -c:s webvtt -map -0:s:1 -map -0:s:2 -dn -map -0:t?" -o "out.webm""#,
            input.display()
        );
        let webm_request = parse(&make(webm.clone()))
            .unwrap()
            .request(&webm_media, None)
            .unwrap();
        assert_eq!(webm_request.source.stream_indices, vec![4, 7, 9]);
        assert_eq!(webm_request.settings.subtitles.len(), 1);
        assert_eq!(webm_request.settings.subtitles[0].stream_index, 9);
        assert_eq!(
            webm_request.settings.subtitles[0].mode,
            SubtitleMode::WebVtt
        );
        assert!(
            parse(&make(webm.replace("-map -0:s:2", "-map 0")))
                .unwrap()
                .request(&webm_media, None)
                .is_err()
        );
        let mp4 = format!(
            r#"-i "{}" -e x264 -v "--crf 24" -a "-c:a copy -sn -dn -map -0:t?" -o "out.mp4""#,
            input.display()
        );
        assert!(
            parse(&make(mp4))
                .unwrap()
                .request(&webm_media, None)
                .unwrap_err()
                .contains("does not record whether text subtitles")
        );
        let svt = command.replace("-e x264", "-e svt-av1").replace(
            "--crf 24 --preset medium --tune film",
            "--crf 24.25 --preset 8 --keyint 240 --lp 0 --film-grain 4 --film-grain-denoise 1",
        );
        let imported = parse(&make(svt.clone()))
            .unwrap()
            .request(&media, Some(VideoEncoder::SvtAv1FiveFish))
            .unwrap();
        assert_eq!(imported.settings.encoder, VideoEncoder::SvtAv1FiveFish);
        assert_eq!(imported.settings.svt_crf_quarter_steps, Some(97));
        assert_eq!(imported.settings.film_grain, 4);
        assert_eq!(
            imported
                .settings
                .av1an_grain
                .as_ref()
                .map(|grain| grain.denoise),
            Some(true)
        );
        assert!(
            imported
                .settings
                .parameters
                .iter()
                .any(|p| p.name == "keyint" && p.value == "240")
        );
        let grain_table = root.join("saved-grain.tbl");
        let table_bytes = "filmgrn1\nE 0 1 1 1 1\n";
        std::fs::write(&grain_table, table_bytes).unwrap();
        let svt_table = svt.replace(
            "--film-grain 4 --film-grain-denoise 1",
            &format!("--fgs-table {}", grain_table.display()),
        );
        let imported = parse(&make(svt_table))
            .unwrap()
            .request(&media, Some(VideoEncoder::SvtAv1FiveFish))
            .unwrap();
        assert_eq!(
            imported.settings.av1an_grain.unwrap().table.as_deref(),
            Some(table_bytes)
        );
        let aom = format!(
            r#"-i "{}" -e aom -v "--end-usage=q --cq-level=24 --cpu-used=6 --disable-kf --kf-min-dist=12 --kf-max-dist=240 --enable-dnl-denoising=1 --denoise-noise-level=8 --threads=0 --tile-rows=0 --tile-columns=1 --disable-warning-prompt" --pix-format yuv420p10le -a "-c:a copy -sn -dn" -o "out.mkv""#,
            input.display()
        );
        let imported = parse(&make(aom.clone()))
            .unwrap()
            .request(&media, None)
            .unwrap();
        assert_eq!(imported.settings.encoder, VideoEncoder::AomAv1);
        assert_eq!(imported.settings.film_grain, 8);
        assert_eq!(
            imported
                .settings
                .av1an_grain
                .as_ref()
                .map(|grain| grain.denoise),
            Some(true)
        );
        for (name, value) in [
            ("disable-kf", "1"),
            ("kf-min-dist", "12"),
            ("kf-max-dist", "240"),
            ("tile-rows", "0"),
            ("tile-columns", "1"),
        ] {
            assert!(
                imported
                    .settings
                    .parameters
                    .iter()
                    .any(|p| p.name == name && p.value == value),
                "{name}"
            );
        }
        let aom_table = aom.replace(
            "--enable-dnl-denoising=1 --denoise-noise-level=8",
            &format!("--film-grain-table={}", grain_table.display()),
        );
        let imported = parse(&make(aom_table))
            .unwrap()
            .request(&media, None)
            .unwrap();
        std::fs::write(&grain_table, "filmgrn1\nE 0 2 1 1 1\n").unwrap();
        assert_eq!(
            imported.settings.av1an_grain.unwrap().table.as_deref(),
            Some(table_bytes)
        );
        let vpx = format!(
            r#"-i "{}" -e vpx -v "--codec=vp9 --profile=2 --bit-depth=10 --end-usage=q --cq-level=24 --cpu-used=4 --kf-max-dist=240 --threads=0 --row-mt=1 --tile-rows=0 --tile-columns=1 --disable-warning-prompt" --pix-format yuv420p10le -a "-c:a copy -sn -dn" -o "out.webm""#,
            input.display()
        );
        let imported = parse(&make(vpx.clone()))
            .unwrap()
            .request(&media, None)
            .unwrap();
        assert_eq!(imported.settings.encoder, VideoEncoder::VpxStandalone);
        assert_eq!(imported.settings.crf, 24);
        for (name, value) in [
            ("kf-max-dist", "240"),
            ("tile-rows", "0"),
            ("tile-columns", "1"),
        ] {
            assert!(
                imported
                    .settings
                    .parameters
                    .iter()
                    .any(|p| p.name == name && p.value == value),
                "{name}"
            );
        }
        assert!(
            parse(&make(vpx.replace("--profile=2", "--profile=0")))
                .unwrap()
                .request(&media, None)
                .is_err()
        );
        let x265 = format!(
            r#"-i "{}" -e x265 -v "--crf 24 --preset medium --keyint 240 --pools 0 --output-depth 10" --pix-format yuv420p10le -a "-c:a copy -sn -dn" -o "out.mkv""#,
            input.display()
        );
        let imported = parse(&make(x265)).unwrap().request(&media, None).unwrap();
        assert_eq!(imported.settings.encoder, VideoEncoder::X265Standalone);
        assert!(
            imported
                .settings
                .parameters
                .iter()
                .any(|p| p.name == "keyint" && p.value == "240")
        );
        for extra in [
            " --target-quality 95",
            " -f crop=100:100",
            " -o another.mkv",
            " -m ffms2 --chunk-method lsmash",
            " -c ffmpeg --concat mkvmerge",
            " ; erase",
        ] {
            assert!(
                parse(&make(format!("{command}{extra}")))
                    .unwrap()
                    .request(&media, None)
                    .is_err(),
                "{extra}"
            );
        }
        assert!(
            parse(&make(command.replace("-e x264", "-e svt-av1")))
                .unwrap()
                .request(&media, None)
                .is_err()
        );
        let intermediate = root.join("trimmed.mkv");
        std::fs::write(&intermediate, b"unchanged intermediate").unwrap();
        assert!(
            parse(&make(command.replace(
                &input.to_string_lossy().to_string(),
                &intermediate.to_string_lossy()
            )))
            .unwrap()
            .request(&media, None)
            .is_err()
        );
        assert_eq!(std::fs::read(&input).unwrap(), b"unchanged source");
        assert_eq!(
            std::fs::read(&intermediate).unwrap(),
            b"unchanged intermediate"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
