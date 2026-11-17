//! Bounded, atomic preference writes. Media drafts and executable arguments are
//! deliberately outside the general-preference import whitelist. Validated
//! parameter presets share this application instance's atomic preference store.
use media_core::{
    AppError, EncoderParameterPreset, EncoderParameterPresetKey, GeneralPreferences,
    PreferenceImportPreview, SavePreferencesRequest, UserPreferences,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

const MAX_BYTES: u64 = 256 * 1024;
const MAX_RECENT: usize = 15;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    preferences: UserPreferences,
}
pub struct PreferencesStore {
    path: PathBuf,
    current: Mutex<UserPreferences>,
    /// Set when the file on disk must not be replaced this session.
    read_only: Option<AppError>,
    /// Reported with every read so the app can explain a recovery.
    notice: Option<String>,
}

/// What opening found. A damaged file never leaves the app without usable
/// preferences: it is set aside or salvaged, and a file this version cannot
/// safely replace is left untouched while the session uses defaults.
struct Opened {
    preferences: UserPreferences,
    read_only: Option<AppError>,
    notice: Option<String>,
}

impl Opened {
    fn clean(preferences: UserPreferences) -> Self {
        Self {
            preferences,
            read_only: None,
            notice: None,
        }
    }
    fn read_only(preferences: UserPreferences, message: String) -> Self {
        Self {
            preferences,
            read_only: Some(AppError::new(
                "PREFERENCES_READ_ONLY",
                message.clone(),
                None,
            )),
            notice: Some(message),
        }
    }
}

/// A free `preferences.json.bad-<seconds>[-n]` name next to the file.
fn aside_path(path: &Path) -> PathBuf {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let base = format!("preferences.json.bad-{seconds}");
    let directory = path.parent().unwrap_or(Path::new("."));
    (0..)
        .map(|n| {
            directory.join(if n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            })
        })
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .expect("an unused name")
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Renames an unreadable file out of the way and starts from defaults.
fn set_aside(path: &Path, reason: &str) -> Opened {
    let target = aside_path(path);
    match fs::rename(path, &target) {
        Ok(()) => Opened {
            preferences: UserPreferences::default(),
            read_only: None,
            notice: Some(format!(
                "Saved preferences could not be read ({reason}). The file was renamed to {} and default preferences are in use.",
                file_name(&target)
            )),
        },
        Err(cause) => Opened::read_only(
            UserPreferences::default(),
            format!(
                "Saved preferences could not be read ({reason}) and could not be set aside ({cause}). Default preferences are in use and changes will not be saved this session."
            ),
        ),
    }
}

