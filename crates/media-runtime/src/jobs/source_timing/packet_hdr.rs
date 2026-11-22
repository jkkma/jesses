//! Inspect dynamic HDR signaling without decoding pictures. FFmpeg demuxes
//! the selected track and drops image units before sending the metadata pipe.

use super::*;
use dolby_vision::{av1::ITU_T35_DOVI_RPU_PAYLOAD_HEADER, rpu::dovi_rpu::DoviRpu};
use std::{
    io::{self, BufRead, BufReader, Read},
    sync::Arc,
};

const HDR10_PLUS: &[u8] = &[0xb5, 0x00, 0x3c, 0x00, 0x01, 0x04];
const SEI_REGISTERED_T35: usize = 4;
// A large private SEI can be valid. Defer to decoding instead of allocating
// an unbounded buffer or guessing whether a subsequent message carries HDR.
const MAX_NAL_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy)]
enum PacketHdrCodec {
    H264,
    Hevc,
    Vvc,
    Av1,
    ContainerOnly,
}

impl PacketHdrCodec {
    fn filters(self) -> Option<&'static str> {
        Some(match self {
            Self::H264 => "h264_mp4toannexb,filter_units=pass_types=6",
            Self::Hevc => "hevc_mp4toannexb,filter_units=pass_types=39|40|62",
            Self::Vvc => "vvc_mp4toannexb,filter_units=pass_types=23|24",
            Self::Av1 => "filter_units=pass_types=5",
            Self::ContainerOnly => return None,
        })
    }
}

#[derive(Clone, Copy)]
pub(in crate::jobs) struct PacketHdrSource {
    codec: PacketHdrCodec,
    matroska: bool,
}

impl PacketHdrSource {
    pub(in crate::jobs) fn new(codec: Option<&str>, format: Option<&str>) -> Option<Self> {
        let codec = match codec? {
            "h264" => PacketHdrCodec::H264,
            "hevc" => PacketHdrCodec::Hevc,
            "vvc" => PacketHdrCodec::Vvc,
            "av1" => PacketHdrCodec::Av1,
            "vp9" | "ffv1" | "rawvideo" => PacketHdrCodec::ContainerOnly,
            _ => return None,
        };
        let matroska = format?
            .split(',')
            .any(|name| matches!(name, "matroska" | "webm"));
        Some(Self { codec, matroska })
    }

