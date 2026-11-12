//! Chunked encoding under the same process ownership and artifact gates.
use super::{encode::encoder_parameters, encode_plan::Plan, *};

mod filters;
mod grain;
mod launcher;
pub(super) use filters::rows as filter_rows;
pub(super) use filters::validate as validate_filters;
pub(super) use grain::stage_standalone as stage_standalone_grain_table;
pub(super) use grain::validate as validate_grain;
pub(super) use grain::{
    TABLE_NAME as GRAIN_TABLE_NAME, aom_parameters as aom_grain_parameters,
    parameters as grain_parameters,
};
pub(super) mod encoder;
mod metrics;
mod options;
mod scenes;
pub(super) mod source_queue_concat;
pub(super) use options::source_resamples;
pub(super) use options::validate_settings;
mod recovery;
mod recovery_receipts;
pub(super) use recovery::{
    CommandRecipe, OwnedRecovery, PreparedSource, Recovery, restore_interrupted_source_cut,
};

/// Admit the joined video for either a fresh encode or Saved finalization.
/// Only detailed mode rebuilds decoded regions and an independent join clock.
#[allow(clippy::too_many_arguments)]
pub(in crate::jobs) async fn verify_source_join(
    manager: &JobManager,
    id: &str,
    ffmpeg: &Path,
    ffprobe: &Path,
    mkvmerge: Option<&Path>,
    recovery: &Recovery,
    video: &Temporary,
    plan: &mut Plan,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<usize>, AppError> {
    let Some(mode) = recovery
        .source_join_mode(plan.requires_source_queue_clock_proof())
        .await?
    else {
        return Ok(None);
    };
    let reason = match mode {
        recovery::SourceJoinMode::Basic {
            frames,
            rate,
            method,
        } => {
            manager
                .phase(
                    id,
                    JobState::Finalizing,
                    "Checking the joined AV1AN video header.",
                )
                .await;
            let document = super::probe_headers(ffprobe, &video.path, cancel, Some(&[0])).await?;
            source_queue_concat::joined_format(&document, &video.path, method)?;
            let stream = source_queue_concat::only_video(&document, &video.path)?;
            plan.observe_source_queue_header(&stream, rate)?;
            return Ok(Some(frames));
        }
        recovery::SourceJoinMode::Detailed { reason } => reason,
    };
    manager
        .change(id, |snapshot| append_log(snapshot, reason.into()))
        .await;
    manager
        .phase(
            id,
            JobState::Finalizing,
            "Checking AV1AN source, tools and saved chunk fingerprints.",
        )
        .await;
    super::encode::boxed(|| {
        recovery.seal_source_chunks(cancel, |verified, total| async move {
            manager
                .phase(
                    id,
                    JobState::Finalizing,
                    &format!("Verifying AV1AN chunks ({verified}/{total} decoded)."),
                )
                .await;
        })
    })
    .await?;
    manager
        .phase(
            id,
            JobState::Finalizing,
            "Checking the joined AV1AN video against the sealed chunk queue.",
        )
        .await;
    let Some(join) = recovery.source_join_inputs(cancel).await? else {
        return Ok(None);
    };
    let (header, clock) = super::encode::boxed(|| {
        source_queue_concat::verify_with_clock(
            ffmpeg,
            ffprobe,
            mkvmerge,
            video,
            &join.chunks,
            join.method,
            join.source_rate,
            join.force_rate,
            cancel,
        )
    })
    .await?;
    if plan.has_source_filters() {
        plan.observe_source_filter_stream(&header, None)?;
    }
    let regions = join.regions.with_reference_header(&header, clock)?;
    plan.source_queue_regions(regions, join.rate);
    Ok(plan.source_queue_frame_count())
}

pub(super) fn validate_encoder_path(
    encoder: &Path,
    settings: &EncodeSettings,
) -> Result<(), AppError> {
    launcher::validate_encoder_path(encoder, settings.encoder)
}

pub(super) fn validate_input(document: &Document, plan: &Plan) -> Result<(), AppError> {
    if document
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .is_none_or(|s| s.index != plan.video_index)
    {
        return Err(AppError::new(
            "ENCODE_INPUT_UNSUPPORTED",
            "av1an currently encodes the first video track. Choose that track or use standalone SVT-AV1.",
            None,
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn arguments(
    input: &Path,
    output: &Path,
    work: &Path,
    log: &Path,
    plan: &Plan,
    settings: &EncodeSettings,
    resume: bool,
    source_filter: Option<&str>,
) -> Vec<OsString> {
    // Only planner-owned scalars enter av1an's nested encoder argument string.
    // User paths are always separate native arguments, never shell text.
    let params = encoder::parameters(plan, settings)
        .iter()
        .map(|v| v.to_str().expect("ASCII encoder parameters"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut args: Vec<OsString> = [
        "--encoder",
        encoder::name(settings.encoder),
        "--passes",
        "1",
        "--no-defaults",
        "--concat",
        // av1an's av-ivf concatenator can panic on valid small AV1 packets.
        // FFmpeg copies the IVF chunks; our complete record and decode checks
        // still verify the resulting stream before publication.
        if encoder::uses_mkvmerge(settings.encoder, settings) {
            "mkvmerge"
        } else {
            "ffmpeg"
        },
        "--pix-format",
        plan.output_pixel_format,
        "--cache-mode",
        "temp",
        "--max-tries",
        // The value is bounded before it reaches the nested av1an CLI.
        // A scalar is appended below because this fixed array contains &str.
        "3",
        "--keep",
        "--verbose",
        "--log-level",
        if settings
            .av1an_options
            .is_some_and(|options| options.target_quality.is_some())
        {
            "debug"
        } else {
            "info"
        },
        "-y",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    if let Some(index) = args.iter().position(|arg| arg == "--max-tries") {
        args[index + 1] = settings
            .av1an_options
            .unwrap_or_default()
            .max_tries
            .to_string()
            .into();
    }
    options::append(&mut args, settings, plan, &params);
    if options::source_resamples(settings, plan.source_av1an_input_rate()) {
        args.push("--ignore-frame-mismatch".into());
    }
    if resume {
        args.push("--resume".into());
    }
    if let Some(filter) = source_filter {
        // The generated filter contains only planner-owned labels, integers,
        // and fixed FFmpeg options. Paths never enter av1an's nested parser.
        let nested = if settings.source_filters {
            format!("-vf '{}'", filter.replace('\'', "'\\''"))
        } else {
            format!("-vf {filter}")
        };
        args.extend(["--ffmpeg".into(), nested.into()]);
        if settings.av1an_options.is_some_and(|options| {
            options.target_quality.is_some() && !options.source_target_defaults
        }) {
            // Target probes encode through --ffmpeg. Score the reference after
            // the same deterministic processing so the search and final chunk
            // compare corresponding pixels, timing, color and geometry.
            args.extend(["--vmaf-filter".into(), filter.into()]);
        }
    }
    for (key, value) in [
        ("-i", input.as_os_str().to_owned()),
        ("-o", output.as_os_str().to_owned()),
        ("--temp", work.join("chunks").into_os_string()),
        ("--log-file", log.as_os_str().to_owned()),
        ("--workers", settings.workers.to_string().into()),
        ("--video-params", params.into()),
        // Exclude attachments as well as audio, subtitles and data. The
        // packaged av1an recognizes this exact sequence as video-only and
        // skips creating or reading audio.mkv.
        ("--audio-params", "-an -sn -dn -map -0:t?".into()),
    ] {
        args.extend([key.into(), value]);
    }
    args
}

impl JobManager {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn encode_av1an(
        &self,
        id: &str,
        input: &Path,
        encoder: &Path,
        executable: &Path,
        ffmpeg: &Path,
        ffprobe: &Path,
        mkvmerge: Option<&Path>,
        recovery: &Recovery,
        video: &Temporary,
        plan: &mut Plan,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        frame_count: usize,
        dynamic_hdr: &super::dynamic_hdr::Resources,
        post_mux: Option<CommandSpec>,
    ) -> Result<bool, AppError> {
        validate_encoder_path(encoder, settings)?;
        if plan.is_hdr10()
            && settings
                .av1an_options
                .is_some_and(|options| options.target_quality.is_some())
        {
            return Err(AppError::new(
                "ENCODE_INPUT_UNSUPPORTED",
                "Quality targeting currently requires SDR input. These metric pipelines are not qualified for preserved HDR output.",
                None,
            ));
        }
        let work = recovery.root.clone();
        let source_filter = recovery.source_filter()?;
        // A crashed staged host remains in its owned attempt directory. Never
        // replace it or let it shadow the encoder selected for this attempt.
        let launch_directory = work.join(format!(
            "launch-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let reservation_guard = recovery.reserve_launch(&launch_directory).await?;
        let launch_executable = executable.to_owned();
        let launch_encoder = encoder.to_owned();
        let launch_work = launch_directory.clone();
        let launch_ffmpeg = ffmpeg.to_owned();
        let launch_ffprobe = ffprobe.to_owned();
        let launch_mkvmerge = mkvmerge.map(Path::to_owned);
        let launch_family = settings.encoder;
        let mut launch = tokio::task::spawn_blocking(move || {
            let _reservation_guard = reservation_guard;
            launcher::Launch::prepare(
                &launch_executable,
                &launch_encoder,
                launch_family,
                &launch_ffmpeg,
                &launch_ffprobe,
                launch_mkvmerge.as_deref(),
                &launch_work,
            )
        })
        .await
        .map_err(|e| files::error("AV1AN_STAGE_FAILED", e.to_string(), executable))??;
        let staged_image = cfg!(windows).then_some(launch.executable.as_path());
        if let Err(error) = recovery.seal_launch(&launch_directory, staged_image).await {
            launch.retain_unsealed();
            return Err(error);
        }
        check_cancel(cancel)?;
        let version = supervisor::run_capture_with_environment(
            &CommandSpec {
                executable: launch.executable.clone(),
                args: vec!["--version".into()],
                cwd: Some(work.clone()),
            },
            cancel.clone(),
            128 * 1024,
            Duration::from_secs(15),
            Some(&launch.environment),
        )
        .await
        .map_err(|e| process_error(e, executable))?;
        if !version.status.success() {
            return Err(files::error(
                "TOOL_FAILED",
                "av1an failed its version check.",
                executable,
            ));
        }
        let version_text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&version.stdout),
            String::from_utf8_lossy(&version.stderr)
        );
        let mut configured = options::effective_chunks(
            settings,
            plan.source_chunk_rate(),
            plan.frame_dimensions(false).1,
        );
        configured.target_quality =
            options::effective_target(configured, settings.encoder, plan.frame_dimensions(false));
        options::validate_plugin(
            &version_text,
            configured,
            source_filter.is_some() && !configured.source_target_defaults,
        )
        .map_err(|message| files::error("AV1AN_DEPENDENCY_MISSING", message, executable))?;
        let previous_scorer = recovery.scorer_identity().await?;
        let scorer_selection = if let Some(target) = configured.target_quality {
            metrics::select_bundled_vship(
                executable,
                &launch.executable,
                &mut launch.environment,
                &work,
                target,
                previous_scorer.as_ref(),
                cancel,
            )
            .await?
        } else {
            None
        };
        options::capabilities(
            &launch.executable,
            ffmpeg,
            &launch.environment,
            &work,
            settings,
            cancel,
        )
        .await?;
        let _grain_table = grain::stage(&work, settings)?;
        let _hdr_resources = dynamic_hdr.stage_av1an(&work)?;
        let version = version_text
            .lines()
            .find(|v| !v.is_empty())
            .unwrap_or("unknown av1an version")
            .to_owned();
        // A resumed target search must keep the same scorer implementation.
        // The GPU plugin's verified bytes are part of the recovery identity;
        // CPU jobs retain the prior version identity and remain resumable.
        let recovery_version = scorer_selection
            .as_ref()
            .and_then(|selection| selection.gpu_sha256.as_deref())
            .map_or_else(
                || version_text.clone(),
                |sha256| recovery::version_with_vship(&version_text, sha256),
            );
        recovery.version(recovery_version).await?;
        recovery.verify_segments(ffmpeg, input, cancel).await?;
        let prepared_scenes = if recovery.resume_chunks {
            None
        } else {
            match scenes::prepare(
                &launch.executable,
                &launch.environment,
                input,
                &launch_directory,
                &work.join("chunks"),
                frame_count,
                configured,
                cancel,
            )
            .await
            {
                Ok(prepared) => prepared,
                Err(error) => {
                    check_cancel(cancel)?;
                    self.change(id, |snapshot| append_log(snapshot, format!("Parallel scene prepass was unavailable ({error}); av1an will detect scenes during encoding."))).await;
                    None
                }
            }
        };
        let internal_log = log_path.with_extension("av1an.log");
        self.change(id, |snapshot| {
            append_log(snapshot, format!("Tool: {} — {version}", executable.display()));
            append_log(snapshot, format!("av1an selected encoder: {}", encoder.display()));
            append_log(snapshot, format!("av1an: {} parallel workers; {} source chunks, {:?} splitting, maximum {} frames (0 means unlimited). Workspace: {}", settings.workers, options::chunk_method(configured), configured.split_method, configured.maximum_chunk_frames, work.display()));
            if let Some(target) = configured.target_quality {
                if let Some(selection) = &scorer_selection {
                    append_log(snapshot, format!("Quality scorer: {}.", selection.detail));
                }
                if configured.source_target_defaults {
                    append_log(snapshot,format!("Source target defaults: {} scalar target {:.1}; encoder search {}–{}, probes4, sampling1, probe resolution=input {}×{}, metric resolution={}×{}, Automatic statistic, built-in encoder probe parameters. Hidden draft values are ignored.",metrics::label(target.metric),f64::from(target.minimum_score_tenths)/10.0,target.minimum_crf,target.maximum_crf,plan.frame_dimensions(false).0,plan.frame_dimensions(false).1,target.probe_width,target.probe_height));
                    if source_filter.is_some(){append_log(snapshot,"Source target probes read the unfiltered input; final chunk filters are intentionally omitted from probe/reference arguments, matching the source application.".into());}
                } else {
                append_log(snapshot, format!("{} target {:.1}–{:.1}, mean score at {}×{}, CRF {}–{}, at most {} probes, sampling every {} frame(s). A search may finish outside the requested score range when its bounds/probes are exhausted.", metrics::label(target.metric), f64::from(target.minimum_score_tenths)/10.0, f64::from(target.maximum_score_tenths)/10.0, target.probe_width, target.probe_height, target.minimum_crf, target.maximum_crf, target.probes, target.probing_rate));
                if source_filter.is_some() { append_log(snapshot, "Quality target: probe encodes and reference scoring use the same validated trim, temporal, tone-map, framing, and aspect-ratio filter chain as final chunks.".into()); }
                else { append_log(snapshot, "Quality target: probe encodes and reference scoring read the same verified lossless processed source as final chunks.".into()); }
                }
                if target.probing_rate > 1 { append_log(snapshot, "Quality target warning: sampled-frame scores may differ from scoring every frame; the target is not a full-output quality measurement.".into()); }
            }
            if let Some(prepared) = &prepared_scenes {
                append_log(snapshot, format!("Parallel scene prepass: {} slices produced {} scenes.", prepared.slices, prepared.scenes));
            }
            append_log(snapshot, format!("av1an detail log: {}", internal_log.display()));
            snapshot.log_path = Some(log_path.to_string_lossy().into_owned());
        }).await;
        let (sender, mut events) = mpsc::channel(256);
        let observer = self.clone();
        let event_id = id.to_owned();
        let event_recovery = recovery.clone();
        let progress_path = work.join("chunks/done.json");
        let frame_seconds = plan.frame_seconds();
        let event_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            let mut previous = 0;
            loop {
                tokio::select! {
                    event = events.recv() => {
                        let Some(ProcessEvent::Stdout(line) | ProcessEvent::Stderr(line)) = event else { break };
                        observer.change(&event_id, |snapshot| append_log(snapshot, line)).await;
                    }
                    _ = interval.tick() => {
                        if let Ok(summary) = event_recovery.checkpoint(false).await {
                            let reader_cache = event_recovery.reader_cache_path().await.ok().flatten();
                            observer.change(&event_id, move |snapshot| {
                                snapshot.recovery = Some(summary);
                                if let Some(path) = reader_cache {
                                    report_reader_cache(snapshot, &path);
                                }
                            }).await;
                        }
                        if let Some((frames, chunks)) = read_progress(&progress_path, frame_count).await
                            && frames > previous {
                                previous = frames;
                                observer.change(&event_id, |snapshot| {
                                    snapshot.progress_seconds = Some(frames as f64 * frame_seconds);
                                    append_log(snapshot, format!("av1an: {chunks} chunks completed; {frames}/{frame_count} frames."));
                                }).await;
                        }
                    }
                }
            }
        });
        let pause = self
            .state
            .lock()
            .await
            .entries
            .iter()
            .find(|entry| entry.snapshot.id == id)
            .expect("registered av1an job")
            .pause
            .clone();
        // mkvmerge refuses an existing destination. It writes to this fresh
        // launch-owned path; after a successful process and receipt checkpoint,
        // copy the result into the identity-guarded durable video reservation.
        let staged_video = encoder::uses_mkvmerge(settings.encoder, settings)
            .then(|| launch_directory.join("encoded.mkv"));
        let av1an_output = staged_video.as_deref().unwrap_or(&video.path);
        let mut args = arguments(
            input,
            av1an_output,
            &work,
            &internal_log,
            plan,
            settings,
            recovery.resume_chunks,
            source_filter.as_deref(),
        );
        if let Some(prepared) = &prepared_scenes {
            args.extend(["--scenes".into(), prepared.path.as_os_str().to_owned()]);
        }
        let native_command = CommandSpec {
            executable: launch.executable.clone(),
            args,
            cwd: Some(work.clone()),
        };
        let mut generated = super::command_edit::format_command(&native_command);
        if let Some(mut mux) = post_mux {
            for argument in &mut mux.args {
                if argument == video.path.as_os_str() {
                    *argument = av1an_output.as_os_str().to_owned();
                }
            }
            generated.push_str(" &&\n");
            generated.push_str(&super::command_edit::format_command(&mux));
        }
        let saved = recovery.edited_command().await?;
        let output = self
            .list_jobs()
            .await
            .into_iter()
            .find(|job| job.id == id)
            .map(|job| PathBuf::from(job.request.output_path))
            .ok_or_else(|| files::error("JOB_NOT_FOUND", "The edited job disappeared.", input))?;
        let initial = saved
            .as_ref()
            .map(|saved| {
                super::command_edit::rebind::rebind(
                    saved,
                    &generated,
                    &work,
                    &output,
                    id,
                    recovery.resume_chunks,
                )
            })
            .transpose()?
            .unwrap_or_else(|| generated.clone());
        let reviewed = self
            .review_command_initial(id, "av1an", &generated, &initial, &output, cancel)
            .await?;
        let command = reviewed.or_else(|| {
            saved.map(|_| media_core::EditedCommand {
                generated: generated.clone(),
                arguments: initial,
            })
        });
        let edited = command
            .as_ref()
            .is_some_and(|command| command.arguments != generated.trim());
        if let Some(command) = &command {
            recovery
                .confirm_edited_command(edited.then(|| command.clone()))
                .await?;
            self.change(id, |snapshot| {
                snapshot.edited_command = edited.then(|| command.clone())
            })
            .await;
            let mut state = self.state.lock().await;
            self.persist(&mut state).await?;
        }
        if command.is_none() {
            recovery.commit_source_cut_recipe().await?;
        }
        let shell = edited
            .then(|| {
                super::command_edit::PreparedShell::new(
                    &command.as_ref().expect("confirmed command").arguments,
                    Some(work.clone()),
                    log_path,
                    Some(&launch.environment),
                )
            })
            .transpose()?;
        let execution = shell
            .as_ref()
            .map_or(&native_command, |shell| &shell.command);
        let environment = shell
            .as_ref()
            .map_or(&launch.environment, |shell| &shell.environment);
        let result = supervisor::with_console_profile(supervisor::ConsoleProfile::Av1an, || {
            supervisor::run_with_environment(
                execution,
                cancel.clone(),
                sender,
                log_path,
                Duration::from_secs(24 * 60 * 60),
                Some(environment),
                Some(&pause),
            )
        })
        .await;
        let _ = event_task.await;
        if result.as_ref().is_ok_and(|result| result.status.success()) {
            self.phase_with_fresh_progress(
                id,
                JobState::Finalizing,
                "Verifying the completed AV1AN encode stage.",
            )
            .await;
        }
        recovery.seal_target_artifacts().await?;
        let stage_cleanup = launch.cleanup();
        // The image guard is released by cleanup; release the directory guard
        // before Windows removes the exact now-empty launch directory below.
        drop(launch);
        drop(prepared_scenes);
        // The process has exited, so seal its last complete receipts even when
        // cancellation won. A crash before this point reuses only prior receipts.
        // Keep the launch directory through this check: mkvmerge's options.json
        // names its output there, including for an edited program.
        let checkpoint = recovery
            .checkpoint(!edited && result.as_ref().is_ok_and(|result| result.status.success()))
            .await;
        if edited {
            if let Some(staged) = &staged_video
                && staged.exists()
            {
                let mut owned = Temporary::durable(staged, true)?;
                owned.cleanup()?;
            }
            recovery.retire_launch(&launch_directory).await?;
        }
        match checkpoint {
            Ok(summary) => {
                let reader_cache = recovery.reader_cache_path().await?;
                self.change(id, move |snapshot| {
                    snapshot.recovery = Some(summary);
                    if let Some(path) = reader_cache {
                        report_reader_cache(snapshot, &path);
                    }
                })
                .await
            }
            Err(error) => return Err(error),
        }
        let result = result.map_err(|e| process_error(e, input))?;
        if result.status.success()
            && let Some(shell) = &shell
        {
            shell.verify()?;
        }
        stage_cleanup?;
        if !result.status.success() {
            return Err(files::error(
                "AV1AN_FAILED",
                format!(
                    "av1an failed ({}). See the job log for the failed chunk or missing dependency.",
                    result.status
                ),
                input,
            ));
        }
        if edited {
            return Ok(true);
        }
        if let Some(staged) = &staged_video {
            let staged = staged.clone();
            let mut destination = video.clone_file()?;
            tokio::task::spawn_blocking(move || -> Result<(), AppError> {
                use std::io::{Seek, SeekFrom, Write};
                let guard = Source::open(&staged)?;
                let mut input = std::fs::File::open(&guard.path).map_err(|e| {
                    files::error("ENCODE_VALIDATION_FAILED", e.to_string(), &staged)
                })?;
                destination
                    .set_len(0)
                    .and_then(|_| destination.seek(SeekFrom::Start(0)).map(|_| ()))
                    .map_err(|e| {
                        files::error("ENCODE_VALIDATION_FAILED", e.to_string(), &staged)
                    })?;
                std::io::copy(&mut input, &mut destination)
                    .and_then(|_| destination.flush())
                    .and_then(|_| destination.sync_all())
                    .map_err(|e| {
                        files::error("ENCODE_VALIDATION_FAILED", e.to_string(), &staged)
                    })?;
                guard.verify()?;
                Ok(())
            })
            .await
            .map_err(|e| {
                files::error(
                    "ENCODE_VALIDATION_FAILED",
                    e.to_string(),
                    staged_video.as_ref().unwrap(),
                )
            })??;
            let _ = tokio::fs::remove_file(staged_video.as_ref().unwrap()).await;
        }
        recovery.retire_launch(&launch_directory).await?;
        // Decoding the joined segments against the source only re-checks
        // av1an's result; the finished file's frame count is checked anyway.
        if super::encode::thorough_verification(settings) {
            if matches!(
                configured.chunk_method,
                media_core::Av1anChunkMethod::Hybrid | media_core::Av1anChunkMethod::Segment
            ) {
                self.change(id, |snapshot| {
                    append_log(
                        snapshot,
                        "Verifying the complete decoded segment sequence against the original source."
                            .into(),
                    )
                })
                .await;
            }
            recovery.verify_segments(ffmpeg, input, cancel).await?;
        }
        video.flush_nonempty_async().await?;
        if settings
            .av1an_options
            .is_some_and(|options| options.source_workflow)
        {
            verify_source_join(
                self, id, ffmpeg, ffprobe, mkvmerge, recovery, video, plan, cancel,
            )
            .await?;
        } else if settings.source_filters {
            // Source graphs run inside each engine chunk. The queue/done
            // receipts and engine FRAME MISMATCH gate retain frame authority;
            // only the actual coded geometry/SAR is learned here, never by
            // replaying arbitrary filters over the whole source.
            let actual = super::probe_headers(ffprobe, &video.path, cancel, Some(&[0])).await?;
            let stream = actual.streams.first().ok_or_else(|| {
                files::error(
                    "ENCODE_VALIDATION_FAILED",
                    "The filtered AV1AN output has no video stream.",
                    &video.path,
                )
            })?;
            plan.observe_source_filter_stream(stream, None)?;
        }
        if encoder::video_extension(settings.encoder, settings) == "ivf"
            && !settings
                .av1an_options
                .is_some_and(|options| options.source_workflow)
        {
            let mut file = video.clone_file()?;
            let geometry = (plan.width, plan.height);
            let rate = (plan.fps_num, plan.fps_den);
            let canceled = cancel.clone();
            let normalized = tokio::task::spawn_blocking(move || {
                normalize_ivf(&mut file, geometry, rate, frame_count, &canceled)
            })
            .await;
            check_cancel(cancel)?;
            let changed = normalized
                .map_err(|e| files::error("ENCODE_VALIDATION_FAILED", e.to_string(), &video.path))?
                .map_err(|e| {
                    files::error("ENCODE_VALIDATION_FAILED", e.to_string(), &video.path)
                })?;
            if changed {
                self.change(id, |snapshot| append_log(snapshot, format!("Corrected av1an IVF rate to {}/{} after checking all {frame_count} sequential frame records.", rate.0, rate.1))).await;
            }
        }
        check_cancel(cancel)?;
        Ok(false)
    }
}

#[derive(serde::Deserialize)]
struct Completed {
    frames: u64,
}
#[derive(serde::Deserialize)]
struct Progress {
    frames: u64,
    done: std::collections::BTreeMap<String, Completed>,
}

fn progress(bytes: &[u8], expected: usize) -> Option<(u64, usize)> {
    let state: Progress = serde_json::from_slice(bytes).ok()?;
    if state.frames != expected as u64 {
        return None;
    }
    let count = state
        .done
        .values()
        .try_fold(0u64, |sum, chunk| sum.checked_add(chunk.frames))?;
    (count <= state.frames).then_some((count, state.done.len()))
}

async fn read_progress(path: &Path, expected: usize) -> Option<(u64, usize)> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    if bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    progress(&bytes, expected)
}

fn report_reader_cache(snapshot: &mut JobSnapshot, path: &Path) {
    if snapshot
        .logs
        .iter()
        .any(|line| line.starts_with("av1an external reader cache:"))
    {
        return;
    }
    append_log(
        snapshot,
        format!(
            "av1an external reader cache: {} (owned by the recovery workspace; retained only while recovery is available).",
            path.display()
        ),
    );
}

/// av1an's experimental IVF concatenator can write a fixed 30 fps header.
/// Correct only a structurally verified owned AV1 stream whose frame records
/// have the exact sequential timestamps/count already established by preflight.
fn normalize_ivf(
    file: &mut std::fs::File,
    geometry: (u32, u32),
    rate: (u32, u32),
    expected: usize,
    cancel: &watch::Receiver<bool>,
) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "av1an IVF frame structure does not match the validated source",
        )
    };
    file.seek(SeekFrom::Start(0))?;
    let length = file.metadata()?.len();
    let mut header = [0u8; 32];
    file.read_exact(&mut header)?;
    let u16_at = |offset| u16::from_le_bytes(header[offset..offset + 2].try_into().unwrap());
    if &header[..4] != b"DKIF"
        || u16_at(4) != 0
        || u16_at(6) != 32
        || &header[8..12] != b"AV01"
        || (u32::from(u16_at(12)), u32::from(u16_at(14))) != geometry
    {
        return Err(invalid());
    }
    let mut offset = 32u64;
    for index in 0..expected {
        if *cancel.borrow() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Job canceled during IVF verification",
            ));
        }
        let mut frame = [0u8; 12];
        file.read_exact(&mut frame)?;
        let size = u32::from_le_bytes(frame[..4].try_into().unwrap());
        let timestamp = u64::from_le_bytes(frame[4..].try_into().unwrap());
        offset = offset
            .checked_add(12 + u64::from(size))
            .ok_or_else(invalid)?;
        if size == 0 || timestamp != index as u64 || offset > length {
            return Err(invalid());
        }
        file.seek(SeekFrom::Start(offset))?;
    }
    if offset != length {
        return Err(invalid());
    }
    let frame_count = u32::try_from(expected).map_err(|_| invalid())?;
    let changed = header[16..20] != rate.0.to_le_bytes()
        || header[20..24] != rate.1.to_le_bytes()
        || header[24..28] != frame_count.to_le_bytes();
    if changed {
        file.seek(SeekFrom::Start(16))?;
        file.write_all(&rate.0.to_le_bytes())?;
        file.write_all(&rate.1.to_le_bytes())?;
        file.write_all(&frame_count.to_le_bytes())?;
        file.sync_all()?;
    }
    Ok(changed)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use media_core::VideoEncoder;

    thread_local! {
        pub(in crate::jobs) static CHUNK_PROOF_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        pub(super) static REFERENCE_JOIN_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[tokio::test]
    #[ignore = "requires packaged av1an, FFmpeg, FFprobe, SVT-AV1-HDR and mkvmerge"]
    async fn basic_source_workflow_never_enters_either_detailed_proof() {
        let resources =
            std::env::var_os("JESSES_TEST_TOOL_RESOURCES").expect("set bundled resources");
        crate::configure_bundled_tools(resources.into()).unwrap();
        let root = std::env::temp_dir().join(format!(
            "jesses-source-proof-calls-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let input = root.join("source.mkv");
        let (_owner, cancel) = watch::channel(false);
        let ffmpeg = super::super::discover("ffmpeg", &cancel).await.unwrap();
        let source = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg,
                args: [
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=96x64:rate=24:duration=1",
                    "-c:v",
                    "ffv1",
                    "-color_primaries",
                    "bt709",
                    "-color_trc",
                    "bt709",
                    "-colorspace",
                    "bt709",
                    "-color_range",
                    "tv",
                    "-chroma_sample_location",
                    "left",
                ]
                .into_iter()
                .map(OsString::from)
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
            source.status.success(),
            "{}",
            String::from_utf8_lossy(&source.stderr)
        );
        let manager = JobManager::new(root.join("logs"));
        for (thorough, resample) in [(false, false), (true, false), (false, true)] {
            CHUNK_PROOF_CALLS.set(0);
            REFERENCE_JOIN_CALLS.set(0);
            let request = EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: root
                        .join(format!("{thorough}-{resample}.mkv"))
                        .to_string_lossy()
                        .into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    backend: media_core::EncodeBackend::Av1an,
                    encoder: VideoEncoder::SvtAv1Hdr,
                    workers: 1,
                    preset: 12,
                    crf: 40,
                    av1an_options: Some(media_core::Av1anOptions {
                        source_workflow: true,
                        chunk_method: media_core::Av1anChunkMethod::Select,
                        split_method: media_core::Av1anSplitMethod::FixedChunks,
                        encoder_threads: Some(1),
                        concat_method: media_core::Av1anConcatMethod::Mkvmerge,
                        ..Default::default()
                    }),
                    temporal: resample.then_some(media_core::TemporalSettings {
                        frame_rate: Some(media_core::FrameRate {
                            numerator: 30,
                            denominator: 1,
                        }),
                        ..Default::default()
                    }),
                    encoding_preferences: Some(media_core::EncodingPreferences {
                        keyframe_interval_seconds: 10,
                        mp4_faststart: true,
                        full_output_verification: thorough,
                    }),
                    ..Default::default()
                },
            };
            let submitted = manager.start_encode(request).await.unwrap();
            let finished = tokio::time::timeout(Duration::from_secs(90), async {
                loop {
                    let job = manager
                        .list_jobs()
                        .await
                        .into_iter()
                        .find(|job| job.id == submitted.id)
                        .unwrap();
                    if job.state.is_terminal() {
                        break job;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(finished.state, JobState::Succeeded, "{finished:#?}");
            let calls = (CHUNK_PROOF_CALLS.get(), REFERENCE_JOIN_CALLS.get());
            eprintln!(
                "thorough={thorough} resample={resample}: chunk/reference proof calls {calls:?}"
            );
            assert_eq!(calls, if thorough || resample { (1, 1) } else { (0, 0) });
        }
        manager.shutdown().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_zero_quality_is_literal_and_distinct_from_explicit_lossless_for_all_adapters() {
        let (document, _) = fixture_plan(0);
        for backend in [
            media_core::EncodeBackend::Standalone,
            media_core::EncodeBackend::Av1an,
        ] {
            for encoder in [
                VideoEncoder::SvtAv1,
                VideoEncoder::X264,
                VideoEncoder::X265Standalone,
                VideoEncoder::AomAv1,
                VideoEncoder::VpxStandalone,
            ] {
                let settings = EncodeSettings {
                    encoder,
                    backend,
                    crf: 0,
                    lossless: false,
                    ..Default::default()
                };
                super::super::encode_plan::validate_settings(&settings).unwrap();
                let plan =
                    Plan::build(&document, &document.selected(&[0]).unwrap(), &settings).unwrap();
                let args = if backend == media_core::EncodeBackend::Standalone {
                    super::super::encode::consumer_arguments(&plan, &settings)
                } else {
                    encoder::parameters(&plan, &settings)
                };
                let args = args
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                assert!(
                    !args
                        .iter()
                        .any(|value| value == "--lossless" || value.starts_with("--lossless=")),
                    "{encoder:?} {backend:?}: {args:?}"
                );
                let flag = if matches!(encoder, VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone)
                {
                    "--cq-level"
                } else {
                    "--crf"
                };
                assert!(
                    args.windows(2)
                        .any(|pair| pair[0] == flag && pair[1] == "0")
                        || args.contains(&format!("{flag}=0")),
                    "{encoder:?} {backend:?}: {args:?}"
                );
            }
        }
    }
    #[test]
    fn source_target_defaults_omit_hidden_overrides_and_resolve_each_encoder_range() {
        let (document, _) = fixture_plan(0);
        for encoder in [
            VideoEncoder::SvtAv1,
            VideoEncoder::X264,
            VideoEncoder::X265Standalone,
            VideoEncoder::AomAv1,
            VideoEncoder::VpxStandalone,
        ] {
            let options = media_core::Av1anOptions {
                source_target_defaults: true,
                chunk_method: media_core::Av1anChunkMethod::Select,
                target_quality: Some(media_core::Av1anTargetQuality {
                    metric: media_core::Av1anTargetMetric::XpsnrWeighted,
                    minimum_score_tenths: 400,
                    maximum_score_tenths: 0,
                    minimum_crf: 0,
                    maximum_crf: 0,
                    probes: 0,
                    probing_rate: 255,
                    probe_width: 1,
                    probe_height: 3,
                }),
                ..Default::default()
            };
            let settings = EncodeSettings {
                encoder,
                backend: media_core::EncodeBackend::Av1an,
                av1an_options: Some(options),
                ..Default::default()
            };
            super::super::encode_plan::validate_settings(&settings).unwrap();
            let plan =
                Plan::build(&document, &document.selected(&[0]).unwrap(), &settings).unwrap();
            let args = arguments(
                Path::new("/input.mkv"),
                Path::new("/output.ivf"),
                Path::new("/work"),
                Path::new("/log"),
                &plan,
                &settings,
                false,
                Some("hue=s=0"),
            );
            for flag in [
                "--qp-range",
                "--probes",
                "--probing-rate",
                "--probe-res",
                "--vmaf-res",
                "--probing-stat",
                "--probe-video-params",
                "--vmaf-filter",
            ] {
                assert!(!args.iter().any(|arg| arg == flag), "{encoder:?}: {flag}");
            }
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == "--target-quality" && pair[1] == "40.0")
            );
            assert!(
                args.iter()
                    .any(|argument| argument == "--source-unfiltered-probes")
            );
            let effective = options::effective_target(options, encoder, (128, 96)).unwrap();
            assert_eq!(
                (effective.minimum_crf, effective.maximum_crf),
                (
                    15,
                    match encoder {
                        VideoEncoder::X264 | VideoEncoder::X265Standalone => 35,
                        VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone => 55,
                        _ => 50,
                    }
                )
            );
            assert_eq!(
                (
                    effective.probes,
                    effective.probing_rate,
                    effective.probe_width,
                    effective.probe_height
                ),
                (4, 1, 1920, 1080)
            );
            assert_eq!(effective.maximum_score_tenths, 400);
        }
    }
    fn fixture_plan(index: u32) -> (Document, Plan) {
        let video = |index| serde_json::json!({"index":index,"codec_type":"video","width":128,"height":96,"pix_fmt":"yuv420p","sample_aspect_ratio":"1:1","avg_frame_rate":"24000/1001","start_time":"0","color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709","color_range":"tv","chroma_location":"left"});
        let document: Document = serde_json::from_value(
            serde_json::json!({"streams":[video(0),video(2)],"format":{"start_time":"0"}}),
        )
        .unwrap();
        let plan = Plan::build(
            &document,
            &document.selected(&[index]).unwrap(),
            &EncodeSettings {
                video_stream_index: index,
                ..Default::default()
            },
        )
        .unwrap();
        (document, plan)
    }
    #[test]
    fn av1an_requires_first_video_instead_of_silently_encoding_another_track() {
        let (document, first) = fixture_plan(0);
        validate_input(&document, &first).unwrap();
        let (_, second) = fixture_plan(2);
        assert_eq!(
            validate_input(&document, &second).unwrap_err().code,
            "ENCODE_INPUT_UNSUPPORTED"
        );
    }
    #[test]
    fn source_none_uses_frozen_typed_rate_and_omits_scene_only_flags() {
        let (document, _) = fixture_plan(0);
        let settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            source_filters: true,
            av1an_filters: vec!["fps=12".into()],
            temporal: Some(media_core::TemporalSettings {
                frame_rate: Some(media_core::FrameRate {
                    numerator: 30,
                    denominator: 1,
                }),
                ..Default::default()
            }),
            encoding_preferences: Some(media_core::EncodingPreferences {
                keyframe_interval_seconds: 2,
                mp4_faststart: true,
                full_output_verification: true,
            }),
            av1an_options: Some(media_core::Av1anOptions {
                source_workflow: true,
                split_method: media_core::Av1anSplitMethod::FixedChunks,
                maximum_chunk_frames: 0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut plan =
            Plan::build(&document, &document.selected(&[0]).unwrap(), &settings).unwrap();
        plan.activate_temporal(240).unwrap();
        assert_eq!(plan.source_chunk_rate(), (30, 1));
        plan.apply_source_filter_contract(&super::super::source_filters::OutputContract {
            width: 128,
            height: 96,
            sar: "1:1".into(),
            numerator: 12,
            denominator: 1,
            frames: 120,
        })
        .unwrap();
        let args = arguments(
            Path::new("/source.mkv"),
            Path::new("/out.ivf"),
            Path::new("/work"),
            Path::new("/log"),
            &plan,
            &settings,
            false,
            None,
        );
        assert!(args.windows(2).any(|pair| pair == ["--extra-split", "60"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--split-method", "none"])
        );
        for flag in ["--sc-method", "--min-scene-len", "--sc-downscale-height"] {
            assert!(!args.iter().any(|arg| arg == flag));
        }
        let legacy = EncodeSettings {
            av1an_options: Some(media_core::Av1anOptions {
                source_workflow: false,
                ..settings.av1an_options.unwrap()
            }),
            ..settings
        };
        let args = arguments(
            Path::new("/source.mkv"),
            Path::new("/out.ivf"),
            Path::new("/work"),
            Path::new("/log"),
            &plan,
            &legacy,
            false,
            None,
        );
        assert!(args.windows(2).any(|pair| pair == ["--extra-split", "0"]));
        assert!(args.iter().any(|arg| arg == "--sc-downscale-height"));
    }
    #[test]
    fn cli_keeps_paths_separate_and_settings_explicit() {
        let (_, plan) = fixture_plan(0);
        let input = Path::new("/media/movie's & $ 日本語.mkv");
        let settings = EncodeSettings {
            film_grain: 8,
            workers: 3,
            ..Default::default()
        };
        let args = arguments(
            input,
            Path::new("/out/owned.ivf"),
            Path::new("/out/work"),
            Path::new("/logs/job.log"),
            &plan,
            &settings,
            false,
            plan.decoder_filter().as_deref(),
        );
        assert!(
            args.windows(2)
                .any(|a| a[0] == "-i" && a[1] == input.as_os_str())
        );
        assert!(args.windows(2).any(|a| a == ["--workers", "3"]));
        assert!(args.windows(2).any(|a| a == ["--chunk-method", "lsmash"]));
        assert!(args.windows(2).any(|a| a == ["--concat", "ffmpeg"]));
        assert!(
            args.windows(2)
                .any(|a| a == ["--audio-params", "-an -sn -dn -map -0:t?"])
        );
        let params = args.windows(2).find(|a| a[0] == "--video-params").unwrap()[1]
            .to_str()
            .unwrap();
        assert!(params.contains("--film-grain 8 --film-grain-denoise 0"));
        assert!(params.contains("--fps-num 24000 --fps-denom 1001"));

        let filtered = arguments(
            input,
            Path::new("/out/owned.ivf"),
            Path::new("/out/work"),
            Path::new("/logs/job.log"),
            &plan,
            &settings,
            false,
            Some("bwdif=mode=send_frame"),
        );
        assert!(
            filtered
                .windows(2)
                .any(|pair| { pair[0] == "--ffmpeg" && pair[1] == "-vf bwdif=mode=send_frame" })
        );
        let targeted_settings = EncodeSettings {
            av1an_options: Some(media_core::Av1anOptions {
                target_quality: Some(media_core::Av1anTargetQuality {
                    metric: media_core::Av1anTargetMetric::Vmaf,
                    minimum_score_tenths: 930,
                    maximum_score_tenths: 950,
                    minimum_crf: 15,
                    maximum_crf: 50,
                    probes: 4,
                    probing_rate: 1,
                    probe_width: 1920,
                    probe_height: 1080,
                }),
                ..Default::default()
            }),
            ..settings.clone()
        };
        let targeted = arguments(
            input,
            Path::new("/out/owned.ivf"),
            Path::new("/out/work"),
            Path::new("/logs/job.log"),
            &plan,
            &targeted_settings,
            false,
            Some("bwdif=mode=send_frame"),
        );
        assert_eq!(
            targeted.iter().filter(|value| *value == "--ffmpeg").count(),
            1
        );
        assert_eq!(
            targeted
                .iter()
                .filter(|value| *value == "--vmaf-filter")
                .count(),
            1
        );
        assert!(
            targeted
                .windows(2)
                .any(|pair| { pair[0] == "--ffmpeg" && pair[1] == "-vf bwdif=mode=send_frame" })
        );
        assert!(
            targeted
                .windows(2)
                .any(|pair| { pair[0] == "--vmaf-filter" && pair[1] == "bwdif=mode=send_frame" })
        );
        let prepared = arguments(
            input,
            Path::new("/out/owned.ivf"),
            Path::new("/out/work"),
            Path::new("/logs/job.log"),
            &plan,
            &settings,
            false,
            None,
        );
        assert!(!prepared.iter().any(|value| value == "--ffmpeg"));
        let targeted_prepared = arguments(
            input,
            Path::new("/out/owned.ivf"),
            Path::new("/out/work"),
            Path::new("/logs/job.log"),
            &plan,
            &targeted_settings,
            false,
            None,
        );
        assert!(
            !targeted_prepared
                .iter()
                .any(|value| value == "--ffmpeg" || value == "--vmaf-filter")
        );
        assert!(!params.contains("movie"));
        assert!(
            !args
                .iter()
                .any(|a| a == "--force" || a == "--ignore-frame-mismatch" || a == "--resume")
        );
    }
    #[test]
    fn chunk_progress_ignores_torn_or_inconsistent_snapshots() {
        assert_eq!(progress(br#"{"frames":236,"done":{"00001":{"frames":55,"size_bytes":42},"00002":{"frames":11}}}"#, 236), Some((66,2)));
        for data in [
            br#"{"frames":236,"done":{"0":{"frames":237}}}"#.as_slice(),
            br#"{"frames":235,"done":{}}"#,
            br#"{"frames":236,"done":{"#,
        ] {
            assert_eq!(progress(data, 236), None);
        }
    }
    #[test]
    fn ivf_header_repair_requires_exact_frame_records_and_leaves_bad_input_untouched() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "jesses-ivf-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("owned.ivf");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let mut bytes = Vec::from(&b"DKIF\0\0\x20\0AV01"[..]);
        bytes.extend(128u16.to_le_bytes());
        bytes.extend(96u16.to_le_bytes());
        bytes.extend(30u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        for index in 0..3u64 {
            bytes.extend(1u32.to_le_bytes());
            bytes.extend(index.to_le_bytes());
            bytes.push(0);
        }
        file.write_all(&bytes).unwrap();
        let (_tx, cancel) = watch::channel(false);
        assert!(normalize_ivf(&mut file, (128, 96), (24000, 1001), 2, &cancel).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(normalize_ivf(&mut file, (128, 96), (24000, 1001), 3, &cancel).unwrap());
        let repaired = std::fs::read(&path).unwrap();
        assert_eq!(&repaired[16..20], &24000u32.to_le_bytes());
        assert_eq!(&repaired[20..24], &1001u32.to_le_bytes());
        assert_eq!(&repaired[24..28], &3u32.to_le_bytes());
        assert_eq!(&repaired[32..], &bytes[32..]);
        assert!(!normalize_ivf(&mut file, (128, 96), (24000, 1001), 3, &cancel).unwrap());
        drop(file);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
