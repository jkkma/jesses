//! Codec configuration records (Matroska CodecPrivate, MP4 sample-entry
//! boxes) that differ only in fields a decoder ignores.
//!
//! A muxer may rewrite the record while it copies a stream's packets. FFmpeg
//! writes an HEVC record with every `array_completeness` flag cleared where
//! mkvmerge sets them. The flag only says whether more parameter sets may
//! follow in the stream; the parameter sets that configure the decoder, and
//! every other field, must stay byte-identical.
use std::{ffi::OsString, path::Path, time::Duration};

use media_core::AppError;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::{metadata::Stream, process_error};
use crate::supervisor::{self, CommandSpec};

/// Whether two probed streams configure their decoder identically.
pub(in crate::jobs) async fn equivalent(
    ffprobe: &Path,
    first: (&Path, &Stream),
    second: (&Path, &Stream),
    cancel: &watch::Receiver<bool>,
) -> Result<bool, AppError> {
    if first.1.extradata_hash == second.1.extradata_hash {
        return Ok(true);
    }
    if first.1.codec_name.as_deref() != Some("hevc")
        || second.1.codec_name.as_deref() != Some("hevc")
    {
        return Ok(false);
    }
    let (Some(a), Some(b)) = (
        extradata(ffprobe, first.0, first.1, cancel).await?,
        extradata(ffprobe, second.0, second.1, cancel).await?,
    ) else {
        return Ok(false);
    };
    Ok(hevc_equivalent(&a, &b))
}

/// The stream's configuration record, or `None` when it cannot be read back
/// exactly: the parsed bytes must match FFprobe's own SHA-256 of them.
async fn extradata(
    ffprobe: &Path,
    path: &Path,
    stream: &Stream,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<Vec<u8>>, AppError> {
    let Some(expected) = stream
        .extradata_hash
        .as_deref()
        .and_then(|hash| hash.strip_prefix("SHA256:"))
    else {
        return Ok(None);
    };
    let output = supervisor::run_capture(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args: [
                "-v",
                "error",
                "-protocol_whitelist",
                "file",
                "-select_streams",
            ]
            .into_iter()
            .map(OsString::from)
            .chain([stream.index.to_string().into()])
            .chain(
                [
                    "-show_entries",
                    "stream=extradata",
                    "-show_data",
                    "-of",
                    "json",
                    "-i",
                ]
                .map(OsString::from),
            )
            .chain([path.as_os_str().to_owned()])
            .collect(),
            cwd: None,
        },
        cancel.clone(),
        1024 * 1024,
        Duration::from_secs(60),
    )
    .await
    .map_err(|error| process_error(error, path))?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Ok(None);
    }
    #[derive(serde::Deserialize)]
    struct Document {
        #[serde(default)]
        streams: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        extradata: Option<String>,
    }
    let Some(bytes) = serde_json::from_slice::<Document>(&output.stdout)
        .ok()
        .and_then(|document| document.streams.into_iter().next())
        .and_then(|entry| entry.extradata)
        .and_then(|text| parse_hex_dump(&text))
    else {
        return Ok(None);
    };
    let actual = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(actual.eq_ignore_ascii_case(expected).then_some(bytes))
}

/// Parses FFprobe's `-show_data` dump: `offset: 16 hex bytes  ascii` per line.
fn parse_hex_dump(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (offset, rest) = line.split_once(": ")?;
        if usize::from_str_radix(offset.trim(), 16).ok()? != bytes.len() {
            return None;
        }
        // 16 bytes print as eight 4-digit groups and a one-space gutter.
        let hex = rest.get(..41).unwrap_or(rest).replace(' ', "");
        if hex.is_empty() || hex.len() > 32 || hex.len() % 2 != 0 {
            return None;
        }
        for pair in hex.as_bytes().chunks(2) {
            bytes.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
        }
    }
    (!bytes.is_empty()).then_some(bytes)
}

