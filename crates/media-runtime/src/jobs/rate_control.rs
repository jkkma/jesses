//! Explicit bitrate passes share a private stats directory, never a decoder process.
use super::files;
use media_core::{AppError, EncodeBackend, EncodeSettings, VideoEncoder, VideoRateControl};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

fn invalid(message: &str) -> AppError {
    AppError::new("ENCODE_SETTINGS_INVALID", message, None)
}

pub(super) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    if settings.lossless && settings.rate_control.is_some() {
        return Err(invalid(
            "Lossless mode cannot be combined with bitrate or target-size rate control.",
        ));
    }
    let Some(control) = settings.rate_control else {
        return Ok(());
    };
    if settings.backend != EncodeBackend::Standalone {
        return Err(invalid(
            "Bitrate and target size currently require standalone encoding.",
        ));
    }
    if matches!(
        settings.encoder,
        VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc
    ) && !matches!(
        control,
        VideoRateControl::Bitrate {
            two_pass: false,
            ..
        }
    ) {
        return Err(invalid(
            "NVENC supports one-pass video bitrate in this workflow. Two-pass bitrate and target size require a software encoder with external pass statistics.",
        ));
    }
    match control {
        VideoRateControl::Bitrate { bitrate_kbps, .. }
            if !(1..=100_000).contains(&bitrate_kbps) =>
        {
            Err(invalid(
                "Video bitrate must be a whole number from 1 to 100000 decimal kb/s.",
            ))
        }
        VideoRateControl::TargetSize { target_size_mib }
            if !(1..=1_048_576).contains(&target_size_mib) =>
        {
            Err(invalid(
                "Target file size must be a whole number from 1 to 1048576 MiB.",
            ))
        }
        _ => Ok(()),
    }
}

pub(super) fn validate_encoder_version(
    settings: &EncodeSettings,
    identity: &str,
) -> Result<(), String> {
    if settings.encoder != VideoEncoder::SvtAv1
        || !matches!(
            settings.rate_control,
            Some(
                VideoRateControl::Bitrate { two_pass: true, .. }
                    | VideoRateControl::TargetSize { .. }
            )
        )
    {
        return Ok(());
    }
    // SVT 2.0 replaced three-pass VBR with two-pass VBR. In older builds,
    // --pass 2 emits an intermediate result, not the final encoded video.
    let major = identity
        .lines()
        .filter(|line| line.to_ascii_lowercase().starts_with("svt-av1"))
        .flat_map(str::split_whitespace)
        .find_map(|word| {
            let mut parts = word.strip_prefix('v')?.split('.');
            let major = parts.next()?.parse::<u32>().ok()?;
            parts.next()?.parse::<u32>().ok()?;
            parts.next()?.split('-').next()?.parse::<u32>().ok()?;
            Some(major)
        });
    if major.is_some_and(|major| major >= 2) {
        Ok(())
    } else {
        Err("Mainline SVT-AV1 two-pass bitrate and target size require version 2.0.0 or newer. Older versions use three-pass VBR. Update the encoder in Tools, or choose CRF or one-pass bitrate.".into())
    }
}

pub(super) struct Rate {
    pub kbps: u32,
    pub two_pass: bool,
}

impl Rate {
    pub fn resolve(
        settings: &EncodeSettings,
        seconds: f64,
        nonvideo_bytes: u64,
    ) -> Result<Option<Self>, AppError> {
        let Some(control) = settings.rate_control else {
            return Ok(None);
        };
        Ok(Some(match control {
            VideoRateControl::Bitrate {
                bitrate_kbps,
                two_pass,
            } => Self {
                kbps: bitrate_kbps,
                two_pass,
            },
            VideoRateControl::TargetSize { target_size_mib } => {
                let total = u64::from(target_size_mib) * 1024 * 1024;
                // A target is an estimate. Reserve 1% plus 64 KiB for video packet
                // framing/indexes and the final container's non-payload bytes.
                let reserve = total / 100 + 65_536;
                let available = total.checked_sub(nonvideo_bytes).and_then(|n| n.checked_sub(reserve))
                    .ok_or_else(|| invalid("The target size cannot contain the measured selected audio, subtitles, attachments, and container reserve."))?;
                let kbps = (available as f64 * 8.0 / seconds / 1000.0).floor();
                if !seconds.is_finite() || seconds <= 0.0 || !(1.0..=100_000.0).contains(&kbps) {
                    return Err(invalid(
                        "The requested target size requires a video bitrate outside 1–100000 kb/s for this validated interval.",
                    ));
                }
                Self {
                    kbps: kbps as u32,
                    two_pass: true,
                }
            }
        }))
    }

