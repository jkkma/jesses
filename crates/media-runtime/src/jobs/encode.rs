pub(super) use super::encode_plan::validate_settings;
use super::encode_plan::{Plan, Validation};
#[path = "aom_vpx.rs"]
mod aom_vpx;
#[path = "command_plan.rs"]
mod command_plan;
#[path = "ffmpeg_video.rs"]
mod ffmpeg_video;
#[path = "frame_scan.rs"]
pub(super) mod frame_scan;
#[path = "x264.rs"]
mod x264;

pub(in crate::jobs) fn standalone_x264_arguments(
    plan: &Plan,
    settings: &EncodeSettings,
) -> Vec<OsString> {
    x264::arguments(plan, settings)
}
pub(in crate::jobs) fn standalone_aom_vpx_arguments(
    plan: &Plan,
    settings: &EncodeSettings,
) -> Vec<OsString> {
    aom_vpx::arguments(plan, settings)
}
#[path = "x265.rs"]
mod x265;
pub(in crate::jobs) fn standalone_x265_arguments(
    plan: &Plan,
    settings: &EncodeSettings,
) -> Vec<OsString> {
    x265::arguments(plan, settings)
}
use super::*;
pub use command_plan::preview_encode_plan;
use media_core::VideoEncoder;

// Construct large async state in a short-lived helper frame. Box::pin at the
// callsite can retain its unboxed construction temporary in the caller's debug
// poll frame even after the future lives on the heap.
#[inline(never)]
pub(super) fn boxed<F: std::future::Future>(make: impl FnOnce() -> F) -> std::pin::Pin<Box<F>> {
    Box::pin(make())
}

