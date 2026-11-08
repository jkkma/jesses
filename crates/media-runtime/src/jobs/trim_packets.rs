//! Bounded packet evidence for the fork's copied-track seek. The reference mux
//! fixes packet selection; an independent source scan proves payload/order and
//! one common output clock, including FFmpeg's negative subtitle-PTS clamp.
use crate::jobs::{metadata::Document, mux, process_error};
use crate::supervisor;
use media_core::AppError;
use std::{future::Future, path::Path, pin::Pin, sync::mpsc, task::Poll, time::Duration};
use tokio::sync::watch;

#[derive(Debug)]
pub(super) struct Evidence {
    pub first: Option<f64>,
    pub end: Option<f64>,
    pub packets: u64,
}

pub(super) type Check<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Drives a batch of per-track checks together and returns their results in
/// the batch's order. Each check owns its FFprobe processes, so the batch size
/// bounds how many run at once; dropping the batch stops them all.
pub(super) async fn join_in_order<T>(checks: Vec<Check<'_, T>>) -> Vec<T> {
    let mut checks: Vec<_> = checks.into_iter().map(Some).collect();
    let mut results: Vec<Option<T>> = checks.iter().map(|_| None).collect();
    std::future::poll_fn(|context| {
        let mut pending = false;
        for (slot, result) in checks.iter_mut().zip(results.iter_mut()) {
            if let Some(check) = slot {
                match check.as_mut().poll(context) {
                    Poll::Ready(value) => {
                        *result = Some(value);
                        *slot = None;
                    }
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    results
        .into_iter()
        .map(|result| result.expect("every joined check completed"))
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn verify(
    ffprobe: &Path,
    source: &Path,
    source_index: u32,
    reference: &Path,
    reference_index: u32,
    output: &Path,
    output_index: u32,
    clock: f64,
    origin: f64,
    subtitle: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<Evidence, AppError> {
    // First compare the entire independently muxed reference sequence. This
    // rejects omitted leading/trailing packets as well as holes in its middle.
    compare_reference(
        ffprobe,
        reference,
        reference_index,
        output,
        output_index,
        cancel,
    )
    .await?;
    let (sender, receiver) = mpsc::sync_channel(16);
    let actual_spec = mux::packet_spec(ffprobe, output, output_index);
    let source_spec = mux::packet_spec(ffprobe, source, source_index);
    let actual = supervisor::run_streaming_stdout(
        &actual_spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            mux::packets(reader, |packet| {
                sender
                    .send(packet)
                    .map_err(|_| "Source comparison stopped.".to_string())
            })
        },
    );
    let expected = supervisor::run_streaming_stdout(
        &source_spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            let mut next = receiver.recv().ok();
            let mut started = false;
            let mut evidence = Evidence {
                first: None,
                end: None,
                packets: 0,
            };
            mux::packets(reader, |packet| {
                let Some(actual) = next.as_ref() else {
                    return Ok(());
                };
                if !started && (packet.hash != actual.hash || packet.size != actual.size) {
                    return Ok(());
                }
                started = true;
                if packet.hash != actual.hash
                    || packet.size != actual.size
                    || !same(packet.duration, actual.duration)
                {
                    return Err(
                        "Copied trim changed a packet payload, duration or its source order."
                            .into(),
                    );
                }
                let shifted = |value: Option<f64>| {
                    value.map(|time| {
                        let shifted = time - clock + origin;
                        if subtitle { shifted.max(0.0) } else { shifted }
                    })
                };
                let pts = shifted(packet.pts);
                let dts = shifted(packet.dts);
                if !same(pts, actual.pts) || !same(dts, actual.dts) {
                    return Err(format!(
                        "Copied trim changed the common seek clock: expected PTS/DTS {pts:?}/{dts:?}, observed {:?}/{:?}.",
                        actual.pts, actual.dts
                    ));
                }
                if evidence.packets == 0 {
                    evidence.first = pts;
                }
                if let Some(end) = pts
                    .zip(packet.duration)
                    .map(|(pts, duration)| pts + duration)
                {
                    evidence.end = Some(evidence.end.map_or(end, |old| old.max(end)));
                }
                evidence.packets += 1;
                next = receiver.recv().ok();
                Ok(())
            })?;
            if next.is_some() {
                return Err(
                    "Copied trim contains packets outside the guarded source sequence.".into(),
                );
            }
            Ok(evidence)
        },
    );
    let (actual, expected) = tokio::join!(Box::pin(actual), Box::pin(expected));
    let actual = actual.map_err(|error| process_error(error, output))?;
    let expected = expected.map_err(|error| process_error(error, source))?;
    if !actual.status.success()
        || !expected.status.success()
        || !actual.stderr.is_empty()
        || !expected.stderr.is_empty()
        || actual.value != expected.value.packets
    {
        return Err(AppError::new(
            "TRIM_VALIDATION_FAILED",
            "The complete source copied-packet trim proof failed.",
            None,
        ));
    }
    Ok(expected.value)
}
fn same(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 0.002,
        (None, None) => true,
        _ => false,
    }
}
#[allow(clippy::too_many_arguments)]
async fn compare_reference(
    ffprobe: &Path,
    reference: &Path,
    reference_index: u32,
    output: &Path,
    output_index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let (sender, receiver) = mpsc::sync_channel(16);
    let source_spec = mux::packet_spec(ffprobe, reference, reference_index);
    let output_spec = mux::packet_spec(ffprobe, output, output_index);
    let reference_run = supervisor::run_streaming_stdout(
        &source_spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            mux::packets(reader, |packet| {
                sender
                    .send(packet)
                    .map_err(|_| "Packet reference comparison stopped.".to_owned())
            })
        },
    );
    let output_run = supervisor::run_streaming_stdout(
        &output_spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            let count = mux::packets(reader, |packet| {
                let expected = receiver
                    .recv()
                    .map_err(|_| "Trim has extra packets.".to_owned())?;
                if expected.hash != packet.hash
                    || expected.size != packet.size
                    || !same(expected.duration, packet.duration)
                {
                    return Err("Trim packet content/order differs from the source-equivalent seek reference.".into());
                }
                Ok(())
            })?;
            if receiver.recv().is_ok() {
                return Err("Trim is missing reference packets.".into());
            }
            Ok(count)
        },
    );
    let (reference_run, output_run) = tokio::join!(Box::pin(reference_run), Box::pin(output_run));
    let reference_run = reference_run.map_err(|error| process_error(error, reference))?;
    let output_run = output_run.map_err(|error| process_error(error, output))?;
    if !reference_run.status.success()
        || !output_run.status.success()
        || !reference_run.stderr.is_empty()
        || !output_run.stderr.is_empty()
        || reference_run.value != output_run.value
    {
        return Err(AppError::new(
            "TRIM_VALIDATION_FAILED",
            "The source-equivalent copied-packet reference did not match.",
            None,
        ));
    }
    Ok(())
}

/// Subtitle encoding is deliberately performed by the same source seek/mux
/// command, so compare every encoded access unit and its actual shared clock.
/// Original subtitle payloads are not copied packets after codec conversion.
/// The callers probe the reference and output once for all of their tracks.
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_converted_reference(
    ffprobe: &Path,
    reference: &Path,
    reference_document: &Document,
    reference_index: u32,
    output: &Path,
    output_document: &Document,
    output_index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<Evidence, AppError> {
    let expected_header = reference_document
        .streams
        .iter()
        .find(|stream| stream.index == reference_index)
        .ok_or_else(|| {
            AppError::new(
                "TRIM_VALIDATION_FAILED",
                "The converted subtitle reference lost its codec header.",
                None,
            )
        })?;
    let actual_header = output_document
        .streams
        .iter()
        .find(|stream| stream.index == output_index)
        .ok_or_else(|| {
            AppError::new(
                "TRIM_VALIDATION_FAILED",
                "The converted subtitle output lost its codec header.",
                None,
            )
        })?;
    if expected_header.codec_name != actual_header.codec_name
        || expected_header.extradata_hash != actual_header.extradata_hash
    {
        return Err(AppError::new(
            "TRIM_VALIDATION_FAILED",
            "Converted subtitle codec headers or styles differ from the source-equivalent command.",
            None,
        ));
    }
    let (sender, receiver) = mpsc::sync_channel(16);
    let before_spec = mux::packet_spec(ffprobe, reference, reference_index);
    let after_spec = mux::packet_spec(ffprobe, output, output_index);
    let before = supervisor::run_streaming_stdout(
        &before_spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            mux::packets(reader, |packet| {
                sender
                    .send(packet)
                    .map_err(|_| "Converted subtitle reference stopped.".into())
            })
        },
    );
    let after = supervisor::run_streaming_stdout(
        &after_spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            let mut evidence = Evidence {
                first: None,
                end: None,
                packets: 0,
            };
            mux::packets(reader, |packet| {
                let expected = receiver
                    .recv()
                    .map_err(|_| "Converted subtitle has extra packets.".to_owned())?;
                if packet.hash != expected.hash
                    || packet.size != expected.size
                    || !same(packet.duration, expected.duration)
                    || !same(packet.pts, expected.pts)
                    || !same(packet.dts, expected.dts)
                {
                    return Err("Converted subtitle payload/order/duration or common clock differs from the source-equivalent encoder command.".into());
                }
                if evidence.packets == 0 {
                    evidence.first = packet.pts;
                }
                if let Some(end) = packet
                    .pts
                    .zip(packet.duration)
                    .map(|(pts, duration)| pts + duration)
                {
                    evidence.end = Some(evidence.end.map_or(end, |old| old.max(end)));
                }
                evidence.packets += 1;
                Ok(())
            })?;
            if receiver.recv().is_ok() {
                return Err("Converted subtitle is missing reference packets.".into());
            }
            Ok(evidence)
        },
    );
    let (before, after) = tokio::join!(Box::pin(before), Box::pin(after));
    let before = before.map_err(|error| process_error(error, reference))?;
    let after = after.map_err(|error| process_error(error, output))?;
    if !before.status.success()
        || !after.status.success()
        || !before.stderr.is_empty()
        || !after.stderr.is_empty()
        || before.value != after.value.packets
    {
        return Err(AppError::new(
            "TRIM_VALIDATION_FAILED",
            "The complete source-equivalent subtitle conversion reference did not match.",
            None,
        ));
    }
    Ok(after.value)
}

pub(super) async fn infer_origin(
    ffprobe: &Path,
    source: &Path,
    source_index: u32,
    reference: &Path,
    reference_index: u32,
    clock: f64,
    cancel: &watch::Receiver<bool>,
) -> Result<f64, AppError> {
    let spec = mux::packet_spec(ffprobe, reference, reference_index);
    let reference_run = supervisor::run_streaming_stdout(
        &spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        |reader| {
            let mut first = None;
            mux::packets(reader, |packet| {
                if first.is_none() {
                    first = Some(packet);
                }
                Ok(())
            })?;
            Ok(first)
        },
    )
    .await
    .map_err(|error| process_error(error, reference))?;
    if !reference_run.status.success() || !reference_run.stderr.is_empty() {
        return Err(AppError::new(
            "TRIM_VALIDATION_FAILED",
            "The copied-trim reference clock could not be read.",
            None,
        ));
    }
    let Some(first) = reference_run.value else {
        return Ok(0.0);
    };
    let spec = mux::packet_spec(ffprobe, source, source_index);
    let source_run = supervisor::run_streaming_stdout(
        &spec,
        cancel.clone(),
        8192,
        Duration::from_secs(86400),
        move |reader| {
            let mut origin = None;
            mux::packets(reader, |packet| {
                if origin.is_none() && packet.hash == first.hash && packet.size == first.size {
                    origin = packet
                        .pts
                        .zip(first.pts)
                        .map(|(original, actual)| actual - original + clock);
                }
                Ok(())
            })?;
            Ok(origin)
        },
    )
    .await
    .map_err(|error| process_error(error, source))?;
    if !source_run.status.success() || !source_run.stderr.is_empty() {
        return Err(AppError::new(
            "TRIM_VALIDATION_FAILED",
            "The original copied-trim clock could not be read.",
            None,
        ));
    }
    source_run.value.ok_or_else(|| {
        AppError::new(
            "TRIM_VALIDATION_FAILED",
            "The source copied-trim reference has no matching first packet.",
            None,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn joined_checks_run_together_and_keep_their_order() {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let waits: Check<'_, u32> = Box::pin(async move { receiver.await.unwrap() });
        let sends: Check<'_, u32> = Box::pin(async move {
            sender.send(2).unwrap();
            1
        });
        // Run one after another, the first check would wait forever.
        assert_eq!(join_in_order(vec![waits, sends]).await, [2, 1]);
        assert!(join_in_order::<u32>(Vec::new()).await.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires qualified CPU FFmpeg/FFprobe and MOV_TEXT"]
    async fn converted_movtext_reference_rejects_changed_payload_despite_equal_movie_duration() {
        let (_owner, cancel) = watch::channel(false);
        let ffmpeg = crate::jobs::discover("ffmpeg", &cancel).await.unwrap();
        let ffprobe = crate::jobs::discover("ffprobe", &cancel).await.unwrap();
        let root = std::env::temp_dir().join(format!(
            "jesses-movtext-reference-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut movies = vec![];
        for word in ["original", "tampered"] {
            let subtitle = root.join(format!("{word}.srt"));
            std::fs::write(&subtitle,format!("1\n00:00:00,000 --> 00:00:02,500\n{word} first\n\n2\n00:00:00,250 --> 00:00:03,500\n{word} overlap\n\n")).unwrap();
            let output = root.join(format!("{word}.mp4"));
            let mut args = [
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "color=black:s=16x16:r=24:d=2",
                "-ss",
                "0.979",
                "-i",
            ]
            .map(Into::into)
            .to_vec();
            args.push(subtitle.into_os_string());
            args.extend(
                [
                    "-map",
                    "0:v",
                    "-map",
                    "1:s",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-c:s",
                    "mov_text",
                    "-t",
                    "2.020",
                ]
                .map(Into::into),
            );
            args.push(output.as_os_str().to_owned());
            let result = supervisor::run_capture(
                &supervisor::CommandSpec {
                    executable: ffmpeg.clone(),
                    args,
                    cwd: None,
                },
                cancel.clone(),
                1024 * 1024,
                Duration::from_secs(60),
            )
            .await
            .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            movies.push(output);
        }
        let before = crate::jobs::probe(&ffprobe, &movies[0], &cancel, None)
            .await
            .unwrap();
        let after = crate::jobs::probe(&ffprobe, &movies[1], &cancel, None)
            .await
            .unwrap();
        assert_eq!(
            before.format.as_ref().unwrap().duration,
            after.format.as_ref().unwrap().duration
        );
        assert_eq!(before.streams[1].duration, after.streams[1].duration);
        assert_eq!(
            before.streams[1].extradata_hash,
            after.streams[1].extradata_hash
        );
        verify_converted_reference(
            &ffprobe, &movies[0], &before, 1, &movies[0], &before, 1, &cancel,
        )
        .await
        .unwrap();
        let error = verify_converted_reference(
            &ffprobe, &movies[0], &before, 1, &movies[1], &after, 1, &cancel,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "TOOL_FAILED");
        assert!(
            error
                .message
                .contains("payload/order/duration or common clock"),
            "{error:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
