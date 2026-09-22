//! Supervised, no-clobber media utilities.
//!
//! Commands are built as argument vectors and never evaluated by a shell. Every
//! source is held read-only for the complete operation. Outputs are written to
//! an owned sibling temporary, validated, and atomically published without
//! replacing an existing path.

use crate::{
    analysis::{check_cancel, fingerprint},
    discovery::find_executable,
    jobs::files::{Source, Temporary},
    supervisor::{
        ChildEnvironment, CommandSpec, SupervisorError, run_capture, run_capture_with_environment,
        run_streaming_stdout,
    },
};
use media_core::{
    AppError, ColorMetadataTransferRequest, ConcatRequest, CrfLadderRequest, CrfLadderResult,
    CrfLadderRung, GrainRequest, GrainSource, GrainTableResult, KeyframeCutRequest, LadderEncoder,
    LadderMetric, QualityMetric, QualityRequest, SubtitleOcrRequest, SubtitleOcrResult,
    UtilityArtifact, UtilityCapabilities, UtilityDependency, UtilityRequest, UtilityResult,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::watch;

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const MEDIA_LIMIT: Duration = Duration::from_secs(6 * 60 * 60);
const PROBE_LIMIT: Duration = Duration::from_secs(60);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn error(code: &str, message: impl Into<String>, path: Option<&Path>) -> AppError {
    AppError::new(
        code,
        message,
        path.map(|path| path.to_string_lossy().into_owned()),
    )
}

fn process_error(error_value: SupervisorError, path: Option<&Path>) -> AppError {
    match error_value {
        SupervisorError::Cancelled => error(
            "UTILITY_CANCELED",
            "The utility was canceled. No output was published.",
            path,
        ),
        SupervisorError::Timeout => error(
            "UTILITY_TIMEOUT",
            "The utility exceeded its execution time limit. No output was published.",
            path,
        ),
        SupervisorError::OutputLimit => error(
            "UTILITY_DIAGNOSTIC_LIMIT",
            "The tool exceeded the bounded diagnostic output limit.",
            path,
        ),
        other => error("UTILITY_TOOL_FAILED", other.to_string(), path),
    }
}

fn nonce(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{prefix}-{}-{nanos}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn os(value: impl AsRef<std::ffi::OsStr>) -> OsString {
    value.as_ref().to_owned()
}

fn valid_absolute(path: &str) -> Result<&Path, AppError> {
    let value = Path::new(path);
    if path.contains('\0') || !value.is_absolute() {
        return Err(error(
            "INVALID_PATH",
            "Use an absolute local file path.",
            Some(value),
        ));
    }
    Ok(value)
}

fn destination(path: &str, sources: &[&Source]) -> Result<PathBuf, AppError> {
    let requested = valid_absolute(path)?;
    let parent = requested.parent().ok_or_else(|| {
        error(
            "INVALID_OUTPUT",
            "Choose an output folder.",
            Some(requested),
        )
    })?;
    let parent = fs::canonicalize(parent).map_err(|cause| {
        error(
            "INVALID_OUTPUT",
            format!("The output folder could not be accessed: {cause}"),
            Some(parent),
        )
    })?;
    if !parent.is_dir() {
        return Err(error(
            "INVALID_OUTPUT",
            "The output parent must be an existing folder.",
            Some(&parent),
        ));
    }
    let name = requested.file_name().ok_or_else(|| {
        error(
            "INVALID_OUTPUT",
            "Choose an output filename.",
            Some(requested),
        )
    })?;
    let resolved = parent.join(name);
    for source in sources {
        let same = if cfg!(windows) {
            source
                .path
                .to_string_lossy()
                .eq_ignore_ascii_case(&resolved.to_string_lossy())
        } else {
            source.path == resolved
        };
        if same {
            return Err(error(
                "SOURCE_OUTPUT_COLLISION",
                "The destination must differ from every source file.",
                Some(&resolved),
            ));
        }
    }
    ensure_absent(&resolved)?;
    Ok(resolved)
}

fn ensure_absent(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path) {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(error(
            "OUTPUT_EXISTS",
            "The destination already exists. Existing files are never replaced.",
            Some(path),
        )),
        Err(cause) => Err(error(
            "OUTPUT_UNREADABLE",
            format!("The destination could not be checked: {cause}"),
            Some(path),
        )),
    }
}

async fn tool(name: &str, purpose: &str) -> Result<PathBuf, AppError> {
    find_utility_executable(&[name])
        .await
        .map_err(|detail| error("UTILITY_DEPENDENCY_CHECK_FAILED", detail, None))?
        .ok_or_else(|| {
            error(
                "UTILITY_DEPENDENCY_MISSING",
                format!(
                    "{purpose} requires {name}, but it was not found in the verified package, Jesses per-user utility directory, or absolute PATH entries. Install {name}, restart Jesses, and run Check utility tools."
                ),
                None,
            )
        })
}

fn managed_ocr_root() -> Result<Option<PathBuf>, String> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let Some(base) = base else { return Ok(None) };
    if !base.is_absolute() {
        return Err("The user data directory for managed OCR tools must be absolute.".into());
    }
    Ok(Some(base.join("jesses").join("tools").join("subtitle-ocr")))
}

fn managed_ocr_candidate(name: &str) -> Result<Option<PathBuf>, String> {
    let (directory, executable) = match name {
        "seconv" => (
            "seconv",
            if cfg!(windows) {
                "seconv.exe"
            } else {
                "seconv"
            },
        ),
        "tesseract" => (
            "tesseract",
            if cfg!(windows) {
                "tesseract.exe"
            } else {
                "tesseract"
            },
        ),
        _ => return Ok(None),
    };
    let Some(root) = managed_ocr_root()? else {
        return Ok(None);
    };
    let candidate = root.join(directory).join(executable);
    let metadata = match fs::symlink_metadata(&candidate) {
        Ok(metadata) => metadata,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => {
            return Err(format!(
                "Cannot inspect managed OCR tool {}: {cause}",
                candidate.display()
            ));
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "The managed OCR tool must be a regular file, not a directory or link: {}",
            candidate.display()
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(format!(
                "The managed OCR tool cannot be a reparse point: {}",
                candidate.display()
            ));
        }
    }
    let resolved_root = fs::canonicalize(&root).map_err(|cause| {
        format!(
            "Cannot resolve managed OCR directory {}: {cause}",
            root.display()
        )
    })?;
    let resolved = fs::canonicalize(&candidate).map_err(|cause| {
        format!(
            "Cannot resolve managed OCR tool {}: {cause}",
            candidate.display()
        )
    })?;
    if !resolved.starts_with(&resolved_root) {
        return Err(format!(
            "The managed OCR tool resolves outside its per-user directory: {}",
            candidate.display()
        ));
    }
    #[cfg(windows)]
    if !resolved
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err(format!(
            "The managed OCR tool must be a native .exe file: {}",
            resolved.display()
        ));
    }
    Ok(Some(candidate))
}

async fn find_utility_executable(names: &[&str]) -> Result<Option<PathBuf>, String> {
    for name in names {
        if let Some(path) = managed_ocr_candidate(name)? {
            return Ok(Some(path));
        }
    }
    find_executable(names).await
}

fn managed_tessdata(tesseract: &Path) -> Option<PathBuf> {
    let root = managed_ocr_root().ok().flatten()?;
    let root = fs::canonicalize(root).ok()?;
    let resolved = fs::canonicalize(tesseract).ok()?;
    if !resolved.starts_with(root) {
        return None;
    }
    let tessdata = tesseract.parent()?.join("tessdata");
    fs::symlink_metadata(&tessdata)
        .ok()
        .filter(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())?;
    Some(tessdata)
}

fn ocr_environment(tesseract: &Path) -> Result<ChildEnvironment, AppError> {
    let parent = tesseract.parent().ok_or_else(|| {
        error(
            "UTILITY_DEPENDENCY_CHECK_FAILED",
            "The resolved Tesseract executable has no parent directory.",
            Some(tesseract),
        )
    })?;
    let mut paths = vec![parent.to_path_buf()];
    if let Some(inherited) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&inherited).filter(|path| path.is_absolute()));
    }
    let path = std::env::join_paths(paths).map_err(|cause| {
        error(
            "UTILITY_DEPENDENCY_CHECK_FAILED",
            format!("A child-only OCR PATH could not be constructed: {cause}"),
            Some(tesseract),
        )
    })?;
    let variables = managed_tessdata(tesseract)
        .map(|path| vec![("TESSDATA_PREFIX", Some(path.into_os_string()))])
        .unwrap_or_default();
    Ok(ChildEnvironment {
        path: Some(path),
        variables,
    })
}

fn diagnostic_tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(2400)..]
        .iter()
        .collect::<String>()
        .trim()
        .to_owned()
}

async fn capture(
    executable: PathBuf,
    args: Vec<OsString>,
    cancel: &watch::Receiver<bool>,
    limit: Duration,
    path: Option<&Path>,
) -> Result<crate::supervisor::CapturedOutput, AppError> {
    run_capture(
        &CommandSpec {
            executable,
            args,
            cwd: None,
        },
        cancel.clone(),
        OUTPUT_LIMIT,
        limit,
    )
    .await
    .map_err(|cause| process_error(cause, path))
}

async fn capture_with_environment(
    executable: PathBuf,
    args: Vec<OsString>,
    cancel: &watch::Receiver<bool>,
    limit: Duration,
    path: Option<&Path>,
    environment: &ChildEnvironment,
) -> Result<crate::supervisor::CapturedOutput, AppError> {
    run_capture_with_environment(
        &CommandSpec {
            executable,
            args,
            cwd: None,
        },
        cancel.clone(),
        OUTPUT_LIMIT,
        limit,
        Some(environment),
    )
    .await
    .map_err(|cause| process_error(cause, path))
}

async fn checked(
    executable: PathBuf,
    args: Vec<OsString>,
    cancel: &watch::Receiver<bool>,
    limit: Duration,
    path: Option<&Path>,
    action: &str,
) -> Result<crate::supervisor::CapturedOutput, AppError> {
    let output = capture(executable, args, cancel, limit, path).await?;
    if output.status.success() {
        Ok(output)
    } else {
        let stderr = diagnostic_tail(&output.stderr);
        let stdout = diagnostic_tail(&output.stdout);
        Err(error(
            "UTILITY_TOOL_FAILED",
            format!(
                "{action} failed ({}). {}{}{}",
                output.status,
                stderr,
                if !stderr.is_empty() && !stdout.is_empty() {
                    " | "
                } else {
                    ""
                },
                stdout
            ),
            path,
        ))
    }
}

async fn checked_with_environment(
    executable: PathBuf,
    args: Vec<OsString>,
    cancel: &watch::Receiver<bool>,
    limit: Duration,
    path: Option<&Path>,
    action: &str,
    environment: &ChildEnvironment,
) -> Result<crate::supervisor::CapturedOutput, AppError> {
    let output =
        capture_with_environment(executable, args, cancel, limit, path, environment).await?;
    if output.status.success() {
        Ok(output)
    } else {
        let stderr = diagnostic_tail(&output.stderr);
        let stdout = diagnostic_tail(&output.stdout);
        Err(error(
            "UTILITY_TOOL_FAILED",
            format!(
                "{action} failed ({}). {}{}{}",
                output.status,
                stderr,
                if !stderr.is_empty() && !stdout.is_empty() {
                    " | "
                } else {
                    ""
                },
                stdout
            ),
            path,
        ))
    }
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn create(label: &str) -> Result<Self, AppError> {
        let path = std::env::temp_dir().join(nonce(&format!("jesses-{label}")));
        fs::create_dir(&path).map_err(|cause| {
            error(
                "UTILITY_SCRATCH_FAILED",
                format!("A private scratch directory could not be created: {cause}"),
                Some(&path),
            )
        })?;
        Ok(Self { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Ok(entries) = fs::read_dir(&self.path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_file() || kind.is_symlink())
                {
                    let _ = fs::remove_file(path);
                }
            }
        }
        let _ = fs::remove_dir(&self.path);
    }
}

async fn dependency(
    id: &str,
    names: &[&str],
    version_arg: &str,
    detail: &str,
    cancel: &watch::Receiver<bool>,
) -> UtilityDependency {
    let found = find_utility_executable(names).await;
    let path = match found {
        Ok(Some(path)) => path,
        Ok(None) => {
            return UtilityDependency {
                id: id.into(),
                available: false,
                path: None,
                version: None,
                detail: detail.into(),
            };
        }
        Err(cause) => {
            return UtilityDependency {
                id: id.into(),
                available: false,
                path: None,
                version: None,
                detail: cause,
            };
        }
    };
    let response = capture(
        path.clone(),
        vec![version_arg.into()],
        cancel,
        Duration::from_secs(10),
        None,
    )
    .await;
    match response {
        Ok(output) if output.status.success() => {
            let bytes = if output.stdout.is_empty() {
                &output.stderr
            } else {
                &output.stdout
            };
            UtilityDependency {
                id: id.into(),
                available: true,
                path: Some(path.to_string_lossy().into_owned()),
                version: String::from_utf8_lossy(bytes)
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| line.trim().chars().take(240).collect()),
                detail: "Available and responded to its version probe.".into(),
            }
        }
        Ok(output) => UtilityDependency {
            id: id.into(),
            available: false,
            path: Some(path.to_string_lossy().into_owned()),
            version: None,
            detail: format!("Version probe failed ({}).", output.status),
        },
        Err(cause) => UtilityDependency {
            id: id.into(),
            available: false,
            path: Some(path.to_string_lossy().into_owned()),
            version: None,
            detail: cause.message,
        },
    }
}

async fn ffmpeg_encoders(
    ffmpeg: Option<&Path>,
    cancel: &watch::Receiver<bool>,
) -> Vec<LadderEncoder> {
    let Some(ffmpeg) = ffmpeg else {
        return Vec::new();
    };
    let Ok(output) = capture(
        ffmpeg.to_owned(),
        vec!["-hide_banner".into(), "-encoders".into()],
        cancel,
        Duration::from_secs(15),
        None,
    )
    .await
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    [
        (LadderEncoder::H264, "libx264"),
        (LadderEncoder::Hevc, "libx265"),
        (LadderEncoder::Av1, "libsvtav1"),
        (LadderEncoder::Vp9, "libvpx-vp9"),
    ]
    .into_iter()
    .filter_map(|(encoder, name)| text.contains(name).then_some(encoder))
    .collect()
}

fn parse_grain_presets(bytes: &[u8]) -> Vec<String> {
    let mut presets = Vec::new();
    let mut applicable = Vec::new();
    let mut modifiers = Vec::new();
    let text = String::from_utf8_lossy(bytes);
    let mut section = 0_u8;
    for line in text.lines() {
        let line = line.trim();
        if line == "Available Presets:" {
            section = 1;
            continue;
        }
        if let Some(names) = line
            .strip_prefix("Available film stock modifiers (applies to ")
            .and_then(|line| line.strip_suffix("):"))
        {
            applicable = names
                .split(',')
                .map(str::trim)
                .filter(|name| valid_grain_preset_name(name))
                .map(str::to_owned)
                .collect();
            section = 2;
            continue;
        }
        if line.starts_with("Example:") {
            section = 0;
            continue;
        }
        if section == 1 && !line.is_empty() {
            let name = line.split("  (").next().unwrap_or_default().trim();
            if valid_grain_preset_name(name) && !presets.iter().any(|preset| preset == name) {
                presets.push(name.to_owned());
            }
        } else if section == 2 {
            let suffix = line.split_whitespace().next().unwrap_or_default();
            if let Some(number) = suffix.strip_prefix('-')
                && number
                    .parse::<u8>()
                    .is_ok_and(|number| (1..=99).contains(&number))
                && line.len() > suffix.len()
                && !modifiers.iter().any(|modifier| modifier == suffix)
            {
                modifiers.push(suffix.to_owned());
            }
        }
    }
    let bases = presets.clone();
    for base in bases.iter().filter(|base| applicable.contains(base)) {
        for modifier in &modifiers {
            let name = format!("{base}{modifier}");
            if valid_grain_preset_name(&name) {
                presets.push(name);
            }
        }
    }
    presets
}

fn valid_grain_preset_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

