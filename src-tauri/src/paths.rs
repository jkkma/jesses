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

#[cfg(windows)]
fn expected_scoop_persist_root(executable_parent: &Path) -> Option<PathBuf> {
    let app_directory = executable_parent.parent()?;
    if !app_directory
        .file_name()?
        .to_string_lossy()
        .eq_ignore_ascii_case("jesses")
    {
        return None;
    }
    let apps_directory = app_directory.parent()?;
    if !apps_directory
        .file_name()?
        .to_string_lossy()
        .eq_ignore_ascii_case("apps")
    {
        return None;
    }
    Some(
        apps_directory
            .parent()?
            .join("persist")
            .join("jesses")
            .join(PORTABLE_DIRECTORY),
    )
}

#[cfg(windows)]
fn same_windows_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(windows)]
fn require_ordinary_scoop_directory(path: &Path, description: &str) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "Cannot inspect Scoop's {description} {}: {error}",
                path.display()
            ),
        )
    })?;
    if !metadata.is_dir() || redirected(&metadata) {
        return Err(invalid(format!(
            "Scoop's {description} must be an ordinary directory: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn resolve_scoop_persist_root(executable_parent: &Path, root: &Path) -> io::Result<PathBuf> {
    let Some(expected) = expected_scoop_persist_root(executable_parent) else {
        return Err(invalid(format!(
            "Portable data may use a directory junction only for Scoop's managed persist location: {}",
            root.display()
        )));
    };
    let persist_app = expected
        .parent()
        .ok_or_else(|| invalid("Scoop's persist app directory has no parent."))?;
    let persist = persist_app
        .parent()
        .ok_or_else(|| invalid("Scoop's persist directory has no parent."))?;
    let scoop_root = persist
        .parent()
        .ok_or_else(|| invalid("Scoop's root directory has no parent."))?;
    for (path, description) in [
        (scoop_root, "root directory"),
        (&scoop_root.join("apps"), "apps directory"),
        (&scoop_root.join("apps/jesses"), "Jesses app directory"),
        (persist, "persist directory"),
        (persist_app, "Jesses persist directory"),
        (&expected, "expected persist location"),
    ] {
        require_ordinary_scoop_directory(path, description)?;
    }
    let resolved = root.canonicalize().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "Cannot resolve Scoop's persisted portable data {}: {error}",
                root.display()
            ),
        )
    })?;
    let expected = expected.canonicalize().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "Cannot resolve Scoop's expected persist location {}: {error}",
                expected.display()
            ),
        )
    })?;
    let managed_expected = scoop_root
        .canonicalize()?
        .join("persist/jesses")
        .join(PORTABLE_DIRECTORY);
    let metadata = fs::symlink_metadata(&resolved)?;
    if !metadata.is_dir()
        || redirected(&metadata)
        || !same_windows_path(&resolved, &expected)
        || !same_windows_path(&expected, &managed_expected)
    {
        return Err(invalid(format!(
            "Portable data junction {} must resolve to Scoop's ordinary managed directory {}.",
            root.display(),
            expected.display()
        )));
    }
    Ok(resolved)
}

