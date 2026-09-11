use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::SystemTime,
};

use media_core::{AppError, RemuxRequest};

pub(super) fn error(code: &str, message: impl Into<String>, path: &Path) -> AppError {
    AppError::new(code, message, Some(path.to_string_lossy().into_owned()))
}

#[derive(Debug, PartialEq, Eq)]
struct Fingerprint {
    length: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl Fingerprint {
    fn read(metadata: fs::Metadata) -> std::io::Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified()?,
            created: metadata.created().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}

pub(super) struct Source {
    pub path: PathBuf,
    // Windows share mode denies writes and deletion for the whole job.
    file: File,
    fingerprint: Fingerprint,
}

impl Source {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let path = fs::canonicalize(path).map_err(|e| {
            error(
                "FILE_UNREADABLE",
                format!("The source could not be opened: {e}"),
                path,
            )
        })?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1); // FILE_SHARE_READ
        }
        let file = options.open(&path).map_err(|e| {
            error(
                "FILE_UNREADABLE",
                format!("The source could not be opened safely: {e}"),
                &path,
            )
        })?;
        let metadata = file
            .metadata()
            .map_err(|e| error("FILE_UNREADABLE", e.to_string(), &path))?;
        if !metadata.is_file() {
            return Err(error(
                "NOT_A_FILE",
                "Choose an individual local media file.",
                &path,
            ));
        }
        let fingerprint = Fingerprint::read(metadata)
            .map_err(|e| error("FILE_UNREADABLE", e.to_string(), &path))?;
        Ok(Self {
            path,
            file,
            fingerprint,
        })
    }

    pub fn verify(&self) -> Result<(), AppError> {
        let inspect = || -> std::io::Result<bool> {
            #[cfg(windows)]
            if windows_file_id(&self.file)? != windows_file_id(&File::open(&self.path)?)? {
                return Ok(false);
            }
            Ok(
                Fingerprint::read(self.file.metadata()?)? == self.fingerprint
                    && Fingerprint::read(fs::metadata(&self.path)?)? == self.fingerprint,
            )
        };
        match inspect() {
            Ok(true) => Ok(()),
            _ => Err(error(
                "SOURCE_CHANGED",
                "The source changed during the job. The output was not published; import the source again.",
                &self.path,
            )),
        }
    }
}

pub(super) fn validate_request(request: &RemuxRequest) -> Result<(), AppError> {
    for text in [&request.input_path, &request.output_path] {
        if text.contains('\0') || !Path::new(text).is_absolute() {
            return Err(error(
                "INVALID_PATH",
                "Use absolute local file paths.",
                Path::new(text),
            ));
        }
    }
    let output = Path::new(&request.output_path);
    if !output
        .extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("mkv"))
    {
        return Err(error(
            "OUTPUT_FORMAT_UNSUPPORTED",
            "This workflow writes Matroska files. Choose an output ending in .mkv.",
            output,
        ));
    }
    let mut indices = std::collections::HashSet::new();
    if request.stream_indices.is_empty()
        || request
            .stream_indices
            .iter()
            .any(|index| !indices.insert(*index))
    {
        return Err(error(
            "STREAM_SELECTION_INVALID",
            "Select at least one stream, without duplicate indices.",
            Path::new(&request.input_path),
        ));
    }
    Ok(())
}

pub(super) fn output_path(request: &RemuxRequest, source: &Source) -> Result<PathBuf, AppError> {
    let output = Path::new(&request.output_path);
    let parent = output
        .parent()
        .ok_or_else(|| error("INVALID_OUTPUT", "Choose an output folder.", output))?;
    let parent = fs::canonicalize(parent).map_err(|e| {
        error(
            "INVALID_OUTPUT",
            format!("The output folder could not be accessed: {e}"),
            parent,
        )
    })?;
    if !parent.is_dir() {
        return Err(error(
            "INVALID_OUTPUT",
            "The output parent must be an existing folder.",
            &parent,
        ));
    }
    let name = output
        .file_name()
        .ok_or_else(|| error("INVALID_OUTPUT", "Choose an output filename.", output))?;
    let output = parent.join(name);
    let source_text = source.path.to_string_lossy();
    let output_text = output.to_string_lossy();
    let same_path = if cfg!(windows) {
        source_text.eq_ignore_ascii_case(&output_text)
    } else {
        source.path == output
    };
    if same_path {
        return Err(error(
            "SOURCE_OUTPUT_COLLISION",
            "The destination must differ from the source file.",
            &output,
        ));
    }
    ensure_absent(&output)?;
    Ok(output)
}

