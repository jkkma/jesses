//! Explicit selected-target replacement. New files are validated by the caller;
//! the displaced object remains recoverable until its original identity agrees.
use super::*;
use tokio::sync::watch;

fn source_at(source: &Source, path: &Path) -> Result<(), AppError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| error("SOURCE_CHANGED", e.to_string(), path))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(error(
            "SOURCE_CHANGED",
            "The selected target was replaced.",
            path,
        ));
    }
    #[cfg(windows)]
    if windows_file_id(&source.file).ok()
        != File::open(path)
            .ok()
            .and_then(|file| windows_file_id(&file).ok())
    {
        return Err(error(
            "SOURCE_CHANGED",
            "The selected target identity changed.",
            path,
        ));
    }
    if Fingerprint::read(
        source
            .file
            .metadata()
            .map_err(|e| error("SOURCE_CHANGED", e.to_string(), path))?,
    )
    .ok()
    .as_ref()
        != Some(&source.fingerprint)
        || Fingerprint::read(metadata).ok().as_ref() != Some(&source.fingerprint)
    {
        return Err(error(
            "SOURCE_CHANGED",
            "The selected target content changed.",
            path,
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn exchange(left: &Path, right: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let left = std::ffi::CString::new(left.as_os_str().as_bytes())?;
    let right = std::ffi::CString::new(right.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Atomic exchange is required for selected-target replacement.",
        ));
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
#[cfg(windows)]
fn replace(target: &Path, replacement: &Path, backup: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>()
    };
    let target = wide(target);
    let replacement = wide(replacement);
    let backup = wide(backup);
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::ReplaceFileW(
            target.as_ptr(),
            replacement.as_ptr(),
            backup.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Temporary {
    pub(crate) fn replace_target(
        self,
        target: Source,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        self.replace_target_inner(target, cancel, || {})
    }
    fn replace_target_inner(
        mut self,
        target: Source,
        cancel: &watch::Receiver<bool>,
        before_commit: impl FnOnce(),
    ) -> Result<(), AppError> {
        #[cfg(windows)]
        let mut target = target;
        if *cancel.borrow() {
            return Err(error(
                "ANALYSIS_CANCELLED",
                "Metadata transfer was canceled before replacement.",
                &target.path,
            ));
        }
        target.verify()?;
        self.verify_identity()?;
        self.clone_file()?
            .sync_all()
            .map_err(|e| error("OUTPUT_FLUSH_FAILED", e.to_string(), &self.path))?;
        let original_path = target.path.clone();
        let temporary_path = self.path.clone();
        #[cfg(unix)]
        fs::set_permissions(
            &temporary_path,
            fs::metadata(&original_path)
                .map_err(|e| error("OUTPUT_REPLACE_FAILED", e.to_string(), &original_path))?
                .permissions(),
        )
        .map_err(|e| error("OUTPUT_REPLACE_FAILED", e.to_string(), &temporary_path))?;
        #[cfg(windows)]
        let backup_directory = {
            use std::os::windows::fs::OpenOptionsExt;
            let guard = OpenOptions::new()
                .read(true)
                .share_mode(1 | 4)
                .open(&original_path)
                .map_err(|e| error("OUTPUT_REPLACE_FAILED", e.to_string(), &original_path))?;
            if windows_file_id(&guard).ok() != windows_file_id(&target.file).ok() {
                return Err(error(
                    "SOURCE_CHANGED",
                    "The selected target changed before replacement.",
                    &original_path,
                ));
            }
            target.file = guard;
            super::super::rate_control::Stats::create(
                &original_path,
                &format!(
                    "metadata-replace-{}-{}",
                    std::process::id(),
                    super::super::NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ),
            )?
        };
        #[cfg(unix)]
        let backup_path = temporary_path.clone();
        #[cfg(windows)]
        let backup_path = backup_directory.path.join("original");
        #[cfg(windows)]
        ensure_absent(&backup_path)?;
        // Cancellation is honored up to this commit point. The following swap,
        // identity check and cleanup contain no await that can interrupt them.
        if *cancel.borrow() {
            return Err(error(
                "ANALYSIS_CANCELLED",
                "Metadata transfer was canceled before replacement.",
                &original_path,
            ));
        }
        target.verify()?;
        before_commit();
        if *cancel.borrow() {
            return Err(error(
                "ANALYSIS_CANCELLED",
                "Metadata transfer was canceled before replacement.",
                &original_path,
            ));
        }
        #[cfg(unix)]
        exchange(&temporary_path, &original_path).map_err(|e| {
            error(
                "OUTPUT_REPLACE_FAILED",
                format!("Cannot atomically replace the selected target: {e}"),
                &original_path,
            )
        })?;
        #[cfg(windows)]
        {
            self.close_for_path_writer()?;
            if let Err(error_value) = replace(&original_path, &temporary_path, &backup_path) {
                let _ = self.reopen_after_path_writer();
                return Err(error(
                    "OUTPUT_REPLACE_FAILED",
                    format!(
                        "Cannot atomically replace the selected target: {error_value}; any backup remains at {}.",
                        backup_path.display()
                    ),
                    &original_path,
                ));
            }
        }
        self.path = original_path.clone();
        self.preserve = true;
        #[cfg(windows)]
        self.reopen_after_path_writer()?;
        let valid = self
            .verify_identity()
            .and_then(|_| source_at(&target, &backup_path));
        #[cfg(unix)]
        let valid = valid.and_then(|_| {
            File::open(original_path.parent().expect("target parent"))
                .and_then(|parent| parent.sync_all())
                .map_err(|e| error("OUTPUT_FLUSH_FAILED", e.to_string(), &original_path))
        });

        if let Err(reason) = valid {
            // A competing path replacement is never authority to delete its
            // displaced file. Restore it only while our new target still agrees.
            self.verify_identity().map_err(|_| {
                error(
                    "SOURCE_CHANGED",
                    format!(
                        "The target changed during replacement; displaced data was retained at {}.",
                        backup_path.display()
                    ),
                    &original_path,
                )
            })?;
            #[cfg(unix)]
            {
                exchange(&backup_path,&original_path).map_err(|e|error("SOURCE_CHANGED",format!("Replacement validation failed and rollback failed: {e}. Original/displaced data remains at {}.",backup_path.display()),&original_path))?;
                self.path = backup_path;
                self.preserve = false;
                self.cleanup()?;
            }
            #[cfg(windows)]
            {
                self.close_for_path_writer()?;
                let failed = backup_directory.path.join("failed-replacement");
                ensure_absent(&failed)?;
                replace(&original_path, &backup_path, &failed).map_err(|e| {
                    error(
                        "SOURCE_CHANGED",
                        format!(
                            "Rollback failed: {e}. Displaced data remains at {}.",
                            backup_path.display()
                        ),
                        &original_path,
                    )
                })?;
                self.path = failed;
                self.preserve = false;
                self.reopen_after_path_writer()?;
                self.cleanup()?;
            }
            return Err(error(
                "SOURCE_CHANGED",
                format!(
                    "The target changed during metadata transfer; its pre-commit contents were restored. {}",
                    reason.message
                ),
                &original_path,
            ));
        }
        source_at(&target, &backup_path)?;
        #[cfg(unix)]
        fs::remove_file(&backup_path).map_err(|e| {
            error(
                "OUTPUT_CLEANUP_FAILED",
                format!(
                    "Metadata was replaced, but the original backup remains at {}: {e}",
                    backup_path.display()
                ),
                &original_path,
            )
        })?;
        #[cfg(windows)]
        {
            let expected = windows_file_id(&target.file)
                .map_err(|e| error("OUTPUT_CLEANUP_FAILED", e.to_string(), &backup_path))?;
            drop(target);
            windows_delete_owned(&backup_path, expected).map_err(|e| {
                error(
                    "OUTPUT_CLEANUP_FAILED",
                    format!(
                        "Metadata was replaced, but the original backup remains at {}: {e}",
                        backup_path.display()
                    ),
                    &original_path,
                )
            })?;
        }
        // The verified exchange was synced before deleting the old backup.
        // A crash can at worst resurrect that harmless backup, not lose either file.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "jesses-replace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let target = root.join("target.mkv");
        fs::write(&target, b"original").unwrap();
        (root, target)
    }
    fn prepared(target: &Path) -> Temporary {
        use std::io::Write;
        let temp = Temporary::create(target, "verified").unwrap();
        temp.clone_file()
            .unwrap()
            .write_all(b"verified new metadata")
            .unwrap();
        temp
    }
    #[test]
    fn replaces_only_the_selected_identity_and_cancellation_preserves_it() {
        let (root, path) = fixture();
        let (_owner, cancel) = watch::channel(false);
        prepared(&path)
            .replace_target(Source::open(&path).unwrap(), &cancel)
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"verified new metadata");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let (_owner, cancel) = watch::channel(true);
        assert!(
            prepared(&path)
                .replace_target(Source::open(&path).unwrap(), &cancel)
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"verified new metadata");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn concurrent_target_replacement_is_restored_and_foreign_data_is_retained() {
        let (root, path) = fixture();
        let displaced = root.join("externally-moved-original");
        let source = Source::open(&path).unwrap();
        let (_owner, cancel) = watch::channel(false);
        let result = prepared(&path).replace_target_inner(source, &cancel, || {
            fs::rename(&path, &displaced).unwrap();
            fs::write(&path, b"competing target").unwrap();
        });
        assert_eq!(result.unwrap_err().code, "SOURCE_CHANGED");
        assert_eq!(fs::read(&path).unwrap(), b"competing target");
        assert_eq!(fs::read(displaced).unwrap(), b"original");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cancellation_at_the_commit_boundary_keeps_the_original() {
        let (root, path) = fixture();
        let source = Source::open(&path).unwrap();
        let (owner, cancel) = watch::channel(false);
        let result = prepared(&path).replace_target_inner(source, &cancel, || {
            owner.send(true).unwrap();
        });
        assert_eq!(result.unwrap_err().code, "ANALYSIS_CANCELLED");
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
