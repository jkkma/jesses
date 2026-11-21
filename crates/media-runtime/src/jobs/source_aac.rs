//! Canonical Matroska requires ASC instead of ADTS transport framing.
use crate::jobs::metadata::{Document, Stream};

pub(in crate::jobs) fn needed(document: &Document, stream: &Stream) -> bool {
    stream.codec_name.as_deref() == Some("aac")
        && document
            .format
            .as_ref()
            .and_then(|format| format.format_name.as_deref())
            .is_some_and(|formats| {
                formats
                    .split(',')
                    .any(|format| matches!(format, "mpegts" | "aac"))
            })
}
