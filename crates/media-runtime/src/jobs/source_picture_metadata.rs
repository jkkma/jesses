//! Bounded actual decoded color/HDR declarations for sparse source headers.
use super::*;

/// Header-only FFprobe can omit decoded HEVC color and static HDR SEI. This
/// belongs to actual operation preparation; file-list loading stays cheap.
pub(super) fn needs_enrichment(video: &super::metadata::Stream) -> bool {
    let missing = |value: &Option<String>| {
        value
            .as_deref()
            .is_none_or(|value| matches!(value, "unknown" | "unspecified"))
    };
    [
        &video.color_primaries,
        &video.color_transfer,
        &video.color_space,
        &video.color_range,
        &video.chroma_location,
    ]
    .into_iter()
    .any(missing)
        || (matches!(
            video.color_transfer.as_deref(),
            Some("smpte2084" | "arib-std-b67")
        ) && !video.side_data_list.iter().any(|value| {
            value
                .get("side_data_type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| kind.contains("Mastering display"))
        }))
}

pub(super) async fn enrich(
    ffprobe: &Path,
    input: &Path,
    document: &mut Document,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    #[derive(serde::Deserialize)]
    struct Frame {
        color_primaries: Option<String>,
        color_transfer: Option<String>,
        color_space: Option<String>,
        color_range: Option<String>,
        chroma_location: Option<String>,
        #[serde(default)]
        side_data_list: Vec<serde_json::Value>,
    }
    let mut video = document
        .streams
        .iter()
        .find(|s| s.index == index)
        .cloned()
        .ok_or_else(|| {
            files::error(
                "STREAM_SELECTION_INVALID",
                "The chosen source picture is absent.",
                input,
            )
        })?;
    let args=["-v","error","-err_detect","explode","-protocol_whitelist","file","-select_streams"].into_iter().map(OsString::from).chain([index.to_string().into()]).chain(["-show_frames","-show_entries","frame=color_primaries,color_transfer,color_space,color_range,chroma_location:frame_side_data","-of","json","-i"].into_iter().map(OsString::from)).chain([input.as_os_str().to_owned()]).collect();
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(86400),
        move |reader| {
            let mut static_hdr =
                super::encode_plan::hdr::StaticMetadata::parse(&video.side_data_list)
                    .map_err(|e| e.message)?;
            let mut failure = None;
            let mut count = 0_u64;
            super::encode::frame_scan::parse::<Frame>(reader, |frame| {
                if failure.is_some() {
                    return;
                }
                let checked = (|| -> Result<(), String> {
                    for (header, actual) in [
                        (&mut video.color_primaries, &frame.color_primaries),
                        (&mut video.color_transfer, &frame.color_transfer),
                        (&mut video.color_space, &frame.color_space),
                        (&mut video.color_range, &frame.color_range),
                        (&mut video.chroma_location, &frame.chroma_location),
                    ] {
                        let known = |value: &Option<String>| {
                            value
                                .as_deref()
                                .is_some_and(|value| !matches!(value, "unknown" | "unspecified"))
                        };
                        if known(header) && known(actual) && header != actual {
                            return Err(
                                "Stream and decoded frame color declarations disagree.".into()
                            );
                        }
                        if !known(header) && known(actual) {
                            *header = actual.clone();
                        }
                    }
                    let actual =
                        super::encode_plan::hdr::StaticMetadata::parse(&frame.side_data_list)
                            .map_err(|e| e.message)?;
                    static_hdr
                        .absorb_first_frame(&actual)
                        .map_err(|e| e.message)?;
                    for value in frame.side_data_list {
                        let kind = value
                            .get("side_data_type")
                            .and_then(serde_json::Value::as_str);
                        if kind.is_some_and(|kind| {
                            kind.contains("Mastering display") || kind.contains("Content light")
                        }) && !video.side_data_list.iter().any(|v| {
                            v.get("side_data_type").and_then(serde_json::Value::as_str) == kind
                        }) {
                            video.side_data_list.push(value);
                        }
                    }
                    count += 1;
                    Ok(())
                })();
                if let Err(cause) = checked {
                    failure = Some(cause);
                }
            })?;
            if let Some(failure) = failure {
                return Err(failure);
            }
            if count == 0 {
                return Err("The source did not decode a picture for color planning.".into());
            }
            Ok(video)
        },
    )
    .await
    .map_err(|e| process_error(e, input))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(files::error(
            "ENCODE_INPUT_UNSUPPORTED",
            "The source picture declarations could not be decoded without errors.",
            input,
        ));
    }
    *document
        .streams
        .iter_mut()
        .find(|s| s.index == index)
        .expect("selected source metadata") = result.value;
    Ok(())
}
