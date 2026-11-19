//! Lossless canonical sources for display transforms and authored variable clocks.
//! Original requests remain immutable. An owned receipt binds both complete files
//! so native/av1an recovery can safely reuse a stable source across attempts.
use super::*;
#[path = "source_aac.rs"]
pub(super) mod aac;
#[path = "source_av1an_trim.rs"]
pub(super) mod av1an_trim;
#[path = "source_retime.rs"]
mod retime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
};

const LIMIT: u64 = 16 * 1024 * 1024;
const TIME: Duration = Duration::from_secs(24 * 60 * 60);
fn invalid(path: &Path, message: impl Into<String>) -> AppError {
    files::error("SOURCE_NORMALIZATION_INVALID", message, path)
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Identity(Vec<u64>);
fn file_identity(file: &File) -> Result<Identity, AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file
            .metadata()
            .map_err(|e| AppError::new("FILE_UNREADABLE", e.to_string(), None))?;
        Ok(Identity(vec![m.dev(), m.ino()]))
    }
    #[cfg(windows)]
    {
        let (volume, high, low) = files::windows_file_id(file)
            .map_err(|e| AppError::new("FILE_UNREADABLE", e.to_string(), None))?;
        Ok(Identity(vec![volume as u64, high as u64, low as u64]))
    }
}
fn identity(path: &Path) -> Result<Identity, AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| invalid(path, e.to_string()))?;
    if metadata.file_type().is_symlink() {
        return Err(invalid(
            path,
            "Canonical source ownership cannot follow links.",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid(
                path,
                "Canonical source ownership cannot follow reparse points.",
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(0x02000000)
            .open(path)
            .map_err(|e| invalid(path, e.to_string()))?;
        file_identity(&file)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Identity(vec![metadata.dev(), metadata.ino()]))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    path: PathBuf,
    identity: Identity,
    length: u64,
    sha256: String,
}
fn digest(source: &Source, cancel: &watch::Receiver<bool>) -> Result<Stamp, AppError> {
    source.verify()?;
    let mut file = File::open(&source.path).map_err(|e| invalid(&source.path, e.to_string()))?;
    let identity = file_identity(&file)?;
    let mut hash = Sha256::new();
    let mut bytes = [0_u8; 65536];
    let mut length = 0;
    loop {
        check_cancel(cancel)?;
        let count = file
            .read(&mut bytes)
            .map_err(|e| invalid(&source.path, e.to_string()))?;
        if count == 0 {
            break;
        }
        length += count as u64;
        hash.update(&bytes[..count]);
    }
    source.verify()?;
    Ok(Stamp {
        path: source.path.clone(),
        identity,
        length,
        sha256: format!("{:x}", hash.finalize()),
    })
}
// Match AV1AN's ordinary recovery policy. Full-byte receipts remain valid
// and are checked byte-for-byte when reopening older or thorough jobs.
fn metadata_stamp(source: &Source, cancel: &watch::Receiver<bool>) -> Result<Stamp, AppError> {
    check_cancel(cancel)?;
    source.verify()?;
    let metadata = source
        .file()
        .metadata()
        .map_err(|error| invalid(&source.path, error.to_string()))?;
    let modified = metadata
        .modified()
        .map_err(|error| invalid(&source.path, error.to_string()))?
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| invalid(&source.path, error.to_string()))?;
    let result = Stamp {
        path: source.path.clone(),
        identity: file_identity(source.file())?,
        length: metadata.len(),
        sha256: format!("metadata-v1:{}", modified.as_nanos()),
    };
    source.verify()?;
    Ok(result)
}

impl Stamp {
    fn includes_contents(&self) -> bool {
        !self.sha256.starts_with("metadata-v1:")
    }
}

async fn recovery_stamp(
    source: &Source,
    cancel: &watch::Receiver<bool>,
    include_contents: bool,
) -> Result<Stamp, AppError> {
    if include_contents {
        stamp(source, cancel).await
    } else {
        metadata_stamp(source, cancel)
    }
}

async fn stamp(source: &Source, cancel: &watch::Receiver<bool>) -> Result<Stamp, AppError> {
    source.verify()?;
    let path = source.path.clone();
    let cancel = cancel.clone();
    let result = tokio::task::spawn_blocking(move || {
        let source = Source::open(&path)?;
        digest(&source, &cancel)
    })
    .await
    .map_err(|error| AppError::new("SOURCE_NORMALIZATION_INVALID", error.to_string(), None))??;
    source.verify()?;
    Ok(result)
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Recipe {
    origin: f64,
    rotation: i32,
    numerator: u32,
    denominator: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trim_end_seconds: Option<f64>,
}
impl Recipe {
    fn filter(&self) -> String {
        format!(
            "fps=fps={}/{}:start_time=0:round=near:eof_action=pass",
            self.numerator, self.denominator
        )
    }
}
fn number(text: Option<&str>) -> Option<f64> {
    text?.parse::<f64>().ok().filter(|v| v.is_finite())
}
fn clock(text: &str) -> Option<f64> {
    let mut parts = text.split(':');
    let hours = parts.next()?.parse::<f64>().ok()?;
    let minutes = parts.next()?.parse::<f64>().ok()?;
    let seconds = parts.next()?.parse::<f64>().ok()?;
    (parts.next().is_none() && hours >= 0.0 && minutes >= 0.0 && seconds >= 0.0)
        .then_some(hours * 3600.0 + minutes * 60.0 + seconds)
}
fn stream_end(stream: &metadata::Stream) -> Option<f64> {
    // FFprobe stream.duration is a span. Matroska DURATION tags are absolute
    // track end timestamps; treating these as interchangeable truncates TS.
    number(stream.duration.as_deref())
        .map(|span| number(stream.start_time.as_deref()).unwrap_or(0.0) + span)
        .or_else(|| {
            stream
                .tags
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("DURATION"))
                .and_then(|(_, value)| clock(value))
        })
}
fn normalized_end(document: &Document, origin: f64) -> Option<f64> {
    let track_end = document
        .streams
        .iter()
        .filter_map(stream_end)
        .reduce(f64::max);
    track_end
        .or_else(|| {
            document.format.as_ref().and_then(|format| {
                let duration = number(format.duration.as_deref())?;
                // Matroska Segment Duration is relative to its segment timestamp zero;
                // FFprobe exposes it directly, including a positive authored origin.
                let matroska = format.format_name.as_deref().is_some_and(|name| {
                    name.split(',')
                        .any(|name| matches!(name, "matroska" | "webm"))
                });
                Some(
                    duration
                        + if matroska {
                            0.0
                        } else {
                            number(format.start_time.as_deref()).unwrap_or(0.0)
                        },
                )
            })
        })
        .map(|end| (end - origin).max(0.0))
        .filter(|end| end.is_finite() && *end > 0.0)
}
fn rotation(video: &metadata::Stream) -> Result<i32, AppError> {
    let mut angle = video
        .side_data_list
        .iter()
        .find_map(|value| value.get("rotation").and_then(serde_json::Value::as_f64))
        .or_else(|| {
            video
                .tags
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("rotate"))
                .and_then(|(_, value)| value.parse().ok())
        })
        .unwrap_or(0.0);
    if !angle.is_finite() {
        return Err(AppError::new(
            "ENCODE_INPUT_UNSUPPORTED",
            "The display rotation is invalid.",
            None,
        ));
    }
    angle = ((angle % 360.0) + 360.0) % 360.0;
    let quarter = (angle / 90.0).round() * 90.0;
    if (angle - quarter).abs() > 0.001 {
        return Err(AppError::new(
            "ENCODE_INPUT_UNSUPPORTED",
            "Canonical source preparation supports right-angle display rotations. Arbitrary projective transforms require an explicit rendering workflow.",
            None,
        ));
    }
    Ok(quarter as i32 % 360)
}
/// The right-angle rotation Quick Convert applies while it encodes, when this
/// source can skip the canonical copy (0 for a later start alone). Reflected
/// or otherwise unusual display matrices keep the canonical copy.
pub(super) fn direct_rotation(video: &metadata::Stream, settings: &EncodeSettings) -> Option<i32> {
    if !super::encode_plan::direct_source_clock(video, settings) {
        return None;
    }
    let rotation = rotation(video).ok()?;
    let (_, transform) =
        crate::analysis::display_transform_parts(&video.side_data_list, &video.tags).ok()?;
    let expected = match rotation {
        90 => "transpose=cclock,",
        180 => "hflip,vflip,",
        270 => "transpose=clock,",
        _ => "",
    };
    (transform == expected).then_some(rotation)
}
fn recipe(
    document: &Document,
    settings: &EncodeSettings,
    force: bool,
) -> Result<Option<Recipe>, AppError> {
    let video = document
        .streams
        .iter()
        .find(|stream| {
            stream.index == settings.video_stream_index
                && stream.codec_type.as_deref() == Some("video")
        })
        .ok_or_else(|| {
            AppError::new(
                "STREAM_SELECTION_INVALID",
                "Select the source video for encoding.",
                None,
            )
        })?;
    let rotation = rotation(video)?;
    let origin = number(video.start_time.as_deref()).unwrap_or(0.0);
    if !force && rotation == 0 && origin.abs() <= 0.001 {
        return Ok(None);
    }
    if !force && direct_rotation(video, settings).is_some() {
        // The encode rotates in its decoder and restores the origin at the
        // mux. A variable clock is still caught and retried with `force`.
        return Ok(None);
    }
    if rotation == 0 && super::encode_plan::source_av1an_base_clock(video, settings) {
        // The source AV1AN reader consumes original decoded frame ordinals.
        // Compatible CPU base-layer tone mapping runs inside each chunk; an
        // FFV1/CFR pre-render would change its input frames and copied clocks.
        return Ok(None);
    }
    if ((!force && rotation == 0) || settings.tone_map.is_some_and(|tone| tone.source_workflow))
        && super::encode_plan::preserves_dolby_source_clock(video, settings)
    {
        // Keep the compressed single-layer RPU attached to its exact frames.
        // The actual source validator proves CFR relative to this origin; the
        // encoder pipe starts its new output clock at zero without reordering.
        return Ok(None);
    }
    if settings.hdr_metadata.as_ref().is_some_and(|metadata| {
        metadata.dolby_vision_rpu_path.is_some() || metadata.hdr10_plus_json_path.is_some()
    }) {
        return Err(AppError::new(
            "ENCODE_INPUT_UNSUPPORTED",
            "External per-frame HDR imports require the unchanged source clock and orientation. Render/normalize the source first, then author metadata for those exact frames.",
            None,
        ));
    }
    if video.side_data_list.iter().any(|value| {
        value
            .get("side_data_type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| {
                kind.contains("DOVI")
                    || kind.contains("Dolby")
                    || kind.contains("Dynamic")
                    || kind.contains("HDR10+")
            })
    }) {
        return Err(AppError::new(
            "ENCODE_INPUT_UNSUPPORTED",
            "Display/timestamp canonicalization cannot transform per-frame Dolby Vision or HDR10+ rendering metadata. Use the explicit HDR10 fallback/rendering route before changing its frame correspondence.",
            None,
        ));
    }
    if settings.temporal.is_some_and(|value| {
        value.deinterlace.is_some() || value.qtgmc.is_some() || value.cadence_repair.is_some()
    }) {
        return Err(AppError::new(
            "ENCODE_INPUT_UNSUPPORTED",
            "Canonical VFR/rotation preparation currently requires progressive input. Reconstruct interlaced or telecined footage with the export utility before normalizing its clock.",
            None,
        ));
    }
    // A demuxer's nominal rate can describe the VFR timestamp grid. At that
    // rate, normalizing a 60 fps recording with one short interval can create
    // four times as many pictures. Prefer the presentation average.
    let rate = [&video.avg_frame_rate, &video.r_frame_rate]
        .into_iter()
        .filter_map(|value| value.as_deref()?.split_once('/'))
        .filter_map(|(n, d)| Some((n.parse::<u32>().ok()?, d.parse::<u32>().ok()?)))
        .find(|&(n, d)| d > 0 && (0.001..=1000.0).contains(&(f64::from(n) / f64::from(d))))
        .ok_or_else(|| {
            AppError::new(
                "ENCODE_INPUT_UNSUPPORTED",
                "The source has no bounded rational clock for CFR canonicalization.",
                None,
            )
        })?;
    let trim_end_seconds = (!super::encode::thorough_verification(settings))
        .then_some(settings.trim)
        .flatten()
        .and_then(|trim| {
            let end = if let Some(time) = trim.time {
                f64::from(time.end_milliseconds) / 1000.0
            } else if trim.end_frame_exclusive > 0 {
                f64::from(trim.end_frame_exclusive) * f64::from(rate.1) / f64::from(rate.0)
            } else {
                return None;
            };
            let bound = (end + 2.0).max(1.0);
            let total = document
                .duration()
                .into_iter()
                .chain(normalized_end(document, origin))
                .reduce(f64::max)?;
            (total.is_finite() && bound + 5.0 < total).then_some(bound)
        });
    Ok(Some(Recipe {
        origin,
        rotation,
        numerator: rate.0,
        denominator: rate.1,
        trim_end_seconds,
    }))
}
fn root_for(id: &str, request: &RemuxRequest) -> Result<PathBuf, AppError> {
    if id.is_empty() || !id.bytes().all(|v| v.is_ascii_alphanumeric() || v == b'-') {
        return Err(AppError::new(
            "SOURCE_NORMALIZATION_INVALID",
            "Invalid job identifier.",
            None,
        ));
    }
    let parent = Path::new(&request.output_path)
        .parent()
        .ok_or_else(|| AppError::new("INVALID_OUTPUT", "Choose an output folder.", None))?;
    Ok(parent
        .canonicalize()
        .map_err(|e| invalid(parent, e.to_string()))?
        .join(format!(".jesses-{id}.source")))
}
#[derive(Debug, Serialize, Deserialize)]
struct Owner {
    version: u8,
    id: String,
    request: RemuxRequest,
    settings: EncodeSettings,
    directory: Identity,
    files: BTreeMap<String, Identity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original: Option<Stamp>,
    recipe: Recipe,
}
#[derive(Debug, Serialize, Deserialize)]
struct Complete {
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<Stamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decoded_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decoded_bytes: Option<u64>,
}
struct Workspace {
    root: PathBuf,
    owner: Owner,
    lock: Option<files::WorkspaceLock>,
    guard: Option<rate_control::Stats>,
}
fn create_file(path: &Path) -> Result<File, AppError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    options.open(path).map_err(|e| invalid(path, e.to_string()))
}
fn lock_file(path: &Path, existing: bool) -> Result<files::WorkspaceLock, AppError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(!existing);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let file = options.open(path).map_err(|e| {
        invalid(
            path,
            format!("The canonical source is in use or cannot be locked: {e}"),
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(invalid(path, "The canonical source is already in use."));
        }
    }
    Ok(files::WorkspaceLock(file))
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| invalid(path, e.to_string()))?;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return Err(invalid(
            path,
            "Canonical source receipt is not a bounded regular file.",
        ));
    }
    serde_json::from_slice(&fs::read(path).map_err(|e| invalid(path, e.to_string()))?)
        .map_err(|e| invalid(path, format!("Canonical source receipt is unreadable: {e}")))
}
impl Workspace {
    fn open(id: &str, request: &RemuxRequest, settings: &EncodeSettings) -> Result<Self, AppError> {
        let root = root_for(id, request)?;
        identity(&root)?;
        let guard = rate_control::Stats::durable(&root, true)?;
        identity(&root.join("workspace.lock"))?;
        let lock = lock_file(&root.join("workspace.lock"), true)?;
        let owner: Owner = read_json(&root.join("owner.json"))?;
        if owner.version != 1
            || owner.id != id
            || owner.request != *request
            || owner.settings != *settings
            || owner.directory != identity(&root)?
        {
            return Err(invalid(
                &root,
                "Canonical source ownership, request or settings changed.",
            ));
        }
        let expected_names = [
            "owner.json".to_owned(),
            "complete.json".to_owned(),
            "source.mkv".to_owned(),
            "chapters.ffmeta".to_owned(),
            "workspace.lock".to_owned(),
            format!(".jesses-{id}-normalized.partial.mkv"),
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
        if owner
            .files
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            != expected_names
        {
            return Err(invalid(
                &root,
                "The canonical source receipt contains an invalid owned-file layout.",
            ));
        }
        let value = Self {
            root,
            owner,
            lock: Some(lock),
            guard: Some(guard),
        };
        value.verify_layout()?;
        Ok(value)
    }
    fn verify_layout(&self) -> Result<(), AppError> {
        self.guard.as_ref().expect("directory guard").check()?;
        if identity(&self.root)? != self.owner.directory {
            return Err(invalid(
                &self.root,
                "The canonical source directory was replaced.",
            ));
        }
        for entry in fs::read_dir(&self.root).map_err(|e| invalid(&self.root, e.to_string()))? {
            let path = entry
                .map_err(|e| invalid(&self.root, e.to_string()))?
                .path();
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| invalid(&path, "Unexpected canonical source file name."))?;
            let expected = self.owner.files.get(name).ok_or_else(|| {
                invalid(
                    &path,
                    "Unexpected file in canonical source workspace; it was preserved.",
                )
            })?;
            if !fs::symlink_metadata(&path)
                .map_err(|e| invalid(&path, e.to_string()))?
                .is_file()
                || identity(&path)? != *expected
            {
                return Err(invalid(
                    &path,
                    "A canonical source file was replaced; the workspace was preserved.",
                ));
            }
        }
        for required in [
            "owner.json",
            "workspace.lock",
            "complete.json",
            "chapters.ffmeta",
        ] {
            if self.owner.files.get(required).is_none_or(|expected| {
                identity(&self.root.join(required)).ok().as_ref() != Some(expected)
            }) {
                return Err(invalid(
                    &self.root,
                    "A required ownership receipt is absent or changed.",
                ));
            }
        }
        Ok(())
    }
    fn cleanup(mut self) -> Result<(), AppError> {
        self.verify_layout()?;
        let present = self
            .owner
            .files
            .iter()
            .filter_map(|(name, identity)| {
                self.root
                    .join(name)
                    .exists()
                    .then_some((self.root.join(name), identity.clone()))
            })
            .collect::<Vec<_>>();
        for (path, expected) in present.iter().filter(|(path, _)| {
            !matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("owner.json" | "workspace.lock")
            )
        }) {
            remove_owned(path, expected)?;
        }
        let owner = self.root.join("owner.json");
        remove_owned(&owner, &self.owner.files["owner.json"])?;
        drop(self.lock.take());
        let lock = self.root.join("workspace.lock");
        remove_owned(&lock, &self.owner.files["workspace.lock"])?;
        if identity(&self.root)? != self.owner.directory {
            return Err(invalid(
                &self.root,
                "The canonical source directory changed before cleanup.",
            ));
        }
        drop(self.guard.take());
        fs::remove_dir(&self.root).map_err(|e| invalid(&self.root, e.to_string()))
    }
}

