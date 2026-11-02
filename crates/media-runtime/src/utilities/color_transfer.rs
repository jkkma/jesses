use super::*;
use media_core::{ColorMetadataInspectionRequest, ColorMetadataReport};
#[derive(Debug, Deserialize)]
struct MkvDocument {
    #[serde(default)]
    tracks: Vec<MkvTrack>,
    #[serde(default)]
    errors: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MkvTrack {
    id: u32,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    properties: BTreeMap<String, Value>,
}

async fn mkv_document(
    mkvmerge: &Path,
    path: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<MkvDocument, AppError> {
    let output = capture(
        mkvmerge.to_owned(),
        vec![
            "--ui-language".into(),
            "en_US".into(),
            "-J".into(),
            os(path),
        ],
        cancel,
        PROBE_LIMIT,
        Some(path),
    )
    .await?;
    let document: MkvDocument = serde_json::from_slice(&output.stdout).map_err(|cause| {
        error(
            "UTILITY_PROBE_INVALID",
            format!("mkvmerge returned invalid identification data: {cause}"),
            Some(path),
        )
    })?;
    if !output.status.success() || !document.errors.is_empty() {
        return Err(error(
            "UTILITY_PROBE_FAILED",
            format!(
                "mkvmerge could not identify this file: {} {}",
                document.errors.join("; "),
                diagnostic_tail(&output.stderr)
            ),
            Some(path),
        ));
    }
    Ok(document)
}

fn video_ordinal(
    document: &ProbeDocument,
    stream_index: u32,
    path: &Path,
) -> Result<usize, AppError> {
    let mut ordinal = 0;
    for stream in &document.streams {
        if stream.codec_type.as_deref() == Some("video") {
            if stream.index == stream_index {
                return Ok(ordinal);
            }
            ordinal += 1;
        }
    }
    Err(error(
        "UTILITY_STREAM_INVALID",
        format!("Stream {stream_index} is not a video stream."),
        Some(path),
    ))
}

fn mkv_video(document: &MkvDocument, ordinal: usize) -> Option<&MkvTrack> {
    document
        .tracks
        .iter()
        .filter(|track| track.kind == "video")
        .nth(ordinal)
}

fn mkv_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() && value.len() <= 256 => {
            Some(value.clone())
        }
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

const COLOR_PROPERTIES: [(&str, &str); 10] = [
    ("color_matrix_coefficients", "--colour-matrix"),
    (
        "color_transfer_characteristics",
        "--colour-transfer-characteristics",
    ),
    ("color_primaries", "--colour-primaries"),
    ("color_range", "--colour-range"),
    ("max_content_light", "--max-content-light"),
    ("max_frame_light", "--max-frame-light"),
    ("chromaticity_coordinates", "--chromaticity-coordinates"),
    ("white_color_coordinates", "--white-colour-coordinates"),
    ("max_luminance", "--max-luminance"),
    ("min_luminance", "--min-luminance"),
];

fn transferable_color(track: &MkvTrack) -> Vec<(&'static str, &'static str, String)> {
    COLOR_PROPERTIES
        .into_iter()
        .filter_map(|(property, option)| {
            mkv_value(track.properties.get(property)?).map(|value| (property, option, value))
        })
        .collect()
}

fn mkv_field_order(value: &str) -> Option<u8> {
    match value {
        "progressive" => Some(0),
        "tt" => Some(1),
        "unknown" => Some(2),
        "bb" => Some(6),
        "tb" => Some(9),
        "bt" => Some(14),
        _ => None,
    }
}

pub(super) async fn run_color_transfer(
    request: ColorMetadataTransferRequest,
    cancel: &watch::Receiver<bool>,
) -> Result<UtilityResult, AppError> {
    check_cancel(cancel)?;
    if request.in_place {
        let path = valid_absolute(&request.input_path)?;
        if fs::symlink_metadata(path)
            .map_err(|e| error("FILE_UNREADABLE", e.to_string(), Some(path)))?
            .file_type()
            .is_symlink()
        {
            return Err(error(
                "UTILITY_TARGET_SYMLINK",
                "Select the target file itself instead of a symbolic link for in-place metadata editing.",
                Some(path),
            ));
        }
    }
    let metadata_source = Source::open(valid_absolute(&request.metadata_source_path)?)?;
    let input = Source::open(valid_absolute(&request.input_path)?)?;
    if (!request.copy_color && !request.copy_hdr)
        || (request.in_place && metadata_source.same_file(&input)?)
    {
        return no_change(
            &input,
            &metadata_source,
            "No metadata change was requested; source and selected target were left unchanged.",
        );
    }
    let output = if request.in_place {
        input.path.clone()
    } else {
        destination(&request.output_path, &[&metadata_source, &input])?
    };
    if !request.in_place {
        require_mkv(&output, "Color metadata transfer")?;
    }
    let ffprobe = tool("ffprobe", "Color metadata transfer").await?;
    let ffmpeg = tool("ffmpeg", "Color metadata transfer packet verification").await?;
    let mkvmerge = tool("mkvmerge", "Color metadata transfer").await?;
    let source_probe = probe(&ffprobe, &metadata_source.path, cancel, false).await?;
    let input_probe = probe(&ffprobe, &input.path, cancel, false).await?;
    selected_stream(
        &source_probe,
        request.metadata_source_video_stream_index,
        "video",
        &metadata_source.path,
    )?;
    selected_stream(
        &input_probe,
        request.input_video_stream_index,
        "video",
        &input.path,
    )?;
    let source_ordinal = video_ordinal(
        &source_probe,
        request.metadata_source_video_stream_index,
        &metadata_source.path,
    )?;
    let target_ordinal =
        video_ordinal(&input_probe, request.input_video_stream_index, &input.path)?;
    let source_mkv = mkv_document(&mkvmerge, &metadata_source.path, cancel).await?;
    let target_mkv = mkv_document(&mkvmerge, &input.path, cancel).await?;
    let source_track = mkv_video(&source_mkv, source_ordinal).ok_or_else(|| {
        error(
            "UTILITY_STREAM_INVALID",
            "mkvmerge could not map the selected metadata-source video track.",
            Some(&metadata_source.path),
        )
    })?;
    let target_track = mkv_video(&target_mkv, target_ordinal).ok_or_else(|| {
        error(
            "UTILITY_STREAM_INVALID",
            "mkvmerge could not map the selected target video track.",
            Some(&input.path),
        )
    })?;
    let report = read_report(
        &metadata_source,
        request.metadata_source_video_stream_index,
        &ffprobe,
        cancel,
    )
    .await?;
    let properties = source_properties(source_track, &report, request.copy_color, request.copy_hdr);
    if properties.is_empty() {
        return no_change(
            &input,
            &metadata_source,
            "The donor carries none of the selected metadata; the target was left unchanged.",
        );
    }
    let input_report =
        read_report(&input, request.input_video_stream_index, &ffprobe, cancel).await?;
    let mut effective = source_properties(target_track, &input_report, true, true)
        .into_iter()
        .map(|(key, _, value)| (key, value))
        .collect::<BTreeMap<_, _>>();
    for (key, _, value) in &properties {
        effective.insert(*key, value.clone());
    }
    let before_hash = packet_hash(
        &ffmpeg,
        &input.path,
        request.input_video_stream_index,
        cancel,
    )
    .await?;
    let scratch = Scratch::create("color")?;
    let tool_path = scratch.path.join("color-transfer.mkv");
    let mut args = vec![
        "--ui-language".into(),
        "en_US".into(),
        "--no-date".into(),
        "-o".into(),
        os(&tool_path),
    ];
    for (property, option) in COLOR_PROPERTIES {
        if let Some(value) = effective.get(property) {
            args.push(option.into());
            args.push(format!("{}:{}", target_track.id, cli_property(value)).into());
        }
    }
    for (ordinal, stream) in input_probe
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .enumerate()
    {
        if let Some(order) = stream.field_order.as_deref().and_then(mkv_field_order)
            && let Some(track) = mkv_video(&target_mkv, ordinal)
        {
            args.push("--field-order".into());
            args.push(format!("{}:{order}", track.id).into());
        }
    }
    // Preserve original flat global tags instead of MKVToolNix's ISO
    // MOVIE/COMMENT-style reclassification, which changes their names.
    if let Some(format) = &input_probe.format {
        let tags = format
            .tags
            .iter()
            .filter(|(name, _)| {
                !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "major_brand" | "minor_version" | "compatible_brands" | "encoder"
                )
            })
            .map(|(key, value)| (key.to_ascii_uppercase(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        let path = scratch.path.join("global.xml");
        write_tags(&path, &tags)?;
        args.extend(["--global-tags".into(), os(&path), "--no-global-tags".into()]);
    }
    // MKVToolNix imports ISO track labels but omits other ISO stream tags.
    // Preserve the original declared tags explicitly through the staging file.
    for (ordinal, stream) in input_probe.streams.iter().enumerate() {
        if let Some(track) = target_mkv.tracks.get(ordinal) {
            let tags = stable_stream_tags(stream);
            if !tags.is_empty() {
                let path = scratch.path.join(format!("track-{ordinal}.xml"));
                write_tags(&path, &tags)?;
                args.extend([
                    "--tags".into(),
                    format!("{}:{}", track.id, path.display()).into(),
                ]);
            }
        }
    }
    args.push(os(&input.path));
    let tool_output = checked(
        mkvmerge.clone(),
        args,
        cancel,
        MEDIA_LIMIT,
        Some(&input.path),
        "Color metadata remux",
    )
    .await?;
    let output_probe = probe(&ffprobe, &tool_path, cancel, false).await?;
    compatible_streams_ignoring_color(&input_probe, &output_probe, &tool_path)?;
    let output_stream = output_probe
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .nth(target_ordinal)
        .ok_or_else(|| {
            error(
                "UTILITY_VALIDATION_FAILED",
                "The target video stream is missing from the remuxed output.",
                Some(&tool_path),
            )
        })?;
    let after_hash = packet_hash(&ffmpeg, &tool_path, output_stream.index, cancel).await?;
    if before_hash != after_hash {
        return Err(error(
            "UTILITY_VALIDATION_FAILED",
            "Encoded video packet data changed during metadata transfer. The output was not published.",
            Some(&tool_path),
        ));
    }
    let output_mkv = mkv_document(&mkvmerge, &tool_path, cancel).await?;
    let written_track = mkv_video(&output_mkv, target_ordinal).ok_or_else(|| {
        error(
            "UTILITY_VALIDATION_FAILED",
            "The remuxed output does not expose the target video track.",
            Some(&tool_path),
        )
    })?;
    for (property, _) in COLOR_PROPERTIES {
        let expected = effective.get(property).map(String::as_str);
        let actual = written_track.properties.get(property).and_then(mkv_value);
        if !equivalent_property(property, expected, actual.as_deref()) {
            return Err(error(
                "UTILITY_VALIDATION_FAILED",
                format!(
                    "Metadata property {property} changed unexpectedly: expected {expected:?}, found {actual:?}."
                ),
                Some(&tool_path),
            ));
        }
    }
    check_cancel(cancel)?;
    let identities = vec![fingerprint(&metadata_source)?, fingerprint(&input)?];
    let temporary = Temporary::create(&output, &nonce("color"))?;
    import_artifact(&tool_path, &temporary, cancel).await?;
    let mut converted = Vec::new();
    let candidate = crate::jobs::prepare_utility_container(
        &temporary,
        &output,
        &nonce("color-container"),
        cancel,
        &mut converted,
    )
    .await?;
    let final_document = mkv_document(&mkvmerge, &candidate.path, cancel).await?;
    let final_track = mkv_video(&final_document, target_ordinal).ok_or_else(|| {
        error(
            "UTILITY_VALIDATION_FAILED",
            "Final container lost the selected video.",
            Some(&candidate.path),
        )
    })?;
    let final_report = read_report(
        &Source::open(&candidate.path)?,
        output_stream.index,
        &ffprobe,
        cancel,
    )
    .await?;
    let final_properties = source_properties(final_track, &final_report, true, true)
        .into_iter()
        .map(|(key, _, value)| (key, value))
        .collect::<BTreeMap<_, _>>();
    for (property, _) in COLOR_PROPERTIES {
        let expected = effective.get(property).map(String::as_str);
        let actual = final_properties.get(property).cloned();
        if !equivalent_property(property, expected, actual.as_deref()) {
            return Err(error(
                "UTILITY_COLOR_CONTAINER_UNSUPPORTED",
                format!(
                    "The selected target format cannot retain {property} without re-encoding; expected {expected:?}, found {actual:?}. The original target was preserved."
                ),
                Some(&output),
            ));
        }
    }
    let final_probe = probe(&ffprobe, &candidate.path, cancel, false).await?;
    compatible_streams_ignoring_color(&input_probe, &final_probe, &candidate.path)?;
    compatible_container_metadata(&input_probe, &final_probe, &candidate.path)?;
    crate::jobs::verify_utility_packets(&ffprobe, &input.path, &candidate.path, cancel).await?;
    check_cancel(cancel)?;
    input.verify()?;
    metadata_source.verify()?;
    let bytes = fs::metadata(&candidate.path)
        .map_err(|e| {
            error(
                "UTILITY_VALIDATION_FAILED",
                e.to_string(),
                Some(&candidate.path),
            )
        })?
        .len();
    if request.in_place {
        let replacement = converted.pop().unwrap_or(temporary);
        replacement.replace_target(input, cancel)?;
    } else {
        let replacement = converted.pop().unwrap_or(temporary);
        publish_media(replacement, &output, &[&metadata_source, &input]).await?;
    }
    Ok(UtilityResult::Artifact(UtilityArtifact {
        operation: "colorMetadataTransfer".into(),
        output_path: output.to_string_lossy().into_owned(),
        size_bytes: bytes.to_string(),
        duration_seconds: duration(&output_probe),
        source_fingerprints: identities,
        message: format!(
            "Transferred {} supported color/HDR container properties. Encoded video packet SHA-256 is unchanged.",
            properties.len()
        ),
        diagnostics: [
            diagnostic_tail(&tool_output.stdout),
            diagnostic_tail(&tool_output.stderr),
        ]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect(),
    }))
}

pub async fn read_color_metadata(
    request: ColorMetadataInspectionRequest,
    cancel: watch::Receiver<bool>,
) -> Result<ColorMetadataReport, AppError> {
    let _permit = crate::analysis::permit(&cancel).await?;
    let source = Source::open(valid_absolute(&request.input_path)?)?;
    let ffprobe = tool("ffprobe", "Color metadata inspection").await?;
    read_report(&source, request.video_stream_index, &ffprobe, &cancel).await
}
async fn read_report(
    source: &Source,
    index: u32,
    ffprobe: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<ColorMetadataReport, AppError> {
    let header = checked(
        ffprobe.to_owned(),
        vec![
            "-v".into(),
            "error".into(),
            "-show_streams".into(),
            "-of".into(),
            "json".into(),
            "-i".into(),
            os(&source.path),
        ],
        cancel,
        PROBE_LIMIT,
        Some(&source.path),
        "Color metadata inspection",
    )
    .await?;
    let header: Value = serde_json::from_slice(&header.stdout)
        .map_err(|e| error("UTILITY_PROBE_INVALID", e.to_string(), Some(&source.path)))?;
    let streams = header["streams"].as_array().ok_or_else(|| {
        error(
            "UTILITY_PROBE_INVALID",
            "Missing source streams.",
            Some(&source.path),
        )
    })?;
    let stream = streams
        .iter()
        .find(|stream| {
            stream["index"].as_u64() == Some(u64::from(index)) && stream["codec_type"] == "video"
        })
        .ok_or_else(|| {
            error(
                "UTILITY_STREAM_INVALID",
                "Select a source video stream.",
                Some(&source.path),
            )
        })?;
    let frames=checked(ffprobe.to_owned(),vec!["-v".into(),"error".into(),"-select_streams".into(),index.to_string().into(),"-read_intervals".into(),"%+#32".into(),"-show_frames".into(),"-show_entries".into(),"frame=color_primaries,color_transfer,color_space,color_range:frame_side_data=side_data_type,red_x,red_y,green_x,green_y,blue_x,blue_y,white_point_x,white_point_y,max_luminance,min_luminance,max_content,max_average".into(),"-of".into(),"json".into(),"-i".into(),os(&source.path)],cancel,PROBE_LIMIT,Some(&source.path),"First-frame color metadata inspection").await?;
    let frames: Value = serde_json::from_slice(&frames.stdout)
        .map_err(|e| error("UTILITY_PROBE_INVALID", e.to_string(), Some(&source.path)))?;
    let first = frames["frames"]
        .as_array()
        .and_then(|frames| frames.first())
        .unwrap_or(&Value::Null);
    let mut color = BTreeMap::new();
    for (name, key) in [
        ("primaries", "color_primaries"),
        ("transfer", "color_transfer"),
        ("matrix", "color_space"),
        ("range", "color_range"),
    ] {
        let value = first
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !matches!(*value, "unknown" | "unspecified"))
            .or_else(|| stream.get(key).and_then(Value::as_str))
            .unwrap_or("unknown");
        color.insert(name.to_owned(), value.to_owned());
    }
    let mut hdr = BTreeMap::new();
    let mut dolby_vision_profile = None;
    let mut dolby_vision_compatibility = None;
    for value in stream
        .get("side_data_list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            first
                .get("side_data_list")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
    {
        match value["side_data_type"].as_str().unwrap_or("") {
            "Mastering display metadata" => {
                for key in [
                    "red_x",
                    "red_y",
                    "green_x",
                    "green_y",
                    "blue_x",
                    "blue_y",
                    "white_point_x",
                    "white_point_y",
                    "max_luminance",
                    "min_luminance",
                ] {
                    if let Some(value) = value.get(key).and_then(mkv_value) {
                        hdr.insert(key.into(), value);
                    }
                }
            }
            "Content light level metadata" => {
                for key in ["max_content", "max_average"] {
                    if let Some(value) = value.get(key).and_then(mkv_value) {
                        hdr.insert(key.into(), value);
                    }
                }
            }
            "DOVI configuration record" => {
                dolby_vision_profile = value["dv_profile"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok());
                dolby_vision_compatibility = value["dv_bl_signal_compatibility_id"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok());
            }
            _ => {}
        }
    }
    if let Ok(Some(mkvmerge)) = find_executable(&["mkvmerge"]).await
        && let Ok(document) = mkv_document(&mkvmerge, &source.path, cancel).await
    {
        let ordinal = streams
            .iter()
            .filter(|value| value["codec_type"] == "video")
            .position(|value| value["index"].as_u64() == Some(u64::from(index)))
            .unwrap_or(0);
        if let Some(track) = mkv_video(&document, ordinal) {
            for (property, key) in [
                ("color_primaries", "primaries"),
                ("color_transfer_characteristics", "transfer"),
                ("color_matrix_coefficients", "matrix"),
                ("color_range", "range"),
            ] {
                if let Some(value) = track.properties.get(property).and_then(mkv_value) {
                    let code = value.parse::<u32>().unwrap_or(2);
                    let name = match (key, code) {
                        ("primaries", 9) => "bt2020",
                        ("matrix", 9) => "bt2020nc",
                        ("transfer", 16) => "smpte2084",
                        ("transfer", 18) => "arib-std-b67",
                        ("range", 1) => "tv",
                        ("range", 2) => "pc",
                        (_, 1) => "bt709",
                        (_, 2 | 0) => "unknown",
                        _ => value.as_str(),
                    };
                    color.insert(key.into(), name.into());
                }
            }
            for (property, key) in [
                ("max_content_light", "max_content"),
                ("max_frame_light", "max_average"),
                ("max_luminance", "max_luminance"),
                ("min_luminance", "min_luminance"),
            ] {
                if let Some(value) = track.properties.get(property).and_then(mkv_value) {
                    hdr.insert(key.into(), value);
                }
            }
            for (property, keys) in [
                (
                    "chromaticity_coordinates",
                    vec!["red_x", "red_y", "green_x", "green_y", "blue_x", "blue_y"],
                ),
                (
                    "white_color_coordinates",
                    vec!["white_point_x", "white_point_y"],
                ),
            ] {
                if let Some(value) = track.properties.get(property).and_then(mkv_value) {
                    let values = value.split(',').collect::<Vec<_>>();
                    if values.len() == keys.len() {
                        for (key, value) in keys.into_iter().zip(values) {
                            hdr.insert(key.into(), value.into());
                        }
                    }
                }
            }
        }
    }
    check_cancel(cancel)?;
    let mut lines = vec![format!(
        "Color metadata: {} (video stream {index})",
        source
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    )];
    for (key, value) in &color {
        lines.push(format!("{key}: {value}"));
    }
    for (key, value) in &hdr {
        lines.push(format!("HDR {key}: {value}"));
    }
    if let Some(profile) = dolby_vision_profile {
        lines.push(format!(
            "Dolby Vision profile {profile}, base-layer compatibility {}",
            dolby_vision_compatibility.unwrap_or(0)
        ));
    }
    check_cancel(cancel)?;
    source.verify()?;
    Ok(ColorMetadataReport {
        input_path: source.path.to_string_lossy().into_owned(),
        video_stream_index: index,
        color,
        hdr,
        dolby_vision_profile,
        dolby_vision_compatibility,
        source_fingerprint: fingerprint(source)?,
        summary: lines.join("\n"),
    })
}
fn color_code(kind: &str, name: &str) -> u32 {
    if kind == "range" {
        return match name {
            "tv" => 1,
            "pc" => 2,
            _ => 0,
        };
    }
    match name {
        "bt709" => 1,
        "bt470m" | "gamma22" | "fcc" => 4,
        "bt470bg" | "gamma28" => 5,
        "smpte170m" => 6,
        "smpte240m" => 7,
        "film" | "linear" | "ycgco" => 8,
        "bt2020" | "bt2020nc" | "log" => 9,
        "bt2020c" | "log_sqrt" | "smpte428" if kind != "transfer" => 10,
        "log_sqrt" => 10,
        "smpte431" | "iec61966-2-4" | "smpte2085" => 11,
        "smpte432" | "bt1361e" | "chroma-derived-nc" => 12,
        "iec61966-2-1" | "chroma-derived-c" => 13,
        "bt2020-10" | "ictcp" => 14,
        "bt2020-12" => 15,
        "smpte2084" => 16,
        "smpte428" => 17,
        "arib-std-b67" => 18,
        "jedec-p22" => 22,
        "gbr" => 0,
        _ => 2,
    }
}
fn number(text: &str) -> Option<f64> {
    let value = if let Some((n, d)) = text.split_once('/') {
        n.parse::<f64>().ok()? / d.parse::<f64>().ok()?
    } else {
        text.parse().ok()?
    };
    value.is_finite().then_some(value)
}
fn source_properties(
    track: &MkvTrack,
    report: &ColorMetadataReport,
    copy_color: bool,
    copy_hdr: bool,
) -> Vec<(&'static str, &'static str, String)> {
    let mut properties = transferable_color(track)
        .into_iter()
        .map(|(property, _, value)| (property, value))
        .collect::<BTreeMap<_, _>>();
    for (property, key) in [
        ("color_primaries", "primaries"),
        ("color_transfer_characteristics", "transfer"),
        ("color_matrix_coefficients", "matrix"),
        ("color_range", "range"),
    ] {
        properties.entry(property).or_insert_with(|| {
            color_code(
                key,
                report
                    .color
                    .get(key)
                    .map(String::as_str)
                    .unwrap_or("unknown"),
            )
            .to_string()
        });
    }
    for (property, key) in [
        ("max_content_light", "max_content"),
        ("max_frame_light", "max_average"),
        ("max_luminance", "max_luminance"),
        ("min_luminance", "min_luminance"),
    ] {
        if let Some(value) = report.hdr.get(key).and_then(|value| number(value)) {
            properties
                .entry(property)
                .or_insert_with(|| value.to_string());
        }
    }
    for (property, keys) in [
        (
            "chromaticity_coordinates",
            vec!["red_x", "red_y", "green_x", "green_y", "blue_x", "blue_y"],
        ),
        (
            "white_color_coordinates",
            vec!["white_point_x", "white_point_y"],
        ),
    ] {
        let values = keys
            .iter()
            .map(|key| report.hdr.get(*key).and_then(|value| number(value)))
            .collect::<Option<Vec<_>>>();
        if let Some(values) = values {
            properties.entry(property).or_insert_with(|| {
                values
                    .iter()
                    .map(f64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            });
        }
    }
    COLOR_PROPERTIES
        .into_iter()
        .enumerate()
        .filter(|(index, _)| if *index < 4 { copy_color } else { copy_hdr })
        .filter_map(|(_, (property, option))| {
            properties
                .remove(property)
                .map(|value| (property, option, value))
        })
        .collect()
}
fn equivalent_property(property: &str, expected: Option<&str>, actual: Option<&str>) -> bool {
    if expected == actual {
        return true;
    }
    let default = match property {
        "color_matrix_coefficients" | "color_transfer_characteristics" | "color_primaries" => {
            Some("2")
        }
        "color_range" => Some("0"),
        _ => None,
    };
    let (Some(expected), Some(actual)) = (expected.or(default), actual.or(default)) else {
        return false;
    };
    let expected = expected.split(',').map(number).collect::<Option<Vec<_>>>();
    let actual = actual.split(',').map(number).collect::<Option<Vec<_>>>();
    match (expected, actual) {
        (Some(a), Some(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(a, b)| (a - b).abs() <= 1e-6 * a.abs().max(1.0))
        }
        _ => false,
    }
}
fn no_change(input: &Source, donor: &Source, message: &str) -> Result<UtilityResult, AppError> {
    input.verify()?;
    donor.verify()?;
    Ok(UtilityResult::Artifact(UtilityArtifact {
        operation: "colorMetadataTransfer".into(),
        output_path: input.path.to_string_lossy().into_owned(),
        size_bytes: fs::metadata(&input.path)
            .map_err(|e| error("UTILITY_SOURCE_INVALID", e.to_string(), Some(&input.path)))?
            .len()
            .to_string(),
        duration_seconds: None,
        source_fingerprints: vec![fingerprint(donor)?, fingerprint(input)?],
        message: message.into(),
        diagnostics: vec![],
    }))
}

// mkvmerge 82 parses long decimal mantissas incorrectly. Its EBML color values
// are float32; shortest float32 decimal strings round-trip those values exactly.
fn cli_property(value: &str) -> String {
    value
        .split(',')
        .map(|value| {
            number(value)
                .map(|value| (value as f32).to_string())
                .unwrap_or_else(|| value.to_owned())
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn write_tags(path: &Path, tags: &BTreeMap<String, String>) -> Result<(), AppError> {
    fn escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\"', "&quot;")
            .replace('\'', "&apos;")
    }
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Tags><Tag><Targets/>");
    for (key, value) in tags {
        if key
            .chars()
            .chain(value.chars())
            .any(|c| c < ' ' && !matches!(c, '\t' | '\r' | '\n'))
        {
            return Err(error(
                "UTILITY_METADATA_UNSUPPORTED",
                "This target contains control characters that Matroska XML tags cannot preserve.",
                Some(path),
            ));
        }
        xml.push_str(&format!(
            "<Simple><Name>{}</Name><String>{}</String></Simple>",
            escape(key),
            escape(value)
        ));
    }
    xml.push_str("</Tag></Tags>");
    fs::write(path, xml).map_err(|e| error("UTILITY_TEMPORARY_FAILED", e.to_string(), Some(path)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn float32_color_decimals_round_trip_without_mkvmerge_mantissa_overflow() {
        assert_eq!(cli_property("0.00009999999747378752"), "0.0001");
        assert_eq!(
            cli_property("0.6800000071525574,0.3199999928474426"),
            "0.68,0.32"
        );
        assert!(equivalent_property(
            "min_luminance",
            Some("0.00009999999747378752"),
            Some("0.0001")
        ));
        assert!(!equivalent_property(
            "min_luminance",
            Some("0.00009999999747378752"),
            Some("0.0012876178370788693")
        ));
    }
    #[test]
    fn unselected_or_unknown_hdr_cannot_be_invented_or_lost() {
        assert!(!equivalent_property("max_content_light", None, Some("99")));
        assert!(!equivalent_property("max_luminance", Some("2000"), None));
        assert!(equivalent_property("color_range", None, Some("0")));
    }
}
