//! Verify a source AV1AN join by independently copying its sealed encoded chunks.
//! This never runs an encoder or a filter. Queue/recovery authority stays with
//! the caller; source ordinal counts are not substituted for decoded counts.
use super::super::{
    check_cancel,
    files::{self, Source, Temporary},
    metadata::{Document, Stream},
    mux, probe, process_error,
};
use crate::supervisor::{self, CommandSpec, ConsoleProfile};
use media_core::{AppError, Av1anConcatMethod};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::watch,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const LIMIT: Duration = Duration::from_secs(24 * 60 * 60);
#[cfg(not(windows))]
const GROUP: usize = 960;

#[derive(Clone, Debug)]
pub(in crate::jobs) struct SealedChunk {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: [u8; 32],
    pub source_frames: u64,
    pub decoded_frames: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::jobs) struct Clock {
    pub time_base: (u32, u32),
    pub origin_ticks: i64,
    pub frames: u64,
    pub relative_pts_sha256: [u8; 32],
}
fn invalid(path: &Path, message: impl Into<String>) -> AppError {
    files::error("RECOVERY_INVALID", message, path)
}
async fn checkpoint(cancel: &watch::Receiver<bool>, path: &Path) -> Result<(), AppError> {
    check_cancel(cancel)?;
    if let Some(pause) = supervisor::scoped_pause() {
        pause
            .wait_until_resumed(cancel)
            .await
            .map_err(|cause| process_error(cause, path))?;
    }
    Ok(())
}
async fn digest(
    mut file: tokio::fs::File,
    path: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<(u64, [u8; 32]), AppError> {
    file.seek(io::SeekFrom::Start(0))
        .await
        .map_err(|cause| invalid(path, cause.to_string()))?;
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = vec![0u8; 65536];
    loop {
        checkpoint(cancel, path).await?;
        let n = file
            .read(&mut buffer)
            .await
            .map_err(|cause| invalid(path, cause.to_string()))?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        count = count
            .checked_add(n as u64)
            .ok_or_else(|| invalid(path, "Chunk byte count overflow."))?;
    }
    Ok((count, hash.finalize().into()))
}
async fn source_digest(
    source: &Source,
    cancel: &watch::Receiver<bool>,
) -> Result<(u64, [u8; 32]), AppError> {
    source.verify()?;
    let file = tokio::fs::File::open(&source.path)
        .await
        .map_err(|cause| invalid(&source.path, cause.to_string()))?;
    let result = digest(file, &source.path, cancel).await?;
    source.verify()?;
    Ok(result)
}
async fn temporary_digest(
    temporary: &Temporary,
    cancel: &watch::Receiver<bool>,
) -> Result<(u64, [u8; 32]), AppError> {
    temporary.flush_nonempty_async().await?;
    digest(
        tokio::fs::File::from_std(temporary.clone_file()?),
        &temporary.path,
        cancel,
    )
    .await
}
fn reserve(joined: &Temporary, label: &str, extension: &str) -> Result<Temporary, AppError> {
    Temporary::create_extension(
        &joined.path,
        &format!(
            "source-queue-proof-{}-{}-{label}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
        extension,
    )
}
fn native_path(path: &Path) -> Result<String, AppError> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid(path, "The concat proof needs a UTF-8 owned path."))?;
    #[cfg(windows)]
    let text = if let Some(rest) = text.strip_prefix(r"\\?\") {
        if let Some(rest) = rest.strip_prefix("UNC") {
            format!("\\{rest}")
        } else {
            rest.to_owned()
        }
    } else {
        text.to_owned()
    };
    Ok(text.to_string())
}
async fn write(
    temporary: &Temporary,
    bytes: &[u8],
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    checkpoint(cancel, &temporary.path).await?;
    let mut file = tokio::fs::File::from_std(temporary.clone_file()?);
    file.write_all(bytes)
        .await
        .map_err(|cause| invalid(&temporary.path, cause.to_string()))?;
    file.flush()
        .await
        .map_err(|cause| invalid(&temporary.path, cause.to_string()))?;
    drop(file);
    temporary.flush_nonempty_async().await
}
async fn copy(
    spec: &CommandSpec,
    output: &mut Temporary,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    checkpoint(cancel, &output.path).await?;
    output.close_for_path_writer()?;
    let result = supervisor::run_streaming_stdout(spec, cancel.clone(), 65536, LIMIT, |reader| {
        io::copy(reader, &mut io::sink()).map_err(|cause| cause.to_string())
    })
    .await;
    // Rebind even after cancellation so exact owned paths are cleaned. A
    // replaced inode is retained rather than deleting somebody else's file.
    let rebound = output.reopen_after_path_writer();
    let result = result.map_err(|cause| process_error(cause, &output.path))?;
    rebound?;
    if !result.status.success() {
        return Err(invalid(
            &output.path,
            format!(
                "The source-compatible concat reference failed ({}): {}",
                result.status,
                String::from_utf8_lossy(&result.stderr)
            ),
        ));
    }
    output.flush_nonempty_async().await?;
    checkpoint(cancel, &output.path).await
}
async fn mkvmerge(
    executable: &Path,
    paths: &[PathBuf],
    rate: (u32, u32),
    force_rate: bool,
    output: &mut Temporary,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let mut args = vec!["-o".to_owned(), native_path(&output.path)?];
    if force_rate {
        args.extend([
            "--default-duration".into(),
            format!("0:{}/{}fps", rate.0, rate.1),
        ]);
    }
    args.push("[".into());
    for path in paths {
        args.push(native_path(path)?);
    }
    args.push("]".into());
    let options = reserve(output, "options", "json")?;
    write(
        &options,
        &serde_json::to_vec(&args).map_err(|cause| invalid(&output.path, cause.to_string()))?,
        cancel,
    )
    .await?;
    let identity = temporary_digest(&options, cancel).await?;
    copy(
        &CommandSpec {
            executable: executable.to_owned(),
            args: vec![format!("@{}", native_path(&options.path)?).into()],
            cwd: None,
        },
        output,
        cancel,
    )
    .await?;
    if temporary_digest(&options, cancel).await? != identity {
        return Err(invalid(
            &options.path,
            "The owned concat options changed during reference work.",
        ));
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
async fn reference(
    ffmpeg: &Path,
    mkvmerge_tool: Option<&Path>,
    joined: &Temporary,
    chunks: &[SealedChunk],
    method: Av1anConcatMethod,
    rate: (u32, u32),
    force_rate: bool,
    format: &str,
    cancel: &watch::Receiver<bool>,
) -> Result<Temporary, AppError> {
    #[cfg(test)]
    super::tests::REFERENCE_JOIN_CALLS.with(|calls| calls.set(calls.get() + 1));
    let mut output = reserve(
        joined,
        "reference",
        if format == "ivf" { "ivf" } else { "mkv" },
    )?;
    let paths = chunks
        .iter()
        .map(|chunk| chunk.path.clone())
        .collect::<Vec<_>>();
    match method {
        Av1anConcatMethod::Ffmpeg => {
            let manifest = reserve(joined, "manifest", "concat")?;
            let mut text = String::new();
            for path in &paths {
                checkpoint(cancel, path).await?;
                let path = native_path(path)?;
                if path.contains(['\r', '\n', '\0']) {
                    return Err(invalid(
                        joined.path.as_path(),
                        "Concat chunk paths cannot contain line breaks or NUL.",
                    ));
                }
                text.push_str(&format!(
                    "file {}\n",
                    path.replace('\\', r"\\")
                        .replace(' ', r"\ ")
                        .replace('\'', r"\'")
                ));
            }
            write(&manifest, text.as_bytes(), cancel).await?;
            let identity = temporary_digest(&manifest, cancel).await?;
            let args = [
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-protocol_whitelist",
                "file",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
            ]
            .into_iter()
            .map(OsString::from)
            .chain([manifest.path.as_os_str().to_owned()])
            .chain(["-map", "0", "-c", "copy"].into_iter().map(Into::into))
            .chain([output.path.as_os_str().to_owned()])
            .collect();
            copy(
                &CommandSpec {
                    executable: ffmpeg.to_owned(),
                    args,
                    cwd: None,
                },
                &mut output,
                cancel,
            )
            .await?;
            if temporary_digest(&manifest, cancel).await? != identity {
                return Err(invalid(
                    &manifest.path,
                    "The owned concat list changed during reference work.",
                ));
            }
        }
        Av1anConcatMethod::Mkvmerge => {
            let executable = mkvmerge_tool.ok_or_else(|| {
                invalid(
                    &joined.path,
                    "mkvmerge is required to verify this source queue's concat method.",
                )
            })?;
            #[cfg(not(windows))]
            let mut groups = Vec::new();
            #[cfg(not(windows))]
            let paths = if paths.len() > GROUP {
                for group in paths.chunks(GROUP) {
                    let mut part = reserve(joined, "group", "mkv")?;
                    mkvmerge(executable, group, rate, force_rate, &mut part, cancel).await?;
                    groups.push(part);
                }
                groups
                    .iter()
                    .map(|group| group.path.clone())
                    .collect::<Vec<_>>()
            } else {
                paths
            };
            mkvmerge(executable, &paths, rate, force_rate, &mut output, cancel).await?;
        }
    }
    Ok(output)
}
pub(super) fn only_video(document: &Document, path: &Path) -> Result<Stream, AppError> {
    if document.streams.len() != 1
        || document.streams[0].index != 0
        || document.streams[0].codec_type.as_deref() != Some("video")
    {
        return Err(invalid(
            path,
            "A source concat stage must contain exactly its one encoded video stream.",
        ));
    }
    Ok(document.streams[0].clone())
}
pub(super) fn joined_format<'a>(
    document: &'a Document,
    path: &Path,
    method: Av1anConcatMethod,
) -> Result<&'a str, AppError> {
    let format = document
        .format
        .as_ref()
        .and_then(|format| format.format_name.as_deref())
        .ok_or_else(|| {
            invalid(
                path,
                "The joined source stage has no container declaration.",
            )
        })?;
    if !matches!(format, "ivf" | "matroska,webm")
        || (method == Av1anConcatMethod::Mkvmerge && format != "matroska,webm")
    {
        return Err(invalid(
            path,
            "The source concat stage uses an unsupported method/container pair.",
        ));
    }
    Ok(format)
}
fn headers(expected: &Stream, actual: &Stream, path: &Path) -> Result<(), AppError> {
    if expected.codec_name != actual.codec_name
        || expected.codec_tag_string != actual.codec_tag_string
        || expected.extradata_hash != actual.extradata_hash
        || expected.width != actual.width
        || expected.height != actual.height
        || expected.pix_fmt != actual.pix_fmt
        || expected.sample_aspect_ratio != actual.sample_aspect_ratio
        || expected.color_primaries != actual.color_primaries
        || expected.color_transfer != actual.color_transfer
        || expected.color_space != actual.color_space
        || expected.color_range != actual.color_range
        || expected.chroma_location != actual.chroma_location
        || expected.field_order != actual.field_order
        || expected.time_base != actual.time_base
        || expected.avg_frame_rate != actual.avg_frame_rate
        || expected.r_frame_rate != actual.r_frame_rate
        || expected.side_data_list != actual.side_data_list
    {
        return Err(invalid(
            path,
            "The joined codec/header/color/clock declarations differ from the sealed-chunk concat reference.",
        ));
    }
    Ok(())
}
async fn duration_presence(
    ffprobe: &Path,
    path: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<(u64, [u8; 32]), AppError> {
    let result = supervisor::run_streaming_stdout(
        &mux::packet_spec(ffprobe, path, 0),
        cancel.clone(),
        65536,
        LIMIT,
        |reader| {
            let mut hash = Sha256::new();
            let count = mux::packets(reader, |packet| {
                if packet.pts.is_none() {
                    return Err("A joined source video packet has no presentation clock.".into());
                }
                hash.update([u8::from(packet.duration.is_some())]);
                Ok(())
            })?;
            if count == 0 {
                return Err("A joined source video has no encoded packets.".into());
            }
            Ok((count, hash.finalize().into()))
        },
    )
    .await
    .map_err(|cause| process_error(cause, path))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(invalid(
            path,
            "The complete concat duration-presence scan failed.",
        ));
    }
    Ok(result.value)
}