async fn ocr_languages(tesseract: Option<&Path>, cancel: &watch::Receiver<bool>) -> Vec<String> {
    let Some(tesseract) = tesseract else {
        return Vec::new();
    };
    let Ok(environment) = ocr_environment(tesseract) else {
        return Vec::new();
    };
    let Ok(output) = capture_with_environment(
        tesseract.to_owned(),
        vec!["--list-langs".into()],
        cancel,
        Duration::from_secs(20),
        None,
        &environment,
    )
    .await
    else {
        return Vec::new();
    };
    let mut languages = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && line.len() <= 32
                && line
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    languages.sort();
    languages.dedup();
    languages
}

pub async fn inspect_utility_capabilities(
    cancel: watch::Receiver<bool>,
) -> Result<UtilityCapabilities, AppError> {
    check_cancel(&cancel)?;
    let (ffmpeg, ffprobe, mkvmerge, grav1synth, seconv, tesseract) = tokio::join!(
        dependency(
            "ffmpeg",
            &["ffmpeg"],
            "-version",
            "Required by cut, sample encoding, and stream validation.",
            &cancel,
        ),
        dependency(
            "ffprobe",
            &["ffprobe"],
            "-version",
            "Required for semantic media validation.",
            &cancel,
        ),
        dependency(
            "mkvmerge",
            &["mkvmerge"],
            "--version",
            "Required by color metadata transfer.",
            &cancel,
        ),
        dependency(
            "grav1synth",
            &["grav1synth"],
            "--version",
            "Required by AV1 film-grain table and header operations.",
            &cancel,
        ),
        dependency(
            "seconv",
            &["seconv"],
            "--version",
            "Subtitle OCR requires Subtitle Edit's headless converter to decode timed bitmap subtitle formats.",
            &cancel,
        ),
        dependency(
            "tesseract",
            &["tesseract"],
            "--version",
            "Subtitle OCR with the Tesseract engine requires Tesseract and at least one installed language model.",
            &cancel,
        ),
    );
    check_cancel(&cancel)?;
    let ffmpeg_path = ffmpeg
        .available
        .then_some(ffmpeg.path.as_deref())
        .flatten()
        .map(Path::new);
    let tesseract_path = tesseract
        .available
        .then_some(tesseract.path.as_deref())
        .flatten()
        .map(Path::new);
    let grain_path = grav1synth
        .available
        .then_some(grav1synth.path.as_deref())
        .flatten()
        .map(Path::new);
    let (encoders, languages, grain_presets) = tokio::join!(
        ffmpeg_encoders(ffmpeg_path, &cancel),
        ocr_languages(tesseract_path, &cancel),
        async {
            let Some(path) = grain_path else {
                return Vec::new();
            };
            capture(
                path.to_owned(),
                vec!["presets".into()],
                &cancel,
                Duration::from_secs(10),
                None,
            )
            .await
            .ok()
            .filter(|output| output.status.success())
            .map(|output| parse_grain_presets(&output.stdout))
            .unwrap_or_default()
        }
    );
    check_cancel(&cancel)?;
    Ok(UtilityCapabilities {
        dependencies: vec![ffmpeg, ffprobe, mkvmerge, grav1synth, seconv, tesseract],
        ocr_languages: languages,
        grain_presets,
        ffmpeg_encoders: encoders,
        notes: vec![
            "Lossless cuts begin on a usable keyframe and can include content before the requested start.".into(),
            "CRF ladder projections carry sampled video bitrate across the source duration; they exclude audio, subtitles, and unsampled complexity changes.".into(),
            "Subtitle OCR preserves detected cue timing, but recognized text should be reviewed.".into(),
        ],
    })
}

#[derive(Debug, Deserialize)]
struct ProbeDocument {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    #[serde(default)]
    chapters: Vec<ProbeChapter>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    duration: Option<Value>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ProbeChapter {
    start_time: Option<Value>,
    end_time: Option<Value>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProbeStream {
    index: u32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    codec_tag_string: Option<String>,
    profile: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    sample_rate: Option<Value>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    time_base: Option<String>,
    avg_frame_rate: Option<String>,
    pix_fmt: Option<String>,
    field_order: Option<String>,
    sample_aspect_ratio: Option<String>,
    color_range: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    chroma_location: Option<String>,
    nb_read_frames: Option<Value>,
    extradata_size: Option<u64>,
    #[serde(default)]
    disposition: BTreeMap<String, u8>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

fn json_number(value: Option<&Value>) -> Option<f64> {
    let value = match value? {
        Value::String(value) => value.parse().ok(),
        Value::Number(value) => value.as_f64(),
        _ => None,
    }?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn json_u64(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::String(value) => value.parse().ok(),
        Value::Number(value) => value.as_u64(),
        _ => None,
    }
}

async fn probe(
    ffprobe: &Path,
    source: &Path,
    cancel: &watch::Receiver<bool>,
    count_frames: bool,
) -> Result<ProbeDocument, AppError> {
    let mut args = vec![
        "-v".into(),
        "error".into(),
        "-show_streams".into(),
        "-show_format".into(),
        "-show_chapters".into(),
    ];
    if count_frames {
        args.push("-count_frames".into());
    }
    args.extend(["-of".into(), "json".into(), "-i".into(), os(source)]);
    let output = checked(
        ffprobe.to_owned(),
        args,
        cancel,
        PROBE_LIMIT,
        Some(source),
        "Media inspection",
    )
    .await?;
    serde_json::from_slice(&output.stdout).map_err(|cause| {
        error(
            "UTILITY_PROBE_INVALID",
            format!("FFprobe returned invalid media metadata: {cause}"),
            Some(source),
        )
    })
}

fn duration(document: &ProbeDocument) -> Option<f64> {
    json_number(document.format.as_ref()?.duration.as_ref())
}

fn selected_stream<'a>(
    document: &'a ProbeDocument,
    index: u32,
    kind: &str,
    path: &Path,
) -> Result<&'a ProbeStream, AppError> {
    document
        .streams
        .iter()
        .find(|stream| stream.index == index && stream.codec_type.as_deref() == Some(kind))
        .ok_or_else(|| {
            error(
                "UTILITY_STREAM_INVALID",
                format!("Stream {index} is not a {kind} stream in this source."),
                Some(path),
            )
        })
}

fn clean_field(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .filter(|value| !matches!(*value, "unknown" | "unspecified" | "reserved" | "0/0"))
}

fn stream_signature(stream: &ProbeStream) -> BTreeMap<&'static str, String> {
    let mut signature = BTreeMap::new();
    macro_rules! text {
        ($name:literal, $value:expr) => {
            if let Some(value) = $value {
                signature.insert($name, value.to_owned());
            }
        };
    }
    text!("type", stream.codec_type.as_deref());
    text!("codec", stream.codec_name.as_deref());
    text!("codecTag", clean_field(&stream.codec_tag_string));
    text!("profile", stream.profile.as_deref());
    text!("timeBase", stream.time_base.as_deref());
    text!("frameRate", stream.avg_frame_rate.as_deref());
    text!("pixelFormat", stream.pix_fmt.as_deref());
    text!("channelLayout", stream.channel_layout.as_deref());
    text!(
        "fieldOrder",
        clean_field(&stream.field_order).filter(|value| *value != "progressive")
    );
    text!(
        "sampleAspectRatio",
        clean_field(&stream.sample_aspect_ratio)
    );
    text!("colorRange", clean_field(&stream.color_range));
    text!("colorSpace", clean_field(&stream.color_space));
    text!("colorTransfer", clean_field(&stream.color_transfer));
    text!("colorPrimaries", clean_field(&stream.color_primaries));
    text!("chromaLocation", clean_field(&stream.chroma_location));
    if let Some(value) = stream.width {
        signature.insert("width", value.to_string());
    }
    if let Some(value) = stream.height {
        signature.insert("height", value.to_string());
    }
    if let Some(value) = json_u64(stream.sample_rate.as_ref()) {
        signature.insert("sampleRate", value.to_string());
    }
    if let Some(value) = stream.channels {
        signature.insert("channels", value.to_string());
    }
    if let Some(value) = stream.extradata_size {
        signature.insert("extradataSize", value.to_string());
    }
    signature
}

fn stable_stream_tags(stream: &ProbeStream) -> BTreeMap<String, String> {
    stream
        .tags
        .iter()
        .filter_map(|(name, value)| {
            let upper = name.to_ascii_uppercase();
            (!matches!(
                upper.as_str(),
                "DURATION"
                    | "NUMBER_OF_FRAMES"
                    | "NUMBER_OF_BYTES"
                    | "BPS"
                    | "_STATISTICS_WRITING_APP"
                    | "_STATISTICS_WRITING_DATE_UTC"
                    | "_STATISTICS_TAGS"
            ))
            .then(|| (upper, value.clone()))
        })
        .collect()
}

fn stable_container_tags(tags: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    tags.iter()
        .filter_map(|(name, value)| {
            let upper = name.to_ascii_uppercase();
            (!matches!(
                upper.as_str(),
                "ENCODER" | "CREATION_TIME" | "DATE" | "WRITING_APP" | "MUXING_APP"
            ))
            .then(|| (upper, value.clone()))
        })
        .collect()
}

fn compatible_container_metadata(
    expected: &ProbeDocument,
    actual: &ProbeDocument,
    path: &Path,
) -> Result<(), AppError> {
    let expected_tags = expected
        .format
        .as_ref()
        .map(|format| stable_container_tags(&format.tags))
        .unwrap_or_default();
    let actual_tags = actual
        .format
        .as_ref()
        .map(|format| stable_container_tags(&format.tags))
        .unwrap_or_default();
    if expected_tags != actual_tags {
        return Err(error(
            "UTILITY_STREAM_MISMATCH",
            "The output did not retain the source's stable container metadata.",
            Some(path),
        ));
    }
    if expected.chapters.len() != actual.chapters.len() {
        return Err(error(
            "UTILITY_STREAM_MISMATCH",
            "The output did not retain every source chapter.",
            Some(path),
        ));
    }
    for (ordinal, (left, right)) in expected.chapters.iter().zip(&actual.chapters).enumerate() {
        let left_start = json_number(left.start_time.as_ref());
        let right_start = json_number(right.start_time.as_ref());
        let left_end = json_number(left.end_time.as_ref());
        let right_end = json_number(right.end_time.as_ref());
        let close = |left: Option<f64>, right: Option<f64>| match (left, right) {
            (Some(left), Some(right)) => (left - right).abs() <= 0.002,
            (None, None) => true,
            _ => false,
        };
        if !close(left_start, right_start)
            || !close(left_end, right_end)
            || stable_container_tags(&left.tags) != stable_container_tags(&right.tags)
        {
            return Err(error(
                "UTILITY_STREAM_MISMATCH",
                format!("Chapter {} timing or metadata changed.", ordinal + 1),
                Some(path),
            ));
        }
    }
    Ok(())
}

fn compatible_streams(
    expected: &ProbeDocument,
    actual: &ProbeDocument,
    path: &Path,
) -> Result<(), AppError> {
    compatible_streams_mode(expected, actual, path, true)
}

fn compatible_streams_ignoring_color(
    expected: &ProbeDocument,
    actual: &ProbeDocument,
    path: &Path,
) -> Result<(), AppError> {
    compatible_streams_mode(expected, actual, path, false)
}

fn compatible_streams_mode(
    expected: &ProbeDocument,
    actual: &ProbeDocument,
    path: &Path,
    compare_color: bool,
) -> Result<(), AppError> {
    if expected.streams.len() != actual.streams.len() {
        return Err(error(
            "UTILITY_STREAM_MISMATCH",
            "The files have different stream counts.",
            Some(path),
        ));
    }
    for (ordinal, (left, right)) in expected.streams.iter().zip(&actual.streams).enumerate() {
        let mut left_signature = stream_signature(left);
        let mut right_signature = stream_signature(right);
        if !compare_color {
            for name in [
                "colorRange",
                "colorSpace",
                "colorTransfer",
                "colorPrimaries",
                "chromaLocation",
            ] {
                left_signature.remove(name);
                right_signature.remove(name);
            }
        }
        if left_signature != right_signature {
            let differences = left_signature
                .keys()
                .chain(right_signature.keys())
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter(|name| left_signature.get(name) != right_signature.get(name))
                .map(|name| {
                    format!(
                        "{name}: {:?} -> {:?}",
                        left_signature.get(name),
                        right_signature.get(name)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(error(
                "UTILITY_STREAM_MISMATCH",
                format!(
                    "Stream {} is incompatible. Codec, profile, geometry, time base, frame rate, or audio layout differs ({differences}).",
                    ordinal + 1,
                ),
                Some(path),
            ));
        }
        if left.disposition != right.disposition {
            return Err(error(
                "UTILITY_STREAM_MISMATCH",
                format!("Stream {} did not retain its dispositions.", ordinal + 1),
                Some(path),
            ));
        }
        for (name, expected_value) in stable_stream_tags(left) {
            if right
                .tags
                .iter()
                .find(|(actual_name, _)| actual_name.eq_ignore_ascii_case(&name))
                .map(|(_, value)| value)
                != Some(&expected_value)
            {
                return Err(error(
                    "UTILITY_STREAM_MISMATCH",
                    format!(
                        "Stream {} did not retain the {name} metadata value.",
                        ordinal + 1
                    ),
                    Some(path),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct FrameTimeline {
    frame_count: u64,
    first: Option<f64>,
    last_timestamp: Option<f64>,
    last_end: Option<f64>,
    max_quantum: f64,
}

impl FrameTimeline {
    fn push(&mut self, timestamp: f64, frame_duration: Option<f64>) {
        if let Some(previous) = self.last_timestamp {
            let step = timestamp - previous;
            if step.is_finite() && step > 0.0 {
                self.max_quantum = self.max_quantum.max(step);
            }
        }
        let duration = frame_duration.filter(|value| value.is_finite() && *value > 0.0);
        if let Some(duration) = duration {
            self.max_quantum = self.max_quantum.max(duration);
        }
        self.frame_count = self.frame_count.saturating_add(1);
        self.first = Some(self.first.map_or(timestamp, |value| value.min(timestamp)));
        self.last_timestamp = Some(
            self.last_timestamp
                .map_or(timestamp, |value| value.max(timestamp)),
        );
        let end = timestamp + duration.unwrap_or(0.0);
        self.last_end = Some(self.last_end.map_or(end, |value| value.max(end)));
    }

    fn end(&self) -> Option<f64> {
        let last = self.last_timestamp?;
        Some(self.last_end.unwrap_or(last).max(last + self.max_quantum))
    }

    fn span(&self) -> Option<f64> {
        Some((self.end()? - self.first?).max(0.0))
    }
}

#[derive(Clone, Debug, Default)]
struct DecodedTimeline {
    streams: BTreeMap<u32, FrameTimeline>,
}

impl DecodedTimeline {
    fn origin(&self) -> Option<f64> {
        self.streams
            .values()
            .filter_map(|stream| stream.first)
            .min_by(f64::total_cmp)
    }

    fn end(&self) -> Option<f64> {
        self.streams
            .values()
            .filter_map(FrameTimeline::end)
            .max_by(f64::total_cmp)
    }

    fn span(&self) -> Option<f64> {
        Some((self.end()? - self.origin()?).max(0.0))
    }

    fn max_quantum(&self) -> f64 {
        self.streams
            .values()
            .map(|stream| stream.max_quantum)
            .fold(0.0, f64::max)
    }
}

fn parse_timeline(
    reader: &mut dyn Read,
    interval: Option<(f64, f64)>,
) -> Result<DecodedTimeline, String> {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut timeline = DecodedTimeline::default();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|cause| cause.to_string())?;
        if read == 0 {
            break;
        }
        if line.len() > 4096 {
            return Err("FFprobe returned an unexpectedly long frame record.".into());
        }
        let mut kind = None;
        let mut index = None;
        let mut timestamp = None;
        let mut frame_duration = None;
        for field in line.trim().split('|') {
            let Some((name, value)) = field.split_once('=') else {
                continue;
            };
            match name {
                "media_type" => kind = Some(value),
                "stream_index" => index = value.parse::<u32>().ok(),
                "best_effort_timestamp_time" => timestamp = value.parse::<f64>().ok(),
                "duration_time" => frame_duration = value.parse::<f64>().ok(),
                _ => {}
            }
        }
        if !matches!(kind, Some("video" | "audio")) {
            continue;
        }
        let (Some(index), Some(timestamp)) = (index, timestamp) else {
            return Err(
                "A decoded audio/video frame had no usable stream index or timestamp.".into(),
            );
        };
        if !timestamp.is_finite() {
            return Err("A decoded frame had a non-finite timestamp.".into());
        }
        if let Some((start, end)) = interval
            && (timestamp < start - 0.000_001 || timestamp >= end - 0.000_001)
        {
            continue;
        }
        timeline
            .streams
            .entry(index)
            .or_default()
            .push(timestamp, frame_duration);
    }
    if timeline.streams.is_empty() {
        return Err("No decoded audio/video frames were returned.".into());
    }
    Ok(timeline)
}

async fn decoded_timeline(
    ffprobe: &Path,
    source: &Path,
    interval: Option<(f64, f64)>,
    cancel: &watch::Receiver<bool>,
) -> Result<DecodedTimeline, AppError> {
    let mut args = vec![
        "-v".into(),
        "error".into(),
        "-show_frames".into(),
        "-show_entries".into(),
        "frame=stream_index,media_type,best_effort_timestamp_time,duration_time".into(),
        "-of".into(),
        "compact=p=0:nk=0".into(),
    ];
    args.extend(["-i".into(), os(source)]);
    let output = run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        OUTPUT_LIMIT,
        MEDIA_LIMIT,
        move |reader| parse_timeline(reader, interval),
    )
    .await
    .map_err(|cause| process_error(cause, Some(source)))?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(error(
            "UTILITY_DECODE_VALIDATION_FAILED",
            format!(
                "Decoded frame inspection failed ({}). {}",
                output.status,
                diagnostic_tail(&output.stderr)
            ),
            Some(source),
        ));
    }
    Ok(output.value)
}

fn parse_packet_end(reader: &mut dyn Read) -> Result<Option<f64>, String> {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut end: Option<f64> = None;
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|cause| cause.to_string())?;
        if read == 0 {
            break;
        }
        if line.len() > 4096 {
            return Err("FFprobe returned an unexpectedly long packet record.".into());
        }
        let fields = line
            .trim()
            .split('|')
            .filter_map(|field| field.split_once('='))
            .collect::<BTreeMap<_, _>>();
        let Some(timestamp) = fields
            .get("pts_time")
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite())
        else {
            continue;
        };
        let packet_end = fields
            .get("duration_time")
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
            .map_or(timestamp, |duration| timestamp + duration);
        end = Some(end.map_or(packet_end, |current| current.max(packet_end)));
    }
    Ok(end)
}

async fn packet_end(
    ffprobe: &Path,
    source: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<f64>, AppError> {
    let output = run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args: vec![
                "-v".into(),
                "error".into(),
                "-show_packets".into(),
                "-show_entries".into(),
                "packet=pts_time,duration_time".into(),
                "-of".into(),
                "compact=p=0:nk=0".into(),
                "-i".into(),
                os(source),
            ],
            cwd: None,
        },
        cancel.clone(),
        OUTPUT_LIMIT,
        MEDIA_LIMIT,
        parse_packet_end,
    )
    .await
    .map_err(|cause| process_error(cause, Some(source)))?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            format!(
                "Packet timing inspection failed ({}). {}",
                output.status,
                diagnostic_tail(&output.stderr)
            ),
            Some(source),
        ));
    }
    Ok(output.value)
}

