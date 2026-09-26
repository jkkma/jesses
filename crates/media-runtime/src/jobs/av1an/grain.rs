use super::*;
use std::io::{Read, Write};
pub(in crate::jobs) const TABLE_NAME: &str = "jesses-grain.tbl";

/// Standalone encoders may open a grain table while denying write sharing on
/// Windows. Keep a read-only identity guard during encoding and retain the
/// reserved temporary's identity for safe cleanup afterward.
pub(in crate::jobs) struct StandaloneTable {
    temporary: Temporary,
    guard: Option<Source>,
}

impl StandaloneTable {
    pub(in crate::jobs) fn path(&self) -> &Path {
        &self.temporary.path
    }
}

impl Drop for StandaloneTable {
    fn drop(&mut self) {
        self.guard.take();
        if self.temporary.reopen_after_path_writer().is_ok() {
            let _ = self.temporary.cleanup();
        }
    }
}

pub(in crate::jobs) fn stage_standalone(
    output: &Path,
    attempt_id: &str,
    settings: &EncodeSettings,
) -> Result<Option<StandaloneTable>, AppError> {
    let Some(table) = settings
        .av1an_grain
        .as_ref()
        .and_then(|grain| grain.table.as_ref())
    else {
        return Ok(None);
    };
    validate(settings)?;
    let mut temporary = Temporary::create_extension(output, &format!("{attempt_id}-grain"), "tbl")?;
    let mut writer = temporary.clone_file()?;
    writer
        .write_all(table.as_bytes())
        .and_then(|_| writer.sync_all())
        .map_err(|error| {
            files::error(
                "GRAIN_TABLE_WRITE_FAILED",
                error.to_string(),
                &temporary.path,
            )
        })?;
    drop(writer);
    temporary.close_for_path_writer()?;
    let mut staged = StandaloneTable {
        temporary,
        guard: None,
    };
    let guard = Source::open(staged.path())?;
    let mut bytes = Vec::new();
    std::fs::File::open(staged.path())
        .and_then(|file| file.take(262145).read_to_end(&mut bytes))
        .map_err(|error| {
            files::error("GRAIN_TABLE_UNREADABLE", error.to_string(), staged.path())
        })?;
    if bytes != table.as_bytes() {
        return Err(files::error(
            "GRAIN_TABLE_WRITE_FAILED",
            "The staged grain table differs from the immutable job settings.",
            staged.path(),
        ));
    }
    guard.verify()?;
    staged.guard = Some(guard);
    Ok(Some(staged))
}

pub(in crate::jobs) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    if settings.encoder.is_svt()
        && settings.parameters.iter().any(|value| {
            value.name == "noise"
                && value
                    .value
                    .parse::<u16>()
                    .is_ok_and(|strength| strength > 0)
        })
        && (settings.film_grain > 0
            || settings
                .av1an_grain
                .as_ref()
                .is_some_and(|grain| grain.table.is_some()))
    {
        return Err(AppError::new(
            "ENCODE_SETTINGS_INVALID",
            "Use one grain source: a grain table, film-grain synthesis, or the advanced noise override.",
            None,
        ));
    }
    let Some(grain) = &settings.av1an_grain else {
        return Ok(());
    };
    let invalid = |message| AppError::new("ENCODE_SETTINGS_INVALID", message, None);
    if !(settings.encoder.is_svt() || settings.encoder == media_core::VideoEncoder::AomAv1) {
        return Err(invalid("Film-grain tables require SVT-AV1 or AOM."));
    }
    if !(1..=16).contains(&grain.denoise_strength) {
        return Err(invalid(
            "Film-grain prefilter strength must be from 1 through 16.",
        ));
    }
    if let Some(table) = &grain.table {
        if settings.film_grain != 0
            || table.len() > 262144
            || table.lines().next() != Some("filmgrn1")
        {
            return Err(invalid(
                "Use a grain table of at most 256 KiB with encoder grain strength set to zero.",
            ));
        }
        crate::utilities::validate_grain_table(table.as_bytes())
            .map_err(|message| AppError::new("ENCODE_SETTINGS_INVALID", message, None))?;
    }
    Ok(())
}