fn remove_owned(path: &Path, expected: &Identity) -> Result<(), AppError> {
    if identity(path)? != *expected {
        return Err(invalid(
            path,
            "The canonical source artifact changed before deletion; it was preserved.",
        ));
    }
    #[cfg(unix)]
    {
        fs::remove_file(path).map_err(|error| invalid(path, error.to_string()))
    }
    #[cfg(windows)]
    {
        let [volume, high, low] = expected.0.as_slice() else {
            return Err(invalid(path, "Invalid Windows file identity."));
        };
        files::windows_delete_owned(path, (*volume as u32, *high as u32, *low as u32))
            .map_err(|error| invalid(path, error.to_string()))
    }
}

pub(super) struct Prepared {
    pub original: Source,
    pub source_path: PathBuf,
    workspace: Workspace,
}
pub(super) async fn inspect(
    request: &RemuxRequest,
    settings: &EncodeSettings,
    cancel: &watch::Receiver<bool>,
) -> Result<(Source, Document), AppError> {
    let source = Source::open(Path::new(&request.input_path))?;
    files::output_path(request, &source)?;
    if settings.backend == media_core::EncodeBackend::Av1an
        && super::sequence::is_sequence(&source.path)
    {
        return Err(AppError::new(
            "IMAGE_SEQUENCE_UNSUPPORTED",
            "AV1AN cannot use an image sequence source. Use Quick Convert, as in Nmkoder.",
            Some(request.input_path.clone()),
        ));
    }
    // Missing encoders are a discovery failure before any media subprocess is
    // started, including this lightweight normalization inspection.
    if crate::discovery::find_video_encoder(settings.encoder)
        .await
        .map_err(|message| AppError::new("TOOL_DISCOVERY_FAILED", message, None))?
        .is_none()
    {
        return Err(AppError::new(
            "TOOL_MISSING",
            format!(
                "{} was not found. Check its separate entry in Tools.",
                settings.encoder.name()
            ),
            None,
        ));
    }
    let ffprobe = discover("ffprobe", cancel).await?;
    let document = probe_headers(
        &ffprobe,
        &source.path,
        cancel,
        Some(&request.stream_indices),
    )
    .await?;
    document.selected(&request.stream_indices)?;
    source.verify()?;
    Ok((source, document))
}
pub(super) fn needs(
    document: &Document,
    settings: &EncodeSettings,
    force: bool,
) -> Result<bool, AppError> {
    Ok(recipe(document, settings, force)?.is_some())
}

