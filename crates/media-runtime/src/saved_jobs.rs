//! Foreign resume files are data, never executable arguments or cleanup authority.
use crate::jobs::files::Source;
use media_core::{AppError, EncodeRequest, SavedJobInspection};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedRequest {
    format: String,
    version: u32,
    request: EncodeRequest,
}
pub fn inspect_saved_job(path: String) -> Result<SavedJobInspection, AppError> {
    let source = Source::open(Path::new(&path))?;
    let mut bytes = Vec::new();
    std::fs::File::open(&source.path)
        .and_then(|f| f.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|e| AppError::new("SAVED_JOB_UNREADABLE", e.to_string(), Some(path.clone())))?;
    source.verify()?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(AppError::new(
            "SAVED_JOB_TOO_LARGE",
            "Saved job files must be at most 4 MiB.",
            Some(path),
        ));
    }
    let parsed = serde_json::from_slice::<SavedRequest>(&bytes)
        .ok()
        .filter(|s| s.format == "jesses-encode-request" && s.version == 1);
    Ok(match parsed {
        Some(saved)=>SavedJobInspection {path,compatible:true,message:"This file contains a saved encode request. It can start a new job after source, tools and destination are checked. Recovery is available only through this installation's verified job history.".into(),request:Some(saved.request)},
        None=>SavedJobInspection {path,compatible:false,message:"This saved job cannot be resumed here. Continue it in the application that created it. Its media, arguments and recovery files were left unchanged.".into(),request:None},
    })
}
pub fn export_saved_job(path: String, request: EncodeRequest) -> Result<String, AppError> {
    let p = Path::new(&path);
    if !p.is_absolute()
        || !p
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.eq_ignore_ascii_case("json"))
    {
        return Err(AppError::new(
            "INVALID_OUTPUT",
            "Choose a new .json file with an absolute local path.",
            Some(path),
        ));
    }
    let data = serde_json::to_vec_pretty(&SavedRequest {
        format: "jesses-encode-request".into(),
        version: 1,
        request,
    })
    .map_err(|e| AppError::new("SAVED_JOB_INVALID", e.to_string(), None))?;
    if data.len() > 4 * 1024 * 1024 {
        return Err(AppError::new(
            "SAVED_JOB_TOO_LARGE",
            "The encode request is too large to export.",
            Some(path),
        ));
    }
    use std::io::Write;
    let parent = p
        .parent()
        .and_then(|v| std::fs::canonicalize(v).ok())
        .filter(|v| v.is_dir())
        .ok_or_else(|| {
            AppError::new(
                "INVALID_OUTPUT",
                "Choose an existing parent folder.",
                Some(path.clone()),
            )
        })?;
    let output = parent.join(p.file_name().ok_or_else(|| {
        AppError::new("INVALID_OUTPUT", "Choose a filename.", Some(path.clone()))
    })?);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = crate::jobs::files::Temporary::create_extension(
        &output,
        &format!("saved-job-{}-{stamp}", std::process::id()),
        "json",
    )?;
    let mut file = temporary.clone_file()?;
    file.write_all(&data)
        .and_then(|_| file.sync_all())
        .map_err(|e| AppError::new("SAVED_JOB_WRITE_FAILED", e.to_string(), Some(path.clone())))?;
    drop(file);
    temporary.publish(&output)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_requests_round_trip_and_foreign_commands_are_never_imported() {
        let root = std::env::temp_dir().join(format!(
            "jesses-saved-request-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("request.json").to_string_lossy().into_owned();
        let request = EncodeRequest {
            source: media_core::RemuxRequest {
                input_path: "source.mkv".into(),
                output_path: "new.mkv".into(),
                stream_indices: vec![0],
            },
            settings: Default::default(),
        };
        export_saved_job(file.clone(), request.clone()).unwrap();
        let saved = std::fs::read(&file).unwrap();
        let inspected = inspect_saved_job(file.clone()).unwrap();
        assert!(inspected.compatible);
        assert_eq!(inspected.request, Some(request.clone()));
        assert!(export_saved_job(file.clone(), request).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), saved);
        let foreign = root.join("foreign.json");
        let bytes=br#"{"command":"do not execute","args":["output.mkv"],"input":"trimmed-intermediate.mkv"}"#;
        std::fs::write(&foreign, bytes).unwrap();
        let inspected = inspect_saved_job(foreign.to_string_lossy().into_owned()).unwrap();
        assert!(!inspected.compatible);
        assert!(inspected.request.is_none());
        assert_eq!(std::fs::read(&foreign).unwrap(), bytes);
        std::fs::remove_dir_all(root).unwrap();
    }
}
