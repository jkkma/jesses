//! FFmpeg's MP4 muxer appends bitrate hints to copied tx3g extradata.
//! An identical repeat, or the first exact hint bound to the original MOV's
//! sample-table bitrate, is representational; style/font/unknown atoms stay exact.
use super::{CommandSpec, OsString, Path, Sha256, Stream, invalid, supervisor};
use media_core::AppError;
use sha2::Digest;
use std::time::Duration;
use tokio::sync::watch;

fn normalize(header: &[u8]) -> Option<Vec<u8>> {
    let mut result = header.get(..30)?.to_vec();
    let mut position = 30;
    let mut bitrate: Option<&[u8]> = None;
    while position < header.len() {
        let prefix = header.get(position..position.checked_add(8)?)?;
        let size = u32::from_be_bytes(prefix[..4].try_into().ok()?) as usize;
        if size < 8 {
            return None;
        }
        let end = position.checked_add(size)?;
        let atom = header.get(position..end)?;
        if &prefix[4..8] == b"btrt" {
            // ISO btrt contains exactly three unsigned 32-bit declarations.
            if size != 20 {
                return None;
            }
            if let Some(first) = bitrate {
                if atom != first {
                    return None;
                }
            } else {
                bitrate = Some(atom);
                result.extend_from_slice(atom);
            }
        } else {
            result.extend_from_slice(atom);
        }
        position = end;
    }
    Some(result)
}

/// MP4 also appends the first hint when its MOV input has none. In that
/// case the input bitrate comes from the original sample tables, not btrt.
fn equivalent_bytes(before: &[u8], after: &[u8], source_bitrate: Option<u32>) -> bool {
    let (Some(before), Some(after)) = (normalize(before), normalize(after)) else {
        return false;
    };
    if before == after {
        return true;
    }
    let Some(rate) = source_bitrate.filter(|rate| *rate > 0) else {
        return false;
    };
    let mut position = 30;
    while position < before.len() {
        // normalize already validated each complete atom and its checked end.
        if &before[position + 4..position + 8] == b"btrt" {
            return false;
        }
        position += u32::from_be_bytes(before[position..position + 4].try_into().unwrap()) as usize;
    }
    let mut hint = [0u8; 20];
    hint[..8].copy_from_slice(b"\0\0\0\x14btrt");
    hint[12..16].copy_from_slice(&rate.to_be_bytes());
    hint[16..20].copy_from_slice(&rate.to_be_bytes());
    after.len() == before.len() + hint.len()
        && after.starts_with(&before)
        && after[before.len()..] == hint
}

fn decode_dump(text: &str, size: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(size.min(1024 * 1024));
    for line in text.lines().filter(|line| !line.is_empty()) {
        let (offset, columns) = line.split_once(": ")?;
        if usize::from_str_radix(offset, 16).ok()? != bytes.len() {
            return None;
        }
        let hex = columns.split_once("  ").map_or(columns, |(hex, _)| hex);
        let start = bytes.len();
        for word in hex.split_ascii_whitespace() {
            if !matches!(word.len(), 2 | 4) || !word.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            for pair in word.as_bytes().as_chunks::<2>().0.iter() {
                bytes.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
            }
        }
        if !(1..=16).contains(&(bytes.len() - start)) || bytes.len() > size {
            return None;
        }
    }
    (bytes.len() == size).then_some(bytes)
}

async fn header(
    ffprobe: &Path,
    input: &Path,
    stream: &Stream,
    cancel: &watch::Receiver<bool>,
) -> Result<(Vec<u8>, Option<u32>), AppError> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-select_streams",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(stream.index.to_string().into());
    args.extend(
        [
            "-show_streams",
            "-show_data",
            "-show_entries",
            "stream=index,codec_name,extradata_size,extradata,bit_rate",
            "-of",
            "json",
            "-i",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args.push(input.as_os_str().to_owned());
    let output = supervisor::run_capture(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        4 * 1024 * 1024,
        Duration::from_secs(5 * 60),
    )
    .await
    .map_err(|error| super::super::process_error(error, input))?;
    super::super::check_cancel(cancel)?;
    if !output.status.success() {
        return Err(invalid("The copied tx3g codec header could not be read."));
    }
    let response: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| invalid("The copied tx3g codec header is unreadable."))?;
    let streams = response["streams"]
        .as_array()
        .filter(|streams| streams.len() == 1)
        .ok_or_else(|| invalid("The copied tx3g stream selection changed."))?;
    let raw = &streams[0];
    if raw["index"].as_u64() != Some(u64::from(stream.index))
        || raw["codec_name"].as_str() != Some("mov_text")
    {
        return Err(invalid("The copied tx3g stream identity changed."));
    }
    let size = raw["extradata_size"]
        .as_u64()
        .and_then(|size| usize::try_from(size).ok())
        .filter(|size| *size <= output.stdout.len())
        .ok_or_else(|| invalid("The copied tx3g codec header length is invalid."))?;
    let bytes = raw["extradata"]
        .as_str()
        .and_then(|text| decode_dump(text, size))
        .ok_or_else(|| invalid("The copied tx3g codec header bytes are invalid."))?;
    let hash = format!("SHA256:{:x}", Sha256::digest(&bytes));
    if stream.extradata_hash.as_deref() != Some(hash.as_str()) {
        return Err(invalid(
            "The copied tx3g codec header changed during validation.",
        ));
    }
    let bitrate = raw["bit_rate"]
        .as_str()
        .and_then(|value| value.parse::<u32>().ok());
    Ok((bytes, bitrate))
}