    pub fn arguments(&self, args: &mut Vec<OsString>, encoder: VideoEncoder, pass: u8) {
        let remove_pair = |args: &mut Vec<OsString>, key: &str| {
            if let Some(index) = args.iter().position(|a| a == key) {
                args.drain(index..index + 2);
            }
        };
        let mut add: Vec<OsString> = Vec::new();
        match encoder {
            VideoEncoder::X264 => {
                remove_pair(args, "--crf");
                add.extend(["--bitrate".into(), self.kbps.to_string().into()]);
                if self.two_pass {
                    add.extend([
                        "--pass".into(),
                        pass.to_string().into(),
                        "--stats".into(),
                        "jesses.stats".into(),
                    ]);
                }
            }
            VideoEncoder::X265 | VideoEncoder::Vp9 => {
                remove_pair(args, "-crf");
                remove_pair(args, "-b:v");
                add.extend([
                    "-b:v".into(),
                    (u64::from(self.kbps) * 1000).to_string().into(),
                ]);
                if self.two_pass {
                    if encoder == VideoEncoder::X265 {
                        let at = args
                            .iter()
                            .position(|a| a == "-x265-params")
                            .expect("x265 params")
                            + 1;
                        let mut value = args[at].clone();
                        value.push(format!(":pass={pass}:stats=jesses.stats"));
                        args[at] = value;
                    } else {
                        add.extend([
                            "-pass".into(),
                            pass.to_string().into(),
                            "-passlogfile".into(),
                            "jesses.stats".into(),
                        ]);
                    }
                }
            }
            VideoEncoder::X265Standalone => {
                remove_pair(args, "--crf");
                add.extend(["--bitrate".into(), self.kbps.to_string().into()]);
                if self.two_pass {
                    add.extend([
                        "--pass".into(),
                        pass.to_string().into(),
                        "--stats".into(),
                        "jesses.stats".into(),
                    ]);
                }
            }
            VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone => {
                for option in [
                    "--cq-level=",
                    "--end-usage=",
                    "--target-bitrate=",
                    "--passes=",
                ] {
                    args.retain(|argument| !argument.to_string_lossy().starts_with(option));
                }
                add.extend([
                    "--end-usage=vbr".into(),
                    format!("--target-bitrate={}", self.kbps).into(),
                    format!("--passes={}", if self.two_pass { 2 } else { 1 }).into(),
                ]);
                if self.two_pass {
                    add.extend([format!("--pass={pass}").into(), "--fpf=jesses.stats".into()]);
                }
            }
            VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc => {
                remove_pair(args, "-cq");
                remove_pair(args, "-b:v");
                add.extend([
                    "-b:v".into(),
                    (u64::from(self.kbps) * 1000).to_string().into(),
                ]);
            }
            VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
                remove_pair(args, "--crf");
                remove_pair(args, "--passes");
                add.extend([
                    "--rc".into(),
                    "1".into(),
                    "--tbr".into(),
                    self.kbps.to_string().into(),
                ]);
                if self.two_pass {
                    add.extend([
                        "--pass".into(),
                        pass.to_string().into(),
                        "--stats".into(),
                        "jesses.stats".into(),
                    ]);
                } else {
                    add.extend(["--passes".into(), "1".into()]);
                }
            }
        }
        // FFmpeg distinguishes input and output option scopes. Its bitrate and
        // pass options belong after the Y4M input and before the output URL.
        let at = if encoder.is_ffmpeg() {
            args.len() - 1
        } else {
            0
        };
        args.splice(at..at, add);
    }
}

pub(super) struct Stats {
    pub path: PathBuf,
    preserve: bool,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(windows)]
    handle: fs::File,
}