fn open_file(path: &Path) -> Opened {
    match fs::metadata(path) {
        Ok(_) => {}
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            return Opened::clean(UserPreferences::default());
        }
        Err(cause) => {
            return Opened::read_only(
                UserPreferences::default(),
                format!(
                    "Saved preferences could not be checked ({cause}). Default preferences are in use and the file is left unchanged this session."
                ),
            );
        }
    }
    let bytes = match read_limited(path) {
        Ok(bytes) => bytes,
        Err(Limited::TooLarge) => return set_aside(path, "the file exceeds 256 KiB"),
        Err(Limited::Io(cause)) => {
            return Opened::read_only(
                UserPreferences::default(),
                format!(
                    "Saved preferences could not be read ({cause}). Default preferences are in use and the file is left unchanged this session."
                ),
            );
        }
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(cause) => return set_aside(path, &format!("it is not valid JSON: {cause}")),
    };
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(1) => {}
        Some(version) if version > 1 => {
            return Opened::read_only(
                UserPreferences::default(),
                format!(
                    "Saved preferences were written by a newer version of Jesses (format {version}). The file was left unchanged; default preferences are in use and changes will not be saved until Jesses is updated."
                ),
            );
        }
        _ => return set_aside(path, "it has no supported format version"),
    }
    let record: Record = match serde_json::from_value(value) {
        Ok(record) => record,
        Err(cause) => return set_aside(path, &cause.to_string()),
    };
    let mut preferences = record.preferences;
    preferences.notice = None;
    let mut dropped = Vec::new();
    if validate_general(&preferences.general).is_err() {
        preferences.general = GeneralPreferences::default();
        dropped.push("the general preferences were reset to their defaults".to_owned());
    }
    let presets = std::mem::take(&mut preferences.parameter_presets);
    let saved = presets.len();
    for preset in presets {
        let duplicate = preferences
            .parameter_presets
            .iter()
            .any(|other| same_preset(other, &preset.name, preset.encoder, preset.backend));
        if !duplicate
            && preferences.parameter_presets.len() < 30
            && validate_presets(std::slice::from_ref(&preset)).is_ok()
        {
            preferences.parameter_presets.push(preset);
        }
    }
    if preferences.parameter_presets.len() != saved {
        dropped.push(format!(
            "{} saved parameter preset(s) that this version cannot use were removed",
            saved - preferences.parameter_presets.len()
        ));
    }
    let recents = std::mem::take(&mut preferences.recent_paths);
    let usable: Vec<String> = recents
        .iter()
        .filter(|path| valid_path(path))
        .take(500)
        .cloned()
        .collect();
    if usable.len() != recents.len() {
        dropped.push(format!(
            "{} invalid recent path(s) were removed",
            recents.len() - usable.len()
        ));
    }
    preferences.recent_paths = normalize(usable).unwrap_or_default();
    if dropped.is_empty() {
        return Opened::clean(preferences);
    }
    // Keep the original bytes before the next save replaces them.
    let backup = aside_path(path);
    match fs::write(&backup, &bytes) {
        Ok(()) => Opened {
            preferences,
            read_only: None,
            notice: Some(format!(
                "Some saved preferences could not be used: {}. A copy of the original file was saved as {}.",
                dropped.join("; "),
                file_name(&backup)
            )),
        },
        Err(cause) => Opened::read_only(
            preferences,
            format!(
                "Some saved preferences could not be used: {}. A copy could not be saved ({cause}), so the file is left unchanged and changes will not be saved this session.",
                dropped.join("; ")
            ),
        ),
    }
}
fn error(detail: impl std::fmt::Display) -> AppError {
    AppError::new(
        "PREFERENCES_FAILED",
        format!("Preferences could not be read or saved: {detail}. Existing files were preserved."),
        None,
    )
}
fn valid_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32768
        && !value.chars().any(char::is_control)
        && Path::new(value).is_absolute()
}
fn validate_general(value: &GeneralPreferences) -> Result<(), AppError> {
    if !value.default_output_directory.is_empty() && !valid_path(&value.default_output_directory) {
        return Err(error(
            "the output folder must be an absolute path without control characters",
        ));
    }
    if !(1..=100).contains(&value.auto_crop_samples) {
        return Err(error(
            "automatic crop sample count must be between 1 and 100",
        ));
    }
    if !(1..=120).contains(&value.keyframe_interval_seconds) {
        return Err(error("keyframe interval must be between 1 and 120 seconds"));
    }
    Ok(())
}
fn path_key(path: &str) -> String {
    #[cfg(windows)]
    {
        let normalized = path.replace('/', "\\").to_lowercase();
        let normalized = if let Some(unc) = normalized.strip_prefix("\\\\?\\unc\\") {
            format!("\\\\{unc}")
        } else {
            normalized
                .strip_prefix("\\\\?\\")
                .unwrap_or(&normalized)
                .to_owned()
        };
        normalized.trim_end_matches('\\').to_owned()
    }
    #[cfg(not(windows))]
    {
        path.trim_end_matches('/').to_owned()
    }
}
fn normalize(paths: Vec<String>) -> Result<Vec<String>, AppError> {
    if paths.len() > 500 {
        return Err(error("too many recent paths"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for path in paths {
        if !valid_path(&path) {
            return Err(error(
                "recent paths must be absolute and contain no control characters",
            ));
        }
        if seen.insert(path_key(&path)) && out.len() < MAX_RECENT {
            out.push(path);
        }
    }
    Ok(out)
}
enum Limited {
    TooLarge,
    Io(std::io::Error),
}
fn read_limited(path: &Path) -> Result<Vec<u8>, Limited> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(Limited::Io)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(Limited::Io)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Limited::TooLarge);
    }
    Ok(bytes)
}
fn read_bounded(path: &Path) -> Result<Vec<u8>, AppError> {
    read_limited(path).map_err(|cause| match cause {
        Limited::TooLarge => error("the file exceeds 256 KiB"),
        Limited::Io(cause) => error(cause),
    })
}
fn same_preset(
    value: &EncoderParameterPreset,
    name: &str,
    encoder: media_core::VideoEncoder,
    backend: media_core::EncodeBackend,
) -> bool {
    value.name == name && value.encoder == encoder && value.backend == backend
}
fn validate_presets(values: &[EncoderParameterPreset]) -> Result<(), AppError> {
    if values.len() > 30 {
        return Err(error("at most 30 parameter presets are allowed"));
    }
    for (index, value) in values.iter().enumerate() {
        if value.name.trim().is_empty()
            || value.name.chars().count() > 64
            || value.name.chars().any(char::is_control)
        {
            return Err(error(
                "parameter preset names need 1–64 characters without control characters",
            ));
        }
        if values[..index]
            .iter()
            .any(|other| same_preset(other, &value.name, value.encoder, value.backend))
        {
            return Err(error("parameter presets repeat an encoder, route and name"));
        }
        crate::jobs::parameters::validate_values(value.encoder, value.backend, &value.parameters)
            .map_err(|cause| error(cause.message))?;
    }
    Ok(())
}
impl PreferencesStore {
    pub fn open(directory: PathBuf) -> Self {
        let path = directory.join("preferences.json");
        let opened = open_file(&path);
        crate::supervisor::configure_console_preferences(&opened.preferences.general);
        Self {
            path,
            current: Mutex::new(opened.preferences),
            read_only: opened.read_only,
            notice: opened.notice,
        }
    }
    pub fn get(&self) -> Result<UserPreferences, AppError> {
        let mut preferences = self.current.lock().map_err(error)?.clone();
        crate::supervisor::configure_console_preferences(&preferences.general);
        preferences.notice = self.notice.clone();
        Ok(preferences)
    }
    fn change(
        &self,
        edit: impl FnOnce(&mut UserPreferences) -> Result<(), AppError>,
    ) -> Result<UserPreferences, AppError> {
        let mut current = self.current.lock().map_err(error)?;
        if let Some(read_only) = &self.read_only {
            return Err(read_only.clone());
        }
        let mut next = current.clone();
        edit(&mut next)?;
        if *current == next {
            return Ok(next);
        }
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| error("preference revision exhausted"))?;
        let bytes = serde_json::to_vec_pretty(&Record {
            version: 1,
            preferences: next.clone(),
        })
        .map_err(error)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(error("preferences exceed 256 KiB"));
        }
        let directory = self.path.parent().expect("preference directory");
        fs::create_dir_all(directory).map_err(error)?;
        let temporary = directory.join(format!(
            ".preferences-{}-{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(error)?;
        let result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &self.path)?;
            #[cfg(unix)]
            File::open(directory)?.sync_all()?;
            Ok::<_, std::io::Error>(())
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&temporary);
            return Err(error(e));
        }
        crate::supervisor::configure_console_preferences(&next.general);
        *current = next.clone();
        Ok(next)
    }
    pub fn save(&self, request: SavePreferencesRequest) -> Result<UserPreferences, AppError> {
        validate_general(&request.general)?;
        let recent = request.recent_paths.map(normalize).transpose()?;
        self.change(|next| {
            next.general = request.general;
            if let Some(paths) = recent {
                next.recent_paths = paths;
            }
            Ok(())
        })
    }
    pub fn remember(&self, paths: Vec<String>) -> Result<UserPreferences, AppError> {
        let paths = normalize(paths)?;
        self.change(|next| {
            next.recent_paths =
                normalize(paths.into_iter().chain(next.recent_paths.clone()).collect())?;
            Ok(())
        })
    }
    pub fn parameter_presets(&self) -> Result<Vec<EncoderParameterPreset>, AppError> {
        Ok(self.get()?.parameter_presets)
    }
    pub fn save_parameter_preset(
        &self,
        preset: EncoderParameterPreset,
    ) -> Result<Vec<EncoderParameterPreset>, AppError> {
        validate_presets(std::slice::from_ref(&preset))?;
        Ok(self
            .change(|next| {
                next.parameter_presets.retain(|value| {
                    !same_preset(value, &preset.name, preset.encoder, preset.backend)
                });
                next.parameter_presets.push(preset);
                validate_presets(&next.parameter_presets)
            })?
            .parameter_presets)
    }
    pub fn remove_parameter_preset(
        &self,
        key: EncoderParameterPresetKey,
    ) -> Result<Vec<EncoderParameterPreset>, AppError> {
        Ok(self
            .change(|next| {
                next.parameter_presets
                    .retain(|value| !same_preset(value, &key.name, key.encoder, key.backend));
                Ok(())
            })?
            .parameter_presets)
    }
    pub fn preview_import(&self, path: PathBuf) -> Result<PreferenceImportPreview, AppError> {
        let existing = self.get()?;
        let source: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&read_bounded(&path)?).map_err(error)?;
        if source.len() > 1000 {
            return Err(error("too many imported preference keys"));
        }
        let mut request = SavePreferencesRequest {
            general: existing.general,
            recent_paths: None,
        };
        let mut accepted_keys = Vec::new();
        let mut warnings = Vec::new();
        if let Some(value) = source.get("DefaultOutputDir") {
            if let Some(value) = value.as_str().filter(|v| v.is_empty() || valid_path(v)) {
                request.general.default_output_directory = value.into();
                accepted_keys.push("DefaultOutputDir".into());
            } else {
                warnings.push(
                    "The saved output folder is invalid on this platform and was skipped.".into(),
                );
            }
        }
        if let Some(value) = source.get("RecentFiles") {
            if let Some(paths) = value
                .as_str()
                .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
                .filter(|v| v.len() <= 500)
            {
                let invalid = paths.iter().filter(|p| !valid_path(p)).count();
                request.recent_paths = Some(normalize(
                    paths.into_iter().filter(|p| valid_path(p)).collect(),
                )?);
                accepted_keys.push("RecentFiles".into());
                if invalid > 0 {
                    warnings.push(format!(
                        "Skipped {invalid} recent paths that are invalid on this platform."
                    ));
                }
            } else {
                warnings.push("The saved recent-file list is invalid and was skipped.".into());
            }
        }
        if accepted_keys.is_empty() {
            return Err(error("this file contains no supported general preferences"));
        }
        Ok(PreferenceImportPreview {
            ignored_key_count: (source.len() - accepted_keys.len()) as u32,
            request,
            accepted_keys,
            warnings,
        })
    }
}

