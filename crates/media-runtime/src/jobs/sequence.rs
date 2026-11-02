//! Transparent, content-bound still-folder sources. Receipts live beside their
//! owned lossless source; the original folder is never used as a write target.
use super::images::Directory;
use crate::{
    discovery::find_executable,
    supervisor::{self, CommandSpec},
};
use media_core::{
    AppError, FrameRate, ImagePixelFormat, ImageSequenceImportRequest, ImageSequenceReleaseRequest,
    MediaFile, MediaSource,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio::sync::watch;
mod order;
const PREFIX: &str = ".jesses-sequence-";
const RECEIPT: &str = "source.json";
const MEDIA: &str = "frames.mkv";
const RECEIPT_LIMIT: u64 = 128 * 1024 * 1024;
fn error(code: &str, message: impl Into<String>, path: &Path) -> AppError {
    AppError::new(code, message, Some(path.to_string_lossy().into_owned()))
}
fn invalid(path: &Path, message: impl Into<String>) -> AppError {
    error("IMAGE_SEQUENCE_CHANGED", message, path)
}
fn check(cancel: &watch::Receiver<bool>) -> Result<(), AppError> {
    if *cancel.borrow() {
        Err(AppError::new(
            "ANALYSIS_CANCELLED",
            "Image sequence import was canceled.",
            None,
        ))
    } else {
        Ok(())
    }
}
fn redirected(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}
fn identity(file: &File) -> Result<Vec<u64>, AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file
            .metadata()
            .map_err(|e| AppError::new("FILE_UNREADABLE", e.to_string(), None))?;
        Ok(vec![m.dev(), m.ino()])
    }
    #[cfg(windows)]
    {
        let (a, b, c) = super::files::windows_file_id(file)
            .map_err(|e| AppError::new("FILE_UNREADABLE", e.to_string(), None))?;
        Ok(vec![u64::from(a), u64::from(b), u64::from(c)])
    }
}
fn open(path: &Path, directory: bool) -> Result<File, AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| invalid(path, e.to_string()))?;
    if redirected(&metadata)
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err(invalid(
            path,
            "Sequence ownership requires a regular file/directory without symlink or reparse-point redirection.",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1 | 4).custom_flags(if directory {
            0x02000000 | 0x00200000
        } else {
            0x00200000
        });
    }
    let file = options
        .open(path)
        .map_err(|e| invalid(path, e.to_string()))?;
    // Compare open and named objects before using either as an identity guard.
    if directory
        != file
            .metadata()
            .map_err(|e| invalid(path, e.to_string()))?
            .is_dir()
    {
        return Err(invalid(
            path,
            "The source object changed while being opened.",
        ));
    }
    Ok(file)
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stamp {
    identity: Vec<u64>,
    length: u64,
    modified: SystemTime,
    sha256: String,
}
fn stamp(path: &Path, cancel: Option<&watch::Receiver<bool>>) -> Result<Stamp, AppError> {
    let mut file = open(path, false)?;
    let metadata = file.metadata().map_err(|e| invalid(path, e.to_string()))?;
    let before = (
        identity(&file)?,
        metadata.len(),
        metadata
            .modified()
            .map_err(|e| invalid(path, e.to_string()))?,
    );
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        if let Some(cancel) = cancel {
            check(cancel)?
        }
        let n = file
            .read(&mut bytes)
            .map_err(|e| invalid(path, e.to_string()))?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    let after = file.metadata().map_err(|e| invalid(path, e.to_string()))?;
    let named = open(path, false)?;
    if before
        != (
            identity(&named)?,
            after.len(),
            after.modified().map_err(|e| invalid(path, e.to_string()))?,
        )
    {
        return Err(invalid(path, "An image changed while its bytes were read."));
    }
    Ok(Stamp {
        identity: before.0,
        length: before.1,
        modified: before.2,
        sha256: format!("{:x}", hash.finalize()),
    })
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    name: String,
    stamp: Stamp,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Originals {
    directory: PathBuf,
    identity: Vec<u64>,
    extension: String,
    frame_rate: FrameRate,
    collation_locale: String,
    collation_engine: String,
    frames: Vec<Frame>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    originals: Originals,
    sequence_fingerprint: String,
    directory_identity: Vec<u64>,
    receipt_identity: Vec<u64>,
    original_stream: media_core::MediaStream,
    media: Stamp,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn names(
    directory: &Path,
    extension: Option<&str>,
    locale: &str,
) -> Result<(String, Vec<String>), AppError> {
    let mut names = Vec::new();
    let mut metadata_budget = 0usize;
    for entry in fs::read_dir(directory).map_err(|e| invalid(directory, e.to_string()))? {
        let entry = entry.map_err(|e| invalid(directory, e.to_string()))?;
        let file_type = entry
            .file_type()
            .map_err(|e| invalid(directory, e.to_string()))?;
        if file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        let Some(ext) = path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "webp") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|e| invalid(&path, e.to_string()))?;
        if redirected(&metadata) || !metadata.is_file() {
            return Err(invalid(
                &path,
                "Image sequence entries must be regular still-image files without symbolic-link or reparse-point redirection.",
            ));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid(&path, "Image names must be valid Unicode."))?;
        metadata_budget = metadata_budget
            .saturating_add(name.len())
            .saturating_add(256);
        if metadata_budget > RECEIPT_LIMIT as usize {
            return Err(error(
                "IMAGE_SEQUENCE_RESOURCE_LIMIT",
                "Folder ordering metadata exceeds the128MiB receipt budget.",
                directory,
            ));
        }
        names.push((name, ext));
    }
    let collator = order::collator(locale)?;
    // Stable comparison preserves directory enumeration for collation-equal names,
    // matching .NET OrderBy rather than inventing an ordinal secondary order.
    names.sort_by(|a, b| collator.compare(&a.0, &b.0));
    let extension = extension
        .map(str::to_owned)
        .or_else(|| names.first().map(|(_, extension)| extension.clone()))
        .ok_or_else(|| {
            invalid(
                directory,
                "The folder contains no top-level PNG, JPEG, BMP or WebP images.",
            )
        })?;
    let names = names
        .into_iter()
        .filter(|(_, ext)| *ext == extension)
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    if names.is_empty() {
        return Err(invalid(
            directory,
            "The selected image extension no longer has any frames.",
        ));
    }
    Ok((extension, names))
}
impl Originals {
    fn verify(&self, cancel: Option<&watch::Receiver<bool>>) -> Result<(), AppError> {
        if identity(&open(&self.directory, true)?)? != self.identity {
            return Err(invalid(
                &self.directory,
                "The original sequence directory was replaced.",
            ));
        }
        let (_, names) = names(
            &self.directory,
            Some(&self.extension),
            &self.collation_locale,
        )?;
        if names.into_iter().collect::<HashSet<_>>()
            != self
                .frames
                .iter()
                .map(|frame| frame.name.clone())
                .collect::<HashSet<_>>()
        {
            return Err(invalid(
                &self.directory,
                "The original sequence frame names or order changed. Import the folder again.",
            ));
        }
        for frame in &self.frames {
            if Path::new(&frame.name).components().count() != 1
                || Path::new(&frame.name).file_name().and_then(|n| n.to_str())
                    != Some(frame.name.as_str())
            {
                return Err(invalid(
                    &self.directory,
                    "The sequence receipt contains an invalid frame name.",
                ));
            }
            let path = self.directory.join(&frame.name);
            if stamp(&path, cancel)? != frame.stamp {
                return Err(invalid(
                    &path,
                    "An original sequence image changed. Import the folder again.",
                ));
            }
        }
        Ok(())
    }
    fn fingerprint(&self) -> Result<String, AppError> {
        serde_json::to_vec(self)
            .map(|bytes| hash(&bytes))
            .map_err(|e| invalid(&self.directory, e.to_string()))
    }
}
fn managed(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == MEDIA)
        && path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(PREFIX))
}
fn load(path: &Path, verify_originals: bool) -> Result<(Receipt, File), AppError> {
    let directory = path
        .parent()
        .ok_or_else(|| invalid(path, "Missing sequence cache directory."))?;
    let directory_guard = open(directory, true)?;
    let mut file = open(&directory.join(RECEIPT), false)?;
    if file
        .metadata()
        .map_err(|e| invalid(path, e.to_string()))?
        .len()
        > RECEIPT_LIMIT
    {
        return Err(invalid(
            path,
            "Sequence receipt exceeds its bounded metadata budget.",
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| invalid(path, e.to_string()))?;
    let expected = format!("{PREFIX}{}", hash(&bytes));
    if directory.file_name().and_then(|name| name.to_str()) != Some(expected.as_str()) {
        return Err(invalid(
            path,
            "The immutable sequence receipt changed or was moved.",
        ));
    }
    let receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|e| invalid(path, e.to_string()))?;
    if receipt.version != 1
        || receipt.receipt_identity != identity(&file)?
        || receipt.directory_identity != identity(&directory_guard)?
        || receipt.sequence_fingerprint != receipt.originals.fingerprint()?
    {
        return Err(invalid(path, "Sequence receipt identity is invalid."));
    }
    if stamp(path, None)? != receipt.media {
        return Err(invalid(path, "The managed lossless source changed."));
    }
    if verify_originals {
        receipt.originals.verify(None)?
    }
    Ok((receipt, file))
}
/// Shared file locks prevent cache release while any guarded source/preview uses it.
pub(crate) struct Guard {
    path: PathBuf,
    _lease: super::files::WorkspaceLock,
}
impl Guard {
    pub(crate) fn open(path: &Path) -> Result<Option<Self>, AppError> {
        if !managed(path) {
            return Ok(None);
        }
        let lease = open(&path.parent().unwrap().join(RECEIPT), false)?;
        lease.try_lock_shared().map_err(|e| {
            error(
                "IMAGE_SEQUENCE_BUSY",
                format!("The sequence is being released: {e}"),
                path,
            )
        })?;
        let lease = super::files::WorkspaceLock(lease);
        load(path, true)?;
        Ok(Some(Self {
            path: path.to_owned(),
            _lease: lease,
        }))
    }
    pub(crate) fn verify(&self) -> Result<(), AppError> {
        load(&self.path, true).map(|_| ())
    }
}
pub(crate) fn original_size(path: &Path) -> Result<Option<u64>, AppError> {
    if !managed(path) {
        return Ok(None);
    }
    let (receipt, _) = load(path, true)?;
    receipt
        .originals
        .frames
        .iter()
        .try_fold(0u64, |sum, frame| sum.checked_add(frame.stamp.length))
        .map(Some)
        .ok_or_else(|| invalid(path, "Sequence byte count exceeds the size contract."))
}
pub(crate) fn original_directory(path: &Path) -> Result<Option<PathBuf>, AppError> {
    if !managed(path) {
        return Ok(None);
    }
    load(path, true).map(|(receipt, _)| Some(receipt.originals.directory))
}
pub(crate) fn is_sequence(path: &Path) -> bool {
    managed(path) || fs::canonicalize(path).is_ok_and(|path| managed(&path))
}

/// Packet inspection reads the registered originals, not their FFV1 cache:
/// the latter has different packet sizes and would invent an encoded bitrate.
pub(crate) struct PacketInput {
    pub(crate) arguments: Vec<OsString>,
    stamp: Stamp,
    _manifest: super::files::Temporary,
}
impl PacketInput {
    pub(crate) fn verify(&self, cancel: &watch::Receiver<bool>) -> Result<(), AppError> {
        if stamp(&self._manifest.path, Some(cancel))? != self.stamp {
            return Err(invalid(
                &self._manifest.path,
                "The owned sequence packet manifest changed during analysis.",
            ));
        }
        Ok(())
    }
}
pub(crate) async fn packet_input(
    path: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<PacketInput>, AppError> {
    if !managed(path) {
        return Ok(None);
    }
    let (receipt, _) = load(path, false)?;
    receipt.originals.verify(Some(cancel))?;
    let id = format!(
        "sequence-packets-{}-{}",
        std::process::id(),
        super::NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let manifest = super::files::Temporary::create_extension(path, &id, "ffconcat")?;
    let mut file = std::io::BufWriter::new(manifest.clone_file()?);
    file.write_all(b"ffconcat version 1.0\n")
        .map_err(|e| invalid(path, e.to_string()))?;
    let rate = receipt.originals.frame_rate;
    // The fork's file-only manifest has no usable duration for stills. This
    // correction pins each original packet to the registered import clock.
    // Round absolute boundaries, not one repeated fractional duration, so
    // concat's microsecond timestamps cannot accumulate rounding drift.
    for (index, frame) in receipt.originals.frames.iter().enumerate() {
        check(cancel)?;
        let original = receipt.originals.directory.join(&frame.name);
        let original = original
            .to_str()
            .ok_or_else(|| invalid(path, "Sequence paths must be UTF-8 for packet inspection."))?;
        if original.contains(['\r', '\n', '\0']) {
            return Err(invalid(
                path,
                "Sequence packet paths cannot contain line breaks or NUL.",
            ));
        }
        #[cfg(windows)]
        let original = original.replace('\\', "/");
        // ffconcat preserves backslashes inside single quotes; a quote itself
        // is represented outside the quoted run as an escaped literal.
        let escaped = original.replace('\'', "'\\''");
        let boundary = |frame: usize| {
            let n = frame as u128 * u128::from(rate.denominator) * 1_000_000;
            let d = u128::from(rate.numerator);
            (n + d / 2) / d
        };
        let micros = boundary(index + 1) - boundary(index);
        writeln!(
            file,
            "file '{escaped}'\noption framerate {}/{}\nduration {}.{:06}",
            rate.numerator,
            rate.denominator,
            micros / 1_000_000,
            micros % 1_000_000
        )
        .map_err(|e| invalid(path, e.to_string()))?;
    }
    file.flush().map_err(|e| invalid(path, e.to_string()))?;
    drop(file);
    manifest.flush_nonempty_async().await?;
    let stamp = stamp(&manifest.path, Some(cancel))?;
    Ok(Some(PacketInput {
        arguments: vec![
            "-f".into(),
            "concat".into(),
            "-safe".into(),
            "0".into(),
            "-i".into(),
            manifest.path.as_os_str().to_owned(),
        ],
        stamp,
        _manifest: manifest,
    }))
}

fn source(receipt: &Receipt) -> MediaSource {
    MediaSource::ImageSequence {
        directory_path: receipt.originals.directory.to_string_lossy().into_owned(),
        frame_rate: receipt.originals.frame_rate,
        extension: receipt.originals.extension.clone(),
        frame_count: receipt.originals.frames.len() as u32,
        sequence_fingerprint: receipt.sequence_fingerprint.clone(),
    }
}
pub(crate) fn decorate(media: &mut MediaFile) -> Result<(), AppError> {
    let path = Path::new(&media.path);
    if !managed(path) {
        return Ok(());
    }
    let (receipt, _) = load(path, true)?;
    media.source = Some(source(&receipt));
    media.name = receipt
        .originals
        .directory
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    media.id = format!("sequence-{}", receipt.sequence_fingerprint);
    media.size_bytes = receipt
        .originals
        .frames
        .iter()
        .map(|frame| frame.stamp.length)
        .sum::<u64>()
        .to_string();
    media.modified_at_ms = None;
    let rate = receipt.originals.frame_rate;
    let duration = receipt.originals.frames.len() as f64 * f64::from(rate.denominator)
        / f64::from(rate.numerator);
    media.duration_seconds = Some(duration);
    media.format = Some("Folder".into());
    media.bit_rate = None;
    media.streams = vec![receipt.original_stream.clone()];
    for stream in &mut media.streams {
        stream.index = 0;
        stream.bit_rate = None;
        stream.duration_seconds = Some(duration);
        let rate = format!("{}/{}", rate.numerator, rate.denominator);
        stream.frame_rate = Some(rate.clone());
        stream.average_frame_rate = Some(rate.clone());
        stream.nominal_frame_rate = Some(rate);
    }
    Ok(())
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(Into::into).collect()
}
async fn tool(name: &str) -> Result<PathBuf, AppError> {
    find_executable(&[name])
        .await
        .map_err(|e| AppError::new("TOOL_MISSING", e, None))?
        .ok_or_else(|| {
            AppError::new(
                "TOOL_MISSING",
                format!("Install {name} to import an image sequence."),
                None,
            )
        })
}
async fn run(
    executable: &Path,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    cancel: &watch::Receiver<bool>,
) -> Result<Vec<u8>, AppError> {
    let output = supervisor::run_capture(
        &CommandSpec {
            executable: executable.to_owned(),
            args,
            cwd,
        },
        cancel.clone(),
        2 * 1024 * 1024,
        Duration::from_secs(86400),
    )
    .await
    .map_err(|e| error("IMAGE_SEQUENCE_FAILED", e.to_string(), executable))?;
    check(cancel)?;
    if !output.status.success() {
        return Err(error(
            "IMAGE_SEQUENCE_FAILED",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(2000)
                .collect::<String>(),
            executable,
        ));
    }
    Ok(output.stdout)
}
#[derive(Deserialize)]
struct Probe {
    streams: Vec<ImageStream>,
}
#[derive(Deserialize)]
struct ImageStream {
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    color_range: Option<String>,
    nb_read_frames: Option<String>,
}
async fn first(
    path: &Path,
    ffprobe: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<(ImageStream, media_core::MediaStream), AppError> {
    let mut a = args(&[
        "-v",
        "error",
        "-count_frames",
        "-show_streams",
        "-of",
        "json",
        "-i",
    ]);
    a.push(path.as_os_str().to_owned());
    let bytes = run(ffprobe, a, None, cancel).await?;
    let original = crate::probe::parse_probe(
        &bytes,
        path.to_string_lossy().into_owned(),
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        fs::metadata(path)
            .map_err(|e| invalid(path, e.to_string()))?
            .len(),
    )?
    .streams
    .into_iter()
    .next()
    .ok_or_else(|| invalid(path, "The original image stream is missing."))?;
    let parsed: Probe = serde_json::from_slice(&bytes).map_err(|e| invalid(path, e.to_string()))?;
    if parsed.streams.len() != 1 || parsed.streams[0].nb_read_frames.as_deref() != Some("1") {
        return Err(invalid(
            path,
            "Every sequence entry must be a single still image.",
        ));
    }
    Ok((parsed.streams.into_iter().next().unwrap(), original))
}
fn layout(format: &str) -> Option<(&'static str, ImagePixelFormat)> {
    Some(match format {
        "rgba" | "bgra" | "argb" | "abgr" | "pal8" => ("bgra", ImagePixelFormat::Rgba),
        "rgb24" | "bgr24" | "rgb0" | "bgr0" | "0rgb" | "0bgr" => ("bgr0", ImagePixelFormat::Rgb24),
        "rgb48be" | "rgb48le" | "gbrp16be" | "gbrp16le" => ("rgb48le", ImagePixelFormat::Rgb48),
        "rgba64be" | "rgba64le" | "gbrap16be" | "gbrap16le" => {
            ("rgba64le", ImagePixelFormat::Rgba64)
        }
        "gray" => ("gray", ImagePixelFormat::Rgb24),
        "gray16be" | "gray16le" => ("gray16le", ImagePixelFormat::Rgb48),
        "ya8" => ("ya8", ImagePixelFormat::Rgba),
        "yuvj420p" | "yuv420p" => ("yuv420p", ImagePixelFormat::Yuv420p),
        "yuvj422p" | "yuv422p" => ("yuv422p", ImagePixelFormat::Yuv422p),
        "yuvj444p" | "yuv444p" => ("yuv444p", ImagePixelFormat::Yuv444p),
        _ => return None,
    })
}
fn reserve_descriptors(count: usize, path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
            return Err(invalid(path, std::io::Error::last_os_error().to_string()));
        }
        let mut limit = unsafe { limit.assume_init() };
        let needed = count as u64 + 256;
        if needed > limit.rlim_max {
            return Err(invalid(
                path,
                format!(
                    "This sequence needs {needed} file identity guards; the process hard limit is {}.",
                    limit.rlim_max
                ),
            ));
        }
        if needed > limit.rlim_cur {
            limit.rlim_cur = needed;
            if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
                return Err(invalid(path, std::io::Error::last_os_error().to_string()));
            }
        }
    }
    let _ = (count, path);
    Ok(())
}
/// Native callers supply the app cache root; it is never a user output destination.
pub async fn import_image_sequence(
    request: ImageSequenceImportRequest,
    cache_root: PathBuf,
    cancel: watch::Receiver<bool>,
) -> Result<MediaFile, AppError> {
    let _permit = crate::analysis::permit(&cancel).await?;
    check(&cancel)?;
    let mut rate = request.frame_rate;
    if rate.numerator == 0 || rate.denominator == 0 {
        return Err(AppError::new(
            "IMAGE_SEQUENCE_RATE_INVALID",
            "Enter a positive rational frame rate.",
            None,
        ));
    }
    fn gcd(mut a: u32, mut b: u32) -> u32 {
        while b != 0 {
            (a, b) = (b, a % b)
        }
        a
    }
    let g = gcd(rate.numerator, rate.denominator);
    rate.numerator /= g;
    rate.denominator /= g;
    let fps = f64::from(rate.numerator) / f64::from(rate.denominator);
    if !(0.001..=1000.0).contains(&fps)
        || rate.numerator > i32::MAX as u32
        || rate.denominator > i32::MAX as u32
    {
        return Err(AppError::new(
            "IMAGE_SEQUENCE_RATE_INVALID",
            "Enter a rational frame rate from0.001 through1000fps within FFmpeg's integer time-base range.",
            None,
        ));
    }
    let requested = PathBuf::from(request.directory_path);
    if !requested.is_absolute() {
        return Err(invalid(&requested, "Use an absolute local image folder."));
    }
    let directory_guard = open(&requested, true)?;
    let directory = fs::canonicalize(&requested).map_err(|e| invalid(&requested, e.to_string()))?;
    let directory_identity = identity(&directory_guard)?;
    let collation_locale = order::locale();
    let (extension, names) = names(&directory, None, &collation_locale)?;
    if names.len() > u32::MAX as usize {
        return Err(invalid(
            &directory,
            "The sequence count exceeds the media frame-count contract.",
        ));
    }
    reserve_descriptors(names.len(), &directory)?;
    fs::create_dir_all(&cache_root).map_err(|e| invalid(&cache_root, e.to_string()))?;
    let cache_root =
        fs::canonicalize(&cache_root).map_err(|e| invalid(&cache_root, e.to_string()))?;
    if cache_root.starts_with(&directory) {
        return Err(invalid(
            &directory,
            "The private sequence cache cannot be inside the original image folder.",
        ));
    }
    let ffmpeg = tool("ffmpeg").await?;
    let ffprobe = tool("ffprobe").await?;
    let (stream, original_stream) = first(&directory.join(&names[0]), &ffprobe, &cancel).await?;
    let (width, height) = stream
        .width
        .zip(stream.height)
        .filter(|(w, h)| {
            *w > 0
                && *h > 0
                && *w <= 16384
                && *h <= 16384
                && u64::from(*w) * u64::from(*h) <= 260_000_000
        })
        .ok_or_else(|| {
            invalid(
                &directory,
                "Sequence images exceed the supported geometry/resource range.",
            )
        })?;
    let pixel = stream.pix_fmt.as_deref().ok_or_else(|| {
        invalid(
            &directory,
            "The source still-image pixel format is unknown.",
        )
    })?;
    let (encoded_format, comparison) = layout(pixel).ok_or_else(|| {
        invalid(
            &directory,
            format!("Still-image pixel format {pixel} has no qualified lossless materialization."),
        )
    })?;
    let codec = match extension.as_str() {
        "png" => "png",
        "jpg" | "jpeg" => "mjpeg",
        "bmp" => "bmp",
        "webp" => "webp",
        _ => unreachable!(),
    };
    if stream.codec_name.as_deref() != Some(codec) {
        return Err(invalid(
            &directory,
            "The selected image extension does not match its decoded codec.",
        ));
    }
    let source_size = names
        .iter()
        .try_fold(0u64, |sum, name| {
            fs::metadata(directory.join(name)).map(|m| sum.saturating_add(m.len()))
        })
        .map_err(|e| invalid(&directory, e.to_string()))?;
    let required = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(16)
        .saturating_mul(names.len() as u64)
        .saturating_add(source_size.saturating_mul(2))
        .saturating_add(16 * 1024 * 1024);
    let available = super::available_space(&cache_root)?;
    if required > available {
        return Err(invalid(
            &cache_root,
            format!(
                "Image sequence preparation needs up to{}MiB;{}MiB is available.",
                required.div_ceil(1024 * 1024),
                available / (1024 * 1024)
            ),
        ));
    }
    let mut stage = Directory::new(&cache_root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stage.path, fs::Permissions::from_mode(0o700))
            .map_err(|e| invalid(&stage.path, e.to_string()))?;
    }
    let mut frames = Vec::with_capacity(names.len());
    let mut copies = HashSet::with_capacity(names.len());
    for (index, name) in names.into_iter().enumerate() {
        check(&cancel)?;
        let original = directory.join(&name);
        let expected = stamp(&original, Some(&cancel))?;
        let path = stage
            .path
            .join(format!("frame-{:010}.{extension}", index + 1));
        let mut output = stage.create_file(path.clone())?;
        let mut input = open(&original, false)?;
        let mut bytes = [0u8; 65536];
        let mut copied = Sha256::new();
        loop {
            check(&cancel)?;
            let n = input
                .read(&mut bytes)
                .map_err(|e| invalid(&original, e.to_string()))?;
            if n == 0 {
                break;
            }
            output
                .write_all(&bytes[..n])
                .map_err(|e| invalid(&path, e.to_string()))?;
            copied.update(&bytes[..n]);
        }
        output
            .sync_all()
            .map_err(|e| invalid(&path, e.to_string()))?;
        if expected.sha256 != format!("{:x}", copied.finalize())
            || stamp(&original, Some(&cancel))? != expected
        {
            return Err(invalid(
                &original,
                "An original image changed during staging.",
            ));
        }
        frames.push(Frame {
            name,
            stamp: expected,
        });
        copies.insert(path);
    }
    let originals = Originals {
        directory,
        identity: directory_identity,
        extension: extension.clone(),
        frame_rate: rate,
        collation_locale,
        collation_engine: order::ENGINE.into(),
        frames,
    };
    originals.verify(Some(&cancel))?;
    let pattern = stage.path.join(format!("frame-%010d.{extension}"));
    verify_input(
        &ffprobe,
        &pattern,
        originals.frames.len() as u32,
        (width, height),
        pixel,
        &cancel,
    )
    .await?;
    let reference =
        super::images::scan_pixels(&ffmpeg, &pattern, 0, None, comparison, true, &[], &cancel)
            .await?;
    let media = stage.path.join(MEDIA);
    let mut a = args(&[
        "-v",
        "error",
        "-nostdin",
        "-f",
        "image2",
        "-framerate",
        &format!("{}/{}", rate.numerator, rate.denominator),
        "-pattern_type",
        "sequence",
        "-start_number",
        "1",
        "-c:v",
        codec,
        "-i",
    ]);
    a.push(pattern.as_os_str().to_owned());
    a.extend(args(&[
        "-map",
        "0:v:0",
        "-an",
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-g",
        "1",
        "-pix_fmt",
        encoded_format,
        "-threads",
        "2",
        "-color_range",
        if stream.color_range.as_deref() == Some("pc")
            || codec == "mjpeg"
            || matches!(
                comparison,
                ImagePixelFormat::Rgb24
                    | ImagePixelFormat::Rgba
                    | ImagePixelFormat::Rgb48
                    | ImagePixelFormat::Rgba64
            )
        {
            "pc"
        } else {
            "tv"
        },
        "-f",
        "matroska",
        "-y",
    ]));
    if pixel.starts_with("yuvj") {
        // FFV1 stores the modern planar layout plus an explicit full-range
        // declaration. Older FFmpeg autoscalers otherwise compress JPEG's
        // samples to limited range while merely tagging the result as full.
        // Make the range-preserving conversion explicit; the complete sample
        // hash below still proves every frame against the original decoder.
        a.extend(args(&["-vf", "scale=in_range=pc:out_range=pc"]));
    }
    a.push(media.as_os_str().to_owned());
    drop(stage.create_file(media.clone())?);
    run(&ffmpeg, a, None, &cancel).await?;
    let actual =
        super::images::scan_pixels(&ffmpeg, &media, 0, None, comparison, false, &[], &cancel)
            .await?;
    if actual != reference
        || actual.count != originals.frames.len() as u32
        || actual.dimensions != (width, height)
    {
        return Err(invalid(
            &media,
            "Lossless materialization changed original frame count, order, geometry, colors or alpha samples.",
        ));
    }
    verify_clock(
        &ffprobe,
        &media,
        originals.frames.len() as u32,
        rate,
        &cancel,
    )
    .await?;
    originals.verify(Some(&cancel))?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&media)
        .and_then(|file| file.sync_all())
        .map_err(|e| invalid(&media, e.to_string()))?;
    let media_stamp = stamp(&media, Some(&cancel))?;
    let fingerprint = originals.fingerprint()?;
    let mut file = stage.create_file(stage.path.join(RECEIPT))?;
    let receipt = Receipt {
        version: 1,
        originals,
        sequence_fingerprint: fingerprint,
        directory_identity: identity(&open(&stage.path, true)?)?,
        receipt_identity: identity(&file)?,
        original_stream,
        media: media_stamp,
    };
    let bytes = serde_json::to_vec(&receipt).map_err(|e| invalid(&media, e.to_string()))?;
    if bytes.len() as u64 > RECEIPT_LIMIT {
        return Err(invalid(
            &media,
            "The sequence receipt exceeds its bounded metadata budget.",
        ));
    }
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| invalid(&media, e.to_string()))?;
    stage.remove_files(&copies)?;
    check(&cancel)?;
    let final_directory = cache_root.join(format!("{PREFIX}{}", hash(&bytes)));
    stage.publish(&final_directory)?;
    // Publication owns only this receipt-validated directory; if final probing
    // fails, drop the source guard before releasing our own materialization.
    let path = final_directory.join(MEDIA);
    let result = crate::probe::probe_media(path.to_string_lossy().into_owned())
        .await
        .and_then(|media| check(&cancel).map(|_| media));
    if result.is_err() {
        let _ = release_image_sequence(
            ImageSequenceReleaseRequest {
                input_path: path.to_string_lossy().into_owned(),
                sequence_fingerprint: receipt.sequence_fingerprint,
            },
            cache_root,
        );
    }
    result
}
#[derive(Deserialize)]
struct InputFrame {
    width: u32,
    height: u32,
    pix_fmt: String,
}
async fn verify_input(
    ffprobe: &Path,
    input: &Path,
    count: u32,
    size: (u32, u32),
    pixel: &str,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "image2",
        "-start_number",
        "1",
        "-show_frames",
        "-show_entries",
        "frame=width,height,pix_fmt",
        "-of",
        "json",
        "-i",
    ]);
    a.push(input.as_os_str().to_owned());
    let pixel = pixel.to_owned();
    let result=supervisor::run_streaming_stdout(&CommandSpec{executable:ffprobe.to_owned(),args:a,cwd:None},cancel.clone(),65536,Duration::from_secs(86400),move |reader|{let mut observed=0u32;let mut mismatch=false;super::parse_frame_records(reader,|frame:InputFrame|{observed=observed.saturating_add(1);mismatch|=(frame.width,frame.height)!=size||frame.pix_fmt!=pixel;})?;if observed!=count||mismatch{return Err("Every sequence image must be one still with matching dimensions, bit depth and pixel layout.".into())}Ok(())}).await.map_err(|e|invalid(input,e.to_string()))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(invalid(
            input,
            "Every sequence still must decode successfully.",
        ));
    }
    check(cancel)
}
#[derive(Deserialize)]
struct Clock {
    best_effort_timestamp_time: Option<String>,
}
async fn verify_clock(
    ffprobe: &Path,
    input: &Path,
    count: u32,
    rate: FrameRate,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let mut a = args(&[
        "-v",
        "error",
        "-show_frames",
        "-show_entries",
        "frame=best_effort_timestamp_time",
        "-of",
        "json",
        "-i",
    ]);
    a.push(input.as_os_str().to_owned());
    let result=supervisor::run_streaming_stdout(&CommandSpec{executable:ffprobe.to_owned(),args:a,cwd:None},cancel.clone(),65536,Duration::from_secs(86400),move |reader|{let mut observed=0u32;let mut mismatch=false;super::parse_frame_records(reader,|frame:Clock|{let expected=f64::from(observed)*f64::from(rate.denominator)/f64::from(rate.numerator);mismatch|=!frame.best_effort_timestamp_time.and_then(|v|v.parse::<f64>().ok()).is_some_and(|v|v.is_finite()&&(v-expected).abs()<=0.0021);observed=observed.saturating_add(1);})?;if observed!=count||mismatch{return Err("Materialization did not preserve the chosen sequence FPS and complete frame count.".into())}Ok(())}).await.map_err(|e|invalid(input,e.to_string()))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(invalid(
            input,
            "The complete sequence clock could not be verified.",
        ));
    }
    check(cancel)
}
/// Cache deletion never follows original frame paths. Stale originals may still
/// release an otherwise unchanged cache after their queued/recovery users end.
pub fn release_image_sequence(
    request: ImageSequenceReleaseRequest,
    cache_root: PathBuf,
) -> Result<(), AppError> {
    let path = PathBuf::from(request.input_path);
    if !path.is_absolute() || !managed(&path) {
        return Err(invalid(&path, "Select a managed image-sequence source."));
    }
    let parent = path.parent().unwrap();
    let cache_root = fs::canonicalize(cache_root).map_err(|e| invalid(parent, e.to_string()))?;
    if parent.parent() != Some(cache_root.as_path()) {
        return Err(invalid(
            &path,
            "The source is outside the application's sequence cache.",
        ));
    }
    let lease = open(&parent.join(RECEIPT), false)?;
    lease.try_lock().map_err(|e| {
        error(
            "IMAGE_SEQUENCE_BUSY",
            format!("The source is in use by a preview or job: {e}"),
            &path,
        )
    })?;
    let (receipt, receipt_file) = load(&path, false)?;
    if receipt.sequence_fingerprint != request.sequence_fingerprint {
        return Err(invalid(
            &path,
            "The release request does not match the immutable sequence identity.",
        ));
    }
    let entries = fs::read_dir(parent)
        .map_err(|e| invalid(parent, e.to_string()))?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|e| invalid(parent, e.to_string()))?;
    if entries != HashSet::from([OsString::from(MEDIA), OsString::from(RECEIPT)]) {
        return Err(invalid(
            parent,
            "Unexpected cache entries were retained; cleanup cannot delete unrelated files.",
        ));
    }
    let media_guard = open(&path, false)?;
    #[cfg(unix)]
    let receipt_id = identity(&receipt_file)?;
    #[cfg(unix)]
    let media_id = identity(&media_guard)?;
    drop(receipt_file);
    #[cfg(unix)]
    {
        if identity(&open(&path, false)?)? != media_id {
            return Err(invalid(&path, "The cache changed before cleanup."));
        }
        fs::remove_file(&path).map_err(|e| invalid(&path, e.to_string()))?;
        if identity(&open(&parent.join(RECEIPT), false)?)? != receipt_id {
            return Err(invalid(parent, "The receipt changed before cleanup."));
        }
        fs::remove_file(parent.join(RECEIPT)).map_err(|e| invalid(parent, e.to_string()))?;
    }
    #[cfg(windows)]
    {
        let media_id = super::files::windows_file_id(&media_guard)
            .map_err(|e| invalid(&path, e.to_string()))?;
        let receipt_id =
            super::files::windows_file_id(&lease).map_err(|e| invalid(parent, e.to_string()))?;
        drop(media_guard);
        super::files::windows_delete_owned(&path, media_id)
            .map_err(|e| invalid(&path, e.to_string()))?;
        super::files::windows_delete_owned(&parent.join(RECEIPT), receipt_id)
            .map_err(|e| invalid(parent, e.to_string()))?;
    }
    #[cfg(unix)]
    drop(media_guard);
    drop(lease);
    fs::remove_dir(parent).map_err(|e| invalid(parent, e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        cache: PathBuf,
        path: PathBuf,
        request: ImageSequenceReleaseRequest,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "jesses-sequence-guard-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            let originals = root.join("originals");
            fs::create_dir(&originals).unwrap();
            let image = originals.join("frame.png");
            fs::write(&image, b"immutable original").unwrap();
            let originals = Originals {
                directory: originals.clone(),
                identity: identity(&open(&originals, true).unwrap()).unwrap(),
                extension: "png".into(),
                frame_rate: FrameRate {
                    numerator: 30,
                    denominator: 1,
                },
                collation_locale: "en-US".into(),
                collation_engine: order::ENGINE.into(),
                frames: vec![Frame {
                    name: "frame.png".into(),
                    stamp: stamp(&image, None).unwrap(),
                }],
            };
            let cache = root.join("cache");
            fs::create_dir(&cache).unwrap();
            let mut stage = Directory::new(&cache).unwrap();
            let path = stage.path.join(MEDIA);
            stage
                .create_file(path.clone())
                .unwrap()
                .write_all(b"verified derivative")
                .unwrap();
            let fingerprint = originals.fingerprint().unwrap();
            let mut receipt_file = stage.create_file(stage.path.join(RECEIPT)).unwrap();
            let receipt = Receipt {
                version: 1,
                originals,
                sequence_fingerprint: fingerprint.clone(),
                directory_identity: identity(&open(&stage.path, true).unwrap()).unwrap(),
                receipt_identity: identity(&receipt_file).unwrap(),
                original_stream: crate::probe::parse_probe(
                    br#"{"streams":[{"index":0,"codec_type":"video","codec_name":"png"}]}"#,
                    String::new(),
                    String::new(),
                    0,
                )
                .unwrap()
                .streams
                .remove(0),
                media: stamp(&path, None).unwrap(),
            };
            let bytes = serde_json::to_vec(&receipt).unwrap();
            receipt_file.write_all(&bytes).unwrap();
            let destination = cache.join(format!("{PREFIX}{}", hash(&bytes)));
            stage.publish(&destination).unwrap();
            let path = destination.join(MEDIA);
            let request = ImageSequenceReleaseRequest {
                input_path: path.to_string_lossy().into_owned(),
                sequence_fingerprint: fingerprint,
            };
            Self {
                root,
                cache,
                path,
                request,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn active_source_lease_blocks_release_and_stale_originals_can_release_owned_cache() {
        let f = Fixture::new();
        let source = super::super::files::Source::open(&f.path).unwrap();
        assert_eq!(
            release_image_sequence(f.request.clone(), f.cache.clone())
                .unwrap_err()
                .code,
            "IMAGE_SEQUENCE_BUSY"
        );
        let original = f.root.join("originals/frame.png");
        fs::write(&original, b"changed image").unwrap();
        assert_eq!(source.verify().unwrap_err().code, "IMAGE_SEQUENCE_CHANGED");
        drop(source);
        release_image_sequence(f.request.clone(), f.cache.clone()).unwrap();
        assert_eq!(fs::read(original).unwrap(), b"changed image");
        assert!(!f.path.exists());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn a_forked_child_cannot_keep_a_released_source_lease_locked() {
        let f = Fixture::new();
        let source = super::super::files::Source::open(&f.path).unwrap();
        let mut pipe = [-1; 2];
        // SAFETY: pipe points to two valid descriptor slots. CLOEXEC keeps this
        // synchronization pipe private across unrelated command executions.
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        // The child executes only async-signal-safe syscalls and _exit. In
        // particular it never drops the inherited Rust source/lock owner.
        let child = unsafe { libc::fork() };
        if child == 0 {
            unsafe {
                libc::close(pipe[1]);
                let mut byte = 0u8;
                libc::read(pipe[0], std::ptr::from_mut(&mut byte).cast(), 1);
                libc::_exit(0);
            }
        }
        assert!(
            child > 0,
            "Could not fork the inherited-lease regression child"
        );
        unsafe {
            libc::close(pipe[0]);
        }
        struct Child {
            pid: libc::pid_t,
            signal: libc::c_int,
        }
        impl Drop for Child {
            fn drop(&mut self) {
                unsafe {
                    let byte = 1u8;
                    libc::write(self.signal, std::ptr::from_ref(&byte).cast(), 1);
                    libc::close(self.signal);
                    while libc::waitpid(self.pid, std::ptr::null_mut(), 0) < 0 {
                        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                        {
                            break;
                        }
                    }
                }
            }
        }
        let child = Child {
            pid: child,
            signal: pipe[1],
        };
        assert_eq!(
            release_image_sequence(f.request.clone(), f.cache.clone())
                .unwrap_err()
                .code,
            "IMAGE_SEQUENCE_BUSY"
        );
        drop(source);
        // The forked child still holds the same open file description. Source
        // drop must explicitly unlock it rather than relying on final close.
        release_image_sequence(f.request.clone(), f.cache.clone()).unwrap();
        assert!(!f.path.exists());
        drop(child);
    }
    #[test]
    fn receipt_fps_or_order_changes_cannot_rebind_an_immutable_source_path() {
        let f = Fixture::new();
        let receipt = f.path.parent().unwrap().join(RECEIPT);
        let original = fs::read(&receipt).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        value["originals"]["frame_rate"]["numerator"] = serde_json::json!(60);
        fs::write(&receipt, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(super::super::files::Source::open(&f.path).is_err());
        assert!(release_image_sequence(f.request.clone(), f.cache.clone()).is_err());
        assert!(f.path.is_file());
        fs::write(receipt, original).unwrap();
        release_image_sequence(f.request.clone(), f.cache.clone()).unwrap();
    }
    #[test]
    fn extension_selection_is_top_level_filename_order_and_publication_never_clobbers() {
        let f = Fixture::new();
        let dir = f.root.join("selection");
        fs::create_dir(&dir).unwrap();
        for name in [
            "z.png",
            "b.jpg",
            "a.PNG",
            "a-readme.txt",
            "frame-2.PNG",
            "frame-10.png",
        ] {
            fs::write(dir.join(name), b"still").unwrap();
        }
        fs::create_dir(dir.join("nested")).unwrap();
        fs::write(dir.join("nested/0.jpg"), b"nested").unwrap();
        let (extension, ordered) = names(&dir, None, "en-US").unwrap();
        assert_eq!(extension, "png");
        assert_eq!(
            ordered,
            vec!["a.PNG", "frame-10.png", "frame-2.PNG", "z.png"]
        );
        let mut stage = Directory::new(&f.cache).unwrap();
        let existing = f.path.parent().unwrap();
        let receipt = fs::read(existing.join(RECEIPT)).unwrap();
        assert!(stage.publish(existing).is_err());
        assert_eq!(fs::read(existing.join(RECEIPT)).unwrap(), receipt);
    }
}
