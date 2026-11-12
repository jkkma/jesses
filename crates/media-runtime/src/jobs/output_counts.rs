//! Check the default encode's finished file for the planned streams and video
//! packet count, plus the applicable AV1AN video header fields.
use super::{
    files,
    metadata::{Document, Stream, track_duration},
    process_error,
};
use crate::supervisor::{self, CommandSpec};
use media_core::AppError;
use std::{ffi::OsString, path::Path, time::Duration};
use tokio::sync::watch;

fn failed(message: String) -> AppError {
    AppError::new("OUTPUT_VALIDATION_FAILED", message, None)
}

#[derive(Clone, Copy)]
pub(super) struct VideoCheck {
    pub codec: &'static str,
    pub dimensions: (u32, u32),
    pub frame_seconds: Option<f64>,
}

impl VideoCheck {
    fn verify(self, video: &Stream, frames: usize) -> Result<(), AppError> {
        if video.codec_name.as_deref() != Some(self.codec)
            || video.width.zip(video.height) != Some(self.dimensions)
        {
            return Err(failed(format!(
                "The finished video has codec {:?} and dimensions {:?}; expected {} at {:?}.",
                video.codec_name,
                video.width.zip(video.height),
                self.codec,
                self.dimensions,
            )));
        }
        // Matroska's track duration can include its start offset. Compare a
        // declared track duration only when the video starts at zero, and
        // allow the container's end-of-last-frame rounding.
        if let Some(frame_seconds) = self.frame_seconds
            && video
                .start_time
                .as_deref()
                .and_then(|value| value.parse::<f64>().ok())
                .is_some_and(|start| start.abs() <= 0.001)
            && let Some(duration) = track_duration(video)
            && (duration - frames as f64 * frame_seconds).abs() > frame_seconds + 0.001
        {
            return Err(failed(format!(
                "The finished video declares a {duration:.3}s duration; {frames} frames at the joined rate require {:.3}s.",
                frames as f64 * frame_seconds,
            )));
        }
        Ok(())
    }
}

/// `kinds` lists the planned streams' codec types in output order.
/// An MP4/MOV file's trailing chapter track is not a planned stream.
pub(super) fn check_streams<'a>(
    document: &'a Document,
    kinds: &[&str],
) -> Result<&'a [Stream], AppError> {
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
    Ok(streams)
}

fn check(
    document: &Document,
    kinds: &[&str],
    video_position: usize,
    frames: usize,
) -> Result<(), AppError> {
    // The encoders write one packet (access unit, temporal unit or superframe)
    // per frame. `video_position` is the encoded video's place among streams.
    let streams = check_streams(document, kinds)?;
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
    video_check: Option<VideoCheck>,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-count_packets",
        "-show_entries",
        "stream=index,codec_type,codec_name,codec_tag_string,width,height,start_time,duration,nb_read_packets:stream_tags=handler_name,DURATION:chapter=start_time,end_time",
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
    if !result.status.success() || (video_check.is_some() && !result.stderr.is_empty()) {
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
    check(&document, kinds, video_position, frames)
        .and_then(|()| match video_check {
            Some(check) => check.verify(&document.streams[video_position], frames),
            None => Ok(()),
        })
        .map_err(|error| AppError {
            path: Some(output.to_string_lossy().into_owned()),
            ..error
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::metadata::Document;

    #[tokio::test]
    #[ignore = "requires a qualified FFmpeg/FFprobe pair with libx264"]
    async fn basic_probe_rejects_a_missing_track_and_a_physically_truncated_container() {
        let (_sender, cancel) = watch::channel(false);
        let ffmpeg = super::super::discover("ffmpeg", &cancel).await.unwrap();
        let ffprobe = super::super::discover("ffprobe", &cancel).await.unwrap();
        let root = std::env::temp_dir().join(format!(
            "jesses-basic-output-counts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir(&root).unwrap();
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let fixture = Fixture(root);
        let valid = fixture.0.join("valid.mkv");
        let no_audio = fixture.0.join("no-audio.mkv");
        let truncated = fixture.0.join("truncated.mkv");
        for (output, include_audio) in [(&valid, true), (&no_audio, false)] {
            let mut args: Vec<OsString> = [
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=10:duration=1.2",
            ]
            .into_iter()
            .map(Into::into)
            .collect();
            if include_audio {
                args.extend(
                    [
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=duration=1.2",
                        "-c:a",
                        "pcm_s16le",
                    ]
                    .map(OsString::from),
                );
            }
            args.extend(["-c:v", "libx264", "-preset", "ultrafast"].map(OsString::from));
            args.push(output.as_os_str().to_owned());
            let result = supervisor::run_capture(
                &CommandSpec {
                    executable: ffmpeg.clone(),
                    args,
                    cwd: None,
                },
                cancel.clone(),
                64 * 1024,
                Duration::from_secs(20),
            )
            .await
            .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let bytes = std::fs::read(&valid).unwrap();
        std::fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
        let expectation = Some(VideoCheck {
            codec: "h264",
            dimensions: (64, 64),
            frame_seconds: Some(0.1),
        });
        verify(
            &ffprobe,
            &valid,
            &["video", "audio"],
            0,
            12,
            expectation,
            &cancel,
        )
        .await
        .unwrap();
        for output in [no_audio, truncated] {
            let failure = verify(
                &ffprobe,
                &output,
                &["video", "audio"],
                0,
                12,
                expectation,
                &cancel,
            )
            .await
            .unwrap_err();
            assert_eq!(failure.code, "OUTPUT_VALIDATION_FAILED", "{failure:?}");
        }
    }

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
