use std::{collections::HashMap, ffi::OsString, path::Path, time::Duration};

use media_core::{AppError, MediaFile, MediaStream};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    discovery::find_executable,
    process::{ProcessError, run_tool},
};

#[derive(Debug, Deserialize)]
struct ProbeDocument {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    format_name: Option<String>,
    duration: Option<Value>,
    bit_rate: Option<Value>,
    tags: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    index: u32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    codec_tag_string: Option<String>,
    codec_long_name: Option<String>,
    profile: Option<String>,
    bit_rate: Option<Value>,
    disposition: Option<Value>,
    width: Option<u32>,
    height: Option<u32>,
    sample_aspect_ratio: Option<String>,
    display_aspect_ratio: Option<String>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    field_order: Option<String>,
    sample_rate: Option<Value>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    duration: Option<Value>,
    tags: Option<HashMap<String, String>>,
    pix_fmt: Option<String>,
    bits_per_raw_sample: Option<Value>,
    color_primaries: Option<String>,
    color_transfer: Option<String>,
    color_space: Option<String>,
    color_range: Option<String>,
    #[serde(default)]
    side_data_list: Vec<Value>,
}

fn positive_rational(value: &str) -> bool {
    let Some((numerator, denominator)) = value.split_once('/') else {
        return false;
    };
    matches!((numerator.parse::<u64>(), denominator.parse::<u64>()), (Ok(n), Ok(d)) if n > 0 && d > 0)
}

fn number(value: Option<&Value>) -> Option<f64> {
    let number = match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(number) => number.parse().ok(),
        _ => None,
    }?;
    (number.is_finite() && number >= 0.0).then_some(number)
}

fn sample_rate(value: Option<&Value>) -> Option<u32> {
    let number = match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(number) => number.parse::<u64>().ok(),
        _ => None,
    }?;
    u32::try_from(number).ok().filter(|number| *number > 0)
}

fn decimal_rate(value: Option<&Value>) -> Option<String> {
    let rate = match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(number) => number.parse::<u64>().ok(),
        _ => None,
    }?;
    (rate > 0).then(|| rate.to_string())
}

/// Matroska commonly reports per-stream duration as a tag, without a numeric
/// duration field. Reject malformed clocks instead of substituting file length.
pub(crate) fn duration_tag_seconds(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<u32>().ok()?;
    let minutes = parts.next()?.parse::<u32>().ok()?;
    let seconds = parts.next()?.parse::<f64>().ok()?;
    if parts.next().is_some() || minutes >= 60 || !(0.0..60.0).contains(&seconds) {
        return None;
    }
    Some(f64::from(hours) * 3600.0 + f64::from(minutes) * 60.0 + seconds)
}

