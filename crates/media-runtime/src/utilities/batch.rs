//! Explicit per-file utility batches share cancellation, preserve successes and report every row.
use super::*;
use media_core::{UtilityBatchEntry, UtilityBatchRequest, UtilityBatchResult};

pub async fn run_utility_batch(
    request: UtilityBatchRequest,
    cancel: watch::Receiver<bool>,
) -> Result<UtilityBatchResult, AppError> {
    if request.requests.is_empty() || request.requests.len() > 100 {
        return Err(error(
            "UTILITY_BATCH_INVALID",
            "Choose between 1 and 100 utility requests.",
            None,
        ));
    }
    let mut destinations = BTreeSet::new();
    for request in &request.requests {
        for text in output_paths(request) {
            let path = valid_absolute(text)?;
            let parent = path
                .parent()
                .and_then(|parent| fs::canonicalize(parent).ok())
                .ok_or_else(|| {
                    error(
                        "INVALID_OUTPUT",
                        "Choose an existing output folder for every batch row.",
                        Some(path),
                    )
                })?;
            let resolved = parent.join(path.file_name().ok_or_else(|| {
                error("INVALID_OUTPUT", "Choose an output filename.", Some(path))
            })?);
            let key = if cfg!(windows) {
                resolved.to_string_lossy().to_lowercase()
            } else {
                resolved.to_string_lossy().into_owned()
            };
            if !destinations.insert(key) {
                return Err(error(
                    "UTILITY_BATCH_COLLISION",
                    "Two batch rows name the same output. Review the per-file output names.",
                    Some(path),
                ));
            }
            ensure_absent(&resolved)?;
        }
    }
    let mut result = UtilityBatchResult {
        entries: Vec::with_capacity(request.requests.len()),
        completed: 0,
        canceled: false,
    };
    let row_count = request.requests.len();
    for (index, request) in request.requests.into_iter().enumerate() {
        super::progress::row(index, row_count);
        if *cancel.borrow() {
            super::progress::terminal("skipped", "The batch was stopped before this row ran.");
            result.canceled = true;
            result.entries.push(UtilityBatchEntry {
                index: index as u32,
                request,
                result: None,
                error: Some(error(
                    "UTILITY_BATCH_SKIPPED",
                    "The batch was stopped before this row ran.",
                    None,
                )),
            });
            continue;
        }
        let outcome = super::run_utility(request.clone(), cancel.clone()).await;
        let (value, failure) = match outcome {
            Ok(value) => {
                result.completed += 1;
                (Some(value), None)
            }
            Err(failure) => (None, Some(failure)),
        };
        result.entries.push(UtilityBatchEntry {
            index: index as u32,
            request,
            result: value,
            error: failure,
        });
        result.canceled |= *cancel.borrow();
    }
    Ok(result)
}
fn output_paths(request: &UtilityRequest) -> Vec<&str> {
    match request {
        UtilityRequest::KeyframeCut(r) => vec![&r.output_path],
        UtilityRequest::Concat(r) => vec![&r.output_path],
        UtilityRequest::ColorMetadataTransfer(r) => {
            if r.in_place {
                vec![]
            } else {
                vec![&r.output_path]
            }
        }
        UtilityRequest::SubtitleOcr(r) => vec![&r.output_path],
        UtilityRequest::DeinterlaceExport(r) => vec![&r.output_path],
        UtilityRequest::CadenceRepairExport(r) => vec![&r.output_path],
        UtilityRequest::CrfLadder(r) => r.keep_samples_directory.as_deref().into_iter().collect(),
        UtilityRequest::Grain(r) => match r {
            GrainRequest::MeasureSource {
                output_table_path,
                denoised_output_path,
                ..
            } => std::iter::once(output_table_path.as_str())
                .chain(denoised_output_path.as_deref())
                .collect(),
            GrainRequest::Measure {
                output_table_path, ..
            }
            | GrainRequest::Extract {
                output_table_path, ..
            } => vec![output_table_path],
            GrainRequest::Apply { output_path, .. }
            | GrainRequest::RewriteHeaders { output_path, .. }
            | GrainRequest::Remove { output_path, .. } => vec![output_path],
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn cut(input: &Path, output: &Path) -> UtilityRequest {
        UtilityRequest::KeyframeCut(KeyframeCutRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            start_seconds: 0.0,
            end_seconds: 1.0,
        })
    }
    #[tokio::test]
    async fn batch_rejects_output_collisions_before_running_or_writing() {
        let root = Scratch::create("batch-collision-test").unwrap();
        let output = root.path.join("out.mkv");
        let input = root.path.join("missing.mkv");
        let (_, cancel) = watch::channel(false);
        let result = run_utility_batch(
            UtilityBatchRequest {
                requests: vec![cut(&input, &output), cut(&input, &output)],
            },
            cancel,
        )
        .await
        .unwrap_err();
        assert_eq!(result.code, "UTILITY_BATCH_COLLISION");
        assert!(!output.exists());
    }
    #[tokio::test]
    async fn precanceled_batch_marks_every_row_skipped() {
        let root = Scratch::create("batch-cancel-test").unwrap();
        let (_, cancel) = watch::channel(true);
        let result = run_utility_batch(
            UtilityBatchRequest {
                requests: vec![
                    cut(&root.path.join("missing.mkv"), &root.path.join("one.mkv")),
                    cut(&root.path.join("missing.mkv"), &root.path.join("two.mkv")),
                ],
            },
            cancel,
        )
        .await
        .unwrap();
        assert!(result.canceled);
        assert_eq!(result.completed, 0);
        assert_eq!(result.entries.len(), 2);
        assert!(result.entries.iter().all(|entry| {
            entry
                .error
                .as_ref()
                .is_some_and(|error| error.code == "UTILITY_BATCH_SKIPPED")
        }));
    }
}