/// Detect sources whose demuxer nominal rate (`r_frame_rate`) disagrees materially
/// with `avg_frame_rate` (e.g. VFR screen captures). Stream-copied AV1AN cuts
/// cannot be fed directly to frame-ordinal readers like L-SMASH because the
/// reader adopts `r_frame_rate` while the encoding plan uses `avg_frame_rate`.
pub(super) fn has_vfr_rate_mismatch(document: &Document, settings: &EncodeSettings) -> bool {
    let Some(video) = document.streams.iter().find(|stream| {
        stream.index == settings.video_stream_index && stream.codec_type.as_deref() == Some("video")
    }) else {
        return false;
    };
    let parse = |value: Option<&str>| -> Option<f64> {
        let (n, d) = value?.split_once('/')?;
        let n: f64 = n.parse().ok()?;
        let d: f64 = d.parse().ok()?;
        (n > 0.0 && d > 0.0 && (n / d).is_finite()).then_some(n / d)
    };
    let (Some(avg), Some(nominal)) = (
        parse(video.avg_frame_rate.as_deref()),
        parse(video.r_frame_rate.as_deref()),
    ) else {
        return false;
    };
    (nominal - avg).abs() > avg * 0.005
}
pub(super) fn exists(id: &str, request: &RemuxRequest) -> Result<bool, AppError> {
    Ok(root_for(id, request)?.symlink_metadata().is_ok())
}
pub(super) struct Cleanup(Workspace);
impl Cleanup {
    pub(super) fn finish(self) -> Result<(), AppError> {
        self.0.cleanup()
    }
    pub(super) fn verify_source(&self) -> Result<(), AppError> {
        let expected = self.0.owner.original.as_ref().ok_or_else(|| {
            invalid(
                &self.0.root,
                "This canonical source does not have a recovery fingerprint.",
            )
        })?;
        let original = Source::open(&expected.path)?;
        let (_owner, cancel) = watch::channel(false);
        let actual = if expected.includes_contents() {
            digest(&original, &cancel)?
        } else {
            metadata_stamp(&original, &cancel)?
        };
        if actual != *expected {
            return Err(invalid(
                &original.path,
                "The original source changed since canonical preparation.",
            ));
        }
        self.0.verify_layout()
    }
    /// Settings do not change this complete original-clock/rotation source.
    /// The recovery action separately invalidates encoded and processed stages.
    pub(super) fn rebind(mut self, settings: &EncodeSettings) -> Result<(), AppError> {
        self.verify_source()?;
        if self.0.owner.settings == *settings {
            return Ok(());
        }
        if self.0.owner.settings.video_stream_index != settings.video_stream_index {
            return Err(invalid(
                &self.0.root,
                "New-settings resume must keep the original selected video stream.",
            ));
        }
        if self.0.owner.recipe.trim_end_seconds.is_some()
            && self.0.owner.settings.trim != settings.trim
        {
            return Err(invalid(
                &self.0.root,
                "New-settings resume cannot change trim bounds on a trim-bounded canonical source.",
            ));
        }
        self.0.owner.settings = settings.clone();
        let path = self.0.root.join("owner.json");
        let mut file = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| invalid(&path, e.to_string()))?;
        if file_identity(&file)? != self.0.owner.files["owner.json"] {
            return Err(invalid(
                &path,
                "Canonical ownership receipt changed before settings rebind.",
            ));
        }
        let bytes = serde_json::to_vec(&self.0.owner).map_err(|e| invalid(&path, e.to_string()))?;
        file.set_len(0)
            .and_then(|_| file.write_all(&bytes))
            .and_then(|_| file.sync_all())
            .map_err(|e| invalid(&path, e.to_string()))?;
        self.0.verify_layout()
    }
}
pub(super) fn prepare_discard(
    id: &str,
    request: &RemuxRequest,
    settings: &EncodeSettings,
) -> Result<Option<Cleanup>, AppError> {
    if exists(id, request)? {
        Ok(Some(Cleanup(Workspace::open(id, request, settings)?)))
    } else {
        Ok(None)
    }
}
pub(super) fn prepare_recovery_action(
    id: &str,
    request: &RemuxRequest,
    saved: &EncodeSettings,
    pending: &EncodeSettings,
) -> Result<Option<Cleanup>, AppError> {
    if !exists(id, request)? {
        return Ok(None);
    }
    Workspace::open(id, request, saved)
        .or_else(|_| Workspace::open(id, request, pending))
        .map(|workspace| Some(Cleanup(workspace)))
}
pub(super) fn discard(
    id: &str,
    request: &RemuxRequest,
    settings: &EncodeSettings,
) -> Result<(), AppError> {
    if exists(id, request)? {
        Workspace::open(id, request, settings)?.cleanup()?;
    }
    Ok(())
}

