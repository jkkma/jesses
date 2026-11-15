use super::super::parameters::WINDOWS_PATMAN_RELEASE;
use super::{AppError, Temporary, files};
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const HEADER_SIZE: usize = 32;

pub(super) fn rewrites_count(identity: &str) -> bool {
    // These Windows builds were exercised with seekable output and fewer
    // input frames than requested. Other builds can leave the initial count
    // in the header, so it cannot be used as evidence of a mismatch.
    cfg!(windows)
        && identity.lines().map(str::trim).any(|line| {
            matches!(
                line,
                "SVT-AV1 v4.2.0 (release)"
                    | "SVT-AV1 [5fish] v2.3.260827 (release)"
                    | WINDOWS_PATMAN_RELEASE
            )
        })
}

pub(super) async fn verify(video: &Temporary, expected: usize) -> Result<(), AppError> {
    let file = video.clone_file()?;
    let path = video.path.clone();
    tokio::task::spawn_blocking(move || verify_file(file, &path, expected))
        .await
        .map_err(|cause| invalid(cause.to_string(), &video.path))?
}

fn invalid(message: impl Into<String>, path: &Path) -> AppError {
    files::error("ENCODE_VALIDATION_FAILED", message, path)
}

fn verify_file(mut file: impl Read + Seek, path: &Path, expected: usize) -> Result<(), AppError> {
    let read_error = |cause: std::io::Error| invalid(cause.to_string(), path);
    file.rewind().map_err(read_error)?;
    let mut header = [0; HEADER_SIZE];
    file.read_exact(&mut header).map_err(read_error)?;
    if &header[..12] != b"DKIF\x00\x00\x20\x00AV01" {
        return Err(invalid(
            "The SVT encoder did not write an AV1 IVF header.",
            path,
        ));
    }
    if file.seek(SeekFrom::End(0)).map_err(read_error)? >= 2 * HEADER_SIZE as u64 {
        file.seek(SeekFrom::End(-(HEADER_SIZE as i64)))
            .map_err(read_error)?;
        let mut tail = [0; HEADER_SIZE];
        file.read_exact(&mut tail).map_err(read_error)?;
        // A rewrite keeps every header field except the count. Match the
        // whole header around that field to distinguish it from payload.
        if tail[..24] == header[..24] && tail[28..] == header[28..] {
            return Err(invalid(
                "The SVT encoder appended its IVF header to the output.",
                path,
            ));
        }
    }
    let written = u32::from_le_bytes(header[24..28].try_into().expect("IVF frame count field"));
    if let Ok(expected) = u32::try_from(expected)
        && written != expected
    {
        return Err(invalid(
            format!("The SVT encoder wrote {written} frames, but {expected} were expected."),
            path,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn header(count: u32) -> [u8; HEADER_SIZE] {
        let mut header = [0; HEADER_SIZE];
        header[..12].copy_from_slice(b"DKIF\x00\x00\x20\x00AV01");
        header[24..28].copy_from_slice(&count.to_le_bytes());
        header
    }

    #[test]
    fn checks_final_count_and_rejects_an_appended_header() {
        let path = Path::new("video.ivf");
        let mut file = Vec::from(header(24));
        file.extend([1; 64]);
        verify_file(Cursor::new(&file), path, 24).unwrap();
        let error = verify_file(Cursor::new(&file), path, 25).unwrap_err();
        assert_eq!(error.code, "ENCODE_VALIDATION_FAILED");
        assert!(
            error
                .message
                .contains("wrote 24 frames, but 25 were expected")
        );
        file.extend(header(25));
        let error = verify_file(Cursor::new(&file), path, 24).unwrap_err();
        assert!(error.message.contains("appended its IVF header"));
        file[0] = b'M';
        let error = verify_file(Cursor::new(&file), path, 24).unwrap_err();
        assert!(error.message.contains("did not write an AV1 IVF header"));
    }

    #[test]
    fn only_live_qualified_identities_enable_the_count_check() {
        assert_eq!(
            rewrites_count("SVT-AV1 v4.2.0 (release)\r\n"),
            cfg!(windows)
        );
        assert_eq!(
            rewrites_count("\nSVT-AV1 [5fish] v2.3.260827 (release)"),
            cfg!(windows)
        );
        assert_eq!(
            rewrites_count(
                "SVT-AV1-HDR v4.1.0+77+85-e4b6c4ff5 [Mod by Patman] (release)\nHDR Release: N/A"
            ),
            cfg!(windows)
        );
        for unknown in [
            "SVT-AV1 v4.2.0 (debug)",
            "SVT-AV1 v4.2.1 (release)",
            "SVT-AV1-HDR v4.1.0-21-g00333404f (release)",
        ] {
            assert!(!rewrites_count(unknown), "accepted {unknown}");
        }
    }
}
