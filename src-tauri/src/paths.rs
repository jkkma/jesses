//! Mutable application state is independent of installed resources. Portable
//! storage is selected only by an explicit, versioned marker beside the launcher.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const PORTABLE_MARKER: &str = "jesses.portable";
const PORTABLE_DIRECTORY: &str = "jesses-data";
static PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageMode {
    Installed,
    Portable,
}

impl StorageMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Portable => "portable",
        }
    }
}

/// Supply Tauri's platform paths without changing their existing layout.
#[derive(Debug, Clone)]
pub struct InstalledPaths {
    pub resource_dir: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub mode: StorageMode,
    pub resource_dir: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
    portable_root: Option<PathBuf>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn redirected(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// An APPIMAGE variable inherited from another application must not change this
/// application's history location. Honor it only when this executable is inside
/// the corresponding APPDIR mount. No paths are created or modified here.
#[cfg(any(target_os = "linux", test))]
pub fn launcher_path(
    executable: &Path,
    appimage: Option<&Path>,
    appdir: Option<&Path>,
) -> io::Result<PathBuf> {
    let Some(directory) = appdir.filter(|path| path.is_absolute() && path.is_dir()) else {
        return Ok(executable.to_path_buf());
    };
    let directory = directory.canonicalize()?;
    let executable_location = executable.canonicalize()?;
    if !executable_location.starts_with(&directory) || executable_location == directory {
        return Ok(executable.to_path_buf());
    }
    let Some(image) = appimage.filter(|path| path.is_absolute() && path.is_file()) else {
        return Err(invalid(
            "This AppImage mount has no valid absolute APPIMAGE launcher path. Application data was not changed.",
        ));
    };
    Ok(image.to_path_buf())
}

impl AppPaths {
    /// Resolve without creating directories, moving history, or touching media.
    /// For an AppImage, pass its original absolute launcher path rather than the
    /// executable inside its temporary read-only mount.
    pub fn resolve(executable: &Path, installed: InstalledPaths) -> io::Result<Self> {
        for path in [
            executable,
            &installed.resource_dir,
            &installed.config_dir,
            &installed.data_dir,
            &installed.cache_dir,
            &installed.log_dir,
        ] {
            if !path.is_absolute() {
                return Err(invalid(format!(
                    "Application locations must be absolute: {}",
                    path.display()
                )));
            }
        }
        let parent = executable
            .parent()
            .ok_or_else(|| invalid("The application executable has no parent directory."))?;
        let marker = parent.join(PORTABLE_MARKER);
        let marker_metadata = match fs::symlink_metadata(&marker) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "Cannot inspect portable marker {}: {error}",
                        marker.display()
                    ),
                ));
            }
        };
        if let Some(metadata) = marker_metadata {
            if !metadata.is_file() || redirected(&metadata) || metadata.len() > 32 {
                return Err(invalid(format!(
                    "Portable marker {} must be a regular file containing version 1.",
                    marker.display()
                )));
            }
            let mut version = String::new();
            File::open(&marker)?.take(33).read_to_string(&mut version)?;
            if version.trim() != "1" {
                return Err(invalid(format!(
                    "Unsupported portable marker {}. Expected version 1; existing data was not changed.",
                    marker.display()
                )));
            }
            let root = parent.join(PORTABLE_DIRECTORY);
            return Ok(Self {
                mode: StorageMode::Portable,
                resource_dir: installed.resource_dir,
                config_dir: root.join("config"),
                data_dir: root.join("data"),
                cache_dir: root.join("cache"),
                log_dir: root.join("logs"),
                portable_root: Some(root),
            });
        }
        Ok(Self {
            mode: StorageMode::Installed,
            resource_dir: installed.resource_dir,
            config_dir: installed.config_dir,
            data_dir: installed.data_dir,
            cache_dir: installed.cache_dir,
            log_dir: installed.log_dir,
            portable_root: None,
        })
    }

    pub fn history_dir(&self) -> PathBuf {
        self.data_dir.join("jobs")
    }

    pub fn job_log_dir(&self) -> PathBuf {
        self.log_dir.join("jobs")
    }

    /// Create and check only the chosen mutable locations. An unwritable portable
    /// directory is an error, never permission to use a different history store.
    pub fn prepare(&self) -> io::Result<()> {
        if let Some(root) = &self.portable_root {
            ensure_portable_directory(root)?;
        }
        for directory in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.log_dir,
        ] {
            if self.portable_root.is_some() {
                ensure_portable_directory(directory)?;
            } else {
                fs::create_dir_all(directory)?;
            }
            check_writable(directory)?;
        }
        Ok(())
    }
}

