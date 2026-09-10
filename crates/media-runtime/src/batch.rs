//! Read-only folder discovery and batch proposal preparation.
use media_core::{
    AppError, BatchEncodeInput, BatchEncodeItem, BatchEncodePreview, BatchEncodeRequest,
    EncodeRequest, EncodeSettings, FolderScanRequest, FolderScanResult, MediaFile, RemuxRequest,
};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_MEDIA: usize = 500;
const MAX_ENTRIES: usize = 10_000;
pub(crate) const MAX_BATCH: usize = 100;
const EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "mov", "avi", "webm", "m2ts", "mts", "ts", "mpeg", "mpg", "wmv", "flv",
    "ogv", "vob", "3gp", "mxf", "mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "aif", "aiff",
    "alac",
];

fn error(code: &str, message: impl Into<String>, path: &Path) -> AppError {
    AppError::new(code, message, Some(path.to_string_lossy().into_owned()))
}

fn local_absolute(path: &Path) -> Result<(), AppError> {
    if !path.is_absolute() || path.as_os_str().to_string_lossy().contains('\0') {
        return Err(error(
            "INVALID_PATH",
            "Choose an absolute local filesystem path.",
            path,
        ));
    }
    Ok(())
}

fn is_link(metadata: &fs::Metadata) -> bool {
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

pub async fn scan_media_folder(request: FolderScanRequest) -> Result<FolderScanResult, AppError> {
    let path = PathBuf::from(request.path);
    local_absolute(&path)?;
    tokio::time::timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            scan_folder(&path, request.recursive, MAX_MEDIA, MAX_ENTRIES)
        }),
    )
    .await
    .map_err(|_| {
        AppError::new(
            "FOLDER_SCAN_TIMEOUT",
            "Folder discovery timed out while checking the filesystem.",
            None,
        )
    })?
    .map_err(|e| AppError::new("FOLDER_SCAN_FAILED", e.to_string(), None))?
}

fn scan_folder(
    path: &Path,
    recursive: bool,
    max_media: usize,
    max_entries: usize,
) -> Result<FolderScanResult, AppError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| error("FOLDER_UNREADABLE", e.to_string(), path))?;
    if is_link(&metadata) {
        return Err(error(
            "FOLDER_LINK_UNSUPPORTED",
            "Choose the actual folder; symbolic links and reparse points are skipped.",
            path,
        ));
    }
    if !metadata.is_dir() {
        return Err(error(
            "NOT_A_FOLDER",
            "Choose a folder containing media files.",
            path,
        ));
    }
    let root =
        fs::canonicalize(path).map_err(|e| error("FOLDER_UNREADABLE", e.to_string(), path))?;
    let mut result = FolderScanResult {
        paths: Vec::new(),
        errors: Vec::new(),
        skipped_count: 0,
        truncated: false,
    };
    let mut pending = vec![root.clone()];
    let mut seen = HashSet::new();
    let mut seen_dirs = HashSet::new();
    let mut visited = 0;
    while let Some(directory) = pending.pop() {
        if visited >= max_entries || result.paths.len() >= max_media {
            result.truncated = true;
            break;
        }
        let current = match fs::symlink_metadata(&directory) {
            Ok(meta) => meta,
            Err(e) => {
                result
                    .errors
                    .push(error("FOLDER_UNREADABLE", e.to_string(), &directory));
                continue;
            }
        };
        if is_link(&current) {
            result.skipped_count += 1;
            continue;
        }
        let canonical = match fs::canonicalize(&directory) {
            Ok(path) => path,
            Err(e) => {
                result
                    .errors
                    .push(error("FOLDER_UNREADABLE", e.to_string(), &directory));
                continue;
            }
        };
        if !canonical.starts_with(&root) || !seen_dirs.insert(path_key(&canonical)) {
            result.skipped_count += 1;
            continue;
        }
        let entries = match fs::read_dir(&canonical) {
            Ok(entries) => entries,
            Err(e) => {
                result
                    .errors
                    .push(error("FOLDER_UNREADABLE", e.to_string(), &canonical));
                continue;
            }
        };
        let mut batch = Vec::new();
        for entry in entries {
            if visited >= max_entries {
                result.truncated = true;
                break;
            }
            visited += 1;
            match entry {
                Ok(entry) => batch.push(entry.path()),
                Err(e) => {
                    result
                        .errors
                        .push(error("FOLDER_ENTRY_UNREADABLE", e.to_string(), &canonical))
                }
            }
        }
        batch.sort();
        let mut child_dirs = Vec::new();
        for (index, path) in batch.iter().enumerate() {
            if result.paths.len() >= max_media {
                result.truncated = true;
                break;
            }
            let metadata = match fs::symlink_metadata(path) {
                Ok(meta) => meta,
                Err(e) => {
                    result
                        .errors
                        .push(error("FOLDER_ENTRY_UNREADABLE", e.to_string(), path));
                    continue;
                }
            };
            if is_link(&metadata) {
                result.skipped_count += 1;
                continue;
            }
            if metadata.is_dir() {
                if recursive {
                    child_dirs.push(path.clone());
                } else {
                    result.skipped_count += 1;
                }
                continue;
            }
            if !metadata.is_file()
                || !path
                    .extension()
                    .and_then(|v| v.to_str())
                    .is_some_and(|ext| {
                        EXTENSIONS
                            .iter()
                            .any(|candidate| ext.eq_ignore_ascii_case(candidate))
                    })
            {
                result.skipped_count += 1;
                continue;
            }
            let canonical = match fs::canonicalize(path) {
                Ok(path) => path,
                Err(e) => {
                    result
                        .errors
                        .push(error("FILE_UNREADABLE", e.to_string(), path));
                    continue;
                }
            };
            if !canonical.starts_with(&root) || !seen.insert(path_key(&canonical)) {
                result.skipped_count += 1;
                continue;
            }
            result.paths.push(canonical.to_string_lossy().into_owned());
            if result.paths.len() == max_media
                && (index + 1 < batch.len() || !pending.is_empty() || !child_dirs.is_empty())
            {
                result.truncated = true;
            }
        }
        pending.extend(child_dirs.into_iter().rev());
    }
    result.paths.sort();
    Ok(result)
}