/// Only invoked after the user selects a recent entry; never probe offline
/// shares merely to render a menu or load preferences.
pub fn recent_path_is_folder(path: String) -> Result<bool, AppError> {
    if !valid_path(&path) {
        return Err(error("the recent path is invalid"));
    }
    let metadata = fs::metadata(&path).map_err(|e| {
        AppError::new(
            "RECENT_MEDIA_UNAVAILABLE",
            format!("The recent media is unavailable: {e}"),
            Some(path),
        )
    })?;
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(error("the recent entry is not a file or folder"));
    }
    Ok(metadata.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_settings_defaults_upgrade_old_files_and_preserve_explicit_false() {
        let dir = fixture();
        fs::write(dir.join("preferences.json"), br#"{"version":1,"preferences":{"general":{"defaultOutputDirectory":"","recursiveImport":false},"recentPaths":[],"revision":0}}"#).unwrap();
        let store = PreferencesStore::open(dir.clone());
        let defaults = store.get().unwrap().general;
        assert_eq!(defaults.auto_crop_samples, 10);
        assert_eq!(defaults.keyframe_interval_seconds, 10);
        assert!(defaults.mp4_faststart && defaults.av1an_console_visible);
        assert!(
            defaults.reset_on_new_file.trim
                && defaults.reset_on_new_file.crop
                && defaults.reset_on_new_file.deinterlace
                && defaults.reset_on_new_file.tone_map
        );
        assert!(
            !defaults.reset_on_new_file.frame_rate
                && !defaults.reset_on_new_file.resize
                && !defaults.reset_on_new_file.borders
                && !defaults.reset_on_new_file.custom_filters
        );
        let mut edited = defaults;
        edited.auto_crop_samples = 3;
        edited.keyframe_interval_seconds = 2;
        edited.mp4_faststart = false;
        edited.av1an_console_visible = false;
        edited.console_window = media_core::ConsoleWindowMode::VisibleStayOpen;
        edited.reset_on_new_file.trim = false;
        store
            .save(SavePreferencesRequest {
                general: edited.clone(),
                recent_paths: None,
            })
            .unwrap();
        assert_eq!(
            PreferencesStore::open(dir.clone()).get().unwrap().general,
            edited
        );
        let bytes = fs::read(dir.join("preferences.json")).unwrap();
        edited.auto_crop_samples = 0;
        assert!(
            store
                .save(SavePreferencesRequest {
                    general: edited.clone(),
                    recent_paths: None
                })
                .is_err()
        );
        edited.auto_crop_samples = 10;
        edited.keyframe_interval_seconds = 121;
        assert!(
            store
                .save(SavePreferencesRequest {
                    general: edited,
                    recent_paths: None
                })
                .is_err()
        );
        assert_eq!(fs::read(dir.join("preferences.json")).unwrap(), bytes);
        fs::remove_dir_all(dir).unwrap();
    }
    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "jesses-preferences-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn saves_atomically_recents_keep_order_and_general_edits_do_not_drop_them() {
        let dir = fixture();
        let store = PreferencesStore::open(dir.clone());
        let first = dir.join("1.mkv").display().to_string();
        let second = dir.join("2.mkv").display().to_string();
        store.remember(vec![first.clone(), second.clone()]).unwrap();
        store.remember(vec![second.clone()]).unwrap();
        let prefs = store
            .save(SavePreferencesRequest {
                general: GeneralPreferences {
                    default_output_directory: dir.display().to_string(),
                    recursive_import: true,
                    ..GeneralPreferences::default()
                },
                recent_paths: None,
            })
            .unwrap();
        assert_eq!(prefs.recent_paths, vec![second, first]);
        assert!(prefs.general.recursive_import);
        assert_eq!(PreferencesStore::open(dir.clone()).get().unwrap(), prefs);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_file(dir.join("preferences.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    fn aside_files(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| file_name(path).starts_with("preferences.json.bad-"))
            .collect()
    }
    #[test]
    fn corrupt_files_are_set_aside_and_failed_writes_preserve_previous_bytes() {
        let dir = fixture();
        let path = dir.join("preferences.json");
        fs::write(&path, b"{broken").unwrap();
        let store = PreferencesStore::open(dir.clone());
        let opened = store.get().unwrap();
        assert!(
            opened
                .notice
                .as_deref()
                .unwrap()
                .contains("renamed to preferences.json.bad-")
        );
        assert_eq!(opened.general, GeneralPreferences::default());
        let aside = aside_files(&dir);
        assert_eq!(aside.len(), 1);
        assert_eq!(fs::read(&aside[0]).unwrap(), b"{broken");
        assert!(!path.exists());
        let saved = store.remember(vec![dir.display().to_string()]).unwrap();
        assert!(saved.notice.is_none());
        let reopened = PreferencesStore::open(dir.clone()).get().unwrap();
        assert_eq!(reopened.recent_paths, vec![dir.display().to_string()]);
        assert!(reopened.notice.is_none());
        assert!(!fs::read_to_string(&path).unwrap().contains("notice"));
        fs::remove_file(&aside[0]).unwrap();
        fs::remove_file(&path).unwrap();
        let store = PreferencesStore::open(dir.clone());
        fs::create_dir(&path).unwrap();
        assert!(store.remember(vec![dir.display().to_string()]).is_err());
        assert!(store.get().unwrap().recent_paths.is_empty());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir(&path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn import_is_read_only_whitelisted_and_does_not_restore_media_drafts() {
        let dir = fixture();
        let store = PreferencesStore::open(dir.clone());
        let path = dir.join("import.json");
        let source = serde_json::json!({"DefaultOutputDir":dir.display().to_string(),"RecentFiles":serde_json::to_string(&vec![dir.join("offlinedrive.mkv").display().to_string()]).unwrap(),"EncEncoderArgs":"--unsafe stale args","Av1anEncoderArgs":"--other stale args","MainTab":"3","ResetSettingsList":"Video=false"});
        let bytes = serde_json::to_vec(&source).unwrap();
        fs::write(&path, &bytes).unwrap();
        let preview = store.preview_import(path.clone()).unwrap();
        assert_eq!(preview.accepted_keys.len(), 2);
        assert_eq!(preview.ignored_key_count, 4);
        assert_eq!(store.get().unwrap(), UserPreferences::default());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!dir.join("preferences.json").exists());
        store.save(preview.request).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_file(path).unwrap();
        fs::remove_file(dir.join("preferences.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn rejects_relative_controls_oversized_files_and_bounds_recents() {
        let dir = fixture();
        let store = PreferencesStore::open(dir.clone());
        assert!(store.remember(vec!["relative".into()]).is_err());
        assert!(
            store
                .remember(vec![format!("{}\n", dir.display())])
                .is_err()
        );
        let paths = (0..40)
            .map(|n| dir.join(format!("{n}.mkv")).display().to_string())
            .collect();
        assert_eq!(store.remember(paths).unwrap().recent_paths.len(), 15);
        let path = dir.join("large.json");
        fs::write(&path, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(store.preview_import(path.clone()).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_file(dir.join("preferences.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn recent_aliases_move_to_front_and_stay_deduplicated_after_reopen() {
        let dir = fixture();
        let store = PreferencesStore::open(dir.clone());
        let original = r"C:\Media\日本語.mkv";
        let extended = r"\\?\c:\media\日本語.mkv";
        let unc = r"\\Server\Share\Series\";
        let extended_unc = r"\\?\UNC\server\share\series";
        store.remember(vec![original.into(), unc.into()]).unwrap();
        let updated = store
            .remember(vec![
                extended_unc.into(),
                extended.into(),
                "C:/MEDIA/日本語.mkv".into(),
            ])
            .unwrap();
        assert_eq!(updated.recent_paths, vec![extended_unc, extended]);
        assert_eq!(PreferencesStore::open(dir.clone()).get().unwrap(), updated);
        // Missing or offline paths remain stored without filesystem lookups.
        assert_eq!(
            store.remember(vec![original.into()]).unwrap().recent_paths,
            vec![original, extended_unc]
        );
        fs::remove_file(dir.join("preferences.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    fn preset(name: &str) -> EncoderParameterPreset {
        EncoderParameterPreset {
            name: name.into(),
            encoder: media_core::VideoEncoder::X264,
            backend: media_core::EncodeBackend::Standalone,
            parameters: vec![media_core::EncoderParameter {
                name: "ref".into(),
                value: "3".into(),
            }],
        }
    }
    #[test]
    fn concurrent_presets_general_and_recent_edits_preserve_each_other_and_storage_isolation() {
        let dir = fixture();
        let other = fixture();
        let store = std::sync::Arc::new(PreferencesStore::open(dir.clone()));
        let gate = std::sync::Arc::new(std::sync::Barrier::new(3));
        std::thread::scope(|scope| {
            for mode in 0..3 {
                let store = store.clone();
                let gate = gate.clone();
                let dir = dir.clone();
                scope.spawn(move || {
                    gate.wait();
                    for _ in 0..10 {
                        match mode {
                            0 => {
                                store.save_parameter_preset(preset("Animation")).unwrap();
                            }
                            1 => {
                                store
                                    .save(SavePreferencesRequest {
                                        general: GeneralPreferences {
                                            default_output_directory: dir.display().to_string(),
                                            recursive_import: true,
                                            ..GeneralPreferences::default()
                                        },
                                        recent_paths: None,
                                    })
                                    .unwrap();
                            }
                            _ => {
                                store
                                    .remember(vec![dir.join("source.mkv").display().to_string()])
                                    .unwrap();
                            }
                        }
                    }
                });
            }
        });
        let value = PreferencesStore::open(dir.clone()).get().unwrap();
        assert_eq!(value.parameter_presets, vec![preset("Animation")]);
        assert!(value.general.recursive_import);
        assert_eq!(
            value.recent_paths,
            vec![dir.join("source.mkv").display().to_string()]
        );
        assert!(
            PreferencesStore::open(other.clone())
                .parameter_presets()
                .unwrap()
                .is_empty()
        );
        store
            .remove_parameter_preset(EncoderParameterPresetKey {
                name: "Animation".into(),
                encoder: media_core::VideoEncoder::X264,
                backend: media_core::EncodeBackend::Standalone,
            })
            .unwrap();
        let removed = store.get().unwrap();
        assert!(removed.parameter_presets.is_empty());
        assert_eq!(removed.general, value.general);
        assert_eq!(removed.recent_paths, value.recent_paths);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_file(dir.join("preferences.json")).unwrap();
        fs::remove_dir(dir).unwrap();
        fs::remove_dir(other).unwrap();
    }
    #[test]
    fn legacy_preferences_default_presets_and_invalid_or_corrupt_presets_are_preserved() {
        let dir = fixture();
        let path = dir.join("preferences.json");
        fs::write(&path, br#"{"version":1,"preferences":{"general":{"defaultOutputDirectory":"","recursiveImport":false},"recentPaths":[],"revision":3}}"#).unwrap();
        let store = PreferencesStore::open(dir.clone());
        assert!(store.parameter_presets().unwrap().is_empty());
        store.save_parameter_preset(preset("Valid")).unwrap();
        let original = fs::read(&path).unwrap();
        let mut invalid = preset("Invalid");
        invalid.parameters[0].name = "output".into();
        assert!(store.save_parameter_preset(invalid.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        let mut bad: serde_json::Value = serde_json::from_slice(&original).unwrap();
        bad["preferences"]["parameterPresets"] = serde_json::json!([invalid, preset("Valid")]);
        bad["preferences"]["recentPaths"] = serde_json::json!(["relative.mkv"]);
        let corrupt = serde_json::to_vec(&bad).unwrap();
        fs::write(&path, &corrupt).unwrap();
        // The usable parts survive; the original bytes are kept beside it.
        let reopened = PreferencesStore::open(dir.clone());
        let value = reopened.get().unwrap();
        assert_eq!(value.parameter_presets, vec![preset("Valid")]);
        assert!(value.recent_paths.is_empty());
        let notice = value.notice.unwrap();
        assert!(notice.contains("1 saved parameter preset(s)"), "{notice}");
        assert!(notice.contains("1 invalid recent path(s)"), "{notice}");
        let aside = aside_files(&dir);
        assert_eq!(aside.len(), 1);
        assert_eq!(fs::read(&aside[0]).unwrap(), corrupt);
        assert_eq!(fs::read(&path).unwrap(), corrupt);
        reopened
            .save_parameter_preset(preset("Replacement"))
            .unwrap();
        let saved = PreferencesStore::open(dir.clone()).get().unwrap();
        assert_eq!(
            saved.parameter_presets,
            vec![preset("Valid"), preset("Replacement")]
        );
        assert!(saved.notice.is_none());
        fs::remove_file(&aside[0]).unwrap();
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn newer_files_are_left_untouched_and_saving_is_refused() {
        let dir = fixture();
        let path = dir.join("preferences.json");
        let newer = br#"{"version":2,"preferences":{"future":true}}"#;
        fs::write(&path, newer).unwrap();
        let store = PreferencesStore::open(dir.clone());
        let opened = store.get().unwrap();
        assert!(opened.notice.as_deref().unwrap().contains("format 2"));
        assert_eq!(opened.general, GeneralPreferences::default());
        assert_eq!(
            store
                .remember(vec![dir.display().to_string()])
                .unwrap_err()
                .code,
            "PREFERENCES_READ_ONLY"
        );
        assert!(store.save_parameter_preset(preset("Any")).is_err());
        assert_eq!(fs::read(&path).unwrap(), newer);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