impl JobManager {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn encode(
        &self,
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        temporary: &mut Option<Temporary>,
        scratch: &mut Vec<Temporary>,
    ) -> Result<(), AppError> {
        // The decoded source scan may run beside the encoder. Its failure
        // raises this job-scoped cancellation, so the encoder stops early.
        let verification = super::source_timing::Verification::new(cancel);
        let job_cancel = verification.cancel();
        let execution = self.encode_mode(
            id,
            request,
            settings,
            &job_cancel,
            log_path,
            temporary,
            scratch,
            None,
        );
        let execution = async {
            if (settings.source_filters
                || settings.tone_map.is_some_and(|tone| tone.source_workflow))
                && settings.backend == media_core::EncodeBackend::Standalone
                && !settings.source_nvenc_quality
            {
                super::source_filters::with_observer(
                    super::source_filters::Observation::new(Path::new(&request.output_path)),
                    execution,
                )
                .await
            } else {
                execution.await
            }
        };
        verification.run(cancel, execution).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn encode_mode(
        &self,
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        temporary: &mut Option<Temporary>,
        scratch: &mut Vec<Temporary>,
        preview: Option<&mut media_core::EncodeCommandPlan>,
    ) -> Result<(), AppError> {
        // Keep independent pipelines in boxed futures. The video validator has
        // substantial debug state; placing conversion inside its poll frame
        // consumes the ordinary worker thread's stack before any media runs.
        if settings.video_mode != media_core::VideoMode::Encode {
            boxed(|| {
                self.convert(
                    id, request, settings, cancel, log_path, temporary, scratch, preview,
                )
            })
            .await
        } else if settings.source_trim
            && settings.trim.is_some()
            && settings.backend == media_core::EncodeBackend::Av1an
        {
            boxed(|| {
                self.encode_source_av1an_mode(
                    id, request, settings, cancel, log_path, temporary, scratch, preview,
                )
            })
            .await
        } else {
            boxed(|| {
                self.encode_source_mode(
                    id, request, settings, cancel, log_path, temporary, scratch, preview,
                )
            })
            .await
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn encode_source_av1an_mode(
        &self,
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        temporary: &mut Option<Temporary>,
        scratch: &mut Vec<Temporary>,
        mut preview: Option<&mut media_core::EncodeCommandPlan>,
    ) -> Result<(), AppError> {
        let prepared = super::source_normalization::av1an_trim::Prepared::prepare(
            self, id, request, settings, cancel, log_path,
        )
        .await?;
        let result = boxed(|| {
            self.encode_video_mode(
                id,
                &prepared.request,
                &prepared.settings,
                cancel,
                log_path,
                temporary,
                scratch,
                preview.as_deref_mut(),
                Some(&prepared.source_path),
                Some(&prepared.original),
                Some(settings),
                Some(request),
                Some(prepared.video_origin),
            )
        })
        .await;
        if let Some(preview) = preview {
            preview.source_fingerprint = crate::analysis::fingerprint(&prepared.original)?;
            preview.notes.insert(0,"Source AV1AN first stream-copies the configured section before command review, retaining keyframe preroll and the copied input's native stream clocks. The loaded original remains the naming, metadata and request authority.".into());
        }
        let keep = self
            .state
            .lock()
            .await
            .entries
            .iter()
            .find(|entry| entry.snapshot.id == id)
            .is_some_and(|entry| entry.snapshot.recovery.is_some());
        let cleanup = prepared.finish(keep);
        match (result, cleanup) {
            (Err(error), Err(cleanup)) => Err(AppError::new(
                error.code,
                format!("{} Source-cut cleanup: {}", error.message, cleanup.message),
                cleanup.path,
            )),
            (Err(error), _) => Err(error),
            (Ok(()), result) => result,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn encode_source_mode(
        &self,
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        temporary: &mut Option<Temporary>,
        scratch: &mut Vec<Temporary>,
        mut preview: Option<&mut media_core::EncodeCommandPlan>,
    ) -> Result<(), AppError> {
        self.phase(
            id,
            JobState::Preparing,
            "Inspecting selected streams and source clock.",
        )
        .await;
        let (original, document) =
            super::source_normalization::inspect(request, settings, cancel).await?;
        let existing = super::source_normalization::exists(id, request)?;
        let mut required = super::source_normalization::needs(&document, settings, false)?;
        if !existing && required {
            // A canonical source decodes the complete video. Check the selection
            // and destination against stream headers before that pass.
            let ffprobe = discover("ffprobe", cancel).await?;
            let output = files::output_path(request, &original)?;
            let headers = probe(
                &ffprobe,
                &original.path,
                cancel,
                Some(&request.stream_indices),
            )
            .await?;
            preflight_streams(
                &original, request, settings, &headers, &output, &ffprobe, cancel,
            )
            .await?;
        }
        if !existing && !required {
            let result = boxed(|| {
                self.encode_video_mode(
                    id,
                    request,
                    settings,
                    cancel,
                    log_path,
                    temporary,
                    scratch,
                    preview.as_deref_mut(),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            })
            .await;
            match &result {
                Err(error)
                    if error.code == "ENCODE_INPUT_UNSUPPORTED"
                        && (error
                            .message
                            .contains("Decoded frame timestamps are not constant-rate")
                            || error.message.contains("timelines that start at zero")) =>
                {
                    required = true;
                }
                _ => return result,
            }
            // A rejected source clock has not entered the encoder. Remove its
            // owned capability-probe assets before the normalized retry.
            for temporary in scratch.iter_mut() {
                temporary.cleanup()?;
            }
            scratch.clear();
            if let Some(mut stale) = temporary.take() {
                stale.cleanup()?;
            }
        }
        if existing {
            let recoverable = self
                .state
                .lock()
                .await
                .entries
                .iter()
                .find(|entry| entry.snapshot.id == id)
                .is_some_and(|entry| {
                    entry.snapshot.recovery.is_some()
                        || entry.snapshot.standalone_recovery.is_some()
                });
            if !recoverable {
                return Err(files::error(
                    "OUTPUT_EXISTS",
                    "A canonical source workspace already exists without an admitted recovery locator; it was preserved.",
                    Path::new(&request.output_path),
                ));
            }
        }
        let prepared = Box::pin(super::source_normalization::Prepared::prepare(
            self,
            id,
            request,
            settings,
            original,
            required,
            cancel,
            &log_path.with_extension("source-normalization.log"),
        ))
        .await?;
        let effective = prepared.effective_settings(settings);
        let result = boxed(|| {
            self.encode_video_mode(
                id,
                request,
                &effective,
                cancel,
                log_path,
                temporary,
                scratch,
                preview.as_deref_mut(),
                Some(&prepared.source_path),
                Some(&prepared.original),
                Some(settings),
                None,
                None,
            )
        })
        .await;
        if let Some(preview) = preview {
            preview.source_fingerprint = crate::analysis::fingerprint(&prepared.original)?;
            preview.notes.insert(0,"A verified lossless canonical source applies display rotation, rebases the selected video origin and explicitly normalizes variable timestamps to a constant source clock. Original request paths remain the immutable authority; both complete source identities are checked for recovery.".into());
        }
        let keep = self
            .state
            .lock()
            .await
            .entries
            .iter()
            .find(|entry| entry.snapshot.id == id)
            .is_some_and(|entry| {
                entry.snapshot.recovery.is_some() || entry.snapshot.standalone_recovery.is_some()
            });
        let cleanup = prepared.finish(keep);
        match (result, cleanup) {
            (Err(error), Err(cleanup)) => Err(AppError::new(
                error.code,
                format!(
                    "{} Canonical source cleanup: {}",
                    error.message, cleanup.message
                ),
                cleanup.path,
            )),
            (Err(error), _) => Err(error),
            (Ok(()), result) => result,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn encode_video_mode(
        &self,
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        temporary: &mut Option<Temporary>,
        scratch: &mut Vec<Temporary>,
        preview: Option<&mut media_core::EncodeCommandPlan>,
        normalized_source: Option<&Path>,
        original_guard: Option<&Source>,
        authority_settings: Option<&EncodeSettings>,
        authority_request: Option<&RemuxRequest>,
        source_origin: Option<f64>,
    ) -> Result<(), AppError> {
        check_cancel(cancel)?;
        let attempt_id = if settings.backend == media_core::EncodeBackend::Av1an {
            format!(
                "{id}-attempt-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            )
        } else {
            id.to_owned()
        };
        self.phase(
            id,
            JobState::Preparing,
            "Inspecting selected streams and encoder capabilities.",
        )
        .await;
        let input =
            normalized_source.map_or_else(|| PathBuf::from(&request.input_path), Path::to_path_buf);
        let owned_request = request.clone();
        let (source, output) = tokio::task::spawn_blocking(move || {
            let source = Source::open(&input)?;
            let output = files::output_path(&owned_request, &source)?;
            Ok::<_, AppError>((source, output))
        })
        .await
        .map_err(|e| AppError::new("PREFLIGHT_FAILED", e.to_string(), None))??;
        if settings.output_pixel_format == Some(media_core::Av1anPixelFormat::Yuva420p)
            && !matches!(
                container::format(&output)?,
                media_core::ContainerFormat::Matroska | media_core::ContainerFormat::Webm
            )
        {
            return Err(AppError::new(
                "CONTAINER_INCOMPATIBLE",
                "VP9 alpha output requires a Matroska (.mkv) or WebM (.webm) destination.",
                None,
            ));
        }
        audio::validate_gain_source(&source, settings)?;
        if settings
            .av1an_options
            .is_some_and(|options| options.attach_settings)
            && container::format(&output)? != media_core::ContainerFormat::Matroska
        {
            return Err(AppError::new(
                "CONTAINER_INCOMPATIBLE",
                "Encode settings attachments require a Matroska (.mkv) destination.",
                None,
            ));
        }
        check_cancel(cancel)?;
        let ffmpeg = discover("ffmpeg", cancel).await?;
        let ffprobe = discover("ffprobe", cancel).await?;
        let encoder = crate::discovery::find_video_encoder(settings.encoder)
            .await
            .map_err(|e| AppError::new("TOOL_DISCOVERY_FAILED", e, None))?
            .ok_or_else(|| {
                AppError::new(
                    "TOOL_MISSING",
                    format!(
                        "{} was not found. Check its separate entry in Tools.",
                        settings.encoder.name()
                    ),
                    None,
                )
            })?;
        let mkvmerge = if matches!(
            settings.encoder,
            VideoEncoder::X265Standalone | VideoEncoder::VpxStandalone
        ) || (settings.backend == media_core::EncodeBackend::Av1an
            && super::av1an::encoder::uses_mkvmerge(settings.encoder, settings))
        {
            Some(discover("mkvmerge", cancel).await.map_err(|_| {
                AppError::new(
                    "TOOL_MISSING",
                    "The selected encoder or av1an concat method requires mkvmerge. Install MKVToolNix and refresh Tools.",
                    None,
                )
            })?)
        } else {
            None
        };
        for (path, version_arg) in [
            (&ffmpeg, "-version"),
            (&ffprobe, "-version"),
            (
                &encoder,
                match settings.encoder {
                    VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone => "--help",
                    encoder if encoder.is_ffmpeg() => "-version",
                    _ => "--version",
                },
            ),
        ] {
            let version = supervisor::run_capture(
                &CommandSpec {
                    executable: path.clone(),
                    args: vec![version_arg.into()],
                    cwd: None,
                },
                cancel.clone(),
                64 * 1024,
                Duration::from_secs(5),
            )
            .await
            .map_err(|e| process_error(e, path))?;
            if !version.status.success() {
                return Err(files::error(
                    "TOOL_FAILED",
                    "A selected media tool failed its version check.",
                    path,
                ));
            }
            if path == &encoder {
                let identity = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&version.stdout),
                    String::from_utf8_lossy(&version.stderr)
                );
                crate::discovery::validate_video_encoder_version(settings.encoder, &identity)
                    .map_err(|message| files::error("ENCODER_BUILD_MISMATCH", message, path))?;
                super::rate_control::validate_encoder_version(settings, &identity).map_err(
                    |message| files::error("ENCODER_CAPABILITY_UNSUPPORTED", message, path),
                )?;
            }
            let bytes = if version.stdout.is_empty() {
                &version.stderr
            } else {
                &version.stdout
            };
            let version = String::from_utf8_lossy(bytes)
                .lines()
                .find(|line| !line.is_empty())
                .unwrap_or("unknown version")
                .to_owned();
            self.change(id, |snapshot| {
                append_log(snapshot, format!("Tool: {} — {version}", path.display()))
            })
            .await;
        }
        let mut document = probe(
            &ffprobe,
            &source.path,
            cancel,
            Some(&request.stream_indices),
        )
        .await?;
        // Run every header-only check before any decode pass, so an impossible
        // destination fails in seconds rather than after a frame-by-frame scan.
        // The checks repeat below against the final decoded-color metadata.
        let external = preflight_streams(
            &source, request, settings, &document, &output, &ffprobe, cancel,
        )
        .await?;
        if document
            .streams
            .iter()
            .find(|stream| stream.index == settings.video_stream_index)
            .is_some_and(|video| {
                super::source_picture_metadata::needs_enrichment(video, settings.tone_map.is_some())
            })
        {
            self.change(id, |snapshot| {
                append_log(snapshot, "Reading the first decoded pictures for color and static HDR declarations omitted by stream headers.".into())
            })
            .await;
            Box::pin(super::source_picture_metadata::enrich(
                &ffprobe,
                &source.path,
                &mut document,
                settings.video_stream_index,
                cancel,
            ))
            .await?;
            source.verify()?;
        }
        let source_dolby = settings.tone_map.is_some_and(|tone| tone.source_workflow)
            && document
                .streams
                .iter()
                .find(|stream| stream.index == settings.video_stream_index)
                .is_some_and(|video| {
                    super::encode_plan::preserves_dolby_source_clock(video, settings)
                });
        let mut source_settings = settings.clone();
        if source_dolby {
            source_settings.source_filters = true;
        }
        let authority_settings = authority_settings.or(source_dolby.then_some(settings));
        let settings = if source_dolby {
            &source_settings
        } else {
            settings
        };
        let selected = document.selected(&request.stream_indices)?;
        let mut output_document = external.extend_document(&document, settings);
        let mux_settings = external.mux_settings(settings);
        let preflight_indices =
            external.output_indices(&document, request.stream_indices.clone(), settings)?;
        let preflight_selected = output_document.selected(&preflight_indices)?;
        audio::validate_selection(&preflight_selected, &mux_settings)?;
        super::trim::validate_selection(&preflight_selected, &mux_settings)?;
        super::subtitles::validate_selection(&preflight_selected, &mux_settings)?;
        container::preflight(
            &output,
            &output_document,
            &preflight_selected,
            Some(&mux_settings),
        )?;
        let mut planning_document = document.clone();
        if source_origin.is_some()
            && let Some(video) = planning_document
                .streams
                .iter_mut()
                .find(|stream| stream.index == settings.video_stream_index)
        {
            video.start_time = Some("0".into());
            video.packet_start_time = Some(0.0);
        }
        let mut plan = if source_origin.is_some() {
            Plan::build_source_cut(
                &planning_document,
                &planning_document.selected(&request.stream_indices)?,
                settings,
            )?
        } else {
            Plan::build(
                &planning_document,
                &planning_document.selected(&request.stream_indices)?,
                settings,
            )?
        };
        if let Some(source_origin) = source_origin {
            plan.source_origin = source_origin;
        }
        plan.source_cut_ordinals = source_origin.is_some();
        let video = selected
            .iter()
            .find(|stream| stream.index == plan.video_index)
            .expect("validated video");
        for note in plan.source_interpretation_notes(video) {
            self.change(id, |snapshot| append_log(snapshot, note)).await;
        }

        let mkvmerge = if settings.encoder == VideoEncoder::AomAv1
            && plan.is_hdr10()
            && mkvmerge.is_none()
        {
            Some(discover("mkvmerge", cancel).await.map_err(|_| {
                AppError::new(
                    "TOOL_MISSING",
                    "Preserving HDR10 with AOM requires mkvmerge to carry mastering and content-light metadata. Install MKVToolNix and refresh Tools.",
                    None,
                )
            })?)
        } else {
            mkvmerge
        };
        if settings.encoder == VideoEncoder::AomAv1 && plan.is_hdr10() {
            let tool = mkvmerge.as_ref().expect("HDR10 AOM requires mkvmerge");
            let result = supervisor::run_capture(
                &CommandSpec {
                    executable: tool.clone(),
                    args: vec!["--help".into()],
                    cwd: None,
                },
                cancel.clone(),
                256 * 1024,
                Duration::from_secs(5),
            )
            .await
            .map_err(|error| process_error(error, tool))?;
            let help = String::from_utf8_lossy(&result.stdout);
            if !result.status.success()
                || [
                    "--chromaticity-coordinates",
                    "--white-color-coordinates",
                    "--max-luminance",
                    "--min-luminance",
                    "--max-content-light",
                    "--max-frame-light",
                ]
                .iter()
                .any(|flag| !help.contains(flag))
            {
                return Err(files::error(
                    "ENCODER_CAPABILITY_UNSUPPORTED",
                    "The selected mkvmerge cannot carry HDR10 mastering and content-light metadata for AOM output.",
                    tool,
                ));
            }
        }
        plan.check_temporal_tools(&ffmpeg, cancel).await?;
        audio::check_encoders(&ffmpeg, &mux_settings, cancel).await?;
        if audio::converted(&mux_settings).any(|track| {
            matches!(
                track.codec,
                media_core::AudioCodec::Aac | media_core::AudioCodec::Opus
            )
        }) {
            self.phase(
                id,
                JobState::Preparing,
                "Checking audio codec delay with the installed FFmpeg and FFprobe.",
            )
            .await;
            scratch.push(Temporary::create(
                &output,
                &format!("{attempt_id}-audio-tools"),
            )?);
            audio::check_delay_support(
                &ffmpeg,
                &ffprobe,
                scratch.last().expect("owned audio tool probe"),
                cancel,
            )
            .await?;
        }
        let mut audio_timelines = Vec::new();
        // A trim keeps audio only up to its end (its timeline is clipped below);
        // decoding the rest of a long source track would be wasted work.
        let trim_audio_until = settings
            .trim
            .filter(|_| plan.fps_num > 0 && plan.fps_den > 0)
            .map(|trim| {
                trim.time.map_or_else(
                    || {
                        f64::from(trim.end_frame_exclusive) * f64::from(plan.fps_den)
                            / f64::from(plan.fps_num)
                    },
                    |time| f64::from(time.end_milliseconds) / 1_000.0,
                ) + audio::TRIM_AUDIO_MARGIN_SECONDS
            });
        for track in audio::converted(&mux_settings) {
            let (audio_path, source_track) = external
                .audio_source(track.stream_index)
                .unwrap_or_else(|| {
                    (
                        source.path.as_path(),
                        *selected
                            .iter()
                            .find(|stream| stream.index == track.stream_index)
                            .expect("validated audio selection"),
                    )
                });
            if track.codec != media_core::AudioCodec::Copy {
                self.phase(id, JobState::Preparing, "Checking selected audio settings, codec delay, and final padding with the installed tools.").await;
                scratch.push(Temporary::create(
                    &output,
                    &format!("{attempt_id}-audio-tools-{}", track.stream_index),
                )?);
                audio::check_conversion_support(
                    &ffmpeg,
                    &ffprobe,
                    scratch.last().expect("owned audio tool probe"),
                    source_track,
                    track,
                    cancel,
                )
                .await?;
            }
            // External tracks keep their complete scan: their bounds set the
            // expected output duration below.
            let until =
                trim_audio_until.filter(|_| external.audio_source(track.stream_index).is_none());
            self.phase(
                id,
                JobState::Preparing,
                if until.is_some() {
                    "Decoding selected audio through the trim end to verify its sample timeline."
                } else {
                    "Decoding selected audio to verify its complete sample timeline."
                },
            )
            .await;
            let timeline = match until {
                Some(seconds) => {
                    audio::scan_until(&ffprobe, audio_path, source_track, seconds, cancel).await?
                }
                None => audio::scan(&ffprobe, audio_path, source_track, false, cancel).await?,
            }
            .shifted(external.offset_seconds(track.stream_index));
            if external.audio_source(track.stream_index).is_some() {
                let (start, end) = timeline.bounds();
                let expected = output_document
                    .streams
                    .iter_mut()
                    .find(|stream| stream.index == track.stream_index)
                    .expect("prepared converted external stream");
                expected.packet_start_time = Some(start);
                expected.duration = Some(end.to_string());
            }
            audio_timelines.push((track, timeline));
        }
        if settings.backend == media_core::EncodeBackend::Av1an {
            super::av1an::validate_encoder_path(&encoder, settings)?;
            super::av1an::validate_input(&document, &plan)?;
        }
        if !settings.encoder.is_svt() {
            check_encoder_capabilities(&encoder, &output, &plan, settings, cancel).await?;
        }
        let video = selected
            .iter()
            .find(|stream| stream.index == plan.video_index)
            .expect("validated video selection");
        let declared_rate = (plan.fps_num, plan.fps_den);
        let frame_count = self
            .source_frames(
                id,
                &ffprobe,
                &source.path,
                &mut plan,
                video,
                document.duration(),
                settings.trim,
                preview.is_some(),
                cancel,
            )
            .await?;
        let dynamic_hdr =
            super::dynamic_hdr::Resources::prepare(settings, frame_count, cancel).await?;
        external.validate_timecode_clock(plan.fps_num, plan.fps_den, frame_count)?;
        // Container duration may include a longer audio or subtitle track.
        // Peak sampling belongs to the selected, fully scanned source video.
        let source_video_duration = frame_count as f64 * plan.frame_seconds();
        if source_origin.is_some()
            && let Some(video) = output_document
                .streams
                .iter_mut()
                .find(|stream| stream.index == plan.video_index)
        {
            video.start_time = Some("0".into());
            video.packet_start_time = Some(0.0);
            video.duration = Some(source_video_duration.to_string());
            video
                .tags
                .retain(|key, _| !metadata::is_derived_stream_tag(key));
        }
        // An explicit Vulkan graph remains editable on a machine without its
        // device. Probe physical execution only after unchanged confirmation.
        let deferred_vulkan =
            self.command_review(id).await.is_some() && plan.requires_vulkan_tone_map();
        if !deferred_vulkan {
            plan.resolve_tone_map(&ffmpeg, &source.path, Some(source_video_duration), cancel)
                .await?;
        }
        if !deferred_vulkan && let Some(description) = plan.tone_map_description() {
            self.change(id, |snapshot| append_log(snapshot, description))
                .await;
        }
        if declared_rate != (plan.fps_num, plan.fps_den) {
            self.change(id, |snapshot| {
                append_log(snapshot, format!(
                    "Source timestamps follow {}/{} fps; reconciled declared {}/{} fps after checking all {frame_count} frames within {:.3} ms.",
                    plan.fps_num, plan.fps_den, declared_rate.0, declared_rate.1,
                    plan.tolerance * 1000.0,
                ));
            }).await;
        }
        if settings.encoder.is_svt() {
            check_encoder_capabilities(&encoder, &output, &plan, settings, cancel).await?;
        }
        if super::parameters::requires_serial_svt(settings) {
            self.change(id, |snapshot| append_log(snapshot, "SVT high-bit-depth mode decision plus overlays uses one processing lane (lp=1), within the selected worker limit, to avoid the observed SVT4.2 parallel short-clip crash. Both requested controls remain enabled.".into())).await;
        }
        let interval = settings
            .trim
            .map(|trim| super::trim::Interval::build(trim, frame_count, plan.fps_num, plan.fps_den))
            .transpose()?;
        let mut audio_filters = std::collections::BTreeMap::new();
        if let Some(interval) = interval {
            for (track, timeline) in &mut audio_timelines {
                let (clipped, filter) = timeline.clipped(interval.start, interval.end)?;
                *timeline = clipped;
                audio_filters.insert(track.stream_index, filter);
            }
        }
        let normalization = audio::Normalization::prepare(
            &ffmpeg,
            &source.path,
            &output_document.selected(&preflight_indices)?,
            &mux_settings,
            &external,
            &audio_filters,
            cancel,
        )
        .await?;
        for evidence in normalization.evidence() {
            self.change(id, |snapshot| append_log(snapshot, evidence))
                .await;
        }
        // Current MP4/MOV muxing loses AAC priming for positive track origins.
        // Check the effective decoded start after any trim has rebased it, and
        // fail before encoding rather than publish an extra leading AAC frame.
        if audio_timelines.iter().any(|(track, timeline)| {
            track.codec == media_core::AudioCodec::Aac && timeline.bounds().0 > 0.002
        }) && matches!(
            container::format(&output)?,
            media_core::ContainerFormat::Mp4 | media_core::ContainerFormat::Mov
        ) {
            return Err(AppError::new(
                "AUDIO_CONTAINER_UNSUPPORTED",
                "This MP4/MOV workflow cannot preserve AAC priming when the audio starts after zero. Choose Matroska to keep the original audio timing, or choose another audio codec.",
                None,
            ));
        }
        let source_frame_count = interval.map_or(frame_count, |interval| interval.frames);
        if plan.requires_exact_duplicate_scan() {
            self.phase(
                id,
                JobState::Preparing,
                "Characterizing exact decoded-frame duplicates for guarded cadence repair.",
            )
            .await;
            let report = super::cadence::scan(
                &ffmpeg,
                &source.path,
                plan.video_index,
                interval.map(|value| (value.source_start_frame, value.source_end_frame)),
                source_frame_count,
                cancel,
            )
            .await?;
            self.change(id, |snapshot| append_log(snapshot, report.summary()))
                .await;
            plan.set_exact_duplicate_count(source_frame_count, report.unique_frames)?;
        }
        let transformed_frame_count = plan.activate_temporal(source_frame_count)?;
        let mut frame_count = if settings.backend == media_core::EncodeBackend::Av1an
            && (settings.source_filters
                || settings
                    .av1an_options
                    .is_some_and(|options| options.source_workflow))
        {
            source_frame_count
        } else {
            transformed_frame_count
        };
        let combined_selected = output_document.selected(&preflight_indices)?;
        let prepared_trim = if let Some(interval) = interval {
            self.phase(
                id,
                JobState::Preparing,
                "Preparing clipped subtitle cues and chapters for the validated frame interval.",
            )
            .await;
            Some(
                super::trim::Prepared::build(
                    &mux_settings,
                    interval,
                    &output_document,
                    &combined_selected,
                    &external,
                    &ffmpeg,
                    &source.path,
                    &output,
                    &attempt_id,
                    scratch,
                    cancel,
                )
                .await?,
            )
        } else {
            None
        };
        let subtitle_assets = super::subtitles::Prepared::build(
            &mux_settings,
            &document,
            &combined_selected,
            prepared_trim.as_ref(),
            &external,
            &ffmpeg,
            &source.path,
            &output,
            &attempt_id,
            cancel,
        )
        .await?;
        let output_indices = subtitle_assets.effective_indices(&combined_selected);
        let output_selected = output_document.selected(&output_indices)?;
        let explicit_defaults_off = output_selected
            .iter()
            .filter(|stream| {
                external
                    .track_metadata(stream.index, settings)
                    .is_some_and(|options| options.default == Some(false))
            })
            .map(|stream| stream.index)
            .collect::<Vec<_>>();
        container::preflight_explicit_defaults(&output, &output_selected, &explicit_defaults_off)?;
        let measured_nonvideo = if matches!(
            settings.rate_control,
            Some(
                media_core::VideoRateControl::TargetSize { .. }
                    | media_core::VideoRateControl::TargetSizeDecimal { .. }
            )
        ) {
            self.phase(
                id,
                JobState::Preparing,
                "Measuring selected audio, subtitles, and attachments for the target file size.",
            )
            .await;
            let budget_selected = output_selected
                .iter()
                .copied()
                .filter(|stream| {
                    stream.codec_type.as_deref() != Some("video")
                        || stream.index == plan.video_index
                })
                .collect::<Vec<_>>();
            let dummy = Temporary::create(&output, &format!("{attempt_id}-size-video"))?;
            let measured = Temporary::create(&output, &format!("{attempt_id}-size-media"))?;
            let make_dummy = supervisor::run_capture(
                &CommandSpec {
                    executable: ffmpeg.clone(),
                    args: [
                        "-v",
                        "error",
                        "-nostdin",
                        "-f",
                        "lavfi",
                        "-i",
                        "color=s=64x64:r=24",
                        "-frames:v",
                        "1",
                        "-c:v",
                        "ffv1",
                        "-f",
                        "matroska",
                        "-y",
                    ]
                    .into_iter()
                    .map(OsString::from)
                    .chain([dummy.path.as_os_str().to_owned()])
                    .collect(),
                    cwd: None,
                },
                cancel.clone(),
                64 * 1024,
                Duration::from_secs(30),
            )
            .await
            .map_err(|e| process_error(e, &dummy.path))?;
            if !make_dummy.status.success() {
                return Err(files::error(
                    "TARGET_SIZE_FAILED",
                    "Could not create the temporary video placeholder for measuring selected media.",
                    &dummy.path,
                ));
            }
            dummy.flush_nonempty_async().await?;
            let mut args = mux_args(
                &source.path,
                &dummy.path,
                &measured.path,
                &budget_selected,
                &plan,
                settings,
                &external,
            );
            if let Some(prepared) = &prepared_trim {
                prepared.apply_mux(&mut args, &budget_selected, &audio_filters)?;
            }
            normalization.apply_mux(&mut args, &budget_selected)?;
            subtitle_assets.apply_mux(&mut args, &budget_selected);
            keep_last_codec_options(&mut args);
            tokio::fs::create_dir_all(self.log_dir.as_ref())
                .await
                .map_err(|e| {
                    files::error("LOG_CREATE_FAILED", e.to_string(), self.log_dir.as_ref())
                })?;
            let (sender, events) = mpsc::channel(256);
            let observer = self.observe_encode(id, events, None);
            let measured_log = log_path.with_extension("size.log");
            self.change(id, |snapshot| {
                snapshot.log_path = Some(measured_log.to_string_lossy().into_owned())
            })
            .await;
            let result = supervisor::run(
                &CommandSpec {
                    executable: ffmpeg.clone(),
                    args,
                    cwd: None,
                },
                cancel.clone(),
                sender,
                &measured_log,
                Duration::from_secs(24 * 60 * 60),
            )
            .await;
            let _ = observer.await;
            let result = result.map_err(|e| process_error(e, &source.path))?;
            if !result.status.success() {
                return Err(files::error(
                    "TARGET_SIZE_FAILED",
                    "Selected media could not be measured for the target file size.",
                    &measured.path,
                ));
            }
            measured.flush_nonempty_async().await?;
            let bytes = measured
                .clone_file()?
                .metadata()
                .map_err(|e| files::error("OUTPUT_UNREADABLE", e.to_string(), &measured.path))?
                .len();
            self.change(id, |snapshot| append_log(snapshot, format!("Target-size budget: measured {bytes} bytes for selected non-video media, its Matroska overhead, and a conservative one-frame placeholder. Container reserve: 1% of target plus 65536 bytes. Measurement log: {}", measured_log.display()))).await;
            bytes
        } else {
            0
        };
        let rate = super::rate_control::Rate::resolve(
            settings,
            frame_count as f64 * plan.frame_seconds(),
            measured_nonvideo,
        )?;

        if preview.is_none() {
            tokio::fs::create_dir_all(self.log_dir.as_ref())
                .await
                .map_err(|e| {
                    files::error("LOG_CREATE_FAILED", e.to_string(), self.log_dir.as_ref())
                })?;
        }
        source.verify()?;
        check_cancel(cancel)?;
        let source_qtgmc = settings.source_filters
            && settings.backend == media_core::EncodeBackend::Standalone
            && plan.requires_qtgmc();
        let source_qtgmc_checkpoint_ready = source_qtgmc
            && self
                .state
                .lock()
                .await
                .entries
                .iter()
                .find(|entry| entry.snapshot.id == id)
                .and_then(|entry| entry.snapshot.standalone_recovery.as_ref())
                .is_some_and(|recovery| {
                    matches!(
                        recovery.phase,
                        media_core::StandaloneRecoveryPhase::VideoComplete
                            | media_core::StandaloneRecoveryPhase::TimingWrapComplete
                            | media_core::StandaloneRecoveryPhase::Finalizing
                    )
                });
        let defer_source_qtgmc = source_qtgmc
            && !source_qtgmc_checkpoint_ready
            && self.command_review(id).await.is_some();
        let mut qtgmc = boxed(||async {Ok::<_,AppError>(if plan.requires_qtgmc() {
            self.phase(
                id,
                JobState::Preparing,
                "Preparing a verified lossless QTGMC intermediate with the configured VapourSynth runtime.",
            )
            .await;
            let qtgmc_plan=if settings.source_filters && settings.backend==media_core::EncodeBackend::Av1an {plan.without_source_filter_rows()}else{plan.clone()};
            let prepared = super::qtgmc::Prepared::build(
                &source.path,
                &output,
                &attempt_id,
                &qtgmc_plan,
                interval.map(|value| {
                    (
                        u32::try_from(value.source_start_frame).expect("validated frame bound"),
                        u32::try_from(value.source_end_frame).expect("validated frame bound"),
                    )
                }),
                &subtitle_assets,
                &ffmpeg,
                cancel,
            )
            .await?;
            if preview.is_none() && !defer_source_qtgmc && !source_qtgmc_checkpoint_ready {
                let prep_log = log_path.with_extension("qtgmc.log");
                self.change(id, |snapshot| {
                    snapshot.log_path = Some(prep_log.to_string_lossy().into_owned());
                    append_log(
                        snapshot,
                        format!(
                            "QTGMC uses one owned FFV1 intermediate for every encoder pass. Preparation log: {}",
                            prep_log.display()
                        ),
                    );
                })
                .await;
                let (sender, events) = mpsc::channel(256);
                let observer = self.observe_encode(id, events, Some(plan.frame_seconds()));
                let result = prepared.run(cancel.clone(), sender, &prep_log).await;
                let _ = observer.await;
                result?;
                if settings.source_filters {frame_count=boxed(||prepared.validate_source_filters(&ffprobe,&plan,cancel)).await?;}
                else {prepared.validate(&ffprobe, &plan, frame_count, cancel).await?;}
                source.verify()?;
            }
            Some(prepared)
        } else {
            None
        })}).await?;
        self.change(id,|snapshot| {
            snapshot.duration_seconds = Some(frame_count as f64*plan.frame_seconds());
            snapshot.progress_seconds = None;
            append_log(snapshot,format!("{} {frame_count} progressive CFR frames at {}/{} fps; {}-bit {}, {}, preset {}.",if super::source_timing::pending() {"Will encode"} else {"Validated"},plan.fps_num,plan.fps_den,plan.output_bit_depth(),plan.output_codec(),rate.as_ref().map_or_else(|| format!("CRF {}", settings.crf), |rate| format!("target {} kb/s", rate.kbps)),settings.preset));
            if settings.encoder.is_svt() {
                append_log(snapshot, format!("Film grain synthesis {} (denoising off).", settings.film_grain));
            }
            match settings.encoder {
                VideoEncoder::SvtAv1FiveFish => append_log(snapshot, format!("SVT-AV1 5fish: line-art bias {}, texture bias {}.", settings.lineart_psy_bias, settings.texture_psy_bias)),
                VideoEncoder::SvtAv1Hdr => append_log(snapshot, format!("SVT-AV1-HDR: {} tune.", settings.parameters.iter().find(|parameter| parameter.name == "tune").map_or_else(|| match settings.hdr_tune { media_core::HdrTune::VisualQuality => "visual quality (0)".to_string(), media_core::HdrTune::FilmGrain => "film grain retention (5)".to_string() }, |parameter| format!("explicit override ({})", parameter.value)))),
                _ => {},
            }
            if plan.is_hdr10() {
                append_log(snapshot, "HDR10: preserving BT.2020/PQ color and validated static mastering/content light metadata.".into());
                if settings.hdr10_fallback { append_log(snapshot, "HDR10 fallback enabled: Dolby Vision enhancement data and HDR10+ dynamic metadata are discarded.".into()); }
            }
        }).await;
        if let Some(preview) = preview {
            command_plan::build(
                preview,
                &source,
                &output,
                &ffmpeg,
                &ffprobe,
                &encoder,
                &plan,
                settings,
                &output_selected,
                &external,
                prepared_trim.as_ref(),
                interval.map(|value| {
                    (
                        value.source_start_frame as u32,
                        value.source_end_frame as u32,
                    )
                }),
                qtgmc.as_ref(),
                &subtitle_assets,
                &audio_filters,
                &normalization,
                rate.as_ref(),
                frame_count,
                &attempt_id,
                cancel,
            )
            .await?;
            source.verify()?;
            external.verify()?;
            return check_cancel(cancel);
        }
        let qtgmc_identity = if !source_qtgmc && let Some(prepared) = &qtgmc {
            self.phase(
                id,
                JobState::Preparing,
                "Fingerprinting every decoded QTGMC frame for safe recovery reuse.",
            )
            .await;
            Some(
                prepared
                    .decoded_identity(&ffmpeg, frame_count, cancel)
                    .await?,
            )
        } else {
            None
        };
        let av1an_prepared = if settings.backend == media_core::EncodeBackend::Av1an
            && qtgmc.is_none()
            && plan.requires_av1an_preprocess()
        {
            if subtitle_assets.bitmap_input().is_some() {
                return Err(AppError::new(
                    "AV1AN_PREPROCESS_SUBTITLE_UNSUPPORTED",
                    "av1an cannot burn a bitmap subtitle while preparing its verified lossless processed source. Copy the subtitle track or use standalone encoding.",
                    None,
                ));
            }
            self.phase(
                id,
                JobState::Preparing,
                "Preparing one verified lossless processed source for av1an scene detection, chunk encoding, and quality references.",
            )
            .await;
            let prepared = super::av1an_preprocess::Prepared::build(
                &source.path,
                &output,
                &attempt_id,
                &plan,
                &ffmpeg,
                interval.map(|value| {
                    (
                        u32::try_from(value.source_start_frame).expect("validated frame bound"),
                        u32::try_from(value.source_end_frame).expect("validated frame bound"),
                    )
                }),
                subtitle_assets.text_filter(),
                cancel,
            )
            .await?;
            let preparation_log = log_path.with_extension("av1an-preprocess.log");
            self.change(id, |snapshot| {
                snapshot.log_path = Some(preparation_log.to_string_lossy().into_owned());
                append_log(
                    snapshot,
                    format!(
                        "av1an processed-source preparation log: {}",
                        preparation_log.display()
                    ),
                );
            })
            .await;
            let (sender, events) = mpsc::channel(256);
            let observer = self.observe_encode(id, events, Some(plan.frame_seconds()));
            let result = prepared.run(cancel.clone(), sender, &preparation_log).await;
            let _ = observer.await;
            result?;
            prepared
                .validate(&ffprobe, &plan, frame_count, cancel)
                .await?;
            let identity = prepared
                .decoded_identity(&ffmpeg, frame_count, cancel)
                .await?;
            source.verify()?;
            Some((prepared, identity))
        } else {
            None
        };
        let mut recovery = None;
        let mut av1an_executable = None;
        let mut av1an_input = None;
        let mut standalone_recovery = None;
        let mut standalone_stats = None;
        let mut standalone_timed = None;
        let mut standalone_final = None;
        let durable = if settings.backend == media_core::EncodeBackend::Av1an {
            let executable = discover("av1an", cancel).await?;
            let locator = self
                .state
                .lock()
                .await
                .entries
                .iter()
                .find(|entry| entry.snapshot.id == id)
                .and_then(|entry| entry.snapshot.recovery.clone());
            let prepared_source = if let (Some(prepared), Some(identity)) =
                (qtgmc.as_ref(), qtgmc_identity.as_ref())
            {
                Some(av1an::PreparedSource {
                    path: prepared.video.path.clone(),
                    decoded_identity: identity.clone(),
                })
            } else {
                av1an_prepared
                    .as_ref()
                    .map(|(prepared, identity)| av1an::PreparedSource {
                        path: prepared.video.path.clone(),
                        decoded_identity: identity.clone(),
                    })
            };
            let mut tools = vec![
                ffmpeg.clone(),
                ffprobe.clone(),
                encoder.clone(),
                executable.clone(),
            ];
            if let Some(mkvmerge) = &mkvmerge {
                tools.push(mkvmerge.clone());
            }
            if let Some(qtgmc) = &qtgmc {
                tools.push(qtgmc.producer.executable.clone());
            }
            let (workspace, intermediate, stable_input) = av1an::Recovery::prepare(
                id,
                authority_request.unwrap_or(request),
                authority_settings.unwrap_or(settings),
                &source.path,
                prepared_source,
                tools,
                &plan,
                declared_rate,
                frame_count,
                locator,
                cancel,
            )
            .await?;
            let summary = workspace.summary().await?;
            self.change(id, |snapshot| snapshot.recovery = Some(summary))
                .await;
            check_cancel(cancel)?;
            av1an_executable = Some(executable);
            av1an_input = Some(stable_input);
            recovery = Some(workspace);
            Some(intermediate)
        } else {
            let locator = self
                .state
                .lock()
                .await
                .entries
                .iter()
                .find(|entry| entry.snapshot.id == id)
                .and_then(|entry| entry.snapshot.standalone_recovery.clone());
            let two_pass = rate.as_ref().is_some_and(|value| value.two_pass);
            let mut arguments = consumer_arguments(&plan, settings);
            if let Some(rate) = &rate {
                rate.arguments_for_settings(&mut arguments, settings, if two_pass { 2 } else { 1 });
            }
            let recovery_frames = if source_qtgmc {
                transformed_frame_count
            } else {
                frame_count
            };
            let mut signature = arguments
                .into_iter()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            signature.extend([
                format!("frames={recovery_frames}"),
                format!("fps={}/{}", plan.fps_num, plan.fps_den),
                format!("size={}x{}", plan.width, plan.height),
                format!("sar={}", plan.output_sar()),
                format!("decoder={:?}", plan.decoder_filter()),
                format!("post-qtgmc={:?}", plan.post_qtgmc_filter_with_text(None)),
            ]);
            let mut tools = vec![ffmpeg.clone(), ffprobe.clone(), encoder.clone()];
            if let Some(mkvmerge) = &mkvmerge {
                tools.push(mkvmerge.clone());
            }
            if let Some(qtgmc) = &qtgmc {
                tools.push(qtgmc.producer.executable.clone());
                if source_qtgmc {
                    signature.push("qtgmc-source-filter=deferred-render-v1".into());
                } else {
                    signature.push(format!(
                        "qtgmc-decoded={}",
                        qtgmc_identity
                            .as_deref()
                            .expect("executed QTGMC decoded identity")
                    ));
                }
            }
            let extension = match settings.encoder {
                VideoEncoder::SvtAv1
                | VideoEncoder::SvtAv1FiveFish
                | VideoEncoder::SvtAv1Hdr
                | VideoEncoder::AomAv1
                | VideoEncoder::VpxStandalone => "ivf",
                VideoEncoder::X265Standalone => "hevc",
                _ => "mkv",
            };
            // Hashing a large source takes a while before the encoder starts.
            let source_bytes = tokio::fs::metadata(&source.path)
                .await
                .map_or(0, |metadata| metadata.len());
            self.phase(
                id,
                JobState::Preparing,
                &format!(
                    "Fingerprinting the {:.1} GB source and the tools so an interrupted encode can resume safely.",
                    source_bytes as f64 / 1_000_000_000.0
                ),
            )
            .await;
            let prepared = super::standalone_recovery::Recovery::prepare(
                id,
                request,
                authority_settings.unwrap_or(settings),
                &source.path,
                tools,
                signature,
                recovery_frames,
                two_pass,
                extension,
                locator,
                cancel,
            )
            .await?;
            let super::standalone_recovery::Prepared {
                recovery: workspace,
                video,
                timed_video,
                final_stage,
                stats,
            } = prepared;
            if let Some(summary) = workspace.summary() {
                self.change(id, |snapshot| snapshot.standalone_recovery = Some(summary))
                    .await;
            }
            standalone_stats = stats;
            standalone_timed = timed_video;
            standalone_final = final_stage;
            standalone_recovery = Some(workspace);
            video
        };
        // A resumed final stage is a committed recovery checkpoint, not an
        // attempt owned by execute's unconditional cleanup. Keep its durable
        // owner local so another cancellation releases the handle while
        // preserving the checkpoint for the next explicit resume.
        let source_final_iso =
            container::source_iso_format(&output, &output_selected, &mux_settings);
        let staging_format = source_final_iso
            .unwrap_or_else(|| container::staging_format(&output_selected, Some(&mux_settings)));
        let mut source_iso_receipt = None;
        if standalone_final.is_none() {
            *temporary = Some(if let Some(recovery) = recovery.as_ref() {
                // A forced host exit cannot run Temporary::drop. Keep av1an's final
                // mux attempt inside the identity-guarded recovery workspace so a
                // verified successful resume removes both current and crashed
                // attempts with the workspace; never infer ownership from a name
                // beside the user's destination.
                Temporary::durable(
                    &recovery.root.join(format!(
                        "attempt-{attempt_id}.partial.{}",
                        staging_format.extension()
                    )),
                    false,
                )?
            } else {
                Temporary::create_extension(&output, &attempt_id, staging_format.extension())?
            });
        }
        if durable.is_none() {
            scratch.push(match settings.encoder {
                VideoEncoder::SvtAv1
                | VideoEncoder::SvtAv1FiveFish
                | VideoEncoder::SvtAv1Hdr
                | VideoEncoder::AomAv1
                | VideoEncoder::VpxStandalone => {
                    Temporary::create_ivf(&output, &format!("{attempt_id}-video"))?
                }
                VideoEncoder::X265Standalone => {
                    Temporary::create_extension(&output, &format!("{attempt_id}-video"), "hevc")?
                }
                VideoEncoder::X264
                | VideoEncoder::X265
                | VideoEncoder::Vp9
                | VideoEncoder::H264Nvenc
                | VideoEncoder::HevcNvenc => {
                    Temporary::create(&output, &format!("{attempt_id}-video"))?
                }
            });
        }
        let temp = standalone_final
            .as_ref()
            .or(temporary.as_ref())
            .expect("owned Matroska output");
        let intermediate = durable
            .as_ref()
            .or_else(|| scratch.last())
            .expect("owned encoded video intermediate");
        let standalone_grain_table = if settings.backend == media_core::EncodeBackend::Standalone {
            super::av1an::stage_standalone_grain_table(&output, &attempt_id, settings)?
        } else {
            None
        };
        let mut standalone_hdr = if settings.backend == media_core::EncodeBackend::Standalone {
            Some(dynamic_hdr.stage_standalone(&output, &attempt_id)?)
        } else {
            None
        };
        self.change(id, |snapshot| {
            append_log(
                snapshot,
                format!("Owned temporary Matroska: {}", temp.path.display()),
            );
            append_log(
                snapshot,
                format!("Owned temporary video: {}", intermediate.path.display()),
            );
        })
        .await;
        check_cancel(cancel)?;
        if settings.backend == media_core::EncodeBackend::Standalone
            && self.command_review(id).await.is_some()
        {
            // The held program owns many asynchronous construction temporaries.
            // Isolate that complete phase so its debug poll frame is not also
            // reserved while the normal QTGMC/encoder process tree is polled.
            let edited=boxed(||async {
            let two_pass = rate.as_ref().is_some_and(|rate| rate.two_pass);
            if two_pass && standalone_stats.is_none() {
                standalone_stats = Some(super::rate_control::Stats::create(&output, &format!("{attempt_id}-review"))?);
            }
            let first_pass = two_pass.then(|| Temporary::create(&output, &format!("{attempt_id}-review-pass-one"))).transpose()?;
            let mut commands = Vec::new();
            if defer_source_qtgmc && let Some(prepared)=&qtgmc {
                commands.push(format!("{} | {} > {}",super::command_edit::format_command(&prepared.producer),super::command_edit::format_command(&prepared.consumer),super::command_edit::quote(&prepared.video.path.to_string_lossy())));
            }
            let mut path_outputs = Vec::new();
            let mut producer = CommandSpec {
                executable: ffmpeg.clone(),
                args: if let Some(qtgmc) = &qtgmc {
                    decoder_args_preprocessed(&qtgmc.video.path, &plan)
                } else {
                    decoder_args_with_subtitles(&source.path, &plan, subtitle_assets.text_filter(), subtitle_assets.bitmap_input())
                },
                cwd: if qtgmc.is_some() { None } else { subtitle_assets.decoder_cwd().map(Path::to_path_buf) },
            };
            aom_vpx::configure_producer_output(&mut producer.args, &plan, settings);
            let mut encoded_path = intermediate.path.clone();
            for pass in 1..=if two_pass { 2 } else { 1 } {
                let pass_output = if two_pass && pass == 1 { first_pass.as_ref().expect("review first pass") } else { intermediate };
                let mut args = consumer_arguments(&plan, settings);
                if let Some(resources) = &standalone_hdr { resources.apply(&mut args); }
                if let Some(table) = &standalone_grain_table {
                    for index in 0..args.len() {
                        if args[index] == "--fgs-table" && args.get(index + 1) == Some(&OsString::from(super::av1an::GRAIN_TABLE_NAME)) { args[index + 1] = table.path().as_os_str().to_owned(); }
                        else if args[index] == OsString::from(format!("--film-grain-table={}", super::av1an::GRAIN_TABLE_NAME)) { let mut argument = OsString::from("--film-grain-table="); argument.push(table.path().as_os_str()); args[index] = argument; }
                    }
                }
                if let Some(rate) = &rate { rate.arguments_for_settings(&mut args, settings, pass); }
                let redirect = if !dynamic_hdr.is_empty() {
                    let native = Temporary::create_ivf(&output, &format!("{attempt_id}-review-hdr-{pass}"))?;
                    let at = args.iter().position(|arg| arg == "-b").expect("SVT output");
                    args[at + 1] = native.path.as_os_str().to_owned();
                    if !two_pass || pass == 2 { encoded_path = native.path.clone(); }
                    path_outputs.push(native);
                    String::new()
                } else {
                    format!(" > {}", super::command_edit::quote(&pass_output.path.to_string_lossy()))
                };
                let consumer = CommandSpec { executable: encoder.clone(), args, cwd: standalone_stats.as_ref().map(|stats| stats.path.clone()) };
                commands.push(format!("{} | {}{redirect}", super::command_edit::format_command(&producer), super::command_edit::format_command(&consumer)));
            }
            let needs_wrapper = matches!(settings.encoder, VideoEncoder::X265Standalone | VideoEncoder::VpxStandalone) || (settings.encoder == VideoEncoder::AomAv1 && plan.is_hdr10());
            if needs_wrapper {
                let wrapped = Temporary::create(&output, &format!("{attempt_id}-review-timed"))?;
                let mut args = x265::mkvmerge_arguments(&encoded_path, &wrapped.path, &plan);
                if settings.encoder == VideoEncoder::AomAv1 { let metadata = plan.aom_hdr_mkvmerge_arguments(); args.splice(args.len() - 1..args.len() - 1, metadata); }
                let command = super::command_edit::format_command(&CommandSpec { executable: mkvmerge.as_ref().expect("timing dependency").clone(), args, cwd: None });
                commands.push(if cfg!(windows) { format!("({command} & if errorlevel 2 (exit /b 2) else (exit /b 0))") } else { format!("({command}; result=$?; [ \"$result\" -le 1 ])") });
                encoded_path = wrapped.path.clone(); path_outputs.push(wrapped);
            }
            let attachment = super::encode_settings::SettingsAttachment::prepare(&temp.path, &format!("{attempt_id}-review"), settings)?;
            let mut mux = mux_args(&source.path, &encoded_path, &temp.path, &output_selected, &plan, settings, &external);
            if let Some(trim) = &prepared_trim { trim.apply_mux(&mut mux, &output_selected, &audio_filters)?; }
            normalization.apply_mux(&mut mux, &output_selected)?; subtitle_assets.apply_mux(&mut mux, &output_selected); keep_last_codec_options(&mut mux);
            if let Some(attachment) = &attachment { attachment.apply(&mut mux, output_selected.iter().filter(|stream| stream.codec_type.as_deref() == Some("attachment")).count()); }
            commands.push(super::command_edit::format_command(&CommandSpec { executable: ffmpeg.clone(), args: mux, cwd: None }));
            let program = commands.join(" &&\n");
            if let Some(review) = self.review_command(id, "ffmpeg", &program, &output, cancel).await?
                && review.arguments != review.generated.trim()
            {
                for temporary in &mut path_outputs { temporary.close_for_path_writer()?; }
                if defer_source_qtgmc && let Some(prepared)=&mut qtgmc {prepared.video.close_for_path_writer()?;}
                let result = boxed(||self.run_edited_program_with_environment(id, &review, None, &source.path, cancel, log_path,qtgmc.as_ref().and_then(|prepared|prepared.environment()))).await;
                if defer_source_qtgmc && let Some(prepared)=&mut qtgmc {prepared.video.reopen_after_path_writer()?;}
                for temporary in &mut path_outputs { if temporary.path.exists() { temporary.reopen_after_path_writer()?; } }
                result?;
                let actual = super::command_edit::verify_actual_output(&ffmpeg, &ffprobe, temp, cancel).await?;
                source.verify()?; external.verify()?; if let Some(original) = original_guard { original.verify()?; }
                self.change(id, |snapshot| { snapshot.duration_seconds = actual.duration(); append_log(snapshot, "The complete edited program produced fully decodable media; original typed picture expectations were replaced by the confirmed program.".into()); }).await;
                return Ok::<bool,AppError>(true);
            }
                Ok::<bool,AppError>(false)
            }).await?;
            if edited {
                self.finalize_verified(
                    id,
                    cancel,
                    &source,
                    Some(&external),
                    temp,
                    &output,
                    None,
                    None,
                    None,
                    scratch,
                )
                .await?;
                drop(standalone_stats);
                drop(standalone_timed);
                drop(standalone_final);
                drop(durable);
                if let Some(recovery) = standalone_recovery {
                    recovery.cleanup()?;
                    self.change(id, |snapshot| snapshot.standalone_recovery = None)
                        .await;
                }
                return Ok(());
            }
        }
        if deferred_vulkan {
            plan.resolve_tone_map(&ffmpeg, &source.path, Some(source_video_duration), cancel)
                .await?;
            if let Some(description) = plan.tone_map_description() {
                self.change(id, |snapshot| append_log(snapshot, description))
                    .await;
            }
        }
        if defer_source_qtgmc && let Some(prepared) = &qtgmc {
            let prep_log = log_path.with_extension("qtgmc.log");
            let (sender, events) = mpsc::channel(256);
            let observer = self.observe_encode(id, events, Some(plan.frame_seconds()));
            let result = boxed(|| prepared.run(cancel.clone(), sender, &prep_log)).await;
            let _ = observer.await;
            result?;
            frame_count =
                boxed(|| prepared.validate_source_filters(&ffprobe, &plan, cancel)).await?;
            source.verify()?;
        }
        if source_qtgmc
            && !source_qtgmc_checkpoint_ready
            && let Some(prepared) = &qtgmc
        {
            let identity =
                boxed(|| prepared.decoded_identity(&ffmpeg, frame_count, cancel)).await?;
            let recovery = standalone_recovery.as_mut().expect("standalone recovery");
            boxed(|| recovery.remember_source_qtgmc(&prepared.video.path, identity, cancel))
                .await?;
            boxed(|| recovery.verify_source_qtgmc(&prepared.video.path, cancel)).await?;
        }
        let video_position = output_selected
            .iter()
            .position(|stream| stream.index == plan.video_index)
            .expect("selected video");
        let resume_finalizing = standalone_recovery.as_ref().is_some_and(|recovery| {
            recovery.phase() == Some(media_core::StandaloneRecoveryPhase::Finalizing)
        });
        if settings.source_filters
            && let Some(contract) = standalone_recovery
                .as_ref()
                .and_then(|recovery| recovery.source_filter_contract())
        {
            plan.apply_source_filter_contract(contract)?;
            frame_count = usize::try_from(contract.frames).map_err(|_| {
                files::error(
                    "ENCODE_VALIDATION_FAILED",
                    "The observed filter frame count exceeds this platform.",
                    &output,
                )
            })?;
        }
        if resume_finalizing {
            self.change(id, |snapshot| {
                append_log(
                    snapshot,
                    "Reusing the fully verified durable Matroska stage; continuing final container publication."
                        .into(),
                )
            })
            .await;
        }
        if !resume_finalizing {
            if settings.backend == media_core::EncodeBackend::Av1an {
                let recovery = recovery.as_ref().expect("av1an recovery workspace");
                if settings
                    .av1an_options
                    .is_some_and(|options| options.source_workflow)
                {
                    frame_count = recovery.queue_frame_count().await?;
                }
                if !recovery.finalizing || self.command_review(id).await.is_some() {
                    self.phase(id, JobState::Running, &format!("av1an is detecting scenes and encoding parallel {} chunks; selected audio settings will be applied afterward.", settings.encoder.name())).await;
                    let review_attachment = (self.command_review(id).await.is_some()
                        || recovery.edited_command().await?.is_some())
                    .then(|| {
                        super::encode_settings::SettingsAttachment::prepare(
                            &temp.path,
                            &format!("{attempt_id}-review"),
                            settings,
                        )
                    })
                    .transpose()?
                    .flatten();
                    let post_mux = if self.command_review(id).await.is_some()
                        || recovery.edited_command().await?.is_some()
                    {
                        let mut args = mux_args(
                            &source.path,
                            &intermediate.path,
                            &temp.path,
                            &output_selected,
                            &plan,
                            settings,
                            &external,
                        );
                        if let Some(trim) = &prepared_trim {
                            trim.apply_mux(&mut args, &output_selected, &audio_filters)?;
                        }
                        normalization.apply_mux(&mut args, &output_selected)?;
                        subtitle_assets.apply_mux(&mut args, &output_selected);
                        keep_last_codec_options(&mut args);
                        if let Some(attachment) = &review_attachment {
                            attachment.apply(
                                &mut args,
                                output_selected
                                    .iter()
                                    .filter(|stream| {
                                        stream.codec_type.as_deref() == Some("attachment")
                                    })
                                    .count(),
                            );
                        }
                        Some(CommandSpec {
                            executable: ffmpeg.clone(),
                            args,
                            cwd: None,
                        })
                    } else {
                        None
                    };
                    let edited = self
                        .encode_av1an(
                            id,
                            av1an_input.as_deref().expect("prepared av1an input"),
                            &encoder,
                            av1an_executable.as_ref().expect("av1an executable"),
                            &ffmpeg,
                            &ffprobe,
                            mkvmerge.as_deref(),
                            recovery,
                            intermediate,
                            &mut plan,
                            settings,
                            cancel,
                            log_path,
                            frame_count,
                            &dynamic_hdr,
                            post_mux,
                        )
                        .await?;
                    if let Some(actual) = plan.source_queue_frame_count() {
                        frame_count = actual;
                    }
                    if edited {
                        let actual = super::command_edit::verify_actual_output(
                            &ffmpeg, &ffprobe, temp, cancel,
                        )
                        .await?;
                        source.verify()?;
                        external.verify()?;
                        if let Some(original) = original_guard {
                            original.verify()?;
                        }
                        self.change(id, |snapshot| { snapshot.duration_seconds = actual.duration(); append_log(snapshot, "The confirmed AV1AN program and selected-track mux produced fully decodable media.".into()); }).await;
                        self.finalize_verified(
                            id,
                            cancel,
                            &source,
                            Some(&external),
                            temp,
                            &output,
                            None,
                            None,
                            None,
                            scratch,
                        )
                        .await?;
                        if let Some(mut attempt) = temporary.take() {
                            attempt.cleanup()?;
                        }
                        drop(durable);
                        recovery.cleanup().await?;
                        self.change(id, |snapshot| snapshot.recovery = None).await;
                        return Ok(());
                    }
                    let summary = recovery.finalizing().await?;
                    self.change(id, |snapshot| snapshot.recovery = Some(summary))
                        .await;
                } else {
                    self.change(id,|snapshot|append_log(snapshot,"Reusing the verified durable AV1 intermediate; continuing output finalization.".into())).await;
                    if settings.source_filters {
                        let actual =
                            probe(&ffprobe, &intermediate.path, cancel, Some(&[0])).await?;
                        let stream = actual.streams.first().ok_or_else(|| {
                            files::error(
                                "ENCODE_VALIDATION_FAILED",
                                "The saved filtered AV1AN stage has no video.",
                                &intermediate.path,
                            )
                        })?;
                        plan.observe_source_filter_stream(stream, None)?;
                    }
                    if settings
                        .av1an_options
                        .is_some_and(|options| options.source_workflow)
                        && let Some(actual) = super::av1an::verify_source_join(
                            &ffmpeg,
                            &ffprobe,
                            mkvmerge.as_deref(),
                            recovery,
                            intermediate,
                            &mut plan,
                            cancel,
                        )
                        .await?
                    {
                        frame_count = actual;
                    }
                }
            } else {
                let mut producer = CommandSpec {
                    executable: ffmpeg.clone(),
                    args: if let Some(qtgmc) = &qtgmc {
                        decoder_args_preprocessed(&qtgmc.video.path, &plan)
                    } else if subtitle_assets.text_filter().is_none()
                        && subtitle_assets.bitmap_input().is_none()
                    {
                        decoder_args(&source.path, &plan)
                    } else {
                        decoder_args_with_subtitles(
                            &source.path,
                            &plan,
                            subtitle_assets.text_filter(),
                            subtitle_assets.bitmap_input(),
                        )
                    },
                    cwd: if qtgmc.is_some() {
                        None
                    } else {
                        subtitle_assets.decoder_cwd().map(Path::to_path_buf)
                    },
                };
                let mut consumer = CommandSpec {
                    executable: encoder.clone(),
                    // Both native encoders write into the already-owned file handle
                    // through stdout, preserving its identity guard and Unicode path.
                    args: consumer_arguments(&plan, settings),
                    cwd: None,
                };
                if let Some(resources) = &standalone_hdr {
                    resources.apply(&mut consumer.args);
                }
                if let Some(table) = &standalone_grain_table {
                    let name = super::av1an::GRAIN_TABLE_NAME;
                    for index in 0..consumer.args.len() {
                        if consumer.args[index] == "--fgs-table"
                            && consumer.args.get(index + 1) == Some(&OsString::from(name))
                        {
                            consumer.args[index + 1] = table.path().as_os_str().to_owned();
                        } else if consumer.args[index]
                            == OsString::from(format!("--film-grain-table={name}"))
                        {
                            let mut argument = OsString::from("--film-grain-table=");
                            argument.push(table.path().as_os_str());
                            consumer.args[index] = argument;
                        }
                    }
                }
                let raw_aom_input =
                    aom_vpx::configure_producer_output(&mut producer.args, &plan, settings);
                self.phase(id, JobState::Running, match settings.encoder {
                VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => "Encoding 10-bit AV1 with the standalone SVT encoder; selected audio settings will be applied afterward.",
                VideoEncoder::X264 => "Encoding H.264 with standalone x264 at the source bit depth; selected audio settings will be applied afterward.",
                VideoEncoder::X265 => "Encoding HEVC with FFmpeg libx265 at the source bit depth; selected audio settings will be applied afterward.",
                VideoEncoder::Vp9 => "Encoding VP9 with FFmpeg libvpx at the source bit depth; selected audio settings will be applied afterward.",
                VideoEncoder::AomAv1 => "Encoding AV1 with standalone aomenc at the source bit depth; selected audio settings will be applied afterward.",
                VideoEncoder::X265Standalone => "Encoding raw HEVC with standalone x265; mkvmerge will assign exact rational presentation timing before the final selected-track mux.",
                VideoEncoder::VpxStandalone => "Encoding VP9 with standalone vpxenc at the source bit depth; selected audio settings will be applied afterward.",
                VideoEncoder::H264Nvenc => "Encoding H.264 with NVIDIA NVENC after a successful hardware capability probe.",
                VideoEncoder::HevcNvenc => "Encoding HEVC with NVIDIA NVENC after a successful hardware capability probe.",
            }).await;
                if raw_aom_input {
                    self.change(id, |snapshot| {
                        append_log(
                            snapshot,
                            "The 10-bit AOM input uses explicit raw I420 geometry, cadence, depth, and chroma placement because high-bit-depth Y4M cannot represent left chroma placement unambiguously."
                                .into(),
                        )
                    })
                    .await;
                }
                let two_pass = rate.as_ref().is_some_and(|rate| rate.two_pass);
                let saved_phase = standalone_recovery
                    .as_ref()
                    .and_then(super::standalone_recovery::Recovery::phase);
                let video_ready = matches!(
                    saved_phase,
                    Some(
                        media_core::StandaloneRecoveryPhase::VideoComplete
                            | media_core::StandaloneRecoveryPhase::TimingWrapComplete
                            | media_core::StandaloneRecoveryPhase::Finalizing
                    )
                );
                let stats = if two_pass {
                    Some(if let Some(saved) = standalone_stats.take() {
                        saved
                    } else {
                        super::rate_control::Stats::create(&output, &attempt_id)?
                    })
                } else {
                    None
                };
                let pass_one_complete = saved_phase
                    == Some(media_core::StandaloneRecoveryPhase::PassOneComplete)
                    || video_ready;
                let first_pass = (two_pass && !pass_one_complete)
                    .then(|| Temporary::create(&output, &format!("{attempt_id}-pass-one")))
                    .transpose()?;
                let base_arguments = consumer.args.clone();
                let first = if video_ready {
                    3
                } else if two_pass && pass_one_complete {
                    2
                } else {
                    1
                };
                if first == 2 {
                    self.change(id, |snapshot| {
                        append_log(
                            snapshot,
                            "Resuming after verified pass one; only pass two will run.".into(),
                        )
                    })
                    .await;
                }
                for pass in first..=if two_pass { 2 } else { 1 } {
                    check_cancel(cancel)?;
                    source.verify()?;
                    consumer.args = base_arguments.clone();
                    if let Some(rate) = &rate {
                        rate.arguments_for_settings(&mut consumer.args, settings, pass);
                    }
                    consumer.cwd = stats.as_ref().map(|stats| stats.path.clone());
                    if let Some(stats) = &stats {
                        if pass == 2 {
                            stats.verify_stats()?;
                        } else {
                            stats.check()?;
                        }
                    }
                    let pass_log = if two_pass && pass == 1 {
                        log_path.with_extension("pass1.log")
                    } else {
                        log_path.to_owned()
                    };
                    self.change(id, |snapshot| {
                    snapshot.progress_seconds = Some(0.0);
                    snapshot.log_path = Some(pass_log.to_string_lossy().into_owned());
                    if let Some(rate) = &rate {
                        append_log(snapshot, format!("Video bitrate {} kb/s; pass {pass}/{} starts a fresh source decoder and encoder. Pass log: {}", rate.kbps, if two_pass { 2 } else { 1 }, pass_log.display()));
                    }
                }).await;
                    let pass_output = if two_pass && pass == 1 {
                        first_pass.as_ref().expect("owned first pass")
                    } else {
                        intermediate
                    };
                    let (sender, events) = mpsc::channel(256);
                    let event_task = self.observe_encode(id, events, Some(plan.frame_seconds()));
                    let result = if !dynamic_hdr.is_empty() {
                        // These fork builds print metadata-loading messages to
                        // stdout. A separate identity-guarded path writer keeps
                        // those diagnostics out of the IVF byte stream.
                        let mut native = Temporary::create_ivf(
                            &output,
                            &format!("{attempt_id}-hdr-pass-{pass}"),
                        )?;
                        let output_arg = consumer
                            .args
                            .iter()
                            .position(|arg| arg == "-b")
                            .expect("SVT output argument");
                        consumer.args[output_arg + 1] = native.path.as_os_str().to_owned();
                        native.close_for_path_writer()?;
                        let execution = supervisor::run_pipeline(
                            &producer,
                            &consumer,
                            cancel.clone(),
                            sender,
                            &pass_log,
                            Duration::from_secs(24 * 60 * 60),
                        )
                        .await;
                        let rebound = native.reopen_after_path_writer();
                        rebound?;
                        match execution {
                            Ok(result) => {
                                if result.producer_status.success()
                                    && result.consumer_status.success()
                                {
                                    native.flush_nonempty_async().await?;
                                    super::dynamic_hdr::copy_encoded(&native, pass_output, cancel)
                                        .await?;
                                }
                                Ok(result)
                            }
                            Err(error) => Err(error),
                        }
                    } else {
                        supervisor::run_pipeline_to_file(
                            &producer,
                            &consumer,
                            cancel.clone(),
                            sender,
                            &pass_log,
                            Duration::from_secs(24 * 60 * 60),
                            pass_output.clone_file()?,
                        )
                        .await
                    };
                    let _ = event_task.await;
                    let result = result.map_err(|e| process_error(e, &source.path))?;
                    if !result.producer_status.success() || !result.consumer_status.success() {
                        return Err(files::error(
                            "ENCODE_FAILED",
                            format!(
                                "Standalone encoder pass {pass} did not complete successfully."
                            ),
                            &source.path,
                        ));
                    }
                    if two_pass && pass == 1 {
                        let stats = stats.as_ref().expect("two-pass stats");
                        stats.verify_stats()?;
                        let summary = standalone_recovery
                            .as_mut()
                            .expect("standalone recovery")
                            .checkpoint_pass_one(stats)?;
                        self.change(id, |snapshot| snapshot.standalone_recovery = Some(summary))
                            .await;
                    }
                }
                if video_ready {
                    self.change(id, |snapshot| {
                    append_log(
                        snapshot,
                        "Reusing the verified complete standalone video; the interrupted encoder phase is not replayed."
                            .into(),
                    )
                })
                .await;
                } else {
                    intermediate.flush_nonempty_async().await?;
                    validate_video_checkpoint(
                        &ffprobe,
                        intermediate,
                        video,
                        &mut plan,
                        &mut frame_count,
                        if settings.source_filters {
                            Some(
                                super::source_filters::current()
                                    .expect("source filter observation scope")
                                    .completed(cancel)
                                    .await?
                                    .into_iter()
                                    .find(|clock| clock.stream_index == 0)
                                    .ok_or_else(|| {
                                        files::error(
                                            "SOURCE_FILTER_OBSERVATION_INVALID",
                                            "The filtered producer omitted video stream zero.",
                                            &output,
                                        )
                                    })?,
                            )
                        } else {
                            None
                        },
                        full_output_verification(settings),
                        cancel,
                    )
                    .await?;
                    if source_qtgmc && let Some(prepared) = &qtgmc {
                        boxed(|| {
                            standalone_recovery
                                .as_ref()
                                .expect("standalone recovery")
                                .verify_source_qtgmc(&prepared.video.path, cancel)
                        })
                        .await?;
                    }
                    if settings.source_filters {
                        standalone_recovery
                            .as_mut()
                            .expect("standalone recovery")
                            .remember_source_filter_contract(
                                plan.source_filter_contract(frame_count),
                            )?;
                    }
                    let summary = standalone_recovery
                        .as_mut()
                        .expect("standalone recovery")
                        .checkpoint_video(
                            intermediate,
                            media_core::StandaloneRecoveryPhase::VideoComplete,
                            cancel,
                        )
                        .await?;
                    self.change(id, |snapshot| snapshot.standalone_recovery = Some(summary))
                        .await;
                }
            }
            let needs_timing_wrapper = (settings.backend == media_core::EncodeBackend::Standalone
                && matches!(
                    settings.encoder,
                    VideoEncoder::X265Standalone | VideoEncoder::VpxStandalone
                ))
                || (settings.encoder == VideoEncoder::AomAv1 && plan.is_hdr10());
            let wrapped_timed = if needs_timing_wrapper {
                if let Some(saved) = standalone_timed.take() {
                    let encoder_name = match settings.encoder {
                        VideoEncoder::X265Standalone => "x265",
                        VideoEncoder::VpxStandalone => "vpxenc",
                        VideoEncoder::AomAv1 => "aomenc",
                        _ => unreachable!("validated timing wrapper"),
                    };
                    self.change(id, |snapshot| {
                    append_log(
                        snapshot,
                        format!("Reusing the verified {encoder_name} rational timing wrapper from the last complete phase."),
                    )
                })
                .await;
                    Some(saved)
                } else {
                    check_cancel(cancel)?;
                    source.verify()?;
                    intermediate.flush_nonempty_async().await?;
                    let encoder_name = match settings.encoder {
                        VideoEncoder::X265Standalone => "x265",
                        VideoEncoder::VpxStandalone => "vpxenc",
                        VideoEncoder::AomAv1 => "aomenc",
                        _ => unreachable!("validated timing wrapper"),
                    };
                    let mut wrapped =
                        Temporary::create(&output, &format!("{attempt_id}-{encoder_name}-timed"))?;
                    let wrap_log = log_path.with_extension(format!("{encoder_name}-mkvmerge.log"));
                    self.phase(
                id,
                JobState::Running,
                if settings.encoder == VideoEncoder::X265Standalone {
                    "Assigning the exact rational frame rate and B-frame presentation order with mkvmerge."
                } else if settings.encoder == VideoEncoder::AomAv1 {
                    "Carrying verified HDR10 mastering and content-light metadata through mkvmerge."
                } else {
                    "Replacing vpxenc's normalized IVF rate with the validated exact rational frame rate using mkvmerge."
                },
            )
            .await;
                    let (sender, events) = mpsc::channel(256);
                    let observer = self.observe_encode(id, events, None);
                    wrapped.close_for_path_writer()?;
                    let result = supervisor::run(
                        &CommandSpec {
                            executable: mkvmerge.as_ref().expect("x265 timing dependency").clone(),
                            args: {
                                let mut args = x265::mkvmerge_arguments(
                                    &intermediate.path,
                                    &wrapped.path,
                                    &plan,
                                );
                                if settings.encoder == VideoEncoder::AomAv1 {
                                    let metadata = plan.aom_hdr_mkvmerge_arguments();
                                    args.splice(args.len() - 1..args.len() - 1, metadata);
                                }
                                args
                            },
                            cwd: None,
                        },
                        cancel.clone(),
                        sender,
                        &wrap_log,
                        Duration::from_secs(60 * 60),
                    )
                    .await;
                    let rebound = wrapped.reopen_after_path_writer();
                    let _ = observer.await;
                    let result =
                        result.map_err(|error| process_error(error, &intermediate.path))?;
                    rebound?;
                    if !matches!(result.status.code(), Some(0 | 1)) {
                        return Err(files::error(
                            "ENCODE_TIMING_WRAP_FAILED",
                            format!(
                                "mkvmerge could not assign timestamps to the standalone {encoder_name} stream. Exit 1 is accepted as a warning; exit 2 is fatal."
                            ),
                            &intermediate.path,
                        ));
                    }
                    wrapped.flush_nonempty_async().await?;
                    self.change(id, |snapshot| {
                        append_log(
                            snapshot,
                            format!("{encoder_name} timing log: {}", wrap_log.display()),
                        )
                    })
                    .await;
                    let artifact = probe(&ffprobe, &wrapped.path, cancel, Some(&[0])).await?;
                    let encoded = artifact.streams.first().ok_or_else(|| {
                        files::error(
                            "ENCODE_VALIDATION_FAILED",
                            "The timed standalone encoder checkpoint has no video stream.",
                            &wrapped.path,
                        )
                    })?;
                    if settings.encoder == VideoEncoder::VpxStandalone {
                        plan.validate_elementary_checkpoint(video, encoded)?;
                    } else {
                        plan.validate_encoded_stream(video, encoded)?;
                    }
                    if decodes_output(settings, &plan) {
                        let mut checkpoint_plan = plan.clone();
                        let decoded = self
                            .scan_frames(
                                id,
                                &ffprobe,
                                &wrapped.path,
                                &mut checkpoint_plan,
                                encoded,
                                true,
                                cancel,
                            )
                            .await?;
                        if decoded != frame_count {
                            return Err(files::error(
                                "ENCODE_VALIDATION_FAILED",
                                "The timed standalone encoder checkpoint did not decode to the exact planned frame count.",
                                &wrapped.path,
                            ));
                        }
                    } else {
                        // The muxed output receives the decoded-frame sample.
                        super::packet_clock::verify(
                            &ffprobe,
                            &wrapped.path,
                            0,
                            super::packet_clock::Expected {
                                origin: plan.output_origin,
                                frame_seconds: plan.frame_seconds(),
                                tolerance: super::encode_plan::encoded_timestamp_tolerance(encoded),
                                frames: frame_count,
                            },
                            cancel,
                        )
                        .await?;
                    }
                    if let Some(recovery) = standalone_recovery.as_mut() {
                        let summary = recovery
                            .checkpoint_video(
                                &wrapped,
                                media_core::StandaloneRecoveryPhase::TimingWrapComplete,
                                cancel,
                            )
                            .await?;
                        self.change(id, |snapshot| snapshot.standalone_recovery = Some(summary))
                            .await;
                    }
                    Some(wrapped)
                }
            } else {
                None
            };
            let intermediate = wrapped_timed.as_ref().unwrap_or(intermediate);
            check_cancel(cancel)?;
            source.verify()?;
            intermediate.flush_nonempty_async().await?;
            self.change(id, |snapshot| {
                snapshot.log_path = Some(log_path.to_string_lossy().into_owned())
            })
            .await;
            self.change(id, |snapshot| {
            if !matches!(snapshot.state, JobState::Canceling | JobState::Stopping) {
                snapshot.state = JobState::Finalizing;
            }
            snapshot.progress_seconds = None;
            append_log(snapshot, "Muxing encoded video, applying selected audio settings, and preserving subtitles, metadata, chapters, and attachments.".into());
        }).await;
            let mux_log = log_path.with_extension("mux.log");
            let settings_attachment =
                super::encode_settings::SettingsAttachment::prepare(&temp.path, id, settings)?;
            let mut mux_arguments = mux_args(
                &source.path,
                &intermediate.path,
                &temp.path,
                &output_selected,
                &plan,
                settings,
                &external,
            );
            if let Some(prepared) = &prepared_trim {
                prepared.apply_mux(&mut mux_arguments, &output_selected, &audio_filters)?;
            }
            normalization.apply_mux(&mut mux_arguments, &output_selected)?;
            subtitle_assets.apply_mux(&mut mux_arguments, &output_selected);
            keep_last_codec_options(&mut mux_arguments);
            if let Some(attachment) = &settings_attachment {
                attachment.apply(
                    &mut mux_arguments,
                    output_selected
                        .iter()
                        .filter(|stream| stream.codec_type.as_deref() == Some("attachment"))
                        .count(),
                );
            }
            let (sender, events) = mpsc::channel(256);
            let observer = self.observe_encode(id, events, None);
            let result = supervisor::run(
                &CommandSpec {
                    executable: ffmpeg.clone(),
                    args: mux_arguments,
                    cwd: None,
                },
                cancel.clone(),
                sender,
                &mux_log,
                Duration::from_secs(60 * 60),
            )
            .await;
            let _ = observer.await;
            let result = result.map_err(|e| process_error(e, &source.path))?;
            if !result.status.success() {
                return Err(files::error(
                    "ENCODE_MUX_FAILED",
                    "The encoded video could not be muxed with the selected source tracks.",
                    &temp.path,
                ));
            }
            self.change(id, |snapshot| {
                append_log(snapshot, format!("Mux log: {}", mux_log.display()))
            })
            .await;
            check_cancel(cancel)?;
            temp.flush_nonempty_async().await?;
            self.phase(id,JobState::Finalizing,"Verifying the completed output's exact frame count and timing, then checking copied tracks and metadata.").await;
            if let Some(format) = source_final_iso {
                source_iso_receipt = Some(container::seal_source_iso(temp, format, cancel).await?);
            }
            let mut artifact = probe(&ffprobe, &temp.path, cancel, None).await?;
            if let Some(attachment) = &settings_attachment {
                attachment.verify_and_remove(&mut artifact)?;
            }
            let verified_trim = if let Some(prepared) = &prepared_trim {
                let (expected, origin) = prepared
                    .verify_source(
                        &ffmpeg,
                        &ffprobe,
                        &temp.path,
                        &output_selected,
                        &artifact,
                        Some(plan.video_index),
                        Some(&intermediate.path),
                        cancel,
                    )
                    .await?;
                prepared.normalize_verified_packet_starts(
                    &mut artifact,
                    &expected,
                    &output_selected,
                );
                plan.output_origin = origin;
                if !prepared.copied_indices().is_empty() {
                    super::mux::verify_shifted_video_packets(
                        &ffprobe,
                        &intermediate.path,
                        0,
                        &temp.path,
                        video_position as u32,
                        origin,
                        cancel,
                    )
                    .await?;
                }
                Some(expected)
            } else {
                None
            };
            let expected_document = subtitle_assets
                .expected_document(verified_trim.as_ref().unwrap_or(&output_document));
            let expected_document = plan.temporal_document(expected_document);
            let expected_document = plan.source_filter_document(expected_document, frame_count);
            let (expected_document, converted_subtitles) = container::normalize_staging(
                &expected_document,
                &output_indices,
                &mut artifact,
                Some(&mux_settings),
            )?;
            let expected_selected = expected_document.selected(&output_indices)?;
            metadata::verify_encoded_with_subtitles(
                &expected_document,
                &expected_selected,
                &artifact,
                plan.video_index,
                plan.output_codec(),
                (plan.width, plan.height),
                &mux_settings.audio,
                &converted_subtitles,
            )?;
            external.verify_metadata_overrides(
                settings,
                &expected_selected,
                &artifact,
                container::staging_format(&output_selected, Some(&mux_settings)),
            )?;
            if let Some(prepared) = &prepared_trim {
                prepared
                    .verify_subtitles(
                        &ffmpeg,
                        &temp.path,
                        &output_selected,
                        subtitle_assets.overridden_indices(),
                        cancel,
                    )
                    .await?;
            }
            subtitle_assets
                .verify(&ffmpeg, &temp.path, &output_selected, cancel)
                .await?;
            for (track, timeline) in &audio_timelines {
                let position = output_selected
                    .iter()
                    .position(|stream| stream.index == track.stream_index)
                    .expect("selected audio");
                let decoded = audio::scan(
                    &ffprobe,
                    &temp.path,
                    &artifact.streams[position],
                    true,
                    cancel,
                )
                .await?;
                let evidence = audio::verify_timeline(
                    &timeline.shifted(
                        prepared_trim
                            .as_ref()
                            .map_or(plan.output_origin, |prepared| {
                                prepared.audio_origin(plan.output_origin)
                            }),
                    ),
                    &decoded,
                    track.codec,
                )?;
                self.change(id, |snapshot| {
                    append_log(
                        snapshot,
                        format!("Source audio stream {}: {evidence}", track.stream_index),
                    )
                })
                .await;
            }
            let encoded = &artifact.streams[video_position];
            if plan.has_source_queue() {
                super::mux::verify_shifted_video_packets(
                    &ffprobe,
                    &intermediate.path,
                    0,
                    &temp.path,
                    video_position as u32,
                    0.0,
                    cancel,
                )
                .await?;
            }
            if let Some(format) = source_final_iso.filter(|_| plan.output_codec() == "vp9") {
                super::mux::verify_shifted_video_packets(
                    &ffprobe,
                    &intermediate.path,
                    0,
                    &temp.path,
                    video_position as u32,
                    plan.output_origin,
                    cancel,
                )
                .await?;
                let checkpoint = probe(&ffprobe, &intermediate.path, cancel, Some(&[0])).await?;
                let checkpoint = checkpoint.streams.first().ok_or_else(|| {
                    files::error(
                        "ENCODE_VALIDATION_FAILED",
                        "The verified VP9 checkpoint lost its video stream.",
                        &intermediate.path,
                    )
                })?;
                plan.validate_packet_copied_iso_vp9(video, checkpoint, encoded, format)?;
            } else {
                plan.validate_encoded_stream(video, encoded)?;
            }
            self.change(id, |snapshot| snapshot.progress_seconds = Some(0.0))
                .await;
            // The timing wrapper was decoded completely and every frame was
            // validated against this plan. Prove the mux copied it instead of
            // decoding it again, for the muxes that cannot rewrite or retime
            // its packets beyond the planned origin.
            let copied_wrapper = wrapped_timed.is_some()
                && prepared_trim.is_none()
                && source_final_iso.is_none()
                && !plan.has_source_queue()
                && !mux_settings
                    .hdr_metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.dolby_vision_rpu_path.is_some());
            let mut final_video_plan = plan.clone();
            final_video_plan.output_origin = plan.encoded_mux_origin();
            if !decodes_output(settings, &plan) {
                // Fast verification: every frame's count and clock from packet
                // timestamps, and the first decoded frames against the plan.
                let sampled =
                    frame_sample(&ffprobe, &temp.path, &final_video_plan, encoded, cancel).await?;
                super::packet_clock::verify(
                    &ffprobe,
                    &temp.path,
                    video_position as u32,
                    super::packet_clock::Expected {
                        origin: final_video_plan.output_origin,
                        frame_seconds: plan.frame_seconds(),
                        tolerance: super::encode_plan::encoded_timestamp_tolerance(encoded),
                        frames: frame_count,
                    },
                    cancel,
                )
                .await?;
                self.change(id, |snapshot| {
                    append_log(
                        snapshot,
                        format!(
                            "Fast output verification: all {frame_count} frames are on the planned clock by their packet timestamps, and the first {sampled} decoded frames match the plan. The whole video was not decoded; turn on full output verification in Settings to decode every frame."
                        ),
                    )
                })
                .await;
            } else if copied_wrapper
                && prove_copied_video(
                    &ffprobe,
                    &intermediate.path,
                    &temp.path,
                    video_position as u32,
                    &plan,
                    frame_count,
                    cancel,
                )
                .await?
            {
                self.change(id, |snapshot| {
                    append_log(
                        snapshot,
                        format!(
                            "The muxed video is a byte-identical copy of the decoded and verified timing wrapper, with all {frame_count} frames on the planned clock; it was not decoded again."
                        ),
                    )
                })
                .await;
            } else {
                let decoded_count = self
                    .scan_frames(
                        id,
                        &ffprobe,
                        &temp.path,
                        &mut final_video_plan,
                        encoded,
                        true,
                        cancel,
                    )
                    .await?;
                if decoded_count != frame_count {
                    return Err(AppError::new(
                        "ENCODE_VALIDATION_FAILED",
                        "The encoded video does not decode to exactly the validated frame count for the selected interval.",
                        None,
                    ));
                }
            }
            source.verify()?;
            check_cancel(cancel)?;
            if let Some(target) = settings
                .rate_control
                .and_then(super::rate_control::target_bytes)
            {
                let actual = temp
                    .clone_file()?
                    .metadata()
                    .map_err(|e| files::error("OUTPUT_UNREADABLE", e.to_string(), &temp.path))?
                    .len();
                self.change(id, |snapshot| append_log(snapshot, format!("Target-size result before any final container conversion: {actual} bytes, requested {target} bytes ({:.2}% difference). Two-pass targets are estimates; encoder content and container overhead determine actual size.", (actual as f64 / target as f64 - 1.0) * 100.0))).await;
            }
            if let Some(recovery) = standalone_recovery.as_mut() {
                let summary = recovery
                    .checkpoint_video(
                        temp,
                        media_core::StandaloneRecoveryPhase::Finalizing,
                        cancel,
                    )
                    .await?;
                self.change(id, |snapshot| snapshot.standalone_recovery = Some(summary))
                    .await;
            }
        }
        if plan.has_alpha() {
            self.phase(
                id,
                JobState::Finalizing,
                "Verifying every VP9 alpha frame with the libvpx decoder before publication.",
            )
            .await;
            let reference = CommandSpec {
                executable: ffmpeg.clone(),
                args: decoder_args(&source.path, &plan),
                cwd: None,
            };
            let mut candidate_args = decoder_args_preprocessed(&temp.path, &plan);
            let map = candidate_args
                .iter()
                .position(|argument| argument == "-map")
                .expect("decoder stream map");
            candidate_args[map + 1] = format!("0:{video_position}").into();
            let candidate = CommandSpec {
                executable: ffmpeg.clone(),
                args: candidate_args,
                cwd: None,
            };
            let evidence = super::alpha::verify(
                &reference,
                &candidate,
                plan.width,
                plan.height,
                frame_count,
                settings.lossless,
                &temp.path,
                cancel,
            )
            .await?;
            source.verify()?;
            self.change(id, |snapshot| append_log(snapshot, evidence))
                .await;
        }
        if settings.lossless && !plan.has_alpha() {
            self.phase(id, JobState::Finalizing, "Comparing every decoded lossless pixel against the processed encoder input before publication.").await;
            let prepared_path = qtgmc
                .as_ref()
                .map(|prepared| prepared.video.path.as_path())
                .or_else(|| {
                    av1an_prepared
                        .as_ref()
                        .map(|(prepared, _)| prepared.video.path.as_path())
                });
            let reference = CommandSpec {
                executable: ffmpeg.clone(),
                args: if let Some(path) = prepared_path {
                    decoder_args_preprocessed(path, &plan)
                } else {
                    decoder_args_with_subtitles(
                        &source.path,
                        &plan,
                        subtitle_assets.text_filter(),
                        subtitle_assets.bitmap_input(),
                    )
                },
                cwd: if prepared_path.is_some() {
                    None
                } else {
                    subtitle_assets.decoder_cwd().map(Path::to_path_buf)
                },
            };
            let mut decoded_args = decoder_args_preprocessed(&temp.path, &plan);
            let map = decoded_args
                .iter()
                .position(|argument| argument == "-map")
                .expect("decoder stream map");
            decoded_args[map + 1] = format!("0:{video_position}").into();
            let candidate = CommandSpec {
                executable: ffmpeg.clone(),
                args: decoded_args,
                cwd: None,
            };
            let pixels = super::lossless::verify(
                &reference,
                &candidate,
                plan.width,
                plan.height,
                plan.output_bit_depth(),
                frame_count,
                cancel,
            )
            .await?;
            source.verify()?;
            self.change(id, |snapshot| {
                append_log(
                    snapshot,
                    format!("Verified lossless decoded pixel SHA-256: {pixels}"),
                )
            })
            .await;
        }
        if source_origin.is_some() {
            let artifact = probe(&ffprobe, &temp.path, cancel, None).await?;
            for (position, stream) in output_selected.iter().enumerate() {
                let Some(original) = document
                    .streams
                    .iter()
                    .find(|original| original.index == stream.index)
                else {
                    continue;
                };
                if !matches!(
                    stream.codec_type.as_deref(),
                    Some("audio" | "subtitle" | "data")
                ) || audio::converted(&mux_settings)
                    .any(|track| track.stream_index == stream.index)
                    || subtitle_assets.overridden_indices().contains(&stream.index)
                {
                    continue;
                }
                if super::source_normalization::aac::needed(&document, original) {
                    let evidence = super::source_normalization::aac::verify(
                        &ffmpeg,
                        &ffprobe,
                        &source.path,
                        original,
                        &temp.path,
                        &artifact.streams[position],
                        0.0,
                        cancel,
                    )
                    .await?;
                    self.change(id, |snapshot| append_log(snapshot, evidence))
                        .await;
                } else {
                    super::mux::verify_packets(
                        &ffprobe,
                        &source.path,
                        stream.index,
                        &temp.path,
                        position as u32,
                        cancel,
                    )
                    .await?;
                    if stream.codec_type.as_deref() == Some("audio") {
                        let before =
                            audio::scan(&ffprobe, &source.path, original, false, cancel).await?;
                        let after = audio::scan_container_output(
                            &ffprobe,
                            &temp.path,
                            &artifact.streams[position],
                            &before,
                            cancel,
                        )
                        .await?;
                        let evidence =
                            audio::verify_timeline(&before, &after, media_core::AudioCodec::Copy)?;
                        self.change(id, |snapshot| {
                            append_log(snapshot, format!("Copied cut audio: {evidence}"))
                        })
                        .await;
                    }
                }
            }
        }
        external
            .verify_packets_excluding(
                &ffprobe,
                &temp.path,
                &output_selected,
                &prepared_trim.as_ref().map_or_else(Vec::new, |trim| {
                    let mut indices = trim.subtitle_indices();
                    indices.extend(trim.copied_indices());
                    indices
                }),
                cancel,
            )
            .await?;
        for (position, stream) in output_selected
            .iter()
            .enumerate()
            .filter(|(_, stream)| stream.codec_type.as_deref() == Some("data"))
        {
            if external.audio_source(stream.index).is_none() {
                super::mux::verify_shifted_packets(
                    &ffprobe,
                    &source.path,
                    stream.index,
                    &temp.path,
                    position as u32,
                    0.0,
                    cancel,
                )
                .await?;
            }
        }
        if let Some(original) = original_guard {
            original.verify()?;
        }
        if let Some(staged) = standalone_hdr.take() {
            staged.finish()?;
        }
        self.finalize_verified(
            id,
            cancel,
            &source,
            Some(&external),
            temp,
            &output,
            Some(container::Cadence {
                index: video_position as u32,
                numerator: plan.fps_num,
                denominator: plan.fps_den,
                frames: frame_count,
                source_iso: if let Some(receipt) = source_iso_receipt {
                    receipt.verify(temp, cancel).await?;
                    Some(receipt)
                } else {
                    None
                },
                full_verification: decodes_output(settings, &plan),
            }),
            plan.has_alpha().then_some((
                &plan,
                ffmpeg.as_path(),
                video_position,
                frame_count,
                settings.lossless,
            )),
            Some((
                &dynamic_hdr,
                ffmpeg.as_path(),
                ffprobe.as_path(),
                video_position,
                (plan.fps_num, plan.fps_den),
            )),
            scratch,
        )
        .await?;
        drop(standalone_stats);
        drop(standalone_timed);
        drop(standalone_final);
        drop(durable);
        if let Some(recovery) = recovery {
            // Close and unlink the current attempt before removing its parent
            // workspace on Windows. If its identity check fails, retain the
            // entire workspace: inferring ownership of the replacement from
            // its location would violate the no-clobber boundary.
            let attempt_cleanup = match temporary.take() {
                Some(mut attempt) => attempt.cleanup(),
                None => Ok(()),
            };
            let cleanup = match attempt_cleanup {
                Ok(()) => recovery.cleanup().await,
                Err(error) => Err(error),
            };
            match cleanup {
                Ok(()) => self.change(id, |snapshot| snapshot.recovery = None).await,
                Err(error) => {
                    self.change(id, |snapshot| {
                        append_log(
                            snapshot,
                            format!(
                                "Output succeeded; recovery files were retained: {}",
                                error.message
                            ),
                        );
                        snapshot.error = Some(error.clone());
                    })
                    .await
                }
            }
        }
        if let Some(recovery) = standalone_recovery {
            // A resumed durable stage must stay in the inventory until verified
            // workspace cleanup. Fresh attempts are separate scratch links;
            // keep them owned by execute so a locked link is reported there.
            if let Some(attempt) = temporary.take() {
                scratch.push(attempt);
            }
            match recovery.cleanup() {
                Ok(()) => {
                    self.change(id, |snapshot| snapshot.standalone_recovery = None)
                        .await
                }
                Err(error) => {
                    self.change(id, |snapshot| {
                        append_log(
                            snapshot,
                            format!(
                                "Output succeeded; standalone recovery files were retained: {}",
                                error.message
                            ),
                        );
                        snapshot.error = Some(error.clone());
                    })
                    .await
                }
            }
        }
        Ok(())
    }

    /// Plan the source frame count and cadence. When packet timestamps can
    /// stand in for decoded frames, plan from them and the first decoded frames
    /// and verify the decoded frames beside the encoder (see `source_timing`):
    /// every frame, or for a trim every frame through its end plus look-ahead.
    /// Otherwise decode the complete source first, as before.
    #[allow(clippy::too_many_arguments)]
    async fn source_frames(
        &self,
        id: &str,
        ffprobe: &Path,
        input: &Path,
        plan: &mut Plan,
        stream: &metadata::Stream,
        duration: Option<f64>,
        trim: Option<media_core::VideoTrim>,
        preview: bool,
        cancel: &watch::Receiver<bool>,
    ) -> Result<usize, AppError> {
        let concurrent = super::source_timing::available();
        if preview || concurrent {
            self.change(id, |snapshot| {
                append_log(snapshot, "Planning source frames from every packet timestamp and the first decoded frames.".into())
            })
            .await;
            match super::source_timing::plan_from_packets(ffprobe, input, plan, stream, cancel)
                .await?
            {
                Ok((planned, count)) => {
                    if concurrent && !preview {
                        // Frames after a trim's end are never encoded; the
                        // packet plan has already checked all of their times.
                        let limit = trim
                            .map(|trim| {
                                super::trim::Interval::build(
                                    trim,
                                    count,
                                    planned.fps_num,
                                    planned.fps_den,
                                )
                            })
                            .transpose()?
                            .map(|interval| {
                                super::source_timing::trim_scan_frames(
                                    interval.source_end_frame,
                                    count,
                                )
                            })
                            .filter(|&frames| frames < count);
                        let mut scan_plan = plan.clone();
                        if limit.is_some() {
                            // A short prefix cannot show the drift that chose
                            // the packet plan's cadence; check against it.
                            scan_plan.fps_num = planned.fps_num;
                            scan_plan.fps_den = planned.fps_den;
                            scan_plan.cadence_reconciled = planned.cadence_reconciled;
                        }
                        super::source_timing::spawn(
                            self.clone(),
                            id.to_owned(),
                            ffprobe.to_owned(),
                            input.to_owned(),
                            scan_plan,
                            stream.clone(),
                            super::source_timing::Expected::new(&planned, limit.unwrap_or(count)),
                            limit,
                        );
                        self.change(id, |snapshot| {
                            append_log(snapshot, match limit {
                                Some(frames) => format!("Planned {count} constant-rate frames from packet timestamps. The first {frames} decoded source frames (through the trim end and its look-ahead: every timestamp, progressive scan, color, HDR and Dolby Vision metadata) are verified alongside the encoder; the job fails and nothing is published if any frame is rejected."),
                                None => format!("Planned {count} constant-rate frames from packet timestamps. The complete decoded source (every timestamp, progressive scan, color, HDR and Dolby Vision metadata) is verified alongside the encoder; the job fails and nothing is published if any frame is rejected."),
                            })
                        })
                        .await;
                    }
                    *plan = planned;
                    return Ok(count);
                }
                Err(reason) => {
                    self.change(id, |snapshot| {
                        append_log(snapshot, format!("Packet timing cannot stand in for decoded frames ({reason}); decoding the complete source before encoding."))
                    })
                    .await;
                }
            }
        }
        self.change(id, |snapshot| {
            snapshot.progress_seconds = Some(0.0);
            snapshot.duration_seconds = duration.filter(|duration| *duration > 0.0);
            append_log(snapshot, "Decoding the complete source to verify every frame timestamp, progressive scan, color, and HDR metadata with bounded memory.".into());
        })
        .await;
        self.scan_frames(id, ffprobe, input, plan, stream, false, cancel)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn scan_frames(
        &self,
        id: &str,
        ffprobe: &Path,
        input: &Path,
        plan: &mut Plan,
        stream: &metadata::Stream,
        encoded: bool,
        cancel: &watch::Receiver<bool>,
    ) -> Result<usize, AppError> {
        let (progress, mut updates) = watch::channel(0.0_f64);
        let observer = async {
            while updates.changed().await.is_ok() {
                let seconds = *updates.borrow_and_update();
                self.change(id, |snapshot| {
                    snapshot.progress_seconds = Some(
                        snapshot
                            .duration_seconds
                            .map_or(seconds, |duration| seconds.min(duration)),
                    );
                })
                .await;
            }
        };
        let (result, ()) = tokio::join!(
            frame_scan(
                ffprobe,
                input,
                plan,
                stream,
                encoded,
                cancel,
                Some(progress)
            ),
            observer,
        );
        result
    }

    pub(super) fn observe_encode(
        &self,
        id: &str,
        mut events: mpsc::Receiver<ProcessEvent>,
        frame_seconds: Option<f64>,
    ) -> tokio::task::JoinHandle<()> {
        let manager = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let line = match event {
                    ProcessEvent::Stdout(line) | ProcessEvent::Stderr(line) => line,
                };
                manager
                    .change(&id, |snapshot| {
                        if let Some(progress) = progress_seconds(&line) {
                            snapshot.progress_seconds = Some(
                                snapshot
                                    .duration_seconds
                                    .map_or(progress, |duration| progress.min(duration)),
                            );
                        }
                        if let (Some(value), Some(frame_seconds)) = (
                            svt_frame_counter(&line).or_else(|| x264::frame_counter(&line)),
                            frame_seconds,
                        ) {
                            let progress = value as f64 * frame_seconds;
                            snapshot.progress_seconds = Some(
                                snapshot
                                    .duration_seconds
                                    .map_or(progress, |duration| progress.min(duration)),
                            );
                        }
                        append_log(snapshot, line);
                    })
                    .await;
            }
        })
    }
}

fn svt_frame_counter(line: &str) -> Option<u64> {
    line.split_once("Encoding:")
        .map(|(_, rest)| rest)
        .or_else(|| line.split_once("Encoding frame").map(|(_, rest)| rest))?
        .split_whitespace()
        .next()?
        .split('/')
        .next()?
        .parse()
        .ok()
}

#[allow(clippy::too_many_arguments)]
/// Later mux steps may replace a stream's copy with a conversion (for example
/// `-c:2 mov_text`). Keep only the last codec option for each output stream so
/// FFmpeg receives exactly one.
fn keep_last_codec_options(args: &mut Vec<OsString>) {
    let stream = |arg: &OsString| {
        arg.to_str()
            .and_then(|text| {
                text.strip_prefix("-c:")
                    .or_else(|| text.strip_prefix("-codec:"))
            })
            .and_then(|index| index.parse::<usize>().ok())
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut superseded = std::collections::BTreeSet::new();
    for position in (0..args.len().saturating_sub(1)).rev() {
        if let Some(stream) = stream(&args[position])
            && !seen.insert(stream)
        {
            superseded.extend([position, position + 1]);
        }
    }
    let mut position = 0;
    args.retain(|_| {
        let keep = !superseded.contains(&position);
        position += 1;
        keep
    });
}

/// Container, stream and settings checks that need only stream headers.
async fn preflight_streams(
    source: &Source,
    request: &RemuxRequest,
    settings: &EncodeSettings,
    document: &Document,
    output: &Path,
    ffprobe: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<super::external_tracks::ExternalTracks, AppError> {
    let external = super::external_tracks::ExternalTracks::prepare(
        source, request, settings, document, ffprobe, cancel,
    )
    .await?;
    let output_document = external.extend_document(document, settings);
    let mux_settings = external.mux_settings(settings);
    let indices = external.output_indices(document, request.stream_indices.clone(), settings)?;
    let selected = output_document.selected(&indices)?;
    audio::validate_selection(&selected, &mux_settings)?;
    super::trim::validate_selection(&selected, &mux_settings)?;
    super::subtitles::validate_selection(&selected, &mux_settings)?;
    container::preflight(output, &output_document, &selected, Some(&mux_settings))?;
    Ok(external)
}

pub(in crate::jobs) async fn frame_scan(
    ffprobe: &Path,
    input: &Path,
    plan: &mut Plan,
    stream: &metadata::Stream,
    encoded: bool,
    cancel: &watch::Receiver<bool>,
    progress: Option<watch::Sender<f64>>,
) -> Result<usize, AppError> {
    check_cancel(cancel)?;
    let threads = scan_threads();
    let args = if encoded {
        frame_scan_args(input, stream.index, threads)
    } else {
        source_scan_args(input, stream.index, threads, plan)
    };
    validate_frames(
        ffprobe, input, args, plan, stream, encoded, None, cancel, progress,
    )
    .await
}

/// Decodes and validates the first `frames` source frames in presentation
/// order. FFprobe stops reading after enough packets to output them, with the
/// same reorder margin the encoded-output sample uses.
pub(in crate::jobs) async fn source_prefix_scan(
    ffprobe: &Path,
    input: &Path,
    plan: &mut Plan,
    stream: &metadata::Stream,
    frames: usize,
    cancel: &watch::Receiver<bool>,
) -> Result<usize, AppError> {
    check_cancel(cancel)?;
    let mut args = source_scan_args(input, stream.index, scan_threads(), plan);
    let input_position = args
        .iter()
        .position(|arg| arg == "-i")
        .expect("frame scan input");
    args.splice(
        input_position..input_position,
        [
            "-read_intervals".into(),
            format!("%+#{}", frames.saturating_add(SAMPLE_PACKETS)).into(),
        ],
    );
    validate_frames(
        ffprobe,
        input,
        args,
        plan,
        stream,
        false,
        Some(frames),
        cancel,
        None,
    )
    .await
}

fn scan_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8)
}

/// Frames that fast output verification decodes and validates. They carry the
/// stream's per-frame properties (dimensions, aspect, bit depth, scan, color,
/// HDR side data); packet timestamps give every frame's clock.
const SAMPLE_FRAMES: usize = 16;

/// Packets read for the sample. A decoder outputs frames in presentation
/// order, but a reordered (B-frame) stream decodes some early frames from
/// later packets: stopping after `SAMPLE_FRAMES` packets would leave gaps. H.264
/// and HEVC reorder by at most 16 pictures, so the first `SAMPLE_FRAMES`
/// frames decoded from this many packets are complete.
const SAMPLE_PACKETS: usize = 64;

/// Decodes and validates the first frames of an encoded stream without
/// changing `plan`. Returns the number of frames checked.
async fn frame_sample(
    ffprobe: &Path,
    input: &Path,
    plan: &Plan,
    stream: &metadata::Stream,
    cancel: &watch::Receiver<bool>,
) -> Result<usize, AppError> {
    check_cancel(cancel)?;
    let mut args = frame_scan_args(input, stream.index, scan_threads());
    let input_position = args
        .iter()
        .position(|arg| arg == "-i")
        .expect("frame scan input");
    args.splice(
        input_position..input_position,
        [
            "-read_intervals".into(),
            format!("%+#{SAMPLE_PACKETS}").into(),
        ],
    );
    let mut sampled = plan.clone();
    validate_frames(
        ffprobe,
        input,
        args,
        &mut sampled,
        stream,
        true,
        Some(SAMPLE_FRAMES),
        cancel,
        None,
    )
    .await
}

/// Validates the frames that `args` make FFprobe decode: every frame, or the
/// first `limit` in presentation order (later ones are read and ignored).
#[allow(clippy::too_many_arguments)]
async fn validate_frames(
    ffprobe: &Path,
    input: &Path,
    args: Vec<OsString>,
    plan: &mut Plan,
    stream: &metadata::Stream,
    encoded: bool,
    limit: Option<usize>,
    cancel: &watch::Receiver<bool>,
    progress: Option<watch::Sender<f64>>,
) -> Result<usize, AppError> {
    let mut validation = Validation::new(plan.clone(), stream.clone(), encoded);
    let mut remaining = limit.unwrap_or(usize::MAX);
    let output = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(24 * 60 * 60),
        move |reader| {
            let mut last_update = std::time::Instant::now();
            frame_scan::parse(reader, |frame| {
                if remaining == 0 {
                    return;
                }
                remaining -= 1;
                validation.push(&frame);
                if last_update.elapsed() >= Duration::from_millis(500) {
                    if let Some(progress) = &progress {
                        progress.send_replace(validation.progress_seconds());
                    }
                    last_update = std::time::Instant::now();
                }
            })?;
            if let Some(progress) = &progress {
                progress.send_replace(validation.progress_seconds());
            }
            Ok(validation.finish())
        },
    )
    .await
    .map_err(|error| match error {
        SupervisorError::OutputParse(detail) => files::error(
            "PROBE_INVALID_RESPONSE",
            format!("The frame scan did not return valid metadata: {detail}"),
            input,
        ),
        error => process_error(error, input),
    })?;
    check_cancel(cancel)?;
    if !output.status.success() || !output.stderr.is_empty() {
        let detail: String = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(600)
            .collect();
        return Err(files::error(
            "DECODE_VALIDATION_FAILED",
            format!("The complete video could not be decoded without errors: {detail}"),
            input,
        ));
    }
    let (validated, count) = output.value.map_err(|error| {
        if encoded {
            files::error(
                "ENCODE_VALIDATION_FAILED",
                format!("Encoded frame validation failed: {}", error.message),
                input,
            )
        } else {
            error
        }
    })?;
    *plan = validated;
    Ok(count)
}

/// Source frame scan arguments, selecting libvpx for VP9 alpha sources.
pub(in crate::jobs) fn source_scan_args(
    input: &Path,
    index: u32,
    threads: usize,
    plan: &Plan,
) -> Vec<OsString> {
    let mut args = frame_scan_args(input, index, threads);
    if plan.source_is_vp9_alpha() {
        let input_position = args
            .iter()
            .position(|arg| arg == "-i")
            .expect("frame scan input");
        args.splice(
            input_position..input_position,
            ["-c:v".into(), "libvpx-vp9".into()],
        );
    }
    args
}

pub(in crate::jobs) fn frame_scan_args(input: &Path, index: u32, threads: usize) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-err_detect",
        "explode",
        "-select_streams",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(index.to_string().into());
    args.extend([
        format!("-threads:{}", index).into(),
        threads.to_string().into(),
    ]);
    args.extend(["-show_frames","-show_entries","frame=best_effort_timestamp_time,interlaced_frame,top_field_first,width,height,pix_fmt,sample_aspect_ratio,chroma_location,color_space,color_transfer,color_primaries,color_range:frame_side_data=side_data_type,displaymatrix,rotation,red_x,red_y,green_x,green_y,blue_x,blue_y,white_point_x,white_point_y,max_luminance,min_luminance,max_content,max_average,dv_profile,dv_bl_signal_compatibility_id,bl_present_flag,rpu_present_flag,el_present_flag,bl_bit_depth,bl_video_full_range_flag,disable_residual_flag","-of","json","-i"].into_iter().map(OsString::from));
    args.push(input.as_os_str().to_owned());
    args
}

/// Whether the user asked to decode every frame of the finished video before
/// publishing. Plans saved without the preference use fast verification.
fn full_output_verification(settings: &media_core::EncodeSettings) -> bool {
    settings
        .encoding_preferences
        .is_some_and(|preferences| preferences.full_output_verification)
}

/// Whether the encoded output is decoded completely. A source-queue plan has
/// per-region clocks that the constant-rate packet check cannot represent, so
/// it is always decoded.
fn decodes_output(settings: &media_core::EncodeSettings, plan: &Plan) -> bool {
    full_output_verification(settings) || plan.has_source_queue()
}

/// Proves that stream `index` of `staging` decodes to the same frames as
/// `verified`, a video-only file that was decoded completely with every frame
/// validated against `plan`, without decoding it again. Every packet must be
/// byte-identical and in the same order, the codec configuration must set up
/// the decoder identically, the stream side data must be unchanged, and each
/// presentation time must sit on the planned clock.
///
/// The staging mux may still set container fields such as aspect ratio,
/// color, and chroma siting; the decoder reports them for every frame. The
/// caller validates them against the plan on the staging stream.
///
/// Returns `false`, and the caller decodes the staging file, when the codec
/// configuration or side data changed. A packet or clock mismatch is an error.
async fn prove_copied_video(
    ffprobe: &Path,
    verified: &Path,
    staging: &Path,
    index: u32,
    plan: &Plan,
    frames: usize,
    cancel: &watch::Receiver<bool>,
) -> Result<bool, AppError> {
    let reference = probe(ffprobe, verified, cancel, None).await?;
    let copy = probe(ffprobe, staging, cancel, None).await?;
    let (Some(reference), Some(copy)) = (
        reference.streams.iter().find(|stream| stream.index == 0),
        copy.streams.iter().find(|stream| stream.index == index),
    ) else {
        return Ok(false);
    };
    if reference.codec_type.as_deref() != Some("video")
        || copy.codec_type.as_deref() != Some("video")
        || reference.side_data_list != copy.side_data_list
        || !super::codec_config::equivalent(ffprobe, (verified, reference), (staging, copy), cancel)
            .await?
    {
        return Ok(false);
    }
    super::mux::verify_shifted_video_packets(
        ffprobe,
        verified,
        0,
        staging,
        index,
        plan.encoded_mux_origin() - plan.output_origin,
        cancel,
    )
    .await?;
    super::packet_clock::verify(
        ffprobe,
        staging,
        index,
        super::packet_clock::Expected {
            origin: plan.encoded_mux_origin(),
            frame_seconds: plan.frame_seconds(),
            tolerance: super::encode_plan::encoded_timestamp_tolerance(copy),
            frames,
        },
        cancel,
    )
    .await?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
async fn validate_video_checkpoint(
    ffprobe: &Path,
    checkpoint: &Temporary,
    source: &metadata::Stream,
    plan: &mut Plan,
    expected_frames: &mut usize,
    producer: Option<super::source_filters::ProducerClock>,
    decode: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let document = if plan.encoder == VideoEncoder::X265Standalone {
        // Raw HEVC has no presentation timestamps until mkvmerge assigns the
        // verified rational clock. Probe its codec and color fields here,
        // then require exact decoded frame count before the timing wrapper.
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: ffprobe.to_owned(),
                args: [
                    "-v",
                    "error",
                    "-show_streams",
                    "-show_format",
                    "-of",
                    "json",
                    "-i",
                ]
                .into_iter()
                .map(OsString::from)
                .chain([checkpoint.path.as_os_str().to_owned()])
                .collect(),
                cwd: None,
            },
            cancel.clone(),
            512 * 1024,
            Duration::from_secs(20),
        )
        .await
        .map_err(|error| process_error(error, &checkpoint.path))?;
        if !result.status.success() {
            return Err(files::error(
                "ENCODE_VALIDATION_FAILED",
                "The raw HEVC checkpoint could not be inspected before timing is assigned.",
                &checkpoint.path,
            ));
        }
        serde_json::from_slice::<metadata::Document>(&result.stdout).map_err(|error| {
            files::error(
                "PROBE_INVALID_RESPONSE",
                error.to_string(),
                &checkpoint.path,
            )
        })?
    } else {
        probe(ffprobe, &checkpoint.path, cancel, Some(&[0])).await?
    };
    let encoded = document.streams.first().ok_or_else(|| {
        files::error(
            "ENCODE_VALIDATION_FAILED",
            "The complete video checkpoint has no decodable video stream.",
            &checkpoint.path,
        )
    })?;
    if let Some(producer) = &producer {
        plan.observe_source_filter_stream(encoded, Some(producer))?;
        *expected_frames = usize::try_from(producer.frames).map_err(|_| {
            files::error(
                "ENCODE_VALIDATION_FAILED",
                "The filtered producer frame count exceeds this platform.",
                &checkpoint.path,
            )
        })?;
    }
    plan.validate_elementary_checkpoint(source, encoded)?;
    // Full verification decodes every frame. Fast verification counts packets:
    // these encoders write exactly one packet (access unit, temporal unit, or
    // superframe) per frame.
    let (count, entry) = if decode {
        ("-count_frames", "stream=nb_read_frames")
    } else {
        ("-count_packets", "stream=nb_read_packets")
    };
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-err_detect",
        "explode",
        "-select_streams",
        "0",
        count,
        "-show_entries",
        entry,
        "-of",
        "default=nokey=1:noprint_wrappers=1",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(checkpoint.path.as_os_str().to_owned());
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(24 * 60 * 60),
    )
    .await
    .map_err(|cause| process_error(cause, &checkpoint.path))?;
    let frames = String::from_utf8_lossy(&result.stdout)
        .trim()
        .parse::<usize>()
        .ok();
    if !result.status.success() || !result.stderr.is_empty() || frames != Some(*expected_frames) {
        return Err(files::error(
            "ENCODE_VALIDATION_FAILED",
            if decode {
                format!(
                    "The complete video checkpoint did not decode to the planned {expected_frames} frames."
                )
            } else {
                format!(
                    "The complete video checkpoint does not contain the planned {expected_frames} frames."
                )
            },
            &checkpoint.path,
        ));
    }
    Ok(())
}