async fn validate_full_decode(
    ffmpeg: &Path,
    source: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    checked(
        ffmpeg.to_owned(),
        vec![
            "-hide_banner".into(),
            "-v".into(),
            "error".into(),
            "-xerror".into(),
            "-err_detect".into(),
            "explode".into(),
            "-nostdin".into(),
            "-i".into(),
            os(source),
            "-map".into(),
            "0:v?".into(),
            "-map".into(),
            "0:a?".into(),
            "-sn".into(),
            "-dn".into(),
            "-f".into(),
            "null".into(),
            "-".into(),
        ],
        cancel,
        MEDIA_LIMIT,
        Some(source),
        "Decoded output validation",
    )
    .await?;
    Ok(())
}

fn timeline_tolerance(left: &DecodedTimeline, right: &DecodedTimeline, boundaries: usize) -> f64 {
    let quantum = left.max_quantum().max(right.max_quantum());
    0.02 + quantum * (boundaries as f64 + 2.0)
}

fn validate_cut_timeline(
    expected: &DecodedTimeline,
    actual: &DecodedTimeline,
    path: &Path,
) -> Result<f64, AppError> {
    let expected_origin = expected.origin().ok_or_else(|| {
        error(
            "UTILITY_VALIDATION_FAILED",
            "The selected interval has no decoded audio/video frames.",
            Some(path),
        )
    })?;
    let actual_origin = actual.origin().ok_or_else(|| {
        error(
            "UTILITY_VALIDATION_FAILED",
            "The cut has no decoded audio/video frames.",
            Some(path),
        )
    })?;
    let tolerance = timeline_tolerance(expected, actual, 1);
    if expected.streams.len() != actual.streams.len() {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "The cut did not retain every decoded audio/video stream.",
            Some(path),
        ));
    }
    for (index, expected_stream) in &expected.streams {
        let actual_stream = actual.streams.get(index).ok_or_else(|| {
            error(
                "UTILITY_VALIDATION_FAILED",
                format!("The cut has no decoded frames for stream {index}."),
                Some(path),
            )
        })?;
        if expected_stream
            .frame_count
            .abs_diff(actual_stream.frame_count)
            > 2
        {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "Stream {index} decoded {} frames in the selected source interval but {} frames in the cut.",
                    expected_stream.frame_count, actual_stream.frame_count
                ),
                Some(path),
            ));
        }
        let expected_span = expected_stream.span().unwrap_or(0.0);
        let actual_span = actual_stream.span().unwrap_or(0.0);
        if (expected_span - actual_span).abs() > tolerance {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "Stream {index} decoded span changed from {expected_span:.6}s to {actual_span:.6}s, beyond the packet/frame-derived {tolerance:.6}s tolerance."
                ),
                Some(path),
            ));
        }
        let expected_offset = expected_stream.first.unwrap_or(expected_origin) - expected_origin;
        let actual_offset = actual_stream.first.unwrap_or(actual_origin) - actual_origin;
        if (expected_offset - actual_offset).abs() > tolerance {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "Stream {index} start offset changed by more than the packet/frame-derived {tolerance:.6}s tolerance."
                ),
                Some(path),
            ));
        }
    }
    Ok(tolerance)
}

fn validate_concat_timeline(
    inputs: &[DecodedTimeline],
    actual: &DecodedTimeline,
    path: &Path,
) -> Result<f64, AppError> {
    let first = inputs.first().ok_or_else(|| {
        error(
            "UTILITY_VALIDATION_FAILED",
            "No concatenation timeline was available.",
            Some(path),
        )
    })?;
    if first.streams.len() != actual.streams.len() {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "The concatenated output did not retain every decoded audio/video stream.",
            Some(path),
        ));
    }
    let actual_origin = actual.origin().unwrap_or(0.0);
    let first_origin = first.origin().unwrap_or(0.0);
    let mut max_input_quantum = 0.0_f64;
    for timeline in inputs {
        max_input_quantum = max_input_quantum.max(timeline.max_quantum());
    }
    let tolerance =
        0.02 + max_input_quantum.max(actual.max_quantum()) * (inputs.len() as f64 + 2.0);
    for (index, first_stream) in &first.streams {
        let actual_stream = actual.streams.get(index).ok_or_else(|| {
            error(
                "UTILITY_VALIDATION_FAILED",
                format!("The concatenated output has no decoded frames for stream {index}."),
                Some(path),
            )
        })?;
        let expected_count = inputs.iter().try_fold(0_u64, |total, timeline| {
            timeline
                .streams
                .get(index)
                .map(|stream| total.saturating_add(stream.frame_count))
                .ok_or(())
        });
        let Ok(expected_count) = expected_count else {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!("An input has no decoded frames for stream {index}."),
                Some(path),
            ));
        };
        if actual_stream.frame_count != expected_count {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "Stream {index} should contain {expected_count} decoded frames after concatenation but contains {}.",
                    actual_stream.frame_count
                ),
                Some(path),
            ));
        }
        let expected_offset = first_stream.first.unwrap_or(first_origin) - first_origin;
        let actual_offset = actual_stream.first.unwrap_or(actual_origin) - actual_origin;
        if (expected_offset - actual_offset).abs() > tolerance {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "Stream {index} start offset changed by more than the packet/frame-derived {tolerance:.6}s tolerance."
                ),
                Some(path),
            ));
        }
    }
    Ok(tolerance)
}

fn validate_preserved_timeline(
    expected: &DecodedTimeline,
    actual: &DecodedTimeline,
    path: &Path,
) -> Result<f64, AppError> {
    let expected_origin = expected.origin().ok_or_else(|| {
        error(
            "UTILITY_VALIDATION_FAILED",
            "The source has no decoded audio/video timeline.",
            Some(path),
        )
    })?;
    let actual_origin = actual.origin().ok_or_else(|| {
        error(
            "UTILITY_VALIDATION_FAILED",
            "The output has no decoded audio/video timeline.",
            Some(path),
        )
    })?;
    if expected.streams.len() != actual.streams.len() {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "The output did not retain every decoded audio/video stream.",
            Some(path),
        ));
    }
    let tolerance = timeline_tolerance(expected, actual, 0);
    for (index, expected_stream) in &expected.streams {
        let actual_stream = actual.streams.get(index).ok_or_else(|| {
            error(
                "UTILITY_VALIDATION_FAILED",
                format!("The output has no decoded frames for stream {index}."),
                Some(path),
            )
        })?;
        if expected_stream.frame_count != actual_stream.frame_count {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "Stream {index} decoded {} frames before the rewrite and {} afterward.",
                    expected_stream.frame_count, actual_stream.frame_count
                ),
                Some(path),
            ));
        }
        let expected_span = expected_stream.span().unwrap_or(0.0);
        let actual_span = actual_stream.span().unwrap_or(0.0);
        if (expected_span - actual_span).abs() > tolerance {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "Stream {index} decoded span changed from {expected_span:.6}s to {actual_span:.6}s, beyond the packet/frame-derived {tolerance:.6}s tolerance."
                ),
                Some(path),
            ));
        }
        let expected_offset = expected_stream.first.unwrap_or(expected_origin) - expected_origin;
        let actual_offset = actual_stream.first.unwrap_or(actual_origin) - actual_origin;
        if (expected_offset - actual_offset).abs() > tolerance {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "Stream {index} start offset changed by more than the packet/frame-derived {tolerance:.6}s tolerance."
                ),
                Some(path),
            ));
        }
    }
    Ok(tolerance)
}

fn require_mkv(path: &Path, operation: &str) -> Result<(), AppError> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mkv"))
    {
        Ok(())
    } else {
        Err(error(
            "UTILITY_OUTPUT_FORMAT_UNSUPPORTED",
            format!("{operation} currently publishes Matroska output; choose a .mkv filename."),
            Some(path),
        ))
    }
}

async fn packet_hash(
    ffmpeg: &Path,
    source: &Path,
    stream_index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<String, AppError> {
    let output = checked(
        ffmpeg.to_owned(),
        vec![
            "-hide_banner".into(),
            "-v".into(),
            "error".into(),
            "-nostdin".into(),
            "-i".into(),
            os(source),
            "-map".into(),
            format!("0:{stream_index}").into(),
            "-c".into(),
            "copy".into(),
            "-f".into(),
            "hash".into(),
            "-hash".into(),
            "sha256".into(),
            "-".into(),
        ],
        cancel,
        MEDIA_LIMIT,
        Some(source),
        "Packet hash verification",
    )
    .await?;
    let text = String::from_utf8_lossy(&output.stdout);
    let hash = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("SHA256="))
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| {
            error(
                "UTILITY_VALIDATION_FAILED",
                "FFmpeg did not return a valid packet hash.",
                Some(source),
            )
        })?;
    Ok(hash.to_ascii_lowercase())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PacketEvidence {
    stream_index: u32,
    packet_count: u64,
    duration_count: u64,
    last_end_ticks: Option<i128>,
    content_and_timing_hash: String,
}

fn parse_packet_evidence(
    reader: &mut dyn Read,
    expected_stream_index: u32,
) -> Result<PacketEvidence, String> {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut hasher = Sha256::new();
    let mut packet_count = 0_u64;
    let mut duration_count = 0_u64;
    let mut last_end_ticks = None;
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|cause| cause.to_string())?;
        if read == 0 {
            break;
        }
        if line.len() > 4096 {
            return Err("FFprobe returned an unexpectedly long packet record.".into());
        }
        let fields = line
            .trim()
            .split('|')
            .filter_map(|field| field.split_once('='))
            .collect::<BTreeMap<_, _>>();
        let stream_index = fields
            .get("stream_index")
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or("A packet has no usable stream index.")?;
        if stream_index != expected_stream_index {
            return Err("FFprobe returned a packet from an unselected stream.".into());
        }
        let pts = *fields.get("pts").ok_or("A packet has no PTS field.")?;
        let dts = *fields.get("dts").ok_or("A packet has no DTS field.")?;
        let size = *fields
            .get("size")
            .filter(|value| value.parse::<u64>().is_ok())
            .ok_or("A packet has no usable size.")?;
        let flags = *fields.get("flags").ok_or("A packet has no flags field.")?;
        let data_hash = fields
            .get("data_hash")
            .and_then(|value| value.strip_prefix("SHA256:"))
            .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or("A packet has no valid SHA-256 payload hash.")?;
        let stream_index_text = stream_index.to_string();
        for value in [stream_index_text.as_str(), pts, dts, size, flags, data_hash] {
            hasher.update(value.as_bytes());
            hasher.update([0]);
        }
        if let Some(duration) = fields.get("duration").filter(|value| **value != "N/A") {
            let duration = duration
                .parse::<u64>()
                .map_err(|_| "A packet has an invalid duration.")?;
            duration_count = duration_count.saturating_add(1);
            if let Ok(pts) = pts.parse::<i64>() {
                last_end_ticks = Some(i128::from(pts) + i128::from(duration));
            }
        }
        packet_count = packet_count
            .checked_add(1)
            .ok_or("The stream contains too many packets.")?;
    }
    if packet_count == 0 {
        return Err(
            "The stream contains no packets, so its payload cannot be preserved safely.".into(),
        );
    }
    Ok(PacketEvidence {
        stream_index: expected_stream_index,
        packet_count,
        duration_count,
        last_end_ticks,
        content_and_timing_hash: format!("{:x}", hasher.finalize()),
    })
}

async fn packet_evidence(
    ffprobe: &Path,
    source: &Path,
    stream_index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<PacketEvidence, AppError> {
    let args = vec![
        "-v".into(),
        "error".into(),
        "-select_streams".into(),
        stream_index.to_string().into(),
        "-show_packets".into(),
        "-show_data_hash".into(),
        "sha256".into(),
        "-show_entries".into(),
        "packet=stream_index,pts,dts,duration,size,flags,data_hash".into(),
        "-of".into(),
        "compact=p=0:nk=0".into(),
        "-i".into(),
        os(source),
    ];
    let output = run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        OUTPUT_LIMIT,
        MEDIA_LIMIT,
        move |reader| parse_packet_evidence(reader, stream_index),
    )
    .await
    .map_err(|cause| process_error(cause, Some(source)))?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            format!(
                "Packet preservation inspection failed ({}). {}",
                output.status,
                diagnostic_tail(&output.stderr)
            ),
            Some(source),
        ));
    }
    Ok(output.value)
}

async fn nonvideo_packet_evidence(
    ffprobe: &Path,
    source: &Path,
    document: &ProbeDocument,
    cancel: &watch::Receiver<bool>,
) -> Result<Vec<PacketEvidence>, AppError> {
    let mut evidence = Vec::new();
    for stream in document
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() != Some("video"))
    {
        check_cancel(cancel)?;
        evidence.push(packet_evidence(ffprobe, source, stream.index, cancel).await?);
    }
    Ok(evidence)
}

