//! Image sequences use explicit order and a new, atomically published destination.
use super::files::{Source, Temporary};
use crate::{
    discovery::find_executable,
    supervisor::{self, CommandSpec},
};
use media_core::{AppError, ImageOutput, ImageRequest, ImageResult};
use serde::Deserialize;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::watch,
};

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
        Duration::from_secs(3600),
    )
    .await
    .map_err(|e| error(e.to_string(), program))?;
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

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
}
#[derive(Deserialize)]
struct Stream {
    index: u32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    nb_read_frames: Option<String>,
    color_transfer: Option<String>,
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
    identity_guard: Option<File>,
}

/// Holds a fresh directory reservation and removes only entries whose file
/// identities still match the files created by this job.
struct Directory {
    path: PathBuf,
    identity: Identity,
    #[cfg(unix)]
    identity_guard: Option<File>,
    entries: Vec<Entry>,
    published: bool,
}
impl Directory {
    fn new(parent: &Path) -> Result<Self, AppError> {
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

    fn create_file(&mut self, path: PathBuf) -> Result<File, AppError> {
        self.verify_directory()?;
        if path.parent() != Some(self.path.as_path())
            || self.entries.iter().any(|entry| entry.path == path)
        {
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
        let identity_guard = file
            .try_clone()
            .map_err(|cause| error(cause.to_string(), &path))?;
        self.entries.push(Entry {
            path,
            identity,
            identity_guard: Some(identity_guard),
        });
        Ok(file)
    }

    fn entry_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.entries.iter().map(|entry| &entry.path)
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
            if !self.entries.iter().any(|entry| entry.path == p) {
                return Err(error(
                    "Unexpected file in image output; it was retained for review.",
                    &p,
                ));
            }
        }
        Ok(())
    }
    fn publish(&mut self, output: &Path) -> Result<(), AppError> {
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
        #[cfg(windows)]
        fs::rename(&self.path, output).map_err(|e| error(e.to_string(), output))?;
        #[cfg(not(any(windows, target_os = "linux")))]
        return Err(error(
            "Atomic image-directory publication is supported on Windows and Linux.",
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
            // On Windows, a deletion may finish only after the last handle is
            // closed. Release each guard before removing the owned directory.
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

pub async fn run_image_job(
    request: ImageRequest,
    cancel: watch::Receiver<bool>,
) -> Result<ImageResult, AppError> {
    check(&cancel)?;
    let ffmpeg = tool("ffmpeg").await?;
    let ffprobe = tool("ffprobe").await?;
    match request {
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
            let manifest = stage.path.join("frames.ffconcat");
            let mut text = "ffconcat version 1.0\n".to_owned();
            for i in 0..sources.len() {
                text.push_str(&format!(
                    "file frame-{i:05}.img\nduration {:.12}\n",
                    f64::from(frame_rate.denominator) / f64::from(frame_rate.numerator)
                ));
            }
            let mut manifest_file = stage.create_file(manifest.clone())?;
            manifest_file
                .write_all(text.as_bytes())
                .and_then(|_| manifest_file.sync_all())
                .map_err(|e| error(e.to_string(), &manifest))?;
            let temporary = Temporary::create(&output, &id())?;
            let mut a = args(&[
                "-v", "error", "-nostdin", "-y", "-f", "concat", "-safe", "1", "-i",
            ]);
            // A relative manifest plus an owned cwd avoids FFmpeg treating a
            // Windows extended-path prefix as a concat URL authority.
            a.push("frames.ffconcat".into());
            a.extend(args(&[
                "-map",
                "0:v:0",
                "-an",
                "-r",
                &format!("{}/{}", frame_rate.numerator, frame_rate.denominator),
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
            format,
            output_path,
            width,
        } => {
            let output = destination(&output_path)?;
            if frame_count == 0
                || frame_count > LIMIT
                || start_frame.checked_add(frame_count).is_none()
                || width.is_some_and(|w| !(1..=8192).contains(&w))
            {
                return Err(error(
                    "Choose 1–10000 frames and an optional width from 1 to 8192 pixels.",
                    &output,
                ));
            }
            let sequence = matches!(format, ImageOutput::PngSequence | ImageOutput::JpegSequence);
            if matches!(format, ImageOutput::Png | ImageOutput::Jpeg) && frame_count != 1 {
                return Err(error(
                    "Still-image output requires exactly one frame.",
                    &output,
                ));
            }
            let extension = match format {
                ImageOutput::Png | ImageOutput::PngSequence => "png",
                ImageOutput::Jpeg | ImageOutput::JpegSequence => "jpg",
                ImageOutput::Gif => "gif",
            };
            if !sequence
                && output
                    .extension()
                    .and_then(|v| v.to_str())
                    .map(|v| v.eq_ignore_ascii_case(extension))
                    != Some(true)
            {
                return Err(error(
                    format!("Choose a .{extension} destination."),
                    &output,
                ));
            }
            let source = Source::open(Path::new(&input_path))?;
            let streams = inspect(&source.path, &ffprobe, &cancel).await?;
            let s = streams
                .iter()
                .find(|s| s.index == stream_index && s.codec_type.as_deref() == Some("video"))
                .ok_or_else(|| error("Select a video stream.", &source.path))?;
            let (w, h) = geometry(s, &source.path)?;
            if matches!(
                s.color_transfer.as_deref(),
                Some("smpte2084" | "arib-std-b67")
            ) {
                return Err(error(
                    "Convert HDR to SDR in Quick Convert before exporting display images.",
                    &source.path,
                ));
            }
            let total = s
                .nb_read_frames
                .as_deref()
                .and_then(|v| v.parse::<u32>().ok())
                .ok_or_else(|| error("Could not verify the source frame count.", &source.path))?;
            if start_frame + frame_count > total {
                return Err(error(
                    "The selected interval extends past the last source frame.",
                    &source.path,
                ));
            }
            let mut stage = Directory::new(output.parent().unwrap())?;
            let mut filter = format!(
                "trim=start_frame={start_frame}:end_frame={},setpts=PTS-STARTPTS",
                start_frame + frame_count
            );
            if let Some(target) = width {
                filter.push_str(&format!(",scale={target}:-1:flags=lanczos"));
            }
            let destination = stage.path.join(if sequence {
                format!("frame-%06d.{extension}")
            } else {
                format!("result.{extension}")
            });
            let staged_paths = if sequence {
                (1..=frame_count)
                    .map(|i| stage.path.join(format!("frame-{i:06}.{extension}")))
                    .collect::<Vec<_>>()
            } else {
                vec![destination.clone()]
            };
            for path in staged_paths {
                drop(stage.create_file(path)?);
            }
            let mut a = args(&["-v", "error", "-nostdin", "-y", "-i"]);
            a.push(source.path.as_os_str().to_owned());
            if format == ImageOutput::Gif {
                filter.push_str(",split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a");
                a.extend(args(&[
                    "-filter_complex",
                    &format!("[0:{stream_index}]{filter}[gif]"),
                    "-map",
                    "[gif]",
                    "-loop",
                    "0",
                ]));
            } else {
                a.extend(args(&[
                    "-map",
                    &format!("0:{stream_index}"),
                    "-vf",
                    &filter,
                    "-fps_mode",
                    "passthrough",
                ]));
                if extension == "jpg" {
                    a.extend(args(&["-q:v", "2"]));
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
            let executed = run(&ffmpeg, a, None, &cancel).await;
            stage.record_files()?;
            executed?;
            let expected = if sequence { frame_count as usize } else { 1 };
            if stage.entry_paths().filter(|p| p.is_file()).count() != expected {
                return Err(error(
                    "Image export produced an unexpected number of files.",
                    &output,
                ));
            }
            let mut result_size = None;
            for p in stage.entry_paths() {
                let verified = inspect(p, &ffprobe, &cancel).await?;
                let v = verified
                    .first()
                    .ok_or_else(|| error("An exported image cannot be decoded.", p))?;
                let expected_frames = if format == ImageOutput::Gif {
                    frame_count
                } else {
                    1
                };
                if v.nb_read_frames
                    .as_deref()
                    .and_then(|v| v.parse::<u32>().ok())
                    != Some(expected_frames)
                {
                    return Err(error(
                        "The exported image failed decoded frame-count validation.",
                        p,
                    ));
                }
                let size = geometry(v, p)?;
                if result_size.is_some_and(|v| v != size) {
                    return Err(error("Exported images have inconsistent dimensions.", p));
                }
                result_size = Some(size);
            }
            source.verify()?;
            check(&cancel)?;
            if sequence {
                stage.publish(&output)?;
            } else {
                stage.verify_entries()?;
                fs::hard_link(&stage.entries[0].path, &output).map_err(|e| {
                    error(
                        format!(
                            "Could not publish the image without overwriting an existing file: {e}"
                        ),
                        &output,
                    )
                })?;
            }
            let (w, h) = result_size.unwrap_or((w, h));
            Ok(ImageResult {
                output_path: output.to_string_lossy().into_owned(),
                frame_count,
                width: w,
                height: h,
                notes: if format == ImageOutput::Gif {
                    vec!["GIF uses a 256-color palette and centisecond frame timing.".into()]
                } else {
                    vec![]
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
