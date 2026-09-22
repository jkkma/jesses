//! Encode settings are an optional, verified Matroska attachment. They are added
//! to the unpublished mux attempt, never by replacing a completed user output.
use super::{files::Temporary, metadata::Document};
use media_core::{AppError, EncodeSettings};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, io::Write, path::Path};

const NAME: &str = "jesses-encode-settings.json";
const MIME: &str = "application/json";

pub(super) struct SettingsAttachment {
    file: Temporary,
    digest: String,
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
        Ok(Some(Self {
            file,
            digest: format!("SHA256:{:x}", Sha256::digest(&bytes)),
        }))
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

    pub fn verify_and_remove(&self, artifact: &mut Document) -> Result<(), AppError> {
        let valid = artifact.streams.last().is_some_and(|stream| {
            let tag = |key: &str| {
                stream
                    .tags
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(key))
                    .map(|(_, value)| value.as_str())
            };
            stream.codec_type.as_deref() == Some("attachment")
                && tag("filename") == Some(NAME)
                && tag("mimetype") == Some(MIME)
                && stream
                    .extradata_hash
                    .as_deref()
                    .is_some_and(|hash| hash.eq_ignore_ascii_case(&self.digest))
        });
        if !valid {
            return Err(AppError::new(
                "OUTPUT_VALIDATION_FAILED",
                "The output encode-settings attachment is missing or differs from the requested settings.",
                None,
            ));
        }
        artifact.streams.pop();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_attachment_requires_exact_content_and_does_not_hide_source_tracks() {
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
        let mut artifact:Document=serde_json::from_value(serde_json::json!({"streams":[{"index":0,"codec_type":"video"},{"index":1,"codec_type":"attachment","tags":{"filename":NAME,"mimetype":MIME},"extradata_hash":attachment.digest}]})).unwrap();
        let mut altered = artifact.clone();
        altered.streams[1].extradata_hash = Some("SHA256:changed".into());
        assert!(attachment.verify_and_remove(&mut altered).is_err());
        assert_eq!(altered.streams.len(), 2);
        attachment.verify_and_remove(&mut artifact).unwrap();
        assert_eq!(artifact.streams.len(), 1);
        drop(attachment);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir(dir).unwrap();
    }
}
