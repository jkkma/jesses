//! Source-style automatic names, held for the whole admitted operation. Final
//! publication still uses the filesystem's atomic no-replace operation, so an
//! external writer racing a suggested name can never be overwritten.
use super::*;
use std::sync::Mutex;

static RESERVED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

pub struct UtilityOutputReservations(Vec<PathBuf>);
impl Drop for UtilityOutputReservations {
    fn drop(&mut self) {
        if let Ok(mut names) = RESERVED.lock() {
            for name in &self.0 {
                names.remove(name);
            }
        }
    }
}

fn preferred(
    input: &str,
    directory: &str,
    suffix: &str,
    extension: &str,
) -> Result<PathBuf, AppError> {
    let source = Source::open(valid_absolute(input)?)?;
    let parent = if directory.is_empty() {
        source.path.parent().expect("source parent").to_owned()
    } else {
        fs::canonicalize(valid_absolute(directory)?).map_err(|cause| {
            error(
                "INVALID_OUTPUT",
                cause.to_string(),
                Some(Path::new(directory)),
            )
        })?
    };
    if !parent.is_dir() {
        return Err(error(
            "INVALID_OUTPUT",
            "The output folder is unavailable.",
            Some(&parent),
        ));
    }
    let stem = source
        .path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    Ok(parent.join(format!(
        "{}{suffix}{extension}",
        stem.chars().take(180).collect::<String>()
    )))
}

fn allocate(
    slot: &mut String,
    path: PathBuf,
    lease: &mut UtilityOutputReservations,
) -> Result<(), AppError> {
    if !slot.is_empty() {
        return Ok(());
    }
    let mut names = RESERVED.lock().map_err(|_| {
        error(
            "UTILITY_OUTPUT_FAILED",
            "Automatic output naming is unavailable.",
            None,
        )
    })?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    for index in 0..=9999 {
        let candidate = if index == 0 {
            path.clone()
        } else {
            path.with_file_name(format!("{stem} ({index}){extension}"))
        };
        match fs::symlink_metadata(&candidate) {
            Ok(_) => continue,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => {
                return Err(error(
                    "UTILITY_OUTPUT_FAILED",
                    cause.to_string(),
                    Some(&candidate),
                ));
            }
        }
        if names.insert(candidate.clone()) {
            *slot = candidate.to_string_lossy().into_owned();
            lease.0.push(candidate);
            return Ok(());
        }
    }
    Err(error(
        "UTILITY_OUTPUT_FAILED",
        "No free automatic output name was found.",
        Some(&path),
    ))
}

