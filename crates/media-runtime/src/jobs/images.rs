//! Image sequences use explicit order and a new, atomically published destination.
use super::files::{Source, Temporary};
use crate::{
    discovery::find_executable,
    supervisor::{self, CommandSpec},
};
use media_core::{AppError, ImageOutput, ImagePixelFormat, ImageRequest, ImageResult};
use serde::Deserialize;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::watch,
};
#[path = "images/implicit.rs"]
mod implicit;
#[path = "images/metadata.rs"]
mod metadata;
#[path = "images/review.rs"]
mod review;
#[path = "images/subtitle.rs"]
mod subtitle;

static NEXT: AtomicU64 = AtomicU64::new(1);
const LIMIT: u32 = 10_000;
const MAX_SOURCE_IMAGE_BYTES: u64 = 1024 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;

fn error(message: impl Into<String>, path: &Path) -> AppError {
    AppError::new(
        "IMAGE_JOB_FAILED",
        message,
        Some(path.to_string_lossy().into_owned()),
    )
}
fn check(cancel: &watch::Receiver<bool>) -> Result<(), AppError> {
    if *cancel.borrow() {
        Err(AppError::new(
            "JOB_CANCELED",
            "Image processing was canceled.",
            None,
        ))
    } else {
        Ok(())
    }
}
async fn pause_checkpoint(cancel: &watch::Receiver<bool>) -> Result<(), AppError> {
    if let Some(control) = supervisor::scoped_pause() {
        control
            .wait_until_resumed(cancel)
            .await
            .map_err(|cause| process_error(cause, Path::new("")))?;
    }
    check(cancel)
}
fn process_error(cause: supervisor::SupervisorError, path: &Path) -> AppError {
    match cause {
        supervisor::SupervisorError::Cancelled => {
            AppError::new("JOB_CANCELED", "Image processing was canceled.", None)
        }
        other => error(other.to_string(), path),
    }
}
async fn tool(name: &str) -> Result<PathBuf, AppError> {
    find_executable(&[name])
        .await
        .map_err(|e| AppError::new("TOOL_MISSING", e, None))?
        .ok_or_else(|| {
            AppError::new(
                "TOOL_MISSING",
                format!("Install {name} or configure its tool path."),
                None,
            )
        })
}
async fn run(
    program: &Path,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    cancel: &watch::Receiver<bool>,
) -> Result<Vec<u8>, AppError> {
    check(cancel)?;
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: program.to_owned(),
            args,
            cwd,
        },
        cancel.clone(),
        4 * 1024 * 1024,
        Duration::from_secs(24 * 60 * 60),
    )
    .await
    .map_err(|e| match e {
        supervisor::SupervisorError::Cancelled => {
            AppError::new("JOB_CANCELED", "Image processing was canceled.", None)
        }
        other => error(other.to_string(), program),
    })?;
    check(cancel)?;
    if !result.status.success() {
        return Err(error(
            format!(
                "Image tool failed: {}",
                String::from_utf8_lossy(&result.stderr)
                    .chars()
                    .rev()
                    .take(2000)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            ),
            program,
        ));
    }
    Ok(result.stdout)
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn frame_name(index: u32, extension: &str, source: bool) -> String {
    if source {
        format!("{index:08}.{extension}")
    } else {
        format!("frame-{index:06}.{extension}")
    }
}
fn frame_pattern(extension: &str, source: bool) -> String {
    if source {
        format!("%8d.{extension}")
    } else {
        format!("frame-%06d.{extension}")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Pixels {
    pub(crate) count: u32,
    pub(crate) dimensions: (u32, u32),
    pub(crate) digest: String,
}
#[allow(clippy::too_many_arguments)]
pub(crate) async fn scan_pixels(
    ffmpeg: &Path,
    input: &Path,
    stream: u32,
    filter: Option<&str>,
    format: ImagePixelFormat,
    sequence: bool,
    device_args: &[OsString],
    cancel: &watch::Receiver<bool>,
) -> Result<Pixels, AppError> {
    let mut a = args(&["-v", "error", "-nostdin"]);
    a.extend_from_slice(device_args);
    if sequence {
        a.extend(args(&["-f", "image2", "-start_number", "1"]));
    }
    a.push("-i".into());
    a.push(input.as_os_str().to_owned());
    a.extend(args(&["-map", &format!("0:{stream}")]));
    if let Some(filter) = filter {
        a.extend(args(&["-vf", filter]));
    }
    a.extend(args(&[
        "-fps_mode",
        "passthrough",
        "-c:v",
        "rawvideo",
        "-pix_fmt",
        format.ffmpeg(),
        "-threads",
        "2",
        "-f",
        "framemd5",
        "-",
    ]));
    scan_pixel_command(ffmpeg, a, None, input, cancel).await
}
async fn scan_pixel_command(
    ffmpeg: &Path,
    a: Vec<OsString>,
    cwd: Option<PathBuf>,
    input: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<Pixels, AppError> {
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args: a,
            cwd,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(86400),
        |reader| {
            use sha2::{Digest, Sha256};
            use std::io::{BufRead, Read};
            let mut reader = std::io::BufReader::new(reader);
            let mut line = Vec::new();
            let mut hash = Sha256::new();
            let mut count = 0u32;
            let mut dimensions = None;
            loop {
                line.clear();
                let n = (&mut reader)
                    .take(4097)
                    .read_until(b'\n', &mut line)
                    .map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                if n > 4096 {
                    return Err("An image frame hash record exceeds 4096 bytes.".into());
                }
                let text = std::str::from_utf8(&line)
                    .map_err(|e| e.to_string())?
                    .trim();
                if let Some(value) = text.strip_prefix("#dimensions 0:") {
                    let (w, h) = value
                        .trim()
                        .split_once('x')
                        .ok_or("Invalid image dimensions.")?;
                    dimensions = Some((
                        w.parse::<u32>().map_err(|e| e.to_string())?,
                        h.parse::<u32>().map_err(|e| e.to_string())?,
                    ));
                } else if !text.starts_with('#') && !text.is_empty() {
                    let digest = text.rsplit(',').next().unwrap_or("").trim();
                    if digest.len() != 32 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err("Invalid decoded image hash.".into());
                    }
                    hash.update(digest.as_bytes());
                    count = count
                        .checked_add(1)
                        .ok_or("Image frame count exceeds the contract range.")?;
                }
            }
            Ok(Pixels {
                count,
                dimensions: dimensions.ok_or("No decoded image dimensions.")?,
                digest: format!("{:x}", hash.finalize()),
            })
        },
    )
    .await
    .map_err(|cause| process_error(cause, input))?;
    check(cancel)?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(error(
            format!(
                "The complete image stream could not be decoded: {}",
                String::from_utf8_lossy(&result.stderr)
            ),
            input,
        ));
    }
    Ok(result.value)
}
async fn verify_frames(
    probe: &Path,
    input: &Path,
    sequence: bool,
    expected: u32,
    size: (u32, u32),
    pixel_format: Option<ImagePixelFormat>,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    #[derive(Deserialize)]
    struct Frame {
        width: u32,
        height: u32,
        pix_fmt: String,
    }
    let mut a = args(&["-v", "error"]);
    if sequence {
        a.extend(args(&["-f", "image2", "-start_number", "1"]));
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
    let result=supervisor::run_streaming_stdout(&CommandSpec{executable:probe.to_owned(),args:a,cwd:None},cancel.clone(),65536,Duration::from_secs(86400),move |reader|{let mut count=0u32;let mut mismatch=false;super::encode::frame_scan::parse(reader,|frame:Frame|{count=count.saturating_add(1);mismatch|=(frame.width,frame.height)!=size||pixel_format.is_some_and(|format|frame.pix_fmt!=format.ffmpeg());})?;if count!=expected||mismatch{return Err(format!("Image stream frame count/geometry/pixel layout changed: expected {expected} frames at {}x{}, observed {count}.",size.0,size.1))}Ok(())}).await.map_err(|cause|process_error(cause,input))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(error(
            "An exported image could not be completely decoded.",
            input,
        ));
    }
    check(cancel)
}
#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
}
#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    nb_read_frames: Option<String>,
    pix_fmt: Option<String>,
}
async fn inspect(
    path: &Path,
    probe: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<Vec<Stream>, AppError> {
    let mut a = args(&[
        "-v",
        "error",
        "-count_frames",
        "-show_streams",
        "-of",
        "json",
    ]);
    a.push(path.as_os_str().to_owned());
    let data = run(probe, a, None, cancel).await?;
    serde_json::from_slice::<Probe>(&data)
        .map(|p| p.streams)
        .map_err(|e| error(e.to_string(), path))
}
fn geometry(s: &Stream, path: &Path) -> Result<(u32, u32), AppError> {
    match (s.width, s.height) {
        (Some(w), Some(h)) if (1..=16384).contains(&w) && (1..=16384).contains(&h) => Ok((w, h)),
        _ => Err(error(
            "Images must have dimensions between 1 and 16384 pixels.",
            path,
        )),
    }
}
fn destination(path: &str) -> Result<PathBuf, AppError> {
    let p = Path::new(path);
    if !p.is_absolute() || path.contains('\0') {
        return Err(error("Choose an absolute local destination.", p));
    }
    let parent = p
        .parent()
        .and_then(|v| fs::canonicalize(v).ok())
        .filter(|v| v.is_dir())
        .ok_or_else(|| error("Choose an existing output folder.", p))?;
    let name = p
        .file_name()
        .ok_or_else(|| error("Choose an output name.", p))?;
    let output = parent.join(name);
    super::files::ensure_absent(&output)?;
    Ok(output)
}
fn id() -> String {
    format!(
        "images-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    value: (u32, u32, u32),
}

impl Identity {
    fn from_file(file: &File) -> Result<Self, std::io::Error> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = file.metadata()?;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                value: super::files::windows_file_id(file)?,
            })
        }
    }

    fn matches_path(self, path: &Path, directory: bool) -> bool {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return false;
        };
        if metadata.file_type().is_symlink()
            || if directory {
                !metadata.is_dir()
            } else {
                !metadata.is_file()
            }
        {
            return false;
        }
        open_identity(path, directory).is_ok_and(|identity| identity == self)
    }
}

