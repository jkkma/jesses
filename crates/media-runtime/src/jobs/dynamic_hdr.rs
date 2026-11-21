//! Validate per-frame HDR inputs and stage guarded copies for the encoders.
use super::*;
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use hdr10plus::{
    metadata::{Hdr10PlusMetadata, Hdr10PlusMetadataEncOpts},
    metadata_json::MetadataJsonRoot,
};
use std::io::{Read, Write};

const LIMIT: u64 = 256 * 1024 * 1024;
pub(super) const DOVI_NAME: &str = "jesses-dovi.rpu";
pub(super) const HDR10_NAME: &str = "jesses-hdr10plus.json";

fn invalid(path: &Path, message: impl Into<String>) -> AppError {
    files::error("HDR_METADATA_INVALID", message, path)
}
struct Resource {
    source: Source,
    bytes: Vec<u8>,
    name: &'static str,
    option: &'static str,
}
#[derive(Default)]
pub(super) struct Resources {
    values: Vec<Resource>,
}

fn validate_rpu(bytes: &[u8], frames: usize) -> Result<(), String> {
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
    }
    Ok(())
}
fn validate_json(bytes: &[u8], frames: usize) -> Result<(), String> {
    let root: MetadataJsonRoot =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if root.scene_info.len() != frames {
        return Err(format!(
            "HDR10+ JSON must contain exactly {frames} SceneInfo records; found {}.",
            root.scene_info.len()
        ));
    }
    for (index, frame) in root.scene_info.iter().enumerate() {
        if frame.sequence_frame_index != index || frame.number_of_windows != 1 {
            return Err(format!(
                "HDR10+ frame {index} has an invalid SequenceFrameIndex or unsupported processing windows."
            ));
        }
        let metadata = Hdr10PlusMetadata::try_from(frame).map_err(|e| e.to_string())?;
        metadata
            .encode_with_opts(&Hdr10PlusMetadataEncOpts {
                validate: true,
                with_country_code: true,
            })
            .map_err(|e| format!("HDR10+ frame {index}: {e}"))?;
    }
    Ok(())
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
                    if dovi {
                        validate_rpu(&bytes, frames)
                    } else {
                        validate_json(&bytes, frames)
                    }
                    .map_err(|error| invalid(&source.path, error))?;
                    source.verify()?;
                    check_cancel(&cancel)?;
                    values.push(Resource {
                        source,
                        bytes,
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
            if exists {
                let staged = Temporary::durable(&path, true)?;
                let mut bytes = Vec::new();
                staged
                    .clone_file()?
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
                // Recovery compares this name with the saved source fingerprint
                // before any Resume or Discard, so a failed or interrupted write
                // must never leave a partial copy under it.
                super::av1an::write_complete_recovery_file(
                    &path,
                    &resource.bytes,
                    &|path, cause| invalid(path, cause),
                )?;
            }
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
        validate_rpu(&bytes, 1).unwrap();
        assert!(validate_rpu(&bytes, 2).is_err());
        assert!(validate_rpu(&bytes[..bytes.len() - 5], 1).is_err());
        assert!(validate_rpu(&rpu(Default::default(), 5), 1).is_err());
        assert!(validate_rpu(&rpu(Default::default(), 84), 1).is_err());
        assert!(validate_json(br#"{}"#, 1).is_err());
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
        validate_json(&serde_json::to_vec(&json).unwrap(), 1).unwrap();
        assert!(validate_json(&serde_json::to_vec(&json).unwrap(), 2).is_err());
        json["SceneInfo"][0]["SequenceFrameIndex"] = 1.into();
        assert!(validate_json(&serde_json::to_vec(&json).unwrap(), 1).is_err());
    }
}
