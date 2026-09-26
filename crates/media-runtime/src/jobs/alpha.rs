//! Verify VP9 alpha from a forced libvpx decode before output publication.
//! FFmpeg's default VP9 decoder exposes the color planes but drops alpha.

use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use media_core::AppError;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::supervisor::{self, CommandSpec, SupervisorError};

const GRID: usize = 16;
const STDERR_LIMIT: usize = 64 * 1024;
const READ_BYTES: usize = 64 * 1024;
const TIME_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct FrameStats {
    sum: [u64; GRID * GRID],
    count: [u32; GRID * GRID],
    transparent: u64,
    partial: u64,
    opaque: u64,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self {
            sum: [0; GRID * GRID],
            count: [0; GRID * GRID],
            transparent: 0,
            partial: 0,
            opaque: 0,
        }
    }
}

impl FrameStats {
    fn write_to(&self, file: &mut File) -> std::io::Result<()> {
        let mut bytes = [0u8; 3096];
        let mut position = 0;
        for value in self.sum {
            bytes[position..position + 8].copy_from_slice(&value.to_le_bytes());
            position += 8;
        }
        for value in self.count {
            bytes[position..position + 4].copy_from_slice(&value.to_le_bytes());
            position += 4;
        }
        for value in [self.transparent, self.partial, self.opaque] {
            bytes[position..position + 8].copy_from_slice(&value.to_le_bytes());
            position += 8;
        }
        file.write_all(&bytes)
    }

    fn read_from(file: &mut File) -> std::io::Result<Self> {
        let mut bytes = [0u8; 3096];
        file.read_exact(&mut bytes)?;
        let mut position = 0;
        let mut current = Self::default();
        for value in &mut current.sum {
            *value = u64::from_le_bytes(bytes[position..position + 8].try_into().unwrap());
            position += 8;
        }
        for value in &mut current.count {
            *value = u32::from_le_bytes(bytes[position..position + 4].try_into().unwrap());
            position += 4;
        }
        for value in [
            &mut current.transparent,
            &mut current.partial,
            &mut current.opaque,
        ] {
            *value = u64::from_le_bytes(bytes[position..position + 8].try_into().unwrap());
            position += 8;
        }
        Ok(current)
    }
}

fn compare(
    index: usize,
    source: &FrameStats,
    output: &FrameStats,
    pixels: u64,
) -> Result<(), String> {
    for (name, expected, actual) in [
        ("transparent", source.transparent, output.transparent),
        ("partial", source.partial, output.partial),
        ("opaque", source.opaque, output.opaque),
    ] {
        if expected.abs_diff(actual) > (pixels / 50).max(1) {
            return Err(format!(
                "VP9 frame {index} changed the {name} alpha coverage beyond the validated tolerance."
            ));
        }
    }
    for cell in 0..GRID * GRID {
        if source.count[cell] != output.count[cell] {
            return Err(format!(
                "VP9 frame {index} alpha grid did not cover every decoded pixel."
            ));
        }
        // Valid small frames may not populate all 16×16 cells.
        if source.count[cell] == 0 {
            continue;
        }
        let count = u64::from(source.count[cell]);
        if source.sum[cell].abs_diff(output.sum[cell]) > count * 24 {
            return Err(format!(
                "VP9 frame {index} changed alpha opacity in grid cell {cell} beyond the validated tolerance."
            ));
        }
    }
    Ok(())
}

fn failed(message: impl Into<String>) -> AppError {
    AppError::new("ENCODE_ALPHA_VALIDATION_FAILED", message, None)
}

fn decoder(spec: &CommandSpec, forced_vpx: bool) -> Result<CommandSpec, AppError> {
    let mut spec = spec.clone();
    if !spec
        .args
        .windows(2)
        .any(|pair| pair[0] == "-f" && pair[1] == "rawvideo")
        || spec.args.last().is_none_or(|arg| arg != "pipe:1")
        || !spec
            .args
            .windows(2)
            .any(|pair| pair[0] == "-pix_fmt" && pair[1] == "yuva420p")
    {
        return Err(failed(
            "The VP9 alpha decoder command does not emit raw YUVA420P frames.",
        ));
    }
    if forced_vpx {
        let input = spec
            .args
            .iter()
            .position(|arg| arg == "-i")
            .ok_or_else(|| failed("The VP9 alpha decoder has no input."))?;
        spec.args.splice(
            input..input,
            [OsString::from("-c:v"), OsString::from("libvpx-vp9")],
        );
    }
    Ok(spec)
}

