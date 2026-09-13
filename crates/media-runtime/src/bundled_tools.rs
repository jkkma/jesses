//! Verified package resources. Bundled corruption is reported instead of silently
//! selecting a different PATH executable. Development builds may have no bundle.
use std::{
    collections::HashSet,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

static RESOURCE_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
const MANIFEST_LIMIT: u64 = 1024 * 1024;
const TOOL_LIMIT: u64 = 512 * 1024 * 1024;

/// Set the package resource root once during desktop startup. This does not
/// modify PATH, per-user tool installs, or any application data.
pub fn configure_bundled_tools(resource_dir: PathBuf) -> Result<(), String> {
    if !resource_dir.is_absolute() {
        return Err("The bundled resource directory must be absolute.".into());
    }
    if let Some(previous) = RESOURCE_DIRECTORY.get() {
        return if previous == &resource_dir {
            Ok(())
        } else {
            Err("The bundled resource directory was already configured.".into())
        };
    }
    RESOURCE_DIRECTORY
        .set(resource_dir)
        .map_err(|_| "The bundled resource directory was already configured.".into())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u8,
    target: String,
    tools: Vec<Tool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tool {
    id: String,
    path: String,
    sha256: String,
    #[serde(default)]
    support_files: Vec<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    path: String,
    sha256: String,
}

fn target() -> &'static str {
    #[cfg(windows)]
    return "x86_64-pc-windows-msvc";
    #[cfg(target_os = "linux")]
    return "x86_64-unknown-linux-gnu";
    #[cfg(not(any(windows, target_os = "linux")))]
    return "unsupported";
}

pub(crate) fn find(id: &str) -> Result<Option<PathBuf>, String> {
    let Some(resource_dir) = RESOURCE_DIRECTORY.get() else {
        return Ok(None);
    };
    let root = checked_resource_root(resource_dir)?;
    find_at(&root, id)
}

fn checked_resource_root(resource_dir: &Path) -> Result<PathBuf, String> {
    let root = resource_dir.join("resources/tools");
    if let Ok(actual) = root.canonicalize() {
        let resources = resource_dir
            .canonicalize()
            .map_err(|error| format!("Cannot resolve package resources: {error}"))?;
        if !actual.starts_with(resources) {
            return Err("Bundled tools cannot leave the package resource directory.".into());
        }
    }
    Ok(root)
}

fn find_at(root: &Path, id: &str) -> Result<Option<PathBuf>, String> {
    Ok(find_tool_at(root, id)?.map(|tool| tool.executable))
}

struct VerifiedTool {
    executable: PathBuf,
    support: HashSet<PathBuf>,
}

/// Portable frameserver paths only when the selected executable is this bundle.
/// An explicit executable outside the package keeps its external environment.
pub(crate) fn av1an_runtime(executable: &Path) -> Result<Option<PathBuf>, String> {
    let Some(resources) = RESOURCE_DIRECTORY.get() else {
        return Ok(None);
    };
    let runtime = av1an_runtime_at(&resources.join("resources/tools"), executable)?;
    if runtime.is_some() {
        checked_resource_root(resources)?;
    }
    Ok(runtime)
}

fn av1an_runtime_at(root: &Path, executable: &Path) -> Result<Option<PathBuf>, String> {
    #[cfg(windows)]
    let expected = root.join("av1an/av1an.exe");
    #[cfg(not(windows))]
    let expected = root.join("av1an/av1an");
    let Some(expected) = expected.canonicalize().ok() else {
        return Ok(None);
    };
    if executable.canonicalize().ok().as_ref() != Some(&expected) {
        return Ok(None);
    }
    let verified = find_tool_at(root, "av1an")?.ok_or("The bundled av1an record is missing.")?;
    if verified.executable != expected {
        return Err("The bundled av1an record changed its executable path.".into());
    }
    let directory = expected
        .parent()
        .expect("bundled executable parent")
        .join("python/Lib/site-packages/vapoursynth");
    #[cfg(windows)]
    let required = [
        directory.join("vsscript.dll"),
        directory.join("vspipe.exe"),
        expected.parent().unwrap().join("python/python.exe"),
    ];
    #[cfg(not(windows))]
    let required = [directory.join("libvsscript.so"), directory.join("vspipe")];
    for path in required {
        let path = path
            .canonicalize()
            .map_err(|error| format!("Bundled frameserver is missing: {error}"))?;
        if !verified.support.contains(&path) {
            return Err(
                "A bundled frameserver dependency is absent from its verified manifest.".into(),
            );
        }
    }
    let runtime_root = expected.parent().unwrap().join("python");
    let mut actual = HashSet::new();
    runtime_inventory(&runtime_root, &mut actual, 0, &mut 512)?;
    if actual != verified.support {
        return Err("The bundled frameserver inventory differs from its verified manifest.".into());
    }
    Ok(Some(directory))
}

pub(crate) fn frameserver_environment(
    directory: &Path,
    path: &std::ffi::OsStr,
) -> crate::supervisor::ChildEnvironment {
    #[cfg(windows)]
    let script = directory.join("vsscript.dll");
    #[cfg(not(windows))]
    let script = directory.join("libvsscript.so");
    let mut environment = crate::supervisor::ChildEnvironment::with_path(path);
    environment.variables = vec![
        ("VSSCRIPT_PATH", Some(script.into_os_string())),
        ("VAPOURSYNTH_EXTRA_PLUGIN_PATH", None),
        ("PYTHONHOME", None),
        ("PYTHONPATH", None),
        ("PYTHONSTARTUP", None),
    ];
    environment
}

fn runtime_inventory(
    directory: &Path,
    files: &mut HashSet<PathBuf>,
    depth: usize,
    budget: &mut usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err("The bundled frameserver nesting exceeds its limit.".into());
    }
    let directory_metadata = fs::symlink_metadata(directory)
        .map_err(|error| format!("Cannot inspect bundled frameserver: {error}"))?;
    if !directory_metadata.is_dir() || directory_metadata.file_type().is_symlink() {
        return Err("The bundled frameserver directory cannot be redirected.".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if directory_metadata.file_attributes() & 0x400 != 0 {
            return Err("The bundled frameserver directory cannot be redirected.".into());
        }
    }
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("Cannot inspect bundled frameserver: {error}"))?;
    for entry in entries {
        *budget = budget
            .checked_sub(1)
            .ok_or("The bundled frameserver exceeds its inventory limit.")?;
        let entry =
            entry.map_err(|error| format!("Cannot inspect bundled frameserver: {error}"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("Cannot inspect bundled frameserver: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("The bundled frameserver cannot contain redirections.".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("The bundled frameserver cannot contain redirections.".into());
            }
        }
        if metadata.is_dir() {
            runtime_inventory(&entry.path(), files, depth + 1, budget)?;
        } else if metadata.is_file() {
            files.insert(
                entry
                    .path()
                    .canonicalize()
                    .map_err(|error| format!("Cannot resolve bundled frameserver: {error}"))?,
            );
            if files.len() > 256 {
                return Err("The bundled frameserver exceeds its file limit.".into());
            }
        } else {
            return Err("The bundled frameserver contains an unsupported file.".into());
        }
    }
    Ok(())
}

fn find_tool_at(root: &Path, id: &str) -> Result<Option<VerifiedTool>, String> {
    let root_metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot inspect bundled tools directory: {error}")),
    };
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err("The bundled tools directory cannot be redirected.".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if root_metadata.file_attributes() & 0x400 != 0 {
            return Err("The bundled tools directory cannot be redirected.".into());
        }
    }
    let manifest_path = root.join("manifest.json");
    let manifest_metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|error| format!("The bundled tool manifest is missing or unreadable: {error}"))?;
    if !manifest_metadata.is_file() || manifest_metadata.file_type().is_symlink() {
        return Err("The bundled tool manifest must be an ordinary file.".into());
    }
    let manifest_file = match File::open(&manifest_path) {
        Ok(file) => file,
        Err(error) => return Err(format!("Cannot read bundled tool manifest: {error}")),
    };
    let mut content = Vec::new();
    manifest_file
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut content)
        .map_err(|error| format!("Cannot read bundled tool manifest: {error}"))?;
    if content.len() as u64 > MANIFEST_LIMIT {
        return Err("The bundled tool manifest exceeds its size limit.".into());
    }
    let manifest: Manifest = serde_json::from_slice(&content)
        .map_err(|error| format!("Invalid bundled tool manifest: {error}"))?;
    if manifest.schema_version != 1 || manifest.target != target() || manifest.tools.len() > 32 {
        return Err(
            "The bundled tool manifest has an unsupported version, platform or tool count.".into(),
        );
    }
    let mut identifiers = HashSet::new();
    for tool in &manifest.tools {
        if tool.id.is_empty() || !identifiers.insert(&tool.id) {
            return Err(
                "The bundled tool manifest contains missing or duplicate identities.".into(),
            );
        }
    }
    let Some(tool) = manifest.tools.iter().find(|tool| tool.id == id) else {
        return Ok(None);
    };
    if tool.support_files.len() > 256 {
        return Err(format!("Bundled {id} has too many runtime dependencies."));
    }
    let executable = verify_payload(root, &tool.path, &tool.sha256, id, true)?;
    let mut paths = HashSet::new();
    let mut verified_support = HashSet::new();
    paths.insert(&tool.path);
    for support in &tool.support_files {
        if !paths.insert(&support.path) {
            return Err(format!("Bundled {id} repeats a runtime dependency."));
        }
        verified_support.insert(verify_payload(
            root,
            &support.path,
            &support.sha256,
            id,
            false,
        )?);
    }
    Ok(Some(VerifiedTool {
        executable,
        support: verified_support,
    }))
}

