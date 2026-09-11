use std::{
    env,
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use media_core::{ToolInfo, VideoEncoder};

use crate::process::run_tool;

#[derive(Clone, Copy)]
struct ToolSpec {
    id: &'static str,
    name: &'static str,
    executables: &'static [&'static str],
    version_arg: &'static str,
}

const TOOLS: [ToolSpec; 7] = [
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
    ToolSpec {
        id: "svt-av1-5fish",
        name: "SVT-AV1 5fish",
        executables: &["SvtAv1EncApp-5fish"],
        version_arg: "--version",
    },
    ToolSpec {
        id: "svt-av1-hdr",
        name: "SVT-AV1-HDR",
        executables: &["SvtAv1EncApp-HDR"],
        version_arg: "--version",
    },
];

fn tool_encoder(id: &str) -> Option<VideoEncoder> {
    match id {
        "svt-av1" => Some(VideoEncoder::SvtAv1),
        "svt-av1-5fish" => Some(VideoEncoder::SvtAv1FiveFish),
        "svt-av1-hdr" => Some(VideoEncoder::SvtAv1Hdr),
        "x264" => Some(VideoEncoder::X264),
        _ => None,
    }
}

fn fork_config(encoder: VideoEncoder) -> Option<(&'static str, &'static str, &'static str)> {
    match encoder {
        VideoEncoder::SvtAv1FiveFish => Some((
            "JESSES_SVT_AV1_5FISH",
            "svt-av1-5fish",
            "SvtAv1EncApp-5fish",
        )),
        VideoEncoder::SvtAv1Hdr => Some(("JESSES_SVT_AV1_HDR", "svt-av1-hdr", "SvtAv1EncApp-HDR")),
        _ => None,
    }
}

/// The managed location is per user; each fork retains its upstream filename in
/// a separate directory so av1an can receive a job-specific PATH.
pub(crate) fn managed_encoder_path(encoder: VideoEncoder) -> Result<Option<PathBuf>, String> {
    let Some((_, directory, _)) = fork_config(encoder) else {
        return Ok(None);
    };
    #[cfg(windows)]
    let base = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base =
        env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = env::var_os("XDG_DATA_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let Some(base) = base else { return Ok(None) };
    if !base.is_absolute() {
        return Err("The user data directory for managed encoders must be absolute.".into());
    }
    Ok(Some(base.join("jesses/tools").join(directory).join(
        if cfg!(windows) {
            "SvtAv1EncApp.exe"
        } else {
            "SvtAv1EncApp"
        },
    )))
}

/// Resolve only the requested implementation. A bad explicit override or managed
/// installation is an error, never permission to run a different PATH encoder.
pub(crate) async fn find_video_encoder(encoder: VideoEncoder) -> Result<Option<PathBuf>, String> {
    let search = tokio::task::spawn_blocking(move || {
        if let Some((variable, _, alias)) = fork_config(encoder) {
            let override_path = env::var_os(variable).map(PathBuf::from);
            resolve_fork_location(
                override_path.as_deref(),
                || managed_encoder_path(encoder),
                || find_executable_blocking(&[alias.to_owned()]),
            )
            .map_err(|error| {
                if override_path.is_some() {
                    format!("{variable}: {error}")
                } else {
                    error
                }
            })
        } else {
            let names = match encoder {
                VideoEncoder::SvtAv1 => ["SvtAv1EncApp", "svtav1encapp"].as_slice(),
                VideoEncoder::X264 => ["x264"].as_slice(),
                _ => unreachable!("forks handled above"),
            };
            find_executable_blocking(
                &names
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect::<Vec<_>>(),
            )
        }
    });
    match tokio::time::timeout(Duration::from_secs(5), search).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(format!("Encoder discovery could not finish: {error}")),
        Err(_) => {
            Err("Encoder discovery timed out while checking its configured locations.".into())
        }
    }
}

fn resolve_fork_location(
    explicit: Option<&Path>,
    managed: impl FnOnce() -> Result<Option<PathBuf>, String>,
    alias: impl FnOnce() -> Result<Option<PathBuf>, String>,
) -> Result<Option<PathBuf>, String> {
    if let Some(path) = explicit {
        return resolve_video_candidate(path).map(Some);
    }
    if let Some(path) = managed()? {
        match path.try_exists() {
            Ok(true) => return resolve_video_candidate(&path).map(Some),
            Ok(false) => {}
            Err(error) => {
                return Err(format!(
                    "Cannot inspect managed encoder {}: {error}",
                    path.display()
                ));
            }
        }
    }
    alias()
}

fn resolve_video_candidate(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || !executable_file(path) {
        return Err(format!(
            "The encoder must be an existing absolute executable file: {}",
            path.display()
        ));
    }
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err("Encoder overrides must point to a native .exe file.".into());
    }
    resolve_executable(path)
}

