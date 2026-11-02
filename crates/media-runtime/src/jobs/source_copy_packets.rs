//! Full source Copy seek selection, original payload order and one shared clock.
//! A separate original-input copy reference fixes both ends of every selected
//! packet sequence; converted streams remain the caller's separate consumers.
use super::*;
use std::sync::mpsc as sync;
const TIME: Duration = Duration::from_secs(86400);
fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("COPY_TRIM_VALIDATION_FAILED", message, None)
}
struct Track {
    index: u32,
    output: u32,
    input: usize,
    source: u32,
    path: PathBuf,
    clock: f64,
    bound: f64,
    subtitle: bool,
    video: bool,
    copied: bool,
}
fn same(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 0.002,
        (None, None) => true,
        _ => false,
    }
}
fn shifted(time: Option<f64>, shift: f64, subtitle: bool) -> Option<f64> {
    time.map(|time| {
        if subtitle {
            (time + shift).max(0.0)
        } else {
            time + shift
        }
    })
}
async fn first(
    ffprobe: &Path,
    path: &Path,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<mux::Packet, AppError> {
    let run = supervisor::run_streaming_stdout(
        &mux::packet_spec(ffprobe, path, index),
        cancel.clone(),
        8192,
        TIME,
        |reader| {
            let mut first = None;
            mux::packets(reader, |packet| {
                if first.is_none() {
                    first = Some(packet);
                }
                Ok(())
            })?;
            first.ok_or_else(|| "The copied picture has no packets.".to_owned())
        },
    )
    .await
    .map_err(|error| process_error(error, path))?;
    if !run.status.success() || !run.stderr.is_empty() {
        return Err(invalid(
            "The complete copied-picture packet clock could not be read.",
        ));
    }
    Ok(run.value)
}
async fn anchor(
    ffprobe: &Path,
    track: &Track,
    first: &mux::Packet,
    cancel: &watch::Receiver<bool>,
) -> Result<f64, AppError> {
    let hash = first.hash.clone();
    let size = first.size;
    let bound = track.bound;
    let run=supervisor::run_streaming_stdout(&mux::packet_spec(ffprobe,&track.path,track.source),cancel.clone(),8192,TIME,move|reader|{let mut found=None;mux::packets(reader,|packet|{if packet.hash==hash&&packet.size==size&&packet.pts.is_some_and(|pts|pts<=bound+0.002){found=packet.pts;}Ok(())})?;found.ok_or_else(||"The source-equivalent first GOP has no original access unit at or before its seek.".to_owned())}).await.map_err(|error|process_error(error,&track.path))?;
    if !run.status.success() || !run.stderr.is_empty() {
        return Err(invalid(
            "The original copied-picture seek anchor could not be verified.",
        ));
    }
    Ok(run.value)
}
#[derive(Debug)]
struct Evidence {
    first: Option<f64>,
    end: Option<f64>,
    count: u64,
}
async fn compare_reference(
    ffprobe: &Path,
    reference: &Path,
    reference_index: u32,
    output: &Path,
    track: &Track,
    delta: f64,
    cancel: &watch::Receiver<bool>,
) -> Result<Evidence, AppError> {
    let (sender, receiver) = sync::sync_channel(16);
    let before = mux::packet_spec(ffprobe, reference, reference_index);
    let after = mux::packet_spec(ffprobe, output, track.output);
    let subtitle = track.subtitle && track.copied;
    let before =
        supervisor::run_streaming_stdout(&before, cancel.clone(), 8192, TIME, move |reader| {
            mux::packets(reader, |packet| {
                sender
                    .send(packet)
                    .map_err(|_| "Copied seek reference comparison stopped.".into())
            })
        });
    let after = supervisor::run_streaming_stdout(
        &after,
        cancel.clone(),
        8192,
        TIME,
        move |reader| {
            let mut proof = Evidence {
                first: None,
                end: None,
                count: 0,
            };
            mux::packets(reader, |packet| {
                let expected = receiver
                    .recv()
                    .map_err(|_| "The copied seek contains extra packets.".to_owned())?;
                if expected.hash != packet.hash
                    || expected.size != packet.size
                    || !same(expected.duration, packet.duration)
                {
                    return Err("Copied seek payload/order/duration differs from the complete independent source reference.".into());
                }
                if !subtitle
                    && (!same(shifted(expected.pts, delta, false), packet.pts)
                        || !same(shifted(expected.dts, delta, false), packet.dts))
                {
                    return Err(
                        "A copied track uses a different clock from the selected source GOP."
                            .into(),
                    );
                }
                if proof.count == 0 {
                    proof.first = packet.pts;
                }
                if let Some(end) = packet
                    .pts
                    .zip(packet.duration)
                    .map(|(pts, duration)| pts + duration)
                {
                    proof.end = Some(proof.end.map_or(end, |previous| previous.max(end)));
                }
                proof.count += 1;
                Ok(())
            })?;
            if receiver.recv().is_ok() {
                return Err("The copied seek is missing reference packets, including a leading or trailing access unit.".into());
            }
            Ok(proof)
        },
    );
    let (before, after) = tokio::join!(Box::pin(before), Box::pin(after));
    let before = before.map_err(|error| process_error(error, reference))?;
    let after = after.map_err(|error| process_error(error, output))?;
    if !before.status.success()
        || !after.status.success()
        || !before.stderr.is_empty()
        || !after.stderr.is_empty()
        || before.value != after.value.count
    {
        return Err(invalid("The full source copied-seek reference failed."));
    }
    Ok(after.value)
}
async fn original_sequence(
    ffprobe: &Path,
    output: &Path,
    track: &Track,
    origin: f64,
    cancel: &watch::Receiver<bool>,
) -> Result<Evidence, AppError> {
    let (sender, receiver) = sync::sync_channel(16);
    let before = mux::packet_spec(ffprobe, &track.path, track.source);
    let after = mux::packet_spec(ffprobe, output, track.output);
    let shift = origin - track.clock;
    let subtitle = track.subtitle;
    let video = track.video;
    let output_format = container::format(output)?;
    let allow_omission = video
        && matches!(
            output_format,
            media_core::ContainerFormat::Matroska | media_core::ContainerFormat::Webm
        );
    let allow_reconstruction = video
        && matches!(
            output_format,
            media_core::ContainerFormat::Mp4 | media_core::ContainerFormat::Mov
        );
    let after =
        supervisor::run_streaming_stdout(&after, cancel.clone(), 8192, TIME, move |reader| {
            mux::packets(reader, |packet| {
                sender
                    .send(packet)
                    .map_err(|_| "Original copied-seek comparison stopped.".into())
            })
        });
    let before = supervisor::run_streaming_stdout(
        &before,
        cancel.clone(),
        8192,
        TIME,
        move |reader| {
            let mut next = receiver.recv().ok();
            let mut started = false;
            let mut source_dts_seen = false;
            let mut target_dts_seen = false;
            let mut previous_source_dts = None;
            let mut previous_target_dts = None;
            let mut evidence = Evidence {
                first: None,
                end: None,
                count: 0,
            };
            mux::packets(reader, |packet| {
                let Some(actual) = next.as_ref() else {
                    return Ok(());
                };
                let pts = shifted(packet.pts, shift, subtitle);
                let dts = shifted(packet.dts, shift, subtitle);
                let equal = packet.hash == actual.hash
                    && packet.size == actual.size
                    && same(packet.duration, actual.duration)
                    && same(pts, actual.pts);
                if !started {
                    if !equal {
                        return Ok(());
                    }
                    started = true;
                    evidence.first = pts;
                }
                let omitted = allow_omission
                    && evidence.count < 16
                    && !target_dts_seen
                    && actual.dts.is_none()
                    && dts.zip(pts).is_some_and(|(dts, pts)| {
                        dts <= pts && previous_source_dts.is_none_or(|previous| dts >= previous)
                    });
                let reconstructed = allow_reconstruction
                    && evidence.count < 16
                    && !source_dts_seen
                    && dts.is_none()
                    && actual.dts.zip(actual.pts).is_some_and(|(dts, pts)| {
                        dts <= pts && previous_target_dts.is_none_or(|previous| dts >= previous)
                    });
                source_dts_seen |= dts.is_some();
                target_dts_seen |= actual.dts.is_some();
                previous_source_dts = dts;
                previous_target_dts = actual.dts;
                if !equal || (!(omitted || reconstructed) && !same(dts, actual.dts)) {
                    return Err("A copied seek packet changed original payload/order/duration or the shared source PTS/DTS clock.".into());
                }
                if let Some(end) = pts
                    .zip(packet.duration)
                    .map(|(pts, duration)| pts + duration)
                {
                    evidence.end = Some(evidence.end.map_or(end, |previous| previous.max(end)));
                }
                evidence.count += 1;
                next = receiver.recv().ok();
                Ok(())
            })?;
            if next.is_some() {
                return Err(
                    "A copied seek packet is not in the contiguous original source sequence."
                        .into(),
                );
            }
            Ok(evidence)
        },
    );
    let (before, after) = tokio::join!(Box::pin(before), Box::pin(after));
    let before = before.map_err(|error| process_error(error, &track.path))?;
    let after = after.map_err(|error| process_error(error, output))?;
    if !before.status.success()
        || !after.status.success()
        || !before.stderr.is_empty()
        || !after.stderr.is_empty()
        || before.value.count != after.value
    {
        return Err(invalid(
            "The complete original copied-seek packet proof failed.",
        ));
    }
    Ok(before.value)
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_copied(
    ffmpeg: &Path,
    ffprobe: &Path,
    primary: &Path,
    document: &metadata::Document,
    output: &Path,
    selected: &[&metadata::Stream],
    artifact: &metadata::Document,
    expected: &mut metadata::Document,
    settings: &EncodeSettings,
    external: &external_tracks::ExternalTracks,
    window: (f64, f64),
    excluded: &[u32],
    scratch: &mut Vec<Temporary>,
    cancel: &watch::Receiver<bool>,
) -> Result<f64, AppError> {
    verify_inner(
        ffmpeg, ffprobe, primary, document, output, selected, artifact, expected, settings,
        external, window, excluded, scratch, cancel, None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_with_normalization(
    ffmpeg: &Path,
    ffprobe: &Path,
    primary: &Path,
    document: &metadata::Document,
    output: &Path,
    selected: &[&metadata::Stream],
    artifact: &metadata::Document,
    expected: &mut metadata::Document,
    settings: &EncodeSettings,
    external: &external_tracks::ExternalTracks,
    window: (f64, f64),
    excluded: &[u32],
    scratch: &mut Vec<Temporary>,
    cancel: &watch::Receiver<bool>,
    normalization: &audio::Normalization,
) -> Result<f64, AppError> {
    verify_inner(
        ffmpeg,
        ffprobe,
        primary,
        document,
        output,
        selected,
        artifact,
        expected,
        settings,
        external,
        window,
        excluded,
        scratch,
        cancel,
        Some(normalization),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn verify_inner(
    ffmpeg: &Path,
    ffprobe: &Path,
    primary: &Path,
    document: &metadata::Document,
    output: &Path,
    selected: &[&metadata::Stream],
    artifact: &metadata::Document,
    expected: &mut metadata::Document,
    settings: &EncodeSettings,
    external: &external_tracks::ExternalTracks,
    window: (f64, f64),
    excluded: &[u32],
    scratch: &mut Vec<Temporary>,
    cancel: &watch::Receiver<bool>,
    normalization: Option<&audio::Normalization>,
) -> Result<f64, AppError> {
    if !window.0.is_finite() || !window.1.is_finite() || window.0 < 0.0 || window.1 <= window.0 {
        return Err(invalid("The source copy seek window is invalid."));
    }
    let normalized = audio::converted(settings)
        .filter(|track| track.normalization.is_some())
        .map(|track| track.stream_index)
        .collect::<Vec<_>>();
    if !normalized.is_empty() && normalization.is_none() {
        return Err(invalid(
            "A normalized source-copy reference needs the exact measured filter snapshot.",
        ));
    }
    let mut inputs: Vec<(PathBuf, f64)> = vec![];
    let mut tracks = vec![];
    for (position, stream) in selected.iter().enumerate() {
        if stream.codec_type.as_deref() == Some("attachment") {
            continue;
        }
        if !matches!(
            stream.codec_type.as_deref(),
            Some("video" | "audio" | "subtitle" | "data")
        ) {
            return Err(invalid("An unknown copied media kind cannot be proven."));
        }
        let (path, source, index) =
            external
                .source_track(stream.index)
                .unwrap_or((primary, document, stream.index));
        let source_origin = source
            .format
            .as_ref()
            .and_then(|format| format.start_time.as_deref())
            .and_then(|time| time.parse::<f64>().ok())
            .filter(|time| time.is_finite())
            .unwrap_or(0.0);
        let offset = external.offset_seconds(stream.index);
        let input = inputs
            .iter()
            .position(|(existing, shift)| existing == path && *shift == offset)
            .unwrap_or_else(|| {
                inputs.push((path.to_owned(), offset));
                inputs.len() - 1
            });
        tracks.push(Track {
            index: stream.index,
            output: position as u32,
            input,
            source: index,
            path: path.to_owned(),
            clock: source_origin + window.0 - offset,
            bound: source_origin + window.0,
            subtitle: stream.codec_type.as_deref() == Some("subtitle"),
            video: stream.codec_type.as_deref() == Some("video"),
            copied: !excluded.contains(&stream.index),
        });
    }
    let video_index = tracks
        .iter()
        .position(|track| track.video && track.copied)
        .ok_or_else(|| invalid("A source Copy seek requires at least one copied picture."))?;
    if artifact.streams.len() < selected.len() {
        return Err(invalid("The copied seek changed selected stream count."));
    }
    let format = container::format(output)?;
    let reference_index = scratch.len();
    scratch.push(Temporary::create_extension(
        output,
        &format!(
            "source-copy-seek-reference-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
        format.extension(),
    )?);
    let reference = scratch[reference_index].path.clone();
    let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-v", "error"]
        .into_iter()
        .map(Into::into)
        .collect();
    let guards = inputs
        .iter()
        .map(|(path, _)| Source::open(path))
        .collect::<Result<Vec<_>, _>>()?;
    for (path, offset) in &inputs {
        if *offset != 0.0 {
            args.extend(["-itsoffset".into(), format!("{offset:.9}").into()]);
        }
        args.extend([
            "-ss".into(),
            format!("{:.3}", window.0).into(),
            "-protocol_whitelist".into(),
            "file".into(),
            "-i".into(),
            path.as_os_str().to_owned(),
        ]);
    }
    for track in &tracks {
        args.extend([
            "-map".into(),
            format!("{}:{}", track.input, track.source).into(),
        ]);
    }
    args.extend([
        "-c".into(),
        "copy".into(),
        "-map_metadata".into(),
        "-1".into(),
        "-map_chapters".into(),
        "-1".into(),
        "-t".into(),
        format!("{:.3}", window.1 - window.0).into(),
        "-max_interleave_delta".into(),
        "0".into(),
    ]);
    // Include encoded audio in the joint reference's mux clock even though its
    // readable/sample/gain proof is owned by the caller. Omitting codec priming
    // here would allow a fitted arbitrary global timestamp shift.
    for (position, track) in tracks.iter().enumerate() {
        let stream = selected[track.output as usize];
        if let Some(audio) =
            audio::converted(settings).find(|audio| audio.stream_index == track.index)
        {
            audio::append_arguments(&mut args, position, audio, stream);
        }
        if track.subtitle && !track.copied {
            let mode = settings
                .subtitles
                .iter()
                .find(|subtitle| subtitle.stream_index == track.index)
                .map(|subtitle| subtitle.mode)
                .ok_or_else(|| invalid("An excluded subtitle has no frozen conversion recipe."))?;
            let codec = match mode {
                media_core::SubtitleMode::Ass => "ass",
                media_core::SubtitleMode::SubRip => "subrip",
                media_core::SubtitleMode::WebVtt => "webvtt",
                media_core::SubtitleMode::MovText => "mov_text",
                _ => {
                    return Err(invalid(
                        "The source Copy seek cannot invent an excluded subtitle conversion.",
                    ));
                }
            };
            args.extend([format!("-c:{position}").into(), codec.into()]);
        }
    }
    let copied = tracks
        .iter()
        .map(|track| selected[track.output as usize])
        .collect::<Vec<_>>();
    if let Some(normalization) = normalization {
        normalization.apply_mux(&mut args, &copied)?;
    }
    container::append_format_options(&mut args, format, &copied);
    args.extend([
        "-f".into(),
        format.ffmpeg_format().into(),
        "-y".into(),
        reference.as_os_str().to_owned(),
    ]);
    scratch[reference_index].close_for_path_writer()?;
    let run = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        1024 * 1024,
        TIME,
    )
    .await;
    let reopened = scratch[reference_index].reopen_after_path_writer();
    let run = run.map_err(|error| process_error(error, primary))?;
    reopened?;
    if !run.status.success() {
        return Err(invalid(format!(
            "The independent source Copy seek reference failed: {}",
            String::from_utf8_lossy(&run.stderr)
        )));
    }
    scratch[reference_index].flush_nonempty_async().await?;
    let video = &tracks[video_index];
    let first_reference = first(ffprobe, &reference, video_index as u32, cancel).await?;
    let first_actual = first(ffprobe, output, video.output, cancel).await?;
    if first_reference.hash != first_actual.hash || first_reference.size != first_actual.size {
        return Err(invalid(
            "The copied picture omitted the source-seek-eligible leading GOP.",
        ));
    }
    let original = anchor(ffprobe, video, &first_reference, cancel).await?;
    let origin = first_actual
        .pts
        .ok_or_else(|| invalid("The copied picture lacks presentation timestamps."))?
        - original
        + video.clock;
    let reference_origin = first_reference
        .pts
        .ok_or_else(|| invalid("The copied seek reference lacks picture timestamps."))?
        - original
        + video.clock;
    if !origin.is_finite() {
        return Err(invalid(
            "The source copied seek has an invalid common origin.",
        ));
    }
    if (origin - reference_origin).abs() > 0.002 {
        return Err(invalid(format!(
            "The copied seek common origin {origin:.6}s differs from the independent joint source mux {reference_origin:.6}s."
        )));
    }
    let mut verified = vec![];
    let reference_document = Box::pin(super::probe(ffprobe, &reference, cancel, None)).await?;
    for (index, track) in tracks.iter().enumerate() {
        if !track.copied && !track.subtitle && !normalized.contains(&track.index) {
            continue;
        }
        let reference_proof = Box::pin(compare_reference(
            ffprobe,
            &reference,
            index as u32,
            output,
            track,
            origin - reference_origin,
            cancel,
        ))
        .await?;
        let codec = if track.subtitle {
            let reference_stream = reference_document
                .streams
                .get(index)
                .ok_or_else(|| invalid("The converted subtitle reference lost its stream."))?;
            let actual_stream = artifact
                .streams
                .get(track.output as usize)
                .ok_or_else(|| invalid("The converted subtitle output lost its stream."))?;
            if reference_stream.codec_name != actual_stream.codec_name
                || reference_stream.extradata_hash != actual_stream.extradata_hash
            {
                return Err(invalid(
                    "Converted subtitle codec headers/styles differ from the source reference.",
                ));
            }
            if track.copied {
                None
            } else {
                reference_stream.codec_name.clone()
            }
        } else if normalized.contains(&track.index) {
            let reference_stream = reference_document
                .streams
                .get(index)
                .ok_or_else(|| invalid("The normalized reference lost its audio track."))?;
            let actual_stream = artifact
                .streams
                .get(track.output as usize)
                .ok_or_else(|| invalid("The normalized output lost its audio track."))?;
            if reference_stream.codec_name != actual_stream.codec_name
                || reference_stream.extradata_hash != actual_stream.extradata_hash
                || reference_stream.sample_rate != actual_stream.sample_rate
                || reference_stream.channels != actual_stream.channels
                || reference_stream.channel_layout != actual_stream.channel_layout
                || reference_stream.bits_per_raw_sample != actual_stream.bits_per_raw_sample
            {
                return Err(invalid(
                    "Normalized audio codec headers/rate/channels differ from the exact source reference.",
                ));
            }
            let rate = reference_stream
                .sample_rate
                .as_deref()
                .and_then(|rate| rate.parse::<u32>().ok())
                .filter(|rate| *rate > 0)
                .ok_or_else(|| invalid("Normalized audio has no declared sample clock."))?;
            let before = decoded_clock(ffprobe, &reference, index as u32, rate, cancel);
            let after = decoded_clock(ffprobe, output, track.output, rate, cancel);
            let (before, after) = tokio::join!(Box::pin(before), Box::pin(after));
            if before? != after? {
                return Err(invalid(
                    "Normalized audio decoded sample/frame clock differs from the exact measured-filter reference.",
                ));
            }
            let before = pcm_hash(ffmpeg, &reference, index as u32, cancel);
            let after = pcm_hash(ffmpeg, output, track.output, cancel);
            let (before, after) = tokio::join!(Box::pin(before), Box::pin(after));
            if before? != after? {
                return Err(invalid(
                    "Normalized audio PCM changed from the exact measured-filter reference.",
                ));
            }
            reference_stream.codec_name.clone()
        } else {
            None
        };
        let proof = if track.copied {
            Box::pin(original_sequence(ffprobe, output, track, origin, cancel)).await?
        } else {
            reference_proof
        };
        verified.push((track, proof, codec));
    }
    for guard in &guards {
        guard.verify()?;
    }
    external.verify()?;
    super::check_cancel(cancel)?;
    for (track, proof, codec) in verified {
        let stream = expected
            .streams
            .iter_mut()
            .find(|stream| stream.index == track.index)
            .ok_or_else(|| {
                invalid("The copied seek lost its original expected stream identity.")
            })?;
        stream.packet_start_time = proof.first;
        stream.start_time = proof.first.map(|time| time.to_string());
        stream.nb_read_packets = Some(proof.count.to_string());
        stream.duration = if codec.as_deref() == Some("mov_text") {
            None
        } else {
            Some(proof.end.unwrap_or(0.0).to_string())
        };
        if let Some(codec) = codec {
            stream.codec_name = Some(codec);
            stream.extradata_hash = None;
        }
        stream
            .tags
            .retain(|key, _| !metadata::is_derived_stream_tag(key));
    }
    Ok(origin)
}

#[derive(serde::Deserialize)]
struct AudioFrame {
    best_effort_timestamp_time: Option<String>,
    nb_samples: Option<u64>,
}
async fn decoded_clock(
    ffprobe: &Path,
    path: &Path,
    index: u32,
    rate: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<([u8; 32], u64, u64), AppError> {
    let spec = CommandSpec {
        executable: ffprobe.to_owned(),
        args: [
            "-v",
            "error",
            "-err_detect",
            "explode",
            "-protocol_whitelist",
            "file",
            "-select_streams",
        ]
        .into_iter()
        .map(OsString::from)
        .chain([index.to_string().into()])
        .chain(
            [
                "-show_frames",
                "-show_entries",
                "frame=best_effort_timestamp_time,nb_samples",
                "-of",
                "json",
                "-i",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .chain([path.as_os_str().to_owned()])
        .collect(),
        cwd: None,
    };
    let result =
        supervisor::run_streaming_stdout(&spec, cancel.clone(), 64 * 1024, TIME, move |reader| {
            use sha2::{Digest, Sha256};
            let mut hash = Sha256::new();
            let mut frames = 0_u64;
            let mut samples = 0_u64;
            let mut failure = None;
            encode::frame_scan::parse::<AudioFrame>(reader, |frame| {
                if failure.is_some() {
                    return;
                }
                let checked = (|| -> Result<(), String> {
                    let pts = frame
                        .best_effort_timestamp_time
                        .as_deref()
                        .and_then(|time| time.parse::<f64>().ok())
                        .filter(|time| time.is_finite())
                        .ok_or("A normalized decoded audio frame has no finite PTS.")?;
                    let count = frame
                        .nb_samples
                        .filter(|count| *count > 0)
                        .ok_or("A normalized decoded audio frame has no samples.")?;
                    let tick = (pts * 1_000_000.).round();
                    if tick < i64::MIN as f64 || tick > i64::MAX as f64 {
                        return Err("Normalized audio PTS exceeds the bounded clock.".into());
                    }
                    hash.update((tick as i64).to_le_bytes());
                    hash.update(count.to_le_bytes());
                    hash.update(rate.to_le_bytes());
                    frames = frames
                        .checked_add(1)
                        .ok_or("Normalized frame count overflow.")?;
                    samples = samples
                        .checked_add(count)
                        .ok_or("Normalized sample count overflow.")?;
                    Ok(())
                })();
                if let Err(error) = checked {
                    failure = Some(error);
                }
            })?;
            if let Some(error) = failure {
                return Err(error);
            }
            if frames == 0 {
                return Err("The normalized reference decoded no audio samples.".into());
            }
            Ok((hash.finalize().into(), frames, samples))
        })
        .await
        .map_err(|error| process_error(error, path))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(invalid(
            "The complete normalized audio reference/output did not decode cleanly.",
        ));
    }
    Ok(result.value)
}
async fn pcm_hash(
    ffmpeg: &Path,
    path: &Path,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<Vec<u8>, AppError> {
    let spec = CommandSpec {
        executable: ffmpeg.to_owned(),
        args: [
            "-v",
            "error",
            "-nostdin",
            "-xerror",
            "-err_detect",
            "explode",
            "-protocol_whitelist",
            "file",
            "-i",
        ]
        .into_iter()
        .map(OsString::from)
        .chain([path.as_os_str().to_owned()])
        .chain(["-map".into(), format!("0:{index}").into()])
        .chain(
            [
                "-c:a",
                "pcm_s32le",
                "-f",
                "hash",
                "-hash",
                "sha256",
                "pipe:1",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .collect(),
        cwd: None,
    };
    let result = supervisor::run_capture(&spec, cancel.clone(), 64 * 1024, TIME)
        .await
        .map_err(|error| process_error(error, path))?;
    if !result.status.success()
        || !result.stderr.is_empty()
        || !result.stdout.starts_with(b"SHA256=")
        || result.stdout.len() > 128
    {
        return Err(invalid(
            "The complete normalized audio PCM hash could not be qualified.",
        ));
    }
    Ok(result.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires qualified CPU FFmpeg/FFprobe and libx264"]
    async fn copied_seek_rejects_global_clock_shift_and_missing_interior_packet() {
        let (_owner, cancel) = watch::channel(false);
        let ffmpeg = discover("ffmpeg", &cancel).await.unwrap();
        let ffprobe = discover("ffprobe", &cancel).await.unwrap();
        let root = std::env::temp_dir().join(format!(
            "jesses-copy-clock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let input = root.join("source.mkv");
        let mut args = [
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=64x48:r=24:d=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=4",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "48",
            "-c:a",
            "pcm_s16le",
        ]
        .map(Into::into)
        .to_vec();
        args.push(input.as_os_str().to_owned());
        let run = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.clone(),
                args,
                cwd: None,
            },
            cancel.clone(),
            1024 * 1024,
            TIME,
        )
        .await
        .unwrap();
        assert!(run.status.success());
        let source = Source::open(&input).unwrap();
        let document = probe(&ffprobe, &input, &cancel, None).await.unwrap();
        let settings = EncodeSettings {
            video_mode: media_core::VideoMode::Copy,
            source_trim: true,
            ..Default::default()
        };
        let selected = document.selected(&[0, 1]).unwrap();
        let request = RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: root.join("unused.mkv").to_string_lossy().into_owned(),
            stream_indices: vec![0, 1],
        };
        let external = external_tracks::ExternalTracks::prepare(
            &source, &request, &settings, &document, &ffprobe, &cancel,
        )
        .await
        .unwrap();
        let mut scratch = vec![];
        for variant in ["valid", "global-shift", "missing-packet"] {
            let output = root.join(format!("{variant}.mkv"));
            let mut args = ["-v", "error", "-ss", "1.000", "-i"]
                .map(Into::into)
                .to_vec();
            args.push(input.as_os_str().to_owned());
            args.extend(["-t", "2.000", "-map", "0", "-c", "copy"].map(Into::into));
            if variant == "global-shift" {
                args.extend(
                    [
                        "-avoid_negative_ts",
                        "disabled",
                        "-output_ts_offset",
                        "1.100",
                    ]
                    .map(Into::into),
                );
            }
            if variant == "missing-packet" {
                args.extend(["-bsf:a", "noise=drop=eq(n\\,30)"].map(Into::into));
            }
            args.push(output.as_os_str().to_owned());
            let run = supervisor::run_capture(
                &CommandSpec {
                    executable: ffmpeg.clone(),
                    args,
                    cwd: None,
                },
                cancel.clone(),
                1024 * 1024,
                TIME,
            )
            .await
            .unwrap();
            assert!(
                run.status.success(),
                "{}",
                String::from_utf8_lossy(&run.stderr)
            );
            let artifact = probe(&ffprobe, &output, &cancel, None).await.unwrap();
            let mut expected = document.clone();
            let result = verify_copied(
                &ffmpeg,
                &ffprobe,
                &input,
                &document,
                &output,
                &selected,
                &artifact,
                &mut expected,
                &settings,
                &external,
                (1.0, 3.0),
                &[],
                &mut scratch,
                &cancel,
            )
            .await;
            if variant == "valid" {
                assert!(result.is_ok(), "{result:?}");
            } else {
                let error = result.unwrap_err();
                assert!(
                    matches!(
                        error.code.as_str(),
                        "COPY_TRIM_VALIDATION_FAILED" | "TOOL_FAILED"
                    ),
                    "{error:?}"
                );
                assert!(
                    error.message.contains("origin") || error.message.contains("reference"),
                    "{error:?}"
                );
            }
        }
        drop(scratch);
        drop(external);
        drop(source);
        std::fs::remove_dir_all(root).unwrap();
    }
}