fn verify_payload(
    root: &Path,
    name: &str,
    checksum: &str,
    id: &str,
    executable: bool,
) -> Result<PathBuf, String> {
    let relative = Path::new(name);
    if name.is_empty()
        || name.contains(['\\', ':'])
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || checksum.len() != 64
        || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!("Invalid bundled path or checksum for {id}."));
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("Cannot resolve bundled tools directory: {error}"))?;
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("Bundled {id} is missing or unreadable: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > TOOL_LIMIT {
        return Err(format!("Bundled {id} must be an ordinary executable file."));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0
            || (executable
                && !path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("exe")))
        {
            return Err(format!(
                "Bundled {id} must be a native .exe file without redirection."
            ));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if executable && metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "Bundled {id} does not have executable permissions."
            ));
        }
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("Cannot resolve bundled {id}: {error}"))?;
    if !path.starts_with(&root) || (executable && path.with_extension("shim").exists()) {
        return Err(format!(
            "Bundled {id} must remain inside its package and cannot be a launcher shim."
        ));
    }
    let mut file =
        File::open(&path).map_err(|error| format!("Cannot open bundled {id}: {error}"))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Cannot verify bundled {id}: {error}"))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    if !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(checksum) {
        return Err(format!(
            "Bundled {id} failed its SHA-256 integrity check. Restore the package or choose an explicit tool override."
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root =
                std::env::temp_dir().join(format!("jesses-bundled-{}-{nonce}", std::process::id()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn executable(&self) -> &'static str {
            if cfg!(windows) { "tool.exe" } else { "tool" }
        }
        fn stage(&self) {
            let executable = self.0.join(self.executable());
            fs::write(&executable, b"synthetic verified executable").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
            }
            self.manifest(self.executable(), target());
        }
        fn manifest(&self, path: &str, platform: &str) {
            fs::write(self.0.join("manifest.json"), serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1, "target": platform,
                "tools": [{ "id": "ffmpeg", "path": path, "sha256": format!("{:x}", Sha256::digest(b"synthetic verified executable")) }]
            })).unwrap()).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn verified_bundle_and_unbundled_tools_are_distinguished() {
        let fixture = Fixture::new();
        assert!(
            find_at(&fixture.0.join("absent"), "ffmpeg")
                .unwrap()
                .is_none()
        );
        assert!(find_at(&fixture.0, "ffmpeg").is_err());
        fixture.stage();
        assert_eq!(
            find_at(&fixture.0, "ffmpeg").unwrap(),
            Some(fixture.0.join(fixture.executable()).canonicalize().unwrap())
        );
        assert!(find_at(&fixture.0, "av1an").unwrap().is_none());
    }

    #[test]
    fn corrupt_missing_or_wrong_platform_bundles_do_not_fall_back() {
        let fixture = Fixture::new();
        fixture.stage();
        fs::write(fixture.0.join(fixture.executable()), b"changed").unwrap();
        assert!(
            find_at(&fixture.0, "ffmpeg")
                .unwrap_err()
                .contains("integrity")
        );
        fs::remove_file(fixture.0.join(fixture.executable())).unwrap();
        assert!(find_at(&fixture.0, "ffmpeg").is_err());
        fixture.manifest(fixture.executable(), "wrong-platform");
        assert!(find_at(&fixture.0, "ffmpeg").is_err());
        fs::write(fixture.0.join("manifest.json"), b"broken manifest").unwrap();
        assert!(find_at(&fixture.0, "ffmpeg").is_err());
    }

    #[test]
    fn package_paths_cannot_escape_or_use_shims() {
        let fixture = Fixture::new();
        fixture.stage();
        for path in [
            "../tool.exe",
            "/tool.exe",
            "C:/tool.exe",
            "directory\\tool.exe",
        ] {
            fixture.manifest(path, target());
            assert!(find_at(&fixture.0, "ffmpeg").is_err());
        }
        fixture.stage();
        fs::write(
            fixture.0.join(fixture.executable()).with_extension("shim"),
            b"unexpected",
        )
        .unwrap();
        assert!(find_at(&fixture.0, "ffmpeg").is_err());
    }

    #[test]
    fn runtime_dependency_corruption_and_escape_are_rejected() {
        let fixture = Fixture::new();
        fixture.stage();
        let dependency = fixture.0.join("codec.bin");
        fs::write(&dependency, b"codec dependency").unwrap();
        let manifest_path = fixture.0.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["tools"][0]["supportFiles"] = serde_json::json!([{
            "path": "codec.bin",
            "sha256": format!("{:x}", Sha256::digest(b"codec dependency"))
        }]);
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(find_at(&fixture.0, "ffmpeg").unwrap().is_some());
        fs::write(&dependency, b"modified codec").unwrap();
        assert!(
            find_at(&fixture.0, "ffmpeg")
                .unwrap_err()
                .contains("integrity")
        );
        manifest["tools"][0]["supportFiles"][0]["path"] = "../codec.bin".into();
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(find_at(&fixture.0, "ffmpeg").unwrap_err().contains("path"));
    }

    #[test]
    fn portable_av1an_requires_a_complete_unchanged_runtime_inventory() {
        let fixture = Fixture::new();
        let executable_name = if cfg!(windows) {
            "av1an/av1an.exe"
        } else {
            "av1an/av1an"
        };
        let executable = fixture.0.join(executable_name);
        let runtime = fixture.0.join("av1an/python/Lib/site-packages/vapoursynth");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(&executable, b"engine").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let names: &[&str] = if cfg!(windows) {
            &[
                "av1an/python/Lib/site-packages/vapoursynth/vsscript.dll",
                "av1an/python/Lib/site-packages/vapoursynth/vspipe.exe",
                "av1an/python/python.exe",
            ]
        } else {
            &[
                "av1an/python/Lib/site-packages/vapoursynth/libvsscript.so",
                "av1an/python/Lib/site-packages/vapoursynth/vspipe",
            ]
        };
        let support: Vec<_> = names.iter().map(|name| {
            fs::write(fixture.0.join(name), b"runtime").unwrap();
            serde_json::json!({"path": name, "sha256": format!("{:x}", Sha256::digest(b"runtime"))})
        }).collect();
        let manifest = serde_json::json!({"schemaVersion": 1, "target": target(), "tools": [{"id": "av1an", "path": executable_name, "sha256": format!("{:x}", Sha256::digest(b"engine")), "supportFiles": support}]});
        fs::write(
            fixture.0.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert_eq!(
            av1an_runtime_at(&fixture.0, &executable)
                .unwrap()
                .unwrap()
                .canonicalize()
                .unwrap(),
            runtime.canonicalize().unwrap()
        );
        let injected = runtime.join("unlisted-plugin.dll");
        fs::write(&injected, b"unexpected plugin").unwrap();
        assert!(
            av1an_runtime_at(&fixture.0, &executable)
                .unwrap_err()
                .contains("inventory")
        );
        fs::remove_file(injected).unwrap();
        fs::write(fixture.0.join(names[0]), b"changed").unwrap();
        assert!(
            av1an_runtime_at(&fixture.0, &executable)
                .unwrap_err()
                .contains("integrity")
        );
        let external = fixture.0.join("external.exe");
        fs::write(&external, b"external choice").unwrap();
        assert!(av1an_runtime_at(&fixture.0, &external).unwrap().is_none());
    }
}
