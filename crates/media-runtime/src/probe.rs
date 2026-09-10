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
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    index: u32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    sample_rate: Option<Value>,
    channels: Option<u32>,
    duration: Option<Value>,
    tags: Option<HashMap<String, String>>,
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

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
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
                .filter_map(|stream| number(stream.duration.as_ref()))
                .reduce(f64::max)
        });
    let streams = document
        .streams
        .into_iter()
        .map(|stream| {
            let tags = stream.tags.unwrap_or_default();
            let frame_rate = stream
                .avg_frame_rate
                .filter(|value| positive_rational(value))
                .or_else(|| stream.r_frame_rate.filter(|value| positive_rational(value)));
            MediaStream {
                index: stream.index,
                kind: nonempty(stream.codec_type).unwrap_or_else(|| "unknown".into()),
                codec: nonempty(stream.codec_name),
                width: stream.width,
                height: stream.height,
                frame_rate,
                sample_rate: sample_rate(stream.sample_rate.as_ref()),
                channels: stream.channels,
                language: tag(&tags, "language"),
                title: tag(&tags, "title"),
            }
        })
        .collect();
    Ok(MediaFile {
        id: media_id(&path),
        path,
        name,
        size_bytes: size.to_string(),
        duration_seconds,
        format: document
            .format
            .and_then(|format| nonempty(format.format_name)),
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