// Geometry, frame count, scratch handle and cancellation remain explicit for the
// bounded streaming decoder; grouping them would obscure the two-pass contract.
#[allow(clippy::too_many_arguments)]
async fn summarize(
    label: &'static str,
    spec: &CommandSpec,
    width: u32,
    height: u32,
    frames: usize,
    mut summaries: File,
    compare_to_source: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<String, AppError> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| failed("VP9 alpha dimensions overflow."))?;
    let frame_bytes = pixels
        .checked_mul(5)
        .map(|value| value / 2)
        .ok_or_else(|| failed("VP9 alpha frame size overflow."))?;
    let alpha_offset = pixels * 3 / 2;
    let expected = frame_bytes
        .checked_mul(frames as u64)
        .ok_or_else(|| failed("VP9 alpha frame count overflow."))?;
    let result = supervisor::run_streaming_stdout(
        spec,
        cancel.clone(),
        STDERR_LIMIT,
        TIME_LIMIT,
        move |reader| {
            let mut buffer = vec![0u8; READ_BYTES];
            let mut digest = Sha256::new();
            let mut observed = 0u64;
            let mut stats = FrameStats::default();
            loop {
                let count = reader
                    .read(&mut buffer)
                    .map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                let next = observed
                    .checked_add(count as u64)
                    .ok_or("VP9 alpha byte count overflow.")?;
                if next > expected {
                    return Err(format!("{label} decoded more than {expected} bytes."));
                }
                digest.update(&buffer[..count]);
                for (offset, value) in buffer[..count].iter().copied().enumerate() {
                    let absolute = observed + offset as u64;
                    let position = absolute % frame_bytes;
                    if position < alpha_offset {
                        continue;
                    }
                    let frame = (absolute / frame_bytes) as usize;
                    let alpha_pixel = position - alpha_offset;
                    let x = alpha_pixel % u64::from(width);
                    let y = alpha_pixel / u64::from(width);
                    let cell_x = (x * GRID as u64 / u64::from(width)) as usize;
                    let cell_y = (y * GRID as u64 / u64::from(height)) as usize;
                    let cell = cell_y * GRID + cell_x;
                    stats.sum[cell] += u64::from(value);
                    stats.count[cell] += 1;
                    if value <= 16 {
                        stats.transparent += 1;
                    } else if value >= 239 {
                        stats.opaque += 1;
                    } else {
                        stats.partial += 1;
                    }
                    if position + 1 == frame_bytes {
                        if compare_to_source {
                            let source = FrameStats::read_from(&mut summaries)
                                .map_err(|error| error.to_string())?;
                            compare(frame, &source, &stats, pixels)?;
                        } else {
                            stats
                                .write_to(&mut summaries)
                                .map_err(|error| error.to_string())?;
                        }
                        stats = FrameStats::default();
                    }
                }
                observed = next;
            }
            if observed != expected {
                return Err(format!(
                    "{label} decoded {observed} bytes; expected {expected}."
                ));
            }
            Ok(format!("{:x}", digest.finalize()))
        },
    )
    .await
    .map_err(|error| match error {
        SupervisorError::Cancelled => {
            AppError::new("JOB_CANCELED", "VP9 alpha verification was canceled.", None)
        }
        other => failed(format!("Could not completely decode {label}: {other}")),
    })?;
    if !result.status.success() {
        return Err(failed(format!(
            "{label} decoder failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    Ok(result.value)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn verify(
    reference: &CommandSpec,
    candidate: &CommandSpec,
    width: u32,
    height: u32,
    frames: usize,
    lossless: bool,
    scratch_base: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<String, AppError> {
    if width == 0
        || height == 0
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || frames == 0
    {
        return Err(failed(
            "VP9 alpha validation requires positive even dimensions and frames.",
        ));
    }
    let scratch = super::files::Temporary::create_extension(
        scratch_base,
        &format!(
            "alpha-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
        "stats",
    )?;
    let source_file = scratch.clone_file()?;
    let reference = summarize(
        "source alpha",
        &decoder(reference, false)?,
        width,
        height,
        frames,
        source_file,
        false,
        cancel,
    )
    .await?;
    let mut candidate_file = scratch.clone_file()?;
    candidate_file
        .seek(SeekFrom::Start(0))
        .map_err(|error| failed(error.to_string()))?;
    let candidate = summarize(
        "encoded alpha",
        &decoder(candidate, true)?,
        width,
        height,
        frames,
        candidate_file,
        true,
        cancel,
    )
    .await?;
    if lossless && reference != candidate {
        return Err(failed("Lossless VP9 output changed decoded YUVA samples."));
    }
    Ok(if lossless {
        format!("VP9 alpha and color samples match sha256:{reference} across {frames} frames")
    } else {
        format!("VP9 alpha coverage and 16×16 opacity grid verified across {frames} frames")
    })
}