fn validate_nonvideo_packet_evidence(
    expected: &[PacketEvidence],
    actual: &[PacketEvidence],
    path: &Path,
) -> Result<(), AppError> {
    if expected.len() != actual.len() {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "The output did not retain every non-video stream.",
            Some(path),
        ));
    }
    for (expected, actual) in expected.iter().zip(actual) {
        if expected.stream_index != actual.stream_index
            || expected.packet_count != actual.packet_count
            || expected.duration_count != actual.duration_count
            || expected.content_and_timing_hash != actual.content_and_timing_hash
            || match (expected.last_end_ticks, actual.last_end_ticks) {
                (Some(expected), Some(actual)) => expected.abs_diff(actual) > 2,
                (None, None) => false,
                _ => true,
            }
        {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "Non-video stream {} did not retain its packet payloads, order, timestamps, flags, sizes, and duration coverage.",
                    expected.stream_index
                ),
                Some(path),
            ));
        }
    }
    Ok(())
}

struct GrainMediaEvidence {
    decoded_timeline: DecodedTimeline,
    nonvideo_packets: Vec<PacketEvidence>,
}

async fn grain_media_evidence(
    ffprobe: &Path,
    source: &Path,
    document: &ProbeDocument,
    cancel: &watch::Receiver<bool>,
) -> Result<GrainMediaEvidence, AppError> {
    Ok(GrainMediaEvidence {
        decoded_timeline: decoded_timeline(ffprobe, source, None, cancel).await?,
        nonvideo_packets: nonvideo_packet_evidence(ffprobe, source, document, cancel).await?,
    })
}

async fn validate_grain_media(
    ffmpeg: &Path,
    ffprobe: &Path,
    actual_path: &Path,
    expected_document: &ProbeDocument,
    actual_document: &ProbeDocument,
    expected_evidence: &GrainMediaEvidence,
    cancel: &watch::Receiver<bool>,
) -> Result<f64, AppError> {
    compatible_streams(expected_document, actual_document, actual_path)?;
    compatible_container_metadata(expected_document, actual_document, actual_path)?;
    let actual_evidence =
        grain_media_evidence(ffprobe, actual_path, actual_document, cancel).await?;
    validate_full_decode(ffmpeg, actual_path, cancel).await?;
    let tolerance = validate_preserved_timeline(
        &expected_evidence.decoded_timeline,
        &actual_evidence.decoded_timeline,
        actual_path,
    )?;
    validate_nonvideo_packet_evidence(
        &expected_evidence.nonvideo_packets,
        &actual_evidence.nonvideo_packets,
        actual_path,
    )?;
    if duration(expected_document)
        .zip(duration(actual_document))
        .is_some_and(|(expected, actual)| (expected - actual).abs() > tolerance)
    {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            format!(
                "The container duration changed beyond the packet/frame-derived {tolerance:.6}s tolerance."
            ),
            Some(actual_path),
        ));
    }
    Ok(tolerance)
}

async fn publish_media(
    mut temporary: Temporary,
    output: &Path,
    sources: &[&Source],
) -> Result<u64, AppError> {
    for source in sources {
        source.verify()?;
    }
    temporary.flush_nonempty_async().await?;
    let bytes = fs::metadata(&temporary.path)
        .map_err(|cause| {
            error(
                "OUTPUT_UNREADABLE",
                cause.to_string(),
                Some(&temporary.path),
            )
        })?
        .len();
    temporary.publish(output)?;
    temporary.cleanup()?;
    Ok(bytes)
}

async fn import_artifact(
    source: &Path,
    temporary: &Temporary,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let mut input = tokio::fs::File::open(source).await.map_err(|cause| {
        error(
            "OUTPUT_UNREADABLE",
            format!("The tool output could not be opened: {cause}"),
            Some(source),
        )
    })?;
    let file = temporary.clone_file()?;
    let mut output = tokio::fs::File::from_std(file);
    output.set_len(0).await.map_err(|cause| {
        error(
            "OUTPUT_WRITE_FAILED",
            cause.to_string(),
            Some(&temporary.path),
        )
    })?;
    output.seek(SeekFrom::Start(0)).await.map_err(|cause| {
        error(
            "OUTPUT_WRITE_FAILED",
            cause.to_string(),
            Some(&temporary.path),
        )
    })?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = input
            .read(&mut buffer)
            .await
            .map_err(|cause| error("OUTPUT_UNREADABLE", cause.to_string(), Some(source)))?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count]).await.map_err(|cause| {
            error(
                "OUTPUT_WRITE_FAILED",
                cause.to_string(),
                Some(&temporary.path),
            )
        })?;
    }
    output.flush().await.map_err(|cause| {
        error(
            "OUTPUT_FLUSH_FAILED",
            cause.to_string(),
            Some(&temporary.path),
        )
    })
}

async fn keyframe_at_or_before(
    ffprobe: &Path,
    source: &Path,
    stream_index: u32,
    requested: f64,
    cancel: &watch::Receiver<bool>,
) -> Result<f64, AppError> {
    let interval = format!("{requested:.9}%+0.25");
    let output = checked(
        ffprobe.to_owned(),
        vec![
            "-v".into(),
            "error".into(),
            "-threads".into(),
            "2".into(),
            "-select_streams".into(),
            stream_index.to_string().into(),
            "-skip_frame".into(),
            "nokey".into(),
            "-read_intervals".into(),
            interval.into(),
            "-show_frames".into(),
            "-show_entries".into(),
            "frame=best_effort_timestamp_time".into(),
            "-of".into(),
            "csv=p=0".into(),
            "-i".into(),
            os(source),
        ],
        cancel,
        PROBE_LIMIT,
        Some(source),
        "Keyframe inspection",
    )
    .await?;
    let mut candidates = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0 && *value <= requested + 0.002)
        .collect::<Vec<_>>();
    candidates.sort_by(f64::total_cmp);
    candidates.first().copied().or_else(|| (requested <= 0.002).then_some(0.0)).ok_or_else(|| {
        error(
            "UTILITY_KEYFRAME_NOT_FOUND",
            "No usable video keyframe was found at or before the requested start. Choose another start point.",
            Some(source),
        )
    })
}

async fn run_keyframe_cut(
    request: KeyframeCutRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    if !request.start_seconds.is_finite()
        || !request.end_seconds.is_finite()
        || request.start_seconds < 0.0
        || request.end_seconds <= request.start_seconds
    {
        return Err(error(
            "UTILITY_INVALID_INTERVAL",
            "Choose a finite end after a non-negative start.",
            Some(Path::new(&request.input_path)),
        ));
    }
    let source = Source::open(valid_absolute(&request.input_path)?)?;
    let output = destination(&request.output_path, &[&source])?;
    require_mkv(&output, "Lossless cut")?;
    let identity = fingerprint(&source)?;
    let ffprobe = tool("ffprobe", "Lossless keyframe cuts").await?;
    let ffmpeg = tool("ffmpeg", "Lossless keyframe cuts").await?;
    let input = probe(&ffprobe, &source.path, cancel, false).await?;
    let video = input
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| {
            error(
                "UTILITY_STREAM_INVALID",
                "Lossless cut needs at least one video stream.",
                Some(&source.path),
            )
        })?;
    let source_duration = duration(&input).ok_or_else(|| {
        error(
            "UTILITY_DURATION_UNAVAILABLE",
            "Lossless keyframe cutting requires a reliable source duration.",
            Some(&source.path),
        )
    })?;
    if request.start_seconds >= source_duration {
        return Err(error(
            "UTILITY_INVALID_INTERVAL",
            "The requested start is at or beyond the source duration.",
            Some(&source.path),
        ));
    }
    let actual_start = keyframe_at_or_before(
        &ffprobe,
        &source.path,
        video.index,
        request.start_seconds,
        cancel,
    )
    .await?;
    let actual_end = request.end_seconds.min(source_duration);
    if actual_end <= actual_start {
        return Err(error(
            "UTILITY_INVALID_INTERVAL",
            "The keyframe-aligned interval contains no media.",
            Some(&source.path),
        ));
    }
    let temporary = Temporary::create(&output, &nonce("cut"))?;
    let tool_output = checked(
        ffmpeg.clone(),
        vec![
            "-hide_banner".into(),
            "-v".into(),
            "warning".into(),
            "-nostdin".into(),
            "-ss".into(),
            format!("{actual_start:.9}").into(),
            "-i".into(),
            os(&source.path),
            "-t".into(),
            format!("{:.9}", actual_end - actual_start).into(),
            "-map".into(),
            "0".into(),
            "-map_metadata".into(),
            "0".into(),
            "-map_chapters".into(),
            "0".into(),
            "-c".into(),
            "copy".into(),
            "-avoid_negative_ts".into(),
            "make_zero".into(),
            "-y".into(),
            "-f".into(),
            "matroska".into(),
            os(&temporary.path),
        ],
        cancel,
        MEDIA_LIMIT,
        Some(&source.path),
        "Lossless keyframe cut",
    )
    .await?;
    let result_probe = probe(&ffprobe, &temporary.path, cancel, false).await?;
    compatible_streams(&input, &result_probe, &temporary.path)?;
    let expected_timeline = decoded_timeline(
        &ffprobe,
        &source.path,
        Some((actual_start, actual_end)),
        cancel,
    )
    .await?;
    let result_timeline = decoded_timeline(&ffprobe, &temporary.path, None, cancel).await?;
    validate_full_decode(&ffmpeg, &temporary.path, cancel).await?;
    validate_cut_timeline(&expected_timeline, &result_timeline, &temporary.path)?;
    let produced_duration = duration(&result_probe).ok_or_else(|| {
        error(
            "UTILITY_VALIDATION_FAILED",
            "The cut has no measurable duration.",
            Some(&temporary.path),
        )
    })?;
    let decoded_end = result_timeline.end().unwrap_or(produced_duration);
    let retained_end = packet_end(&ffprobe, &temporary.path, cancel)
        .await?
        .unwrap_or(decoded_end)
        .max(decoded_end);
    let container_tolerance = 0.02 + result_timeline.max_quantum() * 2.0;
    if produced_duration <= 0.0 || (produced_duration - retained_end).abs() > container_tolerance {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            format!(
                "The cut duration ({produced_duration:.6}s) differs from its last retained packet/frame end ({retained_end:.6}s) beyond the packet/frame-derived {container_tolerance:.6}s tolerance."
            ),
            Some(&temporary.path),
        ));
    }
    check_cancel(cancel)?;
    let bytes = publish_media(temporary, &output, &[&source]).await?;
    Ok(UtilityResult::Artifact(UtilityArtifact {
        operation: "keyframeCut".into(),
        output_path: output.to_string_lossy().into_owned(),
        size_bytes: bytes.to_string(),
        duration_seconds: Some(produced_duration),
        source_fingerprints: vec![identity],
        message: format!(
            "Copied streams without re-encoding. The requested interval was {:.3}s to {:.3}s; the output begins at the usable seek keyframe at {:.3}s (start pre-roll {:.3}s).",
            request.start_seconds,
            request.end_seconds,
            actual_start,
            request.start_seconds - actual_start,
        ),
        diagnostics: [
            diagnostic_tail(&tool_output.stdout),
            diagnostic_tail(&tool_output.stderr),
        ]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect(),
    }))
}

async fn run_concat(
    request: ConcatRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    if !(2..=512).contains(&request.input_paths.len()) {
        return Err(error(
            "UTILITY_CONCAT_INPUTS_INVALID",
            "Choose between 2 and 512 input files in playback order.",
            None,
        ));
    }
    let mut sources = Vec::with_capacity(request.input_paths.len());
    let mut canonical = BTreeSet::new();
    for path in &request.input_paths {
        let source = Source::open(valid_absolute(path)?)?;
        let key = if cfg!(windows) {
            source.path.to_string_lossy().to_ascii_lowercase()
        } else {
            source.path.to_string_lossy().into_owned()
        };
        if !canonical.insert(key) {
            return Err(error(
                "UTILITY_CONCAT_INPUTS_INVALID",
                "Each concatenation input must be a distinct file.",
                Some(&source.path),
            ));
        }
        sources.push(source);
    }
    let refs = sources.iter().collect::<Vec<_>>();
    let output = destination(&request.output_path, &refs)?;
    require_mkv(&output, "Concatenation")?;
    let ffprobe = tool("ffprobe", "Concatenation").await?;
    let ffmpeg = tool("ffmpeg", "Concatenation").await?;
    let mut probes = Vec::with_capacity(sources.len());
    let mut timelines = Vec::with_capacity(sources.len());
    let mut identities = Vec::with_capacity(sources.len());
    for source in &sources {
        check_cancel(cancel)?;
        identities.push(fingerprint(source)?);
        let document = probe(&ffprobe, &source.path, cancel, false).await?;
        if let Some(first) = probes.first() {
            compatible_streams(first, &document, &source.path)?;
        }
        probes.push(document);
        timelines.push(decoded_timeline(&ffprobe, &source.path, None, cancel).await?);
    }
    let expected_duration = probes
        .iter()
        .map(duration)
        .collect::<Option<Vec<_>>>()
        .map(|durations| durations.into_iter().sum::<f64>())
        .unwrap_or_else(|| timelines.iter().filter_map(DecodedTimeline::span).sum());
    let scratch = Scratch::create("concat")?;
    let manifest = scratch.path.join("inputs.ffconcat");
    let mut manifest_text = String::from("ffconcat version 1.0\n");
    for source in &sources {
        let text = source.path.to_string_lossy();
        if text.contains('\r') || text.contains('\n') {
            return Err(error(
                "INVALID_PATH",
                "Concatenation paths cannot contain line breaks.",
                Some(&source.path),
            ));
        }
        let normalized = text.replace('\\', "/").replace('\'', "'\\''");
        manifest_text.push_str("file '");
        manifest_text.push_str(&normalized);
        manifest_text.push_str("'\n");
    }
    fs::write(&manifest, manifest_text).map_err(|cause| {
        error(
            "UTILITY_SCRATCH_FAILED",
            format!("The concat manifest could not be written: {cause}"),
            Some(&manifest),
        )
    })?;
    let temporary = Temporary::create(&output, &nonce("concat"))?;
    let tool_output = checked(
        ffmpeg.clone(),
        vec![
            "-hide_banner".into(),
            "-v".into(),
            "warning".into(),
            "-nostdin".into(),
            "-protocol_whitelist".into(),
            "file,pipe".into(),
            "-f".into(),
            "concat".into(),
            "-safe".into(),
            "0".into(),
            "-i".into(),
            os(&manifest),
            "-map".into(),
            "0".into(),
            "-map_metadata".into(),
            "0".into(),
            "-map_chapters".into(),
            "0".into(),
            "-c".into(),
            "copy".into(),
            "-avoid_negative_ts".into(),
            "make_zero".into(),
            "-y".into(),
            "-f".into(),
            "matroska".into(),
            os(&temporary.path),
        ],
        cancel,
        MEDIA_LIMIT,
        Some(&output),
        "Lossless concatenation",
    )
    .await?;
    let result_probe = probe(&ffprobe, &temporary.path, cancel, false).await?;
    compatible_streams(&probes[0], &result_probe, &temporary.path)?;
    let result_timeline = decoded_timeline(&ffprobe, &temporary.path, None, cancel).await?;
    validate_full_decode(&ffmpeg, &temporary.path, cancel).await?;
    let tolerance = validate_concat_timeline(&timelines, &result_timeline, &temporary.path)?;
    let produced_duration = duration(&result_probe).ok_or_else(|| {
        error(
            "UTILITY_VALIDATION_FAILED",
            "The concatenated output has no measurable duration.",
            Some(&temporary.path),
        )
    })?;
    if expected_duration > 0.0 && (produced_duration - expected_duration).abs() > tolerance {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            format!(
                "The concatenated duration ({produced_duration:.3}s) differs from the input sum ({expected_duration:.3}s) beyond the {tolerance:.3}s tolerance."
            ),
            Some(&temporary.path),
        ));
    }
    check_cancel(cancel)?;
    let bytes = publish_media(temporary, &output, &refs).await?;
    Ok(UtilityResult::Artifact(UtilityArtifact {
        operation: "concat".into(),
        output_path: output.to_string_lossy().into_owned(),
        size_bytes: bytes.to_string(),
        duration_seconds: Some(produced_duration),
        source_fingerprints: identities,
        message: format!(
            "Appended {} compatible media segments without re-encoding.",
            sources.len()
        ),
        diagnostics: [
            diagnostic_tail(&tool_output.stdout),
            diagnostic_tail(&tool_output.stderr),
        ]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect(),
    }))
}

