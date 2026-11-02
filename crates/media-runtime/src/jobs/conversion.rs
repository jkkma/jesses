//! Audio conversion and packet-copy video share the ordinary immutable job,
//! cancellation, metadata verification and no-clobber publication lifecycle.
use super::*;
use media_core::{EncodeCommandPlan, EncodeCommandStage, VideoMode};

pub(super) fn validate_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    audio::validate_settings(settings)?;
    external_tracks::validate(settings)?;
    trim::validate_settings(settings)?;
    super::subtitles::validate_settings(settings)?;
    if settings.constant_quantizer.is_some()
        || settings.hdr_metadata.is_some()
        || settings.framing != media_core::VideoFraming::default()
        || settings.temporal.is_some()
        || settings.tone_map.is_some()
        || !settings.parameters.is_empty()
        || !settings.av1an_filters.is_empty()
        || settings.av1an_grain.is_some()
        || settings.output_pixel_format.is_some()
        || settings.av1an_options.is_some()
        || settings.mov_timecode_track.is_some()
        || settings.external_tracks.iter().any(|track| {
            track.subtitle_burn_in || track.subtitle_mode == Some(media_core::SubtitleMode::BurnIn)
        })
        || settings
            .subtitles
            .iter()
            .any(|track| track.mode == media_core::SubtitleMode::BurnIn || track.burn_in)
    {
        return Err(AppError::new(
            "ENCODE_SETTINGS_INVALID",
            "Copy-video and audio-only conversion require picture controls and subtitle burn-in to be disabled. Choose video encoding to apply picture changes.",
            None,
        ));
    }
    Ok(())
}

pub(super) fn arguments(
    input: &Path,
    output: &Path,
    selected: &[&metadata::Stream],
    settings: &EncodeSettings,
    external: &external_tracks::ExternalTracks,
) -> Vec<OsString> {
    arguments_for_inputs(input, output, selected, settings, external, true)
}

pub(super) fn source_arguments(
    input: &Path,
    output: &Path,
    selected: &[&metadata::Stream],
    settings: &EncodeSettings,
    external: &external_tracks::ExternalTracks,
) -> Vec<OsString> {
    arguments_for_inputs(input, output, selected, settings, external, false)
}
fn arguments_for_inputs(
    input: &Path,
    output: &Path,
    selected: &[&metadata::Stream],
    settings: &EncodeSettings,
    external: &external_tracks::ExternalTracks,
    reserve_encoded: bool,
) -> Vec<OsString> {
    let input_number = |index: usize| {
        if reserve_encoded || index < 2 {
            index
        } else {
            index - 1
        }
    };
    let donor_number = |index: isize| {
        if reserve_encoded || index < 2 {
            index
        } else {
            index - 1
        }
    };
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "warning",
        "-xerror",
        "-copyts",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(input.as_os_str().to_owned());
    // ExternalTracks uses input 1 for the encoded-picture intermediate. Keep
    // that slot reserved with an unused read-only source in the copy path.
    if reserve_encoded {
        args.extend([
            "-protocol_whitelist".into(),
            "file".into(),
            "-i".into(),
            input.as_os_str().to_owned(),
        ]);
    }
    external.append_inputs(&mut args);
    for stream in selected {
        let (input, index) = external.input_stream(stream.index);
        args.extend([
            "-map".into(),
            format!("{}:{index}", input_number(input)).into(),
        ]);
    }
    args.extend([
        "-map_metadata".into(),
        donor_number(external.metadata_input_index(settings))
            .to_string()
            .into(),
        "-map_chapters".into(),
        donor_number(external.chapters_input_index(settings))
            .to_string()
            .into(),
        "-c".into(),
        "copy".into(),
    ]);
    super::output_metadata::append(&mut args, settings);
    for key in ["major_brand", "minor_version", "compatible_brands"] {
        args.extend(["-metadata".into(), format!("{key}=").into()]);
    }
    let mux_settings = external.mux_settings(settings);
    for (position, stream) in selected.iter().enumerate() {
        let (input, index) = external.input_stream(stream.index);
        if settings.copy_metadata != Some(false)
            || stream.codec_type.as_deref() == Some("attachment")
        {
            args.extend([
                format!("-map_metadata:s:{position}").into(),
                format!("{}:s:{index}", input_number(input)).into(),
            ]);
        }
        if let Some(track) =
            audio::converted(&mux_settings).find(|track| track.stream_index == stream.index)
        {
            audio::append_arguments(&mut args, position, track, stream);
            for key in stream
                .tags
                .keys()
                .filter(|key| metadata::is_derived_stream_tag(key))
            {
                args.extend([
                    format!("-metadata:s:{position}").into(),
                    format!("{key}=").into(),
                ]);
            }
        }
        let flags = stream
            .disposition
            .iter()
            .filter(|(_, v)| **v != 0)
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>()
            .join("+");
        args.extend([
            format!("-disposition:{position}").into(),
            if flags.is_empty() {
                "0".into()
            } else {
                flags.into()
            },
        ]);
        if let Some(overrides) = external.track_metadata(stream.index, settings) {
            for (key, value) in [
                ("title", overrides.title.clone()),
                ("handler_name", overrides.title),
                ("language", overrides.language),
            ] {
                if let Some(value) = value {
                    args.extend([
                        format!("-metadata:s:{position}").into(),
                        format!("{key}={value}").into(),
                    ]);
                }
            }
        }
    }
    args.extend([
        "-avoid_negative_ts".into(),
        "disabled".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
    ]);
    container::append_staging_format(&mut args, selected, Some(&mux_settings));
    args.push(output.as_os_str().to_owned());
    args
}

