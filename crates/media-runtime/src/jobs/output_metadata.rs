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
    // Derived tags stay in this expected description: verification never
    // requires them of the output, and a Matroska track's DURATION tag is the
    // only length it reports. Without it the duration check would fall back
    // to the whole source file, whose end may be a deselected track's.
    if settings.copy_metadata == Some(false) && stream.codec_type.as_deref() != Some("attachment") {
        stream
            .tags
            .retain(|key, _| metadata::is_derived_stream_tag(key));
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A Matroska track states its length only in its DURATION tag. A
    /// deselected subtitle that ends later sets the whole file's duration.
    fn matroska_source() -> Document {
        serde_json::from_value(json!({"streams":[
            {"index":0,"codec_type":"video","codec_name":"av1","width":64,"height":64,"nb_read_packets":"316","tags":{"title":"Picture","DURATION":"00:00:13.179000000"}},
            {"index":1,"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000","nb_read_packets":"617","tags":{"language":"jpn","DURATION":"00:00:13.177000000"}},
            {"index":2,"codec_type":"subtitle","codec_name":"subrip","nb_read_packets":"4","tags":{"DURATION":"00:00:14.471000000"}}
        ],"format":{"duration":"14.471000","tags":{"title":"Episode"}}}))
        .unwrap()
    }

    #[test]
    fn dropping_metadata_keeps_each_selected_track_length() {
        let settings = EncodeSettings {
            copy_metadata: Some(false),
            ..EncodeSettings::default()
        };
        let mut source = matroska_source();
        for stream in &mut source.streams {
            strip_stream(stream, &settings);
        }
        apply(&mut source, &settings);
        // Descriptive tags are gone and are not required of the output.
        assert!(source.streams.iter().all(|stream| {
            !stream
                .tags
                .keys()
                .any(|key| matches!(key.as_str(), "title" | "language"))
        }));
        let selected = source.selected(&[0, 1]).unwrap();
        assert_eq!(source.selected_duration(&selected), Some(13.179));
        assert_eq!(source.required_duration(&selected, &[]), Some(13.179));
        let mut output = source.clone();
        output.streams.truncate(2);
        output.format.as_mut().unwrap().duration = Some("13.179000".into());
        metadata::verify(&source, &selected, &output).unwrap();
        // A truncated output still fails.
        output.format.as_mut().unwrap().duration = Some("12.500000".into());
        assert!(metadata::verify(&source, &selected, &output).is_err());
    }
}