/// HEVCDecoderConfigurationRecord (ISO/IEC 14496-15): a 22-byte header, the
/// array count, then per array one `completeness | reserved | NAL type` byte,
/// a NAL count, and length-prefixed NAL units. Equal except `completeness`.
fn hevc_equivalent(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.len() < 23 || a[0] != 1 || a[..23] != b[..23] {
        return false;
    }
    let field = |position: usize| -> Option<u16> {
        Some(u16::from_be_bytes(
            a.get(position..position + 2)?.try_into().ok()?,
        ))
    };
    let mut position = 23;
    for _ in 0..a[22] {
        let Some(nalus) = field(position + 1) else {
            return false;
        };
        if a[position] & 0x7f != b[position] & 0x7f
            || a[position + 1..position + 3] != b[position + 1..position + 3]
        {
            return false;
        }
        position += 3;
        for _ in 0..nalus {
            let Some(length) = field(position) else {
                return false;
            };
            let end = position + 2 + usize::from(length);
            if end > a.len() || a[position..end] != b[position..end] {
                return false;
            }
            position = end;
        }
    }
    position == a.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(bytes: &[u8]) -> String {
        let mut text = String::from("\n");
        for (row, chunk) in bytes.chunks(16).enumerate() {
            let mut hex = String::new();
            for (index, byte) in chunk.iter().enumerate() {
                hex.push_str(&format!("{byte:02x}"));
                if index % 2 == 1 {
                    hex.push(' ');
                }
            }
            let ascii: String = chunk
                .iter()
                .map(|byte| {
                    if (32..127).contains(byte) {
                        *byte as char
                    } else {
                        '.'
                    }
                })
                .collect();
            text.push_str(&format!("{:08x}: {hex:<41}{ascii}\n", row * 16));
        }
        text
    }

    /// A record with VPS, SPS and PPS arrays; `complete` sets bit 7 of each.
    fn record(complete: bool) -> Vec<u8> {
        // x265's 22-byte header for 8-bit 4:2:0 Main at level 1.
        let mut bytes = vec![
            1, 1, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 0x1e, 0xf0, 0, 0xfc, 0xfd, 0xf8, 0xf8, 0, 0,
            0x0f,
        ];
        bytes.push(3);
        for (kind, nal) in [
            (32u8, &[0x40, 0x01, 0x0c][..]),
            (33, &[0x42, 0x01, 0x01, 0x01]),
            (34, &[0x44, 0x01, 0xc0]),
        ] {
            bytes.push(kind | if complete { 0x80 } else { 0 });
            bytes.extend([0, 1, 0, nal.len() as u8]);
            bytes.extend(nal);
        }
        bytes
    }

    #[test]
    fn parses_ffprobe_dumps_including_a_short_last_line() {
        let bytes = (0..=40u32)
            .map(|value| (value * 7 % 256) as u8)
            .collect::<Vec<_>>();
        assert_eq!(parse_hex_dump(&dump(&bytes)), Some(bytes));
        assert_eq!(parse_hex_dump("\n00000010: 0101  ..\n"), None);
        assert_eq!(parse_hex_dump("\n00000000: 01zz  ..\n"), None);
    }

    #[test]
    fn hevc_records_may_differ_only_in_array_completeness() {
        assert!(hevc_equivalent(&record(true), &record(false)));
        assert!(hevc_equivalent(&record(false), &record(false)));
        let mut changed_pps = record(false);
        let last = changed_pps.len() - 2;
        changed_pps[last] ^= 1;
        assert!(!hevc_equivalent(&record(true), &changed_pps));
        let mut changed_type = record(false);
        changed_type[23] = 39;
        assert!(!hevc_equivalent(&record(false), &changed_type));
        let mut changed_header = record(false);
        changed_header[1] = 2;
        assert!(!hevc_equivalent(&record(false), &changed_header));
        let mut trailing = record(false);
        trailing.push(0);
        assert!(!hevc_equivalent(&trailing, &trailing.clone()));
    }
}
