//! Source fixed-format export with no checked video: never manufacture a map.
use super::*;
use crate::jobs::metadata::Document;
use media_core::{
    AudioCodec, ImageImplicitAudio, ImageImplicitMetadata, ImageInputTrack, VideoSeekStrategy,
};

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("IMAGE_SELECTION_INVALID", message, None)
}
fn verify(sources: &[Source]) -> Result<(), AppError> {
    for source in sources {
        source.verify()?;
    }
    Ok(())
}
fn track<'a>(
    documents: &'a [Document],
    source: &ImageInputTrack,
) -> Result<&'a crate::jobs::metadata::Stream, AppError> {
    documents
        .get(source.input_index as usize)
        .and_then(|document| {
            document
                .streams
                .iter()
                .find(|stream| stream.index == source.stream_index)
        })
        .ok_or_else(|| invalid("A checked source stream no longer exists. Import the file again."))
}
fn seconds(value: f64) -> String {
    // TrimSettings.GetTimeString uses the TimeSpan millisecond component.
    let ms = (value * 1000.0).floor().max(0.0) as u64;
    format!("{}.{:03}", ms / 1000, ms % 1000)
}
#[derive(Default)]
struct Trim {
    input: Vec<OsString>,
    output: Vec<OsString>,
}
fn trim_args(
    trim: Option<media_core::VideoTrim>,
    current: &Document,
    keep: bool,
) -> Result<Trim, AppError> {
    let Some(trim) = trim else {
        return Ok(Trim::default());
    };
    let rate = current
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .and_then(|video| {
            let rational = |text: &str| {
                let (n, d) = text.split_once('/')?;
                let n = n.parse::<f64>().ok()?;
                let d = d.parse::<f64>().ok()?;
                let rate = n / d;
                (rate.is_finite() && rate > 0.01).then_some(rate)
            };
            video
                .avg_frame_rate
                .as_deref()
                .and_then(rational)
                .or_else(|| video.r_frame_rate.as_deref().and_then(rational))
        });
    let (start, end, frame) = if let Some(time) = trim.time {
        if time.end_milliseconds <= time.start_milliseconds {
            return Err(invalid("The trim interval is empty."));
        }
        (
            f64::from(time.start_milliseconds) / 1000.0,
            f64::from(time.end_milliseconds) / 1000.0,
            false,
        )
    } else {
        if trim.end_frame_exclusive <= trim.start_frame {
            return Err(invalid("The trim interval is empty."));
        }
        let rate = rate.ok_or_else(|| {
            invalid("The current source has no video frame rate for a frame-number trim.")
        })?;
        (
            f64::from(trim.start_frame) / rate,
            f64::from(trim.end_frame_exclusive) / rate,
            true,
        )
    };
    let clipped_end = current
        .duration()
        .filter(|value| *value > 0.0)
        .map_or(end, |duration| end.min(duration));
    if (clipped_end * 1000.0).round_ties_even() <= (start * 1000.0).round_ties_even() {
        return Err(invalid(
            "The current source ends before the configured trim start.",
        ));
    }
    let mut result = Trim::default();
    if frame {
        let frame = 1.0 / rate.expect("validated frame rate");
        result.output = args(&[
            "-ss",
            &seconds((start - frame / 2.0).max(0.0)),
            "-t",
            &seconds(end - start + frame / 2.0),
        ]);
        if keep {
            result.output.extend(args(&[
                "-frames:v",
                &(trim.end_frame_exclusive - trim.start_frame).to_string(),
            ]));
        }
    } else {
        let seek = args(&["-ss", &seconds(start)]);
        if trim.time.expect("time trim").seek_strategy == VideoSeekStrategy::InputSeek {
            result.input = seek;
        } else {
            result.output = seek;
        }
        result.output.extend(args(&["-t", &seconds(end - start)]));
    }
    Ok(result)
}
pub(super) fn metadata_args(
    value: &ImageImplicitMetadata,
    documents: &[Document],
) -> Result<Vec<OsString>, AppError> {
    let input = |index: u32| {
        if (index as usize) < documents.len() {
            Ok(index.to_string())
        } else {
            Err(invalid("The metadata donor is not a loaded input."))
        }
    };
    let mut result = args(&[
        "-map_metadata",
        &value
            .source_input
            .map(input)
            .transpose()?
            .unwrap_or_else(|| "-1".into()),
    ]);
    for index in &value.attachment_inputs {
        input(*index)?;
        if value.source_input.is_some()
            || !documents[*index as usize]
                .streams
                .iter()
                .any(|stream| stream.codec_type.as_deref() == Some("attachment"))
        {
            return Err(invalid(
                "Attachment metadata requires a loaded attachment source and stripped global metadata.",
            ));
        }
        result.extend(args(&["-map_metadata:s:t", &format!("{index}:s:t")]));
    }
    result.extend(args(&[
        "-map_chapters",
        &value
            .chapters_input
            .map(input)
            .transpose()?
            .unwrap_or_else(|| "-1".into()),
    ]));
    for (key, text) in [("title", &value.title), ("language", &value.language)] {
        if let Some(text) = text {
            result.extend(args(&["-metadata", &format!("{key}={text}")]));
        }
    }
    for (index, track) in value.tracks.iter().enumerate() {
        for (key, text) in [("title", &track.title), ("language", &track.language)] {
            if let Some(text) = text {
                result.extend(args(&[
                    &format!("-metadata:s:{index}"),
                    &format!("{key}={text}"),
                ]));
            }
        }
    }
    for (kind, defaults) in [
        ("a", &value.audio_dispositions),
        ("s", &value.subtitle_dispositions),
    ] {
        for (index, default) in defaults.iter().enumerate() {
            result.extend(args(&[
                &format!("-disposition:{kind}:{index}"),
                if *default { "default" } else { "0" },
            ]));
        }
    }
    Ok(result)
}