    pub(in crate::jobs) fn needs_inspection(self) -> bool {
        self.matroska || self.codec.filters().is_some()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DynamicHdr {
    Hdr10Plus,
    DolbyVision,
    Other,
}

impl DynamicHdr {
    fn error(self, input: &Path) -> AppError {
        let message = match self {
            Self::Hdr10Plus => {
                "HDR10+ requires explicit HDR10 fallback. Enable HDR10 fallback to encode the static HDR10 base layer."
            }
            Self::DolbyVision => {
                "Dolby Vision requires supported single-layer GPU/RPU rendering or explicit HDR10 fallback using a compatible profile7/8 base layer."
            }
            Self::Other => "The source contains unsupported dynamic HDR metadata.",
        };
        files::error("ENCODE_INPUT_UNSUPPORTED", message, input)
    }
}

/// False requests the existing decoded inspection if an installed FFmpeg
/// cannot filter these packets or the stream has unusual framing.
pub(super) async fn inspect(
    ffmpeg: &Path,
    ffprobe: &Path,
    input: &Path,
    index: u32,
    source: PacketHdrSource,
    checks: Arc<HdrChecks>,
    cancel: &watch::Receiver<bool>,
) -> Result<bool, AppError> {
    // Matroska BlockAdditional can carry HDR10+ outside a compressed picture
    // packet. It is visible as packet side data and absent from the raw NALs.
    if source.matroska
        && !scan(
            ffprobe,
            vec![
                "-v".into(),
                "error".into(),
                "-protocol_whitelist".into(),
                "file".into(),
                "-select_streams".into(),
                index.to_string().into(),
                "-show_entries".into(),
                "packet=stream_index:packet_side_data=side_data_type".into(),
                "-of".into(),
                "compact=p=0:nk=1".into(),
                "-i".into(),
                input.as_os_str().to_owned(),
            ],
            input,
            container_hdr,
            checks.clone(),
            cancel,
        )
        .await?
    {
        return Ok(false);
    }
    let codec = source.codec;
    let Some(filters) = codec.filters() else {
        return Ok(true);
    };
    let mut args = vec!["-v".into(), "error".into(), "-nostdin".into()];
    args.extend(["-protocol_whitelist".into(), "file".into(), "-i".into()]);
    args.push(input.as_os_str().to_owned());
    args.extend([
        "-map".into(),
        format!("0:{index}").into(),
        "-c:v".into(),
        "copy".into(),
        "-copyinkf".into(),
        "-bsf:v".into(),
        filters.into(),
        "-f".into(),
        "data".into(),
        "pipe:1".into(),
    ]);
    scan(
        ffmpeg,
        args,
        input,
        move |reader| match codec {
            PacketHdrCodec::Av1 => av1_obus(reader).map_err(|error| error.to_string()),
            _ => annex_b(reader, codec),
        },
        checks,
        cancel,
    )
    .await
}

fn container_hdr(reader: &mut dyn Read) -> Result<Option<DynamicHdr>, String> {
    for line in BufReader::new(reader).lines() {
        let line = line.map_err(|error| error.to_string())?;
        for field in line.split('|').skip(1) {
            match field {
                "HDR10+ Dynamic Metadata (SMPTE 2094-40)" => {
                    return Ok(Some(DynamicHdr::Hdr10Plus));
                }
                "DOVI configuration record" => return Ok(Some(DynamicHdr::DolbyVision)),
                "HDR Dynamic Metadata (SMPTE 2094-50)" => return Ok(Some(DynamicHdr::Other)),
                _ => {}
            }
        }
    }
    Ok(None)
}

async fn scan(
    tool: &Path,
    args: Vec<OsString>,
    input: &Path,
    inspect: impl FnOnce(&mut dyn Read) -> Result<Option<DynamicHdr>, String> + Send + 'static,
    checks: Arc<HdrChecks>,
    cancel: &watch::Receiver<bool>,
) -> Result<bool, AppError> {
    let input = input.to_owned();
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: tool.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(24 * 60 * 60),
        move |reader| {
            if let Some(kind) = inspect(reader)? {
                checks.reject(kind.error(&input));
                return Err("Dynamic HDR metadata was found.".into());
            }
            Ok(())
        },
    )
    .await;
    check_cancel(cancel)?;
    Ok(result.is_ok_and(|result| result.status.success() && result.stderr.is_empty()))
}

fn annex_b(reader: &mut dyn Read, codec: PacketHdrCodec) -> Result<Option<DynamicHdr>, String> {
    let mut nal = Vec::new();
    let mut after_start_code = false;
    let mut zeros = 0usize;
    for byte in BufReader::new(reader).bytes() {
        let byte = byte.map_err(|error| error.to_string())?;
        if byte == 0 {
            zeros = zeros.saturating_add(1);
            continue;
        }
        if byte == 1 && zeros >= 2 {
            if let Some(found) = inspect_nal(&mut nal, codec)? {
                return Ok(Some(found));
            }
            nal.clear();
            after_start_code = true;
        } else if after_start_code && zeros < MAX_NAL_BYTES - nal.len() {
            nal.resize(nal.len() + zeros, 0);
            nal.push(byte);
        } else {
            return Err("Cannot inspect this Annex B metadata unit.".into());
        }
        zeros = 0;
    }
    inspect_nal(&mut nal, codec)
}

fn inspect_nal(nal: &mut Vec<u8>, codec: PacketHdrCodec) -> Result<Option<DynamicHdr>, String> {
    let Some(&first) = nal.first() else {
        return Ok(None);
    };
    let header_size = match codec {
        PacketHdrCodec::Hevc if (first >> 1) & 0x3f == 62 => {
            return Ok(DoviRpu::parse_unspec62_nalu(nal)
                .is_ok()
                .then_some(DynamicHdr::DolbyVision));
        }
        PacketHdrCodec::Hevc if matches!((first >> 1) & 0x3f, 39 | 40) => 2,
        PacketHdrCodec::H264 if first & 0x1f == 6 => 1,
        PacketHdrCodec::Vvc if nal.get(1).is_some_and(|byte| matches!(byte >> 3, 23 | 24)) => 2,
        _ => return Err("Unexpected packet in the filtered metadata stream.".into()),
    };
    // Undo Annex B emulation prevention before walking SEI payloads.
    let mut zeros = 0;
    let mut offset = 0;
    nal.retain(|&byte| {
        offset += 1;
        if offset <= header_size {
            return true;
        }
        if zeros == 2 && byte == 3 {
            zeros = 0;
            return false;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        true
    });
    inspect_sei(nal.get(header_size..).unwrap_or_default())
        .ok_or_else(|| "Cannot read the filtered SEI messages.".into())
}

fn sei_number(input: &mut &[u8]) -> Option<usize> {
    let mut value = 0usize;
    loop {
        let (&byte, rest) = input.split_first()?;
        *input = rest;
        value = value.checked_add(usize::from(byte))?;
        if byte != 255 {
            return Some(value);
        }
    }
}

fn inspect_sei(mut messages: &[u8]) -> Option<Option<DynamicHdr>> {
    while messages != [0x80] {
        let kind = sei_number(&mut messages)?;
        let length = sei_number(&mut messages)?;
        let (payload, rest) = messages.split_at_checked(length)?;
        if kind == SEI_REGISTERED_T35
            && let Some(found) = t35(payload)
        {
            return Some(Some(found));
        }
        messages = rest;
    }
    Some(None)
}

fn t35(payload: &[u8]) -> Option<DynamicHdr> {
    if payload.starts_with(HDR10_PLUS) {
        Some(DynamicHdr::Hdr10Plus)
    } else if payload
        .strip_prefix(&[0xb5])
        .is_some_and(|payload| payload.starts_with(ITU_T35_DOVI_RPU_PAYLOAD_HEADER))
    {
        Some(DynamicHdr::DolbyVision)
    } else if payload.starts_with(&[0x26, 0x00, 0x04])
        || payload.starts_with(&[0xb5, 0x00, 0x90, 0x00, 0x01])
    {
        Some(DynamicHdr::Other)
    } else {
        None
    }
}

fn leb128(reader: &mut impl Read) -> io::Result<u64> {
    let mut value = 0;
    for shift in (0..56).step_by(7) {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid AV1 OBU size.",
    ))
}

fn av1_obus(reader: &mut dyn Read) -> io::Result<Option<DynamicHdr>> {
    let mut reader = BufReader::new(reader);
    let mut header = [0];
    while reader.read(&mut header)? != 0 {
        let header = header[0];
        // Low-overhead AV1 OBUs have a size field. The filter emits only
        // metadata OBUs; an unexpected header asks for decoded inspection.
        if header & 0xfb != 0x2a {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected AV1 metadata OBU header.",
            ));
        }
        if header & 0x04 != 0 {
            reader.read_exact(&mut [0])?;
        }
        let length = leb128(&mut reader)?;
        let mut obu = reader.by_ref().take(length);
        if leb128(&mut obu)? == 4 {
            let mut prefix = Vec::new();
            obu.by_ref()
                .take(1 + ITU_T35_DOVI_RPU_PAYLOAD_HEADER.len() as u64)
                .read_to_end(&mut prefix)?;
            if let Some(found) = t35(&prefix) {
                return Ok(Some(found));
            }
        }
        io::copy(&mut obu, &mut io::sink())?;
        if obu.limit() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Incomplete AV1 metadata OBU.",
            ));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hevc_t35_after_static_sei_is_found() {
        let mut source = vec![0, 0, 0, 1, 0x4e, 1, 137, 6];
        // Static mastering payload containing the same byte pattern.
        source.extend(HDR10_PLUS);
        source.push(0x80);
        assert_eq!(
            annex_b(&mut source.as_slice(), PacketHdrCodec::Hevc),
            Ok(None)
        );

        source.extend([0, 0, 1, 0x50, 1, 4, 6]);
        source.extend(HDR10_PLUS);
        source.push(0x80);
        assert_eq!(
            annex_b(&mut source.as_slice(), PacketHdrCodec::Hevc),
            Ok(Some(DynamicHdr::Hdr10Plus))
        );
    }

    #[test]
    fn av1_metadata_checks_its_payload_type_and_size() {
        let mut source = vec![0x2a, 8, 1];
        source.extend(HDR10_PLUS);
        source.push(0x80);
        assert_eq!(av1_obus(&mut source.as_slice()).unwrap(), None);
        source.extend([0x2e, 0, 8, 4]);
        source.extend(HDR10_PLUS);
        source.push(0x80);
        assert_eq!(
            av1_obus(&mut source.as_slice()).unwrap(),
            Some(DynamicHdr::Hdr10Plus)
        );
        let truncated = &[0x2a, 8, 1, 0x80];
        assert!(av1_obus(&mut truncated.as_slice()).is_err());
    }

    #[test]
    fn matroska_packet_types_distinguish_static_and_dynamic_hdr() {
        let static_hdr = b"0\n0|Mastering display metadata|Content light level metadata\n";
        assert_eq!(container_hdr(&mut static_hdr.as_slice()), Ok(None));
        let dynamic = b"0\n0|HDR10+ Dynamic Metadata (SMPTE 2094-40)\n";
        assert_eq!(
            container_hdr(&mut dynamic.as_slice()),
            Ok(Some(DynamicHdr::Hdr10Plus))
        );
    }
}
