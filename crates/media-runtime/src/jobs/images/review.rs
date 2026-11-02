//! A confirmed image program writes a guarded encoded spool. Sequence expansion
//! stream-copies its packets into precreated identity-guarded files afterward.
use super::*;
use crate::jobs::{
    command_edit,
    metadata::{Document, Stream},
};
use media_core::ImageProcessingSettings;
use sha2::{Digest, Sha256};

#[allow(clippy::too_many_arguments)]
pub(super) async fn export(
    session: Option<&super::super::command_edit::Session>,
    source: &Source,
    document: &Document,
    video: &Stream,
    stream: u32,
    processing: &ImageProcessingSettings,
    burn: &subtitle::Burn,
    metadata: &super::metadata::Prepared,
    ffmpeg: &Path,
    ffprobe: &Path,
    output: &Path,
    format: ImageOutput,
    layout: ImagePixelFormat,
    quality: u16,
    sequence: bool,
    source_names: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<ImageResult>, AppError> {
    let mut qtgmc = if processing
        .temporal
        .is_some_and(|value| value.qtgmc.is_some())
    {
        Some(
            plan_qtgmc_image(
                &burn.prepared,
                source,
                document,
                stream,
                processing,
                ffmpeg,
                ffprobe,
                output,
                sequence,
                cancel,
                session.is_some(),
            )
            .await?,
        )
    } else {
        None
    };
    let neutral = subtitle::Burn::empty();
    let (picture, picture_stream, renderer, plan) = if let Some((prepared, plan, _)) = &qtgmc {
        (
            prepared.video.path.as_path(),
            0,
            &neutral,
            crate::jobs::encode_plan::ImageFilterPlan {
                filter: Some("setpts=PTS-STARTPTS".into()),
                prefix: "setpts=PTS-STARTPTS".into(),
                suffix: String::new(),
                device_args: Vec::new(),
                input_seek: None,
                output_trim: Vec::new(),
                source_clock: false,
                width: plan.width,
                height: plan.height,
            },
        )
    } else {
        (
            source.path.as_path(),
            stream,
            burn,
            crate::jobs::encode_plan::image_filter_plan(
                video,
                processing,
                burn.prepared.text_filter(),
                ffmpeg,
                &source.path,
                cancel,
                session.is_some(),
            )
            .await?,
        )
    };
    let extension = match format {
        ImageOutput::Png | ImageOutput::PngSequence => "png",
        ImageOutput::Jpeg | ImageOutput::JpegSequence => "jpg",
        ImageOutput::Gif => "gif",
    };
    let mut spool =
        Temporary::create_extension(output, &format!("{}-review-spool", id()), extension)?;
    let mut reference =
        Temporary::create_extension(output, &format!("{}-review-reference", id()), "framemd5")?;
    let mut palette = if format == ImageOutput::Gif && !processing.source_filters {
        Some(Temporary::create_extension(
            output,
            &format!("{}-review-palette", id()),
            "png",
        )?)
    } else {
        None
    };
    private_file(&spool)?;
    private_file(&reference)?;
    if let Some(palette) = &palette {
        private_file(palette)?;
    }
    let mut program = Vec::new();
    if let Some((prepared, _, _)) = &qtgmc {
        program.push(format!(
            "{} | {} > {}",
            command_edit::format_command(&prepared.producer),
            command_edit::format_command(&prepared.consumer),
            command_edit::quote(&prepared.video.path.to_string_lossy())
        ));
    }
    if !processing.source_filters {
        let mut reference_args = renderer.decoder(picture, picture_stream, &plan, None, None);
        reference_args.extend(args(&[
            "-fps_mode",
            "passthrough",
            "-c:v",
            "rawvideo",
            "-pix_fmt",
            layout.ffmpeg(),
            "-threads",
            "2",
            "-f",
            "framemd5",
        ]));
        reference_args.push(reference.path.as_os_str().to_owned());
        program.push(command_edit::format_command(&CommandSpec {
            executable: ffmpeg.to_owned(),
            args: reference_args,
            cwd: renderer.cwd(),
        }));
    }
    if let Some(palette) = &palette {
        let mut command = renderer.decoder(picture, picture_stream, &plan, None, Some(quality));
        command.extend(args(&["-frames:v", "1", "-update", "1", "-threads", "2"]));
        command.push(palette.path.as_os_str().to_owned());
        program.push(command_edit::format_command(&CommandSpec {
            executable: ffmpeg.to_owned(),
            args: command,
            cwd: renderer.cwd(),
        }));
    }
    let mut command = renderer.decoder(
        picture,
        picture_stream,
        &plan,
        palette.as_ref().map(|value| value.path.as_path()),
        None,
    );
    if processing.source_filters && format == ImageOutput::Gif {
        // Source appends the forced split/palette graph after custom rows.
        // One graph executes the arbitrary user filter once, not once for
        // reference sampling, again for the palette and again for pixels.
        let index = command
            .iter()
            .position(|arg| arg == "-filter_complex")
            .expect("image graph");
        let graph = command[index + 1].to_str().expect("generated UTF-8 graph");
        let prefix = graph
            .strip_suffix("[jesses_image]")
            .expect("image output label");
        command[index+1]=format!("{prefix},split[jesses_palette_source][jesses_pixels];[jesses_palette_source]palettegen=max_colors={quality}[jesses_palette];[jesses_pixels][jesses_palette]paletteuse=dither=floyd_steinberg[jesses_image]").into();
    }
    metadata.append(&mut command)?;
    if format == ImageOutput::Gif {
        command.extend(args(&[
            "-loop",
            "0",
            "-gifflags",
            "-offsetting",
            "-f",
            "gif",
        ]));
    } else {
        command.extend(args(&[
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            layout.ffmpeg(),
        ]));
        if matches!(format, ImageOutput::Jpeg | ImageOutput::JpegSequence) {
            command.extend(args(&[
                "-c:v",
                "mjpeg",
                "-qmin",
                "1",
                "-q:v",
                &quality.to_string(),
            ]));
        } else {
            command.extend(args(&["-c:v", "png", "-compression_level", "3"]));
        }
        command.extend(args(&["-f", "image2pipe"]));
    }
    command.extend(args(&["-an", "-sn", "-threads", "2"]));
    command.push(spool.path.as_os_str().to_owned());
    let render_command = CommandSpec {
        executable: ffmpeg.to_owned(),
        args: command,
        cwd: renderer.cwd(),
    };
    program.push(command_edit::format_command(&render_command));
    let generated = program.join(" &&\n");
    let confirmed = if let Some(session) = session {
        Some(session.ask("ffmpeg", &generated, output, cancel).await?)
    } else {
        None
    };
    source.verify()?;
    burn.verify()?;
    metadata.verify()?;
    check(cancel)?;
    if let Some(session) = session {
        session.acknowledge(Ok(()));
    }
    let source_native = confirmed
        .as_ref()
        .is_none_or(|value| value.arguments == value.generated.trim());
    if source_native && !processing.source_filters {
        return Ok(None);
    }

    let mut log = Temporary::create_extension(output, &format!("{}-review-log", id()), "log")?;
    private_file(&log)?;
    spool.close_for_path_writer()?;
    reference.close_for_path_writer()?;
    log.close_for_path_writer()?;
    if let Some(palette) = &mut palette {
        palette.close_for_path_writer()?;
    }
    if !source_native && let Some((prepared, _, _)) = &mut qtgmc {
        prepared.video.close_for_path_writer()?;
    }
    let (sender, mut events) = tokio::sync::mpsc::channel(64);
    let observer = tokio::spawn(async move { while events.recv().await.is_some() {} });
    let shell = if source_native {
        None
    } else {
        Some(command_edit::PreparedShell::new(
            &confirmed.as_ref().expect("edited command").arguments,
            None,
            &log.path,
            qtgmc.as_ref().and_then(|value| value.0.environment()),
        )?)
    };
    let result = if let Some(shell) = &shell {
        crate::jobs::encode::boxed(|| {
            supervisor::with_console_profile(supervisor::ConsoleProfile::General, || {
                supervisor::run_with_environment(
                    &shell.command,
                    cancel.clone(),
                    sender,
                    &log.path,
                    Duration::from_secs(86400),
                    Some(&shell.environment),
                    None,
                )
            })
        })
        .await
    } else {
        // Unchanged source requests execute native argv directly. Only an
        // explicitly edited held program runs through the shell broker.
        if let Some((prepared, plan, _)) = &qtgmc {
            prepared
                .run(cancel.clone(), sender.clone(), &log.path)
                .await?;
            prepared
                .validate_source_filters(ffprobe, plan, cancel)
                .await?;
        }
        supervisor::run(
            &render_command,
            cancel.clone(),
            sender,
            &log.path,
            Duration::from_secs(86400),
        )
        .await
    };
    let _ = observer.await;
    let rebound = spool.reopen_after_path_writer();
    let _ = reference.reopen_after_path_writer();
    let _ = log.reopen_after_path_writer();
    if let Some(palette) = &mut palette {
        let _ = palette.reopen_after_path_writer();
    }
    if !source_native && let Some((prepared, _, _)) = &mut qtgmc {
        let _ = prepared.video.reopen_after_path_writer();
    }
    let result = result.map_err(|cause| process_error(cause, &source.path))?;
    rebound?;
    if !result.status.success() {
        use std::io::{Read, Seek, SeekFrom};
        let mut log = File::open(&log.path).map_err(|cause| error(cause.to_string(), output))?;
        let length = log
            .metadata()
            .map_err(|cause| error(cause.to_string(), output))?
            .len();
        let _ = log.seek(SeekFrom::Start(length.saturating_sub(4096)));
        let mut tail = String::new();
        let _ = log.read_to_string(&mut tail);
        return Err(error(
            format!(
                "The confirmed image program failed; no output was published. {}",
                tail.trim()
            ),
            output,
        ));
    }
    if let Some(shell) = &shell {
        shell.verify()?;
    }
    spool.flush_nonempty_async().await?;
    source.verify()?;
    burn.verify()?;
    metadata.verify()?;
    check(cancel)?;
    publish_spool(
        &mut spool,
        ffmpeg,
        ffprobe,
        output,
        format,
        sequence,
        source_names,
        cancel,
        || {
            source.verify()?;
            burn.verify()?;
            metadata.verify()
        },
    )
    .await
    .map(Some)
}

/// Validate every actual image and preserve the encoded packets when expanding.
#[allow(clippy::too_many_arguments)]
pub(super) async fn publish_spool(
    spool: &mut Temporary,
    ffmpeg: &Path,
    ffprobe: &Path,
    output: &Path,
    format: ImageOutput,
    sequence: bool,
    source_names: bool,
    cancel: &watch::Receiver<bool>,
    verify: impl Fn() -> Result<(), AppError>,
) -> Result<ImageResult, AppError> {
    let extension = match format {
        ImageOutput::Png | ImageOutput::PngSequence => "png",
        ImageOutput::Jpeg | ImageOutput::JpegSequence => "jpg",
        ImageOutput::Gif => "gif",
    };
    {
        use std::io::Read;
        let mut header = [0u8; 8];
        File::open(&spool.path)
            .and_then(|mut file| file.read_exact(&mut header))
            .map_err(|cause| error(cause.to_string(), output))?;
        let valid = match format {
            ImageOutput::Png | ImageOutput::PngSequence => header == *b"\x89PNG\r\n\x1a\n",
            ImageOutput::Jpeg | ImageOutput::JpegSequence => header[..2] == [0xff, 0xd8],
            ImageOutput::Gif => matches!(&header[..6], b"GIF87a" | b"GIF89a"),
        };
        if !valid {
            return Err(error(
                "The edited spool does not match the selected image format.",
                output,
            ));
        }
    }
    let actual = actual_frames(ffprobe, &spool.path, format, cancel).await?;
    if !sequence && format != ImageOutput::Gif && actual.count != 1 {
        return Err(error(
            "A still image must contain exactly one decoded frame.",
            output,
        ));
    }
    image_preflight(output, actual.count, actual.dimensions, sequence)?;
    let packets = if sequence {
        let packets = packet_hash(ffprobe, &spool.path, true, cancel).await?;
        if packets.count != actual.count
            || packets.bytes
                != fs::metadata(&spool.path)
                    .map_err(|cause| error(cause.to_string(), output))?
                    .len()
        {
            return Err(error(
                "The complete image spool bytes do not match its decoded image packets.",
                output,
            ));
        }
        Some(packets)
    } else {
        None
    };
    if sequence {
        let required = fs::metadata(&spool.path)
            .map_err(|cause| error(cause.to_string(), output))?
            .len()
            .saturating_add(u64::from(actual.count).saturating_mul(65536))
            .saturating_add(16 * 1024 * 1024);
        let available = crate::jobs::source_normalization::available(
            output.parent().expect("validated parent"),
        )?;
        if required > available {
            return Err(error(
                "There is not enough temporary disk space to expand the edited image spool without re-encoding.",
                output,
            ));
        }
        let mut stage = Directory::new(output.parent().expect("validated parent"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            stage
                .identity_guard
                .as_ref()
                .expect("held directory")
                .set_permissions(fs::Permissions::from_mode(0o700))
                .map_err(|cause| error(cause.to_string(), &stage.path))?;
        }
        let pattern = stage.path.join(frame_pattern(extension, source_names));
        for index in 1..=actual.count {
            pause_checkpoint(cancel).await?;
            drop(stage.create_file(stage.path.join(frame_name(index, extension, source_names)))?);
        }
        let mut copy = args(&["-v", "error", "-nostdin", "-y", "-f", "image2pipe", "-i"]);
        copy.push(spool.path.as_os_str().to_owned());
        copy.extend(args(&[
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-fps_mode",
            "passthrough",
            "-f",
            "image2",
            "-start_number",
            "1",
        ]));
        copy.push(pattern.as_os_str().to_owned());
        run(ffmpeg, copy, None, cancel).await?;
        stage.record_files()?;
        verify_frames(
            ffprobe,
            &pattern,
            true,
            actual.count,
            actual.dimensions,
            None,
            cancel,
        )
        .await?;
        if Some(packet_hash(ffprobe, &pattern, false, cancel).await?) != packets {
            return Err(error(
                "Image packet bytes changed during lossless sequence extraction.",
                output,
            ));
        }
        pause_checkpoint(cancel).await?;
        verify()?;
        check(cancel)?;
        stage.publish(output)?;
    } else {
        pause_checkpoint(cancel).await?;
        verify()?;
        check(cancel)?;
        spool.publish(output)?;
    }
    Ok(ImageResult{output_path:output.to_string_lossy().into_owned(),frame_count:actual.count,width:actual.dimensions.0,height:actual.dimensions.1,notes:vec!["The image program was fully decoded before publication. Sequence packets are copied without re-encoding; spooled exports temporarily retain an encoded spool beside their final stage.".into()]})
}

pub(super) fn private_file(file: &Temporary) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.clone_file()?
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|cause| error(cause.to_string(), &file.path))?;
    }
    let _ = file;
    Ok(())
}

