//! av1an names each encoder internally. Route every child to the chosen binary.
use super::*;
use media_core::VideoEncoder;

pub(super) fn validate_encoder_path(encoder: &Path, family: VideoEncoder) -> Result<(), AppError> {
    let expected = super::encoder::binary(family);
    #[cfg(windows)]
    let canonical_name = encoder
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(format!("{expected}.exe")));
    #[cfg(not(windows))]
    let canonical_name = encoder.file_name().is_some_and(|name| name == expected);
    if !encoder.is_absolute() || !canonical_name {
        return Err(files::error(
            "AV1AN_ENCODER_PATH_UNSUPPORTED",
            format!(
                "av1an requires the selected encoder to use its canonical child name {expected} in its own folder."
            ),
            encoder,
        ));
    }
    Ok(())
}

pub(super) struct Launch {
    pub(super) executable: PathBuf,
    pub(super) environment: supervisor::ChildEnvironment,
    #[cfg(windows)]
    image: Option<std::fs::File>,
    #[cfg(windows)]
    identity: (u32, u32, u32),
    #[cfg(windows)]
    staged: bool,
}

impl Launch {
    pub(super) fn prepare(
        av1an: &Path,
        encoder: &Path,
        family: VideoEncoder,
        ffmpeg: &Path,
        ffprobe: &Path,
        mkvmerge: Option<&Path>,
        work: &Path,
    ) -> Result<Self, AppError> {
        validate_encoder_path(encoder, family)?;
        let selected = encoder.parent().expect("absolute encoder parent");
        let original = av1an.parent().expect("absolute av1an parent");
        let ffmpeg_parent = ffmpeg
            .parent()
            .filter(|_| ffmpeg.is_absolute())
            .ok_or_else(|| {
                files::error(
                    "AV1AN_ENCODER_PATH_UNSUPPORTED",
                    "The selected FFmpeg path must be absolute.",
                    ffmpeg,
                )
            })?;
        let ffprobe_parent = ffprobe
            .parent()
            .filter(|_| ffprobe.is_absolute())
            .ok_or_else(|| {
                files::error(
                    "AV1AN_ENCODER_PATH_UNSUPPORTED",
                    "The selected FFprobe path must be absolute.",
                    ffprobe,
                )
            })?;
        let runtime = crate::bundled_tools::av1an_runtime(av1an)
            .map_err(|error| files::error("BUNDLED_TOOL_INVALID", error, av1an))?;
        let mut directories = vec![selected, ffmpeg_parent, ffprobe_parent];
        if let Some(mkvmerge) = mkvmerge {
            let directory = mkvmerge
                .parent()
                .filter(|_| mkvmerge.is_absolute())
                .ok_or_else(|| {
                    files::error(
                        "AV1AN_ENCODER_PATH_UNSUPPORTED",
                        "The selected mkvmerge path must be absolute.",
                        mkvmerge,
                    )
                })?;
            directories.push(directory);
        }
        directories.push(original);
        if let Some(directory) = &runtime {
            directories.push(directory);
        }
        let mut path = std::env::join_paths(directories)
            .map_err(|e| files::error("AV1AN_ENCODER_PATH_UNSUPPORTED", e.to_string(), encoder))?;
        if let Some(inherited) = std::env::var_os("PATH") {
            path.push(if cfg!(windows) { ";" } else { ":" });
            path.push(inherited);
        }
        let mut environment = supervisor::ChildEnvironment::with_path(&path);
        if let Some(directory) = &runtime {
            environment = crate::bundled_tools::frameserver_environment(directory, &path);
        }
        #[cfg(windows)]
        {
            use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};
            reject_system_shadow(encoder)?;
            // Rust's Windows process lookup searches the host executable folder
            // before inherited PATH. Stage an owned host image so a portable
            // av1an installation's sibling child cannot silently override selection.
            // The original directory remains on child PATH for its DLLs/tools.
            let source = OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(av1an)
                .map_err(|e| files::error("AV1AN_STAGE_FAILED", e.to_string(), av1an))?;
            let expected_bytes = source
                .metadata()
                .map_err(|e| files::error("AV1AN_STAGE_FAILED", e.to_string(), av1an))?
                .len();
            let executable = work.join("av1an.exe");
            let output = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .share_mode(1)
                .open(&executable)
                .map_err(|e| files::error("AV1AN_STAGE_FAILED", e.to_string(), &executable))?;
            let identity = files::windows_file_id(&output)
                .map_err(|e| files::error("AV1AN_STAGE_FAILED", e.to_string(), &executable))?;
            // From here onward every error drops the handles and removes only
            // this file identity, including a partially copied host image.
            let mut launch = Self {
                executable,
                environment,
                image: Some(output),
                identity,
                staged: true,
            };
            let output = launch.image.as_mut().expect("owned writable image");
            let copied = std::io::copy(&mut &source, &mut *output)
                .and_then(|count| {
                    output.sync_all()?;
                    Ok(count)
                })
                .map_err(|e| {
                    files::error("AV1AN_STAGE_FAILED", e.to_string(), &launch.executable)
                })?;
            drop(launch.image.take());
            if copied != expected_bytes {
                return Err(files::error(
                    "AV1AN_STAGE_FAILED",
                    "The av1an image changed while staging.",
                    &launch.executable,
                ));
            }
            // Writable handles must close before Windows can map an executable.
            // This read handle denies writes/deletion while the image is in use.
            let image = OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&launch.executable)
                .map_err(|e| {
                    files::error("AV1AN_STAGE_FAILED", e.to_string(), &launch.executable)
                })?;
            if files::windows_file_id(&image).map_err(|e| {
                files::error("AV1AN_STAGE_FAILED", e.to_string(), &launch.executable)
            })? != identity
            {
                return Err(files::error(
                    "AV1AN_STAGE_FAILED",
                    "The staged av1an image was replaced.",
                    &launch.executable,
                ));
            }
            launch.image = Some(image);
            Ok(launch)
        }
        #[cfg(not(windows))]
        {
            let _ = work;
            Ok(Self {
                executable: av1an.to_owned(),
                environment,
            })
        }
    }

    pub(super) fn cleanup(&mut self) -> Result<(), AppError> {
        #[cfg(windows)]
        if self.staged {
            drop(self.image.take());
            files::windows_delete_owned(&self.executable, self.identity).map_err(|e| {
                files::error(
                    "AV1AN_STAGE_CLEANUP_FAILED",
                    e.to_string(),
                    &self.executable,
                )
            })?;
            self.staged = false;
        }
        Ok(())
    }
}