fn ensure_portable_directory(directory: &Path) -> io::Result<()> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.is_dir() && !redirected(&metadata) => Ok(()),
        Ok(_) => Err(invalid(format!(
            "Portable data must use an ordinary directory, without a link or junction: {}",
            directory.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(directory),
        Err(error) => Err(error),
    }
}

fn check_writable(directory: &Path) -> io::Result<()> {
    let sequence = PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let probe = directory.join(format!(
        ".jesses-write-check-{}-{nonce}-{sequence}",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "The selected application data directory is not writable: {}: {error}",
                    directory.display()
                ),
            )
        })?;
    let written = file.write_all(b"jesses\n").and_then(|()| file.sync_all());
    drop(file);
    let removed = fs::remove_file(&probe);
    written.and(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root =
                std::env::temp_dir().join(format!("jesses-paths-{}-{nonce}", std::process::id()));
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("application")).unwrap();
            Self(root)
        }

        fn executable(&self) -> PathBuf {
            self.0.join("application/jesses.exe")
        }

        fn defaults(&self) -> InstalledPaths {
            InstalledPaths {
                resource_dir: self.0.join("application"),
                config_dir: self.0.join("profile/config"),
                data_dir: self.0.join("profile/data"),
                cache_dir: self.0.join("profile/cache"),
                log_dir: self.0.join("profile/logs"),
            }
        }

        fn marker(&self) -> PathBuf {
            self.0.join("application").join(PORTABLE_MARKER)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn installed_locations_preserve_existing_history_without_writing_resources() {
        let fixture = Fixture::new();
        let history = fixture.defaults().data_dir.join("jobs");
        fs::create_dir_all(&history).unwrap();
        fs::write(history.join("history.json"), b"existing history").unwrap();
        let paths = AppPaths::resolve(&fixture.executable(), fixture.defaults()).unwrap();
        assert_eq!(paths.mode, StorageMode::Installed);
        assert_eq!(paths.history_dir(), history);
        assert_eq!(paths.job_log_dir(), fixture.defaults().log_dir.join("jobs"));
        paths.prepare().unwrap();
        assert_eq!(
            fs::read(history.join("history.json")).unwrap(),
            b"existing history"
        );
        assert_eq!(fs::read_dir(paths.resource_dir).unwrap().count(), 0);
    }

    #[test]
    fn portable_marker_selects_separate_data_and_never_imports_installed_history() {
        let fixture = Fixture::new();
        let installed = fixture.defaults().data_dir.join("jobs");
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("history.json"), b"installed").unwrap();
        fs::write(fixture.marker(), b"1\r\n").unwrap();
        let paths = AppPaths::resolve(&fixture.executable(), fixture.defaults()).unwrap();
        assert_eq!(paths.mode.as_str(), "portable");
        assert_eq!(
            paths.data_dir,
            fixture.0.join("application/jesses-data/data")
        );
        assert!(!paths.data_dir.exists(), "resolution alone does not write");
        paths.prepare().unwrap();
        assert!(
            !paths.history_dir().exists(),
            "history is never auto-imported"
        );
        assert_eq!(
            fs::read(installed.join("history.json")).unwrap(),
            b"installed"
        );
        for directory in [
            &paths.config_dir,
            &paths.data_dir,
            &paths.cache_dir,
            &paths.log_dir,
        ] {
            assert_eq!(
                fs::read_dir(directory).unwrap().count(),
                0,
                "write probes are removed"
            );
        }
    }

    #[test]
    fn invalid_marker_does_not_fall_back_or_create_data() {
        let fixture = Fixture::new();
        for version in ["", "2", "true", "1\ntrailing", &"1".repeat(33)] {
            fs::write(fixture.marker(), version).unwrap();
            assert!(AppPaths::resolve(&fixture.executable(), fixture.defaults()).is_err());
            assert!(!fixture.defaults().data_dir.exists());
            assert!(!fixture.0.join("application/jesses-data").exists());
        }
        fs::remove_file(fixture.marker()).unwrap();
        fs::create_dir(fixture.marker()).unwrap();
        assert!(AppPaths::resolve(&fixture.executable(), fixture.defaults()).is_err());
    }

    #[test]
    fn portable_conflict_does_not_fall_back_or_overwrite_existing_file() {
        let fixture = Fixture::new();
        fs::write(fixture.marker(), b"1\n").unwrap();
        let conflict = fixture.0.join("application/jesses-data");
        fs::write(&conflict, b"keep this file").unwrap();
        let paths = AppPaths::resolve(&fixture.executable(), fixture.defaults()).unwrap();
        assert!(paths.prepare().is_err());
        assert_eq!(fs::read(conflict).unwrap(), b"keep this file");
        assert!(!fixture.defaults().data_dir.exists());
    }

    #[test]
    fn relative_locations_are_rejected() {
        let fixture = Fixture::new();
        assert!(AppPaths::resolve(Path::new("jesses.exe"), fixture.defaults()).is_err());
        let mut defaults = fixture.defaults();
        defaults.data_dir = PathBuf::from("relative");
        assert!(AppPaths::resolve(&fixture.executable(), defaults).is_err());
    }

    #[test]
    fn appimage_requires_its_own_mount_and_absolute_launcher() {
        let fixture = Fixture::new();
        let executable = fixture.executable();
        fs::write(&executable, b"test executable").unwrap();
        let mount = executable.parent().unwrap();
        let image = fixture.0.join("jesses.AppImage");
        fs::write(&image, b"test AppImage").unwrap();
        assert_eq!(
            launcher_path(&executable, Some(&image), Some(mount)).unwrap(),
            image
        );
        assert!(
            launcher_path(
                &executable,
                Some(Path::new("relative.AppImage")),
                Some(mount)
            )
            .is_err()
        );
        assert!(launcher_path(&executable, None, Some(mount)).is_err());
        assert_eq!(
            launcher_path(&executable, Some(&image), None).unwrap(),
            executable
        );
        let other_mount = fixture.0.join("other-application");
        fs::create_dir(&other_mount).unwrap();
        assert_eq!(
            launcher_path(&executable, Some(&image), Some(&other_mount)).unwrap(),
            executable,
            "an inherited APPIMAGE/APPDIR pair cannot select another data store"
        );
    }

    #[cfg(unix)]
    #[test]
    fn redirected_portable_marker_and_data_are_rejected() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let target = fixture.0.join("marker-target");
        fs::write(&target, b"1\n").unwrap();
        symlink(&target, fixture.marker()).unwrap();
        assert!(AppPaths::resolve(&fixture.executable(), fixture.defaults()).is_err());
        fs::remove_file(fixture.marker()).unwrap();
        fs::write(fixture.marker(), b"1\n").unwrap();
        let outside = fixture.0.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, fixture.0.join("application/jesses-data")).unwrap();
        let paths = AppPaths::resolve(&fixture.executable(), fixture.defaults()).unwrap();
        assert!(paths.prepare().is_err());
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    }
}