pub(super) fn ensure_absent(output: &Path) -> Result<(), AppError> {
    // symlink_metadata also sees dangling symlinks. Never follow or overwrite one.
    match fs::symlink_metadata(output) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(error(
            "OUTPUT_EXISTS",
            "The destination already exists. Choose a new filename; existing files are never replaced.",
            output,
        )),
        Err(e) => Err(error(
            "OUTPUT_UNREADABLE",
            format!("The destination could not be checked: {e}"),
            output,
        )),
    }
}

pub(super) struct Temporary {
    pub path: PathBuf,
    file: Option<File>,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(windows)]
    windows_identity: (u32, u32, u32),
}

impl Temporary {
    pub fn create(output: &Path, id: &str) -> Result<Self, AppError> {
        Self::create_extension(output, id, "mkv")
    }

    pub fn create_ivf(output: &Path, id: &str) -> Result<Self, AppError> {
        Self::create_extension(output, id, "ivf")
    }

    fn create_extension(output: &Path, id: &str, extension: &str) -> Result<Self, AppError> {
        let path = output
            .parent()
            .expect("validated output parent")
            .join(format!(".jesses-{id}.partial.{extension}"));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1 | 2); // FILE_SHARE_READ | FILE_SHARE_WRITE; deny replacement/deletion.
        }
        let file = options.open(&path).map_err(|e| {
            error(
                "OUTPUT_CREATE_FAILED",
                format!("The temporary output could not be reserved: {e}"),
                &path,
            )
        })?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            let metadata = file
                .metadata()
                .map_err(|e| error("OUTPUT_CREATE_FAILED", e.to_string(), &path))?;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(windows)]
        let windows_identity = windows_file_id(&file)
            .map_err(|e| error("OUTPUT_CREATE_FAILED", e.to_string(), &path))?;
        Ok(Self {
            path,
            file: Some(file),
            #[cfg(unix)]
            identity,
            #[cfg(windows)]
            windows_identity,
        })
    }

    #[cfg(test)]
    pub fn flush_nonempty(&self) -> Result<(), AppError> {
        let file = self.file.as_ref().expect("owned temporary handle");
        let metadata = file
            .metadata()
            .map_err(|e| error("OUTPUT_UNREADABLE", e.to_string(), &self.path))?;
        if metadata.len() == 0 {
            return Err(error(
                "OUTPUT_EMPTY",
                "The tool produced an empty output. Nothing was published.",
                &self.path,
            ));
        }
        self.verify_identity()?;
        file.sync_all().map_err(|e| {
            error(
                "OUTPUT_FLUSH_FAILED",
                format!("The output could not be flushed to disk: {e}"),
                &self.path,
            )
        })
    }

    pub async fn flush_nonempty_async(&self) -> Result<(), AppError> {
        let file = self
            .file
            .as_ref()
            .expect("owned temporary handle")
            .try_clone()
            .map_err(|e| error("OUTPUT_UNREADABLE", e.to_string(), &self.path))?;
        let path = self.path.clone();
        self.verify_identity()?;
        tokio::task::spawn_blocking(move || {
            if file
                .metadata()
                .map_err(|e| error("OUTPUT_UNREADABLE", e.to_string(), &path))?
                .len()
                == 0
            {
                return Err(error(
                    "OUTPUT_EMPTY",
                    "The tool produced an empty output. Nothing was published.",
                    &path,
                ));
            }
            file.sync_all().map_err(|e| {
                error(
                    "OUTPUT_FLUSH_FAILED",
                    format!("The output could not be flushed to disk: {e}"),
                    &path,
                )
            })
        })
        .await
        .map_err(|e| error("OUTPUT_FLUSH_FAILED", e.to_string(), &self.path))?
    }

    pub fn clone_file(&self) -> Result<File, AppError> {
        self.verify_identity()?;
        self.file
            .as_ref()
            .expect("owned temporary handle")
            .try_clone()
            .map_err(|e| error("OUTPUT_UNREADABLE", e.to_string(), &self.path))
    }

    fn verify_identity(&self) -> Result<(), AppError> {
        let metadata = fs::symlink_metadata(&self.path)
            .map_err(|e| error("OUTPUT_CHANGED", e.to_string(), &self.path))?;
        if !metadata.is_file() {
            return Err(error(
                "OUTPUT_CHANGED",
                "The temporary output was replaced.",
                &self.path,
            ));
        }
        #[cfg(windows)]
        {
            let opened = File::open(&self.path)
                .and_then(|file| windows_file_id(&file))
                .map_err(|e| error("OUTPUT_CHANGED", e.to_string(), &self.path))?;
            if opened != self.windows_identity {
                return Err(error(
                    "OUTPUT_CHANGED",
                    "The temporary output was replaced.",
                    &self.path,
                ));
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (metadata.dev(), metadata.ino()) != self.identity {
                return Err(error(
                    "OUTPUT_CHANGED",
                    "The temporary output was replaced.",
                    &self.path,
                ));
            }
        }
        Ok(())
    }

    /// Atomic no-clobber publication on the same filesystem. A filesystem without
    /// hard links fails closed; a copy fallback could expose an incomplete result.
    pub fn publish(&self, output: &Path) -> Result<(), AppError> {
        self.verify_identity()?;
        ensure_absent(output)?;
        fs::hard_link(&self.path, output).map_err(|e| {
            let code = if e.kind() == std::io::ErrorKind::AlreadyExists { "OUTPUT_EXISTS" } else { "OUTPUT_FINALIZE_FAILED" };
            error(code, format!("The output could not be published without replacing files: {e}. The output filesystem must support hard links."), output)
        })
    }

    pub fn cleanup(&mut self) -> Result<(), AppError> {
        self.verify_identity()?;
        self.file.take();
        #[cfg(windows)]
        return windows_delete_owned(&self.path, self.windows_identity).map_err(|e| {
            error(
                "OUTPUT_CLEANUP_FAILED",
                format!("The owned temporary output could not be removed safely: {e}"),
                &self.path,
            )
        });
        #[cfg(not(windows))]
        fs::remove_file(&self.path).map_err(|e| {
            error(
                "OUTPUT_CLEANUP_FAILED",
                format!("The owned temporary output could not be removed: {e}"),
                &self.path,
            )
        })
    }
}

