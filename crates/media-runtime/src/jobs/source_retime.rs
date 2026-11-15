//! Compressed clock normalization for ordered AV1 samples in ISO media.
use super::*;
use std::io::BufRead;

pub(super) async fn frame_count(
    ffprobe: &Path,
    input: &Path,
    document: &Document,
    video: &metadata::Stream,
    frame_seconds: f64,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<u64>, AppError> {
    if !eligible(document, video) {
        return Ok(None);
    }
    let Some(frames) = video
        .nb_frames
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return Ok(None);
    };
    // Leave one millisecond for the staged Matroska clock's quantization.
    let tolerance = frame_seconds - 0.001;
    if frames < 2
        || tolerance <= 0.0
        || !metadata::track_duration(video)
            .is_some_and(|duration| (duration - frames as f64 * frame_seconds).abs() <= tolerance)
    {
        return Ok(None);
    }
    check_cancel(cancel)?;
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args: [
                "-v",
                "error",
                "-protocol_whitelist",
                "file",
                "-select_streams",
            ]
            .map(OsString::from)
            .into_iter()
            .chain([
                video.index.to_string().into(),
                "-show_entries".into(),
                "packet=pts_time,dts_time,duration_time,flags".into(),
                "-of".into(),
                "csv=p=0".into(),
                "-i".into(),
                input.as_os_str().to_owned(),
            ])
            .collect(),
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(60 * 60),
        move |reader| {
            ordered_clock(
                std::io::BufReader::new(reader),
                frame_seconds,
                tolerance,
                frames,
            )
        },
    )
    .await
    .map_err(|error| process_error(error, input))?;
    check_cancel(cancel)?;
    Ok((result.status.success() && result.stderr.is_empty() && result.value).then_some(frames))
}

fn eligible(document: &Document, video: &metadata::Stream) -> bool {
    // An AV1 sample in ISO BMFF carries one temporal unit with a single
    // presentation instant. FFmpeg reports the first output video's frame
    // count, so require one video stream, including attached pictures.
    // https://aomediacodec.github.io/av1-isobmff/#av1sampleformat
    video.codec_tag_string.as_deref() == Some("av01")
        && video.codec_name.as_deref() == Some("av1")
        && matches!(
            video.color_transfer.as_deref(),
            Some("bt709" | "smpte170m" | "bt470bg" | "iec61966-2-1")
        )
        && document
            .streams
            .iter()
            .filter(|stream| stream.codec_type.as_deref() == Some("video"))
            .count()
            == 1
        && document.format.as_ref().is_some_and(|format| {
            format
                .format_name
                .as_deref()
                .is_some_and(|names| names.split(',').any(|name| name == "mov"))
        })
}

fn ordered_clock(
    reader: impl BufRead,
    frame_seconds: f64,
    tolerance: f64,
    frames: u64,
) -> Result<bool, String> {
    let mut count = 0_u64;
    let mut previous = None;
    let mut valid = true;
    for line in reader.lines() {
        let line = line.map_err(|error| error.to_string())?;
        if !valid || line.is_empty() {
            // Drain stdout even when the stream is ineligible.
            continue;
        }
        let mut columns = line.split(',');
        let mut number = || columns.next().and_then(|value| value.parse::<f64>().ok());
        let (Some(pts), Some(dts), Some(duration), Some(flags)) =
            (number(), number(), number(), columns.next())
        else {
            valid = false;
            continue;
        };
        valid = pts.is_finite()
            && pts == dts
            && duration.is_finite()
            && duration > 0.0
            && !flags.contains(['C', 'D'])
            && previous.map_or(pts == 0.0 && flags.contains('K'), |last| pts > last)
            && (pts - count as f64 * frame_seconds).abs() <= tolerance
            && (count + 1 != frames
                || (pts + duration - frames as f64 * frame_seconds).abs() <= tolerance);
        previous = Some(pts);
        count += 1;
    }
    Ok(valid && count == frames)
}

pub(super) fn completed(result: &supervisor::CapturedOutput, frames: u64) -> bool {
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    result.status.success()
        && !stderr.contains("av1_frame_split")
        && stdout.lines().last() == Some("progress=end")
        && stdout
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("frame=")?.trim().parse::<u64>().ok())
            == Some(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_video_progress_can_prove_only_a_single_selected_video() {
        let mut document: Document = serde_json::from_value(serde_json::json!({
            "format": {"format_name":"mov,mp4,m4a,3gp,3g2,mj2"},
            "streams": [{"index":0,"codec_type":"video","codec_name":"av1","codec_tag_string":"av01","color_transfer":"bt709"}]
        })).unwrap();
        assert!(eligible(&document, &document.streams[0]));
        for attached_picture in [0, 1] {
            document.streams.push(serde_json::from_value(serde_json::json!({
                "index":1,"codec_type":"video","codec_name":"mjpeg","disposition":{"attached_pic":attached_picture}
            })).unwrap());
            assert!(!eligible(&document, &document.streams[0]));
            document.streams.pop();
        }
        document.streams[0].color_transfer = Some("smpte2084".into());
        assert!(!eligible(&document, &document.streams[0]));
    }

    #[test]
    fn retiming_requires_an_ordered_counted_clock_within_one_frame() {
        let valid = "0.000,0.000,0.030,K__\n0.030,0.030,0.050,___\n0.080,0.080,0.040,___\n";
        let check = |clock: &str| ordered_clock(clock.as_bytes(), 0.04, 0.039, 3).unwrap();
        assert!(check(valid));
        for bad in [
            valid.replace("K__", "___"),
            valid.replace("K__", "KD_"),
            valid.replace("0.030,0.030,0.050,___", "0.030,0.030,0.050,_C_"),
            valid.replace("0.030,0.030,0.050", "0.030,0.025,0.050"),
            valid.replace("0.080,0.080,0.040", "0.030,0.030,0.090"),
            valid.replace("0.080,0.080,0.040", "0.122,0.122,0.003"),
            valid.replace("0.080,0.080,0.040", "0.080,0.080,0.100"),
            format!("{valid}0.120,0.120,0.040,___\n"),
        ] {
            assert!(!check(&bad), "{bad}");
        }
    }
}