fn open_identity(path: &Path, directory: bool) -> Result<Identity, std::io::Error> {
    Identity::from_file(&open_identity_guard(path, directory)?)
}

fn open_identity_guard(path: &Path, directory: bool) -> Result<File, std::io::Error> {
    #[cfg(unix)]
    let file = {
        let _ = directory;
        File::open(path)?
    };
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options
            .access_mode(0x80) // FILE_READ_ATTRIBUTES
            .share_mode(1 | 2 | 4); // FILE_SHARE_READ | WRITE | DELETE
        if directory {
            options.custom_flags(0x0020_0000 | 0x0200_0000); // OPEN_REPARSE_POINT | BACKUP_SEMANTICS
        } else {
            options.custom_flags(0x0020_0000); // OPEN_REPARSE_POINT
        }
        options.open(path)?
    };
    Ok(file)
}

struct Entry {
    path: PathBuf,
    identity: Identity,
    // Keeping the original object open prevents Unix from reusing its inode
    // for a same-name replacement before cleanup checks the pathname.
    #[cfg(unix)]
    identity_guard: Option<File>,
}

/// Holds a fresh directory reservation and removes only entries whose file
/// identities still match the files created by this job.
pub(crate) struct Directory {
    pub(crate) path: PathBuf,
    identity: Identity,
    #[cfg(unix)]
    identity_guard: Option<File>,
    entries: Vec<Entry>,
    paths: std::collections::HashSet<PathBuf>,
    published: bool,
}
impl Directory {
    pub(crate) fn new(parent: &Path) -> Result<Self, AppError> {
        let path = parent.join(format!(".jesses-{}", id()));
        fs::create_dir(&path).map_err(|e| error(e.to_string(), &path))?;
        #[cfg(unix)]
        let identity_guard =
            open_identity_guard(&path, true).map_err(|e| error(e.to_string(), &path))?;
        #[cfg(unix)]
        let identity =
            Identity::from_file(&identity_guard).map_err(|e| error(e.to_string(), &path))?;
        #[cfg(windows)]
        let identity = open_identity(&path, true).map_err(|e| error(e.to_string(), &path))?;
        Ok(Self {
            path,
            identity,
            #[cfg(unix)]
            identity_guard: Some(identity_guard),
            entries: Vec::new(),
            paths: std::collections::HashSet::new(),
            published: false,
        })
    }

