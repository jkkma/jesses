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
    current: Mutex<Result<UserPreferences, AppError>>,
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
    Ok(())
}
fn path_key(path: &str) -> String {
    #[cfg(windows)]
    {
        path.replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
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
fn read_bounded(path: &Path) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(error)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(error("the file exceeds 256 KiB"));
    }
    Ok(bytes)
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
        let current = (|| {
            if let Err(e) = fs::metadata(&path) {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return Ok(UserPreferences::default());
                }
                return Err(error(e));
            }
            let mut record: Record =
                serde_json::from_slice(&read_bounded(&path)?).map_err(error)?;
            if record.version != 1 {
                return Err(error("unsupported preference version"));
            }
            validate_general(&record.preferences.general)?;
            validate_presets(&record.preferences.parameter_presets)?;
            record.preferences.recent_paths = normalize(record.preferences.recent_paths)?;
            Ok(record.preferences)
        })();
        Self {
            path,
            current: Mutex::new(current),
        }
    }
    pub fn get(&self) -> Result<UserPreferences, AppError> {
        self.current.lock().map_err(error)?.clone()
    }
    fn change(
        &self,
        edit: impl FnOnce(&mut UserPreferences) -> Result<(), AppError>,
    ) -> Result<UserPreferences, AppError> {
        let mut current = self.current.lock().map_err(error)?;
        let mut next = current.clone()?;
        edit(&mut next)?;
        if current.as_ref().is_ok_and(|value| value == &next) {
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
        *current = Ok(next.clone());
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
    #[test]
    fn corrupt_unknown_and_failed_writes_preserve_previous_bytes() {
        let dir = fixture();
        let path = dir.join("preferences.json");
        fs::write(&path, b"{broken").unwrap();
        let store = PreferencesStore::open(dir.clone());
        assert!(store.get().is_err());
        assert!(store.remember(vec![dir.display().to_string()]).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{broken");
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
        bad["preferences"]["parameterPresets"] = serde_json::json!([invalid]);
        let corrupt = serde_json::to_vec(&bad).unwrap();
        fs::write(&path, &corrupt).unwrap();
        let reopened = PreferencesStore::open(dir.clone());
        assert!(reopened.parameter_presets().is_err());
        assert!(
            reopened
                .save_parameter_preset(preset("Replacement"))
                .is_err()
        );
        assert!(
            reopened
                .remove_parameter_preset(EncoderParameterPresetKey {
                    name: "Invalid".into(),
                    encoder: media_core::VideoEncoder::X264,
                    backend: media_core::EncodeBackend::Standalone
                })
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), corrupt);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