fn stream_duration(stream: &ProbeStream) -> Option<f64> {
    number(stream.duration.as_ref())
        .or_else(|| duration_tag_seconds(&tag(stream.tags.as_ref()?, "duration")?))
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

fn color_value(value: Option<String>) -> Option<String> {
    nonempty(value).filter(|text| !matches!(text.as_str(), "unknown" | "unspecified" | "reserved"))
}

fn pixel_depth(pixel_format: Option<&str>) -> Option<u32> {
    let format = pixel_format?;
    if matches!(
        format,
        "yuv420p"
            | "yuv422p"
            | "yuv444p"
            | "yuvj420p"
            | "yuvj422p"
            | "yuvj444p"
            | "nv12"
            | "nv21"
            | "gray"
            | "rgb24"
            | "bgr24"
            | "rgba"
            | "bgra"
    ) {
        return Some(8);
    }
    // Read the bit depth suffix only for known planar/high-depth format families.
    if format.starts_with("yuv") || format.starts_with("gbr") || format.starts_with("gray") {
        let suffix = format
            .strip_suffix("le")
            .or_else(|| format.strip_suffix("be"))?;
        return [9, 10, 12, 14, 16]
            .into_iter()
            .find(|depth| suffix.ends_with(&depth.to_string()));
    }
    match format {
        "p010le" | "p010be" => Some(10),
        "p012le" | "p012be" => Some(12),
        "p016le" | "p016be" => Some(16),
        _ => None,
    }
}

fn hdr_headers(side_data: &[Value]) -> (bool, Vec<String>) {
    let mut static_metadata = false;
    let mut dynamic_formats = Vec::new();
    for data in side_data {
        let name = data
            .get("side_data_type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        static_metadata |=
            name.contains("mastering display") || name.contains("content light level");
        let dynamic = if name.contains("dovi") || name.contains("dolby vision") {
            Some("Dolby Vision")
        } else if name.contains("hdr10+")
            || name.contains("smpte2094-40")
            || name.contains("smpte 2094-40")
        {
            Some("HDR10+")
        } else {
            None
        };
        if let Some(dynamic) = dynamic
            && !dynamic_formats.iter().any(|value| value == dynamic)
        {
            dynamic_formats.push(dynamic.to_string());
        }
    }
    (static_metadata, dynamic_formats)
}

fn tag(tags: &HashMap<String, String>, name: &str) -> Option<String> {
    tags.iter()
        .find(|(key, value)| key.eq_ignore_ascii_case(name) && !value.trim().is_empty())
        .map(|(_, value)| value.clone())
}

/// Deterministic path identity, with ASCII case normalization on Windows.
fn media_id(path: &str) -> String {
    #[cfg(windows)]
    let path = path.to_ascii_lowercase();
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("media-{hash:016x}")
}

pub(crate) fn parse_probe(
    bytes: &[u8],
    path: String,
    name: String,
    size: u64,
) -> Result<MediaFile, AppError> {
    let document: ProbeDocument = serde_json::from_slice(bytes).map_err(|_| {
        AppError::new(
            "PROBE_INVALID_RESPONSE",
            "FFprobe returned unreadable media metadata.",
            Some(path.clone()),
        )
    })?;
    let duration_seconds = document
        .format
        .as_ref()
        .and_then(|format| number(format.duration.as_ref()))
        .or_else(|| {
            document
                .streams
                .iter()
                .filter_map(stream_duration)
                .reduce(f64::max)
        });
    let streams = document
        .streams
        .into_iter()
        .map(|stream| {
            let duration_seconds = stream_duration(&stream);
            let is_video = stream.codec_type.as_deref() == Some("video");
            let bit_depth = sample_rate(stream.bits_per_raw_sample.as_ref())
                .filter(|depth| *depth <= 64)
                .or_else(|| pixel_depth(stream.pix_fmt.as_deref()));
            let color_transfer = color_value(stream.color_transfer);
            let hdr_format = match color_transfer.as_deref() {
                Some("smpte2084") => Some("HDR / PQ".to_string()),
                Some("arib-std-b67") => Some("HDR / HLG".to_string()),
                _ => None,
            };
            let (static_metadata, dynamic_formats) = hdr_headers(&stream.side_data_list);
            let dolby_vision_profile = stream
                .side_data_list
                .iter()
                .filter(|data| {
                    data.get("side_data_type").and_then(Value::as_str)
                        == Some("DOVI configuration record")
                })
                .filter_map(|data| data.get("dv_profile").and_then(Value::as_u64))
                .filter_map(|profile| u8::try_from(profile).ok())
                .reduce(|a, b| if a == b { a } else { 0 })
                .filter(|profile| *profile != 0);
            let tags = stream.tags.unwrap_or_default();
            let rotation_degrees = stream
                .side_data_list
                .iter()
                .filter_map(|data| data.get("rotation"))
                .find_map(|v| v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok()))
                .or_else(|| tag(&tags, "rotate")?.parse::<f64>().ok())
                .filter(|n| n.is_finite())
                .map(|n| n.to_string());
            let aspect = |v: Option<String>| v.filter(|s| positive_rational(&s.replace(':', "/")));
            let average_frame_rate = stream
                .avg_frame_rate
                .filter(|value| positive_rational(value));
            let nominal_frame_rate = stream.r_frame_rate.filter(|value| positive_rational(value));
            let frame_rate = average_frame_rate
                .clone()
                .or_else(|| nominal_frame_rate.clone());
            MediaStream {
                index: stream.index,
                kind: nonempty(stream.codec_type).unwrap_or_else(|| "unknown".into()),
                codec: nonempty(stream.codec_name),
                codec_tag: nonempty(stream.codec_tag_string),
                codec_long_name: nonempty(stream.codec_long_name),
                profile: color_value(stream.profile),
                bit_rate: decimal_rate(stream.bit_rate.as_ref()),
                duration_seconds,
                average_frame_rate,
                nominal_frame_rate,
                is_default: stream.disposition.as_ref().and_then(|value| {
                    match value.get("default")? {
                        Value::Bool(value) => Some(*value),
                        Value::Number(value) => match value.as_u64() {
                            Some(0) => Some(false),
                            Some(1) => Some(true),
                            _ => None,
                        },
                        _ => None,
                    }
                }),
                attachment_filename: tag(&tags, "filename"),
                attachment_mime_type: tag(&tags, "mimetype"),
                width: stream.width,
                height: stream.height,
                sample_aspect_ratio: aspect(stream.sample_aspect_ratio),
                display_aspect_ratio: aspect(stream.display_aspect_ratio),
                rotation_degrees,
                frame_rate,
                field_order: nonempty(stream.field_order),
                sample_rate: sample_rate(stream.sample_rate.as_ref()),
                channels: stream.channels,
                channel_layout: nonempty(stream.channel_layout),
                language: tag(&tags, "language"),
                title: tag(&tags, "title")
                    .or_else(|| tag(&tags, "name"))
                    .or_else(|| {
                        tag(&tags, "handler_name").filter(|value| {
                            !matches!(
                                value.as_str(),
                                "VideoHandler" | "SoundHandler" | "SubtitleHandler"
                            )
                        })
                    }),
                pixel_format: nonempty(stream.pix_fmt),
                bit_depth,
                color_primaries: color_value(stream.color_primaries),
                color_transfer,
                color_space: color_value(stream.color_space),
                color_range: color_value(stream.color_range),
                hdr_format,
                has_hdr_static_metadata: is_video.then_some(static_metadata),
                dynamic_hdr_formats: is_video.then_some(dynamic_formats),
                dolby_vision_profile: is_video.then_some(dolby_vision_profile).flatten(),
            }
        })
        .collect();
    let format = document.format;
    let format_tags = format.as_ref().and_then(|format| format.tags.as_ref());
    Ok(MediaFile {
        id: media_id(&path),
        path,
        name,
        size_bytes: size.to_string(),
        duration_seconds,
        title: format_tags.and_then(|tags| tag(tags, "title")),
        language: format_tags.and_then(|tags| tag(tags, "language")),
        bit_rate: format
            .as_ref()
            .and_then(|format| decimal_rate(format.bit_rate.as_ref())),
        format: format.and_then(|format| nonempty(format.format_name)),
        streams,
    })
}

fn invalid_input(path: &str) -> bool {
    let normalized = path.trim().to_ascii_lowercase();
    normalized.is_empty()
        || path.contains('\0')
        || normalized.contains("://")
        || ["pipe:", "data:", "concat:", "subfile:", "file:"]
            .iter()
            .any(|prefix| normalized.starts_with(prefix))
}

fn probe_arguments(path: &Path) -> Vec<OsString> {
    [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-i",
    ]
    .iter()
    .map(OsString::from)
    .chain(std::iter::once(path.as_os_str().to_owned()))
    .collect()
}

/// Inspect one existing local regular file. No encoding, output paths, generic
/// command execution, or remote input protocols are exposed to the frontend.
pub async fn probe_media(path: String) -> Result<MediaFile, AppError> {
    let fail = |code: &str, message: String| AppError::new(code, message, Some(path.clone()));
    if invalid_input(&path) {
        return Err(fail(
            "INVALID_INPUT",
            "Choose a local media file. URLs and stream inputs are not supported.".into(),
        ));
    }
    let canonical = tokio::fs::canonicalize(&path).await.map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::NotFound {
            "FILE_NOT_FOUND"
        } else {
            "FILE_UNREADABLE"
        };
        fail(
            code,
            format!("The selected file could not be accessed: {error}"),
        )
    })?;
    let metadata = tokio::fs::metadata(&canonical).await.map_err(|error| {
        fail(
            "FILE_UNREADABLE",
            format!("The selected file could not be read: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(fail(
            "NOT_A_FILE",
            "Choose an individual media file; folder import is not available yet.".into(),
        ));
    }
    let canonical_text = canonical
        .to_str()
        .ok_or_else(|| {
            fail(
                "INVALID_INPUT",
                "The file path cannot be represented as Unicode.".into(),
            )
        })?
        .to_owned();
    let executable = find_executable(&["ffprobe"])
        .await
        .map_err(|detail| fail("TOOL_DISCOVERY_FAILED", detail))?
        .ok_or_else(|| {
            fail(
                "TOOL_MISSING",
                "FFprobe was not found on PATH. Install FFmpeg with FFprobe and restart jesses."
                    .into(),
            )
        })?;
    let output = run_tool(
        &executable,
        &probe_arguments(&canonical),
        Duration::from_secs(30),
        2 * 1024 * 1024,
    )
    .await
    .map_err(|error| {
        let code = match &error {
            ProcessError::Timeout => "PROBE_TIMEOUT",
            ProcessError::OutputLimit => "PROBE_OUTPUT_LIMIT",
            ProcessError::Io(_) => "TOOL_FAILED",
        };
        fail(code, error.to_string())
    })?;
    if !output.status.success() {
        let detail: String = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(600)
            .collect();
        return Err(fail(
            "PROBE_FAILED",
            if detail.is_empty() {
                format!("FFprobe could not inspect this file ({}).", output.status)
            } else {
                format!("FFprobe could not inspect this file: {detail}")
            },
        ));
    }
    let name = canonical
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    parse_probe(&output.stdout, canonical_text, name, metadata.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_stream_indices_rationals_and_tags() {
        let parsed = parse_probe(br#"{"streams":[
          {"index":3,"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"24000/1001","r_frame_rate":"24/1"},
          {"index":7,"codec_type":"audio","codec_name":"flac","sample_rate":"48000","channels":2,"tags":{"LANGUAGE":"eng","title":"Original mix"}},
          {"index":8,"codec_type":"subtitle","codec_name":"subrip"}
        ],"format":{"format_name":"matroska,webm","duration":"12.345"}}"#, "movie.mkv".into(), "movie.mkv".into(), 9007199254740993).unwrap();
        assert_eq!(parsed.duration_seconds, Some(12.345));
        assert_eq!(parsed.streams[0].index, 3);
        assert_eq!(parsed.streams[0].frame_rate.as_deref(), Some("24000/1001"));
        assert_eq!(parsed.streams[1].sample_rate, Some(48000));
        assert_eq!(parsed.streams[1].language.as_deref(), Some("eng"));
        assert_eq!(parsed.streams[1].title.as_deref(), Some("Original mix"));
        assert_eq!(parsed.streams[2].width, None);
        assert_eq!(parsed.size_bytes, "9007199254740993");
    }

    #[test]
    fn display_metadata_keeps_rationals_and_prefers_the_matrix_rotation() {
        let parsed = parse_probe(br#"{"streams":[{"index":3,"codec_type":"video","sample_aspect_ratio":"8:9","display_aspect_ratio":"4:3","side_data_list":[{"rotation":90}],"tags":{"rotate":"180"}},{"index":4,"codec_type":"video","sample_aspect_ratio":"0:1","tags":{"rotate":"-90"}}]}"#,
            "source.mp4".into(), "source.mp4".into(), 10).unwrap();
        assert_eq!(
            parsed.streams[0].sample_aspect_ratio.as_deref(),
            Some("8:9")
        );
        assert_eq!(
            parsed.streams[0].display_aspect_ratio.as_deref(),
            Some("4:3")
        );
        assert_eq!(parsed.streams[0].rotation_degrees.as_deref(), Some("90"));
        assert_eq!(parsed.streams[1].sample_aspect_ratio, None);
        assert_eq!(parsed.streams[1].rotation_degrees.as_deref(), Some("-90"));
    }

    #[test]
    fn handles_absent_unknown_and_nonfinite_metadata() {
        let parsed = parse_probe(
            br#"{"streams":[
          {"index":0,"avg_frame_rate":"0/0","r_frame_rate":"30000/1001","duration":"2.5"},
          {"index":1,"avg_frame_rate":"0/1","duration":"NaN","sample_rate":"N/A","tags":null,"width":null,"height":null,"codec_name":null}
        ],"format":{"duration":"Infinity"}}"#,
            "image".into(),
            "image".into(),
            0,
        )
        .unwrap();
        assert_eq!(parsed.duration_seconds, Some(2.5));
        assert_eq!(parsed.streams[0].kind, "unknown");
        assert_eq!(parsed.streams[0].frame_rate.as_deref(), Some("30000/1001"));
        assert_eq!(parsed.streams[1].frame_rate, None);
        assert_eq!(parsed.streams[1].sample_rate, None);
        assert_eq!(parsed.streams[1].language, None);
        assert_eq!(parsed.streams[1].width, None);
        assert_eq!(parsed.streams[1].codec, None);
        assert_eq!(parsed.format, None);
    }

    #[test]
    fn malformed_json_returns_structured_error() {
        let error = parse_probe(b"not json", "broken".into(), "broken".into(), 0).unwrap_err();
        assert_eq!(error.code, "PROBE_INVALID_RESPONSE");
        assert_eq!(error.path.as_deref(), Some("broken"));
    }

    #[test]
    fn preserves_full_inspector_metadata_and_exact_decimal_rates() {
        let parsed = parse_probe(br#"{"streams":[
          {"index":0,"codec_type":"video","codec_name":"h264","codec_long_name":"H.264 / AVC","profile":"High","bit_rate":"9007199254740993","avg_frame_rate":"24000/1001","r_frame_rate":"30/1","field_order":"tt","disposition":{"default":1},"tags":{"DURATION":"00:00:01.001000000"}},
          {"index":3,"codec_type":"audio","codec_name":"dts","profile":"DTS-HD MA","bit_rate":768000,"channel_layout":"5.1(side)","disposition":{"default":0},"duration":"3.5","tags":{"DURATION":"00:00:02.0"}},
          {"index":5,"codec_type":"attachment","codec_name":"ttf","tags":{"FILENAME":"caption-font.ttf","MIMETYPE":"application/x-truetype-font"}},
          {"index":6,"codec_type":"data","codec_name":"bin_data"}
        ],"format":{"bit_rate":"9007199254740995","tags":{"TITLE":"Example title","LANGUAGE":"eng"}}}"#,
            "source.mkv".into(), "source.mkv".into(), 1).unwrap();
        assert_eq!(parsed.duration_seconds, Some(3.5));
        assert_eq!(parsed.title.as_deref(), Some("Example title"));
        assert_eq!(parsed.language.as_deref(), Some("eng"));
        assert_eq!(parsed.bit_rate.as_deref(), Some("9007199254740995"));
        let video = &parsed.streams[0];
        assert_eq!(video.codec_long_name.as_deref(), Some("H.264 / AVC"));
        assert_eq!(video.profile.as_deref(), Some("High"));
        assert_eq!(video.bit_rate.as_deref(), Some("9007199254740993"));
        assert_eq!(video.duration_seconds, Some(1.001));
        assert_eq!(video.average_frame_rate.as_deref(), Some("24000/1001"));
        assert_eq!(video.nominal_frame_rate.as_deref(), Some("30/1"));
        assert_eq!(video.field_order.as_deref(), Some("tt"));
        assert_eq!(video.is_default, Some(true));
        let audio = &parsed.streams[1];
        assert_eq!(audio.codec.as_deref(), Some("dts"));
        assert_eq!(audio.profile.as_deref(), Some("DTS-HD MA"));
        assert_eq!(audio.bit_rate.as_deref(), Some("768000"));
        assert_eq!(audio.channel_layout.as_deref(), Some("5.1(side)"));
        assert_eq!(audio.duration_seconds, Some(3.5));
        assert_eq!(audio.is_default, Some(false));
        assert_eq!(
            parsed.streams[2].attachment_filename.as_deref(),
            Some("caption-font.ttf")
        );
        assert_eq!(
            parsed.streams[2].attachment_mime_type.as_deref(),
            Some("application/x-truetype-font")
        );
        assert_eq!(parsed.streams[3].kind, "data");
        assert_eq!(parsed.streams[3].is_default, None);
        let json = serde_json::to_value(parsed).unwrap();
        assert_eq!(json["streams"][0]["bitRate"], "9007199254740993");
    }

    #[test]
    fn rejects_invalid_extra_metadata_without_inventing_values() {
        for text in [
            "NaN",
            "00:60:01",
            "00:00:60",
            "00:00:NaN",
            "-1:00:00",
            "00:00:-1",
            "1:02",
            "1:02:03:04",
        ] {
            assert_eq!(duration_tag_seconds(text), None, "{text}");
        }
        assert_eq!(duration_tag_seconds("01:02:03.125"), Some(3723.125));
        assert_eq!(duration_tag_seconds("00:00:00.000"), Some(0.0));
        let parsed = parse_probe(br#"{"streams":[
          {"index":0,"bit_rate":"NaN","profile":"unknown","codec_long_name":" ","avg_frame_rate":"0/0","r_frame_rate":"1/0","disposition":{"default":2},"duration":"Infinity","tags":{"DURATION":"00:60:00"}},
          {"index":1,"bit_rate":-1,"duration":"0","tags":{"DURATION":"00:00:05"}}
        ],"format":{"bit_rate":"0"}}"#, "unknown".into(), "unknown".into(), 0).unwrap();
        assert_eq!(parsed.bit_rate, None);
        let stream = &parsed.streams[0];
        assert_eq!(stream.codec_long_name, None);
        assert_eq!(stream.profile, None);
        assert_eq!(stream.bit_rate, None);
        assert_eq!(stream.duration_seconds, None);
        assert_eq!(stream.average_frame_rate, None);
        assert_eq!(stream.nominal_frame_rate, None);
        assert_eq!(stream.is_default, None);
        assert_eq!(parsed.streams[1].bit_rate, None);
        assert_eq!(parsed.streams[1].duration_seconds, Some(0.0));
    }

    #[test]
    fn exposes_hdr_headers_without_assuming_frame_metadata_is_absent() {
        let parsed = parse_probe(br#"{"streams":[
          {"index":0,"codec_type":"video","pix_fmt":"yuv420p10le","bits_per_raw_sample":"0","color_primaries":"bt2020","color_transfer":"smpte2084","color_space":"bt2020nc","color_range":"tv","side_data_list":[
            {"side_data_type":"DOVI configuration record","dv_profile":7},
            {"side_data_type":"Mastering display metadata"},
            {"side_data_type":"Content light level metadata"},
            {"side_data_type":"HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"}
          ]},
          {"index":1,"codec_type":"video","pix_fmt":"p010le","color_transfer":"arib-std-b67"},
          {"index":2,"codec_type":"video","pix_fmt":"yuv420p","color_primaries":"unknown"}
        ]}"#, "hdr.mkv".into(), "hdr.mkv".into(), 1).unwrap();
        let pq = &parsed.streams[0];
        assert_eq!(pq.pixel_format.as_deref(), Some("yuv420p10le"));
        assert_eq!(pq.bit_depth, Some(10));
        assert_eq!(pq.hdr_format.as_deref(), Some("HDR / PQ"));
        assert_eq!(pq.color_primaries.as_deref(), Some("bt2020"));
        assert_eq!(pq.color_space.as_deref(), Some("bt2020nc"));
        assert_eq!(pq.color_range.as_deref(), Some("tv"));
        assert_eq!(pq.has_hdr_static_metadata, Some(true));
        assert_eq!(pq.dolby_vision_profile, Some(7));
        assert_eq!(
            pq.dynamic_hdr_formats.as_deref(),
            Some(["Dolby Vision".into(), "HDR10+".into()].as_slice())
        );
        let hlg = &parsed.streams[1];
        assert_eq!(hlg.hdr_format.as_deref(), Some("HDR / HLG"));
        assert_eq!(hlg.bit_depth, Some(10));
        assert_eq!(hlg.has_hdr_static_metadata, Some(false));
        assert_eq!(hlg.dolby_vision_profile, None);
        assert_eq!(hlg.dynamic_hdr_formats.as_deref(), Some([].as_slice()));
        assert_eq!(parsed.streams[2].bit_depth, Some(8));
        assert_eq!(parsed.streams[2].color_primaries, None);
        assert_eq!(parsed.streams[2].hdr_format, None);
    }

    #[test]
    fn pixel_depth_only_infers_known_formats() {
        assert_eq!(pixel_depth(Some("gbrp12le")), Some(12));
        assert_eq!(pixel_depth(Some("unknown10le")), None);
        assert_eq!(pixel_depth(None), None);
    }

    #[test]
    fn paths_are_one_argument_and_protocols_are_restricted() {
        let path = Path::new("/media/- weird ' & $ % \\unicode.mkv");
        let args = probe_arguments(path);
        assert_eq!(args.last().unwrap(), path.as_os_str());
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-protocol_whitelist", "file"])
        );
        assert!(invalid_input("https://example.com/movie.mkv"));
        assert!(invalid_input("pipe:0"));
        assert!(!invalid_input("C:\\Media\\movie & sample.mkv"));
    }

    #[tokio::test]
    async fn rejects_directory_and_url_before_discovering_tools() {
        assert_eq!(
            probe_media("https://example.com/movie.mkv".into())
                .await
                .unwrap_err()
                .code,
            "INVALID_INPUT"
        );
        assert_eq!(
            probe_media(std::env::temp_dir().to_string_lossy().into_owned())
                .await
                .unwrap_err()
                .code,
            "NOT_A_FILE"
        );
    }
}
