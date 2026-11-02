//! Subtitle input identity and original-clock rendering for fixed image outputs.
use super::*;
use crate::jobs::{
    encode_plan::ImageFilterPlan,
    metadata::{Document, Stream},
};
use media_core::{ImageProcessingSettings, ImageSubtitleSource, VideoTrim};

pub(super) struct Burn {
    pub prepared: crate::jobs::subtitles::Prepared,
    source: Option<Source>,
}
impl Burn {
    pub(super) fn empty() -> Self {
        Self {
            prepared: crate::jobs::subtitles::Prepared::empty(),
            source: None,
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn build(
        primary: &Source,
        document: &Document,
        video: &Stream,
        processing: &ImageProcessingSettings,
        ffmpeg: &Path,
        probe: &Path,
        output: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Self, AppError> {
        let Some(selection) = &processing.subtitle_burn_in else {
            return Ok(Self {
                prepared: crate::jobs::subtitles::Prepared::empty(),
                source: None,
            });
        };
        let (origin, elapsed) =
            picture_clock(probe, &primary.path, video.index, processing.trim, cancel).await?;
        let source_clock = processing.source_filters
            || processing.tone_map.is_some_and(|tone| tone.source_workflow);
        let elapsed = if source_clock {
            processing
                .trim
                .and_then(|trim| trim.time)
                .filter(|time| time.seek_strategy == media_core::VideoSeekStrategy::InputSeek)
                .map_or(0.0, |time| f64::from(time.start_milliseconds) / 1000.0)
        } else {
            elapsed
        };
        let (external, index, offset) = match selection {
            ImageSubtitleSource::Primary { stream_index } => (None, *stream_index, 0.0),
            ImageSubtitleSource::External {
                input_path,
                stream_index,
                offset_milliseconds,
            } => {
                if !Path::new(input_path).is_absolute()
                    || input_path.contains('\0')
                    || i64::from(*offset_milliseconds).abs() > 86_400_000
                {
                    return Err(error(
                        "Use an absolute subtitle source with an offset within 24 hours.",
                        output,
                    ));
                }
                let source = Source::open(Path::new(input_path))?;
                if source.same_file(primary)? {
                    return Err(error(
                        "Choose subtitles from the primary source in its own track list.",
                        output,
                    ));
                }
                (
                    Some(source),
                    *stream_index,
                    f64::from(*offset_milliseconds) / 1000.0,
                )
            }
        };
        let external_document = if let Some(source) = &external {
            Some(crate::jobs::probe(probe, &source.path, cancel, Some(&[index])).await?)
        } else {
            None
        };
        let (input, document, clock) =
            if let (Some(source), Some(document)) = (&external, &external_document) {
                (source.path.as_path(), document, elapsed)
            } else {
                (
                    primary.path.as_path(),
                    document,
                    if source_clock {
                        elapsed
                    } else {
                        origin + elapsed
                    },
                )
            };
        let prepared = crate::jobs::subtitles::Prepared::image_burn(
            document,
            input,
            index,
            video,
            processing.tone_map.is_some(),
            offset,
            clock,
            ffmpeg,
            output,
            &id(),
            cancel,
        )
        .await?;
        primary.verify()?;
        if let Some(source) = &external {
            source.verify()?;
        }
        Ok(Self {
            prepared,
            source: external,
        })
    }
    pub fn verify(&self) -> Result<(), AppError> {
        if let Some(source) = &self.source {
            source.verify()?;
        }
        Ok(())
    }
    pub fn cwd(&self) -> Option<PathBuf> {
        self.prepared.decoder_cwd().map(Path::to_path_buf)
    }

    /// Add exactly one selected caption input and graph. All caller paths are OS
    /// arguments; only generated ASCII relative names enter libass expressions.
    pub fn decoder(
        &self,
        input: &Path,
        stream: u32,
        plan: &ImageFilterPlan,
        palette: Option<&Path>,
        palettegen: Option<u16>,
    ) -> Vec<OsString> {
        let mut a = args(&["-v", "error", "-nostdin", "-y"]);
        a.extend(plan.device_args.iter().cloned());
        let bitmap = self.prepared.bitmap_input();
        if bitmap.is_some() && !plan.source_clock {
            a.push("-copyts".into());
        }
        if let Some(seek) = &plan.input_seek {
            a.extend(args(&["-ss", seek]));
        }
        a.push("-i".into());
        a.push(input.as_os_str().to_owned());
        if let Some((path, _, offset)) = bitmap {
            if let Some(seek) = &plan.input_seek {
                a.extend(args(&["-ss", seek]));
            }
            let offset = offset
                + if plan.source_clock {
                    plan.input_seek
                        .as_deref()
                        .and_then(|value| value.parse::<f64>().ok())
                        .unwrap_or(0.0)
                } else {
                    0.0
                };
            a.extend(args(&[
                "-itsoffset",
                &format!("{offset:.9}"),
                "-protocol_whitelist",
                "file",
                "-i",
            ]));
            a.push(path.as_os_str().to_owned());
        }
        if let Some(path) = palette {
            a.push("-i".into());
            a.push(path.as_os_str().to_owned());
        }
        let mut graph = if let Some((_, index, _)) = bitmap {
            let suffix = if plan.suffix.is_empty() {
                String::new()
            } else {
                format!(",{}", plan.suffix)
            };
            if plan.source_clock {
                format!(
                    "[0:{stream}]{}[jesses_prepared];[jesses_prepared][1:{index}]overlay=shortest=1:format=auto{suffix}",
                    plan.prefix
                )
            } else {
                format!(
                    "[0:{stream}]{},settb=AVTB,setpts=PTS+1[jesses_prepared];[jesses_prepared][1:{index}]overlay=eof_action=pass:repeatlast=0:format=auto,setpts=PTS-1{suffix}",
                    plan.prefix
                )
            }
        } else {
            format!("[0:{stream}]{}", plan.filter.as_deref().unwrap_or("null"))
        };
        if let Some(colors) = palettegen {
            graph.push_str(&format!(",palettegen=max_colors={colors}"));
        }
        if palette.is_some() {
            let index = if bitmap.is_some() { 2 } else { 1 };
            graph.push_str(&format!(
                "[frames];[frames][{index}:v:0]paletteuse=dither=floyd_steinberg"
            ));
        }
        graph.push_str("[jesses_image]");
        a.extend(args(&["-filter_complex", &graph, "-map", "[jesses_image]"]));
        a.extend(plan.output_trim.iter().cloned());
        a
    }
    pub async fn scan(
        &self,
        ffmpeg: &Path,
        input: &Path,
        stream: u32,
        plan: &ImageFilterPlan,
        layout: ImagePixelFormat,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Pixels, AppError> {
        let mut a = self.decoder(input, stream, plan, None, None);
        a.extend(args(&[
            "-fps_mode",
            "passthrough",
            "-c:v",
            "rawvideo",
            "-pix_fmt",
            layout.ffmpeg(),
            "-threads",
            "2",
            "-f",
            "framemd5",
            "-",
        ]));
        scan_pixel_command(ffmpeg, a, self.cwd(), input, cancel).await
    }
}

/// Decode every timestamp with bounded parsing. A frame trim on VFR must use the
/// actual first retained frame, never startFrame / an average rate estimate.
async fn picture_clock(
    probe: &Path,
    input: &Path,
    stream: u32,
    trim: Option<VideoTrim>,
    cancel: &watch::Receiver<bool>,
) -> Result<(f64, f64), AppError> {
    #[derive(Deserialize)]
    struct Frame {
        best_effort_timestamp_time: Option<String>,
    }
    let mut a = args(&[
        "-v",
        "error",
        "-select_streams",
        &stream.to_string(),
        "-show_frames",
        "-show_entries",
        "frame=best_effort_timestamp_time",
        "-of",
        "json",
        "-i",
    ]);
    a.push(input.as_os_str().to_owned());
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: probe.to_owned(),
            args: a,
            cwd: None,
        },
        cancel.clone(),
        65536,
        Duration::from_secs(86400),
        move |reader| {
            let mut origin = None;
            let mut previous = None;
            let mut selected = None;
            let mut count = 0u64;
            let mut invalid = false;
            crate::jobs::encode::frame_scan::parse(reader, |frame: Frame| {
                let Some(time) = frame
                    .best_effort_timestamp_time
                    .as_deref()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|value| value.is_finite())
                else {
                    invalid = true;
                    return;
                };
                if previous.is_some_and(|value| time <= value) {
                    invalid = true;
                }
                previous = Some(time);
                let first = *origin.get_or_insert(time);
                let elapsed = time - first;
                let keep = trim.is_none_or(|trim| {
                    trim.time.map_or(
                        count >= u64::from(trim.start_frame)
                            && count < u64::from(trim.end_frame_exclusive),
                        |range| {
                            elapsed + 1e-9 >= f64::from(range.start_milliseconds) / 1000.0
                                && elapsed < f64::from(range.end_milliseconds) / 1000.0 - 1e-9
                        },
                    )
                });
                if keep && selected.is_none() {
                    selected = Some(elapsed);
                }
                count += 1;
            })?;
            if invalid {
                return Err(
                    "Subtitle rendering requires a finite, increasing decoded picture clock."
                        .into(),
                );
            }
            Ok((
                origin.ok_or("Source has no decoded picture clock.")?,
                selected.ok_or("The selected image interval contains no frames.")?,
            ))
        },
    )
    .await
    .map_err(|cause| process_error(cause, input))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(error(
            "Cannot read the complete picture clock for subtitle burn-in.",
            input,
        ));
    }
    check(cancel)?;
    Ok(result.value)
}
