//! Nmkoder BatchNaming.cs and UiData.cs filename semantics, shared by ordinary
//! encodes and fixed-format image exports. Nothing is created by this module.
use super::*;
use media_core::{
    BatchNamingValues, BatchOutputPathInput, BatchOutputPathItem, BatchOutputPathRequest,
    BatchOutputPaths, OutputPathRequest, OutputPathResolution, OutputTarget,
};

fn replace_case_insensitive(text: &str, token: &str, value: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut result = String::with_capacity(text.len());
    let mut previous = 0;
    for (index, _) in lower.match_indices(token) {
        result.push_str(&text[previous..index]);
        result.push_str(value);
        previous = index + token.len();
    }
    result.push_str(&text[previous..]);
    result
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if matches!(
                character,
                '\0' | '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            ) || (cfg!(windows) && character < ' ')
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .trim_end_matches(['.', ' '])
        .to_owned()
}

#[allow(clippy::too_many_arguments)]
fn render(
    template: Option<&str>,
    source_name: &str,
    extension: &str,
    input: &BatchOutputPathInput,
    naming: &BatchNamingValues,
    date: Option<&str>,
    index: usize,
    total: usize,
) -> Result<String, AppError> {
    let template = template
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("{name}");
    let date = if template.to_ascii_lowercase().contains("{date}") {
        date.filter(|value| valid_naming_date(value))
            .ok_or_else(|| {
                error(
                    "OUTPUT_TEMPLATE_VALUE_UNAVAILABLE",
                    "{date} requires a valid local date snapshot in YYYY-MM-DD format.",
                    Path::new(&input.input_path),
                )
            })?
    } else {
        date.unwrap_or_default()
    };
    // Keep the source's replacement order. A source name containing a later
    // token is expanded again, whereas an unknown token stays literal.
    let values = [
        ("{name}", source_name.to_owned()),
        ("{ext}", extension.to_owned()),
        (
            "{index}",
            format!("{:0width$}", index + 1, width = total.to_string().len()),
        ),
        ("{codec}", naming.codec.clone()),
        ("{crf}", naming.quality.clone()),
        ("{quality}", naming.quality.clone()),
        ("{preset}", naming.preset.clone()),
        (
            "{width}",
            input
                .width
                .map_or_else(String::new, |value| value.to_string()),
        ),
        (
            "{height}",
            input
                .height
                .map_or_else(String::new, |value| value.to_string()),
        ),
        ("{date}", date.to_owned()),
    ];
    let mut result = template.to_owned();
    for (token, value) in values {
        result = replace_case_insensitive(&result, token, &value);
    }
    let sanitized = sanitize(&result);
    Ok(if sanitized.is_empty() {
        source_name.to_owned()
    } else {
        sanitized
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_one(
    input: &BatchOutputPathInput,
    target: OutputTarget,
    naming: &BatchNamingValues,
    template: Option<&str>,
    date: Option<&str>,
    configured: Option<&str>,
    index: usize,
    total: usize,
    reserved: &mut HashSet<String>,
) -> Result<OutputPathResolution, AppError> {
    let source = crate::jobs::files::Source::open(Path::new(&input.input_path))?;
    let original = crate::jobs::sequence::original_directory(&source.path)?;
    let identity = original.as_deref().unwrap_or(&source.path);
    let name = if original.is_some() {
        identity.file_name()
    } else {
        identity.file_stem()
    }
    .and_then(|name| name.to_str())
    .ok_or_else(|| {
        error(
            "OUTPUT_PATH_INVALID",
            "The source has no Unicode filename.",
            identity,
        )
    })?;
    let extension = if original.is_some() {
        ""
    } else {
        identity
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
    };
    let stem = render(template, name, extension, input, naming, date, index, total)?;
    let directory = crate::output_resolution::preferred_directory(identity, configured)?;
    let resolution = crate::output_resolution::resolve_reserved(
        OutputPathRequest {
            input_path: input.input_path.clone(),
            output_stem: Some(directory.join(stem).to_string_lossy().into_owned()),
            default_output_directory: None,
            target,
        },
        reserved,
    )?;
    source.verify()?;
    Ok(resolution)
}

pub async fn resolve_batch_output_paths(
    request: BatchOutputPathRequest,
) -> Result<BatchOutputPaths, AppError> {
    resolve_inner(request, None).await
}

pub async fn resolve_batch_output_paths_cancellable(
    request: BatchOutputPathRequest,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<BatchOutputPaths, AppError> {
    resolve_inner(request, Some(cancel)).await
}

async fn resolve_inner(
    request: BatchOutputPathRequest,
    cancel: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<BatchOutputPaths, AppError> {
    validate_source_batch_len(request.inputs.len())?;
    tokio::task::spawn_blocking(move || {
        check_source_preview_cancel(cancel.as_ref())?;
        let mut reserved = HashSet::new();
        let total = request.inputs.len();
        let mut items = Vec::with_capacity(total);
        for (index, input) in request.inputs.iter().enumerate() {
            check_source_preview_cancel(cancel.as_ref())?;
            let result = resolve_one(
                input,
                request.target,
                &request.naming,
                request.output_name_template.as_deref(),
                request.naming_date.as_deref(),
                request.default_output_directory.as_deref(),
                index,
                total,
                &mut reserved,
            );
            items.push(match result {
                Ok(resolution) => BatchOutputPathItem {
                    input_path: input.input_path.clone(),
                    resolution: Some(resolution),
                    error: None,
                },
                Err(error) => BatchOutputPathItem {
                    input_path: input.input_path.clone(),
                    resolution: None,
                    error: Some(error),
                },
            });
        }
        check_source_preview_cancel(cancel.as_ref())?;
        Ok(BatchOutputPaths { items })
    })
    .await
    .map_err(|cause| AppError::new("OUTPUT_PATH_INVALID", cause.to_string(), None))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_tokens_are_case_insensitive_literal_unknown_and_quality_aliases() {
        let input = BatchOutputPathInput {
            input_path: "/media/Episode.One.MKV".into(),
            width: Some(1920),
            height: Some(1080),
        };
        let naming = BatchNamingValues {
            codec: "av1".into(),
            quality: "3.5".into(),
            preset: "medium".into(),
        };
        let rendered = render(Some("{NaMe}_{EXT}_{index}_{CoDeC}_{crf}_{quality}_{preset}_{width}x{height}_{DATE}_{unknown}"), "Episode.One", "MKV", &input, &naming, Some("2026-11-01"), 8, 100).unwrap();
        assert_eq!(
            rendered,
            "Episode.One_MKV_009_av1_3.5_3.5_medium_1920x1080_2026-11-01_{unknown}"
        );
        assert_eq!(
            render(
                Some("  "),
                "Episode.One",
                "MKV",
                &input,
                &naming,
                None,
                0,
                1
            )
            .unwrap(),
            "Episode.One"
        );
        assert_eq!(
            render(
                Some(" ../a\\b:*?\"<>|. "),
                "Episode.One",
                "MKV",
                &input,
                &naming,
                None,
                0,
                1
            )
            .unwrap(),
            ".._a_b_______"
        );
        assert_eq!(
            render(
                Some(" ... "),
                "Episode.One",
                "MKV",
                &input,
                &naming,
                None,
                0,
                1
            )
            .unwrap(),
            "Episode.One"
        );
        assert_eq!(
            render(
                Some("{unclosed"),
                "Episode.One",
                "MKV",
                &input,
                &naming,
                None,
                0,
                1
            )
            .unwrap(),
            "{unclosed"
        );
        assert_eq!(
            render(
                Some("{name}"),
                "literal.{ext}",
                "MKV",
                &input,
                &naming,
                None,
                0,
                1
            )
            .unwrap(),
            "literal.MKV"
        );
        let audio = BatchOutputPathInput {
            width: None,
            height: None,
            ..input
        };
        assert_eq!(
            render(
                Some("{width}{height}"),
                "sound",
                "wav",
                &audio,
                &naming,
                None,
                0,
                1
            )
            .unwrap(),
            "sound"
        );
    }
}