impl Drop for Launch {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[cfg(windows)]
fn reject_system_shadow(encoder: &Path) -> Result<(), AppError> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::{
        GetSystemDirectoryW, GetWindowsDirectoryW,
    };
    let selected = encoder
        .canonicalize()
        .map_err(|e| files::error("AV1AN_ENCODER_PATH_UNSUPPORTED", e.to_string(), encoder))?;
    for get in [GetSystemDirectoryW, GetWindowsDirectoryW] {
        let mut buffer = vec![0; 32768];
        // SAFETY: buffer is writable for the specified number of UTF-16 units.
        let length = unsafe { get(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        if length == 0 || length >= buffer.len() {
            return Err(files::error(
                "AV1AN_ENCODER_PATH_UNSUPPORTED",
                "Windows system tool lookup could not be checked.",
                encoder,
            ));
        }
        let shadow = PathBuf::from(OsString::from_wide(&buffer[..length]))
            .join(encoder.file_name().expect("validated encoder basename"));
        match shadow.canonicalize() {
            Ok(path) if path != selected => {
                return Err(files::error(
                    "AV1AN_ENCODER_PATH_UNSUPPORTED",
                    "A Windows system-directory encoder would override the selected encoder.",
                    &shadow,
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(files::error(
                    "AV1AN_ENCODER_PATH_UNSUPPORTED",
                    error.to_string(),
                    &shadow,
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_aliases_fail_before_av1an_can_choose_another_binary() {
        let folder = std::env::temp_dir();
        let name = if cfg!(windows) {
            "SvtAv1EncApp.exe"
        } else {
            "SvtAv1EncApp"
        };
        assert!(validate_encoder_path(&folder.join(name), VideoEncoder::SvtAv1).is_ok());
        for path in [PathBuf::from(name), folder.join("SvtAv1EncApp-HDR.exe")] {
            assert_eq!(
                validate_encoder_path(&path, VideoEncoder::SvtAv1)
                    .unwrap_err()
                    .code,
                "AV1AN_ENCODER_PATH_UNSUPPORTED"
            );
        }
    }

    #[cfg(windows)]
    fn helper_args(name: &str) -> Vec<OsString> {
        [
            "--exact".to_owned(),
            format!("jobs::av1an::launcher::tests::{name}"),
            "--ignored".to_owned(),
            "--nocapture".to_owned(),
        ]
        .into_iter()
        .map(Into::into)
        .collect()
    }

    #[cfg(windows)]
    fn is_helper(name: &str) -> bool {
        let arguments: Vec<_> = std::env::args_os().collect();
        arguments
            .windows(2)
            .any(|pair| pair == &helper_args(name)[..2])
            && arguments.iter().any(|arg| arg == "--ignored")
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "owned subprocess fixture"]
    fn lookup_encoder() {
        if !is_helper("lookup_encoder") {
            return;
        }
        let executable = std::env::current_exe().unwrap();
        println!(
            "SELECTED_ENCODER={}",
            std::fs::read_to_string(executable.parent().unwrap().join("identity")).unwrap()
        );
        println!(
            "CHILD_SCOPE={}",
            std::env::var("JESSES_CHILD_SCOPE").unwrap_or_default()
        );
        println!(
            "PROFILE_PRESENT={}",
            std::env::var_os("USERPROFILE").is_some()
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "owned subprocess fixture"]
    fn lookup_host() {
        if !is_helper("lookup_host") {
            return;
        }
        use std::os::windows::process::CommandExt;
        // This descendant is deliberately created inside the supervised host,
        // using the same inherited-PATH lookup as av1an's hardcoded tool name.
        let output = std::process::Command::new("SvtAv1EncApp")
            .args(helper_args("lookup_encoder"))
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
        for name in ["ffmpeg", "ffprobe"] {
            let output = std::process::Command::new(name)
                .args(helper_args("lookup_encoder"))
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(output.status.success());
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn staged_host_routes_past_a_competing_sibling_and_releases_its_owned_image() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root = Fixture(std::env::temp_dir().join(format!(
            "jesses-av1an-launch-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        )));
        let original = root.0.join("original host");
        let selected = root.0.join("selected 日本語's fork");
        let work = root.0.join("owned work");
        let media_tools = root.0.join("selected media tools");
        for directory in [&original, &selected, &work, &media_tools] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let current = std::env::current_exe().unwrap();
        let av1an = original.join("av1an.exe");
        let encoder = selected.join("SvtAv1EncApp.exe");
        for path in [&av1an, &original.join("SvtAv1EncApp.exe"), &encoder] {
            // Hosted Windows runners can place TEMP and the checkout on
            // different volumes. Each helper owns a copy of the test image.
            std::fs::copy(&current, path).unwrap();
        }
        for name in ["ffmpeg.exe", "ffprobe.exe"] {
            std::fs::copy(&current, media_tools.join(name)).unwrap();
            std::fs::copy(&current, original.join(name)).unwrap();
        }
        std::fs::write(media_tools.join("identity"), "selected-media-tools").unwrap();
        std::fs::write(original.join("identity"), "wrong-mainline").unwrap();
        std::fs::write(selected.join("identity"), "selected-fork").unwrap();
        let mut launch = Launch::prepare(
            &av1an,
            &encoder,
            VideoEncoder::SvtAv1,
            &media_tools.join("ffmpeg.exe"),
            &media_tools.join("ffprobe.exe"),
            None,
            &work,
        )
        .unwrap();
        let (_owner, cancel) = watch::channel(false);
        let capture = |executable: PathBuf| CommandSpec {
            executable,
            args: helper_args("lookup_host"),
            cwd: Some(work.clone()),
        };
        let original_result = supervisor::run_capture_with_path(
            &capture(av1an.clone()),
            cancel.clone(),
            65536,
            Duration::from_secs(10),
            launch.environment.path.as_deref(),
        )
        .await
        .unwrap();
        assert!(original_result.status.success());
        assert!(
            String::from_utf8_lossy(&original_result.stdout)
                .contains("SELECTED_ENCODER=wrong-mainline")
        );
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(&launch.executable)
                .is_err()
        );
        assert!(std::fs::remove_file(&launch.executable).is_err());
        launch.environment.variables = vec![
            ("JESSES_CHILD_SCOPE", Some("portable 日本語".into())),
            ("USERPROFILE", None),
        ];
        let parent_profile = std::env::var_os("USERPROFILE");
        let routed = supervisor::run_capture_with_environment(
            &capture(launch.executable.clone()),
            cancel,
            65536,
            Duration::from_secs(10),
            Some(&launch.environment),
        )
        .await
        .unwrap();
        assert!(routed.status.success());
        let output = String::from_utf8_lossy(&routed.stdout);
        assert_eq!(
            output.matches("CHILD_SCOPE=portable 日本語").count(),
            3,
            "{output}"
        );
        assert_eq!(
            output.matches("PROFILE_PRESENT=false").count(),
            3,
            "{output}"
        );
        assert_eq!(std::env::var_os("USERPROFILE"), parent_profile);
        assert!(
            output.contains("SELECTED_ENCODER=selected-fork"),
            "{output}"
        );
        assert!(
            !output.contains("SELECTED_ENCODER=wrong-mainline"),
            "{output}"
        );
        assert_eq!(
            output
                .matches("SELECTED_ENCODER=selected-media-tools")
                .count(),
            2,
            "{output}"
        );
        let staged = launch.executable.clone();
        launch.cleanup().unwrap();
        assert!(!staged.exists());
        assert!(av1an.exists());
        // A preexisting destination is never replaced or deleted on failure.
        std::fs::write(&staged, b"preserve me").unwrap();
        assert!(
            Launch::prepare(
                &av1an,
                &encoder,
                VideoEncoder::SvtAv1,
                &media_tools.join("ffmpeg.exe"),
                &media_tools.join("ffprobe.exe"),
                None,
                &work
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&staged).unwrap(), b"preserve me");
    }
}