#[derive(Debug, Deserialize)]
struct MkvDocument {
    #[serde(default)]
    tracks: Vec<MkvTrack>,
    #[serde(default)]
    errors: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MkvTrack {
    id: u32,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    properties: BTreeMap<String, Value>,
}

async fn mkv_document(
    mkvmerge: &Path,
    path: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<MkvDocument, AppError> {
    let output = capture(
        mkvmerge.to_owned(),
        vec!["--ui-language".into(), "en".into(), "-J".into(), os(path)],
        cancel,
        PROBE_LIMIT,
        Some(path),
    )
    .await?;
    let document: MkvDocument = serde_json::from_slice(&output.stdout).map_err(|cause| {
        error(
            "UTILITY_PROBE_INVALID",
            format!("mkvmerge returned invalid identification data: {cause}"),
            Some(path),
        )
    })?;
    if !output.status.success() || !document.errors.is_empty() {
        return Err(error(
            "UTILITY_PROBE_FAILED",
            format!(
                "mkvmerge could not identify this file: {} {}",
                document.errors.join("; "),
                diagnostic_tail(&output.stderr)
            ),
            Some(path),
        ));
    }
    Ok(document)
}

fn video_ordinal(
    document: &ProbeDocument,
    stream_index: u32,
    path: &Path,
) -> Result<usize, AppError> {
    let mut ordinal = 0;
    for stream in &document.streams {
        if stream.codec_type.as_deref() == Some("video") {
            if stream.index == stream_index {
                return Ok(ordinal);
            }
            ordinal += 1;
        }
    }
    Err(error(
        "UTILITY_STREAM_INVALID",
        format!("Stream {stream_index} is not a video stream."),
        Some(path),
    ))
}

fn mkv_video(document: &MkvDocument, ordinal: usize) -> Option<&MkvTrack> {
    document
        .tracks
        .iter()
        .filter(|track| track.kind == "video")
        .nth(ordinal)
}

fn mkv_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() && value.len() <= 256 => {
            Some(value.clone())
        }
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

const COLOR_PROPERTIES: [(&str, &str); 10] = [
    ("color_matrix_coefficients", "--colour-matrix"),
    (
        "color_transfer_characteristics",
        "--colour-transfer-characteristics",
    ),
    ("color_primaries", "--colour-primaries"),
    ("color_range", "--colour-range"),
    ("max_content_light", "--max-content-light"),
    ("max_frame_light", "--max-frame-light"),
    ("chromaticity_coordinates", "--chromaticity-coordinates"),
    ("white_color_coordinates", "--white-colour-coordinates"),
    ("max_luminance", "--max-luminance"),
    ("min_luminance", "--min-luminance"),
];

fn transferable_color(track: &MkvTrack) -> Vec<(&'static str, &'static str, String)> {
    COLOR_PROPERTIES
        .into_iter()
        .filter_map(|(property, option)| {
            mkv_value(track.properties.get(property)?).map(|value| (property, option, value))
        })
        .collect()
}

fn mkv_field_order(value: &str) -> Option<u8> {
    match value {
        "progressive" => Some(0),
        "tt" => Some(1),
        "unknown" => Some(2),
        "bb" => Some(6),
        "tb" => Some(9),
        "bt" => Some(14),
        _ => None,
    }
}

async fn run_color_transfer(
    request: ColorMetadataTransferRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    let metadata_source = Source::open(valid_absolute(&request.metadata_source_path)?)?;
    let input = Source::open(valid_absolute(&request.input_path)?)?;
    let output = destination(&request.output_path, &[&metadata_source, &input])?;
    require_mkv(&output, "Color metadata transfer")?;
    let ffprobe = tool("ffprobe", "Color metadata transfer").await?;
    let ffmpeg = tool("ffmpeg", "Color metadata transfer packet verification").await?;
    let mkvmerge = tool("mkvmerge", "Color metadata transfer").await?;
    let source_probe = probe(&ffprobe, &metadata_source.path, cancel, false).await?;
    let input_probe = probe(&ffprobe, &input.path, cancel, false).await?;
    selected_stream(
        &source_probe,
        request.metadata_source_video_stream_index,
        "video",
        &metadata_source.path,
    )?;
    selected_stream(
        &input_probe,
        request.input_video_stream_index,
        "video",
        &input.path,
    )?;
    let source_ordinal = video_ordinal(
        &source_probe,
        request.metadata_source_video_stream_index,
        &metadata_source.path,
    )?;
    let target_ordinal =
        video_ordinal(&input_probe, request.input_video_stream_index, &input.path)?;
    let source_mkv = mkv_document(&mkvmerge, &metadata_source.path, cancel).await?;
    let target_mkv = mkv_document(&mkvmerge, &input.path, cancel).await?;
    let source_track = mkv_video(&source_mkv, source_ordinal).ok_or_else(|| {
        error(
            "UTILITY_STREAM_INVALID",
            "mkvmerge could not map the selected metadata-source video track.",
            Some(&metadata_source.path),
        )
    })?;
    let target_track = mkv_video(&target_mkv, target_ordinal).ok_or_else(|| {
        error(
            "UTILITY_STREAM_INVALID",
            "mkvmerge could not map the selected target video track.",
            Some(&input.path),
        )
    })?;
    let properties = transferable_color(source_track);
    if properties.is_empty() {
        return Err(error(
            "UTILITY_COLOR_METADATA_MISSING",
            "The selected metadata source exposes no supported color, luminance, chromaticity, or light-level tags.",
            Some(&metadata_source.path),
        ));
    }
    let before_hash = packet_hash(
        &ffmpeg,
        &input.path,
        request.input_video_stream_index,
        cancel,
    )
    .await?;
    let scratch = Scratch::create("color")?;
    let tool_path = scratch.path.join("color-transfer.mkv");
    let mut args = vec![
        "--ui-language".into(),
        "en".into(),
        "-o".into(),
        os(&tool_path),
    ];
    for (_, option, value) in &properties {
        args.push((*option).into());
        args.push(format!("{}:{value}", target_track.id).into());
    }
    for (ordinal, stream) in input_probe
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .enumerate()
    {
        if let Some(order) = stream.field_order.as_deref().and_then(mkv_field_order)
            && let Some(track) = mkv_video(&target_mkv, ordinal)
        {
            args.push("--field-order".into());
            args.push(format!("{}:{order}", track.id).into());
        }
    }
    args.push(os(&input.path));
    let tool_output = checked(
        mkvmerge.clone(),
        args,
        cancel,
        MEDIA_LIMIT,
        Some(&input.path),
        "Color metadata remux",
    )
    .await?;
    let output_probe = probe(&ffprobe, &tool_path, cancel, false).await?;
    compatible_streams_ignoring_color(&input_probe, &output_probe, &tool_path)?;
    let output_stream = output_probe
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .nth(target_ordinal)
        .ok_or_else(|| {
            error(
                "UTILITY_VALIDATION_FAILED",
                "The target video stream is missing from the remuxed output.",
                Some(&tool_path),
            )
        })?;
    let after_hash = packet_hash(&ffmpeg, &tool_path, output_stream.index, cancel).await?;
    if before_hash != after_hash {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "Encoded video packet data changed during metadata transfer. The output was not published.",
            Some(&tool_path),
        ));
    }
    let output_mkv = mkv_document(&mkvmerge, &tool_path, cancel).await?;
    let written_track = mkv_video(&output_mkv, target_ordinal).ok_or_else(|| {
        error(
            "UTILITY_VALIDATION_FAILED",
            "The remuxed output does not expose the target video track.",
            Some(&tool_path),
        )
    })?;
    for (property, _, expected) in &properties {
        let actual = written_track
            .properties
            .get(*property)
            .and_then(mkv_value)
            .unwrap_or_default();
        if actual != *expected {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "The remuxed output did not retain {property} (expected {expected}, found {actual})."
                ),
                Some(&tool_path),
            ));
        }
    }
    check_cancel(cancel)?;
    let identities = vec![fingerprint(&metadata_source)?, fingerprint(&input)?];
    let temporary = Temporary::create(&output, &nonce("color"))?;
    import_artifact(&tool_path, &temporary, cancel).await?;
    let bytes = publish_media(temporary, &output, &[&metadata_source, &input]).await?;
    Ok(UtilityResult::Artifact(UtilityArtifact {
        operation: "colorMetadataTransfer".into(),
        output_path: output.to_string_lossy().into_owned(),
        size_bytes: bytes.to_string(),
        duration_seconds: duration(&output_probe),
        source_fingerprints: identities,
        message: format!(
            "Transferred {} supported color/HDR container properties. Encoded video packet SHA-256 is unchanged.",
            properties.len()
        ),
        diagnostics: [
            diagnostic_tail(&tool_output.stdout),
            diagnostic_tail(&tool_output.stderr),
        ]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect(),
    }))
}

fn valid_ocr_language(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.split('+').all(|part| {
            !part.is_empty()
                && part.len() <= 32
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

fn srt_time(value: &str) -> Option<u64> {
    let (hours, rest) = value.trim().split_once(':')?;
    let (minutes, rest) = rest.split_once(':')?;
    let (seconds, millis) = rest.split_once(',')?;
    let hours = hours.parse::<u64>().ok()?;
    let minutes = minutes.parse::<u64>().ok()?;
    let seconds = seconds.parse::<u64>().ok()?;
    let millis = millis.parse::<u64>().ok()?;
    (minutes < 60 && seconds < 60 && millis < 1000)
        .then_some((((hours * 60 + minutes) * 60 + seconds) * 1000) + millis)
}

fn validate_srt(bytes: &[u8]) -> Result<u32, String> {
    if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 || bytes.contains(&0) {
        return Err("The OCR output is empty, too large, or contains binary data.".into());
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "The OCR output is not valid UTF-8 text.")?
        .trim_start_matches('\u{feff}');
    let mut count = 0_u32;
    let mut previous_start = 0_u64;
    let mut saw_text = false;
    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if let Some((start, end)) = line.split_once(" --> ") {
            let start =
                srt_time(start).ok_or("The OCR output contains an invalid cue start time.")?;
            let end = srt_time(end.split_whitespace().next().unwrap_or_default())
                .ok_or("The OCR output contains an invalid cue end time.")?;
            if end <= start || (count > 0 && start < previous_start) {
                return Err("The OCR output contains reversed or out-of-order cue timing.".into());
            }
            previous_start = start;
            count = count
                .checked_add(1)
                .ok_or("The OCR output contains too many cues.")?;
        } else if !line.is_empty() && line.parse::<u32>().is_err() {
            saw_text = true;
        }
    }
    if count == 0 || !saw_text {
        return Err("The OCR output contains no timed recognized text cues.".into());
    }
    Ok(count)
}

async fn run_subtitle_ocr(
    request: SubtitleOcrRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    if !valid_ocr_language(&request.language) {
        return Err(error(
            "UTILITY_OCR_LANGUAGE_INVALID",
            "Choose one or more installed Tesseract language codes separated by '+'.",
            Some(Path::new(&request.input_path)),
        ));
    }
    let source = Source::open(valid_absolute(&request.input_path)?)?;
    let output = destination(&request.output_path, &[&source])?;
    if !output
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("srt"))
    {
        return Err(error(
            "UTILITY_OUTPUT_FORMAT_UNSUPPORTED",
            "Subtitle OCR publishes SubRip text; choose a .srt filename.",
            Some(&output),
        ));
    }
    let ffprobe = tool("ffprobe", "Subtitle OCR stream validation").await?;
    let ffmpeg = tool("ffmpeg", "Subtitle OCR track extraction").await?;
    let seconv = tool("seconv", "Timed bitmap subtitle OCR").await?;
    let tesseract = tool("tesseract", "Subtitle OCR text recognition").await?;
    let installed = ocr_languages(Some(&tesseract), cancel).await;
    let missing = request
        .language
        .split('+')
        .filter(|language| !installed.iter().any(|item| item == language))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(error(
            "UTILITY_OCR_MODEL_MISSING",
            format!(
                "The Tesseract language model(s) {} are not installed. Install the corresponding .traineddata files in Tesseract's reported tessdata directory, then run Check utility tools.",
                missing.join(", ")
            ),
            Some(&source.path),
        ));
    }
    let document = probe(&ffprobe, &source.path, cancel, false).await?;
    let subtitle = selected_stream(
        &document,
        request.subtitle_stream_index,
        "subtitle",
        &source.path,
    )?;
    if !matches!(
        subtitle.codec_name.as_deref(),
        Some("hdmv_pgs_subtitle" | "dvd_subtitle" | "dvb_subtitle" | "xsub")
    ) {
        return Err(error(
            "UTILITY_OCR_STREAM_UNSUPPORTED",
            format!(
                "Stream {} uses {}, not a supported bitmap subtitle codec (PGS, VobSub, DVB subtitle, or XSUB). Text subtitle tracks do not need OCR.",
                request.subtitle_stream_index,
                subtitle.codec_name.as_deref().unwrap_or("an unknown codec")
            ),
            Some(&source.path),
        ));
    }
    let scratch = Scratch::create("ocr")?;
    let extracted = scratch.path.join("selected.mks");
    checked(
        ffmpeg,
        vec![
            "-hide_banner".into(),
            "-v".into(),
            "warning".into(),
            "-nostdin".into(),
            "-i".into(),
            os(&source.path),
            "-map".into(),
            format!("0:{}", request.subtitle_stream_index).into(),
            "-c".into(),
            "copy".into(),
            "-y".into(),
            "-f".into(),
            "matroska".into(),
            os(&extracted),
        ],
        cancel,
        MEDIA_LIMIT,
        Some(&source.path),
        "Bitmap subtitle extraction",
    )
    .await?;
    if fs::metadata(&extracted)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
        == 0
    {
        return Err(error(
            "UTILITY_OCR_EMPTY_STREAM",
            "The selected bitmap subtitle track contains no extractable events.",
            Some(&source.path),
        ));
    }
    let generated = scratch.path.join("ocr.srt");
    let environment = ocr_environment(&tesseract)?;
    let tool_output = checked_with_environment(
        seconv,
        vec![
            os(&extracted),
            "subrip".into(),
            "--track-number:1".into(),
            "--ocr-engine:tesseract".into(),
            format!("--ocr-language:{}", request.language).into(),
            format!("--output-folder:{}", scratch.path.to_string_lossy()).into(),
            "--output-filename:ocr.srt".into(),
            "--encoding:utf-8".into(),
            "--overwrite".into(),
            "--json".into(),
        ],
        cancel,
        MEDIA_LIMIT,
        Some(&source.path),
        "Subtitle OCR",
        &environment,
    )
    .await?;
    let mut bytes = Vec::new();
    fs::File::open(&generated)
        .and_then(|file| file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|cause| {
            error(
                "UTILITY_OCR_OUTPUT_MISSING",
                format!("Subtitle OCR did not produce its requested SRT file: {cause}"),
                Some(&generated),
            )
        })?;
    let cue_count = validate_srt(&bytes)
        .map_err(|detail| error("UTILITY_OCR_OUTPUT_INVALID", detail, Some(&generated)))?;
    source.verify()?;
    check_cancel(cancel)?;
    let mut temporary = Temporary::create(&output, &nonce("ocr"))?;
    {
        let mut file = temporary.clone_file()?;
        file.set_len(0).map_err(|cause| {
            error(
                "OUTPUT_WRITE_FAILED",
                cause.to_string(),
                Some(&temporary.path),
            )
        })?;
        file.seek(SeekFrom::Start(0)).map_err(|cause| {
            error(
                "OUTPUT_WRITE_FAILED",
                cause.to_string(),
                Some(&temporary.path),
            )
        })?;
        file.write_all(&bytes).map_err(|cause| {
            error(
                "OUTPUT_WRITE_FAILED",
                cause.to_string(),
                Some(&temporary.path),
            )
        })?;
        file.sync_all().map_err(|cause| {
            error(
                "OUTPUT_FLUSH_FAILED",
                cause.to_string(),
                Some(&temporary.path),
            )
        })?;
    }
    temporary.flush_nonempty_async().await?;
    source.verify()?;
    temporary.publish(&output)?;
    temporary.cleanup()?;
    Ok(UtilityResult::SubtitleOcr(SubtitleOcrResult {
        output_path: output.to_string_lossy().into_owned(),
        cue_count,
        language: request.language,
        source_fingerprint: fingerprint(&source)?,
        message: format!(
            "Recognized {cue_count} timed subtitle cues with Tesseract. Review names, punctuation, line breaks, and hearing-impaired text before publishing."
        ),
        diagnostics: [
            diagnostic_tail(&tool_output.stdout),
            diagnostic_tail(&tool_output.stderr),
        ]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect(),
    }))
}

