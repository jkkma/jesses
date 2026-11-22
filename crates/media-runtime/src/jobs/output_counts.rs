//! Check the final encode's planned streams and available frame-count evidence.
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
        if let Some(frame_seconds) = self.frame_seconds {
            verify_track_duration(video, frames, frame_seconds)?;
        }
        Ok(())
    }
}

fn verify_track_duration(
    video: &Stream,
    frames: usize,
    frame_seconds: f64,
) -> Result<(), AppError> {
    // A Matroska track duration can include the track's start offset. Use the
    // selected zero-origin track only, with room for end-of-frame rounding.
    let expected = frames as f64 * frame_seconds;
    if video
        .start_time
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .is_some_and(|start| start.abs() <= 0.001)
        && let Some(duration) = track_duration(video).filter(|duration| *duration > 0.0)
        && (duration - expected).abs() > frame_seconds + 0.001
    {
        return Err(failed(format!(
            "The finished video declares a {duration:.3}s duration. The planned duration for {frames} frames is {expected:.3}s."
        )));
    }
    Ok(())
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

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Count {
    Declared(usize),
    MuxedPackets(usize),
    ReadPackets(usize),
}

impl Count {
    pub(super) fn summary(&self, expected: usize) -> String {
        match *self {
            Self::Declared(frames) => format!("{frames} video frames declared by the container"),
            Self::MuxedPackets(packets) if packets == expected => {
                format!("{packets} video packets reported by the muxer")
            }
            Self::ReadPackets(packets) if packets == expected => {
                format!("{packets} video packets read from the finished file")
            }
            Self::MuxedPackets(packets) => format!(
                "{packets} video packets reported by the muxer for {expected} planned frames (frame count unconfirmed)"
            ),
            Self::ReadPackets(packets) => format!(
                "{packets} video packets read for {expected} planned video frames (frame count unconfirmed)"
            ),
        }
    }

    fn verify(
        &self,
        video: &Stream,
        expected: usize,
        frame_seconds: Option<f64>,
    ) -> Result<(), AppError> {
        let message = match *self {
            Self::Declared(frames) if frames != expected => {
                format!(
                    "The finished video declares {frames} frames. Encoding prepared {expected}."
                )
            }
            Self::MuxedPackets(0) | Self::ReadPackets(0) => {
                "The finished video has no readable packets.".into()
            }
            Self::MuxedPackets(packets) | Self::ReadPackets(packets)
                if packets != expected
                    && matches!(video.codec_name.as_deref(), Some("h264" | "hevc")) =>
            {
                format!(
                    "The finished video has {packets} video packets. Encoding prepared {expected} frames."
                )
            }
            // AV1 and VP9 can package hidden frames differently from displayed
            // frames. Corroborate a packet discrepancy with a track duration
            // only where the encode established a constant frame clock.
            Self::MuxedPackets(packets) | Self::ReadPackets(packets) if packets != expected => {
                return frame_seconds.map_or(Ok(()), |seconds| {
                    verify_track_duration(video, expected, seconds)
                });
            }
            _ => return Ok(()),
        };
        Err(failed(message))
    }
}

fn declared_frames(video: &Stream) -> Option<usize> {
    let count = |value: &str| value.parse().ok().filter(|&count| count > 0);
    video.nb_frames.as_deref().and_then(count).or_else(|| {
        video
            .tags
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("NUMBER_OF_FRAMES"))
            .and_then(|(_, value)| count(value))
    })
}

// One final report is enough. A long reporting interval keeps capture bounded.
pub(super) const MUX_PROGRESS_ARGS: [&str; 4] = ["-stats_period", "86400", "-progress", "pipe:1"];

/// Reads FFmpeg's final progress block. The frame field counts the first video
/// output stream, so callers must bind it to the selected movie before use.
#[derive(Default)]
pub(super) struct MuxProgress {
    frames: Option<usize>,
    completed: Option<usize>,
}

impl MuxProgress {
    pub(super) fn observe(&mut self, line: &str) {
        if let Some(frames) = line.strip_prefix("frame=") {
            self.frames = frames.trim().parse().ok();
        } else if let Some(progress) = line.strip_prefix("progress=") {
            if progress == "end" {
                self.completed = self.frames;
            }
            self.frames = None;
        }
    }

    pub(super) fn completed(&self) -> Option<usize> {
        self.completed
    }
}

pub(super) fn movie_is_first_video(kinds: &[&str], movie_position: usize) -> bool {
    kinds.iter().position(|kind| *kind == "video") == Some(movie_position)
}

