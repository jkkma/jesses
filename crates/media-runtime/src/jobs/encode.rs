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
#[path = "svt_ivf.rs"]
mod svt_ivf;
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
use media_core::{EncoderProgress, VideoEncoder};

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
        let hdr_checks = super::source_timing::HdrChecks::new(cancel);
        let job_cancel = hdr_checks.cancel();
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
        hdr_checks
            .run(cancel, async {
                if can_observe_standalone_producer(settings)
                    && (settings.source_filters
                        || settings.tone_map.is_some_and(|tone| tone.source_workflow)
                        || header_planning_allowed(settings))
                {
                    super::source_filters::with_observer(
                        super::source_filters::Observation::new(Path::new(&request.output_path)),
                        execution,
                    )
                    .await
                } else {
                    execution.await
                }
            })
            .await
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
            let use_canonical_vfr = if super::source_normalization::exists(id, request)? {
                true
            } else if let Ok((_, document)) =
                super::source_normalization::inspect(request, settings, cancel).await
            {
                super::source_normalization::has_vfr_rate_mismatch(&document, settings)
                    && super::source_normalization::needs(&document, settings, true)
                        .unwrap_or(false)
            } else {
                false
            };
            if use_canonical_vfr {
                self.change(id, |snapshot| {
                    append_log(
                        snapshot,
                        "Source video has a variable frame rate (r_frame_rate differs from avg_frame_rate); using CFR canonical normalization instead of a stream-copied AV1AN section so AV1AN preserves A/V sync and frame-rate consistency.".into(),
                    )
                })
                .await;
                boxed(|| {
                    self.encode_source_mode(
                        id, request, settings, cancel, log_path, temporary, scratch, preview, true,
                    )
                })
                .await
            } else {
                boxed(|| {
                    self.encode_source_av1an_mode(
                        id, request, settings, cancel, log_path, temporary, scratch, preview,
                    )
                })
                .await
            }
        } else {
            boxed(|| {
                self.encode_source_mode(
                    id, request, settings, cancel, log_path, temporary, scratch, preview, false,
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
        force_normalize: bool,
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
        let mut required =
            super::source_normalization::needs(&document, settings, force_normalize)?;
        if !existing && required {
            // A canonical source decodes the complete video. Check the selection
            // and destination against stream headers before that pass.
            let ffprobe = discover("ffprobe", cancel).await?;
            let output = files::output_path(request, &original)?;
            let headers = probe_encode_source(
                &ffprobe,
                &original.path,
                &output,
                &request.stream_indices,
                settings,
                cancel,
            )
            .await?;
            preflight_streams(
                &original, request, settings, &headers, &output, &ffprobe, cancel,
            )
            .await?;
        }
        if !existing && !required {
            // A rotated or later-starting source that Quick Convert encodes
            // directly: crop values are given in coded orientation, so they turn
            // with the picture, as they do for a canonical copy.
            let direct = document
                .streams
                .iter()
                .find(|stream| stream.index == settings.video_stream_index)
                .and_then(|video| {
                    let rotation = super::source_normalization::direct_rotation(video, settings)?;
                    let origin = video
                        .start_time
                        .as_deref()
                        .and_then(|value| value.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    (rotation != 0 || origin.abs() > 0.001).then_some(rotation)
                });
            let oriented = direct
                .filter(|rotation| *rotation != 0)
                .map(|rotation| super::source_normalization::oriented_settings(settings, rotation));
            let result = boxed(|| {
                self.encode_video_mode(
                    id,
                    request,
                    oriented.as_ref().unwrap_or(settings),
                    cancel,
                    log_path,
                    temporary,
                    scratch,
                    preview.as_deref_mut(),
                    None,
                    None,
                    oriented.is_some().then_some(settings),
                    None,
                    None,
                )
            })
            .await;
            match &result {
                Err(error)
                    if super::encode_plan::repairable_source_clock(error)
                        || (direct.is_some() && error.code == "ENCODE_INPUT_UNSUPPORTED") =>
                {
                    if direct.is_some() {
                        let message = format!(
                            "Encoding the rotated or later-starting source directly was refused ({}). Preparing a lossless canonical source instead.",
                            error.message
                        );
                        self.change(id, |snapshot| append_log(snapshot, message))
                            .await;
                    }
                    required = true;
                    self.phase(
                        id,
                        JobState::Preparing,
                        "Preparing a compatible source clock.",
                    )
                    .await;
                }
                _ => return result,
            }
            // Remove the rejected attempt's owned assets before the normalized retry.
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
            &document,
            required,
            cancel,
            &log_path.with_extension("source-normalization.log"),
        ))
        .await?;
        let mut effective = prepared.effective_settings(settings);
        if force_normalize {
            effective.source_trim = false;
        }
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
        let encoder = discover_video_encoder(settings.encoder, cancel).await?;
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
        let path = &encoder;
        let version_arg = match settings.encoder {
            VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone => "--help",
            encoder if encoder.is_ffmpeg() => "-version",
            _ => "--version",
        };
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
        let identity = format!(
            "{}\n{}",
            String::from_utf8_lossy(&version.stdout),
            String::from_utf8_lossy(&version.stderr)
        );
        crate::discovery::validate_video_encoder_version(settings.encoder, &identity)
            .map_err(|message| files::error("ENCODER_BUILD_MISMATCH", message, path))?;
        let verify_svt_ivf = settings.backend == media_core::EncodeBackend::Standalone
            && settings.encoder.is_svt()
            && svt_ivf::rewrites_count(&identity);
        let allow_parallel_svt = super::parameters::allows_parallel_svt(settings, &identity);
        super::rate_control::validate_encoder_version(settings, &identity)
            .map_err(|message| files::error("ENCODER_CAPABILITY_UNSUPPORTED", message, path))?;
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
        if settings.backend == media_core::EncodeBackend::Standalone
            && settings.encoder.is_svt()
            && !verify_svt_ivf
        {
            // The check turns itself off for a build outside the qualified
            // list, for example after an encoder update. Say so.
            self.change(id, |snapshot| {
                append_log(
                    snapshot,
                    "The encoded frame count is not compared for this SVT build: its IVF header count has not been qualified."
                        .into(),
                )
            })
            .await;
        }
        let mut document = probe_encode_source(
            &ffprobe,
            &source.path,
            &output,
            &request.stream_indices,
            settings,
            cancel,
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
        // The cover's one packet is copied out before any encoding, so a cover
        // that cannot be kept is reported now instead of failing a finished job.
        let cover_art = match super::cover_art::CoverArt::prepare(
            &ffmpeg,
            original_guard.map_or(source.path.as_path(), |original| original.path.as_path()),
            &output,
            container::source_iso_format(&output, &preflight_selected, &mux_settings).is_none()
                && container::staging_format(&preflight_selected, Some(&mux_settings))
                    == media_core::ContainerFormat::Matroska,
            &attempt_id,
            settings,
            cancel,
        )
        .await
        {
            Ok(cover) => cover,
            Err(error) if error.code == "COVER_ART_LEFT_OUT" => {
                self.change(id, |snapshot| append_log(snapshot, error.message.clone()))
                    .await;
                None
            }
            Err(error) => return Err(error),
        };
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
        // A rotated source Quick Convert encodes directly is planned in display
        // orientation; the decoder applies its transform to the coded frames.
        let direct_rotation = normalized_source.is_none()
            && planning_document
                .streams
                .iter()
                .find(|stream| stream.index == settings.video_stream_index)
                .and_then(|video| super::source_normalization::direct_rotation(video, settings))
                .is_some_and(|rotation| rotation != 0);
        let mut plan = if source_origin.is_some() || direct_rotation {
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
        audio::check_encoders(&ffmpeg, &mux_settings, cancel).await?;
        plan.check_temporal_tools(&ffmpeg, cancel).await?;
        let mut audio_timelines = Vec::new();
        let aac_priming_container = matches!(
            container::format(&output)?,
            media_core::ContainerFormat::Mp4 | media_core::ContainerFormat::Mov
        );
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
            // External tracks keep their complete scan: their bounds set the
            // expected output duration below.
            let priming_only = trim_audio_until.is_none()
                && external.audio_source(track.stream_index).is_none()
                && track.codec == media_core::AudioCodec::Aac
                && aac_priming_container;
            let until = trim_audio_until
                .filter(|_| external.audio_source(track.stream_index).is_none())
                .or_else(|| priming_only.then_some(1.0));
            // Without a trim, a source track's complete decode only checks its
            // timeline, unless it is AAC bound for MP4/MOV, whose priming gate
            // below reads the decoded start.
            if until.is_none()
                && external.audio_source(track.stream_index).is_none()
                && !(track.codec == media_core::AudioCodec::Aac && aac_priming_container)
            {
                continue;
            }
            self.phase(
                id,
                JobState::Preparing,
                if priming_only {
                    "Checking the first second of audio for MP4/MOV AAC priming."
                } else if until.is_some() {
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
        let video = selected
            .iter()
            .find(|stream| stream.index == plan.video_index)
            .expect("validated video selection");
        let declared_rate = (plan.fps_num, plan.fps_den);
        let check_discard = settings.backend == media_core::EncodeBackend::Av1an
            && matches!(
                settings.av1an_options.unwrap_or_default().chunk_method,
                media_core::Av1anChunkMethod::Lsmash | media_core::Av1anChunkMethod::Select
            )
            && document
                .format
                .as_ref()
                .and_then(|format| format.format_name.as_deref())
                .is_some_and(|names| names.split(',').any(|name| name == "mov"));
        let fast_av1an = settings.backend == media_core::EncodeBackend::Av1an
            && header_planning_allowed(settings)
            && !plan.requires_av1an_preprocess()
            && !plan.requires_qtgmc()
            && !plan.requires_source_queue_clock_proof();
        // Resume with the exact source plan bound to the saved workspace.
        let fast_standalone = quick_header_planning(settings)
            && !self.state.lock().await.entries.iter().any(|entry| {
                entry.snapshot.id == id && entry.snapshot.standalone_recovery.is_some()
            });
        let (frame_count, scanned_source_frames) = self
            .source_frames(
                id,
                &ffprobe,
                &source.path,
                &mut plan,
                video,
                document.duration(),
                preview.is_some(),
                check_discard,
                fast_av1an || fast_standalone,
                cancel,
            )
            .await?;
        if plan.av1an_presented_rate.is_some() {
            for stream in &selected {
                audio::validate_edited_eac3_copy(&ffprobe, &source.path, stream, settings, cancel)
                    .await?;
            }
        }
        let dynamic_hdr =
            super::dynamic_hdr::Resources::prepare(settings, frame_count, cancel).await?;
        external.validate_timecode_clock(plan.fps_num, plan.fps_den, frame_count)?;
        let source_video_duration = frame_count as f64 * plan.frame_seconds();
        let peak_sample_duration = tone_map_sample_duration(
            plan.source_frames_estimated,
            source_video_duration,
            video,
            document.duration(),
        );
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
            plan.resolve_tone_map(&ffmpeg, &source.path, peak_sample_duration, cancel)
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
        let standalone_locator =
            if settings.backend == media_core::EncodeBackend::Standalone && preview.is_none() {
                self.state
                    .lock()
                    .await
                    .entries
                    .iter()
                    .find(|entry| entry.snapshot.id == id)
                    .and_then(|entry| entry.snapshot.standalone_recovery.clone())
            } else {
                None
            };
        // Recoverable jobs keep the lane count recorded by their command
        // signature and use the same lane count across both bitrate passes.
        plan.allow_parallel_svt = allow_parallel_svt && standalone_locator.is_none();
        // The same holds for the other encoders' thread counts: only a job
        // without a recovery workspace sizes them to this machine.
        plan.machine_threads = settings.backend == media_core::EncodeBackend::Standalone
            && standalone_locator.is_none();
        let interval = settings
            .trim
            .map(|trim| super::trim::Interval::build(trim, frame_count, plan.fps_num, plan.fps_den))
            .transpose()?;
        let trim_picture_clock = super::trim::PictureClock {
            origin: plan.source_origin,
            rate: (plan.fps_num, plan.fps_den),
        };
        let compare_hdr_scan_to_producer = settings.backend
            == media_core::EncodeBackend::Standalone
            && plan.source_frames_estimated
            && !settings.source_filters;
        let source_hdr_check =
            if !scanned_source_frames && preview.is_none() && plan.has_source_hdr() {
                let exact_frames = (!plan.source_frames_estimated).then_some(frame_count);
                let selection =
                    interval.map(|interval| interval.source_start_frame..interval.source_end_frame);
                if settings.backend == media_core::EncodeBackend::Standalone {
                    Some(super::source_timing::HdrCheck::start(
                        &ffprobe,
                        &source.path,
                        &plan,
                        selection,
                        exact_frames,
                    ))
                } else if plan.is_hdr10() && !settings.hdr10_fallback {
                    let packet_source = super::source_timing::PacketHdrSource::new(
                        video.codec_name.as_deref(),
                        document
                            .format
                            .as_ref()
                            .and_then(|format| format.format_name.as_deref()),
                    );
                    match packet_source {
                        Some(packets) if !packets.needs_inspection() => None,
                        Some(packets) if selection.is_none() => {
                            Some(super::source_timing::HdrCheck::start_packets(
                                &ffmpeg,
                                &ffprobe,
                                &source.path,
                                &plan,
                                packets,
                                exact_frames,
                            ))
                        }
                        _ => {
                            // Filtered packets omit PTS and can be in decode order.
                            // An ordinal trim needs the decoded selection.
                            Some(super::source_timing::HdrCheck::start(
                                &ffprobe,
                                &source.path,
                                &plan,
                                selection,
                                exact_frames,
                            ))
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };
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
        let grain_coverage = av1an::GrainTableCoverage::from_settings(settings)?;
        // Lean jobs and free-form filters obtain the exact output clock from
        // the normal producer/AV1AN reconciliation, without an extra decode.
        // The header count is still enough to stop a table that is clearly
        // too short before the encode; the exact check runs once it is known.
        if !settings.source_filters {
            let rate = (plan.fps_num, plan.fps_den);
            if plan.source_frames_estimated {
                grain_coverage.verify_estimate(transformed_frame_count, rate)?;
            } else {
                grain_coverage.verify(transformed_frame_count, rate)?;
            }
        }
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
                    Some(trim_picture_clock),
                    &output_document,
                    &combined_selected,
                    &external,
                    &ffmpeg,
                    &ffprobe,
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
                    .track_metadata(stream, settings)
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
            append_attachments(&mut args, &budget_selected, cover_art.as_ref(), None);
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
        let av1an_locator = if settings.backend == media_core::EncodeBackend::Av1an {
            self.state
                .lock()
                .await
                .entries
                .iter()
                .find(|entry| entry.snapshot.id == id)
                .and_then(|entry| entry.snapshot.recovery.clone())
        } else {
            None
        };
        let reusable_av1an_prepared = if preview.is_none()
            && settings.backend == media_core::EncodeBackend::Av1an
            && (plan.requires_qtgmc()
                || plan.requires_av1an_preprocess()
                || plan.av1an_presented_rate.is_some())
            && let Some(locator) = av1an_locator.as_ref()
        {
            super::av1an::Recovery::reusable_prepared(
                id,
                authority_request.unwrap_or(request),
                authority_settings.unwrap_or(settings),
                &source.path,
                locator,
                cancel,
            )
            .await
        } else {
            None
        };
        let source_qtgmc = settings.source_filters
            && settings.backend == media_core::EncodeBackend::Standalone
            && plan.requires_qtgmc();
        let source_qtgmc_checkpoint_ready = source_qtgmc
            && standalone_locator.as_ref().is_some_and(|recovery| {
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
            if reusable_av1an_prepared.is_none() {
                self.phase(
                    id,
                    JobState::Preparing,
                    "Preparing a verified lossless QTGMC intermediate with the configured VapourSynth runtime.",
                )
                .await;
            }
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
            if let Some((_, saved_frames)) = &reusable_av1an_prepared {
                if settings.source_filters {
                    frame_count = *saved_frames;
                }
                self.change(id, |snapshot| {
                    append_log(
                        snapshot,
                        "Reusing the verified prepared QTGMC intermediate from the saved AV1AN workspace.".into(),
                    );
                })
                .await;
            } else if preview.is_none() && !defer_source_qtgmc && !source_qtgmc_checkpoint_ready {
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
            let (crf, preset) = if settings.encoder.is_svt() {
                svt_quality(settings)
            } else {
                (settings.crf.to_string(), settings.preset.to_string())
            };
            let quality = match &rate {
                Some(rate) => format!("target {} kb/s", rate.kbps),
                None if settings.encoder.is_svt() && settings.lossless => String::from("lossless"),
                None => format!("CRF {crf}"),
            };
            let count_status = if plan.source_frames_estimated {
                "Estimated"
            } else {
                "Will encode"
            };
            append_log(
                snapshot,
                format!(
                    "{count_status} {frame_count} progressive CFR frames at {}/{} fps; {}-bit {}, {quality}, preset {preset}.",
                    plan.fps_num, plan.fps_den, plan.output_bit_depth(), plan.output_codec(),
                ),
            );
            if settings.encoder.is_svt() {
                let grain_note = match settings.av1an_grain.as_ref() {
                    Some(grain) if grain.table.is_some() => format!(
                        "Film grain table attached (denoising {}).",
                        if grain.denoise { "on" } else { "off" }
                    ),
                    Some(grain) if grain.denoise => format!(
                        "Film grain synthesis {} (denoising on).",
                        settings.film_grain
                    ),
                    _ => format!(
                        "Film grain synthesis {} (denoising off).",
                        settings.film_grain
                    ),
                };
                append_log(snapshot, grain_note);
            }
            match settings.encoder {
                VideoEncoder::SvtAv1FiveFish => append_log(snapshot, format!("SVT-AV1 5fish: line-art bias {}, texture bias {}.", settings.lineart_psy_bias, settings.texture_psy_bias)),
                VideoEncoder::SvtAv1Hdr => append_log(snapshot, format!("{}: {} tune.", settings.encoder.name(), settings.parameters.iter().find(|parameter| parameter.name == "tune").map_or_else(|| match settings.hdr_tune { media_core::HdrTune::VisualQuality => "visual quality (0)".to_string(), media_core::HdrTune::FilmGrain => "film grain retention (6)".to_string() }, |parameter| format!("explicit override ({})", parameter.value)))),
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
        let qtgmc_identity = if !source_qtgmc
            && reusable_av1an_prepared.is_none()
            && let Some(prepared) = &qtgmc
        {
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
        let presented = plan
            .av1an_presented_rate
            .filter(|_| qtgmc.is_none() && !plan.requires_av1an_preprocess());
        let av1an_prepared = if settings.backend == media_core::EncodeBackend::Av1an
            && qtgmc.is_none()
            && (plan.requires_av1an_preprocess() || plan.av1an_presented_rate.is_some())
            && reusable_av1an_prepared.is_none()
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
                if plan.av1an_presented_rate.is_some() {
                    super::av1an_preprocess::PRESENTED_FRAMES_NOTE
                } else {
                    "Preparing one verified lossless processed source for av1an scene detection, chunk encoding, and quality references."
                },
            )
            .await;
            let prepared = if let Some(rate) = presented {
                super::av1an_preprocess::Prepared::build_presented(
                    &source.path,
                    &output,
                    &attempt_id,
                    &plan,
                    &ffmpeg,
                    rate,
                    cancel,
                )?
            } else {
                super::av1an_preprocess::Prepared::build(
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
                .await?
            };
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
            let identity = if let Some(rate) = presented {
                boxed(|| {
                    super::av1an_preprocess::verify_presented(
                        &source.path,
                        &plan,
                        &ffmpeg,
                        rate,
                        frame_count,
                        &prepared,
                        cancel,
                    )
                })
                .await?
            } else {
                prepared
                    .decoded_identity(&ffmpeg, frame_count, cancel)
                    .await?
            };
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
            let locator = av1an_locator;
            let prepared_source = if let Some((reusable, _)) = reusable_av1an_prepared {
                if qtgmc.is_none() {
                    self.change(id, |snapshot| {
                        append_log(
                            snapshot,
                            "Reusing the verified processed source from the saved AV1AN workspace."
                                .into(),
                        );
                    })
                    .await;
                }
                Some(reusable)
            } else if let (Some(prepared), Some(identity)) =
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
                &mut plan,
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
        } else if let Some(locator) = standalone_locator {
            let two_pass = rate.as_ref().is_some_and(|value| value.two_pass);
            let mut arguments = standalone_recovery_arguments(&plan, settings);
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
            if let Some(trim) = plan.source_ordinal_trim_filter() {
                signature.push(format!("source-ordinal-trim={trim}"));
            }
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
            // Finish a previously admitted durable workspace using its existing
            // source, tool and processing-plan fingerprints.
            let source_bytes = tokio::fs::metadata(&source.path)
                .await
                .map_or(0, |metadata| metadata.len());
            let gigabytes = source_bytes as f64 / 1_000_000_000.0;
            self.phase(
                id,
                JobState::Preparing,
                &format!("Fingerprinting the {gigabytes:.1} GB source and the tools to check the saved standalone workspace."),
            )
            .await;
            let prepared = super::standalone_recovery::Recovery::resume(
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
        } else {
            None
        };
        if requires_serial_svt(&plan, settings) {
            self.change(id, |snapshot| append_log(snapshot, "SVT high-bit-depth mode decision plus overlays uses one processing lane (lp=1), within the selected worker limit, to avoid the observed SVT 4.2 parallel short-clip crash. Both requested controls remain enabled.".into())).await;
        }
        if plan.widens_eight_bit_source() {
            let reason = if super::parameters::high_bit_depth_decision(settings) {
                " so high-bit-depth mode decision runs on a 10-bit picture"
            } else {
                ""
            };
            self.change(id, |snapshot| {
                append_log(
                    snapshot,
                    format!("The 8-bit source is converted to 10-bit before encoding{reason}."),
                )
            })
            .await;
        }
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
            #[rustfmt::skip]
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
                // The held script may replace the producer or change its cadence.
                // If accepted unchanged, the typed pipeline below adds its exact limit.
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
                } else if cfg!(windows) {
                    // CMD cannot redirect into a file this job still holds open,
                    // so the program writes a review-owned file released while it runs.
                    let extension = pass_output.path.extension().and_then(|extension| extension.to_str()).unwrap_or("mkv");
                    let target = Temporary::create_extension(&output, &format!("{attempt_id}-review-video-{pass}"), extension)?;
                    let redirect = format!(" > {}", super::command_edit::quote(&target.path.to_string_lossy()));
                    if !two_pass || pass == 2 { encoded_path = target.path.clone(); }
                    path_outputs.push(target);
                    redirect
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
            append_attachments(&mut mux, &output_selected, cover_art.as_ref(), attachment.as_ref());
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
                source.verify()?; external.verify()?; if let Some(original) = original_guard { original.verify()?; }
                return Ok::<bool,AppError>(true);
            }
                Ok::<bool,AppError>(false)
            }).await?;
            if edited {
                if let Some(check) = source_hdr_check {
                    check.finish(None).await?;
                }
                self.finalize_edited_encode(id, cancel, &source, &external, temp, &output, scratch)
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
            plan.resolve_tone_map(&ffmpeg, &source.path, peak_sample_duration, cancel)
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
        // The decoded identity only records the QTGMC render for resume.
        if source_qtgmc
            && !source_qtgmc_checkpoint_ready
            && let Some(prepared) = &qtgmc
            && let Some(recovery) = standalone_recovery.as_mut()
        {
            let identity =
                boxed(|| prepared.decoded_identity(&ffmpeg, frame_count, cancel)).await?;
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
        let mut mux_video_packets = None;
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
                    || plan.source_frames_estimated
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
                    let mut review_hdr_wrap = None;
                    let post_mux = if self.command_review(id).await.is_some()
                        || recovery.edited_command().await?.is_some()
                    {
                        let mut stages = Vec::new();
                        let mux_video_path = if settings.encoder == VideoEncoder::AomAv1
                            && plan.is_hdr10()
                            && let Some(tool) = mkvmerge.as_ref()
                        {
                            let wrapped = Temporary::create(
                                &output,
                                &format!("{attempt_id}-aomenc-review-hdr"),
                            )?;
                            let mut wrap_args =
                                x265::mkvmerge_arguments(&intermediate.path, &wrapped.path, &plan);
                            let metadata = plan.aom_hdr_mkvmerge_arguments();
                            wrap_args.splice(wrap_args.len() - 1..wrap_args.len() - 1, metadata);
                            stages.push(CommandSpec {
                                executable: tool.clone(),
                                args: wrap_args,
                                cwd: None,
                            });
                            let path = wrapped.path.clone();
                            review_hdr_wrap = Some(wrapped);
                            path
                        } else {
                            intermediate.path.clone()
                        };
                        let mut args = mux_args(
                            &source.path,
                            &mux_video_path,
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
                        append_attachments(
                            &mut args,
                            &output_selected,
                            cover_art.as_ref(),
                            review_attachment.as_ref(),
                        );
                        stages.push(CommandSpec {
                            executable: ffmpeg.clone(),
                            args,
                            cwd: None,
                        });
                        stages
                    } else {
                        Vec::new()
                    };
                    if let Some(wrapped) = review_hdr_wrap.as_mut() {
                        wrapped.close_for_path_writer()?;
                    }
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
                    if let Some(mut wrapped) = review_hdr_wrap {
                        let _ = wrapped.reopen_after_path_writer();
                    }
                    if let Some(actual) = plan.source_queue_frame_count() {
                        frame_count = actual;
                    } else if plan.source_frames_estimated {
                        frame_count = recovery.queue_frame_count().await?;
                    }
                    if plan.source_frames_estimated {
                        if !edited && !plan.has_source_filters() {
                            boxed(|| {
                                self.verify_av1an_source_duration(
                                    id,
                                    &ffprobe,
                                    &source.path,
                                    video,
                                    &plan,
                                    frame_count,
                                    cancel,
                                )
                            })
                            .await?;
                        }
                        self.change(id, |snapshot| {
                            snapshot.duration_seconds =
                                Some(frame_count as f64 * plan.frame_seconds());
                            append_log(
                                snapshot,
                                format!(
                                    "AV1AN established the exact queue count: {frame_count} frames."
                                ),
                            );
                        })
                        .await;
                    }
                    if edited {
                        if let Some(check) = source_hdr_check {
                            check.finish(None).await?;
                        }
                        source.verify()?;
                        external.verify()?;
                        if let Some(original) = original_guard {
                            original.verify()?;
                        }
                        self.finalize_edited_encode(
                            id, cancel, &source, &external, temp, &output, scratch,
                        )
                        .await?;
                        drop(durable);
                        self.cleanup_published_av1an(id, recovery, temporary).await;
                        return Ok(());
                    }
                    let summary = recovery.finalizing(cancel).await?;
                    self.change(id, |snapshot| snapshot.recovery = Some(summary))
                        .await;
                } else {
                    self.phase_with_fresh_progress(
                        id,
                        JobState::Finalizing,
                        "Checking the saved AV1AN intermediate before finalizing output.",
                    )
                    .await;
                    if plan.source_frames_estimated && !plan.has_source_filters() {
                        boxed(|| {
                            self.verify_av1an_source_duration(
                                id,
                                &ffprobe,
                                &source.path,
                                video,
                                &plan,
                                frame_count,
                                cancel,
                            )
                        })
                        .await?;
                    }
                    if settings
                        .av1an_options
                        .is_some_and(|options| options.source_workflow)
                    {
                        if let Some(actual) = super::av1an::verify_source_join(
                            self,
                            id,
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
                    } else if settings.source_filters {
                        let actual =
                            probe_headers(&ffprobe, &intermediate.path, cancel, Some(&[0])).await?;
                        let stream = actual.streams.first().ok_or_else(|| {
                            files::error(
                                "ENCODE_VALIDATION_FAILED",
                                "The saved filtered AV1AN stage has no video.",
                                &intermediate.path,
                            )
                        })?;
                        plan.observe_source_filter_stream(stream, None)?;
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
                    args: standalone_consumer_arguments(&plan, settings, frame_count),
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
                let svt_phase;
                self.phase(id, JobState::Running, match settings.encoder {
                VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
                    svt_phase = format!("Encoding {}-bit AV1 with the standalone SVT encoder; selected audio settings will be applied afterward.", plan.output_bit_depth());
                    &svt_phase
                }
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
                let mut filtered_sar = None;
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
                    snapshot.encoder_progress = None;
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
                    let event_task = self.observe_encode_with_estimate(
                        id,
                        events,
                        Some(plan.frame_seconds()),
                        plan.source_frames_estimated,
                    );
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
                    self.change(id, |snapshot| snapshot.encoder_progress = None)
                        .await;
                    let result = match result {
                        Err(error) => {
                            if error.is_pipeline_stage_failure()
                                && producer.args.iter().any(|arg| arg == "-stats_enc_pre:0")
                                && let Ok(log) = tokio::fs::read(&pass_log).await
                                && let Some(capability) = unsupported_producer_clock_option(
                                    &String::from_utf8_lossy(&log),
                                )
                            {
                                return Err(files::error(
                                    "SOURCE_FILTER_TOOL_UNSUPPORTED",
                                    format!(
                                        "This FFmpeg build does not support {capability}, which this encode needs to verify the input frame clock."
                                    ),
                                    &ffmpeg,
                                ));
                            }
                            return Err(process_error(error, &source.path));
                        }
                        Ok(result) => result,
                    };
                    if !result.producer_status.success() || !result.consumer_status.success() {
                        return Err(files::error(
                            "ENCODE_FAILED",
                            format!(
                                "Standalone encoder pass {pass} did not complete successfully."
                            ),
                            &source.path,
                        ));
                    }
                    if settings.source_filters && plan.standalone_requires_container_sar() {
                        filtered_sar = Some(
                            super::source_filters::y4m_sample_aspect(&result.producer_prefix)?
                                .to_owned(),
                        );
                    }
                    if two_pass && pass == 1 {
                        let stats = stats.as_ref().expect("two-pass stats");
                        stats.verify_stats()?;
                        if let Some(recovery) = standalone_recovery.as_mut() {
                            let summary = recovery.checkpoint_pass_one(stats, cancel).await?;
                            self.change(id, |snapshot| {
                                snapshot.standalone_recovery = Some(summary)
                            })
                            .await;
                        }
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
                    if plan.source_frames_estimated && !settings.source_filters {
                        let cadence = super::source_filters::Cadence {
                            frame_seconds: plan.frame_seconds(),
                            tolerance: plan.tolerance,
                        };
                        let clock = producer_clock(cancel, &output, Some(cadence)).await?;
                        frame_count = usize::try_from(clock.frames).map_err(|_| {
                            files::error(
                                "ENCODE_VALIDATION_FAILED",
                                "The encoder input frame count exceeds this platform.",
                                &output,
                            )
                        })?;
                        plan.source_frames_estimated = false;
                        self.change(id, |snapshot| {
                            snapshot.duration_seconds = Some(frame_count as f64 * plan.frame_seconds());
                            append_log(snapshot, format!("Actual encoder input: {frame_count} frames counted during encoding."));
                        }).await;
                    }
                    // Source filters set the plan's size and frame count from
                    // the filtered producer, so their observation always runs.
                    if settings.source_filters {
                        validate_video_checkpoint(
                            &ffprobe,
                            intermediate,
                            &mut plan,
                            &mut frame_count,
                            producer_clock(cancel, &output, None).await?,
                            filtered_sar.as_deref(),
                            cancel,
                        )
                        .await?;
                    }
                    if verify_svt_ivf {
                        svt_ivf::verify(intermediate, frame_count).await?;
                    }
                    if let Some(recovery) = standalone_recovery.as_mut() {
                        if source_qtgmc && let Some(prepared) = &qtgmc {
                            boxed(|| recovery.verify_source_qtgmc(&prepared.video.path, cancel))
                                .await?;
                        }
                        if settings.source_filters {
                            recovery.remember_source_filter_contract(
                                plan.source_filter_contract(frame_count),
                            )?;
                        }
                        let summary = recovery
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
            self.phase_with_fresh_progress(
                id,
                JobState::Finalizing,
                "Muxing encoded video, applying selected audio settings, and preserving subtitles, metadata, chapters, and attachments.",
            ).await;
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
            append_attachments(
                &mut mux_arguments,
                &output_selected,
                cover_art.as_ref(),
                settings_attachment.as_ref(),
            );
            let observe_movie = container::format(&output)?
                == media_core::ContainerFormat::Matroska
                && output_selected
                    .iter()
                    .position(|stream| stream.codec_type.as_deref() == Some("video"))
                    == Some(video_position);
            if observe_movie {
                mux_arguments.splice(
                    ..0,
                    super::output_counts::MUX_PROGRESS_ARGS.map(OsString::from),
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
            let completed = observer.await.ok().flatten();
            let result = result.map_err(|e| process_error(e, &source.path))?;
            if !result.status.success() {
                return Err(files::error(
                    "ENCODE_MUX_FAILED",
                    "The encoded video could not be muxed with the selected source tracks.",
                    &temp.path,
                ));
            }
            if observe_movie {
                mux_video_packets = completed;
            }
            self.change(id, |snapshot| {
                append_log(snapshot, format!("Mux log: {}", mux_log.display()))
            })
            .await;
            check_cancel(cancel)?;
            temp.flush_nonempty_async().await?;
            self.phase(
                id,
                JobState::Finalizing,
                if source_final_iso.is_none()
                    && container::format(&output)? != media_core::ContainerFormat::Matroska
                {
                    "Writing the final container and checking its stream headers."
                } else {
                    "Checking the final container's stream headers."
                },
            )
            .await;
            if let Some(format) = source_final_iso {
                if format == media_core::ContainerFormat::Mp4
                    && let Some(sar) = standalone_svt_sar(&plan, settings)
                    && !ffmpeg_accepts_sar(sar)
                {
                    container::restore_mp4_aspect(temp, video_position as u32, sar, cancel).await?;
                }
                source_iso_receipt = Some(container::seal_source_iso(temp, format, cancel).await?);
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
        if let Some(original) = original_guard {
            original.verify()?;
        }
        if let Some(staged) = standalone_hdr.take() {
            staged.finish()?;
        }
        grain_coverage.verify(frame_count, (plan.fps_num, plan.fps_den))?;
        if let Some(check) = source_hdr_check {
            check
                .finish(compare_hdr_scan_to_producer.then_some(frame_count))
                .await?;
        }
        let expected_kinds = output_selected
            .iter()
            .map(|stream| stream.codec_type.as_deref().unwrap_or_default())
            // Readers list a cover attachment as an attached picture.
            .chain(cover_art.as_ref().map(|_| "video"))
            .chain(
                settings
                    .av1an_options
                    .filter(|options| options.attach_settings)
                    .map(|_| "attachment"),
            )
            .collect::<Vec<_>>();
        let mp4_svt_sar = if container::format(&output)? == media_core::ContainerFormat::Mp4 {
            standalone_svt_sar(&plan, settings)
        } else {
            None
        };
        let mov_text = prepared_trim
            .as_ref()
            .map(|trim| trim.copied_mov_text_proof())
            .unwrap_or_default()
            .iter()
            .map(|(index, proof)| {
                let position = output_selected
                    .iter()
                    .position(|stream| stream.index == *index)
                    .ok_or_else(|| {
                        files::error(
                            "TRIM_VALIDATION_FAILED",
                            "A planned copied MOV_TEXT track was omitted from the output mapping.",
                            &output,
                        )
                    })?;
                Ok((position, proof))
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        self.finalize(
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
                mux_video_packets,
                count_duration_frame_seconds: (settings.backend
                    == media_core::EncodeBackend::Standalone
                    && !settings.source_filters
                    && !plan.restores_source_dolby_clock()
                    && settings.mov_timecode_track.is_none())
                .then(|| plan.frame_seconds()),
                source_iso: if let Some(receipt) = source_iso_receipt {
                    receipt.verify(temp, cancel).await?;
                    Some(receipt)
                } else {
                    None
                },
                basic_video_check: plan.source_queue_video_check().or_else(|| {
                    mp4_svt_sar.map(|sar| super::output_counts::VideoCheck {
                        codec: plan.output_codec(),
                        dimensions: (plan.width, plan.height),
                        sample_aspect_ratio: Some(sar),
                        frame_seconds: None,
                    })
                }),
                mp4_aspect_override: mp4_svt_sar
                    .filter(|&sar| resume_finalizing || !ffmpeg_accepts_sar(sar)),
                mov_text: &mov_text,
                expected_kinds: &expected_kinds,
            }),
            scratch,
        )
        .await?;
        drop(standalone_stats);
        drop(standalone_timed);
        drop(standalone_final);
        drop(durable);
        if let Some(recovery) = recovery {
            self.cleanup_published_av1an(id, &recovery, temporary).await;
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

    async fn cleanup_published_av1an(
        &self,
        id: &str,
        recovery: &super::av1an::Recovery,
        temporary: &mut Option<Temporary>,
    ) {
        // Unlink the owned attempt before its parent. A failed ownership check
        // retains the workspace, and cleanup cannot undo successful publication.
        let cleanup = match temporary
            .take()
            .map_or(Ok(()), |mut attempt| attempt.cleanup())
        {
            Ok(()) => recovery.cleanup().await,
            Err(error) => Err(error),
        };
        self.change(id, |snapshot| match cleanup {
            Ok(()) => snapshot.recovery = None,
            Err(error) => {
                append_log(
                    snapshot,
                    format!(
                        "Output succeeded; recovery files were retained: {}",
                        error.message
                    ),
                );
                snapshot.error = Some(error);
            }
        })
        .await;
    }

    /// AV1AN's queue has frame ranges, without source presentation timestamps.
    /// Investigate a duration discrepancy only when the video itself declares
    /// a duration. A longer audio or subtitle stream is normal.
    #[allow(clippy::too_many_arguments)]
    async fn verify_av1an_source_duration(
        &self,
        id: &str,
        ffprobe: &Path,
        input: &Path,
        video: &metadata::Stream,
        plan: &Plan,
        queue_frames: usize,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        let frame_seconds = plan.frame_seconds();
        let queue_duration = queue_frames as f64 * frame_seconds;
        if !metadata::track_duration(video).is_some_and(|duration| {
            duration > 0.0
                && (duration - queue_duration).abs() > frame_seconds / 2.0 + plan.tolerance
        }) {
            return Ok(());
        }
        self.change(id, |snapshot| append_log(snapshot,
            "The AV1AN queue count disagrees with the selected video's declared duration. Checking the source packet timeline before publication.".into(),
        )).await;
        let invalid_clock = |reason| {
            files::error(
                "AV1AN_SOURCE_CLOCK_INVALID",
                format!(
                    "The AV1AN queue could not be reconciled with the source video clock: {reason}"
                ),
                input,
            )
        };
        match super::source_timing::plan_from_packets(ffprobe, input, plan, video, false, cancel)
            .await
        {
            Ok(Ok((checked, count)))
                if count == queue_frames
                    && (checked.fps_num, checked.fps_den) == (plan.fps_num, plan.fps_den) =>
            {
                Ok(())
            }
            Ok(Ok(_)) => Err(invalid_clock(
                "the packet count or frame rate differs from the encoded queue".into(),
            )),
            Ok(Err(reason)) => Err(invalid_clock(reason.to_string())),
            Err(error) if error.code == "ENCODE_INPUT_UNSUPPORTED" => {
                Err(invalid_clock(error.message))
            }
            Err(error) => Err(error),
        }
    }

    /// Ordinary jobs plan from headers and learn their exact count during the
    /// encode. When exact boundaries are required, packet timestamps can
    /// stand in for decoded frames. Report whether a complete decoded scan
    /// already checked HDR.
    #[allow(clippy::too_many_arguments)]
    async fn source_frames(
        &self,
        id: &str,
        ffprobe: &Path,
        input: &Path,
        plan: &mut Plan,
        stream: &metadata::Stream,
        duration: Option<f64>,
        preview: bool,
        check_discard: bool,
        fast_start: bool,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(usize, bool), AppError> {
        if fast_start
            && !check_discard
            && let Some((planned, count)) = super::source_timing::plan_from_headers(
                ffprobe, input, plan, stream, duration, cancel,
            )
            .await?
        {
            *plan = planned;
            self.change(id, |snapshot| append_log(snapshot, format!(
                "Planning approximately {count} frames from source headers; the actual encode will establish the exact count."
            ))).await;
            return Ok((count, false));
        }
        self.change(id, |snapshot| {
            append_log(
                snapshot,
                "Planning source frames from every packet timestamp and the first decoded frames."
                    .into(),
            )
        })
        .await;
        match super::source_timing::plan_from_packets(
            ffprobe,
            input,
            plan,
            stream,
            check_discard,
            cancel,
        )
        .await?
        {
            Ok((planned, count)) => {
                if !preview {
                    self.change(id, |snapshot| {
                        append_log(
                            snapshot,
                            format!("Planned {count} constant-rate frames from packet timestamps."),
                        )
                    })
                    .await;
                }
                *plan = planned;
                return Ok((count, false));
            }
            Err(reason) => {
                self.change(id, |snapshot| {
                        append_log(snapshot, format!("Packet timing cannot stand in for decoded frames ({reason}); decoding the complete source before encoding."))
                    })
                    .await;
                if check_discard && reason.discarded {
                    let count = self
                        .scan_frames(id, ffprobe, input, plan, stream, false, cancel)
                        .await?;
                    plan.av1an_presented_rate = Some((plan.fps_num, plan.fps_den));
                    return Ok((count, true));
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
            .map(|count| (count, true))
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
        events: mpsc::Receiver<ProcessEvent>,
        frame_seconds: Option<f64>,
    ) -> tokio::task::JoinHandle<Option<usize>> {
        self.observe_encode_with_estimate(id, events, frame_seconds, false)
    }

    fn observe_encode_with_estimate(
        &self,
        id: &str,
        mut events: mpsc::Receiver<ProcessEvent>,
        frame_seconds: Option<f64>,
        estimated: bool,
    ) -> tokio::task::JoinHandle<Option<usize>> {
        let manager = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let mut progress = super::output_counts::MuxProgress::default();
            while let Some(event) = events.recv().await {
                let line = match event {
                    ProcessEvent::Stdout(line) => {
                        progress.observe(&line);
                        line
                    }
                    ProcessEvent::Stderr(line) => line,
                };
                manager
                    .change(&id, |snapshot| {
                        let record = consumer_record(&line);
                        let ffmpeg_record = record.strip_prefix("[producer] ").unwrap_or(record);
                        if let Some(progress) = progress_seconds(ffmpeg_record) {
                            update_encode_progress(snapshot, progress, estimated);
                        }
                        let encoder_progress = svt_progress(&line);
                        let svt_frames = encoder_progress
                            .as_ref()
                            .map(|stats| stats.frames)
                            .or_else(|| svt_frame_counter(&line));
                        let x264_frames = x264::frame_counter(&line);
                        if let (Some(value), Some(frame_seconds)) =
                            (svt_frames.or(x264_frames), frame_seconds)
                        {
                            update_encode_progress(
                                snapshot,
                                value as f64 * frame_seconds,
                                estimated,
                            );
                        }
                        if let Some(stats) = encoder_progress {
                            snapshot.encoder_progress = Some(stats);
                        }
                        // The footer already shows these live status records. Keep the
                        // bounded job log for diagnostics and encoder summaries.
                        let redraw = svt_frames.is_some()
                            || record == "Encoding"
                            || (x264_frames.is_some() && !record.starts_with("encoded "))
                            || (supervisor::is_ffmpeg_progress_line(ffmpeg_record)
                                && !ffmpeg_record.contains(" Lsize="));
                        if !redraw {
                            append_log(snapshot, line);
                        }
                    })
                    .await;
            }
            progress.completed()
        })
    }
}

fn update_encode_progress(snapshot: &mut JobSnapshot, progress: f64, estimated: bool) {
    if estimated
        && snapshot
            .duration_seconds
            .is_some_and(|duration| progress > duration)
    {
        snapshot.duration_seconds = Some(progress);
    }
    snapshot.progress_seconds = Some(
        snapshot
            .duration_seconds
            .map_or(progress, |duration| progress.min(duration)),
    );
}

fn consumer_record(line: &str) -> &str {
    let line = line.trim();
    line.strip_prefix("[consumer] ").unwrap_or(line)
}

fn svt_frame_counter(line: &str) -> Option<u64> {
    let record = consumer_record(line);
    record
        .strip_prefix("Encoding:")
        .or_else(|| record.strip_prefix("Encoding frame "))?
        .split_whitespace()
        .next()?
        .split('/')
        .next()?
        .parse()
        .ok()
}

fn svt_progress(line: &str) -> Option<EncoderProgress> {
    let record = consumer_record(line);
    let (count, metrics) = record.strip_prefix("Encoding:")?.split_once(" Frames @ ")?;
    let (frames, total_frames) = match count.trim().split_once('/') {
        Some((frames, total)) => (
            frames.parse().ok()?,
            total.parse::<u64>().ok().filter(|n| *n > 0),
        ),
        None => (count.trim().parse().ok()?, None),
    };
    let mut fields = metrics.split('|').map(str::trim);
    let speed = fields.next()?;
    let fps = if let Some(value) = speed.strip_suffix("fps") {
        svt_number(value)
    } else {
        speed
            .strip_suffix("fpm")
            .and_then(svt_number)
            .map(|value| value / 60.0)
    };
    let bitrate_kbps = fields
        .next()
        .and_then(|value| value.strip_suffix("kb/s"))
        .and_then(svt_number);
    let (size_bytes, estimated_size_bytes) = fields
        .next()
        .and_then(|value| value.strip_prefix("Size: "))
        .map_or((None, None), |value| {
            let (size, estimate) = svt_bracketed_detail(value);
            (svt_size_bytes(size), estimate.and_then(svt_size_bytes))
        });
    let (elapsed_seconds, eta_seconds) = fields
        .next()
        .and_then(|value| value.strip_prefix("Time: "))
        .map_or((None, None), |value| {
            let (elapsed, eta) = svt_bracketed_detail(value);
            (
                svt_clock_seconds(elapsed),
                eta.and_then(|value| value.strip_prefix('-'))
                    .and_then(svt_clock_seconds),
            )
        });
    Some(EncoderProgress {
        frames,
        total_frames,
        percent: total_frames.map(|total| (frames as f64 / total as f64 * 100.0).min(100.0)),
        fps,
        bitrate_kbps,
        size_bytes,
        estimated_size_bytes,
        elapsed_seconds,
        eta_seconds,
    })
}

fn svt_bracketed_detail(value: &str) -> (&str, Option<&str>) {
    value
        .split_once(" [")
        .map_or((value, None), |(current, prediction)| {
            (current, prediction.strip_suffix(']'))
        })
}

fn svt_number(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn svt_size_bytes(value: &str) -> Option<u64> {
    let bytes = svt_number(value.strip_suffix(" MB")?)? * 1_000_000.0;
    (bytes < u64::MAX as f64).then(|| bytes.round() as u64)
}

fn svt_clock_seconds(value: &str) -> Option<u64> {
    let (hours, rest) = value.split_once(':')?;
    let (minutes, seconds) = rest.split_once(':')?;
    let hours = hours.parse::<u64>().ok()?;
    let minutes = minutes.parse::<u64>().ok()?;
    let seconds = seconds.parse::<u64>().ok()?;
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    hours.checked_mul(3600)?.checked_add(minutes * 60 + seconds)
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

async fn probe_encode_source(
    ffprobe: &Path,
    input: &Path,
    output: &Path,
    selected: &[u32],
    settings: &EncodeSettings,
    cancel: &watch::Receiver<bool>,
) -> Result<Document, AppError> {
    if settings.mov_timecode_track.is_some() {
        return probe(ffprobe, input, cancel, Some(selected)).await;
    }
    let document = probe_headers(ffprobe, input, cancel, Some(selected)).await?;
    if !container::needs_cue_counts(output, &document, selected) {
        return Ok(document);
    }
    let keep: Vec<_> = selected
        .iter()
        .copied()
        .filter(|index| {
            !settings.subtitles.iter().any(|track| {
                track.stream_index == *index && track.mode == media_core::SubtitleMode::BurnIn
            })
        })
        .collect();
    container::probe_cues(ffprobe, input, document, &keep, cancel).await
}

/// Validate the selected streams and settings before starting the encoder.
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
        ffprobe, input, args, plan, stream, encoded, cancel, progress,
    )
    .await
}

fn scan_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8)
}

/// Validate source frames while consuming FFprobe's structured stream.
#[allow(clippy::too_many_arguments)]
async fn validate_frames(
    ffprobe: &Path,
    input: &Path,
    args: Vec<OsString>,
    plan: &mut Plan,
    stream: &metadata::Stream,
    encoded: bool,
    cancel: &watch::Receiver<bool>,
    progress: Option<watch::Sender<f64>>,
) -> Result<usize, AppError> {
    let mut validation = Validation::new(plan.clone(), stream.clone(), encoded);
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
    frame_scan_arguments(
        input,
        index,
        threads,
        "best_effort_timestamp_time,interlaced_frame,top_field_first,width,height,pix_fmt,sample_aspect_ratio,chroma_location,color_space,color_transfer,color_primaries,color_range",
    )
}

pub(super) fn hdr_scan_args(input: &Path, index: u32) -> Vec<OsString> {
    frame_scan_arguments(input, index, scan_threads(), "best_effort_timestamp_time")
}

fn frame_scan_arguments(input: &Path, index: u32, threads: usize, fields: &str) -> Vec<OsString> {
    const SIDE_DATA_FIELDS: &str = "side_data_type,displaymatrix,rotation,red_x,red_y,green_x,green_y,blue_x,blue_y,white_point_x,white_point_y,max_luminance,min_luminance,max_content,max_average,dv_profile,dv_bl_signal_compatibility_id,bl_present_flag,rpu_present_flag,el_present_flag,bl_bit_depth,bl_video_full_range_flag,disable_residual_flag";
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
    args.extend([
        "-show_frames".into(),
        "-show_entries".into(),
        format!("frame={fields}:frame_side_data={SIDE_DATA_FIELDS}").into(),
        "-of".into(),
        "json".into(),
        "-i".into(),
    ]);
    args.push(input.as_os_str().to_owned());
    args
}

/// Jobs with these operations need exact frame boundaries before starting.
pub(super) fn header_planning_allowed(settings: &EncodeSettings) -> bool {
    settings.trim.is_none()
        && settings.hdr_metadata.is_none()
        && settings.rate_control.is_none()
        && settings.mov_timecode_track.is_none()
        && settings.temporal.is_none_or(|temporal| {
            temporal.frame_rate.is_none()
                && temporal.deinterlace.is_none()
                && temporal.qtgmc.is_none()
                && temporal.cadence_repair.is_none()
        })
}

fn quick_header_planning(settings: &EncodeSettings) -> bool {
    can_observe_standalone_producer(settings) && header_planning_allowed(settings)
}

fn can_observe_standalone_producer(settings: &EncodeSettings) -> bool {
    settings.video_mode == media_core::VideoMode::Encode
        && settings.backend == media_core::EncodeBackend::Standalone
        && !settings.source_nvenc_quality
}

fn unsupported_producer_clock_option(log: &str) -> Option<&'static str> {
    for line in log
        .lines()
        .filter_map(|line| line.strip_prefix("[stderr] [producer] "))
    {
        let line = line.to_ascii_lowercase();
        if (line.contains("stats_enc_pre")
            && (line.contains("unrecognized option") || line.contains("not found")))
            || (line.contains("invalid format directive")
                && (line.ends_with("tbi") || line.ends_with("ptsi")))
        {
            return Some("encoder-pre statistics (-stats_enc_pre)");
        }
    }
    None
}

fn tone_map_sample_duration(
    estimated: bool,
    counted_duration: f64,
    video: &metadata::Stream,
    container_duration: Option<f64>,
) -> Option<f64> {
    if estimated {
        // Prefer the selected stream. Container duration may include a
        // longer audio or subtitle track, and a stale count can skew seeks.
        metadata::track_duration(video)
            .filter(|duration| *duration > 0.0)
            .or(container_duration)
    } else {
        Some(counted_duration)
    }
}

async fn producer_clock(
    cancel: &watch::Receiver<bool>,
    output: &Path,
    cadence: Option<super::source_filters::Cadence>,
) -> Result<super::source_filters::ProducerClock, AppError> {
    super::source_filters::current()
        .ok_or_else(|| {
            files::error(
                "SOURCE_FILTER_OBSERVATION_INVALID",
                "No producer observation was set up for this encode.",
                output,
            )
        })?
        .completed_with_cadence(cancel, cadence)
        .await?
        .into_iter()
        .find(|clock| clock.stream_index == 0 && clock.frames > 0)
        .ok_or_else(|| {
            files::error(
                "SOURCE_FILTER_OBSERVATION_INVALID",
                "The encoder input contained no video frames.",
                output,
            )
        })
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
async fn validate_video_checkpoint(
    ffprobe: &Path,
    checkpoint: &Temporary,
    plan: &mut Plan,
    expected_frames: &mut usize,
    producer: super::source_filters::ProducerClock,
    filtered_sar: Option<&str>,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let document = if plan.encoder == VideoEncoder::X265Standalone {
        // Raw HEVC has no presentation timestamps until mkvmerge assigns the
        // verified rational clock. Probe its codec and color fields here,
        // and use the filtered producer's exact frame count.
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
        probe_headers(ffprobe, &checkpoint.path, cancel, Some(&[0])).await?
    };
    let encoded = document.streams.first().ok_or_else(|| {
        files::error(
            "ENCODE_VALIDATION_FAILED",
            "The complete video checkpoint has no decodable video stream.",
            &checkpoint.path,
        )
    })?;
    plan.observe_source_filter_stream(encoded, Some(&producer))?;
    if let Some(sar) = filtered_sar {
        plan.observe_producer_sar(sar)?;
    }
    *expected_frames = usize::try_from(producer.frames).map_err(|_| {
        files::error(
            "ENCODE_VALIDATION_FAILED",
            "The filtered producer frame count exceeds this platform.",
            &checkpoint.path,
        )
    })?;
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
    if plan.has_source_filters() {
        args.extend(super::source_filters::arguments(false));
    }
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
    // FFmpeg 6.x subtracts the file's start time, which an AAC track's codec
    // delay can place half a frame before the video. Passed through to the
    // Y4M frame clock, two frames would then round to the same tick. Start the
    // producer timeline at zero after every other filter; FFmpeg 7+ already
    // delivers such files from zero, and trims inside the graph are unaffected.
    let zero_timeline =
        !plan.cadence_reconciled && !plan.has_source_filters() && !plan.uses_source_output_trim();
    let timeline = plan
        .source_ordinal_trim_filter()
        .or_else(|| zero_timeline.then(|| "setpts=PTS-STARTPTS".to_owned()));
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
        let (filter, label) = if let Some(timeline) = timeline {
            (
                format!("{filter};[jesses_video]{timeline}[jesses_timeline]"),
                "[jesses_timeline]",
            )
        } else {
            (filter, "[jesses_video]")
        };
        args.extend([
            "-filter_complex".into(),
            filter.into(),
            "-map".into(),
            label.into(),
        ]);
    } else {
        args.extend(["-map".into(), format!("0:{}", plan.video_index).into()]);
        let filter = if text.is_none() {
            plan.decoder_filter()
        } else {
            plan.decoder_filter_with_text(text)
        };
        let filter = if let Some(timeline) = timeline {
            Some(filter.map_or_else(|| timeline.clone(), |filter| format!("{filter},{timeline}")))
        } else {
            filter
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
    if plan.source_frames_estimated || plan.has_source_filters() {
        args.extend(super::source_filters::arguments(!plan.has_source_filters()));
    }
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

fn svt_quality(settings: &EncodeSettings) -> (String, String) {
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
    (svt_crf, svt_preset)
}

pub(super) fn requires_serial_svt(plan: &Plan, settings: &EncodeSettings) -> bool {
    (!plan.allow_parallel_svt
        || (plan.legacy_svt_parameters && settings.backend == media_core::EncodeBackend::Av1an))
        && super::parameters::requires_serial_svt(settings)
}

pub(super) fn encoder_parameters(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let (svt_crf, svt_preset) = svt_quality(settings);
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
    if (plan.legacy_svt_parameters
        || !settings
            .parameters
            .iter()
            .any(|value| value.name == "keyint"))
        && let Some(frames) = super::encoding_preferences::keyframes(plan, settings)
    {
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
                    media_core::HdrTune::FilmGrain => "6",
                }
                .into(),
            ])
        }
        _ => {}
    }
    args.extend(super::parameters::arguments(settings));
    if requires_serial_svt(plan, settings) {
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
            // SVT rewrites the IVF header at EOF. Redirect stdout to a
            // seekable file so the rewrite cannot append a second header.
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

fn standalone_recovery_arguments(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    if !settings.encoder.is_svt()
        || settings.encoding_preferences.is_none()
        || !settings
            .parameters
            .iter()
            .any(|value| value.name == "keyint")
    {
        return consumer_arguments(plan, settings);
    }
    // Version-one standalone receipts retain their argument signature. SVT
    // already used the final keyint value; actual invocation emits it once.
    let mut legacy = plan.clone();
    legacy.legacy_svt_parameters = true;
    consumer_arguments(&legacy, settings)
}

fn svt_progress_frame_limit(settings: &EncodeSettings, frames: usize) -> Option<usize> {
    // --frames is also an encoder input limit. Typed standalone transforms have
    // a deterministic frame plan; source filter graphs may change the count.
    // SVT's progress format prints the total as a signed 32-bit number.
    (settings.backend == media_core::EncodeBackend::Standalone
        && matches!(
            settings.encoder,
            VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1Hdr
        )
        && !settings.source_filters
        && (1..=i32::MAX as usize).contains(&frames))
    .then_some(frames)
}

fn standalone_consumer_arguments(
    plan: &Plan,
    settings: &EncodeSettings,
    frames: usize,
) -> Vec<OsString> {
    let mut args = consumer_arguments(plan, settings);
    if !plan.source_frames_estimated
        && let Some(count) = svt_progress_frame_limit(settings, frames)
    {
        args.extend(["--frames".into(), count.to_string().into()]);
    }
    args
}

fn append_attachments(
    args: &mut Vec<OsString>,
    selected: &[&metadata::Stream],
    cover: Option<&super::cover_art::CoverArt>,
    settings: Option<&super::encode_settings::SettingsAttachment>,
) {
    let mut count = selected
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("attachment"))
        .count();
    if let Some(cover) = cover {
        cover.apply(args, count);
        count += 1;
    }
    if let Some(settings) = settings {
        settings.apply(args, count);
    }
}

fn standalone_svt_sar(plan: &Plan, settings: &EncodeSettings) -> Option<(u32, u32)> {
    (settings.backend == media_core::EncodeBackend::Standalone && settings.encoder.is_svt()).then(
        || {
            super::encode_plan::sample_aspect_ratio(Some(plan.output_sar()))
                .expect("validated output SAR")
        },
    )
}

fn ffmpeg_accepts_sar((numerator, denominator): (u32, u32)) -> bool {
    // FFmpeg's stream-copy codec option accepts SARs up to 10:1.
    numerator <= denominator.saturating_mul(10)
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
    if let Some(origin) = plan.rebased_source_origin() {
        // The encoded video starts at zero; move the source's tracks back by
        // its origin, as a canonical copy would have.
        let position = args.len() - 1;
        args.splice(
            position..position,
            ["-itsoffset".into(), format!("{:.9}", -origin).into()],
        );
    }
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
        if let Some(options) = external.track_metadata(stream, settings) {
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
            if ((!settings.source_filters
                || (settings.backend == media_core::EncodeBackend::Standalone
                    && plan.standalone_requires_container_sar()))
                && !plan.has_source_queue()
                && !plan.has_basic_source_queue())
                || plan.source_queue_needs_mux_aspect()
            {
                if let Some((numerator, denominator)) =
                    standalone_svt_sar(plan, settings).filter(|&sar| ffmpeg_accepts_sar(sar))
                {
                    // IVF has no sample aspect ratio to copy. Use the sample
                    // ratio directly because -aspect rounds the display ratio.
                    args.extend([
                        format!("-sar:{index}").into(),
                        format!("{numerator}/{denominator}").into(),
                    ]);
                } else {
                    let (sar_num, sar_den) =
                        super::encode_plan::sample_aspect_ratio(Some(plan.output_sar()))
                            .expect("validated output SAR");
                    let display_num = u64::from(plan.width) * u64::from(sar_num);
                    let display_den = u64::from(plan.height) * u64::from(sar_den);
                    let (mut a, mut b) = (display_num, display_den);
                    while b != 0 {
                        (a, b) = (b, a % b);
                    }
                    args.extend([
                        format!("-aspect:{index}").into(),
                        format!("{}:{}", display_num / a, display_den / a).into(),
                    ]);
                }
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

    #[tokio::test]
    async fn encode_observer_keeps_progress_in_statistics_and_preserves_the_log() {
        let manager = JobManager::new(PathBuf::new());
        let snapshot = serde_json::from_value(serde_json::json!({
            "id": "svt-progress",
            "state": "running",
            "request": {"inputPath":"source.mkv","outputPath":"output.mkv","streamIndices":[0]},
            "progressSeconds": 0.0,
            "durationSeconds": 2932.0,
            "logs": ["Command: SvtAv1EncApp --tune 0"],
        }))
        .unwrap();
        manager
            .state
            .lock()
            .await
            .entries
            .push(super::super::Entry {
                command_review: None,
                pause: Default::default(),
                phase_before_pause: None,
                snapshot,
                cancel: watch::channel(false).0,
                task: None,
            });
        let (sender, events) = mpsc::channel(32);
        let observer = manager.observe_encode("svt-progress", events, Some(1.0 / 24.0));
        let warning = "[consumer] Svt[warn]: Encoding: 2400 Frames reports estimated size.";
        let error = "[producer] Error: Encoding frame 5 has an invalid timestamp.";
        let summary = "[consumer] Total Encoding Time: 42471 ms";
        let mux_summary = "[producer] frame= 2589 fps=0.0 q=-1.0 Lsize=98210kB time=00:01:47.87";
        for frame in 2289..=2589 {
            let line = format!(
                "Encoding: {frame}/70369 Frames @ 60.97 fps | 6204.94 kb/s | Size: 98.21 MB [2669.45 MB] | Time: 0:00:42 [-0:18:32]"
            );
            let event = if frame % 2 == 0 {
                ProcessEvent::Stdout(line)
            } else {
                ProcessEvent::Stderr(format!("[consumer] {line}"))
            };
            sender.send(event).await.unwrap();
            if frame == 2400 {
                sender
                    .send(ProcessEvent::Stderr(warning.into()))
                    .await
                    .unwrap();
            }
            if frame == 2460 {
                sender
                    .send(ProcessEvent::Stderr(error.into()))
                    .await
                    .unwrap();
            }
        }
        for line in ["[consumer] SUMMARY", summary, mux_summary] {
            sender
                .send(ProcessEvent::Stdout(line.into()))
                .await
                .unwrap();
        }
        drop(sender);
        observer.await.unwrap();

        let snapshot = manager.list_jobs().await.pop().unwrap();
        let stats = snapshot.encoder_progress.unwrap();
        assert_eq!(snapshot.progress_seconds, Some(2589.0 / 24.0));
        assert_eq!(stats.frames, 2589);
        assert_eq!(stats.total_frames, Some(70_369));
        assert_eq!(stats.fps, Some(60.97));
        assert_eq!(stats.bitrate_kbps, Some(6204.94));
        assert_eq!(stats.size_bytes, Some(98_210_000));
        assert_eq!(stats.estimated_size_bytes, Some(2_669_450_000));
        assert_eq!(stats.elapsed_seconds, Some(42));
        assert_eq!(stats.eta_seconds, Some(1112));
        assert_eq!(
            snapshot.logs,
            [
                "Command: SvtAv1EncApp --tune 0",
                warning,
                error,
                "[consumer] SUMMARY",
                summary,
                mux_summary,
            ]
        );
    }

    #[test]
    fn reads_svt_progress_with_and_without_a_frame_total() {
        let known = svt_progress(
            "[consumer] Encoding:   12/50 Frames @ 30.00 fps | 1234.56 kb/s | Size: 12.34 MB [51.42 MB] | Time: 0:00:03 [-0:00:09]",
        ).unwrap();
        assert_eq!(known.frames, 12);
        assert_eq!(known.total_frames, Some(50));
        assert_eq!(known.percent, Some(24.0));
        assert_eq!(known.fps, Some(30.0));
        assert_eq!(known.bitrate_kbps, Some(1234.56));
        assert_eq!(known.size_bytes, Some(12_340_000));
        assert_eq!(known.estimated_size_bytes, Some(51_420_000));
        assert_eq!(known.elapsed_seconds, Some(3));
        assert_eq!(known.eta_seconds, Some(9));

        let unknown = svt_progress(
            "Encoding:    5 Frames @ 15.00 fpm | 53.64 kb/s | Size: 0.01 MB | Time: 1:00:03",
        )
        .unwrap();
        assert_eq!(unknown.frames, 5);
        assert_eq!(unknown.fps, Some(0.25));
        assert_eq!(unknown.elapsed_seconds, Some(3603));
        assert_eq!(unknown.total_frames, None);
        assert_eq!(unknown.percent, None);
        assert_eq!(unknown.eta_seconds, None);
        assert_eq!(unknown.estimated_size_bytes, None);
        assert!(svt_progress("Total Encoding Time: 300 ms").is_none());
    }

    #[test]
    fn only_known_standalone_hdr_frames_set_the_encoder_input_limit() {
        let known = |settings: &EncodeSettings| svt_progress_frame_limit(settings, 123);
        let mut settings = EncodeSettings {
            encoder: VideoEncoder::SvtAv1Hdr,
            ..Default::default()
        };
        let (_, plan) = producer_plan();
        let typed = standalone_consumer_arguments(&plan, &settings, 123);
        assert_eq!(argument_after(&typed, "--frames"), "123");
        let editable = consumer_arguments(&plan, &settings);
        assert!(!editable.iter().any(|arg| arg == "--frames"));
        assert_eq!(known(&settings), Some(123));
        assert_eq!(svt_progress_frame_limit(&settings, 0), None);
        settings.source_filters = true;
        assert_eq!(known(&settings), None);
        assert!(
            !standalone_consumer_arguments(&plan, &settings, 123)
                .iter()
                .any(|arg| arg == "--frames")
        );
        settings.source_filters = false;
        settings.backend = media_core::EncodeBackend::Av1an;
        assert_eq!(known(&settings), None);
        settings.backend = media_core::EncodeBackend::Standalone;
        settings.encoder = VideoEncoder::SvtAv1FiveFish;
        assert_eq!(known(&settings), None);
    }

    #[test]
    fn estimated_source_frames_never_limit_the_standalone_encoder() {
        let (_, mut plan) = producer_plan();
        plan.source_frames_estimated = true;
        for encoder in [VideoEncoder::SvtAv1, VideoEncoder::SvtAv1Hdr] {
            let settings = EncodeSettings {
                encoder,
                ..Default::default()
            };
            assert!(
                !standalone_consumer_arguments(&plan, &settings, 7)
                    .iter()
                    .any(|arg| arg == "--frames")
            );
        }
    }

    #[test]
    fn svt_encoder_arguments_preserve_fractional_crf_and_signed_presets() {
        let (_, plan) = producer_plan();
        let mut settings = EncodeSettings {
            encoder: VideoEncoder::SvtAv1Hdr,
            svt_preset: Some(-1),
            ..Default::default()
        };
        for (quarters, expected) in [(None, "0"), (Some(81), "20.25"), (Some(80), "20")] {
            settings.crf = 0;
            settings.svt_crf_quarter_steps = quarters;
            let args = encoder_parameters(&plan, &settings);
            assert_eq!(argument_after(&args, "--crf"), expected);
            assert_eq!(argument_after(&args, "--preset"), "-1");
        }
    }

    #[test]
    fn svt_keyframe_override_replaces_the_planned_interval() {
        let (_, plan) = producer_plan();
        let mut settings = EncodeSettings {
            encoder: VideoEncoder::SvtAv1Hdr,
            encoding_preferences: Some(media_core::EncodingPreferences {
                keyframe_interval_seconds: 1,
                mp4_faststart: false,
                retired_verification: (),
            }),
            ..Default::default()
        };
        let args = encoder_parameters(&plan, &settings);
        assert_eq!(args.iter().filter(|arg| *arg == "--keyint").count(), 1);
        assert_eq!(
            argument_after(&args, "--keyint"),
            super::super::encoding_preferences::keyframes(&plan, &settings)
                .unwrap()
                .to_string()
        );
        settings.parameters.push(media_core::EncoderParameter {
            name: "keyint".into(),
            value: "321".into(),
        });
        let args = encoder_parameters(&plan, &settings);
        assert_eq!(args.iter().filter(|arg| *arg == "--keyint").count(), 1);
        assert_eq!(argument_after(&args, "--keyint"), "321");
        let original_keyint = super::super::encoding_preferences::keyframes(&plan, &settings)
            .unwrap()
            .to_string();
        let signature = standalone_recovery_arguments(&plan, &settings);
        let saved_keyints: Vec<_> = signature
            .windows(2)
            .filter(|pair| pair[0] == "--keyint")
            .map(|pair| pair[1].to_str().unwrap())
            .collect();
        assert_eq!(saved_keyints, [original_keyint.as_str(), "321"]);
    }

    #[test]
    fn qualified_standalone_plan_omits_the_serial_svt_lane_override() {
        let (_, mut plan) = producer_plan();
        let settings = EncodeSettings {
            encoder: VideoEncoder::SvtAv1Hdr,
            parameters: ["hbd-mds", "enable-overlays"]
                .into_iter()
                .map(|name| media_core::EncoderParameter {
                    name: name.into(),
                    value: "1".into(),
                })
                .collect(),
            ..Default::default()
        };
        assert_eq!(
            argument_after(&consumer_arguments(&plan, &settings), "--lp"),
            "1"
        );
        plan.allow_parallel_svt = true;
        let args = consumer_arguments(&plan, &settings);
        assert!(!args.iter().any(|arg| arg == "--lp"));
        assert_eq!(argument_after(&args, "--hbd-mds"), "1");
        assert_eq!(argument_after(&args, "--enable-overlays"), "1");
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

    fn producer_plan() -> (Document, Plan) {
        let document: Document = serde_json::from_value(serde_json::json!({
            "format":{"start_time":"-0.021"},
            "streams":[
                {"index":0,"codec_type":"video","codec_name":"ffv1","width":128,"height":96,"pix_fmt":"yuv420p","sample_aspect_ratio":"1:1","avg_frame_rate":"24000/1001","start_time":"0","color_space":"bt709","color_primaries":"bt709","color_transfer":"bt709","color_range":"tv"},
                {"index":1,"codec_type":"audio","codec_name":"aac","channels":2,"channel_layout":"stereo","sample_rate":"48000","start_time":"-0.021"}
            ]
        }))
        .unwrap();
        let selected = document.selected(&[0, 1]).unwrap();
        let plan = Plan::build(&document, &selected, &EncodeSettings::default()).unwrap();
        (document, plan)
    }

    fn argument_after<'a>(args: &'a [OsString], flag: &str) -> &'a str {
        let position = args.iter().position(|arg| arg == flag).expect(flag);
        args[position + 1].to_str().unwrap()
    }

    fn quick_convert_dolby_plan(rate: &str, time: Option<media_core::VideoTimeTrim>) -> Plan {
        let document: Document = serde_json::from_value(serde_json::json!({
            "format":{"start_time":"1.5"},
            "streams":[{
                "index":0,"codec_type":"video","codec_name":"hevc","width":128,"height":96,
                "pix_fmt":"yuv420p10le","field_order":"progressive","sample_aspect_ratio":"1:1",
                "avg_frame_rate":rate,"time_base":"1/120000","start_time":"1.5",
                "color_primaries":"bt2020","color_transfer":"smpte2084","color_space":"bt2020nc",
                "color_range":"tv","chroma_location":"topleft","side_data_list":[{
                    "side_data_type":"DOVI configuration record","dv_profile":8,
                    "dv_bl_signal_compatibility_id":1,"rpu_present_flag":1,"bl_present_flag":1,
                    "el_present_flag":0
                }]
            }]
        }))
        .unwrap();
        let settings = EncodeSettings {
            encoder: VideoEncoder::SvtAv1Hdr,
            trim: Some(media_core::VideoTrim {
                start_frame: 2480,
                end_frame_exclusive: 3552,
                time,
            }),
            source_trim: true,
            // Intact Dolby execution enables observation even with no custom rows.
            source_filters: true,
            temporal: Some(
                serde_json::from_value(serde_json::json!({"resizeFilter":"bicubic"})).unwrap(),
            ),
            tone_map: Some(media_core::ToneMapSettings {
                source_workflow: true,
                algorithm: Some(media_core::ToneMapAlgorithm::Spline),
                backend: media_core::ToneMapBackend::Auto,
                peak_mode: media_core::ToneMapPeakMode::Measured,
                source_peak_nits: 1000,
                hdr10_base_layer: false,
            }),
            ..Default::default()
        };
        Plan::build(&document, &document.selected(&[0]).unwrap(), &settings).unwrap()
    }

    #[test]
    fn quick_convert_dolby_frame_trim_uses_picture_ordinals_after_rendering() {
        let input = Path::new("input.mp4");
        let trim = "trim=start_frame=2480:end_frame=3552,setpts=PTS-STARTPTS";
        for rate in ["24/1", "24000/1001", "30000/1001"] {
            let plan = quick_convert_dolby_plan(rate, None);
            assert_eq!(plan.source_origin, 1.5);
            for bitmap in [None, Some((input, 1, 0.0))] {
                let args = decoder_args_with_subtitles(input, &plan, None, bitmap);
                let filter = argument_after(
                    &args,
                    if bitmap.is_some() {
                        "-filter_complex"
                    } else {
                        "-vf"
                    },
                );
                assert!(filter.contains("tonemapping=spline:peak_detect=1:apply_dolbyvision=1"));
                assert!(filter.find("libplacebo=").unwrap() < filter.find(trim).unwrap());
                assert_eq!(filter.matches("trim=").count(), 1, "{filter}");
                if bitmap.is_some() {
                    assert!(filter.ends_with(&format!("[jesses_video]{trim}[jesses_timeline]")));
                    assert_eq!(argument_after(&args, "-map"), "[jesses_timeline]");
                } else {
                    assert!(filter.ends_with(trim), "{filter}");
                }
                assert_eq!(argument_after(&args, "-init_hw_device"), "vulkan");
                assert!(!args.iter().any(|arg| arg == "-fps_mode"));
                assert!(
                    !args
                        .iter()
                        .any(|arg| matches!(arg.to_str(), Some("-ss" | "-t" | "-frames:v")))
                );
            }
        }
    }

    #[test]
    fn quick_convert_dolby_time_trim_keeps_the_requested_seek_strategy() {
        use media_core::VideoSeekStrategy::{DecodeFromStart, InputSeek};

        for strategy in [DecodeFromStart, InputSeek] {
            let plan = quick_convert_dolby_plan(
                "24000/1001",
                Some(media_core::VideoTimeTrim {
                    start_milliseconds: 103_437,
                    end_milliseconds: 148_148,
                    seek_strategy: strategy,
                }),
            );
            let args = decoder_args(Path::new("input.mp4"), &plan);
            assert_eq!(args.iter().filter(|arg| *arg == "-ss").count(), 1);
            assert_eq!(argument_after(&args, "-ss"), "103.437");
            assert_eq!(argument_after(&args, "-t"), "44.711");
            let seek = args.iter().position(|arg| arg == "-ss").unwrap();
            let input = args.iter().position(|arg| arg == "-i").unwrap();
            assert_eq!(seek < input, strategy == InputSeek);
            assert!(!argument_after(&args, "-vf").contains("trim="));
        }
    }

    /// FFmpeg 6.x shifts every stream by the file's start time, which an AAC
    /// track's codec delay makes about half a 23.976 fps frame early. Passed
    /// through to Y4M, two frames then round to the same tick and the muxer
    /// stops. The producer timeline therefore starts at zero after all other
    /// filters, as FFmpeg 7+ already delivers for such files.
    #[test]
    fn passthrough_producer_starts_its_timeline_at_zero() {
        let (_, plan) = producer_plan();
        let args = decoder_args(Path::new("input.mkv"), &plan);
        assert_eq!(argument_after(&args, "-fps_mode"), "passthrough");
        assert!(
            argument_after(&args, "-vf").ends_with(",setpts=PTS-STARTPTS"),
            "{args:?}"
        );
    }

    #[test]
    fn bitmap_subtitle_producer_starts_its_timeline_at_zero() {
        let (_, plan) = producer_plan();
        let args = decoder_args_with_subtitles(
            Path::new("input.mkv"),
            &plan,
            None,
            Some((Path::new("subtitles.mkv"), 0, 0.0)),
        );
        assert_eq!(argument_after(&args, "-fps_mode"), "passthrough");
        let label = argument_after(&args, "-map");
        assert!(
            argument_after(&args, "-filter_complex")
                .ends_with(&format!("setpts=PTS-STARTPTS{label}")),
            "{args:?}"
        );
    }

    /// A source title such as "BTM DDP5.1 Atmos" names the source codec and
    /// channel layout. Once that track is converted, the output title must
    /// name what was actually written, whether the title came from Quick
    /// Convert's track override or only from the copied source tag.
    #[test]
    fn mux_retitles_converted_audio_whose_title_names_the_source_format() {
        let document: Document = serde_json::from_value(serde_json::json!({
            "format":{"start_time":"0"},
            "streams":[
                {"index":0,"codec_type":"video","codec_name":"h264","width":128,"height":96,"pix_fmt":"yuv420p","sample_aspect_ratio":"1:1","avg_frame_rate":"24/1","start_time":"0","color_space":"bt709","color_primaries":"bt709","color_transfer":"bt709","color_range":"tv"},
                {"index":1,"codec_type":"audio","codec_name":"eac3","channels":6,"channel_layout":"5.1(side)","sample_rate":"48000","tags":{"title":"BTM DDP5.1 Atmos"}}
            ]
        }))
        .unwrap();
        let selected = document.selected(&[0, 1]).unwrap();
        let audio = vec![
            serde_json::from_value(serde_json::json!({
                "streamIndex":1,"codec":"opus","bitrateKbps":128,"channels":"stereo"
            }))
            .unwrap(),
        ];
        let with_override = EncodeSettings {
            audio: audio.clone(),
            track_overrides: vec![
                serde_json::from_value(serde_json::json!({
                    "streamIndex":1,"title":"BTM DDP5.1 Atmos","language":"eng","default":true
                }))
                .unwrap(),
            ],
            ..EncodeSettings::default()
        };
        let tags_only = EncodeSettings {
            audio,
            ..EncodeSettings::default()
        };
        for settings in [with_override, tags_only] {
            let plan = Plan::build(&document, &selected, &settings).unwrap();
            let args = mux_args(
                Path::new("input.mkv"),
                Path::new("video.ivf"),
                Path::new("output.mkv"),
                &selected,
                &plan,
                &settings,
                &super::external_tracks::ExternalTracks::default(),
            );
            let audio_tags: Vec<_> = args
                .windows(2)
                .filter(|pair| pair[0] == "-metadata:s:1")
                .map(|pair| pair[1].to_str().unwrap())
                .collect();
            assert!(audio_tags.contains(&"title=BTM Opus 2.0"), "{audio_tags:?}");
            assert!(
                audio_tags.contains(&"handler_name=BTM Opus 2.0"),
                "{audio_tags:?}"
            );
            assert!(
                !audio_tags.iter().any(|tag| tag.contains("DDP")),
                "{audio_tags:?}"
            );
        }
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
        customize: fn(&mut EncodeSettings),
    ) -> (Fixture, PathBuf, JobSnapshot, usize) {
        encode_with_audio_filter(
            encoder,
            preset,
            extension,
            audio_codec,
            customize,
            "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        )
        .await
    }

    async fn encode_with_audio_filter(
        encoder: VideoEncoder,
        preset: u8,
        extension: &str,
        audio_codec: &str,
        customize: fn(&mut EncodeSettings),
        filter: &str,
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
                    filter,
                    "-fps_mode",
                    "passthrough",
                    "-enc_time_base",
                    "filter",
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
                    // Deliberately stale header: conversion must encode the
                    // actual input, never stop at this advertised count.
                    "-metadata:s:v:0",
                    "NUMBER_OF_FRAMES=7",
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
        let mut settings = EncodeSettings {
            encoder,
            preset,
            encoding_preferences: Some(media_core::EncodingPreferences {
                keyframe_interval_seconds: 10,
                mp4_faststart: true,
                retired_verification: (),
            }),
            ..EncodeSettings::default()
        };
        customize(&mut settings);
        manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0, 1],
                },
                settings,
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

    const OUTPUT_CHECKED: &str = "Output check: ";

    #[tokio::test]
    async fn producer_clock_without_an_observer_returns_an_error() {
        let (_owner, cancel) = watch::channel(false);
        let error = producer_clock(&cancel, Path::new("output.mkv"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, "SOURCE_FILTER_OBSERVATION_INVALID");
    }

    #[test]
    fn progress_extends_only_an_estimated_duration() {
        let mut snapshot: JobSnapshot = serde_json::from_value(serde_json::json!({
            "id": "encode", "state": "running",
            "request": {"inputPath": "source.mkv", "outputPath": "output.mkv", "streamIndices": [0]},
            "logs": [], "durationSeconds": 1.0,
        })).unwrap();
        update_encode_progress(&mut snapshot, 1.25, false);
        assert_eq!(
            (snapshot.progress_seconds, snapshot.duration_seconds),
            (Some(1.0), Some(1.0))
        );
        update_encode_progress(&mut snapshot, 1.25, true);
        assert_eq!(
            (snapshot.progress_seconds, snapshot.duration_seconds),
            (Some(1.25), Some(1.25))
        );
    }

    #[test]
    fn tone_map_uses_the_selected_video_duration_for_an_estimated_plan() {
        let video: metadata::Stream = serde_json::from_value(serde_json::json!({
            "index": 0, "duration": "120", "tags": {"NUMBER_OF_FRAMES": "7"},
        }))
        .unwrap();
        assert_eq!(
            tone_map_sample_duration(true, 0.3, &video, Some(140.0)),
            Some(120.0)
        );
        assert_eq!(
            tone_map_sample_duration(false, 0.3, &video, Some(140.0)),
            Some(0.3)
        );
    }

    #[test]
    fn exact_source_boundary_requirements_disallow_header_planning_for_both_backends() {
        let changes: [fn(&mut EncodeSettings); 5] = [
            |settings| {
                settings.trim = Some(media_core::VideoTrim {
                    start_frame: 0,
                    end_frame_exclusive: 10,
                    time: None,
                })
            },
            |settings| settings.hdr_metadata = Some(Default::default()),
            |settings| {
                settings.rate_control = Some(media_core::VideoRateControl::Bitrate {
                    bitrate_kbps: 500,
                    two_pass: false,
                })
            },
            |settings| {
                settings.mov_timecode_track = Some(media_core::EncodeTrackRef {
                    input_path: None,
                    stream_index: 2,
                })
            },
            |settings| {
                settings.temporal = Some(media_core::TemporalSettings {
                    frame_rate: Some(media_core::FrameRate {
                        numerator: 30,
                        denominator: 1,
                    }),
                    ..Default::default()
                })
            },
        ];
        for backend in [
            media_core::EncodeBackend::Standalone,
            media_core::EncodeBackend::Av1an,
        ] {
            let baseline = EncodeSettings {
                backend,
                temporal: Some(media_core::TemporalSettings {
                    resize_filter: media_core::ResizeFilter::Bicubic,
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert!(header_planning_allowed(&baseline));
            for change in changes {
                let mut settings = baseline.clone();
                change(&mut settings);
                assert!(!header_planning_allowed(&settings));
            }
        }
    }

    #[test]
    fn producer_clock_diagnostics_do_not_classify_unrelated_errors() {
        let rejection = "Unrecognized option 'stats_enc_pre:0'.";
        assert_eq!(
            unsupported_producer_clock_option(&format!("[stderr] [producer] {rejection}")),
            Some("encoder-pre statistics (-stats_enc_pre)")
        );
        assert_eq!(
            unsupported_producer_clock_option(&format!("[stderr] [consumer] {rejection}")),
            None
        );
        assert_eq!(
            unsupported_producer_clock_option("[stderr] [producer] An unrelated decode error"),
            None
        );
        assert!(
            unsupported_producer_clock_option("[stderr] [producer] Invalid format directive: tbi")
                .is_some()
        );
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe and x264"]
    async fn quick_convert_rejects_interior_timing_changes_despite_constant_rate_headers() {
        // A sub-frame timestamp shift starts halfway through and ends on the
        // same frame grid. The nominal rates, count and end time all agree.
        let (fixture, output, job, _) = encode_with_audio_filter(
            VideoEncoder::X264,
            5,
            "mkv",
            "flac",
            |_| {},
            r"settb=1/1000,setpts=PTS+if(between(N\,12\,18)\,10\,0),setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        )
        .await;
        let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
        let (_owner, cancel) = watch::channel(false);
        let input = fixture.0.join("source.mkv");
        let document = probe(&ffprobe, &input, &cancel, None).await.unwrap();
        let video = &document.streams[0];
        assert_eq!(video.r_frame_rate.as_deref(), Some("24000/1001"));
        assert_eq!(video.r_frame_rate, video.avg_frame_rate);
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        assert!(
            job.logs
                .iter()
                .any(|line| line == "Preparing a compatible source clock."),
            "{job:#?}"
        );
        assert!(
            job.logs
                .iter()
                .any(|line| line.contains("Verified lossless canonical source")),
            "{job:#?}"
        );
        assert!(output.exists());
    }

    #[tokio::test]
    #[ignore = "requires the qualified AV1AN, FFmpeg, FFprobe and x264 tools"]
    async fn av1an_rejects_a_queue_clock_shorter_than_the_gapped_source() {
        let (fixture, output, job, frames) = encode_with_audio_filter(
            VideoEncoder::X264,
            5,
            "mkv",
            "flac",
            |settings| {
                settings.backend = media_core::EncodeBackend::Av1an;
                settings.av1an_options = Some(media_core::Av1anOptions {
                    chunk_method: media_core::Av1anChunkMethod::Select,
                    ..Default::default()
                });
            },
            r"setpts=PTS+gte(N\,12),setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        )
        .await;
        let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
        let (_owner, cancel) = watch::channel(false);
        let input = fixture.0.join("source.mkv");
        let document = probe(&ffprobe, &input, &cancel, None).await.unwrap();
        let video = &document.streams[0];
        assert_eq!(video.r_frame_rate, video.avg_frame_rate);
        assert_eq!(frames, 23);
        assert_eq!(job.state, JobState::Failed, "{job:#?}");
        assert_eq!(
            job.error.as_ref().unwrap().code,
            "AV1AN_SOURCE_CLOCK_INVALID",
            "{job:#?}"
        );
        assert!(
            job.logs
                .iter()
                .any(|line| line.contains("Checking the source packet timeline")),
            "{job:#?}"
        );
        assert!(!output.exists());
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, SvtAv1EncApp and x264"]
    async fn quick_convert_uses_headers_and_counts_the_actual_encode() {
        for (encoder, preset, codec, extension) in [
            (VideoEncoder::SvtAv1, 12, "av1", "mkv"),
            (VideoEncoder::X264, 5, "h264", "mkv"),
            (VideoEncoder::SvtAv1, 12, "av1", "mp4"),
            (VideoEncoder::X264, 5, "h264", "mp4"),
        ] {
            let (_fixture, output, job, frames) =
                encode_with_audio(encoder, preset, extension, "flac", |settings| {
                    settings.copy_metadata = Some(false);
                    settings.audio = vec![media_core::AudioTrackSettings {
                        stream_index: 1,
                        codec: media_core::AudioCodec::Aac,
                        bitrate_kbps: 128,
                        channels: media_core::AudioChannels::Stereo,
                        gain: None,
                        normalization: None,
                    }];
                })
                .await;
            assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
            assert!(
                job.logs
                    .iter()
                    .any(|line| line.contains(&format!("Planning approximately {frames} frames"))),
                "{:#?}",
                job.logs
            );
            assert!(
                job.logs
                    .iter()
                    .any(|line| line.contains("Actual encoder input:")),
                "{:#?}",
                job.logs
            );
            assert!(
                !job.logs
                    .iter()
                    .any(|line| line.contains("every packet timestamp")
                        || line.contains("Decoding the complete source")),
                "{:#?}",
                job.logs
            );
            assert!(
                !job.logs
                    .iter()
                    .any(|line| line.contains("Checking audio codec delay")
                        || line.contains("Checking selected audio settings")),
                "{:#?}",
                job.logs
            );
            assert_ne!(frames, 7);
            assert_eq!(
                job.logs
                    .iter()
                    .any(|line| line.contains("Checking the first second of audio")),
                extension == "mp4",
                "{:#?}",
                job.logs
            );
            assert_encoded(&output, codec, frames).await;
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires qualified FFmpeg, FFprobe, and standalone SVT-AV1-HDR"]
    async fn standalone_svt_mp4_preserves_exact_sample_aspect_ratio() {
        let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
        let (_sender, cancel) = watch::channel(false);
        // The square-pixel case has a DAR numerator above FFmpeg's -aspect
        // limit. Also exercise a non-square ratio above the -sar value limit.
        for (input_sar, expected) in [
            ("1/1", "1:1"),
            ("1001/1000", "1001:1000"),
            ("101/9", "101:9"),
        ] {
            let filter =
                format!("scale=642:256,setsar={input_sar}:max=65535,setparams=field_mode=prog");
            let (fixture, output, job, frames) = encode_with_audio_filter(
                VideoEncoder::SvtAv1Hdr,
                12,
                "mp4",
                "aac",
                |_| {},
                &filter,
            )
            .await;
            assert_eq!(job.state, JobState::Succeeded, "{input_sar}: {job:#?}");
            let source =
                probe_headers(&ffprobe, &fixture.0.join("source.mkv"), &cancel, Some(&[0]))
                    .await
                    .unwrap();
            let encoded = probe_headers(&ffprobe, &output, &cancel, Some(&[0]))
                .await
                .unwrap();
            assert_eq!(
                source.streams[0].sample_aspect_ratio.as_deref(),
                Some(expected)
            );
            assert_eq!(
                encoded.streams[0].sample_aspect_ratio.as_deref(),
                Some(expected)
            );
            assert_encoded(&output, "av1", frames).await;
        }
    }

    #[tokio::test]
    #[ignore = "requires packaged FFmpeg, FFprobe and SVT-AV1-HDR"]
    async fn quick_convert_reports_actual_svt_quality_and_depth() {
        type Customize = fn(&mut EncodeSettings);
        let cases: [(Customize, &str, &str, &str); 3] = [
            (
                |settings| {
                    settings.output_pixel_format = Some(media_core::Av1anPixelFormat::Yuv420p);
                    settings.svt_crf_quarter_steps = Some(81);
                    settings.svt_preset = Some(-1);
                },
                "8",
                "yuv420p",
                "CRF 20.25, preset -1.",
            ),
            (
                |settings| settings.lossless = true,
                "10",
                "yuv420p10le",
                "lossless, preset 9.",
            ),
            (
                |settings| {
                    settings.rate_control = Some(media_core::VideoRateControl::Bitrate {
                        bitrate_kbps: 300,
                        two_pass: false,
                    });
                },
                "10",
                "yuv420p10le",
                "target 300 kb/s, preset 9.",
            ),
        ];
        for (customize, depth, pixel_format, quality) in cases {
            let (_fixture, output, job, frames) =
                encode_with_audio(VideoEncoder::SvtAv1Hdr, 9, "mkv", "flac", customize).await;
            assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
            assert!(
                job.logs.iter().any(|line| line.contains(quality)),
                "{job:#?}"
            );
            assert!(
                job.logs.iter().any(|line| line.starts_with(&format!(
                    "Encoding {depth}-bit AV1 with the standalone SVT encoder"
                ))),
                "{job:#?}"
            );
            let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
            let (_owner, cancel) = watch::channel(false);
            let document = probe_headers(&ffprobe, &output, &cancel, Some(&[0]))
                .await
                .unwrap();
            assert_eq!(document.streams[0].pix_fmt.as_deref(), Some(pixel_format));
            assert_eq!(decoded_frames(&ffprobe, &output).await, frames);
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires packaged FFmpeg and all three Windows SVT builds"]
    async fn standalone_svt_seekable_output_reports_short_encodes() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(std::env::temp_dir().join(format!("jesses-svt-ivf-{nonce}")));
        std::fs::create_dir(&fixture.0).unwrap();
        let output = fixture.0.join("short.mkv");
        let log = fixture.0.join("short.log");
        let ffmpeg = find_executable(&["ffmpeg"]).await.unwrap().unwrap();
        let producer = CommandSpec {
            executable: ffmpeg,
            args: [
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=128x96:r=24",
                "-frames:v",
                "12",
                "-strict",
                "-1",
                "-pix_fmt",
                "yuv420p10le",
                "-f",
                "yuv4mpegpipe",
                "pipe:1",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            cwd: None,
        };
        let (_, plan) = producer_plan();
        for encoder in [
            VideoEncoder::SvtAv1,
            VideoEncoder::SvtAv1FiveFish,
            VideoEncoder::SvtAv1Hdr,
        ] {
            let executable = crate::discovery::find_video_encoder(encoder)
                .await
                .unwrap()
                .unwrap();
            let settings = EncodeSettings {
                encoder,
                preset: 9,
                ..Default::default()
            };
            let mut args = consumer_arguments(&plan, &settings);
            args.extend(["--frames".into(), "20".into()]);
            let consumer = CommandSpec {
                executable,
                args,
                cwd: None,
            };
            let video = Temporary::create_ivf(&output, "seekable").unwrap();
            let (_owner, cancel) = watch::channel(false);
            let (sender, mut events) = mpsc::channel(32);
            let drain = tokio::spawn(async move { while events.recv().await.is_some() {} });
            let result = supervisor::run_pipeline_to_file(
                &producer,
                &consumer,
                cancel,
                sender,
                &log,
                Duration::from_secs(30),
                video.clone_file().unwrap(),
            )
            .await
            .unwrap();
            drain.await.unwrap();
            assert!(
                result.producer_status.success() && result.consumer_status.success(),
                "{encoder:?}"
            );
            svt_ivf::verify(&video, 12).await.unwrap();
            let error = svt_ivf::verify(&video, 20).await.unwrap_err();
            assert_eq!(error.code, "ENCODE_VALIDATION_FAILED");
            assert!(
                error
                    .message
                    .contains("wrote 12 frames, but 20 were expected")
            );
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires packaged FFprobe and SVT-AV1-HDR, and an FFmpeg override limiting the Y4M producer to 12 frames"]
    async fn quick_convert_does_not_publish_a_short_svt_encode() {
        let (_fixture, output, job, frames) =
            encode_with_audio(VideoEncoder::SvtAv1Hdr, 9, "mkv", "flac", |settings| {
                settings.rate_control = Some(media_core::VideoRateControl::Bitrate {
                    bitrate_kbps: 300,
                    two_pass: false,
                });
            })
            .await;
        assert!(frames > 12);
        assert_eq!(job.state, JobState::Failed, "{job:#?}");
        let error = job.error.as_ref().unwrap();
        assert_eq!(error.code, "ENCODE_VALIDATION_FAILED", "{job:#?}");
        assert!(
            error
                .message
                .contains(&format!("wrote 12 frames, but {frames} were expected"))
        );
        assert!(!output.exists());
    }

    #[tokio::test]
    #[ignore = "requires real FFprobe and x264, with a native FFmpeg test override that lists no audio encoders"]
    async fn quick_convert_reports_a_missing_audio_encoder_before_video_encoding() {
        let (_fixture, output, job, _) =
            encode_with_audio(VideoEncoder::X264, 5, "mkv", "flac", |settings| {
                settings.audio = vec![media_core::AudioTrackSettings {
                    stream_index: 1,
                    codec: media_core::AudioCodec::Opus,
                    bitrate_kbps: 128,
                    channels: media_core::AudioChannels::Stereo,
                    gain: None,
                    normalization: None,
                }]
            })
            .await;
        assert_eq!(job.state, JobState::Failed, "{job:#?}");
        assert_eq!(
            job.error.as_ref().unwrap().code,
            "AUDIO_ENCODER_MISSING",
            "{job:#?}"
        );
        assert!(job.error.as_ref().unwrap().message.contains("libopus"));
        assert!(
            !job.logs.iter().any(|line| line.contains("Encoding H.264")),
            "{job:#?}"
        );
        assert!(!output.exists());
    }

    #[tokio::test]
    #[ignore = "requires real FFprobe and x264, with a native FFmpeg test override that rejects -stats_enc_pre"]
    async fn quick_convert_reports_unsupported_producer_clock_options() {
        let (_fixture, output, job, _) =
            encode_with_audio(VideoEncoder::X264, 5, "mkv", "flac", |_| {}).await;
        assert_eq!(job.state, JobState::Failed, "{job:#?}");
        assert_eq!(
            job.error.as_ref().unwrap().code,
            "SOURCE_FILTER_TOOL_UNSUPPORTED",
            "{job:#?}"
        );
        assert!(
            job.error
                .as_ref()
                .unwrap()
                .message
                .contains("-stats_enc_pre")
        );
        assert!(!output.exists());
    }

    async fn assert_encoded(output: &Path, codec: &str, frames: usize) {
        let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
        let (_sender, cancel) = watch::channel(false);
        let document = probe(&ffprobe, output, &cancel, None).await.unwrap();
        assert_eq!(document.streams[0].codec_name.as_deref(), Some(codec));
        assert_eq!(document.streams[1].codec_type.as_deref(), Some("audio"));
        assert_eq!(decoded_frames(&ffprobe, output).await, frames);
    }

    #[tokio::test]
    #[ignore = "manual real-media qualification; set JESSES_SAMPLE_INPUT to a short clip"]
    async fn encodes_external_sample() {
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
            let nonce = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let fixture = Fixture(
                std::env::temp_dir().join(format!("jesses-sample-{}-{nonce}", std::process::id())),
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
                            retired_verification: (),
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
            eprintln!(
                "{encoder:?} to {extension}: {:?} in {:.1}s",
                job.state,
                started.elapsed().as_secs_f64()
            );
            assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
            assert!(job.logs.iter().any(|line| line.starts_with(OUTPUT_CHECKED)));
        }
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe and x264"]
    async fn default_two_pass_encode_needs_no_resume_workspace() {
        fn two_pass(settings: &mut EncodeSettings) {
            settings.rate_control = Some(media_core::VideoRateControl::Bitrate {
                bitrate_kbps: 300,
                two_pass: true,
            });
        }
        let (_fixture, _output, job, _frames) =
            encode_with_audio(VideoEncoder::X264, 5, "mkv", "aac", two_pass).await;
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        assert!(job.standalone_recovery.is_none(), "{job:#?}");
        assert!(
            !job.logs
                .iter()
                .any(|line| line.starts_with("Fingerprinting")),
            "{:#?}",
            job.logs
        );
    }

    /// Encodes one second of video with sound, a SubRip track converted to MP4
    /// text and two chapters, so FFmpeg writes MP4/MOV staging with a chapter
    /// track that reads back as a trailing data stream.
    async fn encode_text_with_chapters(
        encoder: VideoEncoder,
        preset: u8,
        extension: &str,
    ) -> (Fixture, PathBuf, JobSnapshot) {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(
            std::env::temp_dir().join(format!("jesses-chapters-{}-{nonce}", std::process::id())),
        );
        std::fs::create_dir(&fixture.0).unwrap();
        let subtitles = fixture.0.join("cues.srt");
        std::fs::write(&subtitles, "1\n00:00:00,100 --> 00:00:00,600\nHello\n").unwrap();
        let chapters = fixture.0.join("chapters.txt");
        std::fs::write(
            &chapters,
            ";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=500\ntitle=One\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=500\nEND=1000\ntitle=Two\n",
        )
        .unwrap();
        let input = fixture.0.join("source.mkv");
        let output = fixture.0.join(format!("output.{extension}"));
        let ffmpeg = find_executable(&["ffmpeg"]).await.unwrap().unwrap();
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
                    "-i",
                ]
                .into_iter()
                .map(OsString::from)
                .chain([subtitles.as_os_str().to_owned()])
                .chain(["-f", "ffmetadata", "-i"].map(OsString::from))
                .chain([chapters.as_os_str().to_owned()])
                .chain(
                    [
                        "-t",
                        "1",
                        "-map",
                        "0",
                        "-map",
                        "1",
                        "-map",
                        "2",
                        "-map_chapters",
                        "3",
                        "-c:v",
                        "ffv1",
                        "-pix_fmt",
                        "yuv420p",
                        "-ac",
                        "2",
                        "-c:a",
                        "aac",
                        "-c:s",
                        "srt",
                    ]
                    .map(OsString::from),
                )
                .chain([input.as_os_str().to_owned()])
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
        let manager = JobManager::new(fixture.0.join("logs"));
        manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0, 1, 2],
                },
                settings: EncodeSettings {
                    encoder,
                    preset,
                    subtitles: vec![media_core::SubtitleTrackSettings {
                        stream_index: 2,
                        mode: media_core::SubtitleMode::MovText,
                        burn_in: false,
                    }],
                    encoding_preferences: Some(media_core::EncodingPreferences {
                        keyframe_interval_seconds: 10,
                        mp4_faststart: true,
                        retired_verification: (),
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
        (fixture, output, job)
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, SvtAv1EncApp and x264"]
    async fn default_mp4_and_mov_outputs_with_chapters_pass_the_header_check() {
        // AV1 needs MP4 staging; H.264 is staged in MOV.
        for (encoder, preset, extension) in [
            (VideoEncoder::SvtAv1, 12, "mp4"),
            (VideoEncoder::X264, 5, "mov"),
        ] {
            let (_fixture, output, job) =
                encode_text_with_chapters(encoder, preset, extension).await;
            assert_eq!(job.state, JobState::Succeeded, "{extension}: {job:#?}");
            assert_eq!(
                job.logs
                    .iter()
                    .filter(|line| line.starts_with(OUTPUT_CHECKED))
                    .count(),
                1,
                "{:#?}",
                job.logs
            );
            let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
            let (_sender, cancel) = watch::channel(false);
            let document = probe(&ffprobe, &output, &cancel, None).await.unwrap();
            assert_eq!(document.chapters.len(), 2, "{extension}");
            assert_eq!(
                document.streams[2].codec_name.as_deref(),
                Some("mov_text"),
                "{extension}"
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, mkvmerge, SvtAv1EncApp, x265, and vpxenc"]
    async fn default_encode_checks_finished_stream_headers() {
        fn plain(_: &mut EncodeSettings) {}
        // A filter that changes picture size: the plan takes its size and
        // frame count from the filtered producer.
        fn smaller(settings: &mut EncodeSettings) {
            settings.source_filters = true;
            settings.av1an_filters = vec!["scale=64:48".into()];
        }
        // Encoder, preset, extension, audio, output codec, settings, width.
        type Case = (
            VideoEncoder,
            u8,
            &'static str,
            &'static str,
            &'static str,
            fn(&mut EncodeSettings),
            Option<u32>,
        );
        let cases: [Case; 4] = [
            (VideoEncoder::SvtAv1, 12, "mkv", "aac", "av1", plain, None),
            (
                VideoEncoder::X265Standalone,
                0,
                "mp4",
                "aac",
                "hevc",
                plain,
                None,
            ),
            (
                VideoEncoder::VpxStandalone,
                8,
                "webm",
                "libopus",
                "vp9",
                plain,
                None,
            ),
            (
                VideoEncoder::SvtAv1,
                12,
                "mkv",
                "aac",
                "av1",
                smaller,
                Some(64),
            ),
        ];
        for (encoder, preset, extension, audio, codec, customize, width) in cases {
            let (_fixture, output, job, frames) =
                encode_with_audio(encoder, preset, extension, audio, customize).await;
            assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
            assert_eq!(
                job.logs
                    .iter()
                    .filter(|line| line.starts_with(OUTPUT_CHECKED))
                    .count(),
                1,
                "{:#?}",
                job.logs
            );
            assert!(job.standalone_recovery.is_none(), "{job:#?}");
            assert!(
                !job.logs
                    .iter()
                    .any(|line| line.starts_with("Fingerprinting")),
                "{:#?}",
                job.logs
            );
            if let Some(width) = width {
                let ffprobe = find_executable(&["ffprobe"]).await.unwrap().unwrap();
                let (_sender, cancel) = watch::channel(false);
                let document = probe(&ffprobe, &output, &cancel, None).await.unwrap();
                assert_eq!(document.streams[0].width, Some(width));
            }
            assert!(
                job.logs
                    .iter()
                    .any(|line| line.starts_with("Planning approximately ")
                        && line.contains("from source headers")),
                "{:#?}",
                job.logs
            );
            if extension != "mp4" {
                assert!(
                    !job.logs
                        .iter()
                        .any(|line| line.contains("to verify its complete sample timeline")),
                    "{:#?}",
                    job.logs
                );
            }
            assert_encoded(&output, codec, frames).await;
        }
    }
}