struct Creating {
    root: PathBuf,
    directory: Identity,
    files: BTreeMap<String, Identity>,
    guard: Option<rate_control::Stats>,
    active: bool,
}
impl Drop for Creating {
    fn drop(&mut self) {
        if !self.active || identity(&self.root).ok().as_ref() != Some(&self.directory) {
            return;
        }
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        let paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        if paths.iter().any(|path| {
            path.file_name()
                .and_then(|v| v.to_str())
                .and_then(|name| self.files.get(name))
                .is_none_or(|expected| identity(path).ok().as_ref() != Some(expected))
        }) {
            return;
        }
        for path in paths {
            let _ = fs::remove_file(path);
        }
        drop(self.guard.take());
        let _ = fs::remove_dir(&self.root);
    }
}
fn chapters(document: &Document, origin: f64) -> String {
    let escape = |value: &str| {
        value
            .replace('\\', "\\\\")
            .replace('=', "\\=")
            .replace(';', "\\;")
            .replace('#', "\\#")
            .replace('\n', "\\\n")
            .replace('\r', "")
    };
    let mut result = ";FFMETADATA1\n".to_owned();
    for chapter in &document.chapters {
        let (Some(start), Some(end)) = (
            number(Some(&chapter.start_time)),
            number(Some(&chapter.end_time)),
        ) else {
            continue;
        };
        if end <= origin {
            continue;
        }
        let start = (start - origin).max(0.0);
        let end = (end - origin).max(start);
        result.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000000\nSTART={}\nEND={}\n",
            (start * 1_000_000.0).round() as u64,
            (end * 1_000_000.0).round() as u64
        ));
        for (key, value) in &chapter.tags {
            result.push_str(&format!("{}={}\n", escape(key), escape(value)));
        }
    }
    result
}
pub(super) fn available(path: &Path) -> Result<u64, AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path_string = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|e| invalid(path, e.to_string()))?;
        let mut value = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(path_string.as_ptr(), value.as_mut_ptr()) } != 0 {
            return Err(invalid(path, std::io::Error::last_os_error().to_string()));
        }
        let value = unsafe { value.assume_init() };
        // statvfs field widths differ between Unix platforms.
        #[allow(clippy::useless_conversion)]
        let bytes = u64::from(value.f_bavail).saturating_mul(u64::from(value.f_frsize));
        Ok(bytes)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let name = path
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        let mut bytes = 0_u64;
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                name.as_ptr(),
                &mut bytes,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(invalid(path, std::io::Error::last_os_error().to_string()));
        }
        Ok(bytes)
    }
}
/// Uncompressed size of one pixel in half bytes, from the pixel format name.
/// An unknown format is sized as 16-bit 4:4:4 with alpha (8 bytes).
fn raw_half_bytes_per_pixel(pix_fmt: Option<&str>) -> u64 {
    const UNKNOWN: u64 = 16;
    let Some(name) = pix_fmt.map(str::to_ascii_lowercase) else {
        return UNKNOWN;
    };
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| name.starts_with(prefix));
    let contains = |parts: &[&str]| parts.iter().any(|part| name.contains(part));
    // Samples per pixel, in halves.
    let samples = if contains(&["420", "411", "410"]) || starts(&["nv12", "nv21", "p010", "p016"]) {
        3
    } else if contains(&["422", "440"])
        || starts(&["nv16", "p210", "p216", "y210", "y212", "yuyv", "uyvy"])
    {
        4
    } else if contains(&["444"])
        || starts(&[
            "gbr", "rgb", "bgr", "0rgb", "0bgr", "argb", "abgr", "x2rgb", "x2bgr", "nv24", "p410",
            "p416",
        ])
    {
        6
    } else if starts(&["gray", "ya"]) {
        2
    } else {
        return UNKNOWN;
    };
    let alpha = starts(&["yuva", "rgba", "bgra", "argb", "abgr", "gbrap", "ya"]);
    let samples = samples + if alpha { 2 } else { 0 };
    let wide = contains(&[
        "p9", "p10", "p12", "p14", "p16", "48", "64", "x2rgb10", "x2bgr10",
    ]) || starts(&[
        "p010", "p016", "p210", "p216", "p410", "p416", "y210", "y212",
    ]) || name
        .strip_prefix("gray")
        .or_else(|| name.strip_prefix("ya"))
        .map(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|bits| bits.parse::<u32>().ok())
        .is_some_and(|bits| bits > 8);
    samples * if wide { 2 } else { 1 }
}
fn disk_preflight(
    path: &Path,
    source_length: u64,
    document: &Document,
    settings: &EncodeSettings,
    recipe: &Recipe,
    compressed: bool,
) -> Result<(), AppError> {
    let video = document
        .streams
        .iter()
        .find(|stream| stream.index == settings.video_stream_index)
        .expect("selected video");
    // Overestimating a shifted Matroska file is safe. Never subtract an origin
    // from a span or allow a short peer track to reduce the allocation bound.
    let duration = document
        .duration()
        .into_iter()
        .chain(normalized_end(document, recipe.origin))
        .reduce(f64::max)
        .filter(|value| *value > 0.0)
        .map(|value| {
            recipe
                .trim_end_seconds
                .map_or(value, |bound| value.min(bound))
        })
        .ok_or_else(|| {
            invalid(
                path,
                "Canonical source preparation needs a known positive duration for disk-capacity preflight.",
            )
        })?;
    let pixels =
        u64::from(video.width.unwrap_or(0)).saturating_mul(u64::from(video.height.unwrap_or(0)));
    let frames = (duration * recipe.numerator as f64 / recipe.denominator as f64).ceil() as u64;
    // Allocation bound: the source's uncompressed frames plus a quarter for
    // FFV1 on noise, a small per-frame reserve, the copied packets and a
    // mux/index reserve. No source is downsampled.
    let video_bytes = if compressed {
        0
    } else {
        pixels
            .saturating_mul(raw_half_bytes_per_pixel(video.pix_fmt.as_deref()))
            .saturating_mul(5)
            .div_ceil(8)
            .saturating_add(4096)
            .saturating_mul(frames)
    };
    let required = video_bytes
        .saturating_add(source_length)
        .saturating_add(16 * 1024 * 1024);
    let free = available(path.parent().expect("workspace parent"))?;
    if required > free {
        return Err(invalid(
            path,
            format!(
                "Source normalization needs up to {} MiB of temporary space; {} MiB is available. Choose an output drive with more space.",
                required.div_ceil(1024 * 1024),
                free / (1024 * 1024)
            ),
        ));
    }
    Ok(())
}
fn input_arguments(input: &Path, recipe: &Recipe) -> Vec<OsString> {
    vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-v".into(),
        "warning".into(),
        "-xerror".into(),
        "-copyts".into(),
        "-itsoffset".into(),
        format!("{:.12}", -recipe.origin).into(),
        "-autorotate".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-i".into(),
        input.as_os_str().to_owned(),
    ]
}

