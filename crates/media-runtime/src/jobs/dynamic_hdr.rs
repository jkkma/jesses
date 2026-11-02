//! External metadata remains a separately guarded source. Only complete,
//! unchanged frames are admitted; the encoded ITU-T T.35 payload is checked at
//! its presentation timestamp before any output can be published.
use super::*;
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use hdr10plus::{
    metadata::{Hdr10PlusMetadata, Hdr10PlusMetadataEncOpts},
    metadata_json::MetadataJsonRoot,
};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

const LIMIT: u64 = 256 * 1024 * 1024;
pub(super) const DOVI_NAME: &str = "jesses-dovi.rpu";
pub(super) const HDR10_NAME: &str = "jesses-hdr10plus.json";

fn invalid(path: &Path, message: impl Into<String>) -> AppError {
    files::error("HDR_METADATA_INVALID", message, path)
}
fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn rpu_identity(rpu: &DoviRpu) -> Result<[u8; 32], String> {
    let mut value = serde_json::to_value(rpu).map_err(|e| e.to_string())?;
    value
        .as_object_mut()
        .expect("RPU object")
        .remove("rpu_data_crc32");
    fn canonical(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    canonical(value);
                    if key == "ext_metadata_blocks"
                        && let Some(blocks) = value.as_array_mut()
                    {
                        blocks.sort_by_cached_key(serde_json::Value::to_string);
                    }
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    canonical(value)
                }
            }
            _ => {}
        }
    }
    // The Dolby writer and FFmpeg can order same-level extension records
    // differently. Their complete fields remain authoritative; CRC is already
    // verified by the native parser and reflects that representation order.
    canonical(&mut value);
    Ok(hash(
        &serde_json::to_vec(&value).map_err(|e| e.to_string())?,
    ))
}

struct Resource {
    source: Source,
    bytes: Vec<u8>,
    expected: Vec<[u8; 32]>,
    name: &'static str,
    option: &'static str,
}
#[derive(Default)]
pub(super) struct Resources {
    values: Vec<Resource>,
}

fn rpu_hashes(bytes: &[u8], frames: usize) -> Result<Vec<[u8; 32]>, String> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let size = if bytes[index..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if bytes[index..].starts_with(&[0, 0, 1]) {
            3
        } else {
            index += 1;
            continue;
        };
        starts.push((index, index + size));
        index += size;
    }
    if starts.len() != frames || starts.first().is_none_or(|(start, _)| *start != 0) {
        return Err(format!(
            "The RPU must contain exactly {frames} Annex-B RPU records, one for every source frame; found {}.",
            starts.len()
        ));
    }
    let mut hashes = Vec::with_capacity(frames);
    for (index, (_, start)) in starts.iter().enumerate() {
        let end = starts
            .get(index + 1)
            .map_or(bytes.len(), |(start, _)| *start);
        if end - start > 1024 * 1024 {
            return Err("An RPU record exceeds 1 MiB.".into());
        }
        let rpu = DoviRpu::parse_unspec62_nalu(&bytes[*start..end])
            .map_err(|error| format!("RPU frame {index}: {error}"))?;
        // Profile 7 requires enhancement-layer reconstruction. Profile 8.4 has
        // HLG reshaping. Neither can be applied to this unchanged PQ base layer.
        if rpu.dovi_profile != 8
            || rpu.el_type.is_some()
            || rpu.rpu_data_mapping.as_ref().is_none_or(|mapping| {
                mapping.nlq.is_some()
                    || serde_json::to_value(&mapping.curves).ok()
                        != serde_json::to_value(
                            dolby_vision::rpu::profiles::profile81::Profile81::rpu_data_mapping()
                                .curves,
                        )
                        .ok()
            })
            || rpu
                .vdr_dm_data
                .as_ref()
                .is_none_or(|dm| dm.signal_color_space != 0 || dm.signal_full_range_flag != 1)
        {
            return Err(format!(
                "RPU frame {index} is not a complete profile 8.1 HDR10-compatible RPU. Convert profile 7 to 8.1 first."
            ));
        }
        hashes.push(rpu_identity(&rpu)?);
    }
    Ok(hashes)
}
fn json_hashes(bytes: &[u8], frames: usize) -> Result<Vec<[u8; 32]>, String> {
    let root: MetadataJsonRoot =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if root.scene_info.len() != frames {
        return Err(format!(
            "HDR10+ JSON must contain exactly {frames} SceneInfo records; found {}.",
            root.scene_info.len()
        ));
    }
    root.scene_info.iter().enumerate().map(|(index, frame)| {
        if frame.sequence_frame_index != index || frame.number_of_windows != 1 {
            return Err(format!("HDR10+ frame {index} has an invalid SequenceFrameIndex or unsupported processing windows."));
        }
        let metadata = Hdr10PlusMetadata::try_from(frame).map_err(|e| e.to_string())?;
        metadata.encode_with_opts(&Hdr10PlusMetadataEncOpts { validate: true, with_country_code: true }).map(|data| hash(&data)).map_err(|e| format!("HDR10+ frame {index}: {e}"))
    }).collect()
}