pub fn read_av1an_grain_table(path: String) -> Result<String, AppError> {
    let source = Source::open(valid_absolute(&path)?)?;
    let mut bytes = Vec::new();
    fs::File::open(&source.path)
        .and_then(|file| file.take(262145).read_to_end(&mut bytes))
        .map_err(|e| error("GRAIN_TABLE_UNREADABLE", e.to_string(), Some(&source.path)))?;
    source.verify()?;
    if bytes.len() > 262144 {
        return Err(error(
            "GRAIN_TABLE_INVALID",
            "Encode grain tables must be at most 256 KiB.",
            Some(&source.path),
        ));
    }
    validate_grain_table(&bytes)
        .map_err(|e| error("GRAIN_TABLE_INVALID", e, Some(&source.path)))?;
    String::from_utf8(bytes)
        .map_err(|e| error("GRAIN_TABLE_INVALID", e.to_string(), Some(&source.path)))
}

pub async fn make_av1an_grain_preset(
    preset: String,
    cancel: watch::Receiver<bool>,
) -> Result<String, AppError> {
    let _permit = crate::analysis::permit(&cancel).await?;
    if !valid_grain_preset_name(&preset) {
        return Err(error(
            "GRAIN_PRESET_INVALID",
            "Choose an advertised film-stock preset.",
            None,
        ));
    }
    let grav = tool("grav1synth", "Film-stock presets").await?;
    let ffmpeg = tool("ffmpeg", "Film-stock preset preparation").await?;
    let svt = tool("SvtAv1EncApp", "Film-stock preset reference encoding").await?;
    let listed = checked(
        grav.clone(),
        vec!["presets".into()],
        &cancel,
        Duration::from_secs(15),
        None,
        "Film-stock presets",
    )
    .await?;
    if !parse_grain_presets(&listed.stdout)
        .iter()
        .any(|p| p == &preset)
    {
        return Err(error(
            "GRAIN_PRESET_INVALID",
            "This film-stock preset is not advertised by the installed tool.",
            None,
        ));
    }
    let scratch = Scratch::create("encode-grain-preset")?;
    let stub = scratch.path.join("stub.mkv");
    let grained = scratch.path.join("grained.mkv");
    let table = scratch.path.join("grain.tbl");
    let raw = scratch.path.join("stub.y4m");
    let encoded = scratch.path.join("stub.ivf");
    // The packaged FFmpeg deliberately uses standalone video encoders. Keep
    // preset generation on that same toolchain instead of requiring libsvtav1.
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "color=black:s=64x64:r=24:d=15",
        "-pix_fmt",
        "yuv420p",
        "-f",
        "yuv4mpegpipe",
        "-n",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    args.push(os(&raw));
    checked(
        ffmpeg.clone(),
        args,
        &cancel,
        Duration::from_secs(120),
        None,
        "Film-stock reference frames",
    )
    .await?;
    checked(
        svt,
        vec![
            "-i".into(),
            os(&raw),
            "-b".into(),
            os(&encoded),
            "--preset".into(),
            "12".into(),
            "--crf".into(),
            "63".into(),
            "--input-depth".into(),
            "8".into(),
            "--lp".into(),
            "2".into(),
        ],
        &cancel,
        Duration::from_secs(120),
        None,
        "AV1 preset reference",
    )
    .await?;
    checked(
        ffmpeg,
        vec![
            "-v".into(),
            "error".into(),
            "-nostdin".into(),
            "-i".into(),
            os(&encoded),
            "-c:v".into(),
            "copy".into(),
            "-n".into(),
            os(&stub),
        ],
        &cancel,
        Duration::from_secs(120),
        None,
        "AV1 preset container",
    )
    .await?;
    checked(
        grav.clone(),
        vec![
            "apply".into(),
            os(&stub),
            "--output".into(),
            os(&grained),
            "--preset".into(),
            preset.into(),
            "--replace".into(),
        ],
        &cancel,
        Duration::from_secs(120),
        None,
        "Film-stock preset generation",
    )
    .await?;
    checked(
        grav,
        vec![
            "inspect".into(),
            os(&grained),
            "--output".into(),
            os(&table),
        ],
        &cancel,
        Duration::from_secs(120),
        None,
        "Film-stock table extraction",
    )
    .await?;
    let original = read_av1an_grain_table(table.to_string_lossy().into_owned())?;
    let mut lines = original.lines().map(str::to_owned).collect::<Vec<_>>();
    if let Some(last) = lines.iter_mut().rfind(|line| line.starts_with("E ")) {
        let mut fields = last
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let end = fields[2]
            .parse::<u64>()
            .map_err(|_| error("GRAIN_TABLE_INVALID", "Invalid table coverage.", None))?;
        fields[2] = end.max(864_000_000_000).to_string();
        *last = fields.join(" ");
    }
    let result = lines.join("\n") + "\n";
    validate_grain_table(result.as_bytes()).map_err(|e| error("GRAIN_TABLE_INVALID", e, None))?;
    Ok(result)
}

pub(crate) fn validate_grain_table(bytes: &[u8]) -> Result<u32, String> {
    if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 || bytes.contains(&0) {
        return Err("The grain table is empty, too large, or contains binary data.".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "The grain table is not UTF-8 text.")?;
    let mut count = 0_u32;
    let mut previous_end = None;
    for line in text.lines().map(str::trim) {
        if !line.starts_with("E ") {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 6 {
            return Err("A grain-table segment has too few fields.".into());
        }
        let start = fields[1]
            .parse::<u64>()
            .map_err(|_| "A grain-table segment has an invalid start timestamp.")?;
        let end = fields[2]
            .parse::<u64>()
            .map_err(|_| "A grain-table segment has an invalid end timestamp.")?;
        if end <= start || previous_end.is_some_and(|previous| start < previous) {
            return Err("Grain-table segments overlap or have invalid timing.".into());
        }
        previous_end = Some(end);
        count = count
            .checked_add(1)
            .ok_or("The grain table contains too many segments.")?;
    }
    if count == 0 {
        return Err("The file contains no AV1 film-grain table segments.".into());
    }
    Ok(count)
}

fn read_grain_table(path: &Path) -> Result<(Vec<u8>, u32), AppError> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|cause| {
            error(
                "UTILITY_GRAIN_TABLE_UNREADABLE",
                format!("The grain table could not be read: {cause}"),
                Some(path),
            )
        })?;
    let count = validate_grain_table(&bytes)
        .map_err(|detail| error("UTILITY_GRAIN_TABLE_INVALID", detail, Some(path)))?;
    Ok((bytes, count))
}

fn compatible_picture(left: &ProbeStream, right: &ProbeStream) -> bool {
    left.codec_name == right.codec_name
        && left.width == right.width
        && left.height == right.height
        && left.pix_fmt == right.pix_fmt
        && left.avg_frame_rate == right.avg_frame_rate
        && left.sample_aspect_ratio == right.sample_aspect_ratio
}

async fn inspect_grain_headers(
    grav1synth: &Path,
    input: &Path,
    scratch: &Scratch,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<u32>, AppError> {
    let table = scratch.path.join(format!(
        "inspect-{}.txt",
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let output = capture(
        grav1synth.to_owned(),
        vec![
            "inspect".into(),
            os(input),
            "--output".into(),
            os(&table),
            "--overwrite".into(),
        ],
        cancel,
        MEDIA_LIMIT,
        Some(input),
    )
    .await?;
    if !output.status.success() {
        return Err(error(
            "UTILITY_TOOL_FAILED",
            format!(
                "grav1synth could not inspect the AV1 headers: {} {}",
                diagnostic_tail(&output.stdout),
                diagnostic_tail(&output.stderr)
            ),
            Some(input),
        ));
    }
    if !table.exists() || fs::metadata(&table).map(|item| item.len()).unwrap_or(0) == 0 {
        return Ok(None);
    }
    read_grain_table(&table).map(|(_, count)| Some(count))
}

async fn publish_table(
    mut temporary: Temporary,
    output: &Path,
    sources: &[&Source],
) -> Result<u64, AppError> {
    for source in sources {
        source.verify()?;
    }
    temporary.flush_nonempty_async().await?;
    let bytes = fs::metadata(&temporary.path)
        .map_err(|cause| {
            error(
                "OUTPUT_UNREADABLE",
                cause.to_string(),
                Some(&temporary.path),
            )
        })?
        .len();
    temporary.publish(output)?;
    temporary.cleanup()?;
    Ok(bytes)
}

async fn run_grain(
    request: GrainRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    let grav1synth = tool("grav1synth", "AV1 film-grain utilities").await?;
    let ffprobe = tool("ffprobe", "AV1 film-grain validation").await?;
    match request {
        GrainRequest::Measure {
            source_path,
            denoised_path,
            output_table_path,
        } => {
            let source = Source::open(valid_absolute(&source_path)?)?;
            let denoised = Source::open(valid_absolute(&denoised_path)?)?;
            let output = destination(&output_table_path, &[&source, &denoised])?;
            let source_probe = probe(&ffprobe, &source.path, cancel, true).await?;
            let denoised_probe = probe(&ffprobe, &denoised.path, cancel, true).await?;
            let source_video = source_probe
                .streams
                .iter()
                .find(|stream| stream.codec_type.as_deref() == Some("video"))
                .ok_or_else(|| {
                    error(
                        "UTILITY_STREAM_INVALID",
                        "The source has no video stream.",
                        Some(&source.path),
                    )
                })?;
            let denoised_video = denoised_probe
                .streams
                .iter()
                .find(|stream| stream.codec_type.as_deref() == Some("video"))
                .ok_or_else(|| {
                    error(
                        "UTILITY_STREAM_INVALID",
                        "The denoised copy has no video stream.",
                        Some(&denoised.path),
                    )
                })?;
            let source_frames = json_u64(source_video.nb_read_frames.as_ref());
            let denoised_frames = json_u64(denoised_video.nb_read_frames.as_ref());
            if source_video.width != denoised_video.width
                || source_video.height != denoised_video.height
                || source_video.pix_fmt != denoised_video.pix_fmt
                || source_video.avg_frame_rate != denoised_video.avg_frame_rate
                || source_frames.is_none()
                || source_frames != denoised_frames
            {
                return Err(error(
                    "UTILITY_GRAIN_ALIGNMENT_MISMATCH",
                    "Grain measurement requires frame-aligned source and denoised videos with identical dimensions, pixel format, frame rate, and decoded frame count.",
                    Some(&denoised.path),
                ));
            }
            let temporary = Temporary::create(&output, &nonce("grain-measure"))?;
            let tool_output = checked(
                grav1synth,
                vec![
                    "diff".into(),
                    os(&source.path),
                    os(&denoised.path),
                    "--output".into(),
                    os(&temporary.path),
                    "--overwrite".into(),
                ],
                cancel,
                MEDIA_LIMIT,
                Some(&source.path),
                "Film-grain measurement",
            )
            .await?;
            let (_, segment_count) = read_grain_table(&temporary.path)?;
            check_cancel(cancel)?;
            let fingerprints = vec![fingerprint(&source)?, fingerprint(&denoised)?];
            publish_table(temporary, &output, &[&source, &denoised]).await?;
            Ok(UtilityResult::GrainTable(GrainTableResult {
                output_path: output.to_string_lossy().into_owned(),
                segment_count,
                source_fingerprints: fingerprints,
                message: format!(
                    "Measured {segment_count} AV1 film-grain table segments from frame-aligned source and denoised video."
                ),
                diagnostics: [
                    diagnostic_tail(&tool_output.stdout),
                    diagnostic_tail(&tool_output.stderr),
                ]
                .into_iter()
                .filter(|text| !text.is_empty())
                .collect(),
            }))
        }
        GrainRequest::Extract {
            input_path,
            output_table_path,
        } => {
            let source = Source::open(valid_absolute(&input_path)?)?;
            let output = destination(&output_table_path, &[&source])?;
            let document = probe(&ffprobe, &source.path, cancel, false).await?;
            let video = document
                .streams
                .iter()
                .find(|stream| stream.codec_type.as_deref() == Some("video"))
                .ok_or_else(|| {
                    error(
                        "UTILITY_STREAM_INVALID",
                        "The source has no video stream.",
                        Some(&source.path),
                    )
                })?;
            if video.codec_name.as_deref() != Some("av1") {
                return Err(error(
                    "UTILITY_GRAIN_CODEC_UNSUPPORTED",
                    "Film-grain headers can only be extracted from an AV1 video stream.",
                    Some(&source.path),
                ));
            }
            let temporary = Temporary::create(&output, &nonce("grain-extract"))?;
            let tool_output = checked(
                grav1synth,
                vec![
                    "inspect".into(),
                    os(&source.path),
                    "--output".into(),
                    os(&temporary.path),
                    "--overwrite".into(),
                ],
                cancel,
                MEDIA_LIMIT,
                Some(&source.path),
                "Film-grain extraction",
            )
            .await?;
            if fs::metadata(&temporary.path)
                .map(|metadata| metadata.len() == 0)
                .unwrap_or(true)
            {
                return Err(error(
                    "UTILITY_GRAIN_NOT_PRESENT",
                    format!(
                        "No extractable AV1 film-grain headers were found. {} {}",
                        diagnostic_tail(&tool_output.stdout),
                        diagnostic_tail(&tool_output.stderr)
                    ),
                    Some(&source.path),
                ));
            }
            let (_, segment_count) = read_grain_table(&temporary.path)?;
            let source_fingerprint = fingerprint(&source)?;
            check_cancel(cancel)?;
            publish_table(temporary, &output, &[&source]).await?;
            Ok(UtilityResult::GrainTable(GrainTableResult {
                output_path: output.to_string_lossy().into_owned(),
                segment_count,
                source_fingerprints: vec![source_fingerprint],
                message: format!("Extracted {segment_count} AV1 film-grain table segments."),
                diagnostics: [
                    diagnostic_tail(&tool_output.stdout),
                    diagnostic_tail(&tool_output.stderr),
                ]
                .into_iter()
                .filter(|text| !text.is_empty())
                .collect(),
            }))
        }
        GrainRequest::Apply {
            input_path,
            output_path,
            source,
        } => {
            run_grain_rewrite(
                &grav1synth,
                &ffprobe,
                input_path,
                output_path,
                source,
                false,
                cancel,
            )
            .await
        }
        GrainRequest::RewriteHeaders {
            input_path,
            output_path,
            source,
        } => {
            run_grain_rewrite(
                &grav1synth,
                &ffprobe,
                input_path,
                output_path,
                source,
                true,
                cancel,
            )
            .await
        }
        GrainRequest::Remove {
            input_path,
            output_path,
        } => {
            let ffmpeg = tool("ffmpeg", "AV1 film-grain decode validation").await?;
            let source = Source::open(valid_absolute(&input_path)?)?;
            let output = destination(&output_path, &[&source])?;
            require_mkv(&output, "Film-grain removal")?;
            let input_probe = probe(&ffprobe, &source.path, cancel, false).await?;
            let input_video = input_probe
                .streams
                .iter()
                .find(|stream| stream.codec_type.as_deref() == Some("video"))
                .ok_or_else(|| {
                    error(
                        "UTILITY_STREAM_INVALID",
                        "The source has no video stream.",
                        Some(&source.path),
                    )
                })?;
            if input_video.codec_name.as_deref() != Some("av1") {
                return Err(error(
                    "UTILITY_GRAIN_CODEC_UNSUPPORTED",
                    "Film-grain headers can only be removed from AV1 video.",
                    Some(&source.path),
                ));
            }
            let input_evidence =
                grain_media_evidence(&ffprobe, &source.path, &input_probe, cancel).await?;
            let scratch = Scratch::create("grain-verify")?;
            if inspect_grain_headers(&grav1synth, &source.path, &scratch, cancel)
                .await?
                .is_none()
            {
                return Err(error(
                    "UTILITY_GRAIN_NOT_PRESENT",
                    "The selected AV1 source has no film-grain headers to remove.",
                    Some(&source.path),
                ));
            }
            let temporary = Temporary::create(&output, &nonce("grain-remove"))?;
            let tool_output = checked(
                grav1synth.clone(),
                vec![
                    "remove".into(),
                    os(&source.path),
                    "--output".into(),
                    os(&temporary.path),
                    "--overwrite".into(),
                ],
                cancel,
                MEDIA_LIMIT,
                Some(&source.path),
                "Film-grain header removal",
            )
            .await?;
            let output_probe = probe(&ffprobe, &temporary.path, cancel, false).await?;
            let output_video = output_probe
                .streams
                .iter()
                .find(|stream| stream.codec_type.as_deref() == Some("video"))
                .ok_or_else(|| {
                    error(
                        "UTILITY_VALIDATION_FAILED",
                        "The output has no video stream.",
                        Some(&temporary.path),
                    )
                })?;
            if !compatible_picture(input_video, output_video)
                || inspect_grain_headers(&grav1synth, &temporary.path, &scratch, cancel)
                    .await?
                    .is_some()
            {
                return Err(error(
                    "UTILITY_VALIDATION_FAILED",
                    "The AV1 output geometry changed or grain headers remain after removal.",
                    Some(&temporary.path),
                ));
            }
            let tolerance = validate_grain_media(
                &ffmpeg,
                &ffprobe,
                &temporary.path,
                &input_probe,
                &output_probe,
                &input_evidence,
                cancel,
            )
            .await?;
            let source_fingerprint = fingerprint(&source)?;
            check_cancel(cancel)?;
            let bytes = publish_media(temporary, &output, &[&source]).await?;
            Ok(UtilityResult::Artifact(UtilityArtifact {
                operation: "grainRemove".into(),
                output_path: output.to_string_lossy().into_owned(),
                size_bytes: bytes.to_string(),
                duration_seconds: duration(&output_probe),
                source_fingerprints: vec![source_fingerprint],
                message: "Removed AV1 film-grain synthesis headers without re-encoding the video. Every stream retained its decoded coverage, metadata, and non-video packet payloads and timing.".into(),
                diagnostics: [diagnostic_tail(&tool_output.stdout), diagnostic_tail(&tool_output.stderr), format!("Full-stream timing validation tolerance: {tolerance:.6}s.")]
                    .into_iter()
                    .filter(|text| !text.is_empty())
                    .collect(),
            }))
        }
    }
}

async fn run_grain_rewrite(
    grav1synth: &Path,
    ffprobe: &Path,
    input_path: String,
    output_path: String,
    grain_source: GrainSource,
    replace: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    let ffmpeg = tool("ffmpeg", "AV1 film-grain decode validation").await?;
    let source = Source::open(valid_absolute(&input_path)?)?;
    let mut table_source = None;
    let mut source_args = Vec::<OsString>::new();
    let source_description;
    match grain_source {
        GrainSource::Table { table_path } => {
            let table = Source::open(valid_absolute(&table_path)?)?;
            read_grain_table(&table.path)?;
            source_args.extend(["--grain".into(), os(&table.path)]);
            source_description = "grain table";
            table_source = Some(table);
        }
        GrainSource::Preset { preset } => {
            if preset.is_empty()
                || preset.len() > 64
                || !preset
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(error(
                    "UTILITY_GRAIN_PRESET_INVALID",
                    "Choose a preset advertised by the installed grav1synth build.",
                    Some(&source.path),
                ));
            }
            let listed = checked(
                grav1synth.to_owned(),
                vec!["presets".into()],
                cancel,
                Duration::from_secs(10),
                None,
                "Film-grain preset discovery",
            )
            .await?;
            let presets = parse_grain_presets(&listed.stdout);
            let base = preset.split('-').next().unwrap_or_default();
            if !presets.iter().any(|item| item == base) {
                return Err(error(
                    "UTILITY_GRAIN_PRESET_INVALID",
                    format!("'{preset}' is not advertised by the installed grav1synth build."),
                    Some(&source.path),
                ));
            }
            source_args.extend(["--preset".into(), preset.into()]);
            source_description = "film-stock preset";
        }
        GrainSource::PhotonNoise { iso, chroma } => {
            if iso == 0 {
                return Err(error(
                    "UTILITY_GRAIN_ISO_INVALID",
                    "Photon-noise ISO must be between 1 and 4,294,967,295.",
                    Some(&source.path),
                ));
            }
            source_args.extend(["--iso".into(), iso.to_string().into()]);
            if chroma {
                source_args.push("--chroma".into());
            }
            source_description = "photon-noise model";
        }
    }
    let refs = if let Some(table) = table_source.as_ref() {
        vec![&source, table]
    } else {
        vec![&source]
    };
    let output = destination(&output_path, &refs)?;
    require_mkv(&output, "Film-grain application")?;
    let input_probe = probe(ffprobe, &source.path, cancel, false).await?;
    let input_video = input_probe
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| {
            error(
                "UTILITY_STREAM_INVALID",
                "The source has no video stream.",
                Some(&source.path),
            )
        })?;
    if input_video.codec_name.as_deref() != Some("av1") {
        return Err(error(
            "UTILITY_GRAIN_CODEC_UNSUPPORTED",
            "Film-grain headers can only be applied to AV1 video.",
            Some(&source.path),
        ));
    }
    let input_evidence = grain_media_evidence(ffprobe, &source.path, &input_probe, cancel).await?;
    let scratch = Scratch::create("grain-verify")?;
    let previous = inspect_grain_headers(grav1synth, &source.path, &scratch, cancel).await?;
    if previous.is_some() && !replace {
        return Err(error(
            "UTILITY_GRAIN_ALREADY_PRESENT",
            "The AV1 source already has film-grain headers. Use Rewrite Headers to replace them deliberately.",
            Some(&source.path),
        ));
    }
    let temporary = Temporary::create(&output, &nonce("grain-apply"))?;
    let mut args = vec![
        "apply".into(),
        os(&source.path),
        "--output".into(),
        os(&temporary.path),
    ];
    args.extend(source_args);
    if replace {
        args.push("--replace".into());
    }
    args.push("--overwrite".into());
    let tool_output = checked(
        grav1synth.to_owned(),
        args,
        cancel,
        MEDIA_LIMIT,
        Some(&source.path),
        "Film-grain header write",
    )
    .await?;
    let output_probe = probe(ffprobe, &temporary.path, cancel, false).await?;
    let output_video = output_probe
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| {
            error(
                "UTILITY_VALIDATION_FAILED",
                "The output has no video stream.",
                Some(&temporary.path),
            )
        })?;
    let segments = inspect_grain_headers(grav1synth, &temporary.path, &scratch, cancel)
        .await?
        .ok_or_else(|| {
            error(
                "UTILITY_VALIDATION_FAILED",
                "The AV1 output does not contain readable film-grain headers.",
                Some(&temporary.path),
            )
        })?;
    if !compatible_picture(input_video, output_video) {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "AV1 codec, geometry, pixel format, cadence, or sample aspect ratio changed during the header-only operation.",
            Some(&temporary.path),
        ));
    }
    let tolerance = validate_grain_media(
        &ffmpeg,
        ffprobe,
        &temporary.path,
        &input_probe,
        &output_probe,
        &input_evidence,
        cancel,
    )
    .await?;
    let fingerprints = refs
        .iter()
        .map(|source| fingerprint(source))
        .collect::<Result<Vec<_>, _>>()?;
    check_cancel(cancel)?;
    let bytes = publish_media(temporary, &output, &refs).await?;
    Ok(UtilityResult::Artifact(UtilityArtifact {
        operation: if replace {
            "grainRewriteHeaders"
        } else {
            "grainApply"
        }
        .into(),
        output_path: output.to_string_lossy().into_owned(),
        size_bytes: bytes.to_string(),
        duration_seconds: duration(&output_probe),
        source_fingerprints: fingerprints,
        message: format!(
            "{} AV1 film-grain headers from a {source_description}; {segments} table segments were read back from the result without re-encoding the video. Every stream retained its decoded coverage, metadata, and non-video packet payloads and timing.",
            if replace { "Replaced" } else { "Applied" }
        ),
        diagnostics: [
            diagnostic_tail(&tool_output.stdout),
            diagnostic_tail(&tool_output.stderr),
            format!("Full-stream timing validation tolerance: {tolerance:.6}s."),
        ]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect(),
    }))
}

