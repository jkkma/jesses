//! Encode settings are an optional, verified Matroska attachment. They are added
//! to the unpublished mux attempt, never by replacing a completed user output.
use super::files::Temporary;
use media_core::{AppError, EncodeSettings};
use std::{ffi::OsString, io::Write, path::Path};

const NAME: &str = "jesses-encode-settings.json";
const MIME: &str = "application/json";

pub(super) struct SettingsAttachment {
    file: Temporary,
}

impl SettingsAttachment {
    pub fn prepare(
        output: &Path,
        id: &str,
        settings: &EncodeSettings,
    ) -> Result<Option<Self>, AppError> {
        if !settings.av1an_options.is_some_and(|o| o.attach_settings) {
            return Ok(None);
        }
        if super::container::format(output)? != media_core::ContainerFormat::Matroska {
            return Err(AppError::new(
                "CONTAINER_INCOMPATIBLE",
                "Encode settings attachments require a Matroska (.mkv) destination.",
                None,
            ));
        }
        let bytes = serde_json::to_vec_pretty(&serde_json::json!({"format":"jesses-encode-settings", "version":1, "settings":settings}))
            .map_err(|e| AppError::new("ENCODE_SETTINGS_INVALID", e.to_string(), None))?;
        let file = Temporary::create_extension(output, &format!("{id}-settings"), "json")?;
        let mut handle = file.clone_file()?;
        handle
            .write_all(&bytes)
            .and_then(|_| handle.sync_all())
            .map_err(|e| AppError::new("OUTPUT_WRITE_FAILED", e.to_string(), None))?;
        Ok(Some(Self { file }))
    }

    pub fn apply(&self, args: &mut Vec<OsString>, existing_attachments: usize) {
        let output = args.pop().expect("mux output argument");
        args.extend([
            "-attach".into(),
            self.file.path.as_os_str().to_owned(),
            format!("-metadata:s:t:{existing_attachments}").into(),
            format!("filename={NAME}").into(),
            format!("-metadata:s:t:{existing_attachments}").into(),
            format!("mimetype={MIME}").into(),
        ]);
        args.push(output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_attachment_writes_the_requested_settings_and_mux_options() {
        let dir = std::env::temp_dir().join(format!(
            "jesses-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            av1an_options: Some(media_core::Av1anOptions {
                attach_settings: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(SettingsAttachment::prepare(&dir.join("out.mp4"), "test", &settings).is_err());
        let attachment = SettingsAttachment::prepare(&dir.join("out.mkv"), "test", &settings)
            .unwrap()
            .unwrap();
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&attachment.file.path).unwrap()).unwrap();
        assert_eq!(stored["settings"], serde_json::to_value(&settings).unwrap());
        let mut args = vec!["output.mkv".into()];
        attachment.apply(&mut args, 2);
        assert_eq!(args.last().unwrap(), "output.mkv");
        assert!(args.iter().any(|arg| arg == "-metadata:s:t:2"));
        assert!(
            args.iter()
                .any(|arg| arg == "filename=jesses-encode-settings.json")
        );
        drop(attachment);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir(dir).unwrap();
    }
}
