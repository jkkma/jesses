//! One source, one spatial denoise pass, and one reusable measured grain table.
use super::*;

pub(super) async fn measure_source(
    source_path: String,
    table_path: String,
    strength: u8,
    keep_path: Option<String>,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    if !(1..=16).contains(&strength) {
        return Err(error(
            "UTILITY_GRAIN_STRENGTH_INVALID",
            "Use a denoise strength from 1 through 16; 4 matches the fork's spatial hqdn3d default.",
            Some(Path::new(&source_path)),
        ));
    }
    let source = Source::open(valid_absolute(&source_path)?)?;
    let table_output = destination(&table_path, &[&source])?;
    let kept_output = keep_path
        .as_deref()
        .map(|path| destination(path, &[&source]))
        .transpose()?;
    if let Some(keep) = &kept_output {
        require_mkv(keep, "A retained denoised reference")?;
        if keep == &table_output {
            return Err(error(
                "UTILITY_GRAIN_OUTPUT_COLLISION",
                "The table and denoised video require different new destinations.",
                Some(keep),
            ));
        }
    }
    let ffmpeg = tool("ffmpeg", "Spatial grain-reference denoising").await?;
    let ffprobe = tool("ffprobe", "Denoised reference validation").await?;
    let scratch = Scratch::create("grain-measure-source")?;
    let denoised = scratch.path.join("denoised.mkv");
    let private_table = scratch.path.join("measured.tbl");
    let document = probe(&ffprobe, &source.path, cancel, false).await?;
    let video = document
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| {
            error(
                "UTILITY_STREAM_INVALID",
                "The source has no video stream.",
                Some(&source.path),
            )
        })?;
    let format = video.pix_fmt.as_deref().ok_or_else(|| {
        error(
            "UTILITY_GRAIN_SOURCE_UNSUPPORTED",
            "The source has no reported pixel format.",
            Some(&source.path),
        )
    })?;
    let filter = format!("hqdn3d={}:{}:0:0", strength, f64::from(strength) * 0.75);
    checked(
        ffmpeg.clone(),
        vec![
            "-hide_banner".into(),
            "-v".into(),
            "warning".into(),
            "-nostdin".into(),
            "-i".into(),
            os(&source.path),
            "-map".into(),
            "0:v:0".into(),
            "-an".into(),
            "-sn".into(),
            "-dn".into(),
            "-vf".into(),
            filter.clone().into(),
            "-c:v".into(),
            "ffv1".into(),
            "-level".into(),
            "3".into(),
            "-pix_fmt".into(),
            format!("+{format}").into(),
            "-fps_mode".into(),
            "passthrough".into(),
            "-f".into(),
            "matroska".into(),
            "-n".into(),
            os(&denoised),
        ],
        cancel,
        MEDIA_LIMIT,
        Some(&source.path),
        "Spatial denoised grain reference",
    )
    .await?;
    validate_full_decode(&ffmpeg, &denoised, cancel).await?;
    // Reuse the existing complete geometry/format/frame-count admission and table parser.
    let measured = Box::pin(super::run_grain(
        GrainRequest::Measure {
            source_path: source.path.to_string_lossy().into_owned(),
            denoised_path: denoised.to_string_lossy().into_owned(),
            output_table_path: private_table.to_string_lossy().into_owned(),
        },
        cancel,
    ))
    .await?;
    let UtilityResult::GrainTable(mut result) = measured else {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "Grain measurement returned an unexpected result.",
            None,
        ));
    };
    source.verify()?;
    check_cancel(cancel)?;
    let mut table = Temporary::create(&table_output, &nonce("grain-source-table"))?;
    fs::copy(&private_table, &table.path)
        .map_err(|cause| error("OUTPUT_WRITE_FAILED", cause.to_string(), Some(&table.path)))?;
    read_grain_table(&table.path)?;
    let mut kept = if let Some(output) = &kept_output {
        let copy = Temporary::create(output, &nonce("grain-source-reference"))?;
        // The private FFV1 was completely validated before a user-visible copy is prepared.
        let mut input = tokio::fs::File::open(&denoised)
            .await
            .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(&denoised)))?;
        let mut output_file = tokio::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&copy.path)
            .await
            .map_err(|cause| error("OUTPUT_WRITE_FAILED", cause.to_string(), Some(&copy.path)))?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            check_cancel(cancel)?;
            let count = input
                .read(&mut buffer)
                .await
                .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(&denoised)))?;
            if count == 0 {
                break;
            }
            output_file
                .write_all(&buffer[..count])
                .await
                .map_err(|cause| {
                    error("OUTPUT_WRITE_FAILED", cause.to_string(), Some(&copy.path))
                })?;
        }
        output_file
            .flush()
            .await
            .map_err(|cause| error("OUTPUT_WRITE_FAILED", cause.to_string(), Some(&copy.path)))?;
        drop(output_file);
        copy.flush_nonempty_async().await?;
        Some(copy)
    } else {
        None
    };
    table.flush_nonempty_async().await?;
    let table_size = table
        .clone_file()?
        .metadata()
        .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(&table.path)))?
        .len();
    source.verify()?;
    check_cancel(cancel)?;
    if let (Some(copy), Some(output)) = (kept.as_mut(), kept_output.as_ref()) {
        copy.publish(output)?;
        copy.cleanup()?;
    }
    table.publish(&table_output).map_err(|mut failure| {
        if let Some(path) = &kept_output {
            failure.message.push_str(&format!(
                " The verified denoised copy was already retained at {}.",
                path.display()
            ));
        }
        failure
    })?;
    table.cleanup()?;
    result.output_path = table_output.to_string_lossy().into_owned();
    result.size_bytes = Some(table_size.to_string());
    result.denoised_output_path = kept_output.map(|path| path.to_string_lossy().into_owned());
    result.message = format!(
        "Measured {} grain-table segments after one spatial denoise pass. The table can be reused by later encodes.",
        result.segment_count
    );
    result.diagnostics.push(format!("Reference filter: {filter}. Temporal denoising is disabled so motion is not measured as grain."));
    result.diagnostics.push(
        if result.denoised_output_path.is_some() {
            "The verified lossless denoised video was retained at the selected new destination."
        } else {
            "The private lossless denoised video was removed after measurement."
        }
        .into(),
    );
    Ok(UtilityResult::GrainTable(result))
}