fn plan_samples(source_seconds: f64, count: u8, section_seconds: f64) -> Vec<(f64, f64)> {
    if source_seconds <= section_seconds {
        return vec![(0.0, source_seconds)];
    }
    let mut count = usize::from(count.max(1));
    let skip = (source_seconds * 0.05).min(60.0);
    let mut span_start = skip;
    let mut span = source_seconds - 2.0 * skip;
    if span < count as f64 * section_seconds {
        span_start = 0.0;
        span = source_seconds;
    }
    count = count.min((span / section_seconds).floor().max(1.0) as usize);
    let slice = span / count as f64;
    (0..count)
        .map(|index| {
            let start = (span_start + (index as f64 + 0.5) * slice - section_seconds / 2.0)
                .clamp(0.0, source_seconds - section_seconds);
            (start, section_seconds)
        })
        .collect()
}

fn valid_preset(encoder: LadderEncoder, preset: &str) -> bool {
    if preset.is_empty()
        || preset.len() > 32
        || preset.starts_with('-')
        || !preset
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return false;
    }
    match encoder {
        LadderEncoder::Av1 => preset.parse::<u8>().is_ok_and(|value| value <= 13),
        LadderEncoder::Vp9 => preset.parse::<u8>().is_ok_and(|value| value <= 8),
        LadderEncoder::H264 | LadderEncoder::Hevc => matches!(
            preset,
            "ultrafast"
                | "superfast"
                | "veryfast"
                | "faster"
                | "fast"
                | "medium"
                | "slow"
                | "slower"
                | "veryslow"
                | "placebo"
        ),
    }
}

fn codec_name(encoder: LadderEncoder) -> &'static str {
    match encoder {
        LadderEncoder::H264 => "libx264",
        LadderEncoder::Hevc => "libx265",
        LadderEncoder::Av1 => "libsvtav1",
        LadderEncoder::Vp9 => "libvpx-vp9",
    }
}

fn quality_metric(metric: LadderMetric) -> Option<QualityMetric> {
    match metric {
        LadderMetric::None => None,
        LadderMetric::Psnr => Some(QualityMetric::Psnr),
        LadderMetric::Ssim => Some(QualityMetric::Ssim),
        LadderMetric::Vmaf => Some(QualityMetric::Vmaf),
    }
}

fn default_threshold(metric: LadderMetric) -> Option<f64> {
    match metric {
        LadderMetric::None => None,
        LadderMetric::Psnr => Some(45.0),
        LadderMetric::Ssim => Some(0.98),
        LadderMetric::Vmaf => Some(95.0),
    }
}

fn color_args(stream: &ProbeStream) -> Vec<OsString> {
    let mut args = Vec::new();
    for (flag, value) in [
        ("-color_range", clean_field(&stream.color_range)),
        ("-colorspace", clean_field(&stream.color_space)),
        ("-color_trc", clean_field(&stream.color_transfer)),
        ("-color_primaries", clean_field(&stream.color_primaries)),
        (
            "-chroma_sample_location",
            clean_field(&stream.chroma_location),
        ),
    ] {
        if let Some(value) = value {
            args.extend([flag.into(), value.into()]);
        }
    }
    args
}

fn scored_source_supported(stream: &ProbeStream) -> bool {
    matches!(
        clean_field(&stream.color_transfer),
        Some("bt709" | "bt470bg" | "smpte170m")
    ) && matches!(
        clean_field(&stream.color_space),
        Some("bt709" | "bt470bg" | "smpte170m")
    ) && matches!(
        clean_field(&stream.color_primaries),
        Some("bt709" | "bt470bg" | "smpte170m")
    ) && matches!(clean_field(&stream.color_range), Some("tv" | "pc"))
        && matches!(
            clean_field(&stream.chroma_location),
            Some("left" | "center" | "topleft")
        )
}

async fn encode_reference(
    ffmpeg: &Path,
    source: &Path,
    stream: &ProbeStream,
    window: (f64, f64),
    pixel_format: &str,
    output: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let (start, seconds) = window;
    let mut args = vec![
        "-hide_banner".into(),
        "-v".into(),
        "warning".into(),
        "-nostdin".into(),
        "-ss".into(),
        format!("{start:.9}").into(),
        "-i".into(),
        os(source),
        "-t".into(),
        format!("{seconds:.9}").into(),
        "-map".into(),
        format!("0:{}", stream.index).into(),
        "-an".into(),
        "-sn".into(),
        "-dn".into(),
        "-vf".into(),
        "setsar=1,setparams=field_mode=prog".into(),
        "-pix_fmt".into(),
        pixel_format.into(),
        "-c:v".into(),
        "ffv1".into(),
        "-level".into(),
        "3".into(),
    ];
    args.extend(color_args(stream));
    args.extend(["-y".into(), "-f".into(), "matroska".into(), os(output)]);
    checked(
        ffmpeg.to_owned(),
        args,
        cancel,
        MEDIA_LIMIT,
        Some(source),
        "Aligned lossless sample creation",
    )
    .await?;
    Ok(())
}