/// Return the strongest available final video count, without decoding the movie.
pub(super) async fn verify(
    ffprobe: &Path,
    output: &Path,
    kinds: &[&str],
    clock: super::container::Cadence<'_>,
    mux_packets: Option<usize>,
    cancel: &watch::Receiver<bool>,
) -> Result<Count, AppError> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-show_entries",
        "stream=index,codec_type,codec_name,codec_tag_string,width,height,sample_aspect_ratio,time_base,start_time,duration,nb_frames:stream_disposition=attached_pic:stream_tags=handler_name,DURATION,NUMBER_OF_FRAMES:chapter=start_time,end_time",
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
    let count = if let Some(frames) = declared_frames(video) {
        Count::Declared(frames)
    } else if let Some(packets) = mux_packets {
        Count::MuxedPackets(packets)
    } else {
        Count::ReadPackets(count_video_packets(ffprobe, output, video.index, cancel).await?)
    };
    count
        .verify(video, clock.frames, clock.count_duration_frame_seconds)
        .map_err(|error| AppError {
            path: Some(output.to_string_lossy().into_owned()),
            ..error
        })?;
    Ok(count)
}

async fn count_video_packets(
    ffprobe: &Path,
    output: &Path,
    stream_index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<usize, AppError> {
    // Resume and containers without written count evidence can still report
    // the selected track's packet count with bounded subprocess output.
    let index = stream_index.to_string();
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-select_streams",
        &index,
        "-count_packets",
        "-show_entries",
        "stream=nb_read_packets",
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
                "FFprobe could not read the finished video's packets: {}",
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
                "FFprobe did not report a packet count for the finished video.",
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
    async fn verifies_video_count_and_planned_streams_before_publication() {
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
            mux_video_packets: None,
            count_duration_frame_seconds: None,
            source_iso: None,
            basic_video_check: expectation,
            mp4_aspect_override: None,
            mov_text: &[],
            expected_kinds: &["video", "audio"],
        };
        let count = verify(
            &ffprobe,
            &valid,
            &["video", "audio"],
            cadence(12),
            Some(1),
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(count, Count::Declared(12));
        let mismatch = verify(
            &ffprobe,
            &valid,
            &["video", "audio"],
            cadence(13),
            None,
            &cancel,
        )
        .await
        .unwrap_err();
        assert_eq!(mismatch.code, "OUTPUT_VALIDATION_FAILED");
        assert!(
            mismatch.message.contains("declares 12 frames"),
            "{mismatch:?}"
        );
        for output in [no_audio, corrupt] {
            let failure = verify(
                &ffprobe,
                &output,
                &["video", "audio"],
                cadence(12),
                None,
                &cancel,
            )
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
        for (mux_packets, evidence) in [
            (Some(12), Count::MuxedPackets(12)),
            (None, Count::ReadPackets(12)),
        ] {
            let count = verify(
                &ffprobe,
                &matroska,
                &["video", "audio"],
                cadence(12),
                mux_packets,
                &cancel,
            )
            .await
            .unwrap();
            assert_eq!(count, evidence);
        }

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
                        mux_video_packets: None,
                        count_duration_frame_seconds: None,
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
    fn declared_count_prefers_the_native_field_to_the_track_tag() {
        let mut movie = picture_and_sound().streams.remove(0);
        movie.tags.insert("number_of_frames".into(), "96".into());
        assert_eq!(declared_frames(&movie), Some(96));
        movie.nb_frames = Some("48".into());
        assert_eq!(declared_frames(&movie), Some(48));
        movie.nb_frames = Some("N/A".into());
        assert_eq!(declared_frames(&movie), Some(96));
        movie.tags.insert("number_of_frames".into(), "0".into());
        assert_eq!(declared_frames(&movie), None);
    }

    #[test]
    fn packet_mismatches_only_fail_when_packet_cardinality_is_known() {
        let mut movie = picture_and_sound().streams.remove(0);
        assert!(Count::ReadPackets(11).verify(&movie, 12, None).is_err());
        movie.codec_name = Some("av1".into());
        assert!(Count::ReadPackets(11).verify(&movie, 12, None).is_ok());
        assert!(Count::MuxedPackets(0).verify(&movie, 12, None).is_err());
        assert!(Count::Declared(11).verify(&movie, 12, None).is_err());
    }

    #[test]
    fn a_short_zero_origin_track_corroborates_an_ambiguous_packet_count() {
        let mut movie = picture_and_sound().streams.remove(0);
        movie.codec_name = Some("av1".into());
        movie.start_time = Some("0".into());
        movie.duration = Some("0.8".into());
        assert!(Count::ReadPackets(8).verify(&movie, 12, None).is_ok());
        assert!(Count::ReadPackets(8).verify(&movie, 12, Some(0.1)).is_err());
        assert!(Count::ReadPackets(12).verify(&movie, 12, Some(0.1)).is_ok());
        movie.start_time = Some("0.4".into());
        assert!(Count::ReadPackets(8).verify(&movie, 12, Some(0.1)).is_ok());
    }

    #[test]
    fn mux_count_needs_a_complete_final_report_for_the_movie() {
        assert!(movie_is_first_video(&["audio", "video", "video"], 1));
        assert!(!movie_is_first_video(&["video", "audio", "video"], 2));

        let mut progress = MuxProgress::default();
        progress.observe("frame=24");
        progress.observe("progress=continue");
        assert_eq!(progress.completed(), None);
        progress.observe("progress=end");
        assert_eq!(progress.completed(), None);
        progress.observe("frame= 48");
        progress.observe("progress=end");
        assert_eq!(progress.completed(), Some(48));
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