pub(super) async fn equivalent(
    ffprobe: &Path,
    input: &Path,
    source: &Stream,
    output: &Path,
    target: &Stream,
    cancel: &watch::Receiver<bool>,
) -> Result<bool, AppError> {
    let (before, bitrate) = header(ffprobe, input, source, cancel).await?;
    let (after, _) = header(ffprobe, output, target, cancel).await?;
    Ok(equivalent_bytes(&before, &after, bitrate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        // Actual qualified FFmpeg9 tx3g header, including Arial font and btrt.
        let hex = concat!(
            "0000000001ff000000ff000000000000",
            "00000000000000010010ffffffff0000",
            "0012667461620001000105417269616c",
            "00000014627472740000000000000148",
            "00000148",
        );
        hex.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn only_identical_valid_bitrate_duplicates_are_redundant() {
        let source = fixture();
        assert_eq!(source.len(), 68);
        let mut copied = source.clone();
        copied.extend_from_slice(&source[48..]);
        assert_eq!(normalize(&source), normalize(&copied));

        let mut changed = copied.clone();
        changed[25] ^= 1; // Default font size/style record.
        assert_ne!(normalize(&source), normalize(&changed));
        let mut changed = copied.clone();
        changed[43] ^= 1; // Font table name.
        assert_ne!(normalize(&source), normalize(&changed));
        let mut changed = copied.clone();
        changed[67] ^= 1; // Conflicting bitrate declaration.
        assert!(normalize(&changed).is_none());
        let mut changed = source.clone();
        changed[67] ^= 1; // A changed, nonduplicated bitrate remains exact.
        assert_ne!(normalize(&source), normalize(&changed));

        let mut unknown = source.clone();
        unknown.extend_from_slice(b"\0\0\0\x09testx");
        assert_ne!(normalize(&source), normalize(&unknown));
        let mut duplicate = unknown.clone();
        duplicate.extend_from_slice(&source[48..]);
        assert_eq!(normalize(&unknown), normalize(&duplicate));
        *duplicate.last_mut().unwrap() ^= 1;
        assert!(normalize(&duplicate).is_none());
    }

    #[test]
    fn first_bitrate_hint_requires_original_rate_and_exact_retained_prefix() {
        let copied = fixture();
        let before = &copied[..48];
        assert!(equivalent_bytes(before, &copied, Some(328)));
        for rate in [None, Some(0), Some(327), Some(329)] {
            assert!(!equivalent_bytes(before, &copied, rate));
        }
        for offset in [25, 43, 51, 59, 63, 67] {
            let mut changed = copied.clone();
            changed[offset] ^= 1;
            assert!(
                !equivalent_bytes(before, &changed, Some(328)),
                "offset {offset}"
            );
        }
        let mut unknown_before = before.to_vec();
        unknown_before.extend_from_slice(b"\0\0\0\x09testx");
        let mut unknown_after = unknown_before.clone();
        unknown_after.extend_from_slice(&copied[48..]);
        assert!(equivalent_bytes(&unknown_before, &unknown_after, Some(328)));
        let mut reordered = copied.clone();
        reordered.extend_from_slice(b"\0\0\0\x09testx");
        assert!(!equivalent_bytes(&unknown_before, &reordered, Some(328)));
    }

    #[test]
    fn malformed_boxes_and_dumps_never_authorize_a_header_difference() {
        let source = fixture();
        for length in [0, 1, 7, 19, 21, u32::MAX] {
            let mut changed = source.clone();
            changed[48..52].copy_from_slice(&length.to_be_bytes());
            assert!(normalize(&changed).is_none(), "length {length}");
        }
        assert!(normalize(&source[..29]).is_none());
        assert!(normalize(&source[..67]).is_none());
        assert_eq!(
            decode_dump("\n00000000: 0001 02  ...\n", 3),
            Some(vec![0, 1, 2])
        );
        assert!(decode_dump("00000001: 0001  ..", 2).is_none());
        assert!(decode_dump("00000000: 000z  ..", 2).is_none());
        assert!(decode_dump("00000000: 0001  ..", 3).is_none());
    }
}