#[allow(clippy::too_many_arguments)]
async fn stage_source(
    ffmpeg: &Path,
    input: &Path,
    document: &Document,
    index: u32,
    recipe: &Recipe,
    chapter_file: &Path,
    compressed: bool,
    temporary: &mut Temporary,
    cancel: &watch::Receiver<bool>,
) -> Result<supervisor::CapturedOutput, AppError> {
    let mut args = input_arguments(input, recipe);
    args.extend([
        "-f".into(),
        "ffmetadata".into(),
        "-i".into(),
        chapter_file.as_os_str().to_owned(),
        "-map".into(),
        "0".into(),
        "-map_metadata".into(),
        "0".into(),
        "-map_chapters".into(),
        "1".into(),
        "-c".into(),
        "copy".into(),
    ]);
    if compressed {
        let clock = format!("{}/({}*TB)", recipe.denominator, recipe.numerator);
        args.extend([
            format!("-bsf:{index}").into(),
            format!("av1_frame_split,setts=pts=N*{clock}:dts=N*{clock}:duration={clock}").into(),
            "-progress".into(),
            "pipe:1".into(),
            "-stats_period".into(),
            "60".into(),
        ]);
    } else {
        args.extend([
            format!("-c:{index}").into(),
            "ffv1".into(),
            format!("-level:{index}").into(),
            "3".into(),
            format!("-g:{index}").into(),
            "1".into(),
            format!("-slicecrc:{index}").into(),
            "1".into(),
            format!("-filter:{index}").into(),
            recipe.filter().into(),
            format!("-fps_mode:{index}").into(),
            "cfr".into(),
        ]);
    }
    args.extend([
        format!("-r:{index}").into(),
        format!("{}/{}", recipe.numerator, recipe.denominator).into(),
        format!("-metadata:s:{index}").into(),
        "rotate=0".into(),
    ]);
    for stream in &document.streams {
        for key in stream.tags.keys() {
            if (stream.index == index && metadata::is_derived_stream_tag(key))
                || (recipe.origin != 0.0 && key.to_ascii_lowercase().starts_with("duration"))
            {
                args.extend([
                    format!("-metadata:s:{}", stream.index).into(),
                    format!("{key}=").into(),
                ]);
            }
        }
        if aac::needed(document, stream) {
            args.extend([
                format!("-bsf:{}", stream.index).into(),
                "aac_adtstoasc".into(),
            ]);
        }
    }
    // A seekable Matroska writer records the authored duration and index.
    if !compressed && let Some(end) = recipe.trim_end_seconds {
        args.extend(["-t".into(), format!("{end:.3}").into()]);
    }
    args.extend([
        "-avoid_negative_ts".into(),
        "disabled".into(),
        "-f".into(),
        "matroska".into(),
        "-y".into(),
        "-nostats".into(),
        temporary.path.as_os_str().to_owned(),
    ]);
    temporary.close_for_path_writer()?;
    let execution = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        128 * 1024,
        TIME,
    )
    .await;
    let rebound = temporary.reopen_after_path_writer();
    let result = execution.map_err(|error| process_error(error, input))?;
    rebound?;
    Ok(result)
}
async fn pixels(
    spec: CommandSpec,
    cancel: &watch::Receiver<bool>,
    path: &Path,
) -> Result<(u64, String), AppError> {
    let result =
        supervisor::run_streaming_stdout(&spec, cancel.clone(), 64 * 1024, TIME, |reader| {
            let mut bytes = [0_u8; 65536];
            let mut length = 0_u64;
            let mut hash = Sha256::new();
            loop {
                let count = reader.read(&mut bytes).map_err(|e| e.to_string())?;
                if count == 0 {
                    break;
                }
                length = length
                    .checked_add(count as u64)
                    .ok_or("Decoded canonical byte count overflow.")?;
                hash.update(&bytes[..count]);
            }
            Ok((length, format!("{:x}", hash.finalize())))
        })
        .await
        .map_err(|e| process_error(e, path))?;
    if !result.status.success() {
        return Err(invalid(
            path,
            format!(
                "Canonical pixel verification failed: {}",
                String::from_utf8_lossy(&result.stderr)
            ),
        ));
    }
    Ok(result.value)
}

