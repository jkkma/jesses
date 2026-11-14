//! Bounded actual decoded color/HDR declarations for sparse source headers.
//!
//! Only the first decoded pictures are read. Their declarations become the
//! stream-level plan; the complete source frame scan still checks every
//! frame's color fields and static HDR metadata against that plan, so reading
//! the whole video here would only repeat that work single-threaded.
use super::*;

/// Most inputs produce pictures within this decode prefix.
const FIRST_PACKETS: u32 = 16;
/// An MP4 edit list can hide the whole prefix after a stream-copy cut. Allow
/// more preroll in that case, still bounding the work independently of length.
const MAX_PACKETS: u32 = 1024;

/// Side-data fields the color plan reads. Requesting all frame side data would
/// also print every Dolby Vision RPU and HDR10+ payload.
const SIDE_DATA_FIELDS: &str = "side_data_type,red_x,red_y,green_x,green_y,blue_x,blue_y,white_point_x,white_point_y,min_luminance,max_luminance,max_content,max_average";

/// Header-only FFprobe can omit decoded HEVC color and static HDR SEI. This
/// belongs to actual operation preparation; file-list loading stays cheap.
///
/// A tone-mapped (SDR) output does not need stream-level mastering metadata:
/// the filter graph renders from per-frame metadata and removes HDR side data,
/// and the frame scan still absorbs the first frame's static HDR declarations.
/// Missing color fields are still resolved, because they decide the transform.
pub(super) fn needs_enrichment(video: &super::metadata::Stream, tone_mapped: bool) -> bool {
    let missing = |value: &Option<String>| {
        value
            .as_deref()
            .is_none_or(|value| matches!(value, "unknown" | "unspecified"))
    };
    [
        &video.color_primaries,
        &video.color_transfer,
        &video.color_space,
        &video.color_range,
        &video.chroma_location,
    ]
    .into_iter()
    .any(missing)
        || (!tone_mapped
            && matches!(
                video.color_transfer.as_deref(),
                Some("smpte2084" | "arib-std-b67")
            )
            && !video.side_data_list.iter().any(|value| {
                value
                    .get("side_data_type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| kind.contains("Mastering display"))
            }))
}

fn enrich_args(input: &Path, index: u32, packets: u32) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-err_detect",
        "explode",
        "-protocol_whitelist",
        "file",
        "-select_streams",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(index.to_string().into());
    args.extend([
        format!("-threads:{index}").into(),
        "0".into(),
        "-read_intervals".into(),
        format!("%+#{packets}").into(),
        "-show_frames".into(),
        "-show_entries".into(),
        format!(
            "frame=color_primaries,color_transfer,color_space,color_range,chroma_location:frame_side_data={SIDE_DATA_FIELDS}"
        )
        .into(),
        "-of".into(),
        "json".into(),
        "-i".into(),
        input.as_os_str().to_owned(),
    ]);
    args
}

pub(super) async fn enrich(
    ffprobe: &Path,
    input: &Path,
    document: &mut Document,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let video = document
        .streams
        .iter_mut()
        .find(|s| s.index == index)
        .ok_or_else(|| {
            files::error(
                "STREAM_SELECTION_INVALID",
                "The chosen source picture is absent.",
                input,
            )
        })?;
    for packets in [FIRST_PACKETS, 256, MAX_PACKETS] {
        if let Some(enriched) = probe_prefix(ffprobe, input, video.clone(), packets, cancel).await?
        {
            *video = enriched;
            return Ok(());
        }
    }
    Err(process_error(
        SupervisorError::OutputParse(format!(
            "The source did not decode a picture for color planning within {MAX_PACKETS} packets."
        )),
        input,
    ))
}