impl Stats {
    pub fn create(output: &Path, id: &str) -> Result<Self, AppError> {
        let path = output
            .parent()
            .expect("output parent")
            .join(format!(".jesses-{id}-passes"));
        Self::open_at(path, false, false)
    }

    pub fn durable(path: &Path, existing: bool) -> Result<Self, AppError> {
        Self::open_at(path.to_owned(), existing, true)
    }

    fn open_at(path: PathBuf, existing: bool, preserve: bool) -> Result<Self, AppError> {
        if existing {
            let metadata = fs::symlink_metadata(&path)
                .map_err(|e| files::error("OUTPUT_CREATE_FAILED", e.to_string(), &path))?;
            if !metadata.is_dir() {
                return Err(files::error(
                    "OUTPUT_CHANGED",
                    "The durable pass-statistics path is not a directory.",
                    &path,
                ));
            }
        } else {
            fs::create_dir(&path)
                .map_err(|e| files::error("OUTPUT_CREATE_FAILED", e.to_string(), &path))?;
        }
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::symlink_metadata(&path)
                .map_err(|e| files::error("OUTPUT_CREATE_FAILED", e.to_string(), &path))?;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(windows)]
        let handle = {
            use std::os::windows::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .access_mode(0x0001_0080)
                .share_mode(1 | 2)
                .custom_flags(0x0200_0000)
                .open(&path)
                .map_err(|e| files::error("OUTPUT_CREATE_FAILED", e.to_string(), &path))?
        };
        Ok(Self {
            path,
            preserve,
            #[cfg(unix)]
            identity,
            #[cfg(windows)]
            handle,
        })
    }

    pub fn check(&self) -> Result<(), AppError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::symlink_metadata(&self.path)
                .map_err(|e| files::error("OUTPUT_CHANGED", e.to_string(), &self.path))?;
            if !metadata.is_dir() || (metadata.dev(), metadata.ino()) != self.identity {
                return Err(files::error(
                    "OUTPUT_CHANGED",
                    "The owned pass directory changed.",
                    &self.path,
                ));
            }
        }
        #[cfg(windows)]
        let _ = &self.handle; // Directory handle denies replacement/deletion until dropped.
        Ok(())
    }

    pub fn verify_stats(&self) -> Result<(), AppError> {
        self.check()?;
        let mut nonempty = false;
        for entry in fs::read_dir(&self.path)
            .map_err(|e| files::error("OUTPUT_UNREADABLE", e.to_string(), &self.path))?
        {
            let entry =
                entry.map_err(|e| files::error("OUTPUT_UNREADABLE", e.to_string(), &self.path))?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|e| files::error("OUTPUT_UNREADABLE", e.to_string(), &self.path))?;
            if !metadata.is_file()
                || !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("jesses.stats")
            {
                return Err(files::error(
                    "OUTPUT_CHANGED",
                    "Unexpected content in the owned pass directory.",
                    &self.path,
                ));
            }
            nonempty |= metadata.len() > 0;
        }
        if nonempty {
            Ok(())
        } else {
            Err(files::error(
                "ENCODE_PASS_FAILED",
                "Pass one produced no statistics; pass two was not started.",
                &self.path,
            ))
        }
    }
}