/// Check the process response at both capability discovery and job startup.
/// A familiar filename alone cannot identify an SVT implementation.
pub(crate) fn validate_video_encoder_version(
    encoder: VideoEncoder,
    output: &str,
) -> Result<(), String> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| {
            line.to_ascii_lowercase()
                .starts_with(if encoder == VideoEncoder::X264 {
                    "x264 "
                } else {
                    "svt-av1"
                })
        })
        .ok_or_else(|| {
            "The executable did not report the requested encoder identity.".to_owned()
        })?;
    let lower = line.to_ascii_lowercase();
    let five_fish = lower.contains("[5fish]") || lower.contains("svt-av1-5fish");
    let hdr = lower.starts_with("svt-av1-hdr ") || lower.contains("[hdr]");
    let known_fork = five_fish
        || hdr
        || ["psy", "essential"]
            .iter()
            .any(|marker| lower.contains(marker));
    let matches = match encoder {
        VideoEncoder::SvtAv1 => !known_fork,
        VideoEncoder::SvtAv1FiveFish => five_fish && !hdr,
        VideoEncoder::SvtAv1Hdr => hdr && !five_fish,
        VideoEncoder::X264 => lower.starts_with("x264 "),
    };
    if matches {
        Ok(())
    } else {
        Err(format!(
            "Encoder identity mismatch: requested {}, but the executable reports {line}.",
            encoder.name()
        ))
    }
}

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
    let encoder = tool_encoder(spec.id);
    let found = if let Some(encoder) = encoder {
        find_video_encoder(encoder).await
    } else {
        find_executable(spec.executables).await
    };
    let executable = match found {
        Ok(Some(executable)) => executable,
        Ok(None) => {
            info.detail = Some(
                if encoder.is_some_and(|encoder| fork_config(encoder).is_some()) {
                    "Not found in its configured override, managed install, or distinct PATH alias. Install this fork and refresh Tools.".into()
                } else {
                    "Not found on PATH. Install the tool and restart jesses.".into()
                },
            );
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
            if let Some(encoder) = encoder {
                let combined = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                if let Err(detail) = validate_video_encoder_version(encoder, &combined) {
                    info.detail = Some(detail);
                    return info;
                }
            }
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
    let (ffmpeg, ffprobe, svt, av1an, x264, five_fish, hdr) = tokio::join!(
        discover(TOOLS[0]),
        discover(TOOLS[1]),
        discover(TOOLS[2]),
        discover(TOOLS[3]),
        discover(TOOLS[4]),
        discover(TOOLS[5]),
        discover(TOOLS[6]),
    );
    vec![ffmpeg, ffprobe, svt, av1an, x264, five_fish, hdr]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_distinguish_each_implementation_and_reject_other_known_forks() {
        let identities = [
            (VideoEncoder::SvtAv1, "SVT-AV1 Encoder Lib v4.1.0"),
            (
                VideoEncoder::SvtAv1FiveFish,
                "SVT-AV1 [5fish] v2.3.260827 (release)",
            ),
            (
                VideoEncoder::SvtAv1Hdr,
                "SVT-AV1-HDR v4.1.0-21-g00333404f (release)\nHDR Release: 2026-09-01",
            ),
            (VideoEncoder::X264, "x264 0.165.3222M b35605a"),
        ];
        for (requested, _) in identities {
            for (actual, version) in identities {
                assert_eq!(
                    validate_video_encoder_version(requested, version).is_ok(),
                    requested == actual
                );
            }
            assert!(validate_video_encoder_version(requested, "unrelated tool 1.0").is_err());
        }
        for fork in [
            "SVT-AV1-PSY v3.0",
            "SVT-AV1-PSYEX v3.0",
            "SVT-AV1-ESSENTIAL v4.0",
        ] {
            assert!(validate_video_encoder_version(VideoEncoder::SvtAv1, fork).is_err());
        }
    }

    #[test]
    fn fork_location_precedence_cannot_fall_back_after_a_bad_explicit_selection() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "jesses-fork-location-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        let explicit = directory.join("explicit.exe");
        let managed = directory.join("managed.exe");
        for path in [&explicit, &managed] {
            std::fs::write(path, b"test fixture").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let no_fallback = || -> Result<Option<PathBuf>, String> { panic!("unexpected fallback") };
        assert_eq!(
            resolve_fork_location(Some(&explicit), no_fallback, no_fallback).unwrap(),
            Some(explicit.canonicalize().unwrap())
        );
        assert!(
            resolve_fork_location(
                Some(&directory.join("missing.exe")),
                no_fallback,
                no_fallback
            )
            .is_err()
        );
        assert!(
            resolve_fork_location(Some(Path::new("relative.exe")), no_fallback, no_fallback)
                .is_err()
        );
        assert_eq!(
            resolve_fork_location(None, || Ok(Some(managed.clone())), no_fallback).unwrap(),
            Some(managed.canonicalize().unwrap())
        );
        assert!(resolve_fork_location(None, || Ok(Some(directory.clone())), no_fallback).is_err());
        assert_eq!(
            resolve_fork_location(
                None,
                || Ok(Some(directory.join("missing.exe"))),
                || Ok(Some(explicit.clone()))
            )
            .unwrap(),
            Some(explicit)
        );
    }

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