    fn verify_directory(&self) -> Result<(), AppError> {
        if self.identity.matches_path(&self.path, true) {
            Ok(())
        } else {
            Err(error(
                "The owned image staging directory was replaced; it was retained for review.",
                &self.path,
            ))
        }
    }

    pub(crate) fn create_file(&mut self, path: PathBuf) -> Result<File, AppError> {
        self.verify_directory()?;
        if path.parent() != Some(self.path.as_path()) || self.paths.contains(&path) {
            return Err(error("Invalid duplicate image staging path.", &path));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|cause| error(cause.to_string(), &path))?;
        let identity =
            Identity::from_file(&file).map_err(|cause| error(cause.to_string(), &path))?;
        #[cfg(unix)]
        let identity_guard = file
            .try_clone()
            .map_err(|cause| error(cause.to_string(), &path))?;
        self.paths.insert(path.clone());
        self.entries.push(Entry {
            path,
            identity,
            #[cfg(unix)]
            identity_guard: Some(identity_guard),
        });
        Ok(file)
    }

    fn verify_entries(&self) -> Result<(), AppError> {
        self.verify_directory()?;
        for entry in &self.entries {
            if !entry.identity.matches_path(&entry.path, false) {
                return Err(error(
                    "An owned image staging file was replaced; it was retained for review.",
                    &entry.path,
                ));
            }
        }
        Ok(())
    }