fn resolve_portable_root(executable_parent: &Path) -> io::Result<PathBuf> {
    let root = executable_parent.join(PORTABLE_DIRECTORY);
    match fs::symlink_metadata(&root) {
        Ok(metadata) if redirected(&metadata) => {
            #[cfg(windows)]
            {
                resolve_scoop_persist_root(executable_parent, &root)
            }
            #[cfg(not(windows))]
            {
                Ok(root)
            }
        }
        Ok(metadata) if metadata.is_dir() => Ok(root),
        Ok(_) => Ok(root),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(root),
        Err(error) => Err(error),
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
            let root = resolve_portable_root(parent)?;
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

    /// Portable browser state follows the same explicitly selected storage root.
    /// Installed mode keeps Tauri's existing platform-default WebView profile.
    pub fn webview_data_dir(&self) -> Option<PathBuf> {
        self.portable_root
            .as_ref()
            .map(|_| self.cache_dir.join("webview"))
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
        if let Some(directory) = self.webview_data_dir() {
            ensure_portable_directory(&directory)?;
            check_writable(&directory)?;
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

    static FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "jesses-paths-{}-{nonce}-{sequence}",
                std::process::id()
            ));
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
        assert_eq!(paths.webview_data_dir(), None);
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
        assert_eq!(
            paths.webview_data_dir(),
            Some(fixture.0.join("application/jesses-data/cache/webview"))
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
        for directory in [&paths.config_dir, &paths.data_dir, &paths.log_dir] {
            assert_eq!(
                fs::read_dir(directory).unwrap().count(),
                0,
                "write probes are removed"
            );
        }
        let webview = paths.webview_data_dir().unwrap();
        assert_eq!(fs::read_dir(&paths.cache_dir).unwrap().count(), 1);
        assert_eq!(fs::read_dir(webview).unwrap().count(), 0);
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

    #[cfg(windows)]
    fn create_junction(link: &Path, target: &Path) {
        use std::os::windows::process::CommandExt;

        let command = format!("mklink /J \"{}\" \"{}\"", link.display(), target.display());
        let mut process = std::process::Command::new("cmd.exe");
        process.args(["/d", "/c"]);
        process.raw_arg(command);
        let status = process.status().unwrap();
        assert!(status.success(), "could not create test directory junction");
    }

    #[cfg(windows)]
    #[test]
    fn scoop_versions_share_only_the_expected_persisted_portable_root() {
        let fixture = Fixture::new();
        let scoop = fixture.0.join("scoop");
        let app = scoop.join("apps/jesses");
        let first = app.join("0.1.0");
        let second = app.join("0.1.1");
        let persisted = scoop.join("persist/jesses/jesses-data");
        for directory in [&first, &second, &persisted] {
            fs::create_dir_all(directory).unwrap();
        }
        for directory in [&first, &second] {
            fs::write(directory.join(PORTABLE_MARKER), b"1\n").unwrap();
            create_junction(&directory.join(PORTABLE_DIRECTORY), &persisted);
        }

        let legacy_history = fixture.0.join("legacy-profile/data/jobs");
        fs::create_dir_all(&legacy_history).unwrap();
        fs::write(
            legacy_history.join("jobs.json"),
            b"legacy installed history",
        )
        .unwrap();
        let installed = InstalledPaths {
            resource_dir: first.clone(),
            config_dir: fixture.0.join("legacy-profile/config"),
            data_dir: fixture.0.join("legacy-profile/data"),
            cache_dir: fixture.0.join("legacy-profile/cache"),
            log_dir: fixture.0.join("legacy-profile/logs"),
        };

        let first_paths = AppPaths::resolve(&first.join("jesses.exe"), installed.clone()).unwrap();
        first_paths.prepare().unwrap();
        fs::create_dir_all(first_paths.history_dir()).unwrap();
        fs::write(
            first_paths.history_dir().join("jobs.json"),
            b"scoop history",
        )
        .unwrap();

        let second_paths =
            AppPaths::resolve(&second.join("jesses.exe"), installed.clone()).unwrap();
        second_paths.prepare().unwrap();
        assert_eq!(first_paths.config_dir, second_paths.config_dir);
        assert_eq!(first_paths.data_dir, second_paths.data_dir);
        assert_eq!(first_paths.cache_dir, second_paths.cache_dir);
        assert_eq!(first_paths.log_dir, second_paths.log_dir);
        assert_eq!(
            first_paths.webview_data_dir(),
            Some(persisted.canonicalize().unwrap().join("cache/webview"))
        );
        assert_eq!(
            fs::read(second_paths.history_dir().join("jobs.json")).unwrap(),
            b"scoop history"
        );
        assert_eq!(
            fs::read(legacy_history.join("jobs.json")).unwrap(),
            b"legacy installed history"
        );
        for directory in [
            &second_paths.config_dir,
            &second_paths.data_dir,
            &second_paths.cache_dir,
            &second_paths.log_dir,
        ] {
            let metadata = fs::symlink_metadata(directory).unwrap();
            assert!(metadata.is_dir());
            assert!(!redirected(&metadata));
        }
        let webview = second_paths.webview_data_dir().unwrap();
        let metadata = fs::symlink_metadata(webview).unwrap();
        assert!(metadata.is_dir());
        assert!(!redirected(&metadata));

        for directory in [&first, &second] {
            fs::remove_dir(directory.join(PORTABLE_DIRECTORY)).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn portable_data_junction_outside_scoop_persist_is_rejected_without_writes() {
        let fixture = Fixture::new();
        let version = fixture.0.join("scoop/apps/jesses/0.1.0");
        let outside = fixture.0.join("outside");
        fs::create_dir_all(&version).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(version.join(PORTABLE_MARKER), b"1\n").unwrap();
        create_junction(&version.join(PORTABLE_DIRECTORY), &outside);

        let result = AppPaths::resolve(&version.join("jesses.exe"), fixture.defaults());
        assert!(result.is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

        fs::remove_dir(version.join(PORTABLE_DIRECTORY)).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn scoop_persist_root_does_not_allow_redirected_storage_children() {
        let fixture = Fixture::new();
        let scoop = fixture.0.join("scoop");
        let version = scoop.join("apps/jesses/0.1.0");
        let persisted = scoop.join("persist/jesses/jesses-data");
        let outside = fixture.0.join("outside");
        for directory in [&version, &persisted, &outside] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(version.join(PORTABLE_MARKER), b"1\n").unwrap();
        create_junction(&version.join(PORTABLE_DIRECTORY), &persisted);
        create_junction(&persisted.join("data"), &outside);

        let paths = AppPaths::resolve(&version.join("jesses.exe"), fixture.defaults()).unwrap();
        assert!(paths.prepare().is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

        fs::remove_dir(persisted.join("data")).unwrap();
        fs::remove_dir(version.join(PORTABLE_DIRECTORY)).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn scoop_persist_target_itself_must_not_be_redirected() {
        let fixture = Fixture::new();
        let scoop = fixture.0.join("scoop");
        let version = scoop.join("apps/jesses/0.1.0");
        let persist_parent = scoop.join("persist/jesses");
        let persisted = persist_parent.join(PORTABLE_DIRECTORY);
        let outside = fixture.0.join("outside");
        for directory in [&version, &persist_parent, &outside] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(version.join(PORTABLE_MARKER), b"1\n").unwrap();
        create_junction(&persisted, &outside);
        create_junction(&version.join(PORTABLE_DIRECTORY), &persisted);

        let result = AppPaths::resolve(&version.join("jesses.exe"), fixture.defaults());
        assert!(result.is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

        fs::remove_dir(version.join(PORTABLE_DIRECTORY)).unwrap();
        fs::remove_dir(persisted).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn scoop_persist_ancestor_must_not_be_redirected() {
        let fixture = Fixture::new();
        let scoop = fixture.0.join("scoop");
        let version = scoop.join("apps/jesses/0.1.0");
        let redirected_persist = scoop.join("persist");
        let outside = fixture.0.join("outside-persist");
        let persisted = outside.join("jesses/jesses-data");
        fs::create_dir_all(&version).unwrap();
        fs::create_dir_all(&persisted).unwrap();
        fs::write(version.join(PORTABLE_MARKER), b"1\n").unwrap();
        create_junction(&redirected_persist, &outside);
        create_junction(&version.join(PORTABLE_DIRECTORY), &persisted);

        let result = AppPaths::resolve(&version.join("jesses.exe"), fixture.defaults());
        assert!(result.is_err());
        assert_eq!(fs::read_dir(&persisted).unwrap().count(), 0);

        fs::remove_dir(version.join(PORTABLE_DIRECTORY)).unwrap();
        fs::remove_dir(redirected_persist).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn portable_webview_directory_must_not_be_redirected() {
        let fixture = Fixture::new();
        let scoop = fixture.0.join("scoop");
        let version = scoop.join("apps/jesses/0.1.0");
        let persisted = scoop.join("persist/jesses/jesses-data");
        let outside = fixture.0.join("outside");
        for directory in [&version, &persisted, &outside] {
            fs::create_dir_all(directory).unwrap();
        }
        for directory in ["config", "data", "cache", "logs"] {
            fs::create_dir(persisted.join(directory)).unwrap();
        }
        fs::write(version.join(PORTABLE_MARKER), b"1\n").unwrap();
        create_junction(&version.join(PORTABLE_DIRECTORY), &persisted);
        create_junction(&persisted.join("cache/webview"), &outside);

        let paths = AppPaths::resolve(&version.join("jesses.exe"), fixture.defaults()).unwrap();
        assert!(paths.prepare().is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

        fs::remove_dir(persisted.join("cache/webview")).unwrap();
        fs::remove_dir(version.join(PORTABLE_DIRECTORY)).unwrap();
    }
}