fn trim_bounds(
    settings: &EncodeSettings,
    document: &Document,
) -> Result<Option<(f64, f64)>, AppError> {
    let Some(trim) = settings.trim else {
        return Ok(None);
    };
    if let Some(time) = trim.time {
        return Ok(Some((
            time.start_milliseconds as f64 / 1000.0,
            time.end_milliseconds as f64 / 1000.0,
        )));
    }
    let video=document.streams.iter().find(|stream|stream.index==settings.video_stream_index && stream.codec_type.as_deref()==Some("video")).ok_or_else(||AppError::new("TRIM_UNSUPPORTED","Frame-based trimming requires a source video; use time boundaries for audio-only sources.",None))?;
    let rate = video
        .avg_frame_rate
        .as_deref()
        .or(video.r_frame_rate.as_deref())
        .and_then(|value| value.split_once('/'))
        .and_then(|(n, d)| Some((n.parse::<f64>().ok()?, d.parse::<f64>().ok()?)))
        .filter(|(n, d)| *n > 0.0 && *d > 0.0)
        .ok_or_else(|| {
            AppError::new(
                "TRIM_UNSUPPORTED",
                "The source frame rate is unavailable; use time boundaries.",
                None,
            )
        })?;
    Ok(Some((
        trim.start_frame as f64 * rate.1 / rate.0,
        trim.end_frame_exclusive as f64 * rate.1 / rate.0,
    )))
}