    fn record_files(&mut self) -> Result<(), AppError> {
        self.verify_entries()?;
        for entry in fs::read_dir(&self.path).map_err(|e| error(e.to_string(), &self.path))? {
            let p = entry.map_err(|e| error(e.to_string(), &self.path))?.path();
            let m = fs::symlink_metadata(&p).map_err(|e| error(e.to_string(), &p))?;
            if !m.is_file() || m.file_type().is_symlink() {
                return Err(error("Unexpected directory entry in image output.", &p));
            }
            if !self.paths.contains(&p) {
                return Err(error(
                    "Unexpected file in image output; it was retained for review.",
                    &p,
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn remove_files(
        &mut self,
        paths: &std::collections::HashSet<PathBuf>,
    ) -> Result<(), AppError> {
        self.verify_entries()?;
        for entry in &self.entries {
            if paths.contains(&entry.path) {
                fs::remove_file(&entry.path).map_err(|e| error(e.to_string(), &entry.path))?;
            }
        }
        self.entries.retain(|entry| !paths.contains(&entry.path));
        self.paths.retain(|path| !paths.contains(path));
        Ok(())
    }
    pub(crate) fn publish(&mut self, output: &Path) -> Result<(), AppError> {
        self.record_files()?;
        super::files::ensure_absent(output)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            let a = std::ffi::CString::new(self.path.as_os_str().as_bytes())
                .map_err(|e| error(e.to_string(), &self.path))?;
            let b = std::ffi::CString::new(output.as_os_str().as_bytes())
                .map_err(|e| error(e.to_string(), output))?;
            // RENAME_NOREPLACE closes the exists/rename race for directories.
            if unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    a.as_ptr(),
                    libc::AT_FDCWD,
                    b.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            } != 0
            {
                return Err(error(std::io::Error::last_os_error().to_string(), output));
            }
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::ffi::OsStrExt;
            let from = std::ffi::CString::new(self.path.as_os_str().as_bytes())
                .map_err(|e| error(e.to_string(), &self.path))?;
            let to = std::ffi::CString::new(output.as_os_str().as_bytes())
                .map_err(|e| error(e.to_string(), output))?;
            // RENAME_EXCL is the Darwin equivalent of Linux RENAME_NOREPLACE.
            if unsafe {
                libc::renameatx_np(
                    libc::AT_FDCWD,
                    from.as_ptr(),
                    libc::AT_FDCWD,
                    to.as_ptr(),
                    libc::RENAME_EXCL,
                )
            } != 0
            {
                return Err(error(std::io::Error::last_os_error().to_string(), output));
            }
        }
        #[cfg(windows)]
        fs::rename(&self.path, output).map_err(|e| error(e.to_string(), output))?;
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        return Err(error(
            "Atomic image-directory publication is supported on Windows, Linux and macOS.",
            output,
        ));
        self.published = true;
        Ok(())
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        if !self.identity.matches_path(&self.path, true) {
            return;
        }
        // No recursive delete: foreign, unexpected, or replaced entries are retained.
        for entry in &mut self.entries {
            if entry.identity.matches_path(&entry.path, false) {
                let _ = fs::remove_file(&entry.path);
            }
            #[cfg(unix)]
            // Release each guard before removing the owned directory.
            entry.identity_guard.take();
        }
        if !self.identity.matches_path(&self.path, true) {
            return;
        }
        #[cfg(unix)]
        self.identity_guard.take();
        let _ = fs::remove_dir(&self.path);
    }
}

fn checked_source_size(source: &Source) -> Result<u64, AppError> {
    let size = fs::metadata(&source.path)
        .map_err(|cause| error(cause.to_string(), &source.path))?
        .len();
    if size == 0 || size > MAX_SOURCE_IMAGE_BYTES {
        return Err(error(
            format!(
                "Each source image must be between 1 byte and {} MiB.",
                MAX_SOURCE_IMAGE_BYTES / (1024 * 1024)
            ),
            &source.path,
        ));
    }
    Ok(size)
}

async fn copy_source(
    source: &Source,
    stage: &mut Directory,
    destination: PathBuf,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let expected = checked_source_size(source)?;
    let mut input = tokio::fs::File::open(&source.path)
        .await
        .map_err(|cause| error(cause.to_string(), &source.path))?;
    let output = stage.create_file(destination.clone())?;
    let mut output = tokio::fs::File::from_std(output);
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        check(cancel)?;
        let count = input
            .read(&mut buffer)
            .await
            .map_err(|cause| error(cause.to_string(), &source.path))?;
        if count == 0 {
            break;
        }
        copied = copied.saturating_add(count as u64);
        if copied > expected || copied > MAX_SOURCE_IMAGE_BYTES {
            return Err(error(
                "The source image changed size during staging.",
                &source.path,
            ));
        }
        output
            .write_all(&buffer[..count])
            .await
            .map_err(|cause| error(cause.to_string(), &destination))?;
        tokio::task::yield_now().await;
    }
    if copied != expected {
        return Err(error(
            "The source image changed size during staging.",
            &source.path,
        ));
    }
    output
        .flush()
        .await
        .map_err(|cause| error(cause.to_string(), &destination))?;
    output
        .sync_all()
        .await
        .map_err(|cause| error(cause.to_string(), &destination))?;
    check(cancel)?;
    source.verify()?;
    stage.verify_entries()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn plan_qtgmc_image(
    subtitles: &super::subtitles::Prepared,
    source: &Source,
    document: &super::metadata::Document,
    stream: u32,
    settings: &media_core::ImageProcessingSettings,
    ffmpeg: &Path,
    ffprobe: &Path,
    output: &Path,
    sequence: bool,
    cancel: &watch::Receiver<bool>,
    defer_vulkan: bool,
) -> Result<(super::qtgmc::Prepared, super::encode_plan::Plan, usize), AppError> {
    let settings = media_core::EncodeSettings {
        encoder: media_core::VideoEncoder::X265Standalone,
        video_stream_index: stream,
        framing: settings.framing,
        temporal: settings.temporal,
        trim: settings.trim,
        tone_map: settings.tone_map,
        av1an_filters: settings.filters.clone(),
        source_filters: settings.source_filters,
        ..Default::default()
    };
    let selected = document.selected(&[stream])?;
    let mut plan = super::encode_plan::Plan::build(document, &selected, &settings)?;
    let video = selected[0];
    let source_frames =
        super::encode::frame_scan(ffprobe, &source.path, &mut plan, video, false, cancel, None)
            .await?;
    let interval = settings
        .trim
        .map(|trim| super::trim::Interval::build(trim, source_frames, plan.fps_num, plan.fps_den))
        .transpose()?;
    let count = plan.activate_temporal(interval.map_or(source_frames, |value| value.frames))?;
    image_preflight(
        output,
        u32::try_from(count)
            .map_err(|_| error("Image frame count exceeds the contract range.", output))?,
        (plan.width, plan.height),
        sequence,
    )?;
    if !(defer_vulkan && plan.requires_vulkan_tone_map()) {
        plan.resolve_tone_map(ffmpeg, &source.path, document.duration(), cancel)
            .await?;
    }
    let request_id = id();
    let prepared = super::qtgmc::Prepared::build(
        &source.path,
        output,
        &request_id,
        &plan,
        interval.map(|value| {
            (
                value.source_start_frame as u32,
                value.source_end_frame as u32,
            )
        }),
        subtitles,
        ffmpeg,
        cancel,
    )
    .await?;
    Ok((prepared, plan, count))
}

#[allow(clippy::too_many_arguments)]
async fn prepare_qtgmc_image(
    subtitles: &super::subtitles::Prepared,
    source: &Source,
    document: &super::metadata::Document,
    stream: u32,
    settings: &media_core::ImageProcessingSettings,
    ffmpeg: &Path,
    ffprobe: &Path,
    output: &Path,
    sequence: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<super::qtgmc::Prepared, AppError> {
    let (prepared, plan, count) = plan_qtgmc_image(
        subtitles, source, document, stream, settings, ffmpeg, ffprobe, output, sequence, cancel,
        false,
    )
    .await?;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
    let sink = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    let execution = prepared
        .run(
            cancel.clone(),
            sender,
            &output.with_extension(format!("{}.qtgmc.log", id())),
        )
        .await;
    let _ = sink.await;
    execution?;
    prepared.validate(ffprobe, &plan, count, cancel).await?;
    source.verify()?;
    Ok(prepared)
}

fn image_preflight(
    output: &Path,
    frames: u32,
    size: (u32, u32),
    sequence: bool,
) -> Result<(), AppError> {
    let required = u64::from(size.0)
        .saturating_mul(u64::from(size.1))
        .saturating_mul(16)
        .saturating_add(65536)
        .saturating_mul(u64::from(frames))
        .saturating_add(16 * 1024 * 1024);
    let available =
        super::source_normalization::available(output.parent().expect("destination parent"))?;
    if required > available {
        return Err(error(
            format!(
                "Image export needs up to {} MiB of temporary space; {} MiB is available.",
                required.div_ceil(1024 * 1024),
                available / (1024 * 1024)
            ),
            output,
        ));
    }
    #[cfg(unix)]
    if sequence {
        let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
            return Err(error(std::io::Error::last_os_error().to_string(), output));
        }
        let mut limit = unsafe { limit.assume_init() };
        let required = u64::from(frames) + 256;
        if required > limit.rlim_cur {
            if required > limit.rlim_max {
                return Err(error(
                    format!(
                        "This export needs {required} file descriptors to retain ownership guards; the process hard limit is {}.",
                        limit.rlim_max
                    ),
                    output,
                ));
            }
            limit.rlim_cur = required;
            if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
                return Err(error(std::io::Error::last_os_error().to_string(), output));
            }
        }
    }
    let _ = sequence;
    Ok(())
}

pub async fn run_image_job(
    request: ImageRequest,
    cancel: watch::Receiver<bool>,
) -> Result<ImageResult, AppError> {
    run_image_job_inner(request, cancel, None).await
}

async fn run_image_job_inner(
    request: ImageRequest,
    cancel: watch::Receiver<bool>,
    review: Option<std::sync::Arc<super::command_edit::Session>>,
) -> Result<ImageResult, AppError> {
    check(&cancel)?;
    let ffmpeg = tool("ffmpeg").await?;
    let ffprobe = tool("ffprobe").await?;
    match request {
        request @ ImageRequest::ImplicitExport { .. } => {
            implicit::export(request, &ffmpeg, &ffprobe, &cancel, review.as_deref()).await
        }
        ImageRequest::ImportSequence {
            paths,
            frame_rate,
            output_path,
        } => {
            if paths.is_empty()
                || paths.len() > LIMIT as usize
                || frame_rate.denominator == 0
                || frame_rate.numerator == 0
                || u64::from(frame_rate.numerator) > 120 * u64::from(frame_rate.denominator)
            {
                return Err(error(
                    "Choose 1–10000 images and a positive frame rate up to 120 fps.",
                    Path::new(&output_path),
                ));
            }
            let output = destination(&output_path)?;
            if output
                .extension()
                .and_then(|e| e.to_str())
                .map(|v| v.eq_ignore_ascii_case("mkv"))
                != Some(true)
            {
                return Err(error(
                    "Image sequences are imported as a lossless .mkv file.",
                    &output,
                ));
            }
            let mut sources = Vec::new();
            let mut dimensions = None;
            let mut codec = None;
            let mut pixel_format = None;
            let mut stage = Directory::new(output.parent().unwrap())?;
            for (i, path) in paths.iter().enumerate() {
                check(&cancel)?;
                let source = Source::open(Path::new(path))?;
                checked_source_size(&source)?;
                let streams = inspect(&source.path, &ffprobe, &cancel).await?;
                let s = streams
                    .first()
                    .filter(|s| {
                        s.codec_type.as_deref() == Some("video")
                            && matches!(
                                s.codec_name.as_deref(),
                                Some("png" | "mjpeg" | "bmp" | "tiff" | "webp")
                            )
                    })
                    .ok_or_else(|| {
                        error(
                            "Choose single-frame PNG, JPEG, BMP, TIFF or WebP images.",
                            &source.path,
                        )
                    })?;
                if streams.len() != 1 || s.nb_read_frames.as_deref() != Some("1") {
                    return Err(error(
                        "Every sequence entry must contain exactly one image.",
                        &source.path,
                    ));
                }
                let size = geometry(s, &source.path)?;
                if dimensions.is_some_and(|v| v != size)
                    || codec
                        .as_ref()
                        .is_some_and(|v| Some(v) != s.codec_name.as_ref())
                    || pixel_format
                        .as_ref()
                        .is_some_and(|v| Some(v) != s.pix_fmt.as_ref())
                {
                    return Err(error(
                        "All sequence images must have matching dimensions, bit depths and image formats.",
                        &source.path,
                    ));
                }
                dimensions = Some(size);
                codec = s.codec_name.clone();
                pixel_format = s.pix_fmt.clone();
                let p = stage.path.join(format!("frame-{i:05}.img"));
                copy_source(&source, &mut stage, p, &cancel).await?;
                sources.push(source);
            }
            let temporary = Temporary::create(&output, &id())?;
            let mut a = args(&[
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-f",
                "image2",
                "-framerate",
                &format!("{}/{}", frame_rate.numerator, frame_rate.denominator),
                "-pattern_type",
                "sequence",
                "-start_number",
                "0",
                "-c:v",
                codec.as_deref().unwrap(),
                "-i",
            ]);
            // Every staged name is contiguous and private. image2 assigns the
            // requested input time base before sync can drop or duplicate
            // frames; concat inherits the image decoder's 25 fps time base.
            // A relative pattern plus an owned cwd also avoids Windows
            // extended-path prefixes being parsed as protocol authorities.
            a.push("frame-%05d.img".into());
            a.extend(args(&[
                "-map",
                "0:v:0",
                "-an",
                "-frames:v",
                &sources.len().to_string(),
                "-c:v",
                "ffv1",
                "-level",
                "3",
                "-g",
                "1",
                "-threads",
                "2",
            ]));
            a.push(temporary.path.as_os_str().to_owned());
            run(&ffmpeg, a, Some(stage.path.clone()), &cancel).await?;
            let streams = inspect(&temporary.path, &ffprobe, &cancel).await?;
            let first = streams
                .first()
                .ok_or_else(|| error("The sequence output has no video.", &output))?;
            let size = dimensions.unwrap();
            if first
                .nb_read_frames
                .as_deref()
                .and_then(|v| v.parse::<usize>().ok())
                != Some(sources.len())
                || geometry(first, &output)? != size
            {
                return Err(error(
                    "The sequence output failed frame-count or geometry validation.",
                    &output,
                ));
            }
            for source in &sources {
                source.verify()?;
            }
            check(&cancel)?;
            temporary.flush_nonempty_async().await?;
            temporary.publish(&output)?;
            Ok(ImageResult {output_path:output.to_string_lossy().into_owned(),frame_count:sources.len() as u32,width:size.0,height:size.1,notes:vec!["Images retain the displayed order. The imported FFV1 video can be used in Quick Convert or Batch.".into()]})
        }
        ImageRequest::Export {
            input_path,
            stream_index,
            start_frame,
            frame_count,
            full_source,
            format,
            output_path,
            width,
            pixel_format,
            quality,
            processing,
        } => {
            let output = destination(&output_path)?;
            if (!full_source
                && (frame_count == 0 || start_frame.checked_add(frame_count).is_none()))
                || width.is_some_and(|w| !(1..=16384).contains(&w))
            {
                return Err(error(
                    "Choose a positive frame interval or the full source, with an optional width from 1 through 16384.",
                    &output,
                ));
            }
            let sequence = matches!(format, ImageOutput::PngSequence | ImageOutput::JpegSequence);
            let png = matches!(format, ImageOutput::Png | ImageOutput::PngSequence);
            let jpeg = matches!(format, ImageOutput::Jpeg | ImageOutput::JpegSequence);
            let layout = pixel_format.unwrap_or(if jpeg {
                ImagePixelFormat::Yuv420p
            } else {
                ImagePixelFormat::Rgb24
            });
            let rgb = matches!(
                layout,
                ImagePixelFormat::Rgb24
                    | ImagePixelFormat::Rgba
                    | ImagePixelFormat::Rgb48
                    | ImagePixelFormat::Rgba64
            );
            if (png && !rgb)
                || (jpeg && rgb)
                || (format == ImageOutput::Gif && pixel_format.is_some())
            {
                return Err(error(
                    "PNG uses RGB24, RGBA, RGB48 or RGBA64; JPEG uses YUV420P, YUV422P or YUV444P; GIF owns its palette layout.",
                    &output,
                ));
            }
            if png && quality.is_some() {
                return Err(error(
                    "PNG has no quality control; omit image quality.",
                    &output,
                ));
            }
            let quality = quality.unwrap_or(if format == ImageOutput::Gif { 128 } else { 3 });
            if (format == ImageOutput::Gif && !(3..=256).contains(&quality))
                || (jpeg && !(1..=31).contains(&quality))
            {
                return Err(error(
                    "GIF palette size must be 3–256; JPEG quality must be 1–31.",
                    &output,
                ));
            }
            if matches!(format, ImageOutput::Png | ImageOutput::Jpeg)
                && (full_source || frame_count != 1)
            {
                return Err(error(
                    "Still-image output requires exactly one frame; choose an image sequence for the full video.",
                    &output,
                ));
            }
            let extension = if png {
                "png"
            } else if jpeg {
                "jpg"
            } else {
                "gif"
            };
            if !sequence
                && output
                    .extension()
                    .and_then(|v| v.to_str())
                    .is_none_or(|value| !value.eq_ignore_ascii_case(extension))
            {
                return Err(error(
                    format!("Choose a .{extension} destination."),
                    &output,
                ));
            }
            let source = Source::open(Path::new(&input_path))?;
            let document =
                super::probe(&ffprobe, &source.path, &cancel, Some(&[stream_index])).await?;
            let video = document
                .streams
                .iter()
                .find(|s| s.index == stream_index && s.codec_type.as_deref() == Some("video"))
                .ok_or_else(|| error("Select a source video stream.", &source.path))?;
            let mut processing = processing.unwrap_or_default();
            let metadata =
                metadata::Prepared::prepare(processing.metadata.take(), &ffprobe, &cancel).await?;
            if !full_source {
                if processing.trim.is_some() {
                    return Err(error(
                        "Use a single image interval: either the legacy frame range or shared trim settings.",
                        &output,
                    ));
                }
                processing.trim = Some(media_core::VideoTrim {
                    start_frame,
                    end_frame_exclusive: start_frame + frame_count,
                    time: None,
                });
            }
            // Legacy width remains a fallback for old saved requests.
            if processing.framing.resize.is_none() && processing.framing.resize_width.is_none() {
                processing.framing.resize_width = width;
            }
            let had_temporal = processing.temporal.is_some();
            let mut burn = subtitle::Burn::build(
                &source,
                &document,
                video,
                &processing,
                &ffmpeg,
                &ffprobe,
                &output,
                &cancel,
            )
            .await?;
            if (review.is_some() || processing.source_filters)
                && let Some(result) = review::export(
                    review.as_deref(),
                    &source,
                    &document,
                    video,
                    stream_index,
                    &processing,
                    &burn,
                    &metadata,
                    &ffmpeg,
                    &ffprobe,
                    &output,
                    format,
                    layout,
                    quality,
                    sequence,
                    full_source,
                    &cancel,
                )
                .await?
            {
                return Ok(result);
            }
            let prepared = if processing
                .temporal
                .is_some_and(|value| value.qtgmc.is_some())
            {
                Some(
                    prepare_qtgmc_image(
                        &burn.prepared,
                        &source,
                        &document,
                        stream_index,
                        &processing,
                        &ffmpeg,
                        &ffprobe,
                        &output,
                        sequence,
                        &cancel,
                    )
                    .await?,
                )
            } else {
                None
            };
            let prepared_document = if let Some(prepared) = &prepared {
                Some(super::probe(&ffprobe, &prepared.video.path, &cancel, Some(&[0])).await?)
            } else {
                None
            };
            let (picture_path, picture_stream, video) =
                if let (Some(prepared), Some(document)) = (&prepared, &prepared_document) {
                    processing = Default::default();
                    burn.prepared = super::subtitles::Prepared::empty();
                    (prepared.video.path.as_path(), 0, &document.streams[0])
                } else {
                    (source.path.as_path(), stream_index, video)
                };
            let plan = super::encode_plan::image_filter_plan(
                video,
                &processing,
                burn.prepared.text_filter(),
                &ffmpeg,
                picture_path,
                &cancel,
                false,
            )
            .await?;
            let reference = burn
                .scan(
                    &ffmpeg,
                    picture_path,
                    picture_stream,
                    &plan,
                    layout,
                    &cancel,
                )
                .await?;
            if reference.count == 0 {
                return Err(error(
                    "The selected image interval contains no frames.",
                    &source.path,
                ));
            }
            if !full_source && !had_temporal && reference.count != frame_count {
                return Err(error(
                    "The selected interval extends past the last source frame.",
                    &source.path,
                ));
            }
            let frame_count = reference.count;
            let size = reference.dimensions;
            if size != (plan.width, plan.height) {
                return Err(error(
                    "Picture processing produced geometry different from the shared plan.",
                    &source.path,
                ));
            }
            image_preflight(&output, frame_count, size, sequence)?;
            let mut stage = Directory::new(output.parent().unwrap())?;
            let destination = stage.path.join(if sequence {
                frame_pattern(extension, full_source)
            } else {
                format!("result.{extension}")
            });
            if sequence {
                for index in 1..=frame_count {
                    pause_checkpoint(&cancel).await?;
                    drop(stage.create_file(stage.path.join(frame_name(
                        index,
                        extension,
                        full_source,
                    )))?);
                }
            } else {
                drop(stage.create_file(destination.clone())?);
            }
            let mut palette = None;
            if format == ImageOutput::Gif {
                let mut file =
                    Temporary::create_extension(&output, &format!("{}-palette", id()), "png")?;
                let mut a = burn.decoder(picture_path, picture_stream, &plan, None, Some(quality));
                a.extend(args(&["-frames:v", "1", "-update", "1", "-threads", "2"]));
                a.push(file.path.as_os_str().to_owned());
                file.close_for_path_writer()?;
                let execution = run(&ffmpeg, a, burn.cwd(), &cancel).await;
                let rebound = file.reopen_after_path_writer();
                execution?;
                rebound?;
                file.flush_nonempty_async().await?;
                palette = Some(file);
            }
            let mut a = burn.decoder(
                picture_path,
                picture_stream,
                &plan,
                palette.as_ref().map(|file| file.path.as_path()),
                None,
            );
            metadata.append(&mut a)?;
            if palette.is_some() {
                a.extend(args(&["-loop", "0", "-gifflags", "-offsetting"]));
            } else {
                a.extend(args(&[
                    "-fps_mode",
                    "passthrough",
                    "-pix_fmt",
                    layout.ffmpeg(),
                ]));
                if jpeg {
                    a.extend(args(&[
                        "-c:v",
                        "mjpeg",
                        "-qmin",
                        "1",
                        "-q:v",
                        &quality.to_string(),
                    ]));
                } else {
                    a.extend(args(&["-c:v", "png", "-compression_level", "3"]));
                }
                if !sequence {
                    a.extend(args(&["-update", "1"]));
                }
            }
            a.extend(args(&[
                "-an",
                "-sn",
                "-frames:v",
                &frame_count.to_string(),
                "-threads",
                "2",
            ]));
            a.push(destination.as_os_str().to_owned());
            let execution = run(&ffmpeg, a, burn.cwd(), &cancel).await;
            stage.record_files()?;
            execution?;
            if stage.entries.len() != if sequence { frame_count as usize } else { 1 } {
                return Err(error(
                    "Image export produced an unexpected file count.",
                    &output,
                ));
            }
            verify_frames(
                &ffprobe,
                &destination,
                sequence,
                frame_count,
                size,
                if format == ImageOutput::Gif {
                    None
                } else {
                    Some(layout)
                },
                &cancel,
            )
            .await?;
            if png {
                let observed = scan_pixels(
                    &ffmpeg,
                    &destination,
                    0,
                    None,
                    layout,
                    sequence,
                    &[],
                    &cancel,
                )
                .await?;
                if observed != reference {
                    return Err(error(
                        "The PNG sequence changed decoded RGB/alpha samples, frame count or dimensions.",
                        &output,
                    ));
                }
            }
            pause_checkpoint(&cancel).await?;
            burn.verify()?;
            metadata.verify()?;
            source.verify()?;
            check(&cancel)?;
            if sequence {
                stage.publish(&output)?;
            } else {
                stage.verify_entries()?;
                fs::hard_link(&stage.entries[0].path, &output).map_err(|e| {
                    error(
                        format!("Cannot publish without overwriting an existing file: {e}"),
                        &output,
                    )
                })?;
            }
            Ok(ImageResult {
                output_path: output.to_string_lossy().into_owned(),
                frame_count,
                width: size.0,
                height: size.1,
                notes: if format == ImageOutput::Gif {
                    vec![format!(
                        "GIF uses up to {quality} colors, Floyd-Steinberg dithering and centisecond frame timing. Palette generation and rendering are separate bounded-memory passes."
                    )]
                } else {
                    Vec::new()
                },
            })
        }
    }
}

pub(crate) async fn run_image_job_with_review(
    request: ImageRequest,
    cancel: watch::Receiver<bool>,
    session: std::sync::Arc<super::command_edit::Session>,
) -> Result<ImageResult, AppError> {
    if matches!(request, ImageRequest::ImportSequence { .. }) {
        return Err(AppError::new(
            "COMMAND_EDIT_UNSUPPORTED",
            "Edit Arguments applies to fixed-image exports in Quick Convert.",
            None,
        ));
    }
    run_image_job_inner(request, cancel, Some(session)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "jesses-image-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[tokio::test]
    async fn staged_copy_observes_mid_copy_cancellation_and_removes_its_partial() {
        let root = fixture("copy-cancel");
        let source_path = root.join("large.bin");
        let source_file = File::create(&source_path).unwrap();
        source_file
            .set_len((COPY_BUFFER_BYTES as u64) * 64)
            .unwrap();
        drop(source_file);
        let source = Source::open(&source_path).unwrap();
        let mut stage = Directory::new(&root).unwrap();
        let stage_path = stage.path.clone();
        let destination = stage.path.join("copy.bin");
        let observed = destination.clone();
        let (owner, cancel) = watch::channel(false);
        let cancel_task = tokio::spawn(async move {
            loop {
                if fs::metadata(&observed)
                    .is_ok_and(|metadata| metadata.len() >= COPY_BUFFER_BYTES as u64)
                {
                    owner.send_replace(true);
                    break;
                }
                tokio::task::yield_now().await;
            }
        });
        let failure = copy_source(&source, &mut stage, destination.clone(), &cancel)
            .await
            .unwrap_err();
        cancel_task.await.unwrap();
        assert_eq!(failure.code, "JOB_CANCELED");
        let partial = fs::metadata(&destination).unwrap().len();
        assert!(partial >= COPY_BUFFER_BYTES as u64);
        assert!(partial < (COPY_BUFFER_BYTES as u64) * 64);
        drop(stage);
        assert!(!stage_path.exists());
        assert_eq!(
            fs::metadata(&source_path).unwrap().len(),
            (COPY_BUFFER_BYTES as u64) * 64
        );
        drop(source);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_retains_a_same_name_foreign_replacement() {
        let root = fixture("foreign-replacement");
        let mut stage = Directory::new(&root).unwrap();
        let stage_path = stage.path.clone();
        let entry_path = stage.path.join("result.png");
        let mut owned = stage.create_file(entry_path.clone()).unwrap();
        owned.write_all(b"owned").unwrap();
        owned.sync_all().unwrap();
        drop(owned);
        fs::remove_file(&entry_path).unwrap();
        fs::write(&entry_path, b"foreign").unwrap();
        drop(stage);
        assert_eq!(fs::read(&entry_path).unwrap(), b"foreign");
        fs::remove_file(entry_path).unwrap();
        fs::remove_dir(stage_path).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg"]
    async fn active_image_tool_cancellation_keeps_job_canceled_code() {
        let root = fixture("active-cancel");
        let output = root.join("active.mkv");
        let ffmpeg = tool("ffmpeg").await.unwrap();
        let (owner, cancel) = watch::channel(false);
        let observed = output.clone();
        let task = tokio::spawn(async move {
            let mut command = args(&[
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-re",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=64x48:r=30",
                "-t",
                "30",
                "-c:v",
                "ffv1",
            ]);
            command.push(output.as_os_str().to_owned());
            run(&ffmpeg, command, None, &cancel).await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !observed.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("FFmpeg did not start its output");
        owner.send_replace(true);
        let failure = task.await.unwrap().unwrap_err();
        assert_eq!(failure.code, "JOB_CANCELED");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg PNG encoder"]
    async fn active_streaming_image_scan_keeps_job_canceled_code() {
        let root = fixture("active-scan-cancel");
        let marker = root.join("first.png");
        let observed = marker.clone();
        let ffmpeg = tool("ffmpeg").await.unwrap();
        let (owner, cancel) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut command = args(&[
                "-v",
                "error",
                "-nostdin",
                "-re",
                "-f",
                "lavfi",
                "-i",
                "color=black:size=64x48:rate=4:duration=30",
                "-map",
                "0:v:0",
                "-frames:v",
                "1",
                "-update",
                "1",
                "-threads",
                "2",
            ]);
            command.push(marker.into_os_string());
            command.extend(args(&[
                "-map", "0:v:0", "-c:v", "rawvideo", "-threads", "2", "-f", "framemd5", "-",
            ]));
            scan_pixel_command(&ffmpeg, command, None, &ffmpeg, &cancel).await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !observed.exists() {
                assert!(
                    !task.is_finished(),
                    "Image scan exited before its first frame"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("FFmpeg scan did not produce its first frame");
        assert!(
            !task.is_finished(),
            "Image scan must still be active at cancellation"
        );
        owner.send_replace(true);
        let failure = task.await.unwrap().unwrap_err();
        assert_eq!(failure.code, "JOB_CANCELED");
        fs::remove_dir_all(root).unwrap();
    }
}
