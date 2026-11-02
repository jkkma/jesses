//! The seven source menus, with input memory layout separate from decoded pixels.
use super::*;

pub(super) fn supported(encoder: LadderEncoder, pixel: &str) -> bool {
    let formats: &[&str] = match encoder {
        LadderEncoder::Av1 => &["yuv420p", "yuv420p10le"],
        LadderEncoder::H264 | LadderEncoder::Hevc | LadderEncoder::AomAv1 => &[
            "yuv420p",
            "yuv422p",
            "yuv444p",
            "yuv420p10le",
            "yuv422p10le",
            "yuv444p10le",
        ],
        LadderEncoder::Vp9 => &[
            "yuv420p",
            "yuv422p",
            "yuva420p",
            "yuv444p",
            "yuv420p10le",
            "yuv422p10le",
            "yuv444p10le",
        ],
        LadderEncoder::H264Nvenc => &[],
        LadderEncoder::HevcNvenc => &[],
    };
    formats.contains(&pixel)
}

pub(super) fn decoded(pixel: &str) -> &str {
    match pixel {
        "p010le" => "yuv420p10le",
        "nv12" => "yuv420p",
        other => other,
    }
}

pub(super) async fn check_help(
    ffmpeg: &Path,
    request: &CrfLadderRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let help = checked(
        ffmpeg.to_owned(),
        vec![
            "-hide_banner".into(),
            "-h".into(),
            format!("encoder={}", codec_name(request.encoder)).into(),
        ],
        cancel,
        Duration::from_secs(15),
        None,
        "Ladder encoder pixel-format inspection",
    )
    .await?;
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    if !help
        .lines()
        .find_map(|line| line.trim().strip_prefix("Supported pixel formats:"))
        .is_some_and(|line| {
            line.split_whitespace()
                .any(|format| format == request.pixel_format)
        })
    {
        return Err(error(
            "UTILITY_ENCODER_FORMAT_UNAVAILABLE",
            format!(
                "This {} build does not advertise the requested {} input format. It will not be replaced by another format.",
                codec_name(request.encoder),
                request.pixel_format
            ),
            Some(Path::new(&request.input_path)),
        ));
    }
    Ok(())
}

/// FFprobe's default VP9 decoder omits alpha. Force libvpx for that one codec,
/// then inspect every actual frame rather than trusting container pixel tags.
pub(super) async fn validate_decoded(
    ffprobe: &Path,
    path: &Path,
    pixel: &str,
    codec: Option<&str>,
    dimensions: (u32, u32),
    expected: Option<u64>,
    cancel: &watch::Receiver<bool>,
) -> Result<u64, AppError> {
    let mut args: Vec<OsString> = ["-v", "error", "-threads", "2", "-select_streams", "v:0"]
        .into_iter()
        .map(Into::into)
        .collect();
    if codec == Some("vp9") && pixel == "yuva420p" {
        args.extend(["-c:v".into(), "libvpx-vp9".into()]);
    }
    args.extend([
        "-show_frames".into(),
        "-show_entries".into(),
        "frame=width,height,pix_fmt".into(),
        "-of".into(),
        "compact=p=0:nk=0".into(),
        "-i".into(),
        os(path),
    ]);
    let pixel = decoded(pixel).to_owned();
    let scanned=run_streaming_stdout(&CommandSpec {executable:ffprobe.to_owned(),args,cwd:None},cancel.clone(),65536,MEDIA_LIMIT,move |reader| {
        let mut reader=BufReader::new(reader);let mut record=Vec::new();let mut count=0u64;
        loop {record.clear();let size=(&mut reader).take(1025).read_until(b'\n',&mut record).map_err(|cause|cause.to_string())?;if size==0 {break;}
        if size>1024{return Err("Decoded sample frame record exceeds its bound.".into());}
            let line=std::str::from_utf8(&record).map_err(|cause|cause.to_string())?.trim();if line.is_empty(){continue;}
            let fields=line.split('|').filter_map(|field|field.split_once('=')).collect::<BTreeMap<_,_>>();
            if fields.get("pix_fmt").copied()!=Some(pixel.as_str())||fields.get("width").and_then(|value|value.parse::<u32>().ok())!=Some(dimensions.0)||fields.get("height").and_then(|value|value.parse::<u32>().ok())!=Some(dimensions.1) {return Err(format!("Decoded sample changed requested dimensions/pixel format: {line}; expected {}x{} {pixel}.",dimensions.0,dimensions.1));}
            count=count.checked_add(1).ok_or("Decoded sample frame count overflow.")?;
        }
        if count==0||expected.is_some_and(|expected|expected!=count){return Err(format!("Decoded sample has {count} frames instead of {expected:?}."));}Ok(count)
    }).await.map_err(|cause|process_error(cause,Some(path)))?;
    if !scanned.status.success() || !scanned.stderr.is_empty() {
        return Err(error(
            "UTILITY_SAMPLE_DECODE_FAILED",
            String::from_utf8_lossy(&scanned.stderr),
            Some(path),
        ));
    }
    Ok(scanned.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_source_encoder_format_matrix_and_memory_layout_aliases() {
        let all = [
            "yuv420p",
            "yuv422p",
            "yuva420p",
            "yuv444p",
            "yuv420p10le",
            "yuv422p10le",
            "yuv444p10le",
            "p010le",
            "nv12",
        ];
        for (encoder, count) in [
            (LadderEncoder::Av1, 2),
            (LadderEncoder::AomAv1, 6),
            (LadderEncoder::Hevc, 6),
            (LadderEncoder::H264, 6),
            (LadderEncoder::Vp9, 7),
            (LadderEncoder::HevcNvenc, 0),
            (LadderEncoder::H264Nvenc, 0),
        ] {
            assert_eq!(
                all.iter()
                    .filter(|format| supported(encoder, format))
                    .count(),
                count
            );
            assert!(!supported(encoder, "nv12"));
        }
        assert_eq!(decoded("p010le"), "yuv420p10le");
        assert_eq!(decoded("nv12"), "yuv420p");
        assert_eq!(decoded("yuva420p"), "yuva420p");
    }
}