async fn static_hdr_metadata(
    ffprobe: &Path,
    input: &Path,
    index: u32,
    header: &[serde_json::Value],
    thorough: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<encode_plan::hdr::StaticMetadata, AppError> {
    let limits: &[Option<u32>] = if thorough {
        &[None]
    } else {
        &[Some(16), Some(256), Some(1024)]
    };
    for &limit in limits {
        if let Some(metadata) =
            scan_static_hdr_metadata(ffprobe, input, index, header, limit, cancel).await?
        {
            return Ok(metadata);
        }
    }
    Err(invalid(
        input,
        "Canonical source color inspection found no decoded frames.",
    ))
}

async fn scan_static_hdr_metadata(
    ffprobe: &Path,
    input: &Path,
    index: u32,
    header: &[serde_json::Value],
    packets: Option<u32>,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<encode_plan::hdr::StaticMetadata>, AppError> {
    #[derive(Deserialize)]
    struct Frame {
        #[serde(default)]
        side_data_list: Vec<serde_json::Value>,
        #[serde(default)]
        interlaced_frame: u8,
    }
    let mut metadata = encode_plan::hdr::StaticMetadata::parse(header)?;
    let mut args = vec![
        "-v".into(),
        "error".into(),
        "-select_streams".into(),
        index.to_string().into(),
    ];
    if let Some(packets) = packets {
        args.extend(["-read_intervals".into(), format!("%+#{packets}").into()]);
    }
    args.extend(
        [
            "-show_frames",
            "-show_entries",
            "frame=interlaced_frame:frame_side_data",
            "-of",
            "json",
            "-i",
        ]
        .map(OsString::from),
    );
    args.push(input.as_os_str().to_owned());
    let result=supervisor::run_streaming_stdout(&CommandSpec {executable:ffprobe.to_owned(),args,cwd:None},cancel.clone(),64*1024,TIME,move |reader| {
        let mut first=true;let mut failure=None;
        super::encode::frame_scan::parse(reader,|frame:Frame| {
            if failure.is_some(){return}
            let checked=(|| {
                if frame.interlaced_frame!=0 {return Err("HDR canonicalization requires progressive source frames.".to_owned())}
                if frame.side_data_list.iter().any(|value|value.get("side_data_type").and_then(serde_json::Value::as_str).is_some_and(|kind|kind.contains("DOVI")||kind.contains("Dolby")||kind.contains("Dynamic")||kind.contains("HDR10+"))) {return Err("Per-frame dynamic HDR cannot be duplicated or dropped during clock normalization.".to_owned())}
                let actual=encode_plan::hdr::StaticMetadata::parse(&frame.side_data_list).map_err(|e|e.message)?;
                if first {metadata.absorb_first_frame(&actual).map_err(|e|e.message)?;first=false;}else{metadata.validate_present(&actual,false).map_err(|e|e.message)?;}
                Ok(())
            })();if let Err(error)=checked{failure=Some(error)}
        })?;
        if let Some(error)=failure{return Err(error)}
        Ok((!first).then_some(metadata))
    }).await.map_err(|e|process_error(e,input))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(invalid(
            input,
            format!(
                "HDR metadata scan failed: {}",
                String::from_utf8_lossy(&result.stderr)
            ),
        ));
    }
    Ok(result.value)
}
impl Prepared {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn prepare(
        manager: &JobManager,
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        original: Source,
        headers: &Document,
        force: bool,
        cancel: &watch::Receiver<bool>,
        log: &Path,
    ) -> Result<Self, AppError> {
        let root = root_for(id, request)?;
        if root.symlink_metadata().is_ok() {
            let workspace = Workspace::open(id, request, settings)?;
            let expected = workspace.owner.original.as_ref().ok_or_else(|| {
                invalid(
                    &root,
                    "This canonical source does not have a recovery fingerprint.",
                )
            })?;
            let actual = recovery_stamp(&original, cancel, expected.includes_contents()).await?;
            if actual != *expected {
                return Err(invalid(
                    &original.path,
                    "The original source changed since canonical preparation. Saved encoder work cannot be reused.",
                ));
            }
            let complete: Complete = read_json(&root.join("complete.json"))?;
            let expected = complete.source.as_ref().ok_or_else(|| {
                invalid(
                    &root,
                    "This canonical artifact does not have a recovery fingerprint.",
                )
            })?;
            let source_path = root.join("source.mkv");
            if expected.path != source_path
                || expected.identity != workspace.owner.files["source.mkv"]
            {
                return Err(invalid(
                    &root,
                    "The canonical source receipt names an unrelated artifact.",
                ));
            }
            let canonical = Source::open(&source_path)?;
            if recovery_stamp(&canonical, cancel, expected.includes_contents()).await? != *expected
            {
                return Err(invalid(
                    &source_path,
                    "The canonical source changed since it was verified.",
                ));
            }
            drop(canonical);
            manager.change(id,|snapshot|append_log(snapshot,"Reusing the verified canonical source. Original and derivative file fingerprints were rechecked.".into())).await;
            return Ok(Self {
                original,
                source_path,
                workspace,
            });
        }
        let ffprobe = discover("ffprobe", cancel).await?;
        let ffmpeg = discover("ffmpeg", cancel).await?;
        let thorough = super::encode::thorough_verification(settings);
        let counted = if thorough {
            Some(
                probe(
                    &ffprobe,
                    &original.path,
                    cancel,
                    Some(&request.stream_indices),
                )
                .await?,
            )
        } else {
            None
        };
        let document = counted.as_ref().unwrap_or(headers);
        let recipe = recipe(document, settings, force)?.ok_or_else(|| {
            invalid(
                &original.path,
                "This source does not need canonicalization.",
            )
        })?;
        if document.streams.iter().enumerate().any(|(index, stream)| {
            stream.index != index as u32
                || !matches!(
                    stream.codec_type.as_deref(),
                    Some("video" | "audio" | "subtitle" | "attachment")
                )
        }) {
            return Err(invalid(
                &original.path,
                "Canonical preparation requires contiguous media/attachment stream IDs. Data/timecode inputs need their explicit mux workflow.",
            ));
        }
        let metadata_video = document
            .streams
            .iter()
            .find(|stream| stream.index == settings.video_stream_index)
            .expect("selected video");
        let retime_frames = if recipe.rotation == 0
            && recipe.origin == 0.0
            && super::encode::header_planning_allowed(settings)
        {
            retime::frame_count(
                &ffprobe,
                &original.path,
                document,
                metadata_video,
                f64::from(recipe.denominator) / f64::from(recipe.numerator),
                cancel,
            )
            .await?
        } else {
            None
        };
        let recovery = thorough || settings.backend == media_core::EncodeBackend::Av1an;
        let source_length = original
            .file()
            .metadata()
            .map_err(|error| invalid(&original.path, error.to_string()))?
            .len();
        let original_stamp = if recovery {
            Some(recovery_stamp(&original, cancel, thorough).await?)
        } else {
            None
        };
        let hdr_video = document
            .streams
            .iter()
            .find(|stream| stream.index == settings.video_stream_index)
            .filter(|stream| {
                matches!(
                    stream.color_transfer.as_deref(),
                    Some("smpte2084" | "arib-std-b67")
                )
            });
        disk_preflight(
            &root,
            source_length,
            document,
            settings,
            &recipe,
            retime_frames.is_some(),
        )?;
        let guard = rate_control::Stats::durable(&root, false)?;
        let directory = identity(&root)?;
        let mut creating = Creating {
            root: root.clone(),
            directory: directory.clone(),
            files: BTreeMap::new(),
            guard: Some(guard),
            active: true,
        };
        let lock = lock_file(&root.join("workspace.lock"), false)?;
        creating
            .files
            .insert("workspace.lock".into(), file_identity(&lock.0)?);
        let mut complete_file = create_file(&root.join("complete.json"))?;
        creating
            .files
            .insert("complete.json".into(), file_identity(&complete_file)?);
        let mut chapter_file = create_file(&root.join("chapters.ffmeta"))?;
        creating
            .files
            .insert("chapters.ffmeta".into(), file_identity(&chapter_file)?);
        chapter_file
            .write_all(chapters(document, recipe.origin).as_bytes())
            .and_then(|_| chapter_file.sync_all())
            .map_err(|e| invalid(&root, e.to_string()))?;
        drop(chapter_file);
        let source_path = root.join("source.mkv");
        let mut temporary = Temporary::create(&source_path, &format!("{id}-normalized"))?;
        let partial_name = temporary
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let partial_identity = file_identity(&temporary.clone_file()?)?;
        creating
            .files
            .insert(partial_name, partial_identity.clone());
        creating.files.insert("source.mkv".into(), partial_identity);
        let mut owner_file = create_file(&root.join("owner.json"))?;
        creating
            .files
            .insert("owner.json".into(), file_identity(&owner_file)?);
        let owner = Owner {
            version: 1,
            id: id.to_owned(),
            request: request.clone(),
            settings: settings.clone(),
            directory,
            files: creating.files.clone(),
            original: original_stamp,
            recipe: recipe.clone(),
        };
        owner_file
            .write_all(&serde_json::to_vec(&owner).map_err(|e| invalid(&root, e.to_string()))?)
            .and_then(|_| owner_file.sync_all())
            .map_err(|e| invalid(&root, e.to_string()))?;
        drop(owner_file);
        let workspace = Workspace {
            root: root.clone(),
            owner,
            lock: Some(lock),
            guard: creating.guard.take(),
        };
        creating.active = false;
        let result=async {
            let chapter_file = root.join("chapters.ffmeta");
            let mut compressed = None;
            if let Some(frames) = retime_frames {
                manager.phase(id, JobState::Preparing, &format!("Adjusting the timestamps of {frames} compressed AV1 frames to {}/{} fps.", recipe.numerator, recipe.denominator)).await;
                let staged = stage_source(&ffmpeg, &original.path, document, settings.video_stream_index, &recipe, &chapter_file, true, &mut temporary, cancel).await?;
                if retime::completed(&staged, frames) {
                    compressed = Some(staged);
                } else {
                    check_cancel(cancel)?;
                    original.verify()?;
                    disk_preflight(&root, source_length, document, settings, &recipe, false)?;
                    manager.change(id, |snapshot| append_log(snapshot, "The compressed AV1 clock could not be normalized one frame at a time. Rendering the normalized video instead.".into())).await;
                }
            }
            let retimed = compressed.is_some();
            let original_hdr = if retimed {
                None
            } else {
                // The transform cannot preserve arbitrary dynamic HDR. A
                // declared HDR source needs complete eligibility evidence
                // before rendering even when output verification is lean.
                Some(static_hdr_metadata(&ffprobe, &original.path, metadata_video.index, &metadata_video.side_data_list, thorough || hdr_video.is_some(), cancel).await?)
            };
            let prepared = if let Some(prepared) = compressed {
                prepared
            } else {
                manager.phase(id,JobState::Preparing,&format!("Preparing a lossless canonical source: display rotation {}°, timestamp origin {:.6}s, CFR {}/{}. Variable input intervals retain duration through explicit frame duplication/drop.",recipe.rotation,recipe.origin,recipe.numerator,recipe.denominator)).await;
                stage_source(&ffmpeg, &original.path, document, settings.video_stream_index, &recipe, &chapter_file, false, &mut temporary, cancel).await?
            };
            let diagnostic=format!("Canonical source preparation:\n{}",String::from_utf8_lossy(&prepared.stderr));
            if let Some(parent)=log.parent() {tokio::fs::create_dir_all(parent).await.map_err(|e|invalid(log,e.to_string()))?;}
            tokio::fs::write(log,diagnostic.as_bytes()).await.map_err(|e|invalid(log,e.to_string()))?;
            if !prepared.status.success() {return Err(invalid(&original.path,format!("FFmpeg could not prepare the canonical source: {}",String::from_utf8_lossy(&prepared.stderr))));}
            temporary.flush_nonempty_async().await?;
            let actual = if thorough {
                probe(&ffprobe, &temporary.path, cancel, None).await?
            } else {
                probe_headers(&ffprobe, &temporary.path, cancel, None).await?
            };
            let kinds = document.streams.iter().map(|stream| stream.codec_type.as_deref().unwrap_or_default()).collect::<Vec<_>>();
            super::output_counts::check_streams(&actual, &kinds)?;
            let selected_video=actual.streams.iter().find(|stream|stream.index==settings.video_stream_index).ok_or_else(||invalid(&temporary.path,"The canonical video stream is absent."))?;
            let (mut width, mut height) = (metadata_video.width, metadata_video.height);
            if matches!(recipe.rotation, 90 | 270) {
                std::mem::swap(&mut width, &mut height);
            }
            if selected_video.codec_name.as_deref() != Some(if retimed { "av1" } else { "ffv1" })
                || (selected_video.width, selected_video.height) != (width, height)
            {
                return Err(invalid(&temporary.path, "The canonical video header does not match the planned codec and display dimensions."));
            }
            if let Some(original_hdr) = original_hdr {
                let observed=static_hdr_metadata(&ffprobe,&temporary.path,settings.video_stream_index,&selected_video.side_data_list,thorough,cancel).await?;
                original_hdr.validate_present(&observed,false)?;observed.validate_present(&original_hdr,false)?;
                if let Some(original_video)=hdr_video
                    && (selected_video.color_primaries!=original_video.color_primaries||selected_video.color_transfer!=original_video.color_transfer||selected_video.color_space!=original_video.color_space||selected_video.color_range!=original_video.color_range||selected_video.pix_fmt!=original_video.pix_fmt) {return Err(invalid(&temporary.path,"Lossless canonicalization changed HDR color interpretation or sample format."));}
            }
            let decoded = if thorough {
            let format=selected_video.pix_fmt.as_deref().ok_or_else(||invalid(&temporary.path,"The canonical source pixel format is unknown."))?;
            let mut reference=input_arguments(&original.path,&recipe);reference.extend(["-map".into(),format!("0:{}",settings.video_stream_index).into(),"-vf".into(),recipe.filter().into()]);
            let mut candidate:Vec<OsString>=["-v","error","-nostdin","-noautorotate","-protocol_whitelist","file","-i"].into_iter().map(OsString::from).chain([temporary.path.as_os_str().to_owned()]).collect();candidate.extend(["-map".into(),format!("0:{}",settings.video_stream_index).into()]);
            let suffix=["-an","-sn","-dn","-pix_fmt",format,"-f","rawvideo","pipe:1"].into_iter().map(OsString::from).collect::<Vec<_>>();reference.extend(suffix.clone());candidate.extend(suffix);
            let expected=pixels(CommandSpec {executable:ffmpeg.clone(),args:reference,cwd:None},cancel,&original.path).await?;
            let observed=pixels(CommandSpec {executable:ffmpeg.clone(),args:candidate,cwd:None},cancel,&temporary.path).await?;
            if expected.0==0 || expected!=observed {return Err(invalid(&temporary.path,"The lossless canonical source does not contain exactly the independently decoded transformed source pixels."));}
            let mut expected_document=document.clone();
            if let Some(format)=&mut expected_document.format {format.start_time=Some("0".into());format.duration=normalized_end(document,recipe.origin).map(|duration|duration.to_string());}
            expected_document.chapters.retain(|chapter|number(Some(&chapter.end_time)).is_some_and(|end|end>recipe.origin));
            for chapter in &mut expected_document.chapters {chapter.start_time=(number(Some(&chapter.start_time)).unwrap_or(0.0)-recipe.origin).max(0.0).to_string();chapter.end_time=(number(Some(&chapter.end_time)).unwrap_or(0.0)-recipe.origin).max(0.0).to_string();}
            let normalized_end=expected_document.duration().map(|duration|duration.to_string());
            for stream in &mut expected_document.streams {
                let end=stream_end(stream).map(|end|(end-recipe.origin).max(0.0).to_string());
                stream.tags.retain(|key,_| !((stream.index==settings.video_stream_index && metadata::is_derived_stream_tag(key)) || (recipe.origin!=0.0 && key.to_ascii_lowercase().starts_with("duration"))));
                stream.packet_start_time=stream.packet_start_time.map(|time|time-recipe.origin);stream.start_time=number(stream.start_time.as_deref()).map(|time|(time-recipe.origin).to_string());
                stream.duration=end;
                if stream.index==settings.video_stream_index {stream.duration=normalized_end.clone();stream.tags.retain(|key,_|!key.eq_ignore_ascii_case("rotate"));stream.start_time=Some("0".into());stream.packet_start_time=Some(0.0);}
            }
            metadata::verify_encoded(&expected_document,&expected_document.streams.iter().collect::<Vec<_>>(),&actual,settings.video_stream_index,"ffv1",(selected_video.width.unwrap_or(0),selected_video.height.unwrap_or(0)),&[])?;
            for stream in document.streams.iter().filter(|stream|stream.index!=settings.video_stream_index && matches!(stream.codec_type.as_deref(),Some("video"|"audio"|"subtitle"))) {
                if aac::needed(document,stream){
                    let actual_stream=actual.streams.iter().find(|actual|actual.index==stream.index).expect("canonical AAC stream");
                    let evidence=aac::verify(&ffmpeg,&ffprobe,&original.path,stream,&temporary.path,actual_stream,-recipe.origin,cancel).await?;
                    manager.change(id,|snapshot|append_log(snapshot,evidence)).await;
                }else{mux::verify_shifted_packets(&ffprobe,&original.path,stream.index,&temporary.path,stream.index,-recipe.origin,cancel).await?;}
            }
            Some(observed)
            } else {
                None
            };
            original.verify()?;workspace.verify_layout()?;check_cancel(cancel)?;
            temporary.publish(&source_path)?;drop(temporary);
            let stamp = if recovery { Some(recovery_stamp(&Source::open(&source_path)?, cancel, thorough).await?) } else { None };
            let (decoded_bytes, decoded_sha256) = decoded.map_or((None, None), |(bytes, sha256)| (Some(bytes), Some(sha256)));
            let complete=Complete {source:stamp,decoded_bytes,decoded_sha256};
            complete_file.write_all(&serde_json::to_vec(&complete).map_err(|e|invalid(&root,e.to_string()))?).and_then(|_|complete_file.sync_all()).map_err(|e|invalid(&root,e.to_string()))?;
            let summary = if let Some(frames) = retime_frames.filter(|_| retimed) {
                format!("Prepared the compressed source clock for {frames} AV1 frames without decoding or re-encoding them.")
            } else if let (Some(bytes), Some(sha256)) = (complete.decoded_bytes, complete.decoded_sha256) {
                format!("Verified lossless canonical source: {bytes} decoded bytes, SHA-256 {sha256}. Original source bytes and copied track packet timing were checked.")
            } else {
                "Verified lossless canonical source headers.".into()
            };
            manager.change(id,|snapshot|append_log(snapshot,summary)).await;
            Ok::<_,AppError>(())
        }.await;
        drop(complete_file);
        if let Err(error) = result {
            let cleanup = workspace.cleanup();
            return match cleanup {
                Ok(()) => Err(error),
                Err(cleanup) => Err(AppError::new(
                    error.code,
                    format!("{} Cleanup: {}", error.message, cleanup.message),
                    cleanup.path,
                )),
            };
        }
        Ok(Self {
            original,
            source_path,
            workspace,
        })
    }
    pub(super) fn finish(self, keep: bool) -> Result<(), AppError> {
        let Self {
            original,
            workspace,
            ..
        } = self;
        original.verify()?;
        drop(original);
        if keep {
            workspace.verify_layout()
        } else {
            workspace.cleanup()
        }
    }
}

pub(super) fn oriented_settings(settings: &EncodeSettings, rotation: i32) -> EncodeSettings {
    let mut value = settings.clone();
    let c = settings.framing.crop;
    value.framing.crop = match rotation.rem_euclid(360) {
        90 => media_core::CropSettings {
            top: c.right,
            right: c.bottom,
            bottom: c.left,
            left: c.top,
        },
        180 => media_core::CropSettings {
            top: c.bottom,
            right: c.left,
            bottom: c.top,
            left: c.right,
        },
        270 => media_core::CropSettings {
            top: c.left,
            right: c.top,
            bottom: c.right,
            left: c.bottom,
        },
        _ => c,
    };
    value
}
impl Prepared {
    pub(super) fn effective_settings(&self, settings: &EncodeSettings) -> EncodeSettings {
        oriented_settings(settings, self.workspace.owner.recipe.rotation)
    }
}
/// The stream as displayed: a quarter turn swaps its axes and sample aspect
/// ratio, and the display transform is no longer pending.
fn display_oriented(video: &mut metadata::Stream, rotation: i32) {
    if matches!(rotation, 90 | 270) {
        std::mem::swap(&mut video.width, &mut video.height);
        if let Some((n, d)) = video
            .sample_aspect_ratio
            .as_deref()
            .and_then(|value| value.split_once(':'))
            .map(|(n, d)| (n.to_owned(), d.to_owned()))
        {
            video.sample_aspect_ratio = Some(format!("{d}:{n}"));
        }
    }
    video
        .tags
        .retain(|key, _| !key.eq_ignore_ascii_case("rotate"));
    video.side_data_list.retain(|value| {
        !value
            .get("side_data_type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind == "Display Matrix")
    });
}
/// Batch admission previews the same geometry/clock without writing derivatives.
/// The execution path still proves every original/derived frame and packet.
pub(super) fn preview_document(
    document: &mut Document,
    settings: &EncodeSettings,
) -> Result<EncodeSettings, AppError> {
    let proposed = recipe(document, settings, false)?;
    let Some(recipe) = proposed else {
        // A directly encoded rotation is previewed in display orientation;
        // its origin stays, as the encode restores it.
        let video = document
            .streams
            .iter_mut()
            .find(|stream| stream.index == settings.video_stream_index);
        if let Some(video) = video
            && let Some(rotation) = direct_rotation(video, settings).filter(|value| *value != 0)
        {
            display_oriented(video, rotation);
            return Ok(oriented_settings(settings, rotation));
        }
        return Ok(settings.clone());
    };
    let duration = normalized_end(document, recipe.origin);
    let video = document
        .streams
        .iter_mut()
        .find(|stream| stream.index == settings.video_stream_index)
        .expect("selected video");
    video.start_time = Some("0".into());
    video.packet_start_time = Some(0.0);
    display_oriented(video, recipe.rotation);
    video.avg_frame_rate = Some(format!("{}/{}", recipe.numerator, recipe.denominator));
    video.r_frame_rate = video.avg_frame_rate.clone();
    if let Some(format) = &mut document.format {
        format.start_time = Some("0".into());
        format.duration = duration.map(|duration| duration.to_string());
    }
    Ok(oriented_settings(settings, recipe.rotation))
}

#[cfg(test)]
mod orientation_tests {
    use super::*;

    #[tokio::test]
    async fn canonical_recovery_detects_changes_with_legacy_and_metadata_receipts() {
        let path = std::env::temp_dir().join(format!(
            "jesses-canonical-stamp-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed),
        ));
        let mut file = create_file(&path).unwrap();
        file.write_all(b"original").unwrap();
        drop(file);
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let fixture = Fixture(path);
        let (_owner, cancel) = watch::channel(false);
        let source = Source::open(&fixture.0).unwrap();
        let metadata = recovery_stamp(&source, &cancel, false).await.unwrap();
        let legacy = digest(&source, &cancel).unwrap();
        assert!(!metadata.includes_contents());
        assert!(legacy.includes_contents());
        for expected in [&metadata, &legacy] {
            assert_eq!(
                &recovery_stamp(&source, &cancel, expected.includes_contents())
                    .await
                    .unwrap(),
                expected
            );
        }
        drop(source);

        let mut file = OpenOptions::new().write(true).open(&fixture.0).unwrap();
        file.write_all(b"modified").unwrap();
        file.set_times(
            std::fs::FileTimes::new().set_modified(SystemTime::now() + Duration::from_secs(2)),
        )
        .unwrap();
        drop(file);
        let source = Source::open(&fixture.0).unwrap();
        for expected in [&metadata, &legacy] {
            assert_ne!(
                &recovery_stamp(&source, &cancel, expected.includes_contents())
                    .await
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn canonical_clock_prefers_the_average_to_the_vfr_timestamp_grid() {
        for (average, base, expected) in [
            ("30000/1001", "240/1", (30000, 1001)),
            ("0/0", "24000/1001", (24000, 1001)),
            ("240/1", "0/0", (240, 1)),
        ] {
            let document: Document = serde_json::from_value(serde_json::json!({
                "streams": [{
                    "index": 0, "codec_type": "video", "start_time": "0",
                    "avg_frame_rate": average, "r_frame_rate": base,
                }],
            }))
            .unwrap();
            let normalized = recipe(&document, &EncodeSettings::default(), true)
                .unwrap()
                .unwrap();
            assert_eq!((normalized.numerator, normalized.denominator), expected);
        }
    }

    #[test]
    fn transport_stream_duration_is_a_span_and_audio_lead_survives_preview() {
        let source:Document=serde_json::from_value(serde_json::json!({"format":{"format_name":"mpegts","start_time":"1.431644","duration":"2.010023"},"streams":[{"index":0,"codec_type":"video","width":128,"height":96,"start_time":"1.441667","duration":"2.0","r_frame_rate":"24/1"},{"index":1,"codec_type":"audio","start_time":"1.431644","duration":"1.992"}]})).unwrap();
        assert!((normalized_end(&source, 1.441667).unwrap() - 2.0).abs() < 1e-6);
        // AV1AN still rebases through a canonical source.
        let mut document = source.clone();
        let av1an = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            ..Default::default()
        };
        preview_document(&mut document, &av1an).unwrap();
        assert!((document.duration().unwrap() - 2.0).abs() < 1e-6);
        assert_eq!(document.streams[0].start_time.as_deref(), Some("0"));
        // Quick Convert keeps the origin and restores it at the mux.
        let mut document = source.clone();
        preview_document(&mut document, &EncodeSettings::default()).unwrap();
        assert_eq!(document.streams[0].start_time.as_deref(), Some("1.441667"));
        assert!((document.duration().unwrap() - 2.010023).abs() < 1e-6);
        assert!(!needs(&source, &EncodeSettings::default(), false).unwrap());
        assert!(needs(&source, &av1an, false).unwrap());
    }

    fn rotated(side_data: serde_json::Value) -> Document {
        serde_json::from_value(serde_json::json!({"streams":[{
            "index":0,"codec_type":"video","width":128,"height":96,"start_time":"0",
            "sample_aspect_ratio":"4:3","r_frame_rate":"24/1","side_data_list":side_data
        }]}))
        .unwrap()
    }

    #[test]
    fn quick_convert_rotates_right_angle_turns_itself_and_keeps_the_canonical_copy_otherwise() {
        let quick = EncodeSettings::default();
        let turn = |rotation: i32, matrix: &str| {
            rotated(serde_json::json!([{
                "side_data_type":"Display Matrix","displaymatrix":matrix,"rotation":rotation
            }]))
        };
        // FFprobe's matrices for a quarter turn and a half turn.
        let quarter = turn(
            90,
            "\n00000000: 0 -65536 0\n00000001: 65536 0 0\n00000002: 0 0 1073741824\n",
        );
        let half = turn(
            180,
            "\n00000000: -65536 0 0\n00000001: 0 -65536 0\n00000002: 0 0 1073741824\n",
        );
        // A mirrored quarter turn keeps the canonical copy.
        let mirrored = turn(
            90,
            "\n00000000: 0 65536 0\n00000001: 65536 0 0\n00000002: 0 0 1073741824\n",
        );
        assert_eq!(direct_rotation(&mirrored.streams[0], &quick), None);
        assert!(needs(&mirrored, &quick, false).unwrap());
        for (document, expected) in [(&quarter, 90), (&half, 180)] {
            let video = &document.streams[0];
            assert_eq!(direct_rotation(video, &quick), Some(expected));
            assert!(!needs(document, &quick, false).unwrap());
            // Reshaping, re-timing or AV1AN keeps the canonical copy.
            for other in [
                EncodeSettings {
                    backend: media_core::EncodeBackend::Av1an,
                    ..Default::default()
                },
                EncodeSettings {
                    trim: Some(
                        serde_json::from_value(
                            serde_json::json!({"startFrame":0,"endFrameExclusive":4}),
                        )
                        .unwrap(),
                    ),
                    ..Default::default()
                },
            ] {
                assert_eq!(direct_rotation(video, &other), None);
                assert!(needs(document, &other, false).unwrap());
            }
        }
        // A rotate tag alone, as older demuxers reported it.
        let tagged: Document = serde_json::from_value(serde_json::json!({"streams":[{
            "index":0,"codec_type":"video","width":128,"height":96,"start_time":"0",
            "sample_aspect_ratio":"4:3","r_frame_rate":"24/1","tags":{"rotate":"270"}
        }]}))
        .unwrap();
        assert_eq!(direct_rotation(&tagged.streams[0], &quick), Some(270));
        // The batch preview shows the turned raster and turns the crop with it.
        let mut preview = tagged.clone();
        let settings = EncodeSettings {
            framing: media_core::VideoFraming {
                crop: media_core::CropSettings {
                    top: 8,
                    right: 16,
                    bottom: 24,
                    left: 32,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let oriented = preview_document(&mut preview, &settings).unwrap();
        let video = &preview.streams[0];
        assert_eq!((video.width, video.height), (Some(96), Some(128)));
        assert_eq!(video.sample_aspect_ratio.as_deref(), Some("3:4"));
        assert!(video.tags.is_empty());
        assert_eq!(video.start_time.as_deref(), Some("0"));
        let c = oriented.framing.crop;
        assert_eq!([c.top, c.right, c.bottom, c.left], [32, 8, 16, 24]);
        // Dynamic HDR metadata keeps the canonical route's refusal.
        let dynamic = rotated(serde_json::json!([
            {"side_data_type":"Display Matrix","rotation":90},
            {"side_data_type":"HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"}
        ]));
        assert_eq!(direct_rotation(&dynamic.streams[0], &quick), None);
    }

    #[test]
    fn the_space_check_sizes_frames_from_their_pixel_format() {
        for (format, bytes_x2) in [
            ("yuv420p", 3),
            ("yuvj420p", 3),
            ("nv12", 3),
            ("yuv420p10le", 6),
            ("p010le", 6),
            ("yuv422p", 4),
            ("yuv422p10le", 8),
            ("yuv444p", 6),
            ("yuv444p12le", 12),
            ("yuva420p", 5),
            ("gray", 2),
            ("gray10le", 4),
            ("ya8", 4),
            ("rgb24", 6),
            ("rgba", 8),
            ("rgb48le", 12),
            ("rgba64le", 16),
            ("gbrp10le", 12),
            ("gbrap", 8),
            ("something-new", 16),
        ] {
            assert_eq!(raw_half_bytes_per_pixel(Some(format)), bytes_x2, "{format}");
        }
        assert_eq!(raw_half_bytes_per_pixel(None), 16);
    }
    #[test]
    fn coded_crop_edges_rotate_before_display_geometry_and_preserve_authority() {
        let settings = EncodeSettings {
            framing: media_core::VideoFraming {
                crop: media_core::CropSettings {
                    top: 8,
                    right: 16,
                    bottom: 24,
                    left: 32,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        for (angle, expected) in [
            (90, [16, 24, 32, 8]),
            (180, [24, 32, 8, 16]),
            (270, [32, 8, 16, 24]),
        ] {
            let effective = oriented_settings(&settings, angle);
            let c = effective.framing.crop;
            assert_eq!([c.top, c.right, c.bottom, c.left], expected);
        }
        assert_eq!(settings.framing.crop.top, 8);
    }
    #[test]
    fn detects_vfr_nominal_versus_average_rate_mismatch() {
        let mut document: Document = serde_json::from_value(serde_json::json!({
            "streams": [{
                "index": 0,
                "codec_type": "video",
                "codec_name": "av1",
                "avg_frame_rate": "2722500/45247",
                "r_frame_rate": "240/1"
            }]
        }))
        .unwrap();
        let settings = EncodeSettings::default();
        assert!(has_vfr_rate_mismatch(&document, &settings));
        document.streams[0].r_frame_rate = Some("24000/1001".into());
        document.streams[0].avg_frame_rate = Some("23976/1000".into());
        assert!(!has_vfr_rate_mismatch(&document, &settings));
    }
}
