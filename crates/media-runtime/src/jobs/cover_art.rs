//! Embedded cover art: a still JPEG or PNG picture marked `attached_pic`.
//!
//! The picture's single packet is copied out of the source and attached to
//! the Matroska stage as `cover.jpg` or `cover.png`. Matroska readers show
//! such an attachment as the file's cover, and the MP4 conversion turns the
//! same attachment into the MP4 cover. MOV and WebM cannot keep it, so the
//! app leaves it out there and says so.
use super::{files::Temporary, metadata::Stream};
use crate::supervisor::{self, CommandSpec};
use media_core::{AppError, ContainerFormat, EncodeSettings};
use std::{ffi::OsString, io::Write, path::Path, time::Duration};
use tokio::sync::watch;

/// Covers are small; anything larger is not treated as one.
const MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Picture {
    Jpeg,
    Png,
}

impl Picture {
    fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else {
            None
        }
    }
    fn file_name(self) -> &'static str {
        match self {
            Self::Jpeg => "cover.jpg",
            Self::Png => "cover.png",
        }
    }
    fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
        }
    }
}

fn left_out(message: impl Into<String>) -> AppError {
    AppError::new("COVER_ART_LEFT_OUT", message, None)
}

/// Whether a stream is a cover picture rather than playable video.
pub(super) fn is_cover(stream: &Stream) -> bool {
    stream.codec_type.as_deref() == Some("video")
        && stream
            .disposition
            .get("attached_pic")
            .is_some_and(|value| *value != 0)
}

/// Containers whose finished file keeps cover art.
pub(super) fn supported(container: ContainerFormat) -> bool {
    matches!(container, ContainerFormat::Matroska | ContainerFormat::Mp4)
}

pub(super) struct CoverArt {
    file: Temporary,
    picture: Picture,
}

impl CoverArt {
    /// Copies the requested cover's one packet out of `input`. The error is
    /// a reason to leave the cover out; cancellation keeps its own code.
    pub async fn prepare(
        ffmpeg: &Path,
        input: &Path,
        output: &Path,
        matroska_stage: bool,
        id: &str,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Option<Self>, AppError> {
        let Some(index) = settings.cover_stream_index else {
            return Ok(None);
        };
        let container = super::container::format(output)?;
        if !supported(container) {
            return Err(left_out(
                "Cover art was left out: only MKV and MP4 destinations can store it.",
            ));
        }
        if !matroska_stage {
            return Err(left_out(
                "Cover art was left out: this job's MP4 text subtitles or MOV data tracks are written without the Matroska stage that carries it.",
            ));
        }
        let args: Vec<OsString> = [
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-i",
        ]
        .into_iter()
        .map(OsString::from)
        .chain([input.as_os_str().to_owned()])
        .chain(
            [
                "-map".into(),
                format!("0:{index}"),
                "-c".into(),
                "copy".into(),
                "-frames:v".into(),
                "1".into(),
                "-f".into(),
                "image2pipe".into(),
                "pipe:1".into(),
            ]
            .map(OsString::from),
        )
        .collect();
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            MAX_BYTES,
            Duration::from_secs(60),
        )
        .await
        .map_err(|error| match super::process_error(error, input) {
            canceled if canceled.code == "JOB_CANCELED" => canceled,
            other => left_out(format!(
                "Cover art was left out: its picture could not be read ({}).",
                other.message
            )),
        })?;
        if !result.status.success() {
            return Err(left_out(format!(
                "Cover art was left out: its picture could not be read ({}).",
                String::from_utf8_lossy(&result.stderr).trim()
            )));
        }
        let picture = Picture::sniff(&result.stdout)
            .ok_or_else(|| left_out("Cover art was left out: it is not a JPEG or PNG picture."))?;
        let file = Temporary::create_extension(output, &format!("{id}-cover"), "img")?;
        let mut handle = file.clone_file()?;
        handle
            .write_all(&result.stdout)
            .and_then(|_| handle.sync_all())
            .map_err(|e| AppError::new("OUTPUT_WRITE_FAILED", e.to_string(), None))?;
        Ok(Some(Self { file, picture }))
    }

    /// Adds the cover as the next Matroska attachment of a mux command.
    pub fn apply(&self, args: &mut Vec<OsString>, existing_attachments: usize) {
        let output = args.pop().expect("mux output argument");
        args.extend([
            "-attach".into(),
            self.file.path.as_os_str().to_owned(),
            format!("-metadata:s:t:{existing_attachments}").into(),
            format!("filename={}", self.picture.file_name()).into(),
            format!("-metadata:s:t:{existing_attachments}").into(),
            format!("mimetype={}", self.picture.mime()).into(),
        ]);
        args.push(output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_jpeg_and_png_pictures_are_covers() {
        assert_eq!(
            Picture::sniff(&[0xff, 0xd8, 0xff, 0xe0, 0]),
            Some(Picture::Jpeg)
        );
        assert_eq!(Picture::sniff(b"\x89PNG\r\n\x1a\nrest"), Some(Picture::Png));
        assert_eq!(Picture::sniff(b"\0\0\0\x01h264"), None);
        assert_eq!(Picture::sniff(b""), None);
    }

    #[test]
    fn only_mkv_and_mp4_keep_covers() {
        assert!(supported(ContainerFormat::Matroska));
        assert!(supported(ContainerFormat::Mp4));
        for container in [
            ContainerFormat::Mov,
            ContainerFormat::Webm,
            ContainerFormat::M4a,
            ContainerFormat::Ogg,
        ] {
            assert!(!supported(container));
        }
    }
}