struct Actual {
    count: u32,
    dimensions: (u32, u32),
}
async fn actual_frames(
    probe: &Path,
    input: &Path,
    format: ImageOutput,
    cancel: &watch::Receiver<bool>,
) -> Result<Actual, AppError> {
    #[derive(Deserialize)]
    struct Frame {
        width: u32,
        height: u32,
        pix_fmt: String,
    }
    let mut a = args(&["-v", "error", "-err_detect", "explode"]);
    if format != ImageOutput::Gif {
        a.extend(args(&["-f", "image2pipe"]));
    }
    a.extend(args(&[
        "-select_streams",
        "v:0",
        "-show_frames",
        "-show_entries",
        "frame=width,height,pix_fmt",
        "-of",
        "json",
        "-i",
    ]));
    a.push(input.as_os_str().to_owned());
    let result=supervisor::run_streaming_stdout(&CommandSpec{executable:probe.to_owned(),args:a,cwd:None},cancel.clone(),65536,Duration::from_secs(86400),move|reader|{
        let mut count=0u32;let mut dimensions=None;let mut pixel=None;let mut invalid=false;
        crate::jobs::encode::frame_scan::parse(reader,|frame:Frame|{
            invalid|=!(1..=16384).contains(&frame.width)||!(1..=16384).contains(&frame.height)||dimensions.is_some_and(|value|value!=(frame.width,frame.height));
            invalid|=frame.pix_fmt.is_empty()||frame.pix_fmt=="unknown"||pixel.as_ref().is_some_and(|value|value!=&frame.pix_fmt);
            pixel.get_or_insert(frame.pix_fmt);
            dimensions.get_or_insert((frame.width,frame.height));
            if let Some(next)=count.checked_add(1){count=next;}else{invalid=true;}
        })?;
        if invalid||count==0{return Err("The edited images have invalid or changing geometry/layout, or no decoded frames.".into());}
        Ok(Actual{count,dimensions:dimensions.unwrap()})
    }).await.map_err(|cause|process_error(cause,input))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(error(
            "The complete edited image spool could not be decoded.",
            input,
        ));
    }
    check(cancel)?;
    Ok(result.value)
}

