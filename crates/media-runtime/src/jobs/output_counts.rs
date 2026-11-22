//! Verify the final encode's planned streams and complete decoded video before publication.
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
    pub sample_aspect_ratio: Option<(u32, u32)>,
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
        if let Some(expected) = self.sample_aspect_ratio
            && super::encode_plan::sample_aspect_ratio(video.sample_aspect_ratio.as_deref()).ok()
                != Some(expected)
        {
            return Err(failed(format!(
                "The finished video has sample aspect ratio {:?}; expected {}:{}.",
                video.sample_aspect_ratio, expected.0, expected.1,
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
/// An MP4/MOV file's chapter track is not a planned stream.
pub(super) fn check_streams<'a>(
    document: &'a Document,
    kinds: &[&str],
) -> Result<Vec<&'a Stream>, AppError> {
    let chapter_track = super::container::chapter_track_position(document);
    let streams: Vec<&Stream> = document
        .streams
        .iter()
        .enumerate()
        .filter(|(position, _)| Some(*position) != chapter_track)
        .map(|(_, stream)| stream)
        .collect();
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

fn check<'a>(
    document: &'a Document,
    kinds: &[&str],
    video_position: usize,
) -> Result<&'a Stream, AppError> {
    let streams = check_streams(document, kinds)?;
    streams
        .get(video_position)
        .copied()
        .filter(|stream| {
            stream.codec_type.as_deref() == Some("video")
                && !super::cover_art::is_cover(stream)
                && stream
                    .codec_name
                    .as_deref()
                    .is_some_and(|codec| !matches!(codec, "" | "unknown"))
                && stream.width.is_some_and(|width| width > 0)
                && stream.height.is_some_and(|height| height > 0)
        })
        .ok_or_else(|| {
            failed("The finished file has no readable header for the encoded video.".into())
        })
}

/// Return the movie track's decoded frame count after verifying the final file.
pub(super) async fn verify(
    ffprobe: &Path,
    output: &Path,
    kinds: &[&str],
    clock: super::container::Cadence<'_>,
    cancel: &watch::Receiver<bool>,
) -> Result<usize, AppError> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-show_entries",
        "stream=index,codec_type,codec_name,codec_tag_string,width,height,sample_aspect_ratio,time_base,start_time,duration:stream_disposition=attached_pic:stream_tags=handler_name,DURATION:chapter=start_time,end_time",
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
        Duration::from_secs(5 * 60),
    )
    .await
    .map_err(|cause| process_error(cause, output))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(files::error(
            "OUTPUT_VALIDATION_FAILED",
            format!(
                "FFprobe could not read the finished file's headers: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ),
            output,
        ));
    }
    let document: Document = serde_json::from_slice(&result.stdout).map_err(|_| {
        files::error(
            "PROBE_INVALID_RESPONSE",
            "FFprobe returned unreadable output headers.",
            output,
        )
    })?;
    let video = check(&document, kinds, clock.index as usize)
        .and_then(|video| {
            if let Some(check) = clock.basic_video_check {
                check.verify(video, clock.frames)?;
            }
            Ok(video)
        })
        .map_err(|error| AppError {
            path: Some(output.to_string_lossy().into_owned()),
            ..error
        })?;
    if !clock.mov_text.is_empty() {
        let selected = check_streams(&document, kinds)?;
        super::trim_subtitle_verify::verify_output(
            ffprobe,
            output,
            video,
            &selected,
            clock.mov_text,
            cancel,
        )
        .await?;
    }
    let decoded = count_video_frames(ffprobe, output, video.index, cancel).await?;
    if decoded != clock.frames {
        return Err(files::error(
            "OUTPUT_VALIDATION_FAILED",
            format!(
                "The finished video decodes to {decoded} frames; encoding prepared {}.",
                clock.frames
            ),
            output,
        ));
    }
    Ok(decoded)
}

