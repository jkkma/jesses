use std::{
    env,
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use media_core::ToolInfo;

use crate::process::run_tool;

#[derive(Clone, Copy)]
struct ToolSpec {
    id: &'static str,
    name: &'static str,
    executables: &'static [&'static str],
    version_arg: &'static str,
}

const TOOLS: [ToolSpec; 5] = [
    ToolSpec {
        id: "ffmpeg",
        name: "FFmpeg",
        executables: &["ffmpeg"],
        version_arg: "-version",
    },
    ToolSpec {
        id: "ffprobe",
        name: "FFprobe",
        executables: &["ffprobe"],
        version_arg: "-version",
    },
    ToolSpec {
        id: "svt-av1",
        name: "SVT-AV1",
        executables: &["SvtAv1EncApp", "svtav1encapp"],
        version_arg: "--version",
    },
    ToolSpec {
        id: "av1an",
        name: "av1an",
        executables: &["av1an"],
        version_arg: "--version",
    },
    ToolSpec {
        id: "x264",
        name: "x264",
        executables: &["x264"],
        version_arg: "--version",
    },
];

/// Only absolute PATH entries and native executable files are candidates. Empty
/// or relative PATH entries must never implicitly execute a file from the CWD.
/// Filesystem calls run outside async workers, with a caller-facing time limit.
/// A timeout cannot cancel an OS metadata call already running in the blocking
/// pool, but a disconnected PATH directory cannot hold up the UI response.
pub(crate) async fn find_executable(names: &[&str]) -> Result<Option<PathBuf>, String> {
    let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    let search = tokio::task::spawn_blocking(move || find_executable_blocking(&names));
    match tokio::time::timeout(Duration::from_secs(5), search).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(format!("Tool discovery could not finish: {error}")),
        Err(_) => Err("Tool discovery timed out while checking PATH. Check for inaccessible tool directories and try again.".into()),
    }
}

fn find_executable_blocking(names: &[String]) -> Result<Option<PathBuf>, String> {
    let Some(search_path) = env::var_os("PATH") else {
        return Ok(None);
    };
    for directory in env::split_paths(&search_path).filter(|directory| directory.is_absolute()) {
        for name in names {
            #[cfg(windows)]
            let filename = format!("{name}.exe");
            #[cfg(not(windows))]
            let filename = name;
            let candidate = directory.join(filename);
            if executable_file(&candidate) {
                return resolve_executable(&candidate).map(Some);
            }
        }
    }
    Ok(None)
}

/// Scoop's small launcher executables create a second process. Resolving its
/// transparent path-only sidecar keeps timeout/cancellation ownership on the
/// actual tool. Other shim behavior must not be silently discarded or executed.
fn resolve_executable(candidate: &Path) -> Result<PathBuf, String> {
    let sidecar = candidate.with_extension("shim");
    let file = match std::fs::File::open(&sidecar) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return candidate.canonicalize().map_err(|error| {
                format!(
                    "The executable {} could not be resolved: {error}",
                    candidate.display()
                )
            });
        }
        Err(error) => {
            return Err(format!(
                "The executable shim {} could not be read: {error}",
                sidecar.display()
            ));
        }
    };
    let unsupported = || {
        format!(
            "Unsupported executable shim {}. Only a quoted, absolute path-only shim is supported; add the actual tool directory to PATH. Arguments, environment changes, and elevation are not supported.",
            sidecar.display(),
        )
    };
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            format!(
                "The executable shim {} could not be read: {error}",
                sidecar.display()
            )
        })?;
    if bytes.len() > 16 * 1024 {
        return Err(unsupported());
    }
    let contents = std::str::from_utf8(&bytes).map_err(|_| unsupported())?;
    let mut lines = contents
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let line = lines.next().ok_or_else(&unsupported)?;
    if lines.next().is_some() {
        return Err(unsupported());
    }
    let target = line
        .strip_prefix("path")
        .and_then(|value| value.trim_start().strip_prefix('='))
        .map(str::trim)
        .and_then(|value| value.strip_prefix('"'))
        .and_then(|value| value.strip_suffix('"'))
        .filter(|value| !value.is_empty() && !value.contains(['"', '\0']))
        .ok_or_else(&unsupported)?;
    let target = Path::new(target);
    if !target.is_absolute() || !executable_file(target) {
        return Err(unsupported());
    }
    #[cfg(windows)]
    if !target
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err(unsupported());
    }
    let target = target.canonicalize().map_err(|_| unsupported())?;
    if target
        .with_extension("shim")
        .try_exists()
        .map_err(|_| unsupported())?
    {
        return Err(unsupported());
    }
    Ok(target)
}

fn executable_file(path: &std::path::Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

async fn discover(spec: ToolSpec) -> ToolInfo {
    let mut info = ToolInfo {
        id: spec.id.into(),
        name: spec.name.into(),
        available: false,
        path: None,
        version: None,
        detail: None,
    };
    let executable = match find_executable(spec.executables).await {
        Ok(Some(executable)) => executable,
        Ok(None) => {
            info.detail = Some("Not found on PATH. Install the tool and restart jesses.".into());
            return info;
        }
        Err(detail) => {
            info.detail = Some(detail);
            return info;
        }
    };
    info.path = Some(executable.to_string_lossy().into_owned());
    match run_tool(
        &executable,
        &[OsString::from(spec.version_arg)],
        Duration::from_secs(5),
        64 * 1024,
    )
    .await
    {
        Ok(output) if output.status.success() => {
            info.available = true;
            let combined = if output.stdout.is_empty() {
                &output.stderr
            } else {
                &output.stdout
            };
            info.version = String::from_utf8_lossy(combined)
                .lines()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().chars().take(200).collect());
        }
        Ok(output) => {
            info.detail = Some(format!("Version check failed ({}).", output.status));
        }
        Err(error) => {
            info.detail = Some(error.to_string());
        }
    }
    info
}

/// Checks all known tools concurrently; missing optional tools remain
/// explicit capability results and do not prevent media inspection.
pub async fn get_capabilities() -> Vec<ToolInfo> {
    let (ffmpeg, ffprobe, svt, av1an, x264) = tokio::join!(
        discover(TOOLS[0]),
        discover(TOOLS[1]),
        discover(TOOLS[2]),
        discover(TOOLS[3]),
        discover(TOOLS[4]),
    );
    vec![ffmpeg, ffprobe, svt, av1an, x264]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_path_only_shims_and_rejects_behavior_changing_shims() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            env::temp_dir().join(format!("jesses-shim-test-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        let shim = directory.join("ffprobe.exe");
        let actual = directory.join("actual-ffprobe.exe");
        std::fs::write(&shim, b"test placeholder").unwrap();
        std::fs::write(&actual, b"test placeholder").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&actual, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let contents = format!("path = \"{}\"\r\n", actual.display());
        std::fs::write(shim.with_extension("shim"), &contents).unwrap();
        assert_eq!(
            resolve_executable(&shim).unwrap(),
            actual.canonicalize().unwrap()
        );

        for extra in ["args = --unsafe", "env = VALUE=changed", "elevate = true"] {
            std::fs::write(
                shim.with_extension("shim"),
                format!("{contents}{extra}\r\n"),
            )
            .unwrap();
            let error = resolve_executable(&shim).unwrap_err();
            assert!(error.contains("path-only shim"), "{error}");
            assert!(error.contains("ffprobe.shim"), "{error}");
        }
        std::fs::write(shim.with_extension("shim"), "path = \"relative-tool.exe\"").unwrap();
        assert!(resolve_executable(&shim).is_err());
    }
}