#[derive(Debug, PartialEq, Eq)]
struct Packets {
    count: u32,
    bytes: u64,
    digest: String,
}
async fn packet_hash(
    probe: &Path,
    input: &Path,
    pipe: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<Packets, AppError> {
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        if pipe { "image2pipe" } else { "image2" },
    ]);
    a.extend(args(&[
        "-show_packets",
        "-show_data_hash",
        "sha256",
        "-show_entries",
        "packet=size,data_hash",
        "-of",
        "compact=p=0",
        "-i",
    ]));
    a.push(input.as_os_str().to_owned());
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: probe.to_owned(),
            args: a,
            cwd: None,
        },
        cancel.clone(),
        65536,
        Duration::from_secs(86400),
        |reader| {
            use std::io::{BufRead, Read};
            let mut reader = std::io::BufReader::new(reader);
            let mut line = Vec::new();
            let mut hash = Sha256::new();
            let mut count = 0u32;
            let mut total = 0u64;
            loop {
                line.clear();
                let bytes = (&mut reader)
                    .take(1025)
                    .read_until(b'\n', &mut line)
                    .map_err(|cause| cause.to_string())?;
                if bytes == 0 {
                    break;
                }
                if bytes > 1024 {
                    return Err("An image packet hash record is too long.".into());
                }
                let text = std::str::from_utf8(&line)
                    .map_err(|cause| cause.to_string())?
                    .trim();
                if text.is_empty() {
                    continue;
                }
                let mut size = None;
                let mut digest = None;
                for field in text.split('|') {
                    if let Some(value) = field.strip_prefix("size=") {
                        size = value.parse::<u64>().ok();
                    }
                    if let Some(value) = field.strip_prefix("data_hash=SHA256:") {
                        digest = Some(value);
                    }
                }
                let size = size
                    .filter(|value| *value > 0)
                    .ok_or("Missing image packet size.")?;
                let digest = digest
                    .filter(|value| {
                        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                    .ok_or("Missing image packet hash.")?;
                hash.update(size.to_le_bytes());
                hash.update(digest.as_bytes());
                count = count.checked_add(1).ok_or("Image packet count overflow.")?;
                total = total
                    .checked_add(size)
                    .ok_or("Image byte count overflow.")?;
            }
            if count == 0 {
                return Err("No image packets were found.".into());
            }
            Ok(Packets {
                count,
                bytes: total,
                digest: format!("{:x}", hash.finalize()),
            })
        },
    )
    .await
    .map_err(|cause| process_error(cause, input))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(error("Cannot verify every encoded image packet.", input));
    }
    check(cancel)?;
    Ok(result.value)
}