async fn count_video_frames(
    ffprobe: &Path,
    output: &Path,
    stream_index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<usize, AppError> {
    // A container's declared count and packet count do not establish the
    // number of displayable frames. Decode only the encoded movie track and
    // keep subprocess output independent of the number of frames.
    let index = stream_index.to_string();
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-threads",
        "0",
        "-err_detect",
        "explode",
        "-select_streams",
        &index,
        "-count_frames",
        "-show_entries",
        "stream=nb_read_frames",
        "-of",
        "default=nokey=1:noprint_wrappers=1",
        "-i",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    args.push(output.as_os_str().to_owned());
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(24 * 60 * 60),
    )
    .await
    .map_err(|cause| process_error(cause, output))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(files::error(
            "OUTPUT_VALIDATION_FAILED",
            format!(
                "FFprobe could not decode the complete finished video: {}",
                String::from_utf8_lossy(&result.stderr).trim(),
            ),
            output,
        ));
    }
    String::from_utf8_lossy(&result.stdout)
        .trim()
        .parse()
        .map_err(|_| {
            files::error(
                "OUTPUT_VALIDATION_FAILED",
                "FFprobe did not report a decoded frame count for the finished video.",
                output,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::metadata::Document;

    #[tokio::test]
    #[ignore = "requires a qualified FFmpeg/FFprobe pair with libx264"]
    async fn verifies_decoded_video_and_planned_streams_before_publication() {
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
        let valid = fixture.0.join("valid.mov");
        let no_audio = fixture.0.join("no-audio.mov");
        let corrupt = fixture.0.join("corrupt.mov");
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
        std::fs::write(&corrupt, b"nonempty unreadable output").unwrap();
        let expectation = Some(VideoCheck {
            codec: "h264",
            dimensions: (64, 64),
            sample_aspect_ratio: Some((1, 1)),
            frame_seconds: Some(0.1),
        });
        let cadence = |frames| super::super::container::Cadence {
            index: 0,
            numerator: 10,
            denominator: 1,
            frames,
            source_iso: None,
            basic_video_check: expectation,
            mp4_aspect_override: None,
            mov_text: &[],
            expected_kinds: &["video", "audio"],
        };
        let decoded = verify(&ffprobe, &valid, &["video", "audio"], cadence(12), &cancel)
            .await
            .unwrap();
        assert_eq!(decoded, 12);
        let mismatch = verify(&ffprobe, &valid, &["video", "audio"], cadence(13), &cancel)
            .await
            .unwrap_err();
        assert_eq!(mismatch.code, "OUTPUT_VALIDATION_FAILED");
        assert!(
            mismatch.message.contains("decodes to 12 frames"),
            "{mismatch:?}"
        );
        for output in [no_audio, corrupt] {
            let failure = verify(&ffprobe, &output, &["video", "audio"], cadence(12), &cancel)
                .await
                .unwrap_err();
            assert_eq!(failure.code, "OUTPUT_VALIDATION_FAILED", "{failure:?}");
        }

        let matroska = fixture.0.join("complete.mkv");
        let mut args = ["-v", "error", "-i"].map(OsString::from).to_vec();
        args.push(valid.as_os_str().to_owned());
        args.extend(["-map", "0", "-c", "copy"].map(OsString::from));
        args.push(matroska.as_os_str().to_owned());
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg,
                args,
                cwd: None,
            },
            cancel.clone(),
            64 * 1024,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(result.status.success(), "{:?}", result.stderr);

        let manager = super::super::JobManager::new(fixture.0.join("logs"));
        let source = files::Source::open(&valid).unwrap();
        let contents = std::fs::read(matroska).unwrap();
        for (label, bytes, succeeds) in [
            ("complete", contents.as_slice(), true),
            ("truncated", &contents[..contents.len() / 2], false),
        ] {
            use std::io::Write;

            let destination = fixture.0.join(format!("published-{label}.mkv"));
            let stage = files::Temporary::create(&destination, label).unwrap();
            stage.clone_file().unwrap().write_all(bytes).unwrap();
            let result = manager
                .finalize(
                    label,
                    &cancel,
                    &source,
                    None,
                    &stage,
                    &destination,
                    Some(super::super::container::Cadence {
                        index: 0,
                        numerator: 10,
                        denominator: 1,
                        frames: 12,
                        source_iso: None,
                        basic_video_check: expectation,
                        mp4_aspect_override: None,
                        mov_text: &[],
                        expected_kinds: &["video", "audio"],
                    }),
                    &mut Vec::new(),
                )
                .await;
            assert_eq!(result.is_ok(), succeeds, "{label}: {result:?}");
            assert_eq!(destination.exists(), succeeds, "{label}");
            if succeeds {
                assert_eq!(std::fs::read(destination).unwrap(), bytes);
            } else {
                assert_eq!(result.unwrap_err().code, "OUTPUT_VALIDATION_FAILED");
            }
        }
        manager.shutdown().await;
    }

    fn document(streams: serde_json::Value) -> Document {
        serde_json::from_value(serde_json::json!({ "streams": streams })).unwrap()
    }

    fn picture_and_sound() -> Document {
        document(serde_json::json!([
            {"index": 0, "codec_type": "video", "codec_name": "h264", "width": 64, "height": 64},
            {"index": 1, "codec_type": "audio"}
        ]))
    }

    #[test]
    fn matching_headers_pass_without_a_frame_count() {
        let document = picture_and_sound();
        let video = check(&document, &["video", "audio"], 0).unwrap();
        assert!(video.nb_frames.is_none());
    }

    #[test]
    fn a_missing_extra_or_reordered_stream_fails() {
        for kinds in [
            &["video"][..],
            &["video", "audio", "subtitle"][..],
            &["audio", "video"][..],
        ] {
            let error = check(&picture_and_sound(), kinds, 0).unwrap_err();
            assert_eq!(error.code, "OUTPUT_VALIDATION_FAILED");
        }
    }

    #[test]
    fn an_mp4_chapter_track_is_not_a_planned_stream() {
        let mut value = serde_json::json!({
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "h264", "width": 64, "height": 64},
                {"index": 1, "codec_type": "audio"},
                {"index": 2, "codec_type": "data", "codec_name": "bin_data",
                 "codec_tag_string": "text", "tags": {"handler_name": "SubtitleHandler"}}
            ],
            "chapters": [{"start_time": "0.000000", "end_time": "1.000000"}]
        });
        let with_chapters: Document = serde_json::from_value(value.clone()).unwrap();
        assert!(check(&with_chapters, &["video", "audio"], 0).is_ok());
        // Without chapters the same track is an unplanned stream.
        value["chapters"] = serde_json::json!([]);
        let without_chapters: Document = serde_json::from_value(value).unwrap();
        assert!(check(&without_chapters, &["video", "audio"], 0).is_err());
    }

    #[test]
    fn an_mp4_cover_follows_the_chapter_track() {
        let value = serde_json::json!({
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "h264", "width": 64, "height": 64},
                {"index": 1, "codec_type": "audio"},
                {"index": 2, "codec_type": "data", "codec_name": "bin_data",
                 "codec_tag_string": "text", "tags": {"handler_name": "SubtitleHandler"}},
                {"index": 3, "codec_type": "video", "codec_name": "mjpeg",
                 "disposition": {"attached_pic": 1}}
            ],
            "chapters": [{"start_time": "0.000000", "end_time": "1.000000"}]
        });
        let document: Document = serde_json::from_value(value).unwrap();
        assert!(check(&document, &["video", "audio", "video"], 0).is_ok());
        assert!(check(&document, &["video", "audio"], 0).is_err());
    }

    #[test]
    fn an_unreadable_video_header_fails() {
        for stream in [
            serde_json::json!({"index": 0, "codec_type": "video", "width": 64, "height": 64}),
            serde_json::json!({"index": 0, "codec_type": "video", "codec_name": "h264", "width": 0, "height": 64}),
        ] {
            let document = document(serde_json::json!([stream]));
            let error = check(&document, &["video"], 0).unwrap_err();
            assert_eq!(error.code, "OUTPUT_VALIDATION_FAILED");
        }
    }
}
