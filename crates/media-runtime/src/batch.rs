//! Read-only folder discovery and batch proposal preparation.
use media_core::{
    AppError, Av1anTargetMetric, BatchEncodeInput, BatchEncodeItem, BatchEncodePreview,
    BatchEncodeRequest, EncodeBackend, EncodeRequest, EncodeSettings, FolderScanRequest,
    FolderScanResult, MediaFile, RemuxRequest, VideoEncoder, VideoRateControl,
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
const MAX_NAME_TEMPLATE_BYTES: usize = 512;
const MAX_OUTPUT_COMPONENT_BYTES: usize = 240;
const MAX_SOURCE_STEM_BYTES: usize = 160;
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

fn sanitized_fragment(source: &str) -> String {
    let mut sanitized = String::new();
    for character in source.chars() {
        let character = if character.is_control() || "<>:\"/\\|?*".contains(character) {
            '_'
        } else {
            character
        };
        sanitized.push(character);
    }
    sanitized
}

fn is_windows_device_name(component: &str) -> bool {
    let device = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(['.', ' '])
        .to_ascii_uppercase();
    matches!(
        device.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || device
        .strip_prefix("COM")
        .or_else(|| device.strip_prefix("LPT"))
        .is_some_and(|number| {
            matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

fn safe_component(source: &str) -> String {
    // Preserve the historical 160-byte source-stem allowance. Template
    // suffixes are budgeted separately before the complete component is made.
    let sanitized = sanitized_fragment(source);
    let mut stem = sanitized
        .get(..sanitized.floor_char_boundary(MAX_SOURCE_STEM_BYTES))
        .unwrap_or_default()
        .trim()
        .trim_end_matches(['.', ' '])
        .to_owned();
    if is_windows_device_name(&stem) {
        stem.insert(0, '_');
        stem.truncate(stem.floor_char_boundary(MAX_SOURCE_STEM_BYTES));
        stem = stem.trim_end_matches(['.', ' ']).to_owned();
    }
    if stem.is_empty() || matches!(stem.as_str(), "." | "..") {
        "media".into()
    } else {
        stem
    }
}

fn finish_rendered_component(parts: Vec<(String, bool)>, input: &Path) -> Result<String, AppError> {
    let fixed_bytes = parts
        .iter()
        .filter(|(_, source_name)| !source_name)
        .map(|(value, _)| value.len())
        .sum::<usize>();
    let minimum_name_bytes = parts
        .iter()
        .filter(|(_, source_name)| *source_name)
        .filter_map(|(value, _)| value.chars().next())
        .map(char::len_utf8)
        .sum::<usize>();
    if fixed_bytes + minimum_name_bytes > MAX_OUTPUT_COMPONENT_BYTES {
        return Err(error(
            "OUTPUT_TEMPLATE_TOO_LONG",
            format!(
                "The filename template needs more than {MAX_OUTPUT_COMPONENT_BYTES} bytes after its tokens are expanded. Shorten its fixed text or remove tokens."
            ),
            input,
        ));
    }

    let mut remaining = MAX_OUTPUT_COMPONENT_BYTES - fixed_bytes;
    let mut remaining_minimum = minimum_name_bytes;
    let mut output = String::with_capacity(fixed_bytes + remaining.min(MAX_SOURCE_STEM_BYTES));
    for (value, source_name) in parts {
        if !source_name {
            output.push_str(&value);
            continue;
        }
        let minimum = value.chars().next().map_or(0, char::len_utf8);
        remaining_minimum -= minimum;
        let allowance = (remaining - remaining_minimum).min(MAX_SOURCE_STEM_BYTES);
        let boundary = value.floor_char_boundary(allowance);
        output.push_str(&value[..boundary]);
        remaining -= boundary;
    }

    let mut output = output.trim().trim_end_matches(['.', ' ']).to_owned();
    if output.is_empty() || matches!(output.as_str(), "." | "..") {
        output = "media".into();
    }
    if is_windows_device_name(&output) {
        if output.len() == MAX_OUTPUT_COMPONENT_BYTES {
            return Err(error(
                "OUTPUT_TEMPLATE_TOO_LONG",
                "The filename template leaves no room to make its Windows device name safe.",
                input,
            ));
        }
        output.insert(0, '_');
    }
    debug_assert!(output.len() <= MAX_OUTPUT_COMPONENT_BYTES);
    Ok(output)
}

pub(crate) fn safe_stem(path: &Path) -> String {
    safe_component(&path.file_stem().unwrap_or_default().to_string_lossy())
}

fn codec_name(encoder: VideoEncoder) -> &'static str {
    match encoder {
        VideoEncoder::SvtAv1 => "av1",
        VideoEncoder::SvtAv1FiveFish => "av1_5fish",
        VideoEncoder::SvtAv1Hdr => "av1_hdr",
        VideoEncoder::X264 => "x264",
        VideoEncoder::X265 => "x265",
        VideoEncoder::Vp9 => "vp9",
        VideoEncoder::AomAv1 => "aom",
        VideoEncoder::X265Standalone => "x265_standalone",
        VideoEncoder::VpxStandalone => "vpx",
        VideoEncoder::H264Nvenc => "h264_nvenc",
        VideoEncoder::HevcNvenc => "hevc_nvenc",
    }
}

fn decimal_tenths(value: u16) -> String {
    if value.is_multiple_of(10) {
        (value / 10).to_string()
    } else {
        format!("{}.{:01}", value / 10, value % 10)
    }
}

fn crf_value(request: &BatchEncodeRequest) -> Option<String> {
    if request.lossless
        || request.rate_control.is_some()
        || (request.backend == EncodeBackend::Av1an
            && request
                .av1an_options
                .is_some_and(|options| options.target_quality.is_some()))
    {
        return None;
    }
    match request
        .encoder
        .is_svt()
        .then_some(request.svt_crf_quarter_steps)
        .flatten()
    {
        Some(quarters) => {
            let whole = quarters / 4;
            let fraction = match quarters % 4 {
                0 => "",
                1 => ".25",
                2 => ".5",
                3 => ".75",
                _ => unreachable!(),
            };
            Some(format!("{whole}{fraction}"))
        }
        None => Some(request.crf.to_string()),
    }
}

fn preset_value(request: &BatchEncodeRequest) -> String {
    const X26X_PRESETS: [&str; 10] = [
        "ultrafast",
        "superfast",
        "veryfast",
        "faster",
        "fast",
        "medium",
        "slow",
        "slower",
        "veryslow",
        "placebo",
    ];
    match request.encoder {
        VideoEncoder::X264 | VideoEncoder::X265 | VideoEncoder::X265Standalone => X26X_PRESETS
            .get(usize::from(request.preset))
            .map_or_else(|| request.preset.to_string(), |value| (*value).into()),
        VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc => {
            format!("P{}", u16::from(request.preset) + 1)
        }
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => request
            .svt_preset
            .map_or_else(|| request.preset.to_string(), |value| value.to_string()),
        VideoEncoder::Vp9 | VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone => {
            request.preset.to_string()
        }
    }
}

fn quality_value(request: &BatchEncodeRequest) -> String {
    if request.lossless {
        return "lossless".into();
    }
    if let Some(rate_control) = request.rate_control {
        return match rate_control {
            VideoRateControl::Bitrate {
                bitrate_kbps,
                two_pass,
            } => format!("{bitrate_kbps}kbps{}", if two_pass { "_2pass" } else { "" }),
            VideoRateControl::TargetSize { target_size_mib } => {
                format!("{target_size_mib}MiB")
            }
        };
    }
    if let Some(target) = request
        .av1an_options
        .and_then(|options| options.target_quality)
    {
        let metric = match target.metric {
            Av1anTargetMetric::Vmaf => "vmaf",
            Av1anTargetMetric::Ssimulacra2 => "ssimulacra2",
            Av1anTargetMetric::Butteraugli => "butteraugli",
            Av1anTargetMetric::Xpsnr => "xpsnr",
            Av1anTargetMetric::XpsnrWeighted => "xpsnr-weighted",
        };
        return format!(
            "{metric}_{}-{}",
            decimal_tenths(target.minimum_score_tenths),
            decimal_tenths(target.maximum_score_tenths)
        );
    }
    crf_value(request).unwrap_or_else(|| request.crf.to_string())
}

fn valid_naming_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let parse = |range: std::ops::Range<usize>| {
        bytes[range].iter().try_fold(0_u32, |value, byte| {
            if byte.is_ascii_digit() {
                Some(value * 10 + u32::from(byte - b'0'))
            } else {
                None
            }
        })
    };
    let (Some(year), Some(month), Some(day)) = (parse(0..4), parse(5..7), parse(8..10)) else {
        return false;
    };
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

fn rendered_name(
    request: &BatchEncodeRequest,
    media: &MediaFile,
    video_stream_index: u32,
    index: usize,
    total: usize,
) -> Result<String, AppError> {
    let input = Path::new(&media.path);
    let template = request
        .output_name_template
        .as_deref()
        .unwrap_or("{name}_{codec}");
    if template.len() > MAX_NAME_TEMPLATE_BYTES {
        return Err(error(
            "OUTPUT_TEMPLATE_TOO_LONG",
            format!(
                "The filename template is longer than {MAX_NAME_TEMPLATE_BYTES} bytes. Shorten it before previewing the batch."
            ),
            input,
        ));
    }
    if template.trim().is_empty() {
        return Err(error(
            "OUTPUT_TEMPLATE_INVALID",
            "Enter a filename template. Available tokens: {name}, {ext}, {index}, {codec}, {crf}, {quality}, {preset}, {width}, {height}, and {date}.",
            input,
        ));
    }
    let video = media
        .streams
        .iter()
        .find(|stream| stream.kind == "video" && stream.index == video_stream_index);
    let mut parts: Vec<(String, bool)> = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find(['{', '}']) {
        parts.push((sanitized_fragment(&rest[..open]), false));
        if rest.as_bytes()[open] == b'}' {
            return Err(error(
                "OUTPUT_TEMPLATE_INVALID",
                "The filename template contains a closing brace without a token.",
                input,
            ));
        }
        let token_start = open + 1;
        let Some(close_offset) = rest[token_start..].find('}') else {
            return Err(error(
                "OUTPUT_TEMPLATE_INVALID",
                "The filename template contains an unfinished token.",
                input,
            ));
        };
        let close = token_start + close_offset;
        let token = &rest[token_start..close];
        if token.contains('{') {
            return Err(error(
                "OUTPUT_TEMPLATE_INVALID",
                "The filename template contains nested braces.",
                input,
            ));
        }
        let (replacement, source_name) = match token.to_ascii_lowercase().as_str() {
            "name" => (safe_stem(input), true),
            "ext" => (
                sanitized_fragment(&input.extension().unwrap_or_default().to_string_lossy()),
                false,
            ),
            "index" => (
                format!("{:0width$}", index + 1, width = total.to_string().len()),
                false,
            ),
            "codec" => (codec_name(request.encoder).into(), false),
            "crf" => (
                crf_value(request).ok_or_else(|| {
                    error(
                        "OUTPUT_TEMPLATE_VALUE_UNAVAILABLE",
                        "{crf} is available only for constant-quality batches. Use {quality} for lossless, bitrate, target-size, or av1an target-quality batches.",
                        input,
                    )
                })?,
                false,
            ),
            "quality" => (quality_value(request), false),
            "preset" => (preset_value(request), false),
            "width" => (
                video
                .and_then(|stream| stream.width)
                .map(|value| value.to_string())
                .ok_or_else(|| {
                    error(
                        "OUTPUT_TEMPLATE_VALUE_UNAVAILABLE",
                        "{width} is unavailable because the selected video stream has no reported width.",
                        input,
                    )
                })?,
                false,
            ),
            "height" => (
                video
                .and_then(|stream| stream.height)
                .map(|value| value.to_string())
                .ok_or_else(|| {
                    error(
                        "OUTPUT_TEMPLATE_VALUE_UNAVAILABLE",
                        "{height} is unavailable because the selected video stream has no reported height.",
                        input,
                    )
                })?,
                false,
            ),
            "date" => (
                request
                .naming_date
                .as_deref()
                .filter(|value| valid_naming_date(value))
                .map(str::to_owned)
                .ok_or_else(|| {
                    error(
                        "OUTPUT_TEMPLATE_VALUE_UNAVAILABLE",
                        "{date} requires a valid local date snapshot in YYYY-MM-DD format.",
                        input,
                    )
                })?,
                false,
            ),
            _ => {
                return Err(error(
                    "OUTPUT_TEMPLATE_INVALID",
                    format!(
                        "Unknown filename token {{{token}}}. Available tokens: {{name}}, {{ext}}, {{index}}, {{codec}}, {{crf}}, {{quality}}, {{preset}}, {{width}}, {{height}}, and {{date}}."
                    ),
                    input,
                ));
            }
        };
        parts.push((replacement, source_name));
        rest = &rest[close + 1..];
    }
    parts.push((sanitized_fragment(rest), false));
    finish_rendered_component(parts, input)
}

fn reserve_output(
    directory: &Path,
    input: &Path,
    stem: &str,
    container: media_core::ContainerFormat,
    reserved: &mut HashSet<String>,
) -> Result<PathBuf, AppError> {
    for number in 1..=10_000 {
        let suffix = if number == 1 {
            String::new()
        } else {
            format!("_{number}")
        };
        let extension = container.extension();
        let candidate = directory.join(format!("{stem}{suffix}.{extension}"));
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

#[cfg(test)]
fn proposed_output(
    directory: &Path,
    input: &Path,
    encoder: VideoEncoder,
    container: media_core::ContainerFormat,
    reserved: &mut HashSet<String>,
) -> Result<PathBuf, AppError> {
    let stem = format!("{}_{}", safe_stem(input), codec_name(encoder));
    reserve_output(directory, input, &stem, container, reserved)
}

pub(crate) async fn inspect_selection(
    manager: &crate::JobManager,
    input: &BatchEncodeInput,
    settings: &EncodeSettings,
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
    manager.inspect_encode_source(input, settings, epoch).await
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
    let total = request.inputs.len();
    let mut items = Vec::with_capacity(total);
    for (index, input) in request.inputs.clone().into_iter().enumerate() {
        let settings = EncodeSettings {
            temporal: input.temporal,
            parameters: request.parameters.clone(),
            av1an_options: request.av1an_options,
            av1an_grain: request.av1an_grain.clone(),
            av1an_filters: request.av1an_filters.clone(),
            rate_control: request.rate_control,
            tone_map: input.tone_map,
            trim: input.trim,
            subtitles: input.subtitles.clone(),
            framing: input.framing,
            audio: input.audio.clone(),
            video_stream_index: input.video_stream_index,
            crf: request.crf,
            preset: request.preset,
            lossless: request.lossless,
            svt_crf_quarter_steps: request.svt_crf_quarter_steps,
            svt_preset: request.svt_preset,
            film_grain: request.film_grain,
            lineart_psy_bias: request.lineart_psy_bias,
            texture_psy_bias: request.texture_psy_bias,
            hdr_tune: request.hdr_tune,
            hdr10_fallback: request.hdr10_fallback,
            backend: request.backend,
            encoder: request.encoder,
            workers: request.workers,
        };
        let mut item = BatchEncodeItem {
            input_path: input.input_path.clone(),
            output_path: None,
            request: None,
            error: None,
        };
        match inspect_selection(manager, &input, &settings, epoch).await {
            Err(error) if matches!(error.code.as_str(), "BATCH_CANCELED" | "APP_CLOSING") => {
                return Err(error);
            }
            Err(error) => item.error = Some(error),
            Ok(media) => {
                match rendered_name(&request, &media, input.video_stream_index, index, total)
                    .and_then(|stem| {
                        reserve_output(
                            &directory,
                            Path::new(&media.path),
                            &stem,
                            request.output_container.unwrap_or_default(),
                            &mut reserved,
                        )
                    }) {
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
                            settings,
                        });
                    }
                }
            }
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

    fn naming_fixture(path: &Path) -> (BatchEncodeRequest, MediaFile) {
        let input_path = path.to_string_lossy().into_owned();
        (
            BatchEncodeRequest {
                parameters: Vec::new(),
                av1an_options: None,
                av1an_grain: None,
                av1an_filters: Vec::new(),
                output_container: None,
                rate_control: None,
                backend: EncodeBackend::Standalone,
                encoder: VideoEncoder::SvtAv1FiveFish,
                workers: 2,
                inputs: vec![BatchEncodeInput {
                    temporal: None,
                    tone_map: None,
                    trim: None,
                    subtitles: Vec::new(),
                    framing: Default::default(),
                    audio: Vec::new(),
                    input_path: input_path.clone(),
                    stream_indices: vec![3],
                    video_stream_index: 3,
                }],
                output_directory: path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_string_lossy()
                    .into_owned(),
                output_name_template: Some("{name}_{codec}".into()),
                naming_date: Some("2026-09-20".into()),
                crf: 30,
                preset: 0,
                lossless: false,
                svt_crf_quarter_steps: Some(121),
                svt_preset: Some(-1),
                film_grain: 0,
                lineart_psy_bias: 0,
                texture_psy_bias: 0,
                hdr_tune: Default::default(),
                hdr10_fallback: false,
            },
            MediaFile {
                id: "naming-fixture".into(),
                path: input_path,
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                size_bytes: "1".into(),
                duration_seconds: Some(1.0),
                format: None,
                title: None,
                language: None,
                bit_rate: None,
                streams: vec![media_core::MediaStream {
                    index: 3,
                    kind: "video".into(),
                    codec: Some("h264".into()),
                    codec_long_name: None,
                    profile: None,
                    bit_rate: None,
                    duration_seconds: None,
                    average_frame_rate: None,
                    nominal_frame_rate: None,
                    is_default: None,
                    attachment_filename: None,
                    attachment_mime_type: None,
                    width: Some(1920),
                    height: Some(1080),
                    sample_aspect_ratio: None,
                    display_aspect_ratio: None,
                    rotation_degrees: None,
                    frame_rate: Some("24/1".into()),
                    field_order: None,
                    sample_rate: None,
                    channels: None,
                    channel_layout: None,
                    language: None,
                    title: None,
                    pixel_format: Some("yuv420p".into()),
                    bit_depth: Some(8),
                    color_primaries: None,
                    color_transfer: None,
                    color_space: None,
                    color_range: None,
                    hdr_format: None,
                    has_hdr_static_metadata: None,
                    dynamic_hdr_formats: None,
                }],
            },
        )
    }

    #[test]
    fn naming_template_renders_case_insensitive_tokens_from_reviewed_settings() {
        let (mut request, media) = naming_fixture(Path::new("Episode One.MKV"));
        request.output_name_template = Some(
            "{NaMe}_{EXT}_{INDEX}_{CoDeC}_{CRF}_{quality}_{PRESET}_{width}x{height}_{DATE}".into(),
        );
        assert_eq!(
            rendered_name(&request, &media, 3, 8, 100).unwrap(),
            "Episode One_MKV_009_av1_5fish_30.25_30.25_-1_1920x1080_2026-09-20"
        );

        request.encoder = VideoEncoder::X264;
        request.preset = 9;
        request.svt_crf_quarter_steps = Some(7);
        request.svt_preset = Some(-3);
        request.output_name_template = Some("{preset}_{crf}".into());
        assert_eq!(
            rendered_name(&request, &media, 3, 0, 1).unwrap(),
            "placebo_30",
            "non-SVT encoders ignore irrelevant SVT extension fields"
        );

        let long_path = format!("{}.mkv", "日本語🙂".repeat(60));
        let (mut long_request, long_media) = naming_fixture(Path::new(&long_path));
        let expected_stem = safe_stem(Path::new(&long_path));
        let default_name = rendered_name(&long_request, &long_media, 3, 0, 1).unwrap();
        assert!(default_name.ends_with("_av1_5fish"));
        assert_eq!(
            default_name.len() - "_av1_5fish".len(),
            expected_stem.len(),
            "the default keeps the historical source-stem budget before its codec suffix"
        );
        assert!(expected_stem.len() > 150);
        long_request.output_name_template = Some("{name}_{index}".into());
        let indexed = rendered_name(&long_request, &long_media, 3, 8, 100).unwrap();
        assert!(indexed.ends_with("_009"));
        assert_eq!(indexed.len() - "_009".len(), expected_stem.len());
    }

    #[test]
    fn quality_token_represents_non_crf_modes_without_claiming_a_default_crf() {
        let (mut request, media) = naming_fixture(Path::new("source.mkv"));
        request.output_name_template = Some("{quality}".into());
        request.lossless = true;
        request.svt_crf_quarter_steps = None;
        assert_eq!(
            rendered_name(&request, &media, 3, 0, 1).unwrap(),
            "lossless"
        );

        request.lossless = false;
        request.rate_control = Some(VideoRateControl::Bitrate {
            bitrate_kbps: 2400,
            two_pass: true,
        });
        assert_eq!(
            rendered_name(&request, &media, 3, 0, 1).unwrap(),
            "2400kbps_2pass"
        );
        request.rate_control = Some(VideoRateControl::TargetSize {
            target_size_mib: 700,
        });
        assert_eq!(rendered_name(&request, &media, 3, 0, 1).unwrap(), "700MiB");

        request.rate_control = None;
        request.backend = EncodeBackend::Av1an;
        request.av1an_options = Some(media_core::Av1anOptions {
            target_quality: Some(media_core::Av1anTargetQuality {
                metric: Av1anTargetMetric::Ssimulacra2,
                minimum_score_tenths: 940,
                maximum_score_tenths: 965,
                minimum_crf: 15,
                maximum_crf: 50,
                probes: 4,
                probing_rate: 1,
                probe_width: 1920,
                probe_height: 1080,
            }),
            ..Default::default()
        });
        assert_eq!(
            rendered_name(&request, &media, 3, 0, 1).unwrap(),
            "ssimulacra2_94-96.5"
        );

        request.output_name_template = Some("{crf}".into());
        let error = rendered_name(&request, &media, 3, 0, 1).unwrap_err();
        assert_eq!(error.code, "OUTPUT_TEMPLATE_VALUE_UNAVAILABLE");
        assert!(error.message.contains("Use {quality}"));
    }

    #[test]
    fn naming_template_rejects_bad_syntax_and_sanitizes_final_components() {
        let (mut request, media) = naming_fixture(Path::new("CON.mkv"));
        request.output_name_template = Some("{name}".into());
        assert_eq!(rendered_name(&request, &media, 3, 0, 1).unwrap(), "_CON");
        assert_eq!(safe_component("con.txt"), "_con.txt");
        assert_eq!(safe_component("CON .foo"), "_CON .foo");
        assert_eq!(safe_component("COM1"), "_COM1");
        assert_eq!(safe_component("COM¹.txt"), "_COM¹.txt");
        assert_eq!(safe_component("LPT³"), "_LPT³");
        assert_eq!(safe_component("CONIN$.txt"), "_CONIN$.txt");
        assert_eq!(safe_component("conout$.log"), "_conout$.log");
        assert_eq!(safe_component("COM10"), "COM10");
        assert_eq!(safe_component("folder/name:*?"), "folder_name___");

        request.output_name_template = Some("{mystery}".into());
        let error = rendered_name(&request, &media, 3, 0, 1).unwrap_err();
        assert_eq!(error.code, "OUTPUT_TEMPLATE_INVALID");
        assert!(error.message.contains("Unknown filename token {mystery}"));

        request.output_name_template = Some("x".repeat(MAX_NAME_TEMPLATE_BYTES + 1));
        assert_eq!(
            rendered_name(&request, &media, 3, 0, 1).unwrap_err().code,
            "OUTPUT_TEMPLATE_TOO_LONG"
        );
        request.output_name_template = Some("x".repeat(MAX_OUTPUT_COMPONENT_BYTES + 1));
        assert_eq!(
            rendered_name(&request, &media, 3, 0, 1).unwrap_err().code,
            "OUTPUT_TEMPLATE_TOO_LONG"
        );
        request.output_name_template = Some("CONOUT$.txt".into());
        assert_eq!(
            rendered_name(&request, &media, 3, 0, 1).unwrap(),
            "_CONOUT$.txt"
        );

        request.output_name_template = Some("{date}".into());
        request.naming_date = Some("2026-02-29".into());
        assert_eq!(
            rendered_name(&request, &media, 3, 0, 1).unwrap_err().code,
            "OUTPUT_TEMPLATE_VALUE_UNAVAILABLE"
        );
        for malformed in ["2026-0/-20", "2026-09-2x", "２０２６-09-20"] {
            request.naming_date = Some(malformed.into());
            assert_eq!(
                rendered_name(&request, &media, 3, 0, 1).unwrap_err().code,
                "OUTPUT_TEMPLATE_VALUE_UNAVAILABLE"
            );
        }
        request.output_name_template = Some("{name".into());
        assert_eq!(
            rendered_name(&request, &media, 3, 0, 1).unwrap_err().code,
            "OUTPUT_TEMPLATE_INVALID"
        );
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
            let script = fixture.0.join("create-junction.ps1");
            fs::write(
                &script,
                "New-Item -ItemType Junction -Path (Join-Path $PSScriptRoot 'media\\linked outside') -Target (Join-Path $PSScriptRoot 'outside') -ErrorAction Stop | Out-Null",
            )
            .unwrap();
            let (_owner, cancel) = tokio::sync::watch::channel(false);
            let result = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(crate::supervisor::run_capture(
                    &crate::supervisor::CommandSpec {
                        executable: std::env::var_os("SystemRoot")
                            .map(PathBuf::from)
                            .unwrap()
                            .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
                        // PowerShell -File accepts ordinary native arguments;
                        // cmd /C uses its own incompatible quoting grammar.
                        args: [
                            "-NoProfile",
                            "-NonInteractive",
                            "-ExecutionPolicy",
                            "Bypass",
                            "-File",
                        ]
                        .into_iter()
                        .map(std::ffi::OsString::from)
                        .chain([script.into_os_string()])
                        .collect(),
                        cwd: None,
                    },
                    cancel,
                    65536,
                    // Cold PowerShell startup can exceed five seconds while
                    // the Windows CI runner starts the parallel runtime tests.
                    // This bounds fixture setup, not folder-scan performance.
                    std::time::Duration::from_secs(30),
                ))
                .expect("PowerShell must create the owned test junction within 30 seconds");
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
        let third = proposed_output(
            &directory,
            &input,
            VideoEncoder::SvtAv1,
            media_core::ContainerFormat::Matroska,
            &mut reserved,
        )
        .unwrap();
        let fourth = proposed_output(
            &directory,
            &input,
            VideoEncoder::SvtAv1,
            media_core::ContainerFormat::Matroska,
            &mut reserved,
        )
        .unwrap();
        assert_eq!(third.file_name().unwrap(), "Title 日本語_av1_3.mkv");
        assert_eq!(fourth.file_name().unwrap(), "Title 日本語_av1_4.mkv");
        assert!(!third.exists());
        assert!(!fourth.exists());
        assert_eq!(fs::read(existing).unwrap(), b"existing");
        assert_eq!(fs::read(input).unwrap(), b"source");
        assert_eq!(safe_stem(Path::new("bad:name?.mp4")), "bad_name_");
        assert_eq!(safe_stem(Path::new("  ... .mp4")), "media");
        let first_sanitized = proposed_output(
            &directory,
            Path::new("bad:name?.mp4"),
            VideoEncoder::X264,
            media_core::ContainerFormat::Matroska,
            &mut reserved,
        )
        .unwrap();
        let second_sanitized = proposed_output(
            &directory,
            Path::new("bad*name?.mov"),
            VideoEncoder::X264,
            media_core::ContainerFormat::Matroska,
            &mut reserved,
        )
        .unwrap();
        assert_eq!(first_sanitized.file_name().unwrap(), "bad_name__x264.mkv");
        assert_eq!(
            second_sanitized.file_name().unwrap(),
            "bad_name__x264_2.mkv"
        );
        assert!(!first_sanitized.exists() && !second_sanitized.exists());
        let long = format!("{}.mp4", "日本語🙂".repeat(60));
        let stem = safe_stem(Path::new(&long));
        assert!(stem.len() <= 160);
        assert!(
            stem.chars().count() > 30,
            "Unicode names remain recognizable"
        );
        let output = proposed_output(
            &directory,
            Path::new(&long),
            VideoEncoder::SvtAv1,
            media_core::ContainerFormat::Matroska,
            &mut reserved,
        )
        .unwrap();
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
    async fn x264_preview_propagates_encoder_before_inspecting_sources() {
        let fixture = Fixture::new();
        let manager = crate::JobManager::new(fixture.0.join("logs"));
        let request = BatchEncodeRequest {
            parameters: Vec::new(),
            av1an_options: None,
            av1an_grain: None,
            av1an_filters: Vec::new(),
            output_container: None,
            rate_control: None,
            lossless: false,
            svt_crf_quarter_steps: None,
            svt_preset: None,
            inputs: vec![BatchEncodeInput {
                temporal: None,
                tone_map: None,
                trim: None,
                subtitles: Vec::new(),
                framing: Default::default(),
                audio: Vec::new(),
                input_path: fixture.0.join("missing.mkv").to_string_lossy().into_owned(),
                stream_indices: vec![0],
                video_stream_index: 0,
            }],
            output_directory: fixture.0.to_string_lossy().into_owned(),
            output_name_template: None,
            naming_date: None,
            crf: 0, // Valid x264 CRF 0; the SVT default would reject it.
            preset: 5,
            film_grain: 0,
            lineart_psy_bias: 0,
            texture_psy_bias: 0,
            hdr_tune: Default::default(),
            hdr10_fallback: false,
            backend: Default::default(),
            encoder: VideoEncoder::X264,
            workers: 2,
        };
        let result = manager.preview_encode_batch(request.clone()).await.unwrap();
        assert_eq!(
            result.items[0].error.as_ref().unwrap().code,
            "FILE_NOT_FOUND"
        );
        let mut av1an_request = request.clone();
        av1an_request.backend = media_core::EncodeBackend::Av1an;
        let av1an = manager.preview_encode_batch(av1an_request).await.unwrap();
        assert_eq!(
            av1an.items[0].error.as_ref().unwrap().code,
            "FILE_NOT_FOUND"
        );
        for invalid in [
            BatchEncodeRequest {
                parameters: Vec::new(),
                av1an_options: None,
                av1an_grain: None,
                av1an_filters: Vec::new(),
                output_container: None,
                film_grain: 1,
                ..request.clone()
            },
            BatchEncodeRequest {
                parameters: Vec::new(),
                av1an_options: None,
                av1an_grain: None,
                av1an_filters: Vec::new(),
                output_container: None,
                hdr10_fallback: true,
                ..request.clone()
            },
        ] {
            assert_eq!(
                manager
                    .preview_encode_batch(invalid)
                    .await
                    .unwrap_err()
                    .code,
                "ENCODE_SETTINGS_INVALID"
            );
        }
        assert!(manager.list_jobs().await.is_empty());
    }

    #[test]
    fn x264_proposals_have_distinct_names_and_preserve_existing_destinations() {
        let fixture = Fixture::new();
        let input = fixture.0.join("Title 日本語.mkv");
        let existing = fixture.0.join("Title 日本語_x264.mkv");
        fs::write(&input, b"source").unwrap();
        fs::write(&existing, b"existing").unwrap();
        let mut reserved = HashSet::new();
        let second = proposed_output(
            &fixture.0,
            &input,
            VideoEncoder::X264,
            media_core::ContainerFormat::Matroska,
            &mut reserved,
        )
        .unwrap();
        let third = proposed_output(
            &fixture.0,
            &input,
            VideoEncoder::X264,
            media_core::ContainerFormat::Matroska,
            &mut reserved,
        )
        .unwrap();
        let av1 = proposed_output(
            &fixture.0,
            &input,
            VideoEncoder::SvtAv1,
            media_core::ContainerFormat::Matroska,
            &mut reserved,
        )
        .unwrap();
        assert_eq!(second.file_name().unwrap(), "Title 日本語_x264_2.mkv");
        assert_eq!(third.file_name().unwrap(), "Title 日本語_x264_3.mkv");
        assert_eq!(av1.file_name().unwrap(), "Title 日本語_av1.mkv");
        assert!(!second.exists() && !third.exists() && !av1.exists());
        assert_eq!(fs::read(input).unwrap(), b"source");
        assert_eq!(fs::read(existing).unwrap(), b"existing");
    }

    #[tokio::test]
    async fn preview_keeps_per_file_source_and_selection_errors_visible() {
        let fixture = Fixture::new();
        let manager = crate::JobManager::new(fixture.0.join("logs"));
        let result = manager
            .preview_encode_batch(BatchEncodeRequest {
                parameters: Vec::new(),
                av1an_options: None,
                av1an_grain: None,
                av1an_filters: Vec::new(),
                output_container: None,
                rate_control: None,
                lossless: false,
                svt_crf_quarter_steps: None,
                svt_preset: None,
                inputs: vec![
                    BatchEncodeInput {
                        temporal: None,
                        tone_map: None,
                        trim: None,
                        subtitles: Vec::new(),
                        framing: Default::default(),
                        audio: Vec::new(),
                        input_path: fixture.0.join("missing.mkv").to_string_lossy().into_owned(),
                        stream_indices: vec![0],
                        video_stream_index: 0,
                    },
                    BatchEncodeInput {
                        temporal: None,
                        tone_map: None,
                        trim: None,
                        subtitles: Vec::new(),
                        framing: Default::default(),
                        audio: Vec::new(),
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
                output_name_template: None,
                naming_date: None,
                crf: 30,
                preset: 4,
                film_grain: 0,
                lineart_psy_bias: 0,
                texture_psy_bias: 0,
                hdr_tune: Default::default(),
                hdr10_fallback: false,
                backend: Default::default(),
                encoder: Default::default(),
                workers: 2,
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