pub(super) fn stage(work: &Path, settings: &EncodeSettings) -> Result<Option<Source>, AppError> {
    let Some(table) = settings.av1an_grain.as_ref().and_then(|g| g.table.as_ref()) else {
        return Ok(None);
    };
    validate(settings)?;
    let path = work.join(TABLE_NAME);
    let exists = path
        .try_exists()
        .map_err(|e| files::error("GRAIN_TABLE_UNREADABLE", e.to_string(), &path))?;
    let file = Temporary::durable(&path, exists)?;
    let mut writer = file.clone_file()?;
    if exists {
        let mut bytes = Vec::new();
        (&mut writer)
            .take(262145)
            .read_to_end(&mut bytes)
            .map_err(|e| files::error("GRAIN_TABLE_UNREADABLE", e.to_string(), &path))?;
        if bytes != table.as_bytes() {
            return Err(files::error(
                "RECOVERY_VALIDATION_FAILED",
                "The saved grain table differs from the immutable job settings.",
                &path,
            ));
        }
    } else {
        writer
            .write_all(table.as_bytes())
            .and_then(|_| writer.sync_all())
            .map_err(|e| files::error("GRAIN_TABLE_WRITE_FAILED", e.to_string(), &path))?;
    }
    // Encoders can open tables with deny-write sharing on Windows. Release our writer
    // before retaining a read-only identity guard for all encoder children.
    drop(writer);
    drop(file);
    let guard = Source::open(&path)?;
    let mut bytes = Vec::new();
    std::fs::File::open(&guard.path)
        .and_then(|file| file.take(262145).read_to_end(&mut bytes))
        .map_err(|e| files::error("GRAIN_TABLE_UNREADABLE", e.to_string(), &path))?;
    if bytes != table.as_bytes() {
        return Err(files::error(
            "RECOVERY_VALIDATION_FAILED",
            "The staged grain table differs from the immutable job settings.",
            &path,
        ));
    }
    guard.verify()?;
    Ok(Some(guard))
}

pub(in crate::jobs) fn aom_parameters(settings: &EncodeSettings) -> Vec<OsString> {
    let mut args = Vec::new();
    if settings.encoder == media_core::VideoEncoder::AomAv1 {
        if settings
            .av1an_grain
            .as_ref()
            .is_some_and(|grain| grain.table.is_some())
        {
            args.push(format!("--film-grain-table={TABLE_NAME}").into());
            if let Some(grain) = settings.av1an_grain.as_ref().filter(|grain| grain.denoise) {
                args.push(format!("--denoise-noise-level={}", grain.denoise_strength).into());
                args.push("--enable-dnl-denoising=1".into());
            }
        } else if settings.film_grain > 0 {
            args.push(format!("--denoise-noise-level={}", settings.film_grain).into());
            args.push(
                format!(
                    "--enable-dnl-denoising={}",
                    u8::from(
                        settings
                            .av1an_grain
                            .as_ref()
                            .is_some_and(|grain| grain.denoise)
                    )
                )
                .into(),
            );
        }
    }
    args
}

pub(in crate::jobs) fn parameters(args: &mut Vec<OsString>, settings: &EncodeSettings) {
    if settings.encoder == media_core::VideoEncoder::AomAv1 {
        args.extend(aom_parameters(settings));
        return;
    }
    let Some(grain) = &settings.av1an_grain else {
        return;
    };
    let table = grain.table.is_some();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--film-grain-denoise" || (table && args[index] == "--film-grain") {
            args.drain(index..(index + 2).min(args.len()));
        } else {
            index += 1;
        }
    }
    if table {
        args.extend(["--fgs-table".into(), TABLE_NAME.into()]);
    } else {
        args.extend([
            "--film-grain-denoise".into(),
            if grain.denoise { "1" } else { "0" }.into(),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_noise_cannot_silently_displace_selected_grain() {
        let mut settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            parameters: vec![media_core::EncoderParameter {
                name: "noise".into(),
                value: "50".into(),
            }],
            film_grain: 8,
            ..Default::default()
        };
        assert!(validate(&settings).is_err());
        settings.film_grain = 0;
        assert!(validate(&settings).is_ok());
        settings.av1an_grain = Some(media_core::Av1anGrainSettings {
            table: Some("filmgrn1\nE 0 100 1 2 1\n".into()),
            denoise: false,
            denoise_strength: 4,
        });
        assert!(validate(&settings).is_err());
        settings.parameters[0].value = "0".into();
        assert!(validate(&settings).is_ok());
        settings.backend = media_core::EncodeBackend::Standalone;
        settings.parameters[0].value = "50".into();
        assert!(validate(&settings).is_err());
    }

    #[test]
    fn staged_table_is_read_only_and_changed_recovery_table_is_rejected() {
        let root = std::env::temp_dir().join(format!(
            "jesses-grain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            av1an_grain: Some(media_core::Av1anGrainSettings {
                table: Some("filmgrn1\nE 0 100 1 2 1\n".into()),
                denoise: false,
                denoise_strength: 4,
            }),
            ..Default::default()
        };
        let guard = stage(&root, &settings).unwrap().unwrap();
        let path = root.join(TABLE_NAME);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            settings
                .av1an_grain
                .as_ref()
                .unwrap()
                .table
                .as_ref()
                .unwrap()
                .as_str()
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Mirrors SVT's deny-write read: a writable staging handle would
            // make this fail even when both handles permit other readers.
            assert!(
                std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(1)
                    .open(&path)
                    .is_ok()
            );
            assert!(std::fs::write(&path, b"changed").is_err());
        }
        drop(guard);
        assert!(stage(&root, &settings).is_ok());
        std::fs::write(&path, b"changed").unwrap();
        assert_eq!(
            stage(&root, &settings).err().unwrap().code,
            "RECOVERY_VALIDATION_FAILED"
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
