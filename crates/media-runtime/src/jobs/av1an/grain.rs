use super::*;
use std::io::{Read, Write};
pub(in crate::jobs) const TABLE_NAME: &str = "jesses-grain.tbl";

pub(in crate::jobs) struct TableCoverage(Option<u64>);

impl TableCoverage {
    pub(in crate::jobs) fn from_settings(settings: &EncodeSettings) -> Result<Self, AppError> {
        inline_table(settings).map_or(Ok(Self(None)), |table| Self::parse(settings, table))
    }

    fn parse(settings: &EncodeSettings, table: &str) -> Result<Self, AppError> {
        if settings.encoder == media_core::VideoEncoder::AomAv1 && table.starts_with("filmgrn1\r\n")
        {
            return Err(invalid(
                "AOM requires LF line endings in grain tables. Re-select the file to reload it, or use an LF copy.",
            ));
        }
        let timing = crate::utilities::grain_table_timing(table.as_bytes())
            .map_err(|message| AppError::new("ENCODE_SETTINGS_INVALID", message, None))?;
        let svt = settings.encoder.is_svt();
        let parallel_chunks = settings.backend == media_core::EncodeBackend::Av1an
            && !settings
                .av1an_options
                .unwrap_or_default()
                .guarantees_single_chunk();
        if !svt && !parallel_chunks {
            return Ok(Self(None));
        }
        // SVT applies its first model throughout, even on a full source. AOM
        // honors timing, but each independently encoded chunk restarts at zero.
        if timing.segments != 1 || timing.start != 0 {
            return Err(invalid(if svt {
                "SVT-AV1 uses one film-grain model for the whole video. Use a grain table with one segment starting at zero and covering the whole video."
            } else {
                "Parallel AV1AN chunks restart the AOM grain table. Use a table with one segment starting at zero and covering the whole video, or use standalone AOM for timed grain."
            }));
        }
        Ok(Self(Some(timing.end)))
    }

    pub(in crate::jobs) fn verify(&self, frames: usize, rate: (u32, u32)) -> Result<(), AppError> {
        let Some(end) = self.0 else { return Ok(()) };
        let video_end = video_end(frames, rate);
        if u128::from(end) < video_end {
            return Err(AppError::new(
                "ENCODE_SETTINGS_INVALID",
                format!(
                    "The grain table ends at {:.3} s but the video needs {:.3} s. Use a grain table with one segment covering the whole video.",
                    end as f64 / 10_000_000.0,
                    video_end as f64 / 10_000_000.0
                ),
                None,
            ));
        }
        Ok(())
    }

    /// Default jobs plan from headers, so the frame count is provisional until
    /// the encode has run. Stop a table that is short by a clear margin before
    /// that encode is spent; `verify` still decides once the count is exact.
    pub(in crate::jobs) fn verify_estimate(
        &self,
        frames: usize,
        rate: (u32, u32),
    ) -> Result<(), AppError> {
        let Some(end) = self.0 else { return Ok(()) };
        let video_end = video_end(frames, rate);
        // Header counts are accepted within the same 2 % elsewhere; never
        // refuse a table over a difference smaller than that or one second.
        let margin = (video_end / 50).max(10_000_000);
        if u128::from(end) + margin < video_end {
            return Err(AppError::new(
                "ENCODE_SETTINGS_INVALID",
                format!(
                    "The grain table ends at {:.3} s but the video needs about {:.3} s. Use a grain table with one segment covering the whole video. The video length was read from the file headers; if it is wrong, turn on Thorough verification to count the frames first.",
                    end as f64 / 10_000_000.0,
                    video_end as f64 / 10_000_000.0
                ),
                None,
            ));
        }
        Ok(())
    }
}

// Grain-table ticks are 100 ns. Match that precision without rounding a
// rational video duration up to a tick the format cannot represent.
fn video_end(frames: usize, rate: (u32, u32)) -> u128 {
    frames as u128 * 10_000_000 * u128::from(rate.1) / u128::from(rate.0)
}

fn inline_table(settings: &EncodeSettings) -> Option<&str> {
    settings
        .av1an_grain
        .as_ref()
        .and_then(|grain| grain.table.as_deref())
}