pub(super) fn decoder_args(input: &Path, plan: &Plan) -> Vec<OsString> {
    decoder_args_with_subtitles(input, plan, None, None)
}

/// Decode an already-trimmed and fully processed QTGMC intermediate. Source
/// temporal, tone, geometry and aspect filters have all run exactly once while
/// producing this FFV1 file, so this stage only emits its validated cadence and
/// pixel format as Y4M for the selected encoder.
pub(super) fn decoder_args_preprocessed(input: &Path, plan: &Plan) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "warning",
        "-xerror",
        "-err_detect",
        "explode",
        "-noautorotate",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(input.as_os_str().to_owned());
    args.extend([
        "-map".into(),
        "0:0".into(),
        "-an".into(),
        "-sn".into(),
        "-dn".into(),
        "-fps_mode".into(),
        "passthrough".into(),
        "-pix_fmt".into(),
        plan.output_pixel_format.into(),
        "-color_range".into(),
        if plan.full_range { "pc" } else { "tv" }.into(),
    ]);
    args.extend(super::source_filters::arguments());
    if plan.has_source_filters()
        && let Some(position) = args.iter().position(|argument| argument == "-fps_mode")
    {
        args.drain(position..position + 2);
    }
    if plan.has_alpha() {
        args.extend(["-f".into(), "rawvideo".into(), "pipe:1".into()]);
    } else {
        args.extend([
            "-strict".into(),
            "-1".into(),
            "-f".into(),
            "yuv4mpegpipe".into(),
            "pipe:1".into(),
        ]);
    }
    args
}