impl JobManager {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn convert(
        &self,
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        temporary: &mut Option<Temporary>,
        scratch: &mut Vec<Temporary>,
        mut preview: Option<&mut EncodeCommandPlan>,
    ) -> Result<(), AppError> {
        if settings.source_trim || settings.video_mode != VideoMode::Copy || settings.trim.is_none()
        {
            return Box::pin(self.convert_inner(
                id, request, settings, cancel, log_path, temporary, scratch, preview, None,
            ))
            .await;
        }
        validate_settings(settings)?;
        if !settings.external_tracks.is_empty()
            || settings.metadata_source_path.is_some()
            || settings.chapters_source_path.is_some()
        {
            return Err(AppError::new(
                "TRIM_UNSUPPORTED",
                "Copy-video trim currently uses one source. Trim external sources first, or choose video encoding for a mixed-source trim.",
                None,
            ));
        }
        let original = Source::open(Path::new(&request.input_path))?;
        let output = files::output_path(request, &original)?;
        audio::validate_gain_source(&original, settings)?;
        let ffprobe = discover("ffprobe", cancel).await?;
        let document = probe(
            &ffprobe,
            &original.path,
            cancel,
            Some(&request.stream_indices),
        )
        .await?;
        let (start, end) = trim_bounds(settings, &document)?.expect("trim present");
        if document
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("video"))
            .map(|stream| stream.index)
            != Some(settings.video_stream_index)
        {
            return Err(AppError::new(
                "TRIM_UNSUPPORTED",
                "Lossless copy trimming currently uses the source's first video track.",
                None,
            ));
        }
        self.phase(
            id,
            JobState::Preparing,
            "Preparing and verifying the keyframe-aligned source interval.",
        )
        .await;
        let directory = rate_control::Stats::create(&output, &format!("{id}-copy-trim"))?;
        let trimmed = directory.path.join("jesses.stats.trim.mkv");
        let artifact = Box::pin(crate::utilities::run_keyframe_cut(
            media_core::KeyframeCutRequest {
                input_path: request.input_path.clone(),
                output_path: trimmed.to_string_lossy().into_owned(),
                start_seconds: start,
                end_seconds: end,
            },
            cancel,
        ))
        .await?;
        if let media_core::UtilityResult::Artifact(artifact) = artifact {
            self.change(id, |snapshot| append_log(snapshot, artifact.message))
                .await;
        }
        original.verify()?;
        directory.check()?;
        check_cancel(cancel)?;
        let remap = |index: u32| -> Result<u32, AppError> {
            document
                .streams
                .iter()
                .position(|stream| stream.index == index)
                .map(|position| position as u32)
                .ok_or_else(|| {
                    AppError::new(
                        "STREAM_SELECTION_INVALID",
                        "The requested source track is absent.",
                        None,
                    )
                })
        };
        let mut staged_request = request.clone();
        staged_request.input_path = trimmed.to_string_lossy().into_owned();
        staged_request.stream_indices = request
            .stream_indices
            .iter()
            .map(|index| remap(*index))
            .collect::<Result<_, _>>()?;
        let mut staged = settings.clone();
        staged.trim = None;
        staged.video_stream_index = remap(settings.video_stream_index)?;
        for audio in &mut staged.audio {
            audio.stream_index = remap(audio.stream_index)?;
            if let Some(gain) = &mut audio.gain {
                gain.source_fingerprint = None;
            }
        }
        for subtitle in &mut staged.subtitles {
            subtitle.stream_index = remap(subtitle.stream_index)?;
        }
        for track in &mut staged.track_overrides {
            track.stream_index = remap(track.stream_index)?;
        }
        for track in &mut staged.track_order {
            if track.input_path.is_none() {
                track.stream_index = remap(track.stream_index)?;
            }
        }
        let result = Box::pin(self.convert_inner(
            id,
            &staged_request,
            &staged,
            cancel,
            log_path,
            temporary,
            scratch,
            preview.as_deref_mut(),
            Some(&original),
        ))
        .await;
        original.verify()?;
        directory.check()?;
        if let Some(preview) = preview {
            preview.source_fingerprint = crate::analysis::fingerprint(&original)?;
            preview.notes.insert(0,format!("A verified lossless cut first snaps {start:.6}s to the preceding source keyframe, then converts audio through the same path. The cut ends at {end:.6}s with packet boundary pre-roll documented in the job log."));
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn convert_inner(
        &self,
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        temporary: &mut Option<Temporary>,
        scratch: &mut Vec<Temporary>,
        preview: Option<&mut EncodeCommandPlan>,
        original: Option<&Source>,
    ) -> Result<(), AppError> {
        validate_settings(settings)?;
        let source_copy = settings.source_trim
            && settings.video_mode == VideoMode::Copy
            && settings.trim.is_some();
        self.phase(
            id,
            JobState::Preparing,
            "Checking selected tracks and audio conversion capabilities.",
        )
        .await;
        let source = Source::open(Path::new(&request.input_path))?;
        let output = files::output_path(request, &source)?;
        audio::validate_gain_source(&source, settings)?;
        let ffmpeg = discover("ffmpeg", cancel).await?;
        let ffprobe = discover("ffprobe", cancel).await?;
        let document = probe(
            &ffprobe,
            &source.path,
            cancel,
            Some(&request.stream_indices),
        )
        .await?;
        let source_window = if source_copy {
            let (start, end) = trim_bounds(settings, &document)?.expect("source copy trim");
            Some(super::source_copy::Window::new(start, end)?)
        } else {
            None
        };
        if let Some(window) = source_window {
            self.change(id,|snapshot|append_log(snapshot,format!("Source Copy uses input-side keyframe seeking at {:.3}s with the original {:.3}s duration; every selected original input is sought together, retaining the source GOP preroll.",window.start,window.end-window.start))).await;
        }
        let source_selected = document.selected(&request.stream_indices)?;
        if settings.video_mode == VideoMode::Drop
            && source_selected
                .iter()
                .any(|stream| stream.codec_type.as_deref() == Some("video"))
        {
            return Err(AppError::new(
                "ENCODE_SELECTION_INVALID",
                "Audio-only conversion requires the video tracks to be deselected.",
                None,
            ));
        }
        let external = external_tracks::ExternalTracks::prepare(
            &source, request, settings, &document, &ffprobe, cancel,
        )
        .await?;
        let output_document = external.extend_document(&document, settings);
        let mux_settings = external.mux_settings(settings);
        let indices =
            external.output_indices(&document, request.stream_indices.clone(), settings)?;
        let selected = output_document.selected(&indices)?;
        if selected.is_empty() {
            return Err(AppError::new(
                "ENCODE_SELECTION_INVALID",
                "Select at least one track to convert or copy.",
                None,
            ));
        }
        audio::validate_selection(&selected, &mux_settings)?;
        super::subtitles::validate_selection(&selected, &mux_settings)?;
        if source_copy {
            container::preflight_final(&output, &output_document, &selected, Some(&mux_settings))?;
        } else {
            container::preflight(&output, &output_document, &selected, Some(&mux_settings))?;
        }
        audio::check_encoders(&ffmpeg, &mux_settings, cancel).await?;
        if audio::converted(&mux_settings).any(|track| {
            matches!(
                track.codec,
                media_core::AudioCodec::Aac | media_core::AudioCodec::Opus
            )
        }) {
            scratch.push(Temporary::create(&output, &format!("{id}-audio-tools"))?);
            audio::check_delay_support(&ffmpeg, &ffprobe, scratch.last().unwrap(), cancel).await?;
        }
        let mut timelines = Vec::new();
        for track in audio::converted(&mux_settings) {
            let stream = *selected
                .iter()
                .find(|stream| stream.index == track.stream_index)
                .expect("validated converted track");
            let (path, original) = external
                .audio_source(track.stream_index)
                .unwrap_or((source.path.as_path(), stream));
            if track.codec != media_core::AudioCodec::Copy {
                scratch.push(Temporary::create(
                    &output,
                    &format!("{id}-audio-tools-{}", track.stream_index),
                )?);
                audio::check_conversion_support(
                    &ffmpeg,
                    &ffprobe,
                    scratch.last().unwrap(),
                    original,
                    track,
                    cancel,
                )
                .await?;
            }
            timelines.push((
                track,
                audio::scan(&ffprobe, path, original, false, cancel)
                    .await?
                    .shifted(external.offset_seconds(track.stream_index)),
            ));
        }
        let mut audio_filters = std::collections::BTreeMap::new();
        let prepared_trim = if let Some((start, end)) = trim_bounds(settings, &document)? {
            trim::validate_selection(&selected, &mux_settings)?;
            let duration = output_document
                .selected_duration(&selected)
                .ok_or_else(|| {
                    AppError::new(
                        "TRIM_UNSUPPORTED",
                        "The source duration is unavailable.",
                        None,
                    )
                })?;
            if end > duration + 0.000001 || end <= start {
                return Err(AppError::new(
                    "TRIM_UNSUPPORTED",
                    "The selected interval exceeds the source duration or is empty.",
                    None,
                ));
            }
            for (track, timeline) in &mut timelines {
                let (clipped, filter) = if source_copy {
                    let source_document = external
                        .source_track(track.stream_index)
                        .map_or(&document, |(_, document, _)| document);
                    let origin = source_document
                        .format
                        .as_ref()
                        .and_then(|format| format.start_time.as_deref())
                        .and_then(|time| time.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    // Input-side accurate seek compares the authored shifted
                    // timestamps to its seek boundary. Preserve that selection;
                    // a source offset changes which samples reach zero here.
                    timeline.shifted(-origin).clipped(start, end)?
                } else {
                    timeline.clipped(start, end)?
                };
                *timeline = clipped;
                audio_filters.insert(track.stream_index, filter);
            }
            if source_copy {
                None
            } else {
                Some(
                    trim::Prepared::build(
                        &mux_settings,
                        trim::Interval {
                            frames: 0,
                            source_start_frame: 0,
                            source_end_frame: 0,
                            start,
                            end,
                        },
                        &output_document,
                        &selected,
                        &external,
                        &ffmpeg,
                        &source.path,
                        &output,
                        id,
                        scratch,
                        cancel,
                    )
                    .await?,
                )
            }
        } else {
            None
        };
        let source_subtitles = if source_copy {
            mux_settings
                .subtitles
                .iter()
                .filter(|track| track.mode != media_core::SubtitleMode::Copy)
                .map(|track| track.stream_index)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let subtitle_assets = if source_copy {
            super::subtitles::Prepared::empty()
        } else {
            super::subtitles::Prepared::build(
                &mux_settings,
                &document,
                &selected,
                prepared_trim.as_ref(),
                &external,
                &ffmpeg,
                &source.path,
                &output,
                id,
                cancel,
            )
            .await?
        };
        let normalization = audio::Normalization::prepare(
            &ffmpeg,
            &source.path,
            &selected,
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
        source.verify()?;
        external.verify()?;
        check_cancel(cancel)?;
        let stage_format = if source_copy {
            container::format(&output)?
        } else {
            container::staging_format(&selected, Some(&mux_settings))
        };
        *temporary = Some(Temporary::create_extension(
            &output,
            id,
            stage_format.extension(),
        )?);
        let temp = temporary.as_ref().unwrap();
        let mut args = if let Some(window) = source_window {
            super::source_copy::arguments(
                &source.path,
                &temp.path,
                &selected,
                settings,
                &external,
                window,
                stage_format,
            )
        } else {
            arguments(&source.path, &temp.path, &selected, settings, &external)
        };
        if let Some(prepared) = &prepared_trim {
            prepared.apply_mux(&mut args, &selected, &audio_filters)?;
        }
        normalization.apply_mux(&mut args, &selected)?;
        subtitle_assets.apply_mux(&mut args, &selected);
        if source_copy {
            super::source_copy::append_subtitle_codecs(&mut args, &selected, &mux_settings);
        }
        if let Some(preview) = preview {
            preview.source_fingerprint = crate::analysis::fingerprint(&source)?;
            preview.output_frame_count = "copied".into();
            preview.output_frame_rate = "source".into();
            preview.stages.push(EncodeCommandStage { label: "Convert audio and copy selected tracks".into(),executable:ffmpeg.to_string_lossy().into_owned(),arguments:args.iter().map(|value|value.to_string_lossy().into_owned()).collect(),working_directory:None,notes:vec!["The selected audio tracks are converted; all other selected packets are copied. Output metadata, audio samples and copied packets are verified before publication.".into()] });
            preview.notes.push("Execution uses a private Matroska staging file, then converts to the chosen destination container and validates it before publication.".into());
            return Ok(());
        }
        let command = CommandSpec {
            executable: ffmpeg.clone(),
            args: args.clone(),
            cwd: None,
        };
        if let Some(review) = self
            .review_command(
                id,
                "ffmpeg",
                &super::command_edit::format_command(&command),
                &output,
                cancel,
            )
            .await?
            && review.arguments != review.generated.trim()
        {
            self.run_edited_program(id, &review, None, &source.path, cancel, log_path)
                .await?;
            let actual =
                super::command_edit::verify_actual_output(&ffmpeg, &ffprobe, temp, cancel).await?;
            source.verify()?;
            external.verify()?;
            if let Some(original) = original {
                original.verify()?;
            }
            self.change(id, |snapshot| { snapshot.duration_seconds = actual.duration(); append_log(snapshot, "The edited output was fully decoded; confirmed command semantics replace the original codec, geometry and trim expectations.".into()); }).await;
            if source_copy {
                return self
                    .publish_verified(id, cancel, &source, Some(&external), temp, &output)
                    .await;
            }
            return self
                .finalize_verified(
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
                .await;
        }
        self.change(id, |snapshot| {
            snapshot.duration_seconds = output_document.selected_duration(&selected);
            snapshot.progress_seconds = Some(0.0);
        })
        .await;
        tokio::fs::create_dir_all(self.log_dir.as_ref())
            .await
            .map_err(|cause| {
                files::error(
                    "LOG_CREATE_FAILED",
                    cause.to_string(),
                    self.log_dir.as_ref(),
                )
            })?;
        self.phase(
            id,
            JobState::Running,
            "Converting audio and copying the other selected tracks.",
        )
        .await;
        let (sender, mut receiver) = mpsc::channel(256);
        let observer = self.clone();
        let job_id = id.to_owned();
        let events = tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                observer
                    .change(&job_id, |snapshot| match event {
                        ProcessEvent::Stdout(line) => {
                            if let Some(seconds) = progress_seconds(&line) {
                                snapshot.progress_seconds = Some(seconds);
                            }
                        }
                        ProcessEvent::Stderr(line) => append_log(snapshot, line),
                    })
                    .await;
            }
        });
        let outcome = supervisor::run(
            &CommandSpec {
                executable: ffmpeg.clone(),
                args,
                cwd: None,
            },
            cancel.clone(),
            sender,
            log_path,
            Duration::from_secs(24 * 60 * 60),
        )
        .await;
        let _ = events.await;
        let outcome = outcome.map_err(|cause| process_error(cause, &source.path))?;
        if !outcome.status.success() {
            return Err(files::error(
                "AUDIO_CONVERSION_FAILED",
                "FFmpeg could not convert the selected tracks; inspect the job log.",
                &source.path,
            ));
        }
        self.phase(
            id,
            JobState::Finalizing,
            "Verifying audio samples, copied packets and metadata.",
        )
        .await;
        temp.flush_nonempty_async().await?;
        let mut artifact = probe(&ffprobe, &temp.path, cancel, None).await?;
        let verified_trim = if let Some(prepared) = &prepared_trim {
            Some(
                prepared
                    .verify_source(
                        &ffmpeg, &ffprobe, &temp.path, &selected, &artifact, None, None, cancel,
                    )
                    .await?,
            )
        } else {
            None
        };
        if let (Some(prepared), Some((expected, _))) = (&prepared_trim, &verified_trim) {
            prepared.normalize_verified_packet_starts(&mut artifact, expected, &selected);
        }
        let mut expected_document = if let Some(window) = source_window {
            trim::Interval {
                frames: 0,
                source_start_frame: 0,
                source_end_frame: 0,
                start: window.start,
                end: window.end,
            }
            .expected_document(&output_document)?
        } else {
            verified_trim
                .as_ref()
                .map_or(&output_document, |(document, _)| document)
                .clone()
        };
        let mut copy_origin = None;
        if let Some(window) = source_window {
            let mut excluded = audio::converted(&mux_settings)
                .map(|track| track.stream_index)
                .collect::<Vec<_>>();
            excluded.extend_from_slice(subtitle_assets.overridden_indices());
            excluded.extend_from_slice(&source_subtitles);
            copy_origin = Some(
                super::source_copy_packets::verify_with_normalization(
                    &ffmpeg,
                    &ffprobe,
                    &source.path,
                    &document,
                    &temp.path,
                    &selected,
                    &artifact,
                    &mut expected_document,
                    &mux_settings,
                    &external,
                    (window.start, window.end),
                    &excluded,
                    scratch,
                    cancel,
                    &normalization,
                )
                .await?,
            );
            // Complete packet evidence supersedes AAC's header CodecDelay start.
            for (position, stream) in selected.iter().enumerate() {
                if !excluded.contains(&stream.index)
                    && stream.codec_type.as_deref() != Some("attachment")
                {
                    artifact.streams[position].packet_start_time = expected_document
                        .streams
                        .iter()
                        .find(|item| item.index == stream.index)
                        .and_then(|item| item.packet_start_time);
                }
            }
        }
        let expected_document = subtitle_assets.expected_document(&expected_document);
        let (expected_document, converted_subtitles) = if source_copy {
            let mut expected = expected_document;
            expected.streams = expected.selected(&indices)?.into_iter().cloned().collect();
            let converted = if stage_format == media_core::ContainerFormat::Matroska {
                Vec::new()
            } else {
                container::normalize(&mut expected, &mut artifact, stage_format)?
            };
            (expected, converted)
        } else {
            container::normalize_staging(
                &expected_document,
                &indices,
                &mut artifact,
                Some(&mux_settings),
            )?
        };
        let expected_selected = expected_document.selected(&indices)?;
        metadata::verify_audio_conversion_with_subtitles(
            &expected_document,
            &expected_selected,
            &artifact,
            &mux_settings.audio,
            &converted_subtitles,
        )?;
        if let Some(prepared) = &prepared_trim {
            prepared
                .verify_subtitles(
                    &discover("ffmpeg", cancel).await?,
                    &temp.path,
                    &selected,
                    subtitle_assets.overridden_indices(),
                    cancel,
                )
                .await?;
        }
        subtitle_assets
            .verify(
                &discover("ffmpeg", cancel).await?,
                &temp.path,
                &selected,
                cancel,
            )
            .await?;
        external.verify_metadata_overrides(
            settings,
            &selected,
            &artifact,
            container::staging_format(&selected, Some(&mux_settings)),
        )?;
        for (position, stream) in selected.iter().enumerate() {
            if let Some((track, timeline)) = timelines
                .iter()
                .find(|(track, _)| track.stream_index == stream.index)
            {
                if source_copy && track.normalization.is_some() {
                    continue;
                }
                let decoded = audio::scan(
                    &ffprobe,
                    &temp.path,
                    &artifact.streams[position],
                    true,
                    cancel,
                )
                .await?;
                audio::verify_timeline(
                    &timeline.shifted(copy_origin.unwrap_or_else(|| {
                        verified_trim.as_ref().map_or(0.0, |(_, origin)| *origin)
                    })),
                    &decoded,
                    track.codec,
                )?;
            } else if !source_copy
                && !subtitle_assets.overridden_indices().contains(&stream.index)
                && !prepared_trim.as_ref().is_some_and(|prepared| {
                    prepared.subtitle_indices().contains(&stream.index)
                        || prepared.copied_indices().contains(&stream.index)
                })
                && source_selected
                    .iter()
                    .any(|primary| primary.index == stream.index)
                && matches!(
                    stream.codec_type.as_deref(),
                    Some("audio" | "video" | "subtitle" | "data")
                )
            {
                let (path, index) = external
                    .audio_source(stream.index)
                    .map(|(path, stream)| (path, stream.index))
                    .unwrap_or((source.path.as_path(), stream.index));
                if stream.codec_type.as_deref() == Some("video") {
                    super::mux::verify_shifted_video_packets(
                        &ffprobe,
                        path,
                        index,
                        &temp.path,
                        position as u32,
                        external.offset_seconds(stream.index),
                        cancel,
                    )
                    .await
                } else {
                    super::mux::verify_shifted_packets(
                        &ffprobe,
                        path,
                        index,
                        &temp.path,
                        position as u32,
                        external.offset_seconds(stream.index),
                        cancel,
                    )
                    .await
                }?;
            }
        }
        let mut excluded = prepared_trim
            .as_ref()
            .map_or_else(Vec::new, |prepared| prepared.subtitle_indices());
        if let Some(prepared) = &prepared_trim {
            excluded.extend(prepared.copied_indices());
        }
        excluded.extend_from_slice(subtitle_assets.overridden_indices());
        if !source_copy {
            external
                .verify_packets_excluding(&ffprobe, &temp.path, &selected, &excluded, cancel)
                .await?;
        }
        source.verify()?;
        external.verify()?;
        check_cancel(cancel)?;
        if let Some(original) = original {
            original.verify()?;
        }
        if source_copy {
            let actual =
                super::command_edit::verify_actual_output(&ffmpeg, &ffprobe, temp, cancel).await?;
            self.change(id,|snapshot|{snapshot.duration_seconds=actual.duration();if timelines.iter().any(|(track,_)|track.normalization.is_some()){append_log(snapshot,"Normalized audio matched the complete source-equivalent measured-filter packet/header, decoded sample clock and PCM proof.".into());}}).await;
            return self
                .publish_verified(id, cancel, &source, Some(&external), temp, &output)
                .await;
        }
        self.finalize(
            id,
            cancel,
            &source,
            Some(&external),
            temp,
            &output,
            None,
            None,
            scratch,
        )
        .await
    }
}
