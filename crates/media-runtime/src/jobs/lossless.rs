//! Complete decoded-pixel verification for encoders that claim lossless output.

use crate::supervisor::{self, CommandSpec, SupervisorError};
use media_core::AppError;
use sha2::{Digest, Sha256};
use std::{ffi::OsString, time::Duration};
use tokio::sync::watch;

const TIME_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);
const STDERR_LIMIT: usize = 64 * 1024;
const READ_BYTES: usize = 64 * 1024;

fn failed(message: impl Into<String>, command: Option<&CommandSpec>) -> AppError {
    AppError::new(
        "LOSSLESS_VALIDATION_FAILED",
        message,
        command.map(|spec| spec.executable.to_string_lossy().into_owned()),
    )
}

fn canceled() -> AppError {
    AppError::new(
        "JOB_CANCELED",
        "Lossless pixel verification was canceled. No output was published.",
        None,
    )
}

fn expected_bytes(width: u32, height: u32, bit_depth: u8, frames: usize) -> Result<u64, AppError> {
    if width == 0
        || height == 0
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || frames == 0
        || !(1..=16).contains(&bit_depth)
    {
        return Err(failed(
            "Lossless verification requires positive even 4:2:0 dimensions, one or more frames, and a bit depth from 1 through 16.",
            None,
        ));
    }
    let luma = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| failed("Lossless verification dimensions overflow.", None))?;
    let samples = luma
        .checked_add(luma / 2)
        .ok_or_else(|| failed("Lossless verification sample count overflow.", None))?;
    let bytes_per_sample = u64::from(bit_depth.div_ceil(8));
    let frame_bytes = samples
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| failed("Lossless verification frame size overflow.", None))?;
    frame_bytes
        .checked_mul(
            u64::try_from(frames)
                .map_err(|_| failed("Lossless verification frame count overflow.", None))?,
        )
        .ok_or_else(|| failed("Lossless verification byte count overflow.", None))
}

fn rawvideo(spec: &CommandSpec) -> Result<CommandSpec, AppError> {
    let format = spec
        .args
        .windows(2)
        .rposition(|args| args[0] == "-f" && args[1] == "yuv4mpegpipe")
        .ok_or_else(|| {
            failed(
                "The lossless decoder command does not end in a Y4M output.",
                Some(spec),
            )
        })?;
    if spec.args[format + 2..].iter().any(|arg| arg == "-f")
        || spec.args.last().is_none_or(|arg| arg != "pipe:1")
    {
        return Err(failed(
            "The lossless decoder command has an unexpected output layout.",
            Some(spec),
        ));
    }
    let mut raw = spec.clone();
    raw.args[format + 1] = OsString::from("rawvideo");
    raw.args.splice(
        format..format,
        [OsString::from("-c:v"), OsString::from("rawvideo")],
    );
    Ok(raw)
}

async fn pixel_digest(
    label: &'static str,
    spec: &CommandSpec,
    expected: u64,
    cancel: &watch::Receiver<bool>,
) -> Result<String, AppError> {
    let spec = rawvideo(spec)?;
    let result = supervisor::run_streaming_stdout(
        &spec,
        cancel.clone(),
        STDERR_LIMIT,
        TIME_LIMIT,
        move |reader| {
            let mut buffer = vec![0_u8; READ_BYTES];
            let mut hash = Sha256::new();
            let mut bytes = 0_u64;
            loop {
                let count = reader
                    .read(&mut buffer)
                    .map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                bytes = bytes
                    .checked_add(count as u64)
                    .ok_or("Decoded pixel byte count overflow.")?;
                if bytes > expected {
                    return Err(format!(
                        "The {label} decoded more than the expected {expected} pixel bytes."
                    ));
                }
                hash.update(&buffer[..count]);
            }
            if bytes != expected {
                return Err(format!(
                    "The {label} decoded {bytes} pixel bytes; expected exactly {expected}."
                ));
            }
            Ok(format!("{:x}", hash.finalize()))
        },
    )
    .await
    .map_err(|error| match error {
        SupervisorError::Cancelled => canceled(),
        error => failed(
            format!("The {label} could not be decoded completely: {error}"),
            Some(&spec),
        ),
    })?;
    if !result.status.success() {
        let detail = String::from_utf8_lossy(&result.stderr);
        return Err(failed(
            format!(
                "The {label} decoder exited unsuccessfully{}.",
                if detail.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", detail.trim())
                }
            ),
            Some(&spec),
        ));
    }
    if *cancel.borrow() {
        return Err(canceled());
    }
    Ok(result.value)
}