fn decoder_args_with_subtitles(
    input: &Path,
    plan: &Plan,
    text: Option<&str>,
    bitmap: Option<(&Path, u32, f64)>,
) -> Vec<OsString> {
    let seek = plan.trim.and_then(|trim| trim.time).filter(|time| {
        time.seek_strategy == media_core::VideoSeekStrategy::InputSeek
            && time.start_milliseconds > 0
    });
    // Accurate input seeking still decodes from the preceding keyframe, then
    // discards earlier frames. Its decoded timestamps are relative to the seek
    // position, so the time filter must use the same new origin exactly once.
    let source_bitmap = bitmap.is_some() && plan.source_trim;
    let mut sought_plan;
    let plan = if seek.is_some() && (bitmap.is_none() || source_bitmap) {
        sought_plan = plan.clone();
        let relative = sought_plan
            .trim
            .as_mut()
            .expect("time trim")
            .time
            .as_mut()
            .expect("time trim");
        relative.end_milliseconds -= relative.start_milliseconds;
        relative.start_milliseconds = 0;
        if sought_plan.source_dolby_render {
            sought_plan.source_origin = 0.0;
        }
        &sought_plan
    } else {
        plan
    };
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "warning",
        "-xerror",
        "-err_detect",
        "explode",
        "-noautorotate",
        "-protocol_whitelist",
        "file",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.extend(plan.tone_map_device_args());
    if plan.source_is_vp9_alpha() {
        args.extend(["-c:v".into(), "libvpx-vp9".into()]);
    }
    if let Some(time) = seek {
        args.extend([
            "-ss".into(),
            format!(
                "{}.{:03}",
                time.start_milliseconds / 1000,
                time.start_milliseconds % 1000
            )
            .into(),
            "-accurate_seek".into(),
        ]);
    }
    args.push("-i".into());
    args.push(input.as_os_str().to_owned());
    if let Some((path, bitmap, offset)) = bitmap {
        let shared_source = source_bitmap && path == input && offset == 0.0;
        if !shared_source {
            if source_bitmap {
                if let Some(time) = seek {
                    args.extend([
                        "-ss".into(),
                        format!(
                            "{}.{:03}",
                            time.start_milliseconds / 1000,
                            time.start_milliseconds % 1000
                        )
                        .into(),
                    ]);
                }
            } else {
                args.insert(0, "-copyts".into());
            }
            args.extend([
                "-itsoffset".into(),
                format!("{offset:.6}").into(),
                "-protocol_whitelist".into(),
                "file".into(),
                "-i".into(),
                path.as_os_str().to_owned(),
            ]);
        }
        let filter = if source_bitmap {
            let mut original = plan.clone();
            original.trim = None;
            let trim = plan
                .trim
                .filter(|_| !plan.uses_source_output_trim())
                .map(|trim| {
                    if let Some(time) = trim.time {
                        format!(
                            "trim=start={:.3}:end={:.3},setpts=PTS-STARTPTS",
                            f64::from(time.start_milliseconds) / 1000.0,
                            f64::from(time.end_milliseconds) / 1000.0
                        )
                    } else {
                        format!(
                            "trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS",
                            trim.start_frame, trim.end_frame_exclusive
                        )
                    }
                });
            let geometry = plan.geometry_filter_with_text(text);
            let suffix = trim
                .into_iter()
                .chain(geometry)
                .collect::<Vec<_>>()
                .join(",");
            super::subtitles::source_bitmap_filter(
                &format!("0:{}", plan.video_index),
                original.decoder_prefix().as_deref(),
                (!suffix.is_empty()).then_some(suffix.as_str()),
                plan.output_bit_depth(),
                usize::from(!shared_source),
                bitmap,
            )
        } else {
            super::subtitles::bitmap_filter(
                &format!("0:{}", plan.video_index),
                plan.decoder_prefix().as_deref(),
                plan.geometry_filter_with_text(text).as_deref(),
                plan.output_bit_depth(),
                bitmap,
            )
        };
        args.extend([
            "-filter_complex".into(),
            filter.into(),
            "-map".into(),
            "[jesses_video]".into(),
        ]);
    } else {
        args.extend(["-map".into(), format!("0:{}", plan.video_index).into()]);
        let filter = if text.is_none() {
            plan.decoder_filter()
        } else {
            plan.decoder_filter_with_text(text)
        };
        if let Some(filter) = filter {
            args.extend(["-vf".into(), filter.into()]);
        }
    }
    // All source frames already match this cadence within one timestamp tick.
    // Use it for the Y4M header as well as the encoder, so a reconciled decimal rate
    // cannot be overwritten by the input's declared nominal rate.
    if plan.cadence_reconciled {
        args.extend([
            "-r".into(),
            format!("{}/{}", plan.fps_num, plan.fps_den).into(),
        ]);
    }
    args.extend(
        [
            "-an",
            "-sn",
            "-dn",
            "-fps_mode",
            if plan.cadence_reconciled {
                "cfr"
            } else {
                "passthrough"
            },
            "-pix_fmt",
            plan.output_pixel_format,
            "-color_range",
            if plan.full_range { "pc" } else { "tv" },
        ]
        .into_iter()
        .map(OsString::from),
    );
    args.extend(plan.source_output_trim());
    args.extend(super::source_filters::arguments());
    if plan.has_source_filters()
        && let Some(position) = args.iter().position(|argument| argument == "-fps_mode")
    {
        args.drain(position..position + 2);
    }
    if plan.has_alpha() {
        args.extend(["-f".into(), "rawvideo".into(), "pipe:1".into()]);
    } else {
        args.extend([
            "-strict".into(),
            "-1".into(),
            "-f".into(),
            "yuv4mpegpipe".into(),
            "pipe:1".into(),
        ]);
    }
    args
}

