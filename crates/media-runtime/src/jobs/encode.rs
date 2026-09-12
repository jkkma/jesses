pub(super) use super::encode_plan::validate_settings;
use super::encode_plan::{Plan, Validation};
#[path = "frame_scan.rs"]
pub(super) mod frame_scan;
#[path = "x264.rs"]
mod x264;
use super::*;
use media_core::VideoEncoder;

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
        let input = PathBuf::from(&request.input_path);
        let owned_request = request.clone();
        let (source, output) = tokio::task::spawn_blocking(move || {
            let source = Source::open(&input)?;
            let output = files::output_path(&owned_request, &source)?;
            Ok::<_, AppError>((source, output))
        })
        .await
        .map_err(|e| AppError::new("PREFLIGHT_FAILED", e.to_string(), None))??;
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
        for (path, version_arg) in [
            (&ffmpeg, "-version"),
            (&ffprobe, "-version"),
            (&encoder, "--version"),
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
        let document = probe(
            &ffprobe,
            &source.path,
            cancel,
            Some(&request.stream_indices),
        )
        .await?;
        let selected = document.selected(&request.stream_indices)?;
        let mut plan = Plan::build(&document, &selected, settings)?;
        audio::check_encoders(&ffmpeg, settings, cancel).await?;
        if audio::converted(settings).next().is_some() {
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
        for track in audio::converted(settings) {
            let source_track = selected
                .iter()
                .find(|stream| stream.index == track.stream_index)
                .expect("validated audio selection");
            self.phase(
                id,
                JobState::Preparing,
                "Decoding selected audio to verify its complete sample timeline.",
            )
            .await;
            let timeline = audio::scan(&ffprobe, &source.path, source_track, false, cancel).await?;
            audio_timelines.push((track, timeline));
        }
        if settings.backend == media_core::EncodeBackend::Av1an {
            super::av1an::validate_encoder_path(&encoder)?;
            super::av1an::validate_input(&document, &plan)?;
        }
        if settings.encoder == VideoEncoder::X264 {
            check_encoder_capabilities(&encoder, &plan, settings, cancel).await?;
        }
        let video = selected
            .iter()
            .find(|stream| stream.index == plan.video_index)
            .expect("validated video selection");
        self.change(id, |snapshot| {
            snapshot.progress_seconds = Some(0.0);
            snapshot.duration_seconds = document.duration().filter(|duration| *duration > 0.0);
            append_log(snapshot, "Decoding the complete source to verify every frame timestamp, progressive scan, color, and HDR metadata with bounded memory.".into());
        }).await;
        let declared_rate = (plan.fps_num, plan.fps_den);
        let frame_count = self
            .scan_frames(id, &ffprobe, &source.path, &mut plan, video, false, cancel)
            .await?;
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
            check_encoder_capabilities(&encoder, &plan, settings, cancel).await?;
        }
        source.verify()?;
        check_cancel(cancel)?;
        self.change(id,|snapshot| {
            snapshot.duration_seconds = Some(frame_count as f64*plan.frame_seconds());
            snapshot.progress_seconds = None;
            append_log(snapshot,format!("Validated {frame_count} progressive CFR frames at {}/{} fps; {}-bit {}, CRF {}, preset {}.",plan.fps_num,plan.fps_den,plan.output_bit_depth(),plan.output_codec(),settings.crf,settings.preset));
            if settings.encoder.is_svt() {
                append_log(snapshot, format!("Film grain synthesis {} (denoising off).", settings.film_grain));
            }
            match settings.encoder {
                VideoEncoder::SvtAv1FiveFish => append_log(snapshot, format!("SVT-AV1 5fish: line-art bias {}, texture bias {}.", settings.lineart_psy_bias, settings.texture_psy_bias)),
                VideoEncoder::SvtAv1Hdr => append_log(snapshot, format!("SVT-AV1-HDR: {} tune.", match settings.hdr_tune { media_core::HdrTune::VisualQuality => "visual quality (0)", media_core::HdrTune::FilmGrain => "film grain retention (5)" })),
                _ => {},
            }
            if plan.is_hdr10() {
                append_log(snapshot, "HDR10: preserving BT.2020/PQ color and validated static mastering/content light metadata.".into());
                if settings.hdr10_fallback { append_log(snapshot, "HDR10 fallback enabled: Dolby Vision enhancement data and HDR10+ dynamic metadata are discarded.".into()); }
            }
        }).await;
        tokio::fs::create_dir_all(self.log_dir.as_ref())
            .await
            .map_err(|e| files::error("LOG_CREATE_FAILED", e.to_string(), self.log_dir.as_ref()))?;
        let mut recovery = None;
        let mut av1an_executable = None;
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
            let (workspace, intermediate) = av1an::Recovery::prepare(
                id,
                request,
                settings,
                &source.path,
                vec![
                    ffmpeg.clone(),
                    ffprobe.clone(),
                    encoder.clone(),
                    executable.clone(),
                ],
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
            recovery = Some(workspace);
            Some(intermediate)
        } else {
            None
        };
        *temporary = Some(Temporary::create(&output, &attempt_id)?);
        if durable.is_none() {
            scratch.push(match settings.encoder {
                VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
                    Temporary::create_ivf(&output, &format!("{attempt_id}-video"))?
                }
                VideoEncoder::X264 => Temporary::create(&output, &format!("{attempt_id}-video"))?,
            });
        }
        let temp = temporary.as_ref().expect("owned Matroska output");
        let intermediate = durable
            .as_ref()
            .or_else(|| scratch.last())
            .expect("owned encoded video intermediate");
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
        if settings.backend == media_core::EncodeBackend::Av1an {
            let recovery = recovery.as_ref().expect("av1an recovery workspace");
            if !recovery.finalizing {
                self.phase(id, JobState::Running, "av1an is detecting scenes and encoding parallel SVT-AV1 chunks; selected tracks will be copied afterward.").await;
                self.encode_av1an(
                    id,
                    &source.path,
                    &encoder,
                    av1an_executable.as_ref().expect("av1an executable"),
                    recovery,
                    intermediate,
                    &plan,
                    settings,
                    cancel,
                    log_path,
                    frame_count,
                )
                .await?;
                let summary = recovery.finalizing().await?;
                self.change(id, |snapshot| snapshot.recovery = Some(summary))
                    .await;
            } else {
                self.change(id,|snapshot|append_log(snapshot,"Reusing the verified durable AV1 intermediate; continuing output finalization.".into())).await;
            }
        } else {
            let producer = CommandSpec {
                executable: ffmpeg.clone(),
                args: decoder_args(&source.path, &plan),
                cwd: None,
            };
            let consumer = CommandSpec {
                executable: encoder,
                // Both native encoders write into the already-owned file handle
                // through stdout, preserving its identity guard and Unicode path.
                args: match settings.encoder {
                    VideoEncoder::SvtAv1
                    | VideoEncoder::SvtAv1FiveFish
                    | VideoEncoder::SvtAv1Hdr => encoder_args(Path::new("stdout"), &plan, settings),
                    VideoEncoder::X264 => x264::arguments(&plan, settings),
                },
                cwd: None,
            };
            self.phase(id, JobState::Running, match settings.encoder {
                VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => "Encoding 10-bit AV1 with the standalone SVT encoder; selected audio settings will be applied afterward.",
                VideoEncoder::X264 => "Encoding H.264 with standalone x264 at the source bit depth; selected audio settings will be applied afterward.",
            }).await;
            let (sender, events) = mpsc::channel(256);
            let event_task = self.observe_encode(id, events, Some(plan.frame_seconds()));
            let result = supervisor::run_pipeline_to_file(
                &producer,
                &consumer,
                cancel.clone(),
                sender,
                log_path,
                Duration::from_secs(24 * 60 * 60),
                intermediate.clone_file()?,
            )
            .await;
            let _ = event_task.await;
            result.map_err(|e| process_error(e, &source.path))?;
        }
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
        let (sender, events) = mpsc::channel(256);
        let observer = self.observe_encode(id, events, None);
        let result = supervisor::run(
            &CommandSpec {
                executable: ffmpeg,
                args: mux_args(
                    &source.path,
                    &intermediate.path,
                    &temp.path,
                    &selected,
                    &plan,
                    settings,
                ),
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
        self.phase(id,JobState::Finalizing,"Decoding the completed output to verify exact frame count and timing, then checking copied tracks and metadata.").await;
        let artifact = probe(&ffprobe, &temp.path, cancel, None).await?;
        metadata::verify_encoded(
            &document,
            &selected,
            &artifact,
            plan.video_index,
            plan.output_codec(),
            (plan.width, plan.height),
            &settings.audio,
        )?;
        for (track, timeline) in &audio_timelines {
            let position = selected
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
            let evidence = audio::verify_timeline(timeline, &decoded, track.codec)?;
            self.change(id, |snapshot| {
                append_log(
                    snapshot,
                    format!("Source audio stream {}: {evidence}", track.stream_index),
                )
            })
            .await;
        }
        let position = selected
            .iter()
            .position(|s| s.index == plan.video_index)
            .expect("selected video");
        let encoded = &artifact.streams[position];
        plan.validate_encoded_stream(video, encoded)?;
        self.change(id, |snapshot| snapshot.progress_seconds = Some(0.0))
            .await;
        let decoded_count = self
            .scan_frames(id, &ffprobe, &temp.path, &mut plan, encoded, true, cancel)
            .await?;
        if decoded_count != frame_count {
            return Err(AppError::new(
                "ENCODE_VALIDATION_FAILED",
                "The encoded video does not decode to exactly the original frame count.",
                None,
            ));
        }
        source.verify()?;
        check_cancel(cancel)?;
        self.finalize(id, cancel, &source, temp, &output).await?;
        drop(durable);
        if let Some(recovery) = recovery {
            match recovery.cleanup().await {
                Ok(()) => self.change(id, |snapshot| snapshot.recovery = None).await,
                Err(error) => {
                    self.change(id, |snapshot| {
                        append_log(
                            snapshot,
                            format!(
                                "Output succeeded; recovery files were retained: {}",
                                error.message
                            ),
                        )
                    })
                    .await
                }
            }
        }
        Ok(())
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

    fn observe_encode(
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
async fn frame_scan(
    ffprobe: &Path,
    input: &Path,
    plan: &mut Plan,
    stream: &metadata::Stream,
    encoded: bool,
    cancel: &watch::Receiver<bool>,
    progress: Option<watch::Sender<f64>>,
) -> Result<usize, AppError> {
    check_cancel(cancel)?;
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8);
    let args = frame_scan_args(input, stream.index, threads);
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

fn frame_scan_args(input: &Path, index: u32, threads: usize) -> Vec<OsString> {
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
    args.extend(["-show_frames","-show_entries","frame=best_effort_timestamp_time,interlaced_frame,width,height,pix_fmt,sample_aspect_ratio,chroma_location,color_space,color_transfer,color_primaries,color_range:frame_side_data=side_data_type,rotation,red_x,red_y,green_x,green_y,blue_x,blue_y,white_point_x,white_point_y,max_luminance,min_luminance,max_content,max_average,dv_profile,dv_bl_signal_compatibility_id,bl_present_flag,rpu_present_flag,el_present_flag,bl_bit_depth,bl_video_full_range_flag","-of","json","-i"].into_iter().map(OsString::from));
    args.push(input.as_os_str().to_owned());
    args
}

pub(super) fn decoder_args(input: &Path, plan: &Plan) -> Vec<OsString> {
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
    args.extend(["-map".into(), format!("0:{}", plan.video_index).into()]);
    if let Some(filter) = plan.framing_filter() {
        args.extend(["-vf".into(), filter.into()]);
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
            "-strict",
            "-1",
            "-f",
            "yuv4mpegpipe",
            "pipe:1",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args
}

pub(super) fn encoder_parameters(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let values = [
        "--input-depth".into(),
        "10".into(),
        "--crf".into(),
        settings.crf.to_string(),
        "--preset".into(),
        settings.preset.to_string(),
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
    args.extend(plan.hdr_arguments());
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
        VideoEncoder::SvtAv1Hdr => args.extend([
            "--tune".into(),
            match settings.hdr_tune {
                media_core::HdrTune::VisualQuality => "0",
                media_core::HdrTune::FilmGrain => "5",
            }
            .into(),
        ]),
        _ => {}
    }
    args
}

fn encoder_args(output: &Path, plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-i", "stdin", "--progress", "2", "--passes", "1"]
        .into_iter()
        .map(OsString::from)
        .collect();
    args.extend(encoder_parameters(plan, settings));
    args.push("-b".into());
    args.push(output.as_os_str().to_owned());
    args
}

async fn check_encoder_capabilities(
    encoder: &Path,
    plan: &Plan,
    settings: &EncodeSettings,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: encoder.to_owned(),
            args: vec![
                match settings.encoder {
                    VideoEncoder::SvtAv1
                    | VideoEncoder::SvtAv1FiveFish
                    | VideoEncoder::SvtAv1Hdr => "--help",
                    VideoEncoder::X264 => "--fullhelp",
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
    if settings.encoder == VideoEncoder::X264 {
        let capabilities = x264::validate_help(&help, plan.output_bit_depth());
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
    let mut required = vec!["--film-grain", "--film-grain-denoise"];
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
    if !result.status.success()
        || required
            .iter()
            .any(|option| !help.split_whitespace().any(|word| word == *option))
    {
        return Err(files::error(
            "ENCODER_CAPABILITY_UNSUPPORTED",
            format!(
                "The installed standalone SVT encoder does not advertise the required HDR10/film-grain options (grain level {}).",
                settings.film_grain
            ),
            encoder,
        ));
    }
    Ok(())
}

fn mux_args(
    input: &Path,
    ivf: &Path,
    output: &Path,
    selected: &[&metadata::Stream],
    plan: &Plan,
    settings: &EncodeSettings,
) -> Vec<OsString> {
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
    if audio::converted(settings).next().is_some() {
        // Codec delay can create negative packet DTS. Keep source timestamps
        // and prohibit the muxer from shifting every track to compensate.
        args.insert(0, "-copyts".into());
    }
    args.push("-i".into());
    args.push(ivf.as_os_str().to_owned());
    for stream in selected {
        args.extend([
            "-map".into(),
            if stream.index == plan.video_index {
                "1:v:0".into()
            } else {
                format!("0:{}", stream.index).into()
            },
        ]);
    }
    args.extend(
        ["-map_metadata", "0", "-map_chapters", "0", "-c", "copy"]
            .into_iter()
            .map(OsString::from),
    );
    for (index, stream) in selected.iter().enumerate() {
        let converted_audio =
            audio::converted(settings).find(|track| track.stream_index == stream.index);
        if let Some(track) = converted_audio {
            audio::append_arguments(&mut args, index, track, stream);
        }
        args.extend([
            format!("-map_metadata:s:{index}").into(),
            format!("0:s:{}", stream.index).into(),
        ]);
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
        if stream.index == plan.video_index {
            args.extend([
                format!("-aspect:{index}").into(),
                format!("{}:{}", plan.width, plan.height).into(),
            ]);
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
    if audio::converted(settings).next().is_some() {
        args.extend(["-avoid_negative_ts".into(), "disabled".into()]);
    }
    args.extend(["-f", "matroska", "-y"].into_iter().map(OsString::from));
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
}