impl Drop for Stats {
    fn drop(&mut self) {
        if self.preserve {
            return;
        }
        if self.check().is_err() {
            return;
        }
        if let Ok(entries) = fs::read_dir(&self.path) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("jesses.stats")
                    && fs::symlink_metadata(entry.path()).is_ok_and(|m| m.is_file())
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        // On Windows our directory handle still denies deletion here. Mark the
        // now-empty directory delete-pending through this owned handle below.
        #[cfg(not(windows))]
        let _ = fs::remove_dir(&self.path);
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
            };
            let info = FILE_DISPOSITION_INFO { DeleteFile: true };
            unsafe {
                SetFileInformationByHandle(
                    self.handle.as_raw_handle(),
                    FileDispositionInfo,
                    (&info as *const FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of_val(&info) as u32,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mainline_two_pass_requires_the_final_pass_protocol() {
        for control in [
            VideoRateControl::Bitrate {
                bitrate_kbps: 300,
                two_pass: true,
            },
            VideoRateControl::TargetSize { target_size_mib: 1 },
        ] {
            let settings = EncodeSettings {
                encoder: VideoEncoder::SvtAv1,
                rate_control: Some(control),
                ..Default::default()
            };
            for identity in [
                "SVT-AV1 Encoder Lib v1.7.0",
                "SVT-AV1 Encoder Lib v1.8.0",
                "SVT-AV1 development build",
                "SVT-AV1 version unavailable\nUnrelated tool v4.2.0",
            ] {
                assert!(
                    validate_encoder_version(&settings, identity)
                        .unwrap_err()
                        .contains("2.0.0"),
                    "{identity}"
                );
            }
            for identity in [
                "SVT-AV1 Encoder Lib v2.0.0",
                "SVT-AV1 v4.2.0 (release)",
                "SVT-AV1 v4.2.0-21-g00333404f (release)",
            ] {
                validate_encoder_version(&settings, identity).unwrap();
            }
            for encoder in [
                VideoEncoder::SvtAv1FiveFish,
                VideoEncoder::SvtAv1Hdr,
                VideoEncoder::X264,
                VideoEncoder::X265,
                VideoEncoder::Vp9,
            ] {
                validate_encoder_version(
                    &EncodeSettings {
                        encoder,
                        ..settings.clone()
                    },
                    "This driver's version is checked separately",
                )
                .unwrap();
            }
        }
        for control in [
            None,
            Some(VideoRateControl::Bitrate {
                bitrate_kbps: 300,
                two_pass: false,
            }),
        ] {
            validate_encoder_version(
                &EncodeSettings {
                    encoder: VideoEncoder::SvtAv1,
                    rate_control: control,
                    ..Default::default()
                },
                "SVT-AV1 Encoder Lib v1.7.0",
            )
            .unwrap();
        }
    }

    #[test]
    fn old_settings_keep_quality_and_new_modes_validate_before_tools() {
        let old: EncodeSettings =
            serde_json::from_str(r#"{"videoStreamIndex":0,"crf":30,"preset":4}"#).unwrap();
        assert_eq!(old.rate_control, None);
        assert!(
            serde_json::to_value(&old)
                .unwrap()
                .get("rateControl")
                .is_none()
        );
        for kbps in [0, 100001, u32::MAX] {
            assert!(
                validate(&EncodeSettings {
                    parameters: Vec::new(),
                    av1an_options: None,
                    rate_control: Some(VideoRateControl::Bitrate {
                        bitrate_kbps: kbps,
                        two_pass: true
                    }),
                    ..old.clone()
                })
                .is_err()
            );
        }
        assert!(
            validate(&EncodeSettings {
                backend: EncodeBackend::Av1an,
                rate_control: Some(VideoRateControl::Bitrate {
                    bitrate_kbps: 1000,
                    two_pass: false
                }),
                ..old.clone()
            })
            .is_err()
        );
        for control in [
            VideoRateControl::Bitrate {
                bitrate_kbps: 1000,
                two_pass: true,
            },
            VideoRateControl::TargetSize {
                target_size_mib: 100,
            },
        ] {
            let settings = EncodeSettings {
                parameters: Vec::new(),
                av1an_options: None,
                rate_control: Some(control),
                ..old.clone()
            };
            assert!(validate(&settings).is_ok());
            assert_eq!(
                serde_json::from_value::<EncodeSettings>(serde_json::to_value(&settings).unwrap())
                    .unwrap(),
                settings
            );
        }
    }

    #[test]
    fn target_size_subtracts_measured_media_and_reserve_before_decimal_bitrate() {
        let settings = EncodeSettings {
            parameters: Vec::new(),
            av1an_options: None,
            rate_control: Some(VideoRateControl::TargetSize {
                target_size_mib: 100,
            }),
            ..EncodeSettings::default()
        };
        let rate = Rate::resolve(&settings, 60.0, 10 * 1024 * 1024)
            .unwrap()
            .unwrap();
        assert_eq!(rate.kbps, 12434);
        assert!(rate.two_pass);
        assert!(Rate::resolve(&settings, 60.0, 100 * 1024 * 1024).is_err());
        assert!(Rate::resolve(&settings, 0.0, 0).is_err());
    }
}