pub(crate) fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        let text = text.replace('/', r"\");
        if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{share}").to_lowercase()
        } else {
            text.strip_prefix(r"\\?\").unwrap_or(&text).to_lowercase()
        }
    }
    #[cfg(not(windows))]
    {
        text.into_owned()
    }
}

pub(crate) fn destination_key(path: &Path) -> Result<String, AppError> {
    local_absolute(path)?;
    let parent = path.parent().ok_or_else(|| {
        error(
            "INVALID_OUTPUT",
            "Choose an output folder and filename.",
            path,
        )
    })?;
    let parent =
        fs::canonicalize(parent).map_err(|e| error("INVALID_OUTPUT", e.to_string(), parent))?;
    let name = path
        .file_name()
        .ok_or_else(|| error("INVALID_OUTPUT", "Choose an output filename.", path))?;
    Ok(path_key(&parent.join(name)))
}

pub(crate) fn writable_directory(path: &Path) -> Result<PathBuf, AppError> {
    local_absolute(path)?;
    let canonical = fs::canonicalize(path).map_err(|e| {
        error(
            "INVALID_OUTPUT",
            format!("The existing output folder could not be accessed: {e}"),
            path,
        )
    })?;
    if !canonical.is_dir() {
        return Err(error(
            "INVALID_OUTPUT",
            "The output location must be an existing folder.",
            path,
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .access_mode(0x2 | 0x80)
            .share_mode(1 | 2 | 4)
            .custom_flags(0x0200_0000)
            .open(&canonical)
            .map_err(|e| {
                error(
                    "OUTPUT_DIRECTORY_NOT_WRITABLE",
                    format!("The output folder does not grant permission to add files: {e}"),
                    &canonical,
                )
            })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(canonical.as_os_str().as_bytes())
            .map_err(|e| error("INVALID_OUTPUT", e.to_string(), &canonical))?;
        // SAFETY: name is NUL terminated, and this only asks the OS to check access.
        if unsafe {
            libc::faccessat(
                libc::AT_FDCWD,
                name.as_ptr(),
                libc::W_OK | libc::X_OK,
                libc::AT_EACCESS,
            )
        } != 0
        {
            return Err(error(
                "OUTPUT_DIRECTORY_NOT_WRITABLE",
                std::io::Error::last_os_error().to_string(),
                &canonical,
            ));
        }
    }
    Ok(canonical)
}

pub(crate) fn safe_stem(path: &Path) -> String {
    let source = path.file_stem().unwrap_or_default().to_string_lossy();
    // Keep room for the suffix and the owned temporary filename on filesystems
    // whose component limit is measured in UTF-8 bytes, rather than characters.
    let mut stem = String::new();
    for character in source.chars() {
        let character = if character.is_control() || "<>:\"/\\|?*".contains(character) {
            '_'
        } else {
            character
        };
        if stem.len() + character.len_utf8() > 160 {
            break;
        }
        stem.push(character);
    }
    let stem = stem.trim().trim_end_matches(['.', ' ']);
    if stem.is_empty() {
        "media".into()
    } else {
        stem.into()
    }
}

fn proposed_output(
    directory: &Path,
    input: &Path,
    reserved: &mut HashSet<String>,
) -> Result<PathBuf, AppError> {
    let stem = safe_stem(input);
    for number in 1..=10_000 {
        let suffix = if number == 1 {
            String::new()
        } else {
            format!("_{number}")
        };
        let candidate = directory.join(format!("{stem}_av1{suffix}.mkv"));
        let key = path_key(&candidate);
        if key == path_key(input) || reserved.contains(&key) {
            continue;
        }
        match fs::symlink_metadata(&candidate) {
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                reserved.insert(key);
                return Ok(candidate);
            }
            Err(e) => return Err(error("OUTPUT_UNREADABLE", e.to_string(), &candidate)),
        }
    }
    Err(error(
        "OUTPUT_NAME_EXHAUSTED",
        "No available output filename was found after 10,000 alternatives.",
        directory,
    ))
}

pub(crate) async fn inspect_selection(
    manager: &crate::JobManager,
    input: &BatchEncodeInput,
    epoch: u64,
) -> Result<MediaFile, AppError> {
    let path = Path::new(&input.input_path);
    local_absolute(path)?;
    if input.stream_indices.is_empty()
        || input.stream_indices.iter().collect::<HashSet<_>>().len() != input.stream_indices.len()
    {
        return Err(error(
            "STREAM_SELECTION_INVALID",
            "Choose streams without duplicate indices.",
            path,
        ));
    }
    manager.inspect_encode_source(input, epoch).await
}

pub(crate) async fn preview(
    manager: &crate::JobManager,
    request: BatchEncodeRequest,
    mut reserved: HashSet<String>,
    epoch: u64,
) -> Result<BatchEncodePreview, AppError> {
    validate_batch_len(request.inputs.len())?;
    let directory = PathBuf::from(&request.output_directory);
    let directory = tokio::task::spawn_blocking(move || writable_directory(&directory))
        .await
        .map_err(|e| AppError::new("INVALID_OUTPUT", e.to_string(), None))??;
    let mut items = Vec::with_capacity(request.inputs.len());
    for input in request.inputs {
        let mut item = BatchEncodeItem {
            input_path: input.input_path.clone(),
            output_path: None,
            request: None,
            error: None,
        };
        match inspect_selection(manager, &input, epoch).await {
            Err(error) if matches!(error.code.as_str(), "BATCH_CANCELED" | "APP_CLOSING") => {
                return Err(error);
            }
            Err(error) => item.error = Some(error),
            Ok(media) => match proposed_output(&directory, Path::new(&media.path), &mut reserved) {
                Err(error) => item.error = Some(error),
                Ok(output) => {
                    let output_path = output.to_string_lossy().into_owned();
                    item.output_path = Some(output_path.clone());
                    item.request = Some(EncodeRequest {
                        source: RemuxRequest {
                            input_path: media.path,
                            output_path,
                            stream_indices: input.stream_indices,
                        },
                        settings: EncodeSettings {
                            video_stream_index: input.video_stream_index,
                            crf: request.crf,
                            preset: request.preset,
                        },
                    });
                }
            },
        }
        items.push(item);
    }
    Ok(BatchEncodePreview { items })
}

pub(crate) fn validate_batch_len(count: usize) -> Result<(), AppError> {
    if count == 0 || count > MAX_BATCH {
        return Err(AppError::new(
            "BATCH_SIZE_INVALID",
            "Choose between 1 and 100 files for a batch.",
            None,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            for _ in 0..100 {
                let serial = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "jesses folder test {} {nonce} {serial}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!(
                        "Cannot create folder test fixture {}: {error}",
                        path.display()
                    ),
                }
            }
            panic!("Could not reserve a unique folder test fixture directory");
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn folder_scan_is_sorted_recursive_bounded_and_skips_links() {
        let fixture = Fixture::new();
        let root = fixture.0.join("media");
        let nested = root.join("nested");
        let outside = fixture.0.join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(root.join("b.MKV"), b"b").unwrap();
        fs::write(root.join("a 日本語.mp4"), b"a").unwrap();
        fs::write(root.join("notes.txt"), b"notes").unwrap();
        fs::write(nested.join("c.webm"), b"c").unwrap();
        fs::write(outside.join("not-in-folder.mkv"), b"outside").unwrap();
        let linked = root.join("linked outside");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &linked).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let result = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&linked)
                .arg(&outside)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "Could not create owned test junction: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let flat = scan_folder(&root, false, MAX_MEDIA, MAX_ENTRIES).unwrap();
        assert_eq!(flat.paths.len(), 2);
        assert!(flat.paths[0].contains("a 日本語"));
        assert!(flat.skipped_count >= 3);
        assert!(!flat.truncated);
        let recursive = scan_folder(&root, true, MAX_MEDIA, MAX_ENTRIES).unwrap();
        assert_eq!(recursive.paths.len(), 3);
        assert!(
            !recursive
                .paths
                .iter()
                .any(|path| path.contains("not-in-folder"))
        );
        assert!(!recursive.truncated);
        assert!(scan_folder(&root, true, 2, MAX_ENTRIES).unwrap().truncated);
        assert!(scan_folder(&root, true, MAX_MEDIA, 2).unwrap().truncated);
        assert_eq!(
            scan_folder(&linked, true, MAX_MEDIA, MAX_ENTRIES)
                .unwrap_err()
                .code,
            "FOLDER_LINK_UNSUPPORTED"
        );
        #[cfg(windows)]
        fs::remove_dir(linked).unwrap();
        #[cfg(unix)]
        fs::remove_file(linked).unwrap();
        assert_eq!(
            fs::read(outside.join("not-in-folder.mkv")).unwrap(),
            b"outside"
        );
    }

    #[test]
    fn proposals_reserve_names_without_creating_or_replacing_files() {
        let fixture = Fixture::new();
        let directory = writable_directory(&fixture.0).unwrap();
        let input = directory.join("Title 日本語.mp4");
        fs::write(&input, b"source").unwrap();
        let existing = directory.join("Title 日本語_av1.mkv");
        fs::write(&existing, b"existing").unwrap();
        let queued = directory.join("Title 日本語_av1_2.mkv");
        let mut reserved = HashSet::from([path_key(&queued)]);
        let third = proposed_output(&directory, &input, &mut reserved).unwrap();
        let fourth = proposed_output(&directory, &input, &mut reserved).unwrap();
        assert_eq!(third.file_name().unwrap(), "Title 日本語_av1_3.mkv");
        assert_eq!(fourth.file_name().unwrap(), "Title 日本語_av1_4.mkv");
        assert!(!third.exists());
        assert!(!fourth.exists());
        assert_eq!(fs::read(existing).unwrap(), b"existing");
        assert_eq!(fs::read(input).unwrap(), b"source");
        assert_eq!(safe_stem(Path::new("bad:name?.mp4")), "bad_name_");
        assert_eq!(safe_stem(Path::new("  ... .mp4")), "media");
        let long = format!("{}.mp4", "日本語🙂".repeat(60));
        let stem = safe_stem(Path::new(&long));
        assert!(stem.len() <= 160);
        assert!(
            stem.chars().count() > 30,
            "Unicode names remain recognizable"
        );
        let output = proposed_output(&directory, Path::new(&long), &mut reserved).unwrap();
        assert!(output.file_name().unwrap().to_string_lossy().len() < 255);
        assert_eq!(
            writable_directory(&fixture.0.join("missing"))
                .unwrap_err()
                .code,
            "INVALID_OUTPUT"
        );
        #[cfg(windows)]
        {
            assert_eq!(
                destination_key(&queued).unwrap(),
                destination_key(&fixture.0.join("TITLE 日本語_AV1_2.MKV")).unwrap()
            );
            // The Windows TEMP path may contain an 8.3 alias. Destination
            // identity resolves the parent; path_key only normalizes spelling.
            assert_eq!(
                destination_key(&fixture.0.join("alias-check.mkv")).unwrap(),
                path_key(&directory.join("alias-check.mkv"))
            );
            assert_eq!(
                path_key(Path::new(r"\\?\C:\media\movie.mkv")),
                path_key(Path::new(r"C:\media\movie.mkv"))
            );
            assert_eq!(
                path_key(Path::new(r"\\?\UNC\server\share\movie.mkv")),
                path_key(Path::new(r"\\server\share\movie.mkv"))
            );
        }
    }

    #[tokio::test]
    async fn preview_keeps_per_file_source_and_selection_errors_visible() {
        let fixture = Fixture::new();
        let manager = crate::JobManager::new(fixture.0.join("logs"));
        let result = manager
            .preview_encode_batch(BatchEncodeRequest {
                inputs: vec![
                    BatchEncodeInput {
                        input_path: fixture.0.join("missing.mkv").to_string_lossy().into_owned(),
                        stream_indices: vec![0],
                        video_stream_index: 0,
                    },
                    BatchEncodeInput {
                        input_path: fixture
                            .0
                            .join("also-missing.mkv")
                            .to_string_lossy()
                            .into_owned(),
                        stream_indices: vec![0, 0],
                        video_stream_index: 0,
                    },
                ],
                output_directory: fixture.0.to_string_lossy().into_owned(),
                crf: 30,
                preset: 4,
            })
            .await
            .unwrap();
        assert_eq!(result.items.len(), 2);
        assert_eq!(
            result.items[0].error.as_ref().unwrap().code,
            "FILE_NOT_FOUND"
        );
        assert_eq!(
            result.items[1].error.as_ref().unwrap().code,
            "STREAM_SELECTION_INVALID"
        );
        assert!(
            result
                .items
                .iter()
                .all(|item| item.output_path.is_none() && item.request.is_none())
        );
        assert!(validate_batch_len(0).is_err());
        assert!(validate_batch_len(101).is_err());
    }
}