pub(super) fn encoder_parameters(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let svt_crf = settings
        .svt_crf_quarter_steps
        .map(|quarters| {
            if quarters % 4 == 0 {
                (quarters / 4).to_string()
            } else {
                format!("{}.{:02}", quarters / 4, (quarters % 4) * 25)
            }
        })
        .unwrap_or_else(|| settings.crf.to_string());
    let svt_preset = settings
        .svt_preset
        .map_or_else(|| settings.preset.to_string(), |value| value.to_string());
    let values = [
        "--input-depth".into(),
        plan.output_bit_depth().to_string(),
        "--crf".into(),
        svt_crf,
        "--preset".into(),
        svt_preset,
        "--fps-num".into(),
        plan.fps_num.to_string(),
        "--fps-denom".into(),
        plan.fps_den.to_string(),
        "--color-primaries".into(),
        plan.primaries.to_string(),
        "--transfer-characteristics".into(),
        plan.transfer.to_string(),
        "--matrix-coefficients".into(),
        plan.matrix.to_string(),
        "--color-range".into(),
        u8::from(plan.full_range).to_string(),
        "--chroma-sample-position".into(),
        plan.chroma.into(),
    ];
    let mut args: Vec<OsString> = values.into_iter().map(OsString::from).collect();
    if settings.source_filters {
        for option in ["--fps-num", "--fps-denom"] {
            if let Some(position) = args.iter().position(|argument| argument == option) {
                args.drain(position..position + 2);
            }
        }
    }
    if settings.lossless {
        if let Some(index) = args.iter().position(|value| value == "--crf") {
            args.drain(index..index + 2);
        }
        args.extend(["--lossless".into(), "1".into()]);
    }
    args.extend(plan.hdr_arguments());
    if let Some(frames) = super::encoding_preferences::keyframes(plan, settings) {
        args.extend(["--keyint".into(), frames.to_string().into()]);
    }
    // Keep the coded source texture. Synthesis is an explicit setting and never
    // enables SVT's source denoiser, regardless of encoder version defaults.
    args.extend([
        "--film-grain".into(),
        settings.film_grain.to_string().into(),
        "--film-grain-denoise".into(),
        "0".into(),
    ]);
    match settings.encoder {
        VideoEncoder::SvtAv1FiveFish => args.extend([
            "--lineart-psy-bias".into(),
            settings.lineart_psy_bias.to_string().into(),
            "--texture-psy-bias".into(),
            settings.texture_psy_bias.to_string().into(),
        ]),
        VideoEncoder::SvtAv1Hdr
            if !settings.parameters.iter().any(|value| value.name == "tune") =>
        {
            args.extend([
                "--tune".into(),
                match settings.hdr_tune {
                    media_core::HdrTune::VisualQuality => "0",
                    media_core::HdrTune::FilmGrain => "5",
                }
                .into(),
            ])
        }
        _ => {}
    }
    args.extend(super::parameters::arguments(settings));
    if super::parameters::requires_serial_svt(settings) {
        args.extend(["--lp".into(), "1".into()]);
    }
    args
}

