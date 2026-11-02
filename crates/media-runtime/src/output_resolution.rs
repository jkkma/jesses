//! Read-only source-style output stems. Actual writers retain no-clobber guards.
use media_core::{AppError, ImageOutput, OutputPathRequest, OutputPathResolution, OutputTarget};
use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

fn invalid(message: impl Into<String>, path: &Path) -> AppError {
    AppError::new(
        "OUTPUT_PATH_INVALID",
        message,
        Some(path.to_string_lossy().into_owned()),
    )
}
fn occupied(path: &Path) -> Result<bool, AppError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(invalid(
            format!("Cannot inspect the destination: {error}"),
            path,
        )),
    }
}
fn unnumbered(name: &str, directory: bool) -> &str {
    let Some(open) = name.rfind('(') else {
        return name;
    };
    let Some(digits) = name[open + 1..].strip_suffix(')') else {
        return name;
    };
    if digits.is_empty()
        || (!directory && digits.len() > 4)
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return name;
    }
    let stem = name[..open].trim_end();
    if stem.is_empty() { name } else { stem }
}
pub(crate) fn preferred_directory(
    identity: &Path,
    configured: Option<&str>,
) -> Result<PathBuf, AppError> {
    let parent = identity
        .parent()
        .ok_or_else(|| invalid("Source has no containing folder.", identity))?;
    let default = configured
        .map(|value| value.trim().trim_matches('"'))
        .filter(|value| !value.is_empty())
        .map(Path::new)
        .and_then(|path| fs::canonicalize(path).ok())
        .filter(|path| path.is_dir());
    Ok(default.unwrap_or_else(|| parent.to_owned()))
}

pub(crate) fn resolve_reserved(
    request: OutputPathRequest,
    reserved: &mut HashSet<String>,
) -> Result<OutputPathResolution, AppError> {
    let source = crate::jobs::files::Source::open(Path::new(&request.input_path))?;
    let original = crate::jobs::sequence::original_directory(&source.path)?;
    let identity = original.as_deref().unwrap_or(&source.path);
    let name = if original.is_some() {
        identity.file_name()
    } else {
        identity.file_stem()
    }
    .filter(|name| !name.is_empty())
    .ok_or_else(|| invalid("Source has no output name.", identity))?;
    let preferred = match request.output_stem.as_deref() {
        Some(value) => PathBuf::from(value.trim()),
        None => {
            preferred_directory(identity, request.default_output_directory.as_deref())?.join(name)
        }
    };
    if !preferred.is_absolute() || preferred.to_str().is_none_or(|value| value.contains('\0')) {
        return Err(invalid("Choose an absolute local output stem.", &preferred));
    }
    let directory = preferred
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .filter(|parent| parent.is_dir())
        .ok_or_else(|| invalid("Choose an existing output folder.", &preferred))?;
    let name = preferred
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid("Choose a Unicode output name.", &preferred))?;
    #[cfg(windows)]
    if name
        .chars()
        .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        || name.ends_with(['.', ' '])
    {
        return Err(invalid(
            "The output name contains a character Windows cannot use.",
            &preferred,
        ));
    }
    let (extension, is_directory) = match request.target {
        OutputTarget::Container { format } => (Some(format.extension()), false),
        OutputTarget::Image {
            format: ImageOutput::Gif,
        } => (Some("gif"), false),
        OutputTarget::Image {
            format: ImageOutput::Png,
        } => (Some("png"), false),
        OutputTarget::Image {
            format: ImageOutput::Jpeg,
        } => (Some("jpg"), false),
        OutputTarget::Image {
            format: ImageOutput::PngSequence | ImageOutput::JpegSequence,
        } => (None, true),
    };
    let path_for = |stem: &str| {
        let mut filename = OsString::from(stem);
        if let Some(extension) = extension {
            filename.push(".");
            filename.push(extension);
        }
        directory.join(filename)
    };
    let mut stem = name.to_owned();
    let available = |stem: &str| -> Result<bool, AppError> {
        let path = path_for(stem);
        Ok(!reserved.contains(&crate::batch::path_key(&path)) && !occupied(&path)?)
    };
    let numbered = !available(&stem)?;
    if numbered {
        let base = unnumbered(name, is_directory);
        let mut found = None;
        for number in 1..=u32::MAX {
            let candidate = format!("{base} ({number})");
            if available(&candidate)? {
                found = Some(candidate);
                break;
            }
        }
        stem = found
            .ok_or_else(|| invalid("No available numbered destination remains.", &preferred))?;
    }
    source.verify()?;
    reserved.insert(crate::batch::path_key(&path_for(&stem)));
    Ok(OutputPathResolution {
        output_stem: directory.join(&stem).to_string_lossy().into_owned(),
        output_path: path_for(&stem).to_string_lossy().into_owned(),
        is_directory,
        numbered,
    })
}