/// Empty output strings request source-style automatic naming; nonempty paths
/// retain their existing explicit no-clobber semantics. Hold the returned guard
/// until execution (including every batch row) has completed or been canceled.
pub fn prepare_utility_outputs(
    requests: &mut [UtilityRequest],
    directory: &str,
) -> Result<UtilityOutputReservations, AppError> {
    let mut lease = UtilityOutputReservations(Vec::new());
    for request in requests {
        match request {
            UtilityRequest::KeyframeCut(r) => {
                allocate(
                    &mut r.output_path,
                    preferred(
                        &r.input_path,
                        directory,
                        "_cut",
                        &super::video_container::source_extension(&r.input_path)?,
                    )?,
                    &mut lease,
                )?;
            }
            UtilityRequest::Concat(r) => {
                let first = r.input_paths.first().ok_or_else(|| {
                    error(
                        "UTILITY_INVALID",
                        "Choose input files to concatenate.",
                        None,
                    )
                })?;
                let source = Source::open(valid_absolute(first)?)?;
                let parent = source.path.parent().expect("source parent");
                let name = parent.file_name().unwrap_or_default().to_string_lossy();
                allocate(
                    &mut r.output_path,
                    parent.join(format!("{name}-merge.mkv")),
                    &mut lease,
                )?;
            }
            UtilityRequest::ColorMetadataTransfer(r) if !r.in_place => allocate(
                &mut r.output_path,
                preferred(&r.input_path, directory, "_color", ".mkv")?,
                &mut lease,
            )?,
            UtilityRequest::ColorMetadataTransfer(_) => {}
            UtilityRequest::SubtitleOcr(r) => allocate(
                &mut r.output_path,
                preferred(
                    &r.input_path,
                    directory,
                    &format!("_subtitle{}", r.subtitle_stream_index),
                    ".srt",
                )?,
                &mut lease,
            )?,
            UtilityRequest::DeinterlaceExport(r) => allocate(
                &mut r.output_path,
                preferred(&r.input_path, directory, "_deinterlaced", ".mkv")?,
                &mut lease,
            )?,
            UtilityRequest::CadenceRepairExport(r) => allocate(
                &mut r.output_path,
                preferred(&r.input_path, directory, "_cfr", ".mkv")?,
                &mut lease,
            )?,
            UtilityRequest::CrfLadder(r) => {
                if let Some(slot) = &mut r.keep_samples_directory {
                    allocate(
                        slot,
                        preferred(&r.input_path, directory, "_crfladder", "")?,
                        &mut lease,
                    )?;
                }
            }
            UtilityRequest::Grain(r) => match r {
                GrainRequest::MeasureSource {
                    source_path,
                    output_table_path,
                    denoised_output_path,
                    ..
                } => {
                    allocate(
                        output_table_path,
                        preferred(source_path, directory, ".grain", ".tbl")?,
                        &mut lease,
                    )?;
                    if let Some(slot) = denoised_output_path {
                        allocate(
                            slot,
                            preferred(source_path, directory, "_denoised", ".mkv")?,
                            &mut lease,
                        )?;
                    }
                }
                GrainRequest::Measure {
                    source_path,
                    output_table_path,
                    ..
                } => allocate(
                    output_table_path,
                    preferred(source_path, directory, ".grain", ".tbl")?,
                    &mut lease,
                )?,
                GrainRequest::Extract {
                    input_path,
                    output_table_path,
                } => allocate(
                    output_table_path,
                    preferred(input_path, directory, ".grain", ".tbl")?,
                    &mut lease,
                )?,
                GrainRequest::Apply {
                    input_path,
                    output_path,
                    ..
                }
                | GrainRequest::RewriteHeaders {
                    input_path,
                    output_path,
                    ..
                } => allocate(
                    output_path,
                    preferred(
                        input_path,
                        directory,
                        "_grain",
                        &super::video_container::source_extension(input_path)?,
                    )?,
                    &mut lease,
                )?,
                GrainRequest::Remove {
                    input_path,
                    output_path,
                } => allocate(
                    output_path,
                    preferred(
                        input_path,
                        directory,
                        "_nograin",
                        &super::video_container::source_extension(input_path)?,
                    )?,
                    &mut lease,
                )?,
            },
        }
    }
    Ok(lease)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_names_skip_files_directories_and_live_requests_without_creating_outputs() {
        let folder = std::env::temp_dir().join(nonce("automatic-outputs"));
        fs::create_dir(&folder).unwrap();
        let input = folder.join("source.mkv");
        fs::write(&input, b"source").unwrap();
        fs::write(folder.join("source_cut.mkv"), b"existing").unwrap();
        fs::create_dir(folder.join("source_cut (1).mkv")).unwrap();
        let make = || {
            UtilityRequest::KeyframeCut(KeyframeCutRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: String::new(),
                start_seconds: 0.0,
                end_seconds: 1.0,
            })
        };
        let mut a = [make()];
        let lease = prepare_utility_outputs(&mut a, "").unwrap();
        let mut b = [make()];
        let other = prepare_utility_outputs(&mut b, "").unwrap();
        let UtilityRequest::KeyframeCut(a) = &a[0] else {
            unreachable!()
        };
        let UtilityRequest::KeyframeCut(b) = &b[0] else {
            unreachable!()
        };
        assert!(a.output_path.ends_with("source_cut (2).mkv"));
        assert!(b.output_path.ends_with("source_cut (3).mkv"));
        assert!(!Path::new(&a.output_path).exists());
        assert_eq!(fs::read(&input).unwrap(), b"source");
        drop((lease, other));
        let mut c = [make()];
        let _lease = prepare_utility_outputs(&mut c, "").unwrap();
        assert_eq!(&c[0], &UtilityRequest::KeyframeCut(a.clone()));
        fs::remove_dir_all(folder).unwrap();
    }
}