fn invalid(message: &str) -> AppError {
    AppError::new("ENCODE_SETTINGS_INVALID", message, None)
}

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
        TableCoverage::parse(settings, table)?;
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
    if exists {
        let file = Temporary::durable(&path, true)?;
        let mut bytes = Vec::new();
        file.clone_file()?
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
        // Recovery compares this name with the saved table before any Resume
        // or Discard, so a failed or interrupted write must never leave a
        // partial table under it.
        recovery::write_complete(&path, table.as_bytes(), &|path, cause| {
            files::error("GRAIN_TABLE_WRITE_FAILED", cause, path)
        })?;
    }
    // Encoders can open tables with deny-write sharing on Windows. No writer
    // remains open here; retain a read-only identity guard for all encoder children.
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
        args.retain(|argument| {
            let text = argument.to_string_lossy();
            !text.starts_with("--film-grain-table=")
                && !text.starts_with("--denoise-noise-level=")
                && !text.starts_with("--enable-dnl-denoising=")
        });
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
    fn timed_grain_tables_require_a_whole_input_aom_encode() {
        const TIMED: [&str; 2] = [
            "filmgrn1\nE 0 20000000 1 2 1\nE\t20000000 40000000 1 3 1\n",
            "filmgrn1\nE 10000000 20000000 1 2 1\n",
        ];
        let single_chunk = media_core::Av1anOptions {
            split_method: media_core::Av1anSplitMethod::FixedChunks,
            maximum_chunk_frames: 0,
            ..Default::default()
        };
        for encoder in [
            media_core::VideoEncoder::SvtAv1,
            media_core::VideoEncoder::SvtAv1FiveFish,
            media_core::VideoEncoder::SvtAv1Hdr,
            media_core::VideoEncoder::AomAv1,
        ] {
            for (backend, av1an_options, whole_input) in [
                (media_core::EncodeBackend::Standalone, None, true),
                (media_core::EncodeBackend::Av1an, Some(single_chunk), true),
                (media_core::EncodeBackend::Av1an, None, false),
                (
                    media_core::EncodeBackend::Av1an,
                    Some(media_core::Av1anOptions {
                        source_workflow: true,
                        ..single_chunk
                    }),
                    false,
                ),
            ] {
                let mut settings = EncodeSettings {
                    backend,
                    encoder,
                    av1an_options,
                    ..Default::default()
                };
                for table in TIMED {
                    settings.av1an_grain = Some(media_core::Av1anGrainSettings {
                        table: Some(table.into()),
                        denoise: false,
                        denoise_strength: 4,
                    });
                    let result = validate(&settings);
                    if !encoder.is_svt() && whole_input {
                        assert!(result.is_ok(), "{encoder:?} {backend:?}: {result:?}");
                    } else {
                        assert_eq!(result.unwrap_err().code, "ENCODE_SETTINGS_INVALID");
                    }
                }
                settings.av1an_grain.as_mut().unwrap().table =
                    Some("filmgrn1\nE 0 40000000 1 2 1\n".into());
                assert!(validate(&settings).is_ok());
            }
        }
    }

    #[test]
    fn rejects_aom_inline_tables_with_crlf_magic() {
        let settings = EncodeSettings {
            encoder: media_core::VideoEncoder::AomAv1,
            av1an_grain: Some(media_core::Av1anGrainSettings {
                table: Some("filmgrn1\r\nE 0 40000000 1 2 1\r\n".into()),
                denoise: false,
                denoise_strength: 4,
            }),
            ..Default::default()
        };
        let error = validate(&settings).unwrap_err();
        assert!(error.message.contains("LF") && error.message.contains("Re-select"));
    }

    #[test]
    fn constant_grain_must_cover_the_reconciled_video_clock() {
        let settings = EncodeSettings {
            encoder: media_core::VideoEncoder::SvtAv1Hdr,
            av1an_grain: Some(media_core::Av1anGrainSettings {
                table: Some("filmgrn1\nE 0 40040000 1 2 1\n".into()),
                denoise: false,
                denoise_strength: 4,
            }),
            ..Default::default()
        };
        let coverage = TableCoverage::from_settings(&settings).unwrap();
        coverage.verify(95, (24_000, 1001)).unwrap();
        coverage.verify(96, (24_000, 1001)).unwrap();
        let error = coverage.verify(97, (24_000, 1001)).unwrap_err();
        assert_eq!(error.code, "ENCODE_SETTINGS_INVALID");
        assert!(error.message.contains("the video needs"));
        TableCoverage(Some(416_666)).verify(1, (24, 1)).unwrap();
        assert!(TableCoverage(Some(416_665)).verify(1, (24, 1)).is_err());
    }

    #[test]
    fn a_header_estimate_only_refuses_a_clearly_short_grain_table() {
        // 2,400 frames at 24 fps need 100 s; the margin is 2 %.
        const NEEDED: u64 = 1_000_000_000;
        let rate = (24, 1);
        TableCoverage(Some(NEEDED))
            .verify_estimate(2400, rate)
            .unwrap();
        TableCoverage(Some(NEEDED - 20_000_000))
            .verify_estimate(2400, rate)
            .unwrap();
        let error = TableCoverage(Some(NEEDED - 20_000_001))
            .verify_estimate(2400, rate)
            .unwrap_err();
        assert_eq!(error.code, "ENCODE_SETTINGS_INVALID");
        assert!(error.message.contains("ends at 98.000 s"));
        assert!(error.message.contains("the video needs about 100.000 s"));
        assert!(error.message.contains("Thorough verification"));
        // The exact check still refuses what the estimate lets through.
        assert!(
            TableCoverage(Some(NEEDED - 20_000_000))
                .verify(2400, rate)
                .is_err()
        );
        // Short clips keep a margin of one second.
        TableCoverage(Some(1)).verify_estimate(24, rate).unwrap();
        TableCoverage(Some(10_000_000))
            .verify_estimate(48, rate)
            .unwrap();
        assert!(
            TableCoverage(Some(10_000_000))
                .verify_estimate(49, rate)
                .is_err()
        );
        // Unbounded tables and the usual maximum stamps pass both checks.
        TableCoverage(None).verify_estimate(2400, rate).unwrap();
        for end in [i64::MAX as u64, u64::MAX] {
            TableCoverage(Some(end))
                .verify_estimate(u32::MAX as usize, rate)
                .unwrap();
            TableCoverage(Some(end))
                .verify(u32::MAX as usize, rate)
                .unwrap();
        }
    }

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
        let selected = root.join("selected.tbl");
        let source_bytes = b"filmgrn1\r\nE 0 100 1 2 1\r\n";
        std::fs::write(&selected, source_bytes).unwrap();
        let table =
            crate::utilities::read_av1an_grain_table(selected.to_string_lossy().into_owned())
                .unwrap();
        assert_eq!(table, "filmgrn1\nE 0 100 1 2 1\n");
        assert_eq!(std::fs::read(&selected).unwrap(), source_bytes);
        let settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            av1an_grain: Some(media_core::Av1anGrainSettings {
                table: Some(table),
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
        // A different line ending is enough to break a saved staging binding;
        // only fresh reads normalize, and a resume never rewrites saved bytes.
        std::fs::write(&path, source_bytes).unwrap();
        assert_eq!(
            stage(&root, &settings).err().unwrap().code,
            "RECOVERY_VALIDATION_FAILED"
        );
        assert_eq!(std::fs::read(&path).unwrap(), source_bytes);
        std::fs::remove_file(selected).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn aom_grain_parameters_are_not_duplicated_when_reapplied() {
        let settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            encoder: media_core::VideoEncoder::AomAv1,
            film_grain: 8,
            ..Default::default()
        };
        let mut args = vec![
            OsString::from("--cq-level=35"),
            OsString::from("--denoise-noise-level=8"),
            OsString::from("--enable-dnl-denoising=0"),
        ];
        parameters(&mut args, &settings);
        assert_eq!(
            args.iter()
                .filter(|arg| *arg == "--denoise-noise-level=8")
                .count(),
            1
        );
        assert_eq!(
            args.iter()
                .filter(|arg| *arg == "--enable-dnl-denoising=0")
                .count(),
            1
        );
    }
}