#[cfg(windows)]
pub(super) fn windows_file_id(file: &File) -> std::io::Result<(u32, u32, u32)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: file owns a live OS handle and info is a correctly sized output buffer.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful API call initialized the entire structure.
    let info = unsafe { info.assume_init() };
    Ok((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

#[cfg(windows)]
pub(super) fn windows_delete_owned(path: &Path, expected: (u32, u32, u32)) -> std::io::Result<()> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };
    let file = OpenOptions::new()
        .access_mode(0x0001_0000 | 0x80) // DELETE | FILE_READ_ATTRIBUTES
        .share_mode(1 | 2) // deny replacement while checking identity and deleting
        .custom_flags(0x0020_0000) // FILE_FLAG_OPEN_REPARSE_POINT: do not follow symlinks
        .open(path)?;
    if windows_file_id(&file)? != expected {
        return Err(std::io::Error::other(
            "The temporary pathname now names a different file; it was preserved.",
        ));
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: file has DELETE access and the buffer has the documented layout/size.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(()) // Closing this exact handle removes only this verified directory link.
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if self.file.is_some() {
            let _ = self.cleanup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("jesses-files-{}-{nonce}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn existing_destinations_and_source_are_untouched() {
        let dir = Fixture::new();
        let input = dir.0.join("source.mkv");
        fs::write(&input, "original source").unwrap();
        let source = Source::open(&input).unwrap();
        let mut request = RemuxRequest {
            input_path: input.to_string_lossy().into(),
            output_path: input.to_string_lossy().into(),
            stream_indices: vec![0],
        };
        assert_eq!(
            output_path(&request, &source).unwrap_err().code,
            "SOURCE_OUTPUT_COLLISION"
        );
        let output = dir.0.join("destination.mkv");
        request.output_path = output.to_string_lossy().into();
        let resolved = output_path(&request, &source).unwrap();
        let mut temp = Temporary::create(&resolved, "test").unwrap();
        temp.file
            .as_mut()
            .unwrap()
            .write_all(b"remux bytes")
            .unwrap();
        fs::write(&output, b"unrelated existing output").unwrap();
        assert_eq!(temp.publish(&output).unwrap_err().code, "OUTPUT_EXISTS");
        temp.cleanup().unwrap();
        source.verify().unwrap();
        assert_eq!(fs::read(&input).unwrap(), b"original source");
        assert_eq!(fs::read(&output).unwrap(), b"unrelated existing output");
        let source_alias = dir.0.join("hardlink-to-source.mkv");
        fs::hard_link(&input, &source_alias).unwrap();
        request.output_path = source_alias.to_string_lossy().into_owned();
        assert_eq!(
            output_path(&request, &source).unwrap_err().code,
            "OUTPUT_EXISTS"
        );
        assert_eq!(fs::read(&source_alias).unwrap(), b"original source");
        source.verify().unwrap();
    }

    #[test]
    fn empty_outputs_never_publish_and_owned_temporary_is_cleaned() {
        let dir = Fixture::new();
        let output = dir.0.join("destination.mkv");
        let temp = Temporary::create(&output, "empty").unwrap();
        let path = temp.path.clone();
        assert_eq!(temp.flush_nonempty().unwrap_err().code, "OUTPUT_EMPTY");
        drop(temp);
        assert!(!path.exists());
        assert!(!output.exists());
    }

    #[test]
    fn publishes_nonempty_output_without_changing_source() {
        let dir = Fixture::new();
        let output = dir.0.join("destination.mkv");
        let mut temp = Temporary::create(&output, "success").unwrap();
        temp.file
            .as_mut()
            .unwrap()
            .write_all(b"verified media")
            .unwrap();
        temp.flush_nonempty().unwrap();
        temp.publish(&output).unwrap();
        temp.cleanup().unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"verified media");
    }

    #[cfg(windows)]
    #[test]
    fn windows_cleanup_preserves_replaced_names_and_reports_locked_files() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = Fixture::new();
        let original = dir.0.join("original.txt");
        fs::write(&original, "original").unwrap();
        let file = File::open(&original).unwrap();
        let identity = windows_file_id(&file).unwrap();
        drop(file);
        let replacement = dir.0.join("replacement.txt");
        fs::write(&replacement, "unrelated replacement").unwrap();
        assert!(windows_delete_owned(&replacement, identity).is_err());
        assert_eq!(fs::read(&replacement).unwrap(), b"unrelated replacement");

        let output = dir.0.join("output.mkv");
        let mut temp = Temporary::create(&output, "locked").unwrap();
        let path = temp.path.clone();
        let identity = temp.windows_identity;
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(&path)
            .unwrap();
        assert_eq!(temp.cleanup().unwrap_err().code, "OUTPUT_CLEANUP_FAILED");
        assert!(path.exists());
        drop(lock);
        windows_delete_owned(&path, identity).unwrap();
        assert!(!path.exists());

        let source = Source::open(&original).unwrap();
        assert!(OpenOptions::new().write(true).open(&original).is_err());
        source.verify().unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"original");
    }
}