pub async fn resolve_output_path(
    request: OutputPathRequest,
) -> Result<OutputPathResolution, AppError> {
    tokio::task::spawn_blocking(move || resolve_reserved(request, &mut HashSet::new()))
        .await
        .map_err(|error| AppError::new("OUTPUT_PATH_INVALID", error.to_string(), None))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_core::ContainerFormat;
    #[tokio::test]
    async fn literal_dotted_stems_formats_defaults_and_occupied_entries_create_nothing() {
        let root = std::env::temp_dir().join(format!(
            "jesses-names-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("film.part.one.mkv");
        fs::write(&source, b"original").unwrap();
        let request = |stem: Option<String>, target| OutputPathRequest {
            input_path: source.to_string_lossy().into_owned(),
            output_stem: stem,
            default_output_directory: Some(root.join("missing").to_string_lossy().into_owned()),
            target,
        };
        let container = OutputTarget::Container {
            format: ContainerFormat::Matroska,
        };
        let result = resolve_output_path(request(None, container)).await.unwrap();
        assert_eq!(
            Path::new(&result.output_path),
            root.join("film.part.one (1).mkv")
        );
        fs::create_dir(&result.output_path).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("gone"), root.join("film.part.one (2).mkv")).unwrap();
        let next = resolve_output_path(request(
            Some(
                root.join("film.part.one (1)")
                    .to_string_lossy()
                    .into_owned(),
            ),
            container,
        ))
        .await
        .unwrap();
        assert!(next.output_path.ends_with(if cfg!(unix) {
            "film.part.one (3).mkv"
        } else {
            "film.part.one (2).mkv"
        }));
        assert!(!Path::new(&next.output_path).exists());
        let literal = resolve_output_path(request(
            Some(format!("  {}  ", root.join("manual.mp4").display())),
            container,
        ))
        .await
        .unwrap();
        assert!(literal.output_path.ends_with("manual.mp4.mkv"));
        let gif = resolve_output_path(request(
            None,
            OutputTarget::Image {
                format: ImageOutput::Gif,
            },
        ))
        .await
        .unwrap();
        assert!(gif.output_path.ends_with("film.part.one.gif"));
        let images = resolve_output_path(request(
            None,
            OutputTarget::Image {
                format: ImageOutput::PngSequence,
            },
        ))
        .await
        .unwrap();
        assert!(images.is_directory);
        assert!(images.output_path.ends_with("film.part.one"));
        assert!(!Path::new(&images.output_path).exists());
        let configured = root.join("destination");
        fs::create_dir(&configured).unwrap();
        let mut configured_request = request(None, container);
        configured_request.default_output_directory =
            Some(format!(" \"{}\" ", configured.display()));
        assert_eq!(
            Path::new(
                &resolve_output_path(configured_request)
                    .await
                    .unwrap()
                    .output_path
            ),
            configured.join("film.part.one.mkv")
        );
        assert_eq!(fs::read(&source).unwrap(), b"original");
        fs::remove_dir_all(root).unwrap();
    }
}
