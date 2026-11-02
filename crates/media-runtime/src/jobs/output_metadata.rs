//! Source Quick's global Output File row is applied after the selected donor.
use super::*;

pub(super) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    if (settings.copy_metadata == Some(false) && settings.metadata_source_path.is_some())
        || (settings.copy_chapters == Some(false) && settings.chapters_source_path.is_some())
    {
        return Err(AppError::new(
            "OUTPUT_METADATA_INVALID",
            "A None metadata or chapter donor cannot also name an input file.",
            None,
        ));
    }
    if let Some(values) = &settings.output_metadata {
        for value in [&values.title, &values.language].into_iter().flatten() {
            if value.contains('\0') || value.len() > 4096 {
                return Err(AppError::new(
                    "OUTPUT_METADATA_INVALID",
                    "Output metadata values must be at most 4096 bytes and contain no null characters.",
                    None,
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn append(args: &mut Vec<OsString>, settings: &EncodeSettings) {
    if let Some(values) = &settings.output_metadata {
        for (key, value) in [("title", &values.title), ("language", &values.language)] {
            if let Some(value) = value {
                args.extend(["-metadata".into(), format!("{key}={value}").into()]);
            }
        }
    }
}

pub(super) fn apply(document: &mut Document, settings: &EncodeSettings) {
    if settings.copy_metadata == Some(false)
        && let Some(format) = &mut document.format
    {
        format.tags.clear();
    }
    if settings.copy_chapters == Some(false) {
        document.chapters.clear();
    }
    if let Some(values) = &settings.output_metadata {
        let format = document.format.get_or_insert_with(|| metadata::Format {
            format_name: None,
            duration: None,
            start_time: None,
            tags: Default::default(),
        });
        for (key, value) in [("title", &values.title), ("language", &values.language)] {
            if let Some(value) = value {
                format
                    .tags
                    .retain(|name, _| !name.eq_ignore_ascii_case(key));
                if !value.is_empty() {
                    format.tags.insert(key.into(), value.clone());
                }
            }
        }
    }
}

pub(super) fn strip_stream(stream: &mut metadata::Stream, settings: &EncodeSettings) {
    // The fork restores attachment tags after -map_metadata -1. Ordinary
    // stream tags stay stripped unless its Apply grid supplies overrides.
    if settings.copy_metadata == Some(false) && stream.codec_type.as_deref() != Some("attachment") {
        stream.tags.clear();
    }
}

pub(super) fn verify(settings: &EncodeSettings, output: &Document) -> Result<(), AppError> {
    if let Some(values) = &settings.output_metadata {
        for (key, expected) in [("title", &values.title), ("language", &values.language)] {
            if let Some(expected) = expected {
                let actual = output
                    .format
                    .as_ref()
                    .and_then(|format| {
                        format
                            .tags
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(key))
                    })
                    .map(|(_, value)| value.as_str())
                    .filter(|value| !value.is_empty());
                if actual != (!expected.is_empty()).then_some(expected.as_str()) {
                    return Err(AppError::new(
                        "OUTPUT_VALIDATION_FAILED",
                        "An explicit output title or language change was not retained.",
                        None,
                    ));
                }
            }
        }
    }
    if settings.copy_chapters == Some(false) && !output.chapters.is_empty() {
        return Err(AppError::new(
            "OUTPUT_VALIDATION_FAILED",
            "The output retained chapters although the source selection was None.",
            None,
        ));
    }
    Ok(())
}