/// Decode the reference and candidate to tightly packed planar 4:2:0 bytes and
/// prove complete frame coverage and byte-for-byte pixel equality.
pub(super) async fn verify(
    reference: &CommandSpec,
    candidate: &CommandSpec,
    width: u32,
    height: u32,
    bit_depth: u8,
    frames: usize,
    cancel: &watch::Receiver<bool>,
) -> Result<String, AppError> {
    if *cancel.borrow() {
        return Err(canceled());
    }
    let expected = expected_bytes(width, height, bit_depth, frames)?;
    let reference_digest = pixel_digest("lossless reference", reference, expected, cancel).await?;
    let candidate_digest = pixel_digest("lossless candidate", candidate, expected, cancel).await?;
    if reference_digest != candidate_digest {
        return Err(failed(
            format!(
                "The candidate changed decoded pixels despite lossless mode (reference sha256:{reference_digest}, candidate sha256:{candidate_digest}). This encoder build and preset did not preserve every pixel; choose a different lossless-capable encoder or preset."
            ),
            Some(candidate),
        ));
    }
    Ok(format!(
        "sha256:{reference_digest};bytes:{expected};frames:{frames}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ffmpeg(cancel: &watch::Receiver<bool>) -> std::path::PathBuf {
        super::super::discover("ffmpeg", cancel).await.unwrap()
    }

    fn decoder(
        ffmpeg: &std::path::Path,
        filter: &str,
        frames: usize,
        realtime: bool,
    ) -> CommandSpec {
        let mut args = ["-v", "error", "-nostdin"]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        if realtime {
            args.push("-re".into());
        }
        args.extend(["-f", "lavfi", "-i"].into_iter().map(OsString::from));
        args.push(filter.into());
        args.push("-frames:v".into());
        args.push(frames.to_string().into());
        args.extend(
            [
                "-pix_fmt",
                "yuv420p10le",
                "-fps_mode",
                "passthrough",
                "-f",
                "yuv4mpegpipe",
                "pipe:1",
            ]
            .into_iter()
            .map(OsString::from),
        );
        CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: Some(std::env::temp_dir()),
        }
    }

    const SOURCE: &str = "testsrc2=s=128x72:r=24:d=1,format=yuv420p10le";

    #[test]
    fn rawvideo_rewrite_preserves_command_identity_and_working_directory() {
        let command = CommandSpec {
            executable: "ffmpeg-custom".into(),
            args: [
                "-i",
                "relative-source.mkv",
                "-map",
                "0:3",
                "-f",
                "yuv4mpegpipe",
                "pipe:1",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            cwd: Some(std::path::PathBuf::from("owned-working-directory")),
        };
        let raw = rawvideo(&command).unwrap();
        assert_eq!(raw.executable, command.executable);
        assert_eq!(raw.cwd, command.cwd);
        assert_eq!(
            raw.args,
            [
                "-i",
                "relative-source.mkv",
                "-map",
                "0:3",
                "-c:v",
                "rawvideo",
                "-f",
                "rawvideo",
                "pipe:1",
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg"]
    async fn identical_decoded_pixels_pass() {
        let (_sender, cancel) = watch::channel(false);
        let ffmpeg = ffmpeg(&cancel).await;
        let command = decoder(&ffmpeg, SOURCE, 4, false);
        let identity = verify(&command, &command, 128, 72, 10, 4, &cancel)
            .await
            .unwrap();
        assert!(identity.starts_with("sha256:"));
        assert!(identity.ends_with(";bytes:110592;frames:4"));
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg"]
    async fn one_altered_frame_is_rejected() {
        let (_sender, cancel) = watch::channel(false);
        let ffmpeg = ffmpeg(&cancel).await;
        let reference = decoder(&ffmpeg, SOURCE, 4, false);
        let altered = decoder(
            &ffmpeg,
            "testsrc2=s=128x72:r=24:d=1,format=yuv420p10le,drawbox=x=0:y=0:w=2:h=2:color=white:t=fill:enable=eq(n\\,1)",
            4,
            false,
        );
        let error = verify(&reference, &altered, 128, 72, 10, 4, &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.code, "LOSSLESS_VALIDATION_FAILED");
        assert!(error.message.contains("changed decoded pixels"));
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg"]
    async fn wrong_frame_count_is_rejected() {
        let (_sender, cancel) = watch::channel(false);
        let ffmpeg = ffmpeg(&cancel).await;
        let reference = decoder(&ffmpeg, SOURCE, 4, false);
        let short = decoder(&ffmpeg, SOURCE, 3, false);
        let error = verify(&reference, &short, 128, 72, 10, 4, &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.code, "LOSSLESS_VALIDATION_FAILED");
        assert!(error.message.contains("expected exactly 110592"));
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg"]
    async fn cancellation_stops_a_live_decode() {
        let (sender, cancel) = watch::channel(false);
        let ffmpeg = ffmpeg(&cancel).await;
        let command = decoder(
            &ffmpeg,
            "testsrc2=s=128x72:r=24:d=10,format=yuv420p10le",
            240,
            true,
        );
        let verification = verify(&command, &command, 128, 72, 10, 240, &cancel);
        tokio::pin!(verification);
        tokio::select! {
            result = &mut verification => panic!("verification ended before cancellation: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        sender.send_replace(true);
        let error = verification.await.unwrap_err();
        assert_eq!(error.code, "JOB_CANCELED");
    }
}