async fn probe_prefix(
    ffprobe: &Path,
    input: &Path,
    mut video: super::metadata::Stream,
    packets: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<super::metadata::Stream>, AppError> {
    #[derive(serde::Deserialize)]
    struct Frame {
        color_primaries: Option<String>,
        color_transfer: Option<String>,
        color_space: Option<String>,
        color_range: Option<String>,
        chroma_location: Option<String>,
        #[serde(default)]
        side_data_list: Vec<serde_json::Value>,
    }
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args: enrich_args(input, video.index, packets),
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(10 * 60),
        move |reader| {
            let mut static_hdr =
                super::encode_plan::hdr::StaticMetadata::parse(&video.side_data_list)
                    .map_err(|e| e.message)?;
            let mut failure = None;
            let mut saw_picture = false;
            super::encode::frame_scan::parse::<Frame>(reader, |frame| {
                if failure.is_some() {
                    return;
                }
                let checked = (|| -> Result<(), String> {
                    for (header, actual) in [
                        (&mut video.color_primaries, &frame.color_primaries),
                        (&mut video.color_transfer, &frame.color_transfer),
                        (&mut video.color_space, &frame.color_space),
                        (&mut video.color_range, &frame.color_range),
                        (&mut video.chroma_location, &frame.chroma_location),
                    ] {
                        let known = |value: &Option<String>| {
                            value
                                .as_deref()
                                .is_some_and(|value| !matches!(value, "unknown" | "unspecified"))
                        };
                        if known(header) && known(actual) && header != actual {
                            return Err(
                                "Stream and decoded frame color declarations disagree.".into()
                            );
                        }
                        if !known(header) && known(actual) {
                            *header = actual.clone();
                        }
                    }
                    let actual =
                        super::encode_plan::hdr::StaticMetadata::parse(&frame.side_data_list)
                            .map_err(|e| e.message)?;
                    static_hdr
                        .absorb_first_frame(&actual)
                        .map_err(|e| e.message)?;
                    for value in frame.side_data_list {
                        let kind = value
                            .get("side_data_type")
                            .and_then(serde_json::Value::as_str);
                        if kind.is_some_and(|kind| {
                            kind.contains("Mastering display") || kind.contains("Content light")
                        }) && !video.side_data_list.iter().any(|v| {
                            v.get("side_data_type").and_then(serde_json::Value::as_str) == kind
                        }) {
                            video.side_data_list.push(value);
                        }
                    }
                    saw_picture = true;
                    Ok(())
                })();
                if let Err(cause) = checked {
                    failure = Some(cause);
                }
            })?;
            if let Some(failure) = failure {
                return Err(failure);
            }
            Ok(saw_picture.then_some(video))
        },
    )
    .await
    .map_err(|e| process_error(e, input))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(files::error(
            "ENCODE_INPUT_UNSUPPORTED",
            "The source picture declarations could not be decoded without errors.",
            input,
        ));
    }
    Ok(result.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(side_data: serde_json::Value) -> super::super::metadata::Stream {
        serde_json::from_value(serde_json::json!({
            "index": 0, "codec_type": "video", "codec_name": "hevc",
            "color_primaries": "bt2020", "color_transfer": "smpte2084",
            "color_space": "bt2020nc", "color_range": "tv", "chroma_location": "topleft",
            "side_data_list": side_data
        }))
        .unwrap()
    }

    #[test]
    fn only_sparse_headers_that_affect_the_plan_are_enriched() {
        let sparse = video(serde_json::json!([
            {"side_data_type": "DOVI configuration record", "dv_profile": 8}
        ]));
        assert!(needs_enrichment(&sparse, false));
        // SDR rendering does not use stream-level mastering metadata.
        assert!(!needs_enrichment(&sparse, true));
        let mut unknown = sparse.clone();
        unknown.color_range = Some("unknown".into());
        assert!(needs_enrichment(&unknown, true));
        let complete = video(serde_json::json!([
            {"side_data_type": "Mastering display metadata"}
        ]));
        assert!(!needs_enrichment(&complete, false));
    }

    #[test]
    fn declarations_come_from_a_threaded_bounded_prefix_without_rpu_payloads() {
        let args = enrich_args(Path::new("movie.mp4"), 3, FIRST_PACKETS)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let after = |flag: &str| {
            let position = args.iter().position(|arg| arg == flag).unwrap();
            args[position + 1].clone()
        };
        assert_eq!(after("-select_streams"), "3");
        assert_eq!(after("-threads:3"), "0");
        assert_eq!(after("-read_intervals"), "%+#16");
        let entries = after("-show_entries");
        assert!(entries.contains("frame_side_data=side_data_type,red_x"));
        assert!(entries.ends_with("max_content,max_average"));
        assert_eq!(args.last().unwrap(), "movie.mp4");
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg with libx264 and FFprobe"]
    async fn enrich_reads_past_mp4_edit_list_preroll() {
        let nonce = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir();
        let hint = directory.join("unused.mp4");
        let id = format!("picture-metadata-{}-{nonce}", std::process::id());
        let original = Temporary::create_extension(&hint, &id, "mp4").unwrap();
        let cut = Temporary::create_extension(&hint, &format!("{id}-cut"), "mp4").unwrap();
        let (_sender, cancel) = watch::channel(false);
        let ffmpeg = discover("ffmpeg", &cancel).await.unwrap();
        let ffprobe = discover("ffprobe", &cancel).await.unwrap();
        let ffmpeg_args = |args: &[&str]| args.iter().map(OsString::from).collect::<Vec<_>>();
        #[rustfmt::skip]
        let mut encode = ffmpeg_args(&[
            "-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", "testsrc2=s=64x48:r=60:d=8",
            "-c:v", "libx264", "-threads:v", "1", "-g", "480", "-sc_threshold", "0",
            "-colorspace", "bt709",
        ]);
        encode.push(original.path.as_os_str().to_owned());
        // This cut starts 360 frames into the GOP. The edit list discards the
        // preceding pictures, but the MP4 still contains their coded references.
        let mut remux = ffmpeg_args(&["-v", "error", "-nostdin", "-y", "-ss", "6", "-i"]);
        remux.push(original.path.as_os_str().to_owned());
        remux.extend(ffmpeg_args(&["-t", "1", "-c", "copy"]));
        remux.push(cut.path.as_os_str().to_owned());
        for args in [encode, remux] {
            let result = supervisor::run_capture(
                &CommandSpec {
                    executable: ffmpeg.clone(),
                    args,
                    cwd: None,
                },
                cancel.clone(),
                64 * 1024,
                Duration::from_secs(30),
            )
            .await
            .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }

        let mut document: Document = serde_json::from_value(serde_json::json!({
            "streams": [{"index": 0, "codec_type": "video", "codec_name": "h264"}]
        }))
        .unwrap();
        enrich(&ffprobe, &cut.path, &mut document, 0, &cancel)
            .await
            .unwrap();
        assert_eq!(document.streams[0].color_space.as_deref(), Some("bt709"));
    }
}
