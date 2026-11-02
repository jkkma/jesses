//! Selected attachment export; metadata filenames never become tool arguments.
use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek};

fn attachment_name(value: Option<&str>, index: u32) -> String {
    let name = value.unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
    let mut safe = name
        .chars()
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .take(180)
        .collect::<String>();
    safe = safe.trim_end_matches([' ', '.']).to_owned();
    if safe.is_empty() || safe == "." || safe == ".." {
        return format!("attachment-{index}.bin");
    }
    let stem = safe.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|s| {
                s.len() == 1 && matches!(s, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
    {
        safe.insert(0, '_');
    }
    safe
}

pub async fn extract_attachment(
    input_path: String,
    stream_index: u32,
    cancel: watch::Receiver<bool>,
) -> Result<String, AppError> {
    let _permit = crate::analysis::permit(&cancel).await?;
    check_cancel(&cancel)?;
    let source = Source::open(Path::new(&input_path))?;
    let ffprobe = discover("ffprobe", &cancel).await?;
    let ffmpeg = discover("ffmpeg", &cancel).await?;
    let document = probe(&ffprobe, &source.path, &cancel, None).await?;
    let stream = document
        .streams
        .iter()
        .find(|stream| {
            stream.index == stream_index && stream.codec_type.as_deref() == Some("attachment")
        })
        .ok_or_else(|| {
            AppError::new(
                "ATTACHMENT_INVALID",
                "Select one attachment track from this source.",
                Some(input_path.clone()),
            )
        })?;
    let expected = stream
        .extradata_hash
        .as_deref()
        .and_then(|hash| hash.strip_prefix("SHA256:"))
        .filter(|hash| hash.len() == 64)
        .ok_or_else(|| {
            AppError::new(
                "ATTACHMENT_INVALID",
                "FFprobe did not establish the selected attachment's complete SHA-256 identity.",
                Some(input_path.clone()),
            )
        })?;
    let filename = attachment_name(
        stream
            .tags
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("filename"))
            .map(|(_, value)| value.as_str()),
        stream_index,
    );
    let mut folder = source.path.as_os_str().to_owned();
    folder.push(" Attachments");
    let folder = PathBuf::from(folder);
    match std::fs::symlink_metadata(&folder) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(files::error(
                "ATTACHMENT_OUTPUT_INVALID",
                "The attachment output folder is an existing file or symbolic link.",
                &folder,
            ));
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&folder)
            .map_err(|cause| {
                files::error("ATTACHMENT_OUTPUT_INVALID", cause.to_string(), &folder)
            })?,
        Err(cause) => {
            return Err(files::error(
                "ATTACHMENT_OUTPUT_INVALID",
                cause.to_string(),
                &folder,
            ));
        }
    }
    let output = folder.join(filename);
    files::ensure_absent(&output)?;
    let mut temporary = Temporary::create_extension(
        &output,
        &format!("attachment-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed)),
        "bin",
    )?;
    temporary.close_for_path_writer()?;
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg,
            args: vec![
                "-v".into(),
                "error".into(),
                "-nostdin".into(),
                "-y".into(),
                format!("-dump_attachment:{stream_index}").into(),
                temporary.path.as_os_str().to_owned(),
                "-protocol_whitelist".into(),
                "file".into(),
                "-i".into(),
                source.path.as_os_str().to_owned(),
            ],
            cwd: None,
        },
        cancel.clone(),
        256 * 1024,
        Duration::from_secs(120),
    )
    .await;
    temporary.reopen_after_path_writer()?;
    result.map_err(|cause| process_error(cause, &source.path))?;
    // FFmpeg reports "no output specified" after successfully dumping header
    // attachments. Complete byte identity, rather than that exit code, decides.
    temporary.flush_nonempty_async().await?;
    let mut file = temporary.clone_file()?;
    file.rewind().map_err(|cause| {
        files::error("ATTACHMENT_READ_FAILED", cause.to_string(), &temporary.path)
    })?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let count = file.read(&mut bytes).map_err(|cause| {
            files::error("ATTACHMENT_READ_FAILED", cause.to_string(), &temporary.path)
        })?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    if !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected) {
        return Err(files::error(
            "ATTACHMENT_VALIDATION_FAILED",
            "The extracted bytes differ from the selected attachment.",
            &temporary.path,
        ));
    }
    source.verify()?;
    check_cancel(&cancel)?;
    temporary.publish(&output)?;
    temporary.cleanup()?;
    Ok(folder.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attachment_metadata_names_cannot_escape_or_target_reserved_devices() {
        for (input, expected) in [
            ("../../font.ttf", "font.ttf"),
            ("..\\..\\font.ttf", "font.ttf"),
            ("C:\\temp\\font:bad.ttf", "font_bad.ttf"),
            ("NUL.ttf", "_NUL.ttf"),
            ("..", "attachment-2.bin"),
            ("font\n.ttf", "font_.ttf"),
        ] {
            assert_eq!(attachment_name(Some(input), 2), expected);
        }
    }
}