async fn encode_candidate(
    ffmpeg: &Path,
    reference: &Path,
    stream: &ProbeStream,
    request: &CrfLadderRequest,
    crf: u8,
    output: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<f64, AppError> {
    let started = Instant::now();
    let mut args = vec![
        "-hide_banner".into(),
        "-v".into(),
        "warning".into(),
        "-nostdin".into(),
        "-i".into(),
        os(reference),
        "-map".into(),
        "0:v:0".into(),
        "-an".into(),
        "-sn".into(),
        "-dn".into(),
        "-pix_fmt".into(),
        request.pixel_format.clone().into(),
        "-c:v".into(),
        codec_name(request.encoder).into(),
    ];
    match request.encoder {
        LadderEncoder::H264 | LadderEncoder::Hevc | LadderEncoder::Av1 => {
            args.extend([
                "-preset".into(),
                request.preset.clone().into(),
                "-crf".into(),
                crf.to_string().into(),
            ]);
        }
        LadderEncoder::Vp9 => {
            args.extend([
                "-deadline".into(),
                "good".into(),
                "-cpu-used".into(),
                request.preset.clone().into(),
                "-crf".into(),
                crf.to_string().into(),
                "-b:v".into(),
                "0".into(),
            ]);
        }
    }
    args.extend(color_args(stream));
    args.extend(["-y".into(), "-f".into(), "matroska".into(), os(output)]);
    checked(
        ffmpeg.to_owned(),
        args,
        cancel,
        MEDIA_LIMIT,
        Some(reference),
        &format!("CRF {crf} sample encode"),
    )
    .await?;
    Ok(started.elapsed().as_secs_f64())
}

fn aggregate_score(metric: LadderMetric, scores: &[(Option<f64>, u64)]) -> Option<f64> {
    if metric == LadderMetric::None || scores.is_empty() {
        return None;
    }
    let weight = scores.iter().map(|(_, frames)| *frames as f64).sum::<f64>();
    if weight <= 0.0 {
        return None;
    }
    if metric == LadderMetric::Psnr {
        let mse = scores
            .iter()
            .map(|(score, frames)| {
                score.map(|score| 10_f64.powf(-score / 10.0)).unwrap_or(0.0) * *frames as f64
            })
            .sum::<f64>()
            / weight;
        return (mse > 0.0).then(|| -10.0 * mse.log10());
    }
    Some(
        scores
            .iter()
            .filter_map(|(score, frames)| score.map(|score| score * *frames as f64))
            .sum::<f64>()
            / weight,
    )
}

async fn run_crf_ladder(
    request: CrfLadderRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    if !valid_preset(request.encoder, &request.preset)
        || !matches!(request.pixel_format.as_str(), "yuv420p" | "yuv420p10le")
        || request.crfs.is_empty()
        || request.crfs.len() > 8
        || !(1..=12).contains(&request.sample_count)
        || !request.sample_seconds.is_finite()
        || !(1.0..=120.0).contains(&request.sample_seconds)
    {
        return Err(error(
            "UTILITY_LADDER_SETTINGS_INVALID",
            "Choose a supported preset, yuv420p/yuv420p10le, 1-8 CRFs, 1-12 samples, and a 1-120 second sample length.",
            Some(Path::new(&request.input_path)),
        ));
    }
    let max_crf = match request.encoder {
        LadderEncoder::H264 | LadderEncoder::Hevc => 51,
        LadderEncoder::Av1 | LadderEncoder::Vp9 => 63,
    };
    let mut crfs = request.crfs.clone();
    crfs.sort_unstable();
    crfs.dedup();
    if crfs.len() != request.crfs.len() || crfs.iter().any(|value| *value == 0 || *value > max_crf)
    {
        return Err(error(
            "UTILITY_LADDER_SETTINGS_INVALID",
            format!("CRFs must be distinct and between 1 and {max_crf} for this encoder."),
            Some(Path::new(&request.input_path)),
        ));
    }
    let threshold = request
        .recommendation_threshold
        .or_else(|| default_threshold(request.metric));
    if threshold.is_some_and(|value| {
        !value.is_finite()
            || match request.metric {
                LadderMetric::None => true,
                LadderMetric::Psnr => !(0.0..=100.0).contains(&value),
                LadderMetric::Ssim => !(0.0..=1.0).contains(&value),
                LadderMetric::Vmaf => !(0.0..=100.0).contains(&value),
            }
    }) {
        return Err(error(
            "UTILITY_LADDER_THRESHOLD_INVALID",
            "The recommendation threshold is outside the selected metric's range.",
            Some(Path::new(&request.input_path)),
        ));
    }
    let source = Source::open(valid_absolute(&request.input_path)?)?;
    let source_size = fs::metadata(&source.path)
        .map_err(|cause| error("FILE_UNREADABLE", cause.to_string(), Some(&source.path)))?
        .len();
    let source_fingerprint = fingerprint(&source)?;
    let ffprobe = tool("ffprobe", "CRF ladder source and sample validation").await?;
    let ffmpeg = tool("ffmpeg", "CRF ladder sample encoding").await?;
    let encoder_output = checked(
        ffmpeg.clone(),
        vec!["-hide_banner".into(), "-encoders".into()],
        cancel,
        Duration::from_secs(15),
        None,
        "FFmpeg encoder capability inspection",
    )
    .await?;
    if !String::from_utf8_lossy(&encoder_output.stdout).contains(codec_name(request.encoder)) {
        return Err(error(
            "UTILITY_ENCODER_UNAVAILABLE",
            format!(
                "This FFmpeg build does not advertise the {} wrapper required by the selected ladder encoder.",
                codec_name(request.encoder)
            ),
            Some(&source.path),
        ));
    }
    let source_probe = probe(&ffprobe, &source.path, cancel, false).await?;
    let stream = selected_stream(
        &source_probe,
        request.video_stream_index,
        "video",
        &source.path,
    )?
    .clone();
    if !matches!(
        stream.field_order.as_deref(),
        None | Some("progressive" | "unknown")
    ) || !matches!(stream.sample_aspect_ratio.as_deref(), None | Some("1:1"))
    {
        return Err(error(
            "UTILITY_LADDER_SOURCE_UNSUPPORTED",
            "CRF ladder samples currently require progressive square-pixel video. Prepare an explicit reference for interlaced or anamorphic sources.",
            Some(&source.path),
        ));
    }
    if request.metric != LadderMetric::None && !scored_source_supported(&stream) {
        return Err(error(
            "UTILITY_LADDER_SOURCE_UNSUPPORTED",
            "Scored ladders require explicitly tagged SDR video with supported range, matrix, primaries, transfer, and chroma location. Use size-only mode or prepare a matched SDR reference.",
            Some(&source.path),
        ));
    }
    let source_duration = duration(&source_probe)
        .filter(|value| *value > 0.0)
        .ok_or_else(|| {
            error(
                "UTILITY_LADDER_SOURCE_UNSUPPORTED",
                "The source has no reliable positive duration.",
                Some(&source.path),
            )
        })?;
    let samples = plan_samples(
        source_duration,
        request.sample_count,
        request.sample_seconds,
    );
    let scratch = Scratch::create("crf-ladder")?;
    struct Sample {
        reference: PathBuf,
        seconds: f64,
        frames: u64,
    }
    let mut prepared = Vec::with_capacity(samples.len());
    for (index, (start, length)) in samples.iter().copied().enumerate() {
        check_cancel(cancel)?;
        let path = scratch.path.join(format!("reference-{index:03}.mkv"));
        encode_reference(
            &ffmpeg,
            &source.path,
            &stream,
            (start, length),
            &request.pixel_format,
            &path,
            cancel,
        )
        .await?;
        let reference_probe = probe(&ffprobe, &path, cancel, true).await?;
        let reference_stream = reference_probe
            .streams
            .iter()
            .find(|item| item.codec_type.as_deref() == Some("video"))
            .ok_or_else(|| {
                error(
                    "UTILITY_VALIDATION_FAILED",
                    "A lossless sample has no video stream.",
                    Some(&path),
                )
            })?;
        let seconds = duration(&reference_probe)
            .filter(|value| *value > 0.0)
            .ok_or_else(|| {
                error(
                    "UTILITY_VALIDATION_FAILED",
                    "A lossless sample has no measurable duration.",
                    Some(&path),
                )
            })?;
        let frames = json_u64(reference_stream.nb_read_frames.as_ref())
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                error(
                    "UTILITY_VALIDATION_FAILED",
                    "A lossless sample has no decodable frames.",
                    Some(&path),
                )
            })?;
        prepared.push(Sample {
            reference: path,
            seconds,
            frames,
        });
    }
    let mut rungs = Vec::with_capacity(crfs.len());
    for crf in crfs {
        let mut bytes = 0_u64;
        let mut seconds = 0.0;
        let mut encode_seconds = 0.0;
        let mut scores = Vec::new();
        for (index, sample) in prepared.iter().enumerate() {
            check_cancel(cancel)?;
            let candidate = scratch
                .path
                .join(format!("candidate-{crf:02}-{index:03}.mkv"));
            encode_seconds += encode_candidate(
                &ffmpeg,
                &sample.reference,
                &stream,
                &request,
                crf,
                &candidate,
                cancel,
            )
            .await?;
            let candidate_probe = probe(&ffprobe, &candidate, cancel, true).await?;
            let candidate_stream = candidate_probe
                .streams
                .iter()
                .find(|item| item.codec_type.as_deref() == Some("video"))
                .ok_or_else(|| {
                    error(
                        "UTILITY_VALIDATION_FAILED",
                        "An encoded sample has no video stream.",
                        Some(&candidate),
                    )
                })?;
            let candidate_frames = json_u64(candidate_stream.nb_read_frames.as_ref());
            if candidate_frames != Some(sample.frames) {
                return Err(error(
                    "UTILITY_VALIDATION_FAILED",
                    format!(
                        "CRF {crf} sample {} decoded {} frames; the aligned reference has {}.",
                        index + 1,
                        candidate_frames.unwrap_or(0),
                        sample.frames
                    ),
                    Some(&candidate),
                ));
            }
            let candidate_duration = duration(&candidate_probe).unwrap_or(0.0);
            if (candidate_duration - sample.seconds).abs() > 0.01 + sample.seconds * 0.001 {
                return Err(error(
                    "UTILITY_VALIDATION_FAILED",
                    "An encoded sample's duration differs from its aligned reference.",
                    Some(&candidate),
                ));
            }
            bytes = bytes.saturating_add(
                fs::metadata(&candidate)
                    .map_err(|cause| {
                        error("OUTPUT_UNREADABLE", cause.to_string(), Some(&candidate))
                    })?
                    .len(),
            );
            seconds += sample.seconds;
            if let Some(metric) = quality_metric(request.metric) {
                let frame_count = u32::try_from(sample.frames).map_err(|_| {
                    error(
                        "UTILITY_LADDER_SAMPLE_TOO_LONG",
                        "A ladder sample exceeds the 60,000-frame quality-analysis bound.",
                        Some(&sample.reference),
                    )
                })?;
                let quality = crate::quality::analyze_quality(
                    QualityRequest {
                        reference_path: sample.reference.to_string_lossy().into_owned(),
                        reference_stream_index: 0,
                        reference_start_frame: 0,
                        candidate_path: candidate.to_string_lossy().into_owned(),
                        candidate_stream_index: 0,
                        candidate_start_frame: 0,
                        frame_count,
                        metric,
                    },
                    cancel.clone(),
                )
                .await?;
                scores.push((quality.score, sample.frames));
            }
        }
        let score = aggregate_score(request.metric, &scores);
        let bitrate_kbps = if seconds > 0.0 {
            bytes as f64 * 8.0 / seconds / 1000.0
        } else {
            0.0
        };
        let bytes_per_minute = if seconds > 0.0 {
            (bytes as f64 * 60.0 / seconds).round() as u64
        } else {
            0
        };
        let projected = if seconds > 0.0 {
            (bytes as f64 * source_duration / seconds).round() as u64
        } else {
            0
        };
        rungs.push(CrfLadderRung {
            crf,
            encoded_bytes: bytes.to_string(),
            encoded_seconds: seconds,
            bitrate_kbps,
            bytes_per_minute: bytes_per_minute.to_string(),
            projected_size_bytes: projected.to_string(),
            score,
            encode_seconds,
        });
    }
    source.verify()?;
    if fingerprint(&source)? != source_fingerprint {
        return Err(error(
            "SOURCE_CHANGED",
            "The source changed during CRF ladder analysis. Run it again on a stable source.",
            Some(&source.path),
        ));
    }
    check_cancel(cancel)?;
    let recommended_crf = threshold.and_then(|threshold| {
        rungs
            .iter()
            .filter(|rung| match (request.metric, rung.score) {
                (LadderMetric::Psnr, None) => true,
                (_, Some(score)) => score >= threshold,
                _ => false,
            })
            .map(|rung| rung.crf)
            .max()
    });
    let sampled_seconds = prepared.iter().map(|sample| sample.seconds).sum::<f64>();
    Ok(UtilityResult::CrfLadder(CrfLadderResult {
        encoder: request.encoder,
        preset: request.preset,
        metric: request.metric,
        source_duration_seconds: source_duration,
        source_size_bytes: source_size.to_string(),
        sampled_seconds,
        sampled_fraction: (sampled_seconds / source_duration).min(1.0),
        rungs,
        recommended_crf,
        source_fingerprint,
        message: "Each rung encoded the same lossless, frame-counted samples. Size and bitrate are measured video-only; whole-file size is a projection and can move with unsampled scene complexity.".into(),
        diagnostics: vec![
            "Only the selected FFmpeg wrapper preset, CRF, and pixel format are represented. Audio, subtitles, attachments, chapters, production encode filters, standalone-encoder-only settings, and container overhead do not transfer into this ladder.".into(),
            match threshold {
                Some(value) => format!("The recommendation is the highest tested CRF meeting the selected metric threshold ({value})."),
                None => "No score threshold was selected, so no CRF recommendation was made.".into(),
            },
        ],
    }))
}

pub async fn run_utility(
    request: UtilityRequest,
    cancel: watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    check_cancel(&cancel)?;
    match request {
        UtilityRequest::KeyframeCut(request) => run_keyframe_cut(request, &cancel).await,
        UtilityRequest::Concat(request) => run_concat(request, &cancel).await,
        UtilityRequest::ColorMetadataTransfer(request) => {
            run_color_transfer(request, &cancel).await
        }
        UtilityRequest::SubtitleOcr(request) => run_subtitle_ocr(request, &cancel).await,
        UtilityRequest::Grain(request) => run_grain(request, &cancel).await,
        UtilityRequest::CrfLadder(request) => run_crf_ladder(request, &cancel).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn film_stock_modifiers_expand_only_advertised_applicable_bases() {
        let advertised = b"Available Presets:\n  Super8  (Super 8mm)\n  MaxMid  (Synthetic)\n  16mm  (16mm)\n  Classic35  (35mm)\n  Modern35  (Full Frame)\nExample: use 16mm\nAvailable film stock modifiers (applies to 16mm, Classic35, Modern35):\n  -1  Fujifilm Eterna 500T\n  -2  Kodak Vision3 250D\n  -3  Kodak Vision3 200T\nExample: use 16mm-3\n";
        let presets = parse_grain_presets(advertised);
        assert_eq!(presets.len(), 14);
        for base in ["16mm", "Classic35", "Modern35"] {
            for suffix in 1..=3 {
                assert!(presets.contains(&format!("{base}-{suffix}")));
            }
        }
        assert!(!presets.contains(&"Super8-3".to_owned()));
        assert!(!presets.contains(&"MaxMid-3".to_owned()));
        assert!(!presets.contains(&"16mm-4".to_owned()));
        assert_eq!(
            parse_grain_presets(b"Available Presets:\n  16mm  (16mm)\nExample: use 16mm\n"),
            vec!["16mm"]
        );
    }

    #[test]
    fn sample_plan_is_centered_bounded_and_non_overlapping() {
        let samples = plan_samples(7_200.0, 5, 10.0);
        assert_eq!(samples.len(), 5);
        assert!(samples[0].0 >= 60.0);
        assert!(samples.last().unwrap().0 + 10.0 <= 7_140.0);
        assert!(
            samples
                .windows(2)
                .all(|pair| pair[0].0 + pair[0].1 <= pair[1].0)
        );
        assert_eq!(plan_samples(4.0, 5, 10.0), vec![(0.0, 4.0)]);
    }

    #[test]
    fn grain_table_validation_rejects_overlap_and_binary_data() {
        assert_eq!(
            validate_grain_table(b"filmgrn1\nE 0 100 1 2 1\nE 100 200 1 3 1\n").unwrap(),
            2
        );
        assert!(validate_grain_table(b"E 0 100 1 2 1\nE 99 200 1 3 1\n").is_err());
        assert!(validate_grain_table(b"E 0 100 1\0 1\n").is_err());
    }

    #[tokio::test]
    #[ignore = "requires grav1synth presets, FFmpeg and standalone SVT-AV1"]
    async fn av1an_film_stock_preset_produces_immutable_table_bytes() {
        let (_sender, cancel) = watch::channel(false);
        let table = make_av1an_grain_preset("16mm".into(), cancel)
            .await
            .unwrap();
        assert!(table.starts_with("filmgrn1\n"));
        assert!(table.len() <= 262144);
        assert!(validate_grain_table(table.as_bytes()).unwrap() > 0);
        let end = table
            .lines()
            .rfind(|line| line.starts_with("E "))
            .unwrap()
            .split_whitespace()
            .nth(2)
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(end >= 864_000_000_000);
        let (_sender, cancel) = watch::channel(true);
        assert!(
            make_av1an_grain_preset("16mm".into(), cancel)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "requires grav1synth stock modifiers, FFmpeg and standalone SVT-AV1"]
    async fn av1an_modified_film_stock_preset_generates_valid_table() {
        let (_sender, cancel) = watch::channel(false);
        let table = make_av1an_grain_preset("16mm-3".into(), cancel)
            .await
            .unwrap();
        assert!(table.starts_with("filmgrn1\n"));
        assert!(table.len() <= 262144);
        assert!(validate_grain_table(table.as_bytes()).unwrap() > 0);
    }

    #[test]
    fn srt_validation_requires_ordered_timed_text() {
        let valid = b"1\r\n00:00:01,000 --> 00:00:02,000\r\nHello\r\n\r\n2\r\n00:00:03,000 --> 00:00:04,000\r\nWorld\r\n";
        assert_eq!(validate_srt(valid).unwrap(), 2);
        assert!(validate_srt(b"1\n00:00:02,000 --> 00:00:01,000\nNo\n").is_err());
        assert!(validate_srt(b"1\n00:00:01,000 --> 00:00:02,000\n").is_err());
    }

    #[test]
    fn psnr_pooling_uses_error_energy_and_infinite_samples() {
        let score = aggregate_score(LadderMetric::Psnr, &[(Some(40.0), 10), (None, 10)]).unwrap();
        assert!((score - 43.0103).abs() < 0.001);
        assert_eq!(aggregate_score(LadderMetric::Psnr, &[(None, 10)]), None);
    }
}