/// `method`, `rate` and `force_rate` are the actual engine invocation's policy;
/// neither source ordinals nor observed raw-filter counts can redefine them.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(in crate::jobs) async fn verify(
    ffmpeg: &Path,
    ffprobe: &Path,
    mkvmerge: Option<&Path>,
    joined: &Temporary,
    chunks: &[SealedChunk],
    method: Av1anConcatMethod,
    rate: (u32, u32),
    force_rate: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<Stream, AppError> {
    supervisor::with_console_profile(ConsoleProfile::Hidden, || {
        verify_inner(
            ffmpeg, ffprobe, mkvmerge, joined, chunks, method, rate, force_rate, cancel, true,
        )
    })
    .await
    .map(|(header, _)| header)
}
/// Decode only the independently copied reference; no encoder/filter is replayed.
#[allow(clippy::too_many_arguments)]
pub(in crate::jobs) async fn verify_with_clock(
    ffmpeg: &Path,
    ffprobe: &Path,
    mkvmerge: Option<&Path>,
    joined: &Temporary,
    chunks: &[SealedChunk],
    method: Av1anConcatMethod,
    rate: (u32, u32),
    force_rate: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<(Stream, Clock), AppError> {
    supervisor::with_console_profile(ConsoleProfile::Hidden, || {
        verify_inner(
            ffmpeg, ffprobe, mkvmerge, joined, chunks, method, rate, force_rate, cancel, true,
        )
    })
    .await
    .map(|(header, clock)| (header, clock.expect("requested decoded reference clock")))
}
#[allow(clippy::too_many_arguments)]
async fn verify_inner(
    ffmpeg: &Path,
    ffprobe: &Path,
    mkvmerge: Option<&Path>,
    joined: &Temporary,
    chunks: &[SealedChunk],
    method: Av1anConcatMethod,
    rate: (u32, u32),
    force_rate: bool,
    cancel: &watch::Receiver<bool>,
    with_clock: bool,
) -> Result<(Stream, Option<Clock>), AppError> {
    checkpoint(cancel, &joined.path).await?;
    if chunks.is_empty()
        || rate.0 == 0
        || rate.1 == 0
        || chunks
            .iter()
            .any(|chunk| chunk.bytes == 0 || chunk.source_frames == 0 || chunk.decoded_frames == 0)
    {
        return Err(invalid(
            &joined.path,
            "The concat proof requires a complete nonempty sealed chunk queue and explicit engine rate.",
        ));
    }
    let mut sources = Vec::with_capacity(chunks.len());
    let mut names = HashSet::new();
    for chunk in chunks {
        checkpoint(cancel, &chunk.path).await?;
        let source = Source::open(&chunk.path)?;
        if !names.insert(source.path.clone()) {
            return Err(invalid(
                &chunk.path,
                "A sealed queue repeats an encoded chunk path.",
            ));
        }
        if source_digest(&source, cancel).await? != (chunk.bytes, chunk.sha256) {
            return Err(invalid(
                &chunk.path,
                "An encoded chunk differs from its complete sealed byte receipt.",
            ));
        }
        sources.push(source);
    }
    let identity = temporary_digest(joined, cancel).await?;
    let actual_document = probe(ffprobe, &joined.path, cancel, Some(&[0])).await?;
    let actual = only_video(&actual_document, &joined.path)?;
    let format = joined_format(&actual_document, &joined.path, method)?;
    let reference = reference(
        ffmpeg, mkvmerge, joined, chunks, method, rate, force_rate, format, cancel,
    )
    .await?;
    let reference_identity = temporary_digest(&reference, cancel).await?;
    let expected = only_video(
        &probe(ffprobe, &reference.path, cancel, Some(&[0])).await?,
        &reference.path,
    )?;
    headers(&expected, &actual, &joined.path)?;
    mux::verify_shifted_video_packets(ffprobe, &reference.path, 0, &joined.path, 0, 0., cancel)
        .await?;
    // The shared packet proof permits known absent-duration representations.
    // A same-container concat must additionally preserve their exact presence.
    if duration_presence(ffprobe, &reference.path, cancel).await?
        != duration_presence(ffprobe, &joined.path, cancel).await?
    {
        return Err(invalid(
            &joined.path,
            "The complete concat packet duration declarations differ.",
        ));
    }
    let clock = if with_clock {
        Some(decoded_clock(ffprobe, &reference.path, &expected, cancel).await?)
    } else {
        None
    };
    for (source, chunk) in sources.iter().zip(chunks) {
        if source_digest(source, cancel).await? != (chunk.bytes, chunk.sha256) {
            return Err(invalid(
                &chunk.path,
                "A sealed chunk changed during concat proof.",
            ));
        }
    }
    if temporary_digest(joined, cancel).await? != identity
        || temporary_digest(&reference, cancel).await? != reference_identity
    {
        return Err(invalid(
            &joined.path,
            "An owned concat artifact changed during packet proof.",
        ));
    }
    checkpoint(cancel, &joined.path).await?;
    Ok((expected, clock))
}
async fn decoded_clock(
    ffprobe: &Path,
    path: &Path,
    header: &Stream,
    cancel: &watch::Receiver<bool>,
) -> Result<Clock, AppError> {
    let time_base = header
        .time_base
        .as_deref()
        .and_then(|text| text.split_once('/'))
        .and_then(|(n, d)| Some((n.parse::<u32>().ok()?, d.parse::<u32>().ok()?)))
        .filter(|(n, d)| *n > 0 && *d > 0)
        .ok_or_else(|| {
            invalid(
                path,
                "The concat reference has no positive rational stream timebase.",
            )
        })?;
    #[derive(Deserialize)]
    struct Frame {
        best_effort_timestamp: Option<i64>,
    }
    let spec = CommandSpec {
        executable: ffprobe.to_owned(),
        args: [
            "-v",
            "error",
            "-err_detect",
            "explode",
            "-select_streams",
            "0",
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp",
            "-of",
            "json",
            "-i",
        ]
        .into_iter()
        .map(OsString::from)
        .chain([path.as_os_str().to_owned()])
        .collect(),
        cwd: None,
    };
    let result =
        supervisor::run_streaming_stdout(&spec, cancel.clone(), 65536, LIMIT, move |reader| {
            let mut first = None;
            let mut previous = None;
            let mut frames = 0u64;
            let mut digest = [0u8; 32];
            let mut error = None;
            crate::jobs::encode::frame_scan::parse(reader, |frame: Frame| {
                let mut consume = || -> Result<(), String> {
                    let tick = frame
                        .best_effort_timestamp
                        .ok_or("A decoded reference frame has no timestamp.")?;
                    let seconds = tick as f64 * f64::from(time_base.0) / f64::from(time_base.1);
                    if !seconds.is_finite()
                        || seconds.abs() > 1_000_000_000.
                        || previous.is_some_and(|value| tick <= value)
                    {
                        return Err(
                            "The decoded reference clock must be finite and strictly increasing."
                                .into(),
                        );
                    }
                    let origin = *first.get_or_insert(tick);
                    let relative = tick
                        .checked_sub(origin)
                        .ok_or("The decoded reference clock overflowed.")?;
                    let mut hash = Sha256::new();
                    hash.update(digest);
                    hash.update(relative.to_le_bytes());
                    digest = hash.finalize().into();
                    previous = Some(tick);
                    frames = frames
                        .checked_add(1)
                        .ok_or("The decoded reference count overflowed.")?;
                    Ok(())
                };
                if error.is_none() {
                    error = consume().err();
                }
            })?;
            if let Some(error) = error {
                return Err(error);
            }
            if frames == 0 {
                return Err("The concat reference decoded no frames.".into());
            }
            Ok(Clock {
                time_base,
                origin_ticks: first.expect("decoded nonempty clock"),
                frames,
                relative_pts_sha256: digest,
            })
        })
        .await
        .map_err(|cause| process_error(cause, path))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(invalid(
            path,
            "The complete reference decoded-clock proof failed.",
        ));
    }
    Ok(result.value)
}
#[cfg(test)]
mod tests;