pub(super) async fn export(
    request: ImageRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    cancel: &watch::Receiver<bool>,
    session: Option<&crate::jobs::command_edit::Session>,
) -> Result<ImageResult, AppError> {
    let ImageRequest::ImplicitExport {
        inputs,
        checked,
        current_input,
        format,
        output_path,
        audio,
        subtitle_codec,
        metadata,
        trim,
        keep_frame_count,
    } = request
    else {
        unreachable!()
    };
    let output = destination(&output_path)?;
    if !matches!(
        format,
        ImageOutput::PngSequence | ImageOutput::JpegSequence | ImageOutput::Gif
    ) {
        return Err(invalid(
            "Implicit fixed-format export requires PNG/JPEG sequence or GIF.",
        ));
    }
    if inputs.is_empty() || current_input as usize >= inputs.len() || checked.is_empty() {
        return Err(invalid("Load a source and check at least one track."));
    }
    let mut sources = Vec::with_capacity(inputs.len());
    let mut documents = Vec::with_capacity(inputs.len());
    for input in inputs {
        check(cancel)?;
        let source = Source::open(Path::new(&input))?;
        let document = crate::jobs::probe(ffprobe, &source.path, cancel, None).await?;
        source.verify()?;
        sources.push(source);
        documents.push(document);
    }
    let mut identities = std::collections::HashSet::new();
    let mut checked_audio = Vec::new();
    for source in &checked {
        if !identities.insert((source.input_index, source.stream_index)) {
            return Err(invalid("A checked source track is duplicated."));
        }
        match track(&documents, source)?.codec_type.as_deref() {
            Some("video") => {
                return Err(invalid(
                    "Checked video must use the explicit image export route.",
                ));
            }
            Some("audio") => checked_audio.push(*source),
            Some("subtitle" | "data" | "attachment") => {}
            _ => return Err(invalid("The checked track kind is unsupported.")),
        }
    }
    if checked_audio.is_empty() != audio.is_none() {
        return Err(invalid(
            "Audio options must be present exactly when an audio track is checked.",
        ));
    }
    let trim = trim_args(trim, &documents[current_input as usize], keep_frame_count)?;
    let mut a = args(&["-v", "error", "-nostdin", "-y"]);
    for source in &sources {
        a.extend_from_slice(&trim.input);
        a.push("-i".into());
        a.push(source.path.as_os_str().to_owned());
    }
    if let Some(ImageImplicitAudio::Encode { codec, tracks }) = &audio
        && (*codec == AudioCodec::Copy
            || tracks.iter().map(|value| value.source).collect::<Vec<_>>() != checked_audio)
    {
        return Err(invalid(
            "Converted audio must describe the checked audio tracks in source UI order.",
        ));
    }
    // Deliberate source bug correction: image2 otherwise writes explicitly selected
    // subtitle packets as falsely named PNG/JPEG files, and GIF rejects
    // those streams. Fixed image outputs retain only automatically chosen video.
    // Nonvideo codec/filter/loudness settings have no output stream to affect.
    let _ = subtitle_codec;
    a.extend(args(&["-an", "-sn", "-dn"]));
    a.extend(metadata_args(&metadata, &documents)?);
    a.extend(trim.output);
    let extension = match format {
        ImageOutput::PngSequence => "png",
        ImageOutput::JpegSequence => "jpg",
        ImageOutput::Gif => "gif",
        _ => unreachable!(),
    };
    let mut spool =
        Temporary::create_extension(&output, &format!("{}-implicit-spool", id()), extension)?;
    review::private_file(&spool)?;
    // No explicit map, video codec, picture filter, sync mode, palette, quality or
    // pixel layout: those are absent in the source branch too. image2pipe merely
    // retains the encoded packets for guarded atomic sequence publication.
    a.extend(args(&[
        "-f",
        if format == ImageOutput::Gif {
            "gif"
        } else {
            "image2pipe"
        },
    ]));
    a.push(spool.path.as_os_str().to_owned());
    let command = CommandSpec {
        executable: ffmpeg.to_owned(),
        args: a,
        cwd: None,
    };
    let verify_all = || verify(&sources);
    verify_all()?;
    check(cancel)?;
    let edited = if let Some(session) = session {
        let generated = crate::jobs::command_edit::format_command(&command);
        let edited = session.ask("ffmpeg", &generated, &output, cancel).await?;
        verify_all()?;
        check(cancel)?;
        session.acknowledge(Ok(()));
        (edited.arguments != edited.generated.trim()).then_some(edited.arguments)
    } else {
        None
    };
    let execution = if let Some(program) = edited {
        let mut log =
            Temporary::create_extension(&output, &format!("{}-implicit-log", id()), "log")?;
        review::private_file(&log)?;
        let shell = crate::jobs::command_edit::PreparedShell::new(&program, None, &log.path, None)?;
        // Complete fallible shell preparation while reserved outputs still
        // retain handles; release them only for the supervised path writer.
        log.close_for_path_writer()?;
        spool.close_for_path_writer()?;
        let (sender, mut events) = tokio::sync::mpsc::channel(64);
        let observer = tokio::spawn(async move { while events.recv().await.is_some() {} });
        let result = supervisor::run_with_environment(
            &shell.command,
            cancel.clone(),
            sender,
            &log.path,
            Duration::from_secs(86400),
            Some(&shell.environment),
            None,
        )
        .await;
        let _ = observer.await;
        let rebound = log.reopen_after_path_writer();
        match result.map_err(|cause| process_error(cause, &output)) {
            Err(cause) => Err(cause),
            Ok(result) if !result.status.success() => Err(error(
                "The edited automatic-selection image program failed; no output was published.",
                &output,
            )),
            Ok(_) => rebound.and_then(|()| shell.verify()),
        }
    } else {
        spool.close_for_path_writer()?;
        run(ffmpeg, command.args, None, cancel).await.map(|_| ())
    };
    let rebound = spool.reopen_after_path_writer();
    execution?;
    rebound?;
    spool.flush_nonempty_async().await?;
    verify_all()?;
    check(cancel)?;
    let mut result = review::publish_spool(
        &mut spool,
        ffmpeg,
        ffprobe,
        &output,
        format,
        format != ImageOutput::Gif,
        true,
        cancel,
        verify_all,
    )
    .await?;

    result.notes.push("No video was checked: FFmpeg automatically selected streams from all loaded inputs, with the source's metadata and trim options. Audio, subtitle and data output are suppressed to correct malformed source image files and incompatible GIF streams.".into());
    Ok(result)
}
