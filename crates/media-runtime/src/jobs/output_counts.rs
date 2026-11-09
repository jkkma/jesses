//! The default encode's only output check: the finished file (the staging file,
//! then any final container converted from it) holds the planned number of
//! video frames and the planned streams, in order.
use super::{files, metadata::Document, process_error};
use crate::supervisor::{self, CommandSpec};
use media_core::AppError;
use std::{ffi::OsString, path::Path, time::Duration};
use tokio::sync::watch;

fn failed(message: String) -> AppError {
    AppError::new("OUTPUT_VALIDATION_FAILED", message, None)
}

/// `kinds` lists the planned streams' codec types in output order and
/// `video_position` is the encoded video's place among them. The encoders
/// write one packet (access unit, temporal unit or superframe) per frame.
/// An MP4/MOV file's trailing chapter track is not a planned stream.
pub(super) fn check(
    document: &Document,
    kinds: &[&str],
    video_position: usize,
    frames: usize,
) -> Result<(), AppError> {
    let streams = match document.streams.split_last() {
        Some((last, planned))
            if !document.chapters.is_empty() && super::container::is_chapter_track(last) =>
        {
            planned
        }
        _ => &document.streams[..],
    };
    let actual: Vec<&str> = streams
        .iter()
        .map(|stream| stream.codec_type.as_deref().unwrap_or_default())
        .collect();
    if actual != kinds {
        return Err(failed(format!(
            "The finished file's streams are {actual:?}; {kinds:?} were planned."
        )));
    }
    let counted = streams
        .get(video_position)
        .and_then(|stream| stream.nb_read_packets.as_deref())
        .and_then(|value| value.parse::<usize>().ok());
    match counted {
        Some(count) if count == frames => Ok(()),
        Some(count) => Err(failed(format!(
            "The finished file has {count} video frames; {frames} were planned."
        ))),
        None => Err(failed(format!(
            "The finished file's video frames could not be counted; {frames} were planned."
        ))),
    }
}

/// Counts the finished file's packets in one demux pass and checks them.
pub(super) async fn verify(
    ffprobe: &Path,
    output: &Path,
    kinds: &[&str],
    video_position: usize,
    frames: usize,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-count_packets",
        "-show_entries",
        "stream=index,codec_type,codec_name,codec_tag_string,nb_read_packets:stream_tags=handler_name:chapter=start_time,end_time",
        "-of",
        "json",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(output.as_os_str().to_owned());
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        1024 * 1024,
        Duration::from_secs(24 * 60 * 60),
    )
    .await
    .map_err(|cause| process_error(cause, output))?;
    if !result.status.success() {
        return Err(files::error(
            "OUTPUT_VALIDATION_FAILED",
            format!(
                "FFprobe could not count the finished file's packets: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ),
            output,
        ));
    }
    let document: Document = serde_json::from_slice(&result.stdout).map_err(|_| {
        files::error(
            "PROBE_INVALID_RESPONSE",
            "FFprobe returned unreadable packet counts.",
            output,
        )
    })?;
    check(&document, kinds, video_position, frames).map_err(|error| AppError {
        path: Some(output.to_string_lossy().into_owned()),
        ..error
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::metadata::Document;

    fn document(streams: serde_json::Value) -> Document {
        serde_json::from_value(serde_json::json!({ "streams": streams })).unwrap()
    }

    fn picture_and_sound() -> Document {
        document(serde_json::json!([
            {"index": 0, "codec_type": "video", "nb_read_packets": "48"},
            {"index": 1, "codec_type": "audio", "nb_read_packets": "70"}
        ]))
    }

    #[test]
    fn matching_counts_and_kinds_pass() {
        assert!(check(&picture_and_sound(), &["video", "audio"], 0, 48).is_ok());
    }

    #[test]
    fn a_missing_or_extra_frame_fails() {
        for frames in [47, 49] {
            let error = check(&picture_and_sound(), &["video", "audio"], 0, frames).unwrap_err();
            assert_eq!(error.code, "OUTPUT_VALIDATION_FAILED");
            assert!(
                error.message.contains(&format!("{frames} were planned")),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_missing_extra_or_reordered_stream_fails() {
        for kinds in [
            &["video"][..],
            &["video", "audio", "subtitle"][..],
            &["audio", "video"][..],
        ] {
            let error = check(&picture_and_sound(), kinds, 0, 48).unwrap_err();
            assert_eq!(error.code, "OUTPUT_VALIDATION_FAILED");
        }
    }

    #[test]
    fn an_mp4_chapter_track_is_not_a_planned_stream() {
        let mut value = serde_json::json!({
            "streams": [
                {"index": 0, "codec_type": "video", "nb_read_packets": "48"},
                {"index": 1, "codec_type": "audio", "nb_read_packets": "70"},
                {"index": 2, "codec_type": "data", "codec_name": "bin_data",
                 "codec_tag_string": "text", "tags": {"handler_name": "SubtitleHandler"}}
            ],
            "chapters": [{"start_time": "0.000000", "end_time": "1.000000"}]
        });
        let with_chapters: Document = serde_json::from_value(value.clone()).unwrap();
        assert!(check(&with_chapters, &["video", "audio"], 0, 48).is_ok());
        // Without chapters the same track is an unplanned stream.
        value["chapters"] = serde_json::json!([]);
        let without_chapters: Document = serde_json::from_value(value).unwrap();
        assert!(check(&without_chapters, &["video", "audio"], 0, 48).is_err());
    }

    #[test]
    fn an_unreadable_video_count_fails() {
        for count in [serde_json::Value::Null, serde_json::json!("N/A")] {
            let mut video = serde_json::json!({"index": 0, "codec_type": "video"});
            if !count.is_null() {
                video["nb_read_packets"] = count;
            }
            let document = document(serde_json::json!([video]));
            let error = check(&document, &["video"], 0, 48).unwrap_err();
            assert_eq!(error.code, "OUTPUT_VALIDATION_FAILED");
        }
    }
}