fn encoder_args(output: &Path, plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-i", "stdin", "--progress", "2", "--passes", "1"]
        .into_iter()
        .map(OsString::from)
        .collect();
    args.extend(encoder_parameters(plan, settings));
    super::dynamic_hdr::Resources::parameters(&mut args, settings);
    super::av1an::grain_parameters(&mut args, settings);
    args.push("-b".into());
    args.push(output.as_os_str().to_owned());
    args
}

pub(in crate::jobs) fn consumer_arguments(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    match settings.encoder {
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
            encoder_args(Path::new("stdout"), plan, settings)
        }
        VideoEncoder::X264 => x264::arguments(plan, settings),
        VideoEncoder::X265 | VideoEncoder::Vp9 => ffmpeg_video::arguments(plan, settings),
        VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone => aom_vpx::arguments(plan, settings),
        VideoEncoder::X265Standalone => x265::arguments(plan, settings),
        VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc => {
            ffmpeg_video::arguments(plan, settings)
        }
    }
}

/// av1an's Y4M pipe uses generic 4:2:0 tokens that do not carry source chroma
/// siting. Test the selected aomenc with exactly that token and the explicit
/// left-position flag before creating a resumable encoding workspace.
async fn check_av1an_aom_y4m(
    encoder: &Path,
    output: &Path,
    depth: u8,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    use std::io::Write;
    let id = format!(
        "aom-y4m-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    );
    let input = Temporary::create_extension(output, &id, "y4m")?;
    let mut bytes = format!(
        "YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C{}\nFRAME\n",
        if depth == 10 { "420p10" } else { "420" }
    )
    .into_bytes();
    if depth == 10 {
        for (sample, count) in [(64u16, 64 * 64), (512u16, 2 * 32 * 32)] {
            for _ in 0..count {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }
    } else {
        bytes.extend(std::iter::repeat_n(16u8, 64 * 64));
        bytes.extend(std::iter::repeat_n(128u8, 2 * 32 * 32));
    }
    let mut file = input.clone_file()?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            files::error(
                "ENCODER_CAPABILITY_UNSUPPORTED",
                error.to_string(),
                &input.path,
            )
        })?;
    drop(file);
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: encoder.to_owned(),
            args: vec![
                "--ivf".into(),
                "--output=-".into(),
                "--passes=1".into(),
                "--end-usage=q".into(),
                "--cq-level=63".into(),
                "--cpu-used=8".into(),
                "--profile=0".into(),
                format!("--bit-depth={depth}").into(),
                format!("--input-bit-depth={depth}").into(),
                "--chroma-sample-position=vertical".into(),
                "--disable-warning-prompt".into(),
                input.path.as_os_str().to_owned(),
            ],
            cwd: None,
        },
        cancel.clone(),
        128 * 1024,
        Duration::from_secs(15),
    )
    .await
    .map_err(|error| process_error(error, encoder))?;
    if !result.status.success() || !result.stdout.starts_with(b"DKIF") {
        return Err(files::error(
            "ENCODER_CAPABILITY_UNSUPPORTED",
            format!(
                "The selected aomenc cannot encode {depth}-bit av1an Y4M with explicitly left-positioned chroma. Install a compatible aomenc build and refresh Tools."
            ),
            encoder,
        ));
    }
    Ok(())
}