impl Resources {
    pub(super) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    pub(super) async fn prepare(
        settings: &EncodeSettings,
        frames: usize,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Self, AppError> {
        let settings = settings.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let mut values = Vec::new();
            if let Some(metadata) = &settings.hdr_metadata {
                for (path, name, option, dovi) in [
                    (
                        metadata.dolby_vision_rpu_path.as_deref(),
                        DOVI_NAME,
                        "--dolby-vision-rpu",
                        true,
                    ),
                    (
                        metadata.hdr10_plus_json_path.as_deref(),
                        HDR10_NAME,
                        "--hdr10plus-json",
                        false,
                    ),
                ] {
                    let Some(path) = path else { continue };
                    check_cancel(&cancel)?;
                    let source = Source::open(Path::new(path))?;
                    let mut bytes = Vec::new();
                    let mut reader = std::fs::File::open(&source.path)
                        .map_err(|e| invalid(&source.path, e.to_string()))?
                        .take(LIMIT + 1);
                    let mut chunk = [0u8; 65536];
                    loop {
                        check_cancel(&cancel)?;
                        let count = reader
                            .read(&mut chunk)
                            .map_err(|e| invalid(&source.path, e.to_string()))?;
                        if count == 0 {
                            break;
                        }
                        bytes.extend_from_slice(&chunk[..count]);
                    }
                    if bytes.len() as u64 > LIMIT {
                        return Err(invalid(
                            &source.path,
                            "An HDR metadata resource exceeds 256 MiB.",
                        ));
                    }
                    let expected = if dovi {
                        rpu_hashes(&bytes, frames)
                    } else {
                        json_hashes(&bytes, frames)
                    }
                    .map_err(|error| invalid(&source.path, error))?;
                    source.verify()?;
                    check_cancel(&cancel)?;
                    values.push(Resource {
                        source,
                        bytes,
                        expected,
                        name,
                        option,
                    });
                }
            }
            Ok(Self { values })
        })
        .await
        .map_err(|e| AppError::new("HDR_METADATA_INVALID", e.to_string(), None))?
    }
    pub(super) fn parameters(args: &mut Vec<OsString>, settings: &EncodeSettings) {
        if let Some(metadata) = &settings.hdr_metadata {
            if metadata.dolby_vision_rpu_path.is_some() {
                args.extend(["--dolby-vision-rpu".into(), DOVI_NAME.into()]);
            }
            if metadata.hdr10_plus_json_path.is_some() {
                args.extend(["--hdr10plus-json".into(), HDR10_NAME.into()]);
            }
        }
    }
    pub(super) fn stage_av1an(&self, work: &Path) -> Result<Vec<Source>, AppError> {
        let mut guards = Vec::new();
        for resource in &self.values {
            resource.source.verify()?;
            let path = work.join(resource.name);
            let exists = path
                .try_exists()
                .map_err(|e| invalid(&path, e.to_string()))?;
            let staged = Temporary::durable(&path, exists)?;
            let mut writer = staged.clone_file()?;
            if exists {
                let mut bytes = Vec::new();
                (&mut writer)
                    .take(LIMIT + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| invalid(&path, e.to_string()))?;
                if bytes != resource.bytes {
                    return Err(invalid(
                        &path,
                        "The saved HDR resource differs from the immutable input. The workspace was preserved.",
                    ));
                }
            } else {
                writer
                    .write_all(&resource.bytes)
                    .and_then(|_| writer.sync_all())
                    .map_err(|e| invalid(&path, e.to_string()))?;
            }
            drop(writer);
            drop(staged);
            guards.push(Source::open(&path)?);
        }
        Ok(guards)
    }
    pub(super) fn stage_standalone(&self, output: &Path, id: &str) -> Result<Staged, AppError> {
        let mut staged = Staged {
            files: Vec::new(),
            guards: Vec::new(),
            paths: Vec::new(),
        };
        for resource in &self.values {
            resource.source.verify()?;
            let mut temporary =
                Temporary::create_extension(output, &format!("{id}-{}", resource.name), "hdr")?;
            let mut writer = temporary.clone_file()?;
            writer
                .write_all(&resource.bytes)
                .and_then(|_| writer.sync_all())
                .map_err(|e| invalid(&temporary.path, e.to_string()))?;
            drop(writer);
            temporary.close_for_path_writer()?;
            let guard = Source::open(&temporary.path)?;
            staged.paths.push((resource.option, temporary.path.clone()));
            staged.files.push(temporary);
            staged.guards.push(guard);
        }
        Ok(staged)
    }
    pub(super) fn verify_sources(&self) -> Result<(), AppError> {
        for value in &self.values {
            value.source.verify()?;
        }
        Ok(())
    }
    pub(super) async fn verify_output(
        &self,
        ffmpeg: &Path,
        ffprobe: &Path,
        input: &Path,
        video_index: usize,
        fps: (u32, u32),
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        if self.values.is_empty() {
            return Ok(());
        }
        self.verify_sources()?;
        if self.values.iter().any(|value| value.name == DOVI_NAME) {
            let document = probe(ffprobe, input, cancel, Some(&[video_index as u32])).await?;
            let configurations = document
                .streams
                .iter()
                .find(|stream| stream.index == video_index as u32)
                .into_iter()
                .flat_map(|stream| &stream.side_data_list)
                .filter(|value| value["side_data_type"] == "DOVI configuration record")
                .collect::<Vec<_>>();
            if configurations.len() != 1
                || !configurations.iter().all(|value| {
                    let n = |key| value.get(key).and_then(serde_json::Value::as_u64);
                    n("dv_version_major") == Some(1)
                        && n("dv_version_minor") == Some(0)
                        && n("dv_profile") == Some(10)
                        && n("dv_level").is_some_and(|level| (1..=13).contains(&level))
                        && n("rpu_present_flag") == Some(1)
                        && n("el_present_flag") == Some(0)
                        && n("bl_present_flag") == Some(1)
                        && n("dv_bl_signal_compatibility_id") == Some(1)
                        && value
                            .get("dv_md_compression")
                            .and_then(serde_json::Value::as_str)
                            == Some("none")
                })
            {
                return Err(invalid(
                    input,
                    "The final container lacks the expected AV1 Dolby Vision profile10/HDR10-compatible configuration with an uncompressed RPU, base layer and no enhancement layer.",
                ));
            }
        }
        let expected = self
            .values
            .iter()
            .map(|r| (r.name == DOVI_NAME, r.expected.clone()))
            .collect::<Vec<_>>();
        let output = supervisor::run_streaming_stdout(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args: vec![
                    "-v".into(),
                    "error".into(),
                    "-nostdin".into(),
                    "-i".into(),
                    input.as_os_str().to_owned(),
                    "-map".into(),
                    format!("0:{video_index}").into(),
                    "-c:v".into(),
                    "copy".into(),
                    "-f".into(),
                    "ivf".into(),
                    "pipe:1".into(),
                ],
                cwd: None,
            },
            cancel.clone(),
            64 * 1024,
            Duration::from_secs(24 * 60 * 60),
            move |reader| verify_ivf(reader, &expected, fps),
        )
        .await
        .map_err(|error| process_error(error, input))?;
        if !output.status.success() || !output.stderr.is_empty() {
            return Err(invalid(
                input,
                format!(
                    "The encoded dynamic HDR payload could not be verified: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            ));
        }
        self.verify_sources()
    }
}

pub(super) struct Staged {
    files: Vec<Temporary>,
    guards: Vec<Source>,
    paths: Vec<(&'static str, PathBuf)>,
}
impl Staged {
    pub(super) fn apply(&self, args: &mut [OsString]) {
        for (option, path) in &self.paths {
            if let Some(index) = args.iter().position(|arg| arg == option) {
                args[index + 1] = path.as_os_str().to_owned();
            }
        }
    }
    pub(super) fn finish(mut self) -> Result<(), AppError> {
        self.guards.clear();
        for file in &mut self.files {
            file.reopen_after_path_writer()?;
            file.cleanup()?;
        }
        self.files.clear();
        Ok(())
    }
}
impl Drop for Staged {
    fn drop(&mut self) {
        self.guards.clear();
        for file in &mut self.files {
            if file.reopen_after_path_writer().is_ok() {
                let _ = file.cleanup();
            }
        }
    }
}

pub(super) async fn copy_encoded(
    source: &Temporary,
    target: &Temporary,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let mut read = source.clone_file()?;
    let mut write = target.clone_file()?;
    let cancel = cancel.clone();
    let path = target.path.clone();
    tokio::task::spawn_blocking(move || {
        use std::io::{Seek, SeekFrom};
        let error =
            |error: std::io::Error| files::error("OUTPUT_WRITE_FAILED", error.to_string(), &path);
        read.seek(SeekFrom::Start(0))
            .and_then(|_| write.seek(SeekFrom::Start(0)))
            .and_then(|_| write.set_len(0))
            .map_err(error)?;
        let mut chunk = [0u8; 65536];
        loop {
            check_cancel(&cancel)?;
            let count = read.read(&mut chunk).map_err(error)?;
            if count == 0 {
                break;
            }
            write.write_all(&chunk[..count]).map_err(error)?;
        }
        write.sync_all().map_err(error)
    })
    .await
    .map_err(|error| AppError::new("OUTPUT_WRITE_FAILED", error.to_string(), None))?
}

fn leb(reader: &mut dyn Read, remaining: &mut u64) -> Result<u64, String> {
    let mut value = 0;
    for shift in (0..56).step_by(7) {
        if *remaining == 0 {
            return Err("Truncated AV1 OBU size.".into());
        }
        let mut byte = [0];
        reader.read_exact(&mut byte).map_err(|e| e.to_string())?;
        *remaining -= 1;
        value |= u64::from(byte[0] & 127) << shift;
        if byte[0] & 128 == 0 {
            return Ok(value);
        }
    }
    Err("Invalid AV1 OBU size.".into())
}
fn verify_ivf(
    reader: &mut dyn Read,
    expected: &[(bool, Vec<[u8; 32]>)],
    fps: (u32, u32),
) -> Result<(), String> {
    let mut header = [0u8; 32];
    reader.read_exact(&mut header).map_err(|e| e.to_string())?;
    if &header[..4] != b"DKIF"
        || &header[8..12] != b"AV01"
        || u16::from_le_bytes(header[6..8].try_into().unwrap()) != 32
    {
        return Err("Invalid AV1 IVF header.".into());
    }
    let rate = u32::from_le_bytes(header[16..20].try_into().unwrap());
    let scale = u32::from_le_bytes(header[20..24].try_into().unwrap());
    if rate == 0 || scale == 0 {
        return Err("Invalid IVF time base.".into());
    }
    let mut observed: Vec<Vec<bool>> = expected.iter().map(|(_, v)| vec![false; v.len()]).collect();
    loop {
        let mut packet = [0u8; 12];
        if reader.read(&mut packet[..1]).map_err(|e| e.to_string())? == 0 {
            break;
        }
        reader
            .read_exact(&mut packet[1..])
            .map_err(|e| e.to_string())?;
        let mut remaining = u64::from(u32::from_le_bytes(packet[..4].try_into().unwrap()));
        let pts = u64::from_le_bytes(packet[4..].try_into().unwrap());
        let numerator = u128::from(pts) * u128::from(scale) * u128::from(fps.0);
        let denominator = u128::from(rate) * u128::from(fps.1);
        let index = usize::try_from((numerator + denominator / 2) / denominator)
            .map_err(|e| e.to_string())?;
        while remaining > 0 {
            let mut byte = [0u8];
            reader.read_exact(&mut byte).map_err(|e| e.to_string())?;
            remaining -= 1;
            let header = byte[0];
            if header & 0x83 != 2 {
                return Err("Invalid or unbounded AV1 OBU header.".into());
            }
            if header & 4 != 0 {
                if remaining == 0 {
                    return Err("Truncated AV1 extension.".into());
                }
                reader.read_exact(&mut byte).map_err(|e| e.to_string())?;
                remaining -= 1;
            }
            let size = leb(reader, &mut remaining)?;
            if size > remaining {
                return Err("Truncated AV1 OBU payload.".into());
            }
            remaining -= size;
            if (header >> 3) & 15 == 5 {
                if size > 1024 * 1024 {
                    return Err("An AV1 metadata OBU exceeds1MiB.".into());
                }
                let mut data = vec![0u8; size as usize];
                reader.read_exact(&mut data).map_err(|e| e.to_string())?;
                if data.first() != Some(&4) {
                    continue;
                } // metadata_type ITU-T T.35
                let mut payload = &data[1..];
                if payload.last() == Some(&0x80) {
                    payload = &payload[..payload.len() - 1];
                }
                let dovi = payload.starts_with(&[0xB5, 0, 0x3B]);
                let hdr10 = payload.starts_with(&[0xB5, 0, 0x3C, 0, 1, 4]);
                if !dovi && !hdr10 {
                    continue;
                }
                let Some(kind) = expected.iter().position(|(kind, _)| *kind == dovi) else {
                    return Err("Unexpected dynamic HDR metadata in encoded video.".into());
                };
                let actual = if dovi {
                    rpu_identity(
                        &DoviRpu::parse_itu_t35_dovi_metadata_obu(payload)
                            .map_err(|error| error.to_string())?,
                    )?
                } else {
                    hash(payload)
                };
                if expected[kind].1.get(index) != Some(&actual) {
                    return Err(format!(
                        "Encoded {} payload differs from imported frame {index} (IVF PTS{pts}).",
                        if dovi { "Dolby Vision" } else { "HDR10+" }
                    ));
                }
                if observed[kind][index] {
                    return Err(format!(
                        "Duplicate dynamic HDR payload for source frame {index}."
                    ));
                }
                observed[kind][index] = true;
            } else {
                let copied = std::io::copy(&mut reader.take(size), &mut std::io::sink())
                    .map_err(|e| e.to_string())?;
                if copied != size {
                    return Err("Truncated AV1 frame.".into());
                }
            }
        }
    }
    if observed
        .iter()
        .any(|frames| frames.iter().any(|present| !*present))
    {
        return Err("The encoder omitted dynamic HDR metadata from one or more source frames. This build may lack the required metadata library.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dolby_vision::rpu::generate::GenerateConfig;
    fn rpu(config: GenerateConfig, profile: u8) -> Vec<u8> {
        let value = if profile == 5 {
            DoviRpu::profile5_config(&config)
        } else if profile == 84 {
            DoviRpu::profile84_config(&config)
        } else {
            DoviRpu::profile81_config(&config)
        }
        .unwrap();
        let mut bytes = vec![0, 0, 0, 1];
        bytes.extend(&value.write_hevc_unspec62_nalu().unwrap()[2..]);
        bytes
    }
    #[test]
    fn resources_require_exact_complete_valid_frame_records() {
        let bytes = rpu(Default::default(), 81);
        assert_eq!(rpu_hashes(&bytes, 1).unwrap().len(), 1);
        assert!(rpu_hashes(&bytes, 2).is_err());
        assert!(rpu_hashes(&bytes[..bytes.len() - 5], 1).is_err());
        assert!(rpu_hashes(&rpu(Default::default(), 5), 1).is_err());
        assert!(rpu_hashes(&rpu(Default::default(), 84), 1).is_err());
        assert!(json_hashes(br#"{}"#, 1).is_err());
        let frame = Hdr10PlusMetadata {
            profile: "A".into(),
            itu_t_t35_country_code: 0xB5,
            itu_t_t35_terminal_provider_code: 0x3C,
            itu_t_t35_terminal_provider_oriented_code: 1,
            application_identifier: 4,
            application_version: 1,
            num_windows: 1,
            num_distribution_maxrgb_percentiles: 9,
            distribution_maxrgb: [1, 5, 10, 25, 50, 75, 90, 95, 99]
                .into_iter()
                .enumerate()
                .map(
                    |(index, percentage)| hdr10plus::metadata::DistributionMaxRgb {
                        percentage,
                        percentile: index as u32 * 100,
                    },
                )
                .collect(),
            ..Default::default()
        };
        let mut json = hdr10plus::metadata_json::generate_json(&[&frame], "Jesses", "1");
        json_hashes(&serde_json::to_vec(&json).unwrap(), 1).unwrap();
        assert!(json_hashes(&serde_json::to_vec(&json).unwrap(), 2).is_err());
        json["SceneInfo"][0]["SequenceFrameIndex"] = 1.into();
        assert!(json_hashes(&serde_json::to_vec(&json).unwrap(), 1).is_err());
    }
    #[test]
    fn rpu_identity_only_normalizes_extension_block_order_and_derived_crc() {
        use dolby_vision::rpu::extension_metadata::{DmData, WithExtMetadataBlocks};
        let a = DoviRpu::profile81_config(&GenerateConfig::default()).unwrap();
        let mut b = a.clone();
        b.rpu_data_crc32 = a.rpu_data_crc32.wrapping_add(1);
        if let Some(DmData::V29(data)) = &mut b.vdr_dm_data.as_mut().unwrap().cmv29_metadata {
            data.blocks_mut().reverse();
        }
        if let Some(DmData::V40(data)) = &mut b.vdr_dm_data.as_mut().unwrap().cmv40_metadata {
            data.blocks_mut().reverse();
        }
        assert_eq!(rpu_identity(&a).unwrap(), rpu_identity(&b).unwrap());
        b.vdr_dm_data.as_mut().unwrap().source_max_pq += 1;
        assert_ne!(rpu_identity(&a).unwrap(), rpu_identity(&b).unwrap());
        let mut b = a.clone();
        b.header.vdr_rpu_level += 1;
        assert_ne!(rpu_identity(&a).unwrap(), rpu_identity(&b).unwrap());
    }
    fn ivf(payloads: &[(u64, Vec<u8>)]) -> Vec<u8> {
        let mut bytes = b"DKIF\0\0\x20\0AV01\x80\0\x60\0".to_vec();
        bytes.extend(24u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend([0; 8]);
        for (pts, payload) in payloads {
            let mut obu = vec![0x2a, (payload.len() + 2) as u8, 4];
            obu.extend(payload);
            obu.push(0x80);
            bytes.extend((obu.len() as u32).to_le_bytes());
            bytes.extend(pts.to_le_bytes());
            bytes.extend(obu);
        }
        bytes
    }
    #[test]
    fn encoded_t35_must_match_source_presentation_order_without_omission_or_duplicates() {
        let a = vec![0xb5, 0, 0x3c, 0, 1, 4, 0x10];
        let b = vec![0xb5, 0, 0x3c, 0, 1, 4, 0x20];
        let expected = vec![(false, vec![hash(&a), hash(&b)])];
        // Coded packet order can differ. Its exact PTS identifies the source.
        assert!(
            verify_ivf(
                &mut ivf(&[(1, b.clone()), (0, a.clone())]).as_slice(),
                &expected,
                (24, 1)
            )
            .is_ok()
        );
        for packets in [
            vec![(0, a.clone())],
            vec![(0, a.clone()), (0, a.clone()), (1, b.clone())],
            vec![(0, b.clone()), (1, a.clone())],
        ] {
            assert!(verify_ivf(&mut ivf(&packets).as_slice(), &expected, (24, 1)).is_err());
        }
        let mut bytes = ivf(&[(0, a), (1, b)]);
        bytes.pop();
        assert!(verify_ivf(&mut bytes.as_slice(), &expected, (24, 1)).is_err());
    }
}
