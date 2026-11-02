use super::*;
use media_core::{
    DeinterlaceExportMethod, DeinterlaceExportRequest, DeinterlaceResolution, DetectedFieldOrder,
    FieldOrder, QtgmcSettings,
};

fn resolve_order(order: DetectedFieldOrder, automatic: bool) -> Option<FieldOrder> {
    match order {
        DetectedFieldOrder::TopFirst => Some(FieldOrder::TopFirst),
        DetectedFieldOrder::BottomFirst => Some(FieldOrder::BottomFirst),
        DetectedFieldOrder::Progressive | DetectedFieldOrder::Undetermined => {
            if automatic {
                None
            } else {
                Some(FieldOrder::TopFirst)
            }
        }
    }
}

pub async fn resolve_deinterlace(
    mut request: DeinterlaceExportRequest,
    automatic: bool,
    cancel: watch::Receiver<bool>,
) -> Result<DeinterlaceResolution, AppError> {
    let source = Source::open(valid_absolute(&request.input_path)?)?;
    let verdict = crate::detect_fields(
        media_core::FieldDetectionRequest {
            input_path: request.input_path.clone(),
            video_stream_index: request.video_stream_index,
        },
        cancel.clone(),
    )
    .await?;
    let Some(field) = resolve_order(verdict.order, automatic) else {
        source.verify()?;
        return Ok(DeinterlaceResolution {
            request: None,
            message: format!(
                "Automatic deinterlacing left this source unchanged. {}",
                verdict.evidence
            ),
        });
    };
    request.field_order = field;
    let mut message = verdict.evidence;
    if request.method == DeinterlaceExportMethod::Qtgmc {
        let ffprobe = tool("ffprobe", "Deinterlace source inspection").await?;
        let media = probe(&ffprobe, &source.path, &cancel, false).await?;
        let picture = media
            .streams
            .iter()
            .find(|stream| {
                stream.index == request.video_stream_index
                    && stream.codec_type.as_deref() == Some("video")
            })
            .ok_or_else(|| {
                error(
                    "UTILITY_SOURCE_INVALID",
                    "The selected source video is unavailable.",
                    Some(&source.path),
                )
            })?;
        let format = picture.pix_fmt.as_deref().unwrap_or("");
        let problem = if ["rgb", "bgr", "gbr"]
            .iter()
            .any(|prefix| format.starts_with(prefix))
        {
            Some(format!("QTGMC cannot read RGB input ({format})."))
        } else {
            match crate::jobs::qtgmc::qualify(
                QtgmcSettings {
                    mode: request.mode,
                    field_order: field,
                    preset: request.qtgmc_preset,
                },
                &cancel,
            )
            .await
            {
                Ok(()) => None,
                Err(cause) => {
                    check_cancel(&cancel)?;
                    Some(cause.message)
                }
            }
        };
        if let Some(problem) = problem {
            request.method = DeinterlaceExportMethod::Bwdif;
            message = format!("Deinterlacing with BWDIF rather than QTGMC: {problem} {message}");
        }
    }
    source.verify()?;
    check_cancel(&cancel)?;
    Ok(DeinterlaceResolution {
        request: Some(request),
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_automatic_skips_unestablished_fields_and_explicit_modes_default_top_first() {
        assert_eq!(
            resolve_order(DetectedFieldOrder::TopFirst, true),
            Some(FieldOrder::TopFirst)
        );
        assert_eq!(
            resolve_order(DetectedFieldOrder::BottomFirst, true),
            Some(FieldOrder::BottomFirst)
        );
        for order in [
            DetectedFieldOrder::Progressive,
            DetectedFieldOrder::Undetermined,
        ] {
            assert_eq!(resolve_order(order, true), None);
            assert_eq!(resolve_order(order, false), Some(FieldOrder::TopFirst));
        }
    }
}