async fn check_encoder_capabilities(
    encoder: &Path,
    output: &Path,
    plan: &Plan,
    settings: &EncodeSettings,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    if settings.encoder.is_ffmpeg() {
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: encoder.to_owned(),
                args: vec![
                    "-hide_banner".into(),
                    "-h".into(),
                    format!("encoder={}", ffmpeg_video::library(settings.encoder)).into(),
                ],
                cwd: None,
            },
            cancel.clone(),
            512 * 1024,
            Duration::from_secs(5),
        )
        .await
        .map_err(|error| process_error(error, encoder))?;
        let help = format!(
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let capabilities =
            ffmpeg_video::validate_help(&help, settings.encoder, ffmpeg_video::output_format(plan));
        super::parameters::check_help(encoder, settings, &help, cancel).await?;
        if !result.status.success() || capabilities.is_err() {
            return Err(files::error(
                "ENCODER_CAPABILITY_UNSUPPORTED",
                capabilities
                    .err()
                    .unwrap_or_else(|| "FFmpeg failed its video encoder capability check.".into()),
                encoder,
            ));
        }
        return Ok(());
    }
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: encoder.to_owned(),
            args: vec![
                match settings.encoder {
                    VideoEncoder::SvtAv1
                    | VideoEncoder::SvtAv1FiveFish
                    | VideoEncoder::SvtAv1Hdr => "--help",
                    VideoEncoder::X264 => "--fullhelp",
                    VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone => "--help",
                    VideoEncoder::X265Standalone => "--fullhelp",
                    VideoEncoder::X265
                    | VideoEncoder::Vp9
                    | VideoEncoder::H264Nvenc
                    | VideoEncoder::HevcNvenc => unreachable!("FFmpeg checked above"),
                }
                .into(),
            ],
            cwd: None,
        },
        cancel.clone(),
        512 * 1024,
        Duration::from_secs(5),
    )
    .await
    .map_err(|e| process_error(e, encoder))?;
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    super::parameters::check_help(encoder, settings, &help, cancel).await?;
    if settings.encoder == VideoEncoder::X264 {
        let capabilities = x264::validate_help(
            &help,
            plan.output_bit_depth(),
            x264::output_csp(plan.output_pixel_format),
        );
        if settings.lossless && !help.split_whitespace().any(|word| word == "--qp") {
            return Err(files::error(
                "ENCODER_CAPABILITY_UNSUPPORTED",
                "The installed x264 does not advertise the explicit --qp 0 lossless mode.",
                encoder,
            ));
        }
        if settings.rate_control.is_some()
            && ["--bitrate", "--pass", "--stats"]
                .iter()
                .any(|option| !help.split_whitespace().any(|word| word == *option))
        {
            return Err(files::error(
                "ENCODER_CAPABILITY_UNSUPPORTED",
                "The installed x264 does not advertise bitrate and external pass statistics support.",
                encoder,
            ));
        }
        if !result.status.success() || capabilities.is_err() {
            return Err(files::error(
                "ENCODER_CAPABILITY_UNSUPPORTED",
                capabilities
                    .err()
                    .unwrap_or_else(|| "The installed x264 failed its capability check.".into()),
                encoder,
            ));
        }
        return Ok(());
    }
    if matches!(
        settings.encoder,
        VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone
    ) {
        let capabilities = aom_vpx::validate_help(
            &help,
            settings.encoder,
            plan.output_bit_depth(),
            settings.lossless,
            plan.output_pixel_format,
        );
        if !result.status.success() || capabilities.is_err() {
            return Err(files::error(
                "ENCODER_CAPABILITY_UNSUPPORTED",
                capabilities.err().unwrap_or_else(|| {
                    "The installed standalone AOM/VPX encoder failed its capability check.".into()
                }),
                encoder,
            ));
        }
        if settings.encoder == VideoEncoder::AomAv1
            && (settings.film_grain > 0 || settings.av1an_grain.is_some())
        {
            let table = settings
                .av1an_grain
                .as_ref()
                .is_some_and(|grain| grain.table.is_some());
            let denoise = settings
                .av1an_grain
                .as_ref()
                .is_some_and(|grain| grain.denoise);
            if (table && !help.contains("--film-grain-table="))
                || ((!table || denoise) && !help.contains("--denoise-noise-level="))
                || (denoise && !help.contains("--enable-dnl-denoising="))
            {
                return Err(files::error(
                    "ENCODER_CAPABILITY_UNSUPPORTED",
                    "The selected aomenc does not advertise the requested grain table or denoising options.",
                    encoder,
                ));
            }
        }
        if settings.backend == media_core::EncodeBackend::Av1an
            && settings.encoder == VideoEncoder::AomAv1
            && plan.chroma == "left"
            && plan.output_pixel_format.contains("420")
        {
            check_av1an_aom_y4m(encoder, output, plan.output_bit_depth(), cancel).await?;
        }
        return Ok(());
    }
    if settings.encoder == VideoEncoder::X265Standalone {
        let capabilities = x265::validate_help(&help, plan.output_bit_depth(), settings.lossless);
        // x265 intentionally exits non-zero after printing --help in some
        // official Windows builds. The validated help surface is authoritative.
        if capabilities.is_err() {
            return Err(files::error(
                "ENCODER_CAPABILITY_UNSUPPORTED",
                capabilities.err().unwrap_or_else(|| {
                    "The installed x265 CLI failed its capability check.".into()
                }),
                encoder,
            ));
        }
        if plan.is_hdr10()
            && (!(help.contains("--hdr10") || help.contains("--[no-]hdr10"))
                || !help.contains("--master-display")
                || !help.contains("--max-cll"))
        {
            return Err(files::error(
                "ENCODER_CAPABILITY_UNSUPPORTED",
                "The selected x265 does not advertise HDR10 mastering and content-light SEI options.",
                encoder,
            ));
        }
        return Ok(());
    }
    let mut required = vec!["--film-grain", "--film-grain-denoise"];
    if settings
        .av1an_grain
        .as_ref()
        .is_some_and(|grain| grain.table.is_some())
    {
        required.push("--fgs-table");
    }
    if settings.lossless {
        required.push("--lossless");
    }
    if let Some(metadata) = &settings.hdr_metadata {
        if metadata.dolby_vision_rpu_path.is_some() {
            required.push("--dolby-vision-rpu");
        }
        if metadata.hdr10_plus_json_path.is_some() {
            required.push("--hdr10plus-json");
        }
    }
    if settings.rate_control.is_some() {
        required.extend(["--rc", "--tbr", "--pass", "--stats", "--passes"]);
    }
    match settings.encoder {
        VideoEncoder::SvtAv1FiveFish => {
            required.extend(["--lineart-psy-bias", "--texture-psy-bias"])
        }
        VideoEncoder::SvtAv1Hdr => required.push("--tune"),
        _ => {}
    }
    if plan.is_hdr10() {
        required.extend(["--mastering-display", "--content-light"]);
    }
    if settings
        .svt_crf_quarter_steps
        .is_some_and(|value| value > 63 * 4)
        && !help
            .lines()
            .find(|line| line.split_whitespace().next() == Some("--crf"))
            .is_some_and(|line| line.contains("[1-70]"))
    {
        required.push("--crf-up-to-70");
    }
    if settings
        .svt_crf_quarter_steps
        .is_some_and(|value| value % 4 != 0)
        && !help.contains("0.25 increments")
    {
        required.push("--crf-quarter-steps");
    }
    if let Some(preset) = settings.svt_preset.filter(|value| *value < 0) {
        let supported = help
            .lines()
            .find(|line| line.split_whitespace().next() == Some("--preset"))
            .and_then(|line| line.rsplit_once('[')?.1.split_once(']')?.0.rsplit_once('-'))
            .and_then(|(minimum, maximum)| {
                Some((minimum.parse::<i8>().ok()?, maximum.parse::<i8>().ok()?))
            })
            .is_some_and(|(minimum, maximum)| (minimum..=maximum).contains(&preset));
        if !supported {
            required.push("--selected-research-preset");
        }
    }
    if !result.status.success()
        || required
            .iter()
            .any(|option| !help.split_whitespace().any(|word| word == *option))
    {
        return Err(files::error(
            "ENCODER_CAPABILITY_UNSUPPORTED",
            format!(
                "The installed standalone SVT encoder does not advertise every selected capability (lossless, CRF range/step, research preset, HDR/fork controls, or film grain level {}). Choose values shown by this build's help or install the matching build.",
                settings.film_grain,
            ),
            encoder,
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn mux_args(
    input: &Path,
    ivf: &Path,
    output: &Path,
    selected: &[&metadata::Stream],
    plan: &Plan,
    settings: &EncodeSettings,
    external: &super::external_tracks::ExternalTracks,
) -> Vec<OsString> {
    let effective_settings = external.mux_settings(settings);
    let settings = &effective_settings;
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "warning",
        "-xerror",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(input.as_os_str().to_owned());
    // A copied track can precede the selected video too (for example MP2
    // encoder delay in MPEG-TS). Preserve its negative canonical origin just
    // as strictly as converted audio and external-track timestamps.
    args.insert(0, "-copyts".into());
    if plan.restores_source_dolby_clock() {
        args.extend([
            "-itsoffset".into(),
            format!("{:.9}", plan.encoded_mux_origin()).into(),
        ]);
    }
    args.push("-i".into());
    args.push(ivf.as_os_str().to_owned());
    external.append_inputs(&mut args);
    for stream in selected {
        let (input_index, stream_index) = external.input_stream(stream.index);
        args.extend([
            "-map".into(),
            if stream.index == plan.video_index {
                "1:v:0".into()
            } else {
                format!("{input_index}:{stream_index}").into()
            },
        ]);
    }
    args.extend([
        "-map_metadata".into(),
        external.metadata_input_index(settings).to_string().into(),
        "-map_chapters".into(),
        external.chapters_input_index(settings).to_string().into(),
    ]);
    if settings
        .hdr_metadata
        .as_ref()
        .is_some_and(|metadata| metadata.dolby_vision_rpu_path.is_some())
    {
        let position = selected
            .iter()
            .position(|stream| stream.index == plan.video_index)
            .expect("selected video");
        // IVF has no container configuration record. The native FFmpeg filter
        // authors profile10 signalling; the final verifier checks every parsed
        // RPU field plus the configuration, including after MP4 conversion.
        args.extend([
            format!("-bsf:{position}").into(),
            "dovi_rpu=compression=none".into(),
        ]);
    }
    // MOV/MP4 brands and minor versions describe the container being written.
    // They cannot be copied into the verified Matroska staging file as tags.
    super::output_metadata::append(&mut args, settings);
    for key in ["major_brand", "minor_version", "compatible_brands"] {
        args.extend(["-metadata".into(), format!("{key}=").into()]);
    }
    for (index, stream) in selected.iter().enumerate() {
        let (input_index, stream_index) = external.input_stream(stream.index);
        let converted_audio =
            audio::converted(settings).find(|track| track.stream_index == stream.index);
        // One codec option per output stream: a global `-c copy` beside a
        // converted track's `-c:N` makes FFmpeg warn about multiple options.
        if let Some(track) = converted_audio {
            audio::append_arguments(&mut args, index, track, stream);
        } else {
            args.extend([format!("-c:{index}").into(), "copy".into()]);
        }
        if settings.copy_metadata != Some(false)
            || stream.codec_type.as_deref() == Some("attachment")
        {
            args.extend([
                format!("-map_metadata:s:{index}").into(),
                format!("{input_index}:s:{stream_index}").into(),
            ]);
        }
        let disposition = stream
            .disposition
            .iter()
            .filter(|(_, v)| **v != 0)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join("+");
        args.extend([
            format!("-disposition:{index}").into(),
            if disposition.is_empty() {
                "0".into()
            } else {
                disposition.into()
            },
        ]);
        if let Some(options) = external.track_metadata(stream.index, settings) {
            for (key, value) in [
                ("title", options.title.clone()),
                ("handler_name", options.title),
                ("language", options.language),
            ] {
                if let Some(value) = value {
                    args.extend([
                        format!("-metadata:s:{index}").into(),
                        format!("{key}={value}").into(),
                    ]);
                }
            }
        }
        if stream.index == plan.video_index {
            let (sar_num, sar_den) = plan
                .output_sar()
                .split_once(':')
                .and_then(|(num, den)| Some((num.parse::<u64>().ok()?, den.parse::<u64>().ok()?)))
                .expect("validated output SAR");
            let mut display_num = u64::from(plan.width) * sar_num;
            let mut display_den = u64::from(plan.height) * sar_den;
            let mut a = display_num;
            let mut b = display_den;
            while b != 0 {
                (a, b) = (b, a % b);
            }
            display_num /= a;
            display_den /= a;
            if !settings.source_filters && !plan.has_source_queue() {
                args.extend([
                    format!("-aspect:{index}").into(),
                    format!("{display_num}:{display_den}").into(),
                ]);
            }
            args.extend([
                format!("-color_primaries:{index}").into(),
                plan.primaries.to_string().into(),
                format!("-color_trc:{index}").into(),
                plan.transfer.to_string().into(),
                format!("-colorspace:{index}").into(),
                plan.matrix.to_string().into(),
                format!("-color_range:{index}").into(),
                if plan.full_range { "pc" } else { "tv" }.into(),
                format!("-chroma_sample_location:{index}").into(),
                plan.chroma.into(),
            ]);
            if plan.has_alpha() {
                // Stream metadata is copied from the original input above.
                // The encoded VP9 alpha flag must be explicitly restored after
                // that copy so the final Matroska advertises its alpha blocks.
                args.extend([format!("-metadata:s:{index}").into(), "alpha_mode=1".into()]);
            }
        }
        if stream.index == plan.video_index || converted_audio.is_some() {
            // Keep descriptive tags, but source bitrate/frame/byte statistics
            // and encoder provenance no longer describe this encoded stream.
            // Match the exact source key and output position, including when
            // selected tracks have been reordered; copied tracks are untouched.
            for key in stream
                .tags
                .keys()
                .filter(|key| metadata::is_derived_stream_tag(key))
            {
                args.extend([
                    format!("-metadata:s:{index}").into(),
                    format!("{key}=").into(),
                ]);
            }
        }
    }
    args.extend(["-avoid_negative_ts".into(), "disabled".into()]);
    if let Some(format) = container::source_iso_format(output, selected, settings) {
        container::append_format_options(&mut args, format, selected);
        if format == media_core::ContainerFormat::Mp4
            && settings
                .encoding_preferences
                .is_some_and(|preferences| preferences.mp4_faststart)
            && let Some(index) = args.iter().rposition(|argument| argument == "-movflags")
        {
            args[index + 1] = format!("{}+faststart", args[index + 1].to_string_lossy()).into();
        }
        let index = selected
            .iter()
            .position(|stream| stream.index == plan.video_index)
            .expect("source encoded picture");
        let scale = u64::from(plan.fps_num) * 1000;
        let step = u64::from(plan.fps_den) * 1000;
        args.extend([format!("-bsf:{index}").into(),format!("setts=pts=round(PTS/{step})*{step}:dts=round(DTS/{step})*{step}:duration={step}:time_base=1/{scale}:prescale=1").into(),"-f".into(),format.ffmpeg_format().into(),"-y".into()]);
    } else {
        container::append_staging_format(&mut args, selected, Some(settings));
    }
    args.push(output.as_os_str().to_owned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    #[ignore = "manual real-media qualification; set JESSES_VALIDATION_INPUT"]
    async fn validates_external_source_without_encoding() {
        let Some(input) = std::env::var_os("JESSES_VALIDATION_INPUT") else {
            eprintln!(
                "External source qualification was NOT run: set JESSES_VALIDATION_INPUT to an absolute media path. Synthetic media tests cover CI."
            );
            return;
        };
        let input = PathBuf::from(input);
        assert!(
            input.is_absolute(),
            "JESSES_VALIDATION_INPUT must be absolute"
        );
        let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
        let (_owner, cancel) = watch::channel(false);
        let document = probe(&ffprobe, &input, &cancel, None).await.unwrap();
        let video = document
            .streams
            .iter()
            .find(|stream| {
                stream.codec_type.as_deref() == Some("video")
                    && !stream
                        .disposition
                        .get("attached_pic")
                        .is_some_and(|value| *value != 0)
            })
            .expect("movie video stream");
        let fallback = std::env::var("JESSES_HDR10_FALLBACK")
            .ok()
            .is_some_and(|value| value == "true" || value == "1");
        let mut plan = Plan::build(
            &document,
            &[video],
            &EncodeSettings {
                video_stream_index: video.index,
                hdr10_fallback: fallback,
                ..EncodeSettings::default()
            },
        )
        .unwrap();
        let started = std::time::Instant::now();
        let (progress, mut updates) = watch::channel(0.0_f64);
        let observer = async {
            let mut last = std::time::Instant::now();
            while updates.changed().await.is_ok() {
                let scanned = *updates.borrow_and_update();
                if last.elapsed() >= Duration::from_secs(10) {
                    eprintln!(
                        "Source qualification: {scanned:.1} seconds of video scanned; {:.1}s elapsed",
                        started.elapsed().as_secs_f64()
                    );
                    last = std::time::Instant::now();
                }
            }
        };
        let (result, ()) = tokio::join!(
            frame_scan(
                &ffprobe,
                &input,
                &mut plan,
                video,
                false,
                &cancel,
                Some(progress)
            ),
            observer
        );
        let count = result.unwrap();
        eprintln!(
            "Complete source qualified: {count} frames, {}/{} fps, HDR10={}, cadence reconciled={}, {:.1}s elapsed",
            plan.fps_num,
            plan.fps_den,
            plan.is_hdr10(),
            plan.cadence_reconciled,
            started.elapsed().as_secs_f64()
        );
    }

    #[test]
    fn frame_scan_selects_the_video_decoder_and_bounded_thread_count() {
        let args = frame_scan_args(Path::new("movie.mkv"), 3, 8);
        assert!(args.windows(2).any(|pair| pair == ["-select_streams", "3"]));
        assert!(args.windows(2).any(|pair| pair == ["-threads:3", "8"]));
        assert!(!args.iter().any(|arg| arg == "-read_intervals"));
    }

    #[test]
    fn parses_supported_standalone_progress_formats() {
        assert_eq!(
            svt_frame_counter("[consumer] Encoding:   12/50 Frames @ 30fps"),
            Some(12)
        );
        assert_eq!(svt_frame_counter("Encoding frame 9 40 fps"), Some(9));
        assert_eq!(svt_frame_counter("Total Encoding Time: 300 ms"), None);
    }

    fn tag_value<'a>(stream: &'a metadata::Stream, key: &str) -> Option<&'a str> {
        stream
            .tags
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn encode_mux_clears_only_derived_tags_on_the_reordered_video_stream() {
        let document: Document = serde_json::from_value(serde_json::json!({
            "format":{"start_time":"0"},
            "streams":[
                {"index":3,"codec_type":"video","codec_name":"h264","width":128,"height":96,"pix_fmt":"yuv420p","sample_aspect_ratio":"1:1","avg_frame_rate":"24/1","start_time":"0","color_space":"bt709","color_primaries":"bt709","color_transfer":"bt709","color_range":"tv",
                    "tags":{"bPs":"1","DURATION":"stale","EnCoDeR":"old","NUMBER_OF_FRAMES":"2","NUMBER_OF_BYTES":"3","BPS-eng":"4","number_of_frames-jpn":"5","NUMBER_OF_BYTES-fra":"6","_Statistics_Tags":"old","title":"Picture","language":"eng","comment":"Keep me","encoder-notes":"Descriptive"}},
                {"index":7,"codec_type":"audio","tags":{"BPS":"768000","title":"Original audio"}}
            ]
        })).unwrap();
        let selected = document.selected(&[7, 3]).unwrap();
        let plan = Plan::build(
            &document,
            &selected,
            &EncodeSettings {
                video_stream_index: 3,
                ..EncodeSettings::default()
            },
        )
        .unwrap();
        let args = mux_args(
            Path::new("input.mkv"),
            Path::new("video.ivf"),
            Path::new("output.mkv"),
            &selected,
            &plan,
            &EncodeSettings::default(),
            &super::external_tracks::ExternalTracks::default(),
        );
        let cleared: Vec<_> = args
            .windows(2)
            .filter(|pair| pair[0] == "-metadata:s:1")
            .map(|pair| pair[1].to_str().unwrap())
            .collect();
        for expected in [
            "bPs=",
            "DURATION=",
            "EnCoDeR=",
            "NUMBER_OF_FRAMES=",
            "NUMBER_OF_BYTES=",
            "BPS-eng=",
            "number_of_frames-jpn=",
            "NUMBER_OF_BYTES-fra=",
            "_Statistics_Tags=",
        ] {
            assert!(cleared.contains(&expected), "{expected}");
        }
        assert_eq!(cleared.len(), 9);
        assert!(!args.iter().any(|arg| arg == "-metadata:s:0"));
    }

    /// FFmpeg warns "Multiple -codec options specified for stream N" when a
    /// global `-c copy` and a converted track's `-c:N` both match a stream.
    #[test]
    fn mux_gives_each_output_stream_exactly_one_codec_option() {
        let document: Document = serde_json::from_value(serde_json::json!({
            "format":{"start_time":"0"},
            "streams":[
                {"index":0,"codec_type":"video","codec_name":"h264","width":128,"height":96,"pix_fmt":"yuv420p","sample_aspect_ratio":"1:1","avg_frame_rate":"24/1","start_time":"0","color_space":"bt709","color_primaries":"bt709","color_transfer":"bt709","color_range":"tv"},
                {"index":1,"codec_type":"audio","codec_name":"eac3","channels":6,"channel_layout":"5.1(side)","sample_rate":"48000"},
                {"index":3,"codec_type":"subtitle","codec_name":"subrip"}
            ]
        })).unwrap();
        let selected = document.selected(&[0, 1, 3]).unwrap();
        let settings = EncodeSettings {
            audio: vec![
                serde_json::from_value(serde_json::json!({
                    "streamIndex":1,"codec":"opus","bitrateKbps":128,"channels":"stereo"
                }))
                .unwrap(),
            ],
            ..EncodeSettings::default()
        };
        let plan = Plan::build(&document, &selected, &settings).unwrap();
        let mut args = mux_args(
            Path::new("input.mkv"),
            Path::new("video.ivf"),
            Path::new("output.mkv"),
            &selected,
            &plan,
            &settings,
            &super::external_tracks::ExternalTracks::default(),
        );
        // A later mux step converts the subtitle.
        let last = args.len() - 1;
        args.splice(last..last, ["-c:2".into(), "mov_text".into()]);
        keep_last_codec_options(&mut args);
        let codecs: Vec<_> = args
            .windows(2)
            .filter(|pair| {
                let flag = pair[0].to_str().unwrap();
                flag == "-c" || flag == "-codec" || flag.starts_with("-c:")
            })
            .map(|pair| {
                format!(
                    "{} {}",
                    pair[0].to_str().unwrap(),
                    pair[1].to_str().unwrap()
                )
            })
            .collect();
        assert_eq!(codecs, ["-c:0 copy", "-c:1 libopus", "-c:2 mov_text"]);
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg with libx265, FFprobe, and standalone SVT-AV1"]
    async fn standalone_hdr10_preserves_static_metadata_with_grain_synthesis() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(
            std::env::temp_dir().join(format!("jesses-hdr10-{}-{nonce}", std::process::id())),
        );
        std::fs::create_dir(&fixture.0).unwrap();
        let input = fixture.0.join("hdr10-source.mkv");
        let output = fixture.0.join("hdr10-av1.mkv");
        let ffmpeg = find_executable(&["ffmpeg"]).await.unwrap().unwrap();
        let mut args:Vec<OsString> = ["-v","error","-nostdin","-n","-f","lavfi","-i","testsrc2=s=128x96:r=24","-t","0.5","-vf","setparams=field_mode=prog:range=tv:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc","-c:v","libx265","-preset","ultrafast","-pix_fmt","yuv420p10le","-color_range","tv","-colorspace","bt2020nc","-color_trc","smpte2084","-color_primaries","bt2020","-x265-params","log-level=error:pools=2:frame-threads=2:bframes=0:hdr10=1:chromaloc=2:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=200,142"].into_iter().map(OsString::from).collect();
        args.push(input.as_os_str().to_owned());
        let (_sender, cancel) = watch::channel(false);
        let generated = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg,
                args,
                cwd: None,
            },
            cancel,
            128 * 1024,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let original = std::fs::read(&input).unwrap();
        let manager = JobManager::new(fixture.0.join("logs"));
        manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    preset: 12,
                    film_grain: 8,
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        let job = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let job = manager.list_jobs().await.remove(0);
                if job.state.is_terminal() {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        manager.shutdown().await;
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        assert!(
            job.logs
                .iter()
                .any(|line| line.contains("HDR10: preserving"))
        );
        assert!(output.exists());
        assert_eq!(std::fs::read(&input).unwrap(), original);
        let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
        let (_sender, cancel) = watch::channel(false);
        let document = probe(&ffprobe, &output, &cancel, None).await.unwrap();
        assert_eq!(
            document.streams[0].color_transfer.as_deref(),
            Some("smpte2084")
        );
        let mut plan = Plan::build(
            &document,
            &document.selected(&[0]).unwrap(),
            &EncodeSettings::default(),
        )
        .unwrap();
        let count = frame_scan(
            &ffprobe,
            &output,
            &mut plan,
            &document.streams[0],
            false,
            &cancel,
            None,
        )
        .await
        .unwrap();
        assert_eq!(count, 12);
        assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".partial.")
        }));
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, and standalone SvtAv1EncApp"]
    async fn incompatible_tracks_fail_before_any_source_decode_pass() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "jesses-encode-preflight-{}-{nonce}",
            std::process::id()
        )));
        std::fs::create_dir(&fixture.0).unwrap();
        let cues = fixture.0.join("cues.srt");
        std::fs::write(&cues, "1\n00:00:00,000 --> 00:00:00,500\nHello\n").unwrap();
        let input = fixture.0.join("source.mp4");
        let output = fixture.0.join("output.mkv");
        let ffmpeg = find_executable(&["ffmpeg"]).await.unwrap().unwrap();
        // No color tags: the decoded-color enrichment pass would be needed.
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=128x96:r=24",
            "-i",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        args.push(cues.as_os_str().to_owned());
        args.extend(
            [
                "-t",
                "1",
                "-map",
                "0:v",
                "-map",
                "1:s",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-c:s",
                "mov_text",
            ]
            .into_iter()
            .map(OsString::from),
        );
        args.push(input.as_os_str().to_owned());
        let (_sender, cancel) = watch::channel(false);
        let generated = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg,
                args,
                cwd: None,
            },
            cancel,
            64 * 1024,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let manager = JobManager::new(fixture.0.join("logs"));
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0, 1],
                },
                settings: EncodeSettings {
                    preset: 12,
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        let job = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let job = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|entry| entry.id == job.id)
                    .unwrap();
                if job.state.is_terminal() {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        manager.shutdown().await;
        let error = job.error.as_ref().expect("incompatible copy fails");
        assert_eq!(error.code, "CONTAINER_INCOMPATIBLE", "{job:#?}");
        assert!(error.message.contains("SubRip"), "{}", error.message);
        assert!(
            !job.logs
                .iter()
                .any(|line| line.contains("decoded") || line.contains("Decoding")),
            "{:#?}",
            job.logs
        );
        assert!(!output.exists());
    }

    /// Packets with a constant clock can hide a decoded-frame defect. The
    /// encoder starts from the packet plan, but the complete decoded scan
    /// beside it must still fail the job and publish nothing.
    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, and standalone SvtAv1EncApp"]
    async fn decoded_defects_after_the_packet_plan_fail_without_publication() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "jesses-encode-background-{}-{nonce}",
            std::process::id()
        )));
        std::fs::create_dir(&fixture.0).unwrap();
        let ffmpeg = find_executable(&["ffmpeg"]).await.unwrap().unwrap();
        let (_sender, cancel) = watch::channel(false);
        let run = |args: Vec<String>| {
            let ffmpeg = ffmpeg.clone();
            let cancel = cancel.clone();
            async move {
                let result = supervisor::run_capture(
                    &CommandSpec {
                        executable: ffmpeg,
                        args: args.into_iter().map(OsString::from).collect(),
                        cwd: None,
                    },
                    cancel,
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
        };
        // Two Annex B segments with in-band parameter sets: the second one
        // declares different color primaries from the first frame onward.
        let mut joined = Vec::new();
        for (name, primaries) in [("a.h264", "bt709"), ("b.h264", "bt470bg")] {
            let path = fixture.0.join(name);
            let tags = format!(
                "setparams=field_mode=prog:range=tv:color_primaries={primaries}:color_trc=bt709:colorspace=bt709"
            );
            run([
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=128x96:r=24",
                "-t",
                "1",
                "-vf",
                &tags,
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-bf",
                "0",
                "-g",
                "24",
                "-pix_fmt",
                "yuv420p",
                "-color_range",
                "tv",
                "-colorspace",
                "bt709",
                "-color_trc",
                "bt709",
                "-color_primaries",
                primaries,
                "-chroma_sample_location",
                "left",
                "-f",
                "h264",
            ]
            .into_iter()
            .map(String::from)
            .chain([path.to_string_lossy().into_owned()])
            .collect())
            .await;
            joined.extend(std::fs::read(&path).unwrap());
        }
        let raw = fixture.0.join("joined.h264");
        std::fs::write(&raw, joined).unwrap();
        let input = fixture.0.join("source.mkv");
        run([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "h264",
            "-framerate",
            "24",
            "-i",
        ]
        .into_iter()
        .map(String::from)
        .chain([raw.to_string_lossy().into_owned()])
        .chain(
            ["-c", "copy", "-f", "matroska"]
                .into_iter()
                .map(String::from),
        )
        .chain([input.to_string_lossy().into_owned()])
        .collect())
        .await;
        let output = fixture.0.join("output.mkv");
        let manager = JobManager::new(fixture.0.join("logs"));
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    preset: 12,
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        let job = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let job = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|entry| entry.id == job.id)
                    .unwrap();
                if job.state.is_terminal() {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        manager.shutdown().await;
        assert_eq!(job.state, JobState::Failed, "{job:#?}");
        assert!(
            job.logs.iter().any(|line| line.starts_with("Planned 48 ")),
            "{:#?}",
            job.logs
        );
        let error = job.error.as_ref().unwrap();
        assert!(
            error.message.contains("color metadata differs"),
            "{error:?}\n{:#?}",
            job.logs
        );
        assert!(!output.exists());
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, and standalone SvtAv1EncApp"]
    async fn encodes_fine_timebase_mp4_into_millisecond_matroska() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(
            std::env::temp_dir().join(format!("jesses-encode-mp4-{}-{nonce}", std::process::id())),
        );
        std::fs::create_dir(&fixture.0).unwrap();
        let input = fixture.0.join("source.mp4");
        let output = fixture.0.join("output.mkv");
        let ffmpeg = find_executable(&["ffmpeg"]).await.unwrap().unwrap();
        let mut args:Vec<OsString>=["-v","error","-nostdin","-n","-f","lavfi","-i","testsrc2=s=128x96:r=24","-t","0.5","-vf","setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709","-c:v","libx264","-preset","ultrafast","-bf","0","-pix_fmt","yuv420p","-color_range","tv","-colorspace","bt709","-color_trc","bt709","-color_primaries","bt709","-chroma_sample_location","left"].into_iter().map(OsString::from).collect();
        args.push(input.as_os_str().to_owned());
        let (_sender, cancel) = watch::channel(false);
        let generated = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg,
                args,
                cwd: None,
            },
            cancel,
            64 * 1024,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let source_bytes = std::fs::read(&input).unwrap();
        let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
        let first = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    preset: 12,
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        let second_output = fixture.0.join("queued-second.mkv");
        let second = manager
            .enqueue_encode(EncodeRequest {
                source: RemuxRequest {
                    output_path: second_output.to_string_lossy().into_owned(),
                    ..first.request.clone()
                },
                settings: first.encode_settings.clone().unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(second.state, JobState::Queued);
        let jobs = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let jobs = manager.list_jobs().await;
                assert!(
                    jobs.iter()
                        .filter(|job| !job.state.is_terminal() && job.state != JobState::Queued)
                        .count()
                        <= 1,
                    "Only one job may execute at a time"
                );
                let first_job = jobs.iter().find(|job| job.id == first.id).unwrap();
                let second_job = jobs.iter().find(|job| job.id == second.id).unwrap();
                if second_job.state != JobState::Queued {
                    assert!(
                        first_job.state.is_terminal(),
                        "Queued work must execute FIFO"
                    );
                }
                if jobs.iter().all(|job| job.state.is_terminal()) {
                    return jobs;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        manager.shutdown().await;
        assert!(
            jobs.iter().all(|job| job.state == JobState::Succeeded),
            "{jobs:#?}"
        );
        drop(manager);
        let restored = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
        let persisted = restored.list_jobs().await;
        assert_eq!(persisted.len(), 2);
        assert!(persisted.iter().all(|job| job.state == JobState::Succeeded
            && job.encode_settings.as_ref() == first.encode_settings.as_ref()));
        restored.shutdown().await;
        assert_eq!(std::fs::read(&input).unwrap(), source_bytes);
        assert!(output.exists());

        let canceled_output = fixture.0.join("canceled.mkv");
        let cancel_manager = JobManager::new(fixture.0.join("cancel-logs"));
        let cancel_job = cancel_manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: canceled_output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    preset: 0,
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let job = cancel_manager.list_jobs().await.remove(0);
                assert!(
                    !job.state.is_terminal(),
                    "Expected to observe the live encoder before completion: {job:#?}"
                );
                if job.state == JobState::Running
                    && job
                        .logs
                        .iter()
                        .any(|line| line.contains("[consumer] Svt[info]"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        cancel_manager.cancel_job(cancel_job.id).await.unwrap();
        cancel_manager.shutdown().await;
        let canceled = cancel_manager.list_jobs().await.remove(0);
        assert_eq!(canceled.state, JobState::Canceled, "{canceled:#?}");
        assert!(!canceled_output.exists());
        assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".partial.")
        }));
        assert_eq!(std::fs::read(input).unwrap(), source_bytes);
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, and standalone SvtAv1EncApp"]
    async fn standalone_encode_preserves_frame_timing_copied_tracks_and_source() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(
            std::env::temp_dir().join(format!("jesses-encode-{}-{nonce}", std::process::id())),
        );
        std::fs::create_dir(&fixture.0).unwrap();
        let input = fixture.0.join("source's & 日本語.mkv");
        let output = fixture.0.join("output's & 日本語.mkv");
        let chapters = fixture.0.join("chapters.txt");
        let subtitles = fixture.0.join("subtitles.srt");
        let attachment = fixture.0.join("note.txt");
        std::fs::write(&chapters,";FFMETADATA1\ntitle=Encode fixture\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=500\ntitle=Opening\n").unwrap();
        std::fs::write(&subtitles, "1\n00:00:00,000 --> 00:00:00,450\nA subtitle\n").unwrap();
        std::fs::write(&attachment, "Copied attachment bytes").unwrap();
        let ffmpeg = find_executable(&["ffmpeg"]).await.unwrap().unwrap();
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=128x96:r=24000/1001",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-i",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        args.push(subtitles.into_os_string());
        args.extend(["-f", "ffmetadata", "-i"].into_iter().map(OsString::from));
        args.push(chapters.into_os_string());
        args.extend(
            [
                "-t",
                "0.5",
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-map",
                "2:s",
                "-map_metadata",
                "3",
                "-map_chapters",
                "3",
                "-c:v",
                "ffv1",
                "-vf",
                "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
                "-pix_fmt",
                "yuv420p",
                "-color_range",
                "tv",
                "-colorspace",
                "bt709",
                "-color_trc",
                "bt709",
                "-color_primaries",
                "bt709",
                "-chroma_sample_location",
                "left",
                "-c:a",
                "pcm_s16le",
                "-c:s",
                "srt",
                "-metadata:s:v:0",
                "title=Picture",
                "-metadata:s:v:0",
                "language=jpn",
                "-metadata:s:v:0",
                "BPS=90000000",
                "-metadata:s:v:0",
                "BPS-eng=90000001",
                "-metadata:s:v:0",
                "NUMBER_OF_FRAMES=999999",
                "-metadata:s:v:0",
                "NUMBER_OF_FRAMES-eng=888888",
                "-metadata:s:v:0",
                "NUMBER_OF_BYTES=777777",
                "-metadata:s:v:0",
                "NUMBER_OF_BYTES-eng=666666",
                "-metadata:s:v:0",
                "_STATISTICS_WRITING_APP=stale source statistics",
                "-metadata:s:v:0",
                "_STATISTICS_TAGS=BPS NUMBER_OF_FRAMES NUMBER_OF_BYTES",
                "-metadata:s:a:0",
                "language=jpn",
                "-metadata:s:a:0",
                "title=Original audio",
                "-metadata:s:a:0",
                "BPS=768000",
                "-metadata:s:s:0",
                "language=eng",
                "-disposition:s:0",
                "forced",
                "-attach",
            ]
            .into_iter()
            .map(OsString::from),
        );
        args.push(attachment.into_os_string());
        args.extend(
            ["-metadata:s:t:0", "mimetype=text/plain"]
                .into_iter()
                .map(OsString::from),
        );
        args.push(input.as_os_str().to_owned());
        let (_sender, cancel) = watch::channel(false);
        let generated = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg,
                args,
                cwd: None,
            },
            cancel,
            64 * 1024,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let source_bytes = std::fs::read(&input).unwrap();
        let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
        let (_probe_sender, probe_cancel) = watch::channel(false);
        let source_document = probe(&ffprobe, &input, &probe_cancel, None).await.unwrap();
        assert_eq!(
            tag_value(&source_document.streams[0], "BPS"),
            Some("90000000")
        );
        assert_eq!(
            tag_value(&source_document.streams[0], "NUMBER_OF_FRAMES-eng"),
            Some("888888")
        );
        let manager = JobManager::new(fixture.0.join("logs"));
        manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![1, 0, 2, 3],
                },
                settings: EncodeSettings {
                    preset: 12,
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        let job = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let job = manager.list_jobs().await.remove(0);
                if job.state.is_terminal() {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        manager.shutdown().await;
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        assert_eq!(std::fs::read(input).unwrap(), source_bytes);
        let media = crate::probe_media(output.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(
            media
                .streams
                .iter()
                .map(|s| s.kind.as_str())
                .collect::<Vec<_>>(),
            ["audio", "video", "subtitle", "attachment"]
        );
        assert_eq!(media.streams[1].codec.as_deref(), Some("av1"));
        let output_document = probe(&ffprobe, &output, &probe_cancel, None).await.unwrap();
        let encoded = &output_document.streams[1];
        for key in source_document.streams[0]
            .tags
            .keys()
            .filter(|key| metadata::is_derived_stream_tag(key))
        {
            if !key.eq_ignore_ascii_case("duration") && !key.eq_ignore_ascii_case("encoder") {
                assert!(
                    tag_value(encoded, key).is_none(),
                    "Stale video statistic survived: {key}"
                );
            }
        }
        // The muxer can write current duration/provenance, but the source FFV1
        // encoder must never be presented as the encoder of the AV1 bitstream.
        assert_ne!(
            tag_value(encoded, "encoder"),
            tag_value(&source_document.streams[0], "encoder")
        );
        assert_eq!(tag_value(encoded, "title"), Some("Picture"));
        assert_eq!(tag_value(encoded, "language"), Some("jpn"));
        assert_eq!(
            tag_value(&output_document.streams[0], "BPS"),
            Some("768000")
        );
        assert_eq!(
            tag_value(&output_document.streams[0], "title"),
            Some("Original audio")
        );
        assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".partial.")
        }));
    }

    async fn decoded_frames(ffprobe: &Path, path: &Path) -> usize {
        let (_sender, cancel) = watch::channel(false);
        let counted = supervisor::run_capture(
            &CommandSpec {
                executable: ffprobe.to_owned(),
                args: [
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-count_frames",
                    "-show_entries",
                    "stream=nb_read_frames",
                    "-of",
                    "default=nokey=1:noprint_wrappers=1",
                ]
                .into_iter()
                .map(OsString::from)
                .chain([path.as_os_str().to_owned()])
                .collect(),
                cwd: None,
            },
            cancel,
            64 * 1024,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert!(counted.status.success() && counted.stderr.is_empty());
        String::from_utf8_lossy(&counted.stdout)
            .trim()
            .parse()
            .unwrap()
    }

    /// Encodes one second of 24000/1001 video with stereo audio, copied into
    /// `extension`, and returns the finished job, the output, and the source
    /// frame count.
    async fn encode_with_audio(
        encoder: VideoEncoder,
        preset: u8,
        extension: &str,
        audio_codec: &str,
        full_output_verification: bool,
    ) -> (Fixture, PathBuf, JobSnapshot, usize) {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(
            std::env::temp_dir().join(format!("jesses-copy-proof-{}-{nonce}", std::process::id())),
        );
        std::fs::create_dir(&fixture.0).unwrap();
        let input = fixture.0.join("source.mkv");
        let output = fixture.0.join(format!("output.{extension}"));
        let ffmpeg = find_executable(&["ffmpeg"]).await.unwrap().unwrap();
        let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
        let (_sender, cancel) = watch::channel(false);
        let generated = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg,
                args: [
                    "-v",
                    "error",
                    "-nostdin",
                    "-n",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=s=128x96:r=24000/1001",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=48000",
                    "-t",
                    "1",
                    "-vf",
                    "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
                    "-c:v",
                    "ffv1",
                    "-pix_fmt",
                    "yuv420p",
                    "-color_range",
                    "tv",
                    "-colorspace",
                    "bt709",
                    "-color_trc",
                    "bt709",
                    "-color_primaries",
                    "bt709",
                    "-chroma_sample_location",
                    "left",
                    "-ac",
                    "2",
                    "-c:a",
                ]
                .into_iter()
                .map(OsString::from)
                .chain([audio_codec.into(), input.as_os_str().to_owned()])
                .collect(),
                cwd: None,
            },
            cancel,
            64 * 1024,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let source_frames = decoded_frames(&ffprobe, &input).await;
        let manager = JobManager::new(fixture.0.join("logs"));
        manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0, 1],
                },
                settings: EncodeSettings {
                    encoder,
                    preset,
                    encoding_preferences: Some(media_core::EncodingPreferences {
                        keyframe_interval_seconds: 10,
                        mp4_faststart: true,
                        full_output_verification,
                    }),
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        let job = tokio::time::timeout(Duration::from_secs(300), async {
            loop {
                let job = manager.list_jobs().await.remove(0);
                if job.state.is_terminal() {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        manager.shutdown().await;
        (fixture, output, job, source_frames)
    }

    const COPY_PROVEN: &str = "byte-identical copy of the decoded and verified timing wrapper";
    const FAST_VERIFIED: &str = "Fast output verification: all";

    async fn assert_encoded(output: &Path, codec: &str, frames: usize) {
        let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
        let (_sender, cancel) = watch::channel(false);
        let document = probe(&ffprobe, output, &cancel, None).await.unwrap();
        assert_eq!(document.streams[0].codec_name.as_deref(), Some(codec));
        assert_eq!(document.streams[1].codec_type.as_deref(), Some("audio"));
        assert_eq!(decoded_frames(&ffprobe, output).await, frames);
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, mkvmerge, and standalone x265"]
    async fn x265_mp4_proves_each_copy_instead_of_decoding_it_again() {
        let (_fixture, output, job, frames) =
            encode_with_audio(VideoEncoder::X265Standalone, 0, "mp4", "aac", true).await;
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        assert!(
            job.logs.iter().any(|line| line.contains(COPY_PROVEN)),
            "{:#?}",
            job.logs
        );
        assert_encoded(&output, "hevc", frames).await;
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, mkvmerge, and standalone vpxenc"]
    async fn vpxenc_webm_proves_each_copy_instead_of_decoding_it_again() {
        let (_fixture, output, job, frames) =
            encode_with_audio(VideoEncoder::VpxStandalone, 8, "webm", "libopus", true).await;
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        assert!(
            job.logs.iter().any(|line| line.contains(COPY_PROVEN)),
            "{:#?}",
            job.logs
        );
        assert_encoded(&output, "vp9", frames).await;
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, and standalone SvtAv1EncApp"]
    async fn svt_av1_mp4_reads_the_copied_clock_from_packets() {
        let (_fixture, output, job, frames) =
            encode_with_audio(VideoEncoder::SvtAv1, 12, "mp4", "aac", true).await;
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        // No timing wrapper: the staging file is decoded and validated once.
        assert!(!job.logs.iter().any(|line| line.contains(COPY_PROVEN)));
        assert!(!job.logs.iter().any(|line| line.contains(FAST_VERIFIED)));
        assert_encoded(&output, "av1", frames).await;
    }

    #[tokio::test]
    #[ignore = "manual real-media qualification; set JESSES_SAMPLE_INPUT to a short clip"]
    async fn encodes_external_sample_with_fast_and_full_output_verification() {
        let Some(input) = std::env::var_os("JESSES_SAMPLE_INPUT").map(PathBuf::from) else {
            eprintln!(
                "External sample encode was NOT run: set JESSES_SAMPLE_INPUT to a short clip."
            );
            return;
        };
        let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
        let (_sender, cancel) = watch::channel(false);
        let document = probe(&ffprobe, &input, &cancel, None).await.unwrap();
        let first = |kind: &str| {
            document
                .streams
                .iter()
                .find(|stream| stream.codec_type.as_deref() == Some(kind))
                .map(|stream| stream.index)
        };
        let all = document
            .streams
            .iter()
            .map(|stream| stream.index)
            .collect::<Vec<_>>();
        let picture_and_sound = [first("video"), first("audio")]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        // A Dolby Vision source needs its RPU imported from a file or the
        // HDR10 fallback, which only SVT-AV1 offers.
        let dolby_vision = document.streams.iter().any(|stream| {
            stream.side_data_list.iter().any(|value| {
                value["side_data_type"]
                    .as_str()
                    .is_some_and(|kind| kind.contains("DOVI"))
            })
        });
        if dolby_vision {
            eprintln!("x265 skipped: the source carries Dolby Vision.");
        }
        // MP4 takes picture and sound; deselected tracks may end later.
        let cases = [
            (VideoEncoder::SvtAv1, 12, "mkv", &all),
            (VideoEncoder::X265Standalone, 0, "mkv", &all),
            (VideoEncoder::SvtAv1, 12, "mp4", &picture_and_sound),
            (VideoEncoder::X265Standalone, 0, "mp4", &picture_and_sound),
        ]
        .into_iter()
        .filter(|(encoder, ..)| encoder.is_svt() || !dolby_vision);
        for (encoder, preset, extension, stream_indices) in cases {
            for full_output_verification in [false, true] {
                let nonce = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let fixture = Fixture(
                    std::env::temp_dir()
                        .join(format!("jesses-sample-{}-{nonce}", std::process::id())),
                );
                std::fs::create_dir(&fixture.0).unwrap();
                let output = fixture.0.join(format!("output.{extension}"));
                let manager = JobManager::new(fixture.0.join("logs"));
                let started = std::time::Instant::now();
                manager
                    .start_encode(EncodeRequest {
                        source: RemuxRequest {
                            input_path: input.to_string_lossy().into_owned(),
                            output_path: output.to_string_lossy().into_owned(),
                            stream_indices: stream_indices.clone(),
                        },
                        settings: EncodeSettings {
                            encoder,
                            preset,
                            // SVT-AV1 carries HDR10+ sources as static HDR10.
                            hdr10_fallback: encoder.is_svt(),
                            encoding_preferences: Some(media_core::EncodingPreferences {
                                keyframe_interval_seconds: 10,
                                mp4_faststart: true,
                                full_output_verification,
                            }),
                            // MP4 cannot hold arbitrary Matroska stream tags.
                            copy_metadata: (extension == "mp4").then_some(false),
                            ..EncodeSettings::default()
                        },
                    })
                    .await
                    .unwrap();
                let job = tokio::time::timeout(Duration::from_secs(1800), async {
                    loop {
                        let job = manager.list_jobs().await.remove(0);
                        if job.state.is_terminal() {
                            return job;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                })
                .await
                .unwrap();
                manager.shutdown().await;
                let mode = if full_output_verification {
                    "full"
                } else {
                    "fast"
                };
                eprintln!(
                    "{encoder:?} to {extension}, {mode} verification: {:?} in {:.1}s",
                    job.state,
                    started.elapsed().as_secs_f64()
                );
                for line in job.logs.iter().filter(|line| {
                    line.contains(FAST_VERIFIED)
                        || line.contains(COPY_PROVEN)
                        || line.starts_with("Verified all")
                }) {
                    eprintln!("  {line}");
                }
                assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
                assert_eq!(
                    job.logs.iter().any(|line| line.contains(FAST_VERIFIED)),
                    !full_output_verification
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, mkvmerge, SvtAv1EncApp, x265, and vpxenc"]
    async fn fast_verification_checks_every_frame_clock_without_decoding_the_output() {
        for (encoder, preset, extension, audio, codec) in [
            (VideoEncoder::SvtAv1, 12, "mkv", "aac", "av1"),
            (VideoEncoder::X265Standalone, 0, "mp4", "aac", "hevc"),
            (VideoEncoder::VpxStandalone, 8, "webm", "libopus", "vp9"),
        ] {
            let (_fixture, output, job, frames) =
                encode_with_audio(encoder, preset, extension, audio, false).await;
            assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
            assert!(
                job.logs.iter().any(|line| line.contains(FAST_VERIFIED)),
                "{:#?}",
                job.logs
            );
            assert!(!job.logs.iter().any(|line| line.contains(COPY_PROVEN)));
            assert_encoded(&output, codec, frames).await;
        }
    }
}
