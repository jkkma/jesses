use media_core::AppError;
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

fn write_activity_log(path: String, contents: String) -> Result<(), AppError> {
    let destination = Path::new(&path);
    if !destination.is_absolute() || contents.len() > 2_000_000 || contents.contains('\0') {
        return Err(AppError::new(
            "ACTIVITY_LOG_INVALID",
            "Choose an absolute new log path and a log below 2 MB.",
            None,
        ));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            AppError::new(
                "ACTIVITY_LOG_WRITE_FAILED",
                error.to_string(),
                Some(path.clone()),
            )
        })?;
    // Do not unlink by name after a write failure: another process could have
    // replaced the directory entry since we created it. Report the partial
    // file for the user to inspect instead.
    file.write_all(contents.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            AppError::new(
                "ACTIVITY_LOG_WRITE_FAILED",
                format!("{error}. The new log may be incomplete."),
                Some(path),
            )
        })
}

fn file_manager() -> Result<PathBuf, AppError> {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| {
                AppError::new(
                    "OPEN_DIRECTORY_FAILED",
                    "The Windows system directory is unavailable.",
                    None,
                )
            })?;
        Ok(root.join("explorer.exe"))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(PathBuf::from("/usr/bin/open"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Ok(PathBuf::from("/usr/bin/xdg-open"))
    }
}

/// Write only a new user-selected activity log; never replace an existing file.
#[tauri::command]
pub(crate) async fn save_activity_log(path: String, contents: String) -> Result<(), AppError> {
    tauri::async_runtime::spawn_blocking(move || write_activity_log(path, contents))
        .await
        .map_err(|error| AppError::new("ACTIVITY_LOG_WRITE_FAILED", error.to_string(), None))?
}

/// Reveal an already-existing directory, without interpreting shell syntax.
#[tauri::command]
pub(crate) async fn open_directory(path: String) -> Result<(), AppError> {
    tauri::async_runtime::spawn_blocking(move || {
        let directory = Path::new(&path);
        if !directory.is_absolute() || !directory.is_dir() {
            return Err(AppError::new(
                "OPEN_DIRECTORY_FAILED",
                "Choose an existing absolute directory.",
                Some(path),
            ));
        }
        let mut command = Command::new(file_manager()?);
        command.arg(directory).spawn().map_err(|error| {
            AppError::new("OPEN_DIRECTORY_FAILED", error.to_string(), Some(path))
        })?;
        Ok(())
    })
    .await
    .map_err(|error| AppError::new("OPEN_DIRECTORY_FAILED", error.to_string(), None))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activity_log_never_replaces_an_existing_file() {
        let path = std::env::temp_dir().join(format!(
            "jesses-log-test-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"original content").unwrap();
        let result = write_activity_log(path.to_string_lossy().into_owned(), "replacement".into());
        assert_eq!(result.unwrap_err().code, "ACTIVITY_LOG_WRITE_FAILED");
        assert_eq!(std::fs::read(&path).unwrap(), b"original content");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn file_manager_is_never_resolved_from_the_working_directory_or_path() {
        assert!(file_manager().unwrap().is_absolute());
    }
}
