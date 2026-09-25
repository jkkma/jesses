//! A review uses the real preparation and argument builders. Temporary assets
//! belong to this preview and are removed before returning; execution always
//! revalidates sources/tools and allocates fresh paths.
use super::*;
use media_core::{EncodeCommandPlan, EncodeCommandStage};

pub async fn preview_encode_plan(
    request: EncodeRequest,
    mut cancel: watch::Receiver<bool>,
) -> Result<EncodeCommandPlan, AppError> {
    let _permit = crate::analysis::permit(&cancel).await?;
    files::validate_request(&request.source)?;
    validate_settings(&request.settings)?;
    let source = Source::open(Path::new(&request.source.input_path))?;
    let output = files::output_path(&request.source, &source)?;
    let id = format!(
        "plan-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    );
    let directory = super::super::rate_control::Stats::create(&output, &id)?;
    let manager = JobManager::new(directory.path.join("logs"));
    let (owner, local_cancel) = watch::channel(*cancel.borrow());
    let mut plan = EncodeCommandPlan {
        request: request.clone(),
        source_fingerprint: String::new(),
        output_frame_count: String::new(),
        output_frame_rate: String::new(),
        stages: Vec::new(),
        notes: Vec::new(),
    };
    let mut temporary = None;
    let mut scratch = Vec::new();
    let log = directory.path.join("preview.log");
    let mut work = Box::pin(manager.encode_mode(
        &id,
        &request.source,
        &request.settings,
        &local_cancel,
        &log,
        &mut temporary,
        &mut scratch,
        Some(&mut plan),
    ));
    let outcome = tokio::select! {
        result=&mut work => result,
        _=async { let _=cancel.wait_for(|value|*value).await; } => { owner.send_replace(true); let _=work.as_mut().await; Err(AppError::new("JOB_CANCELED","Command preview canceled.",None)) },
        _=tokio::time::sleep(Duration::from_secs(30*60)) => { owner.send_replace(true); let _=work.as_mut().await; Err(AppError::new("COMMAND_PREVIEW_TIMEOUT","Command preview reached its 30-minute bound.",None)) },
    };
    drop(work);
    outcome?;
    source.verify()?;
    check_cancel(&cancel)?;
    Ok(plan)
}

fn stage(label: impl Into<String>, command: CommandSpec, notes: Vec<String>) -> EncodeCommandStage {
    EncodeCommandStage {
        label: label.into(),
        executable: command.executable.to_string_lossy().into_owned(),
        arguments: command
            .args
            .into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect(),
        working_directory: command.cwd.map(|path| path.to_string_lossy().into_owned()),
        notes,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn build(
    preview: &mut EncodeCommandPlan,
    source: &Source,
    output: &Path,
    ffmpeg: &Path,
    ffprobe: &Path,
    encoder: &Path,
    plan: &Plan,
    settings: &EncodeSettings,
    selected: &[&metadata::Stream],
    external: &super::super::external_tracks::ExternalTracks,
    trim: Option<&super::super::trim::Prepared>,
    source_interval: Option<(u32, u32)>,
    qtgmc: Option<&super::super::qtgmc::Prepared>,
    subtitles: &super::super::subtitles::Prepared,
    audio_filters: &std::collections::BTreeMap<u32, String>,
    rate: Option<&super::super::rate_control::Rate>,
    frames: usize,
    id: &str,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    check_cancel(cancel)?;
    let temporary = Temporary::create(output, &format!("{id}-plan-mux"))?;
    let video = if settings.encoder.is_svt()
        || matches!(
            settings.encoder,
            VideoEncoder::AomAv1 | VideoEncoder::VpxStandalone
        ) {
        Temporary::create_ivf(output, &format!("{id}-plan-video"))?
    } else if settings.encoder == VideoEncoder::X265Standalone {
        Temporary::create_extension(output, &format!("{id}-plan-video"), "hevc")?
    } else {
        Temporary::create(output, &format!("{id}-plan-video"))?
    };
    preview.source_fingerprint = crate::analysis::fingerprint(source)?;
    preview.output_frame_count = frames.to_string();
    preview.output_frame_rate = format!("{}/{}", plan.fps_num, plan.fps_den);
    preview.notes=vec![
        "The complete source and selected converted-audio timelines were validated. Any requested subtitle assets and target-size measurements were prepared using the normal execution path.".into(),
        "Arguments are separate native argv values; the application never evaluates them through a shell. This preview's temporary files are removed before it returns. Execution creates fresh owned paths and repeats validation.".into(),
        "Validation probes and font/subtitle preparation are additional supervised steps. After encoding, every output frame, audio timeline, selected track and metadata is checked before atomic no-overwrite publication.".into(),
    ];
    if settings.lossless {
        preview.notes.push("Before publication, execution compares the complete decoded pixel stream with the processed encoder input. A mismatch fails the job even when the encoder reports lossless coding; try another preset or encoder.".into());
    }
    if !external.is_empty() {
        preview.notes.push("Additional audio, subtitles and attachments come from their displayed input files. Selected audio conversions preserve the shifted decoded sample timeline; copied tracks preserve packet contents and their requested timing offsets. The primary source owns video processing; container metadata and chapters use their selected donors. Every additional source is verified before publication and bound to recovery.".into());
    }
    preview.stages.push(stage("Source frame validation",CommandSpec{executable:ffprobe.to_owned(),args:frame_scan_args(&source.path,plan.video_index,std::thread::available_parallelism().map_or(1,usize::from).min(8)),cwd:None},vec!["Already completed for this preview, including source cadence and per-frame color/field validation.".into()]));
    if let Some(qtgmc) = qtgmc {
        preview.stages.push(stage(
            "QTGMC frameserver",
            qtgmc.producer.clone(),
            vec!["VSPipe decodes the validated source interval, applies QTGMC once, and sends Y4M through an OS pipe. Its generated script and source index live in a private owned workspace.".into()],
        ));
        preview.stages.push(stage(
            "QTGMC lossless intermediate",
            qtgmc.consumer.clone(),
            vec![format!(
                "FFmpeg applies the remaining cadence, tone, framing and aspect transforms and writes a verified FFV1 intermediate to {}. Every encoder pass reuses this exact file.",
                qtgmc.video.path.display()
            )],
        ));
    }
    if settings.backend == media_core::EncodeBackend::Av1an {
        let executable = discover("av1an", cancel).await?;
        let work = super::super::rate_control::Stats::create(output, &format!("{id}-plan-av1an"))?;
        let log = work.path.join("av1an.log");
        let prepared = if qtgmc.is_none() && plan.requires_av1an_preprocess() {
            if subtitles.bitmap_input().is_some() {
                return Err(AppError::new(
                    "AV1AN_PREPROCESS_SUBTITLE_UNSUPPORTED",
                    "av1an cannot burn a bitmap subtitle while preparing its verified lossless processed source. Copy the subtitle track or use standalone encoding.",
                    None,
                ));
            }
            let prepared = super::super::av1an_preprocess::Prepared::build(
                &source.path,
                output,
                id,
                plan,
                ffmpeg,
                source_interval,
                subtitles.text_filter(),
                cancel,
            )
            .await?;
            preview.stages.push(stage(
                "Av1an lossless processed source",
                prepared.producer.clone(),
                vec![format!("FFmpeg sends FFV1 Matroska through stdout to the reserved file {}. Execution verifies every decoded frame, timestamp, pixel format, color and sample aspect, then fingerprints the complete decoded pixels before recovery admission.", prepared.video.path.display())],
            ));
            Some(prepared)
        } else {
            None
        };
        let uses_prepared = qtgmc.is_some() || prepared.is_some();
        let av1an_input = if uses_prepared {
            preview.notes.push("Execution copies the verified lossless source to prepared.mkv in the owned recovery workspace. Scene detection, chunks and quality references read this same source; original selected audio, subtitles, metadata and attachments remain the final mux source. The copy and decoded checks are internal supervised work, not shell commands.".into());
            work.path.join("prepared.mkv")
        } else {
            source.path.clone()
        };
        let source_filter = (!uses_prepared).then(|| plan.decoder_filter()).flatten();
        let arguments = super::super::av1an::arguments(
            &av1an_input,
            &video.path,
            &work.path,
            &log,
            plan,
            settings,
            false,
            source_filter.as_deref(),
        );
        if settings
            .av1an_grain
            .as_ref()
            .is_some_and(|grain| grain.table.is_some())
        {
            preview.notes.push("The immutable grain table from the saved settings is staged as jesses-grain.tbl in the owned AV1AN workspace and verified again for recovery.".into());
        }
        preview.stages.push(stage("Av1an scene/chunk encoding",CommandSpec{executable,args:arguments,cwd:Some(work.path.clone())},vec![format!("The selected child encoder is {}. Execution stages its managed launcher and compatible dependency environment before starting av1an.",encoder.display()),"Chunk commands and quality probes are generated by av1an from this same validated video-parameter vector. Installed plugin/engine checks run again at execution.".into()]));
    } else {
        let two_pass = rate.is_some_and(|rate| rate.two_pass);
        let stats = two_pass
            .then(|| super::super::rate_control::Stats::create(output, &format!("{id}-plan-stats")))
            .transpose()?;
        for pass in 1..=if two_pass { 2 } else { 1 } {
            let mut producer = CommandSpec {
                executable: ffmpeg.to_owned(),
                args: if let Some(qtgmc) = qtgmc {
                    decoder_args_preprocessed(&qtgmc.video.path, plan)
                } else {
                    decoder_args_with_subtitles(
                        &source.path,
                        plan,
                        subtitles.text_filter(),
                        subtitles.bitmap_input(),
                    )
                },
                cwd: if qtgmc.is_some() {
                    None
                } else {
                    subtitles.decoder_cwd().map(Path::to_path_buf)
                },
            };
            let raw_aom_input =
                aom_vpx::configure_producer_output(&mut producer.args, plan, settings);
            let mut args = consumer_arguments(plan, settings);
            if let Some(rate) = rate {
                rate.arguments(&mut args, settings.encoder, pass);
            }
            let consumer = CommandSpec {
                executable: encoder.to_owned(),
                args,
                cwd: stats.as_ref().map(|stats| stats.path.clone()),
            };
            preview.stages.push(stage(
                format!("Pass {pass}: source decoder"),
                producer,
                vec![
                    if raw_aom_input {
                        "A fresh producer sends headerless 10-bit I420 to AOM through an OS pipe. Geometry, cadence, depth, and chroma placement are explicit encoder arguments."
                            .into()
                    } else {
                        "A fresh producer sends Y4M to the following encoder through an OS pipe."
                            .into()
                    },
                ],
            ));
            preview.stages.push(stage(
                format!("Pass {pass}: video encoder"),
                consumer,
                vec![
                    format!(
                        "Encoder stdout is attached to an owned output handle{}.",
                        if two_pass && pass == 1 {
                            " for the first pass"
                        } else {
                            " for the video intermediate"
                        }
                    ),
                    format!(
                        "Final video intermediate for this preview: {}",
                        video.path.display()
                    ),
                ],
            ));
        }
    }
    let wrapped_video = if matches!(
        settings.encoder,
        VideoEncoder::X265Standalone | VideoEncoder::VpxStandalone
    ) {
        let timed = Temporary::create(output, &format!("{id}-plan-timed"))?;
        let executable = discover("mkvmerge", cancel).await?;
        preview.stages.push(stage(
            "Standalone rational timing wrapper",
            CommandSpec {
                executable,
                args: x265::mkvmerge_arguments(&video.path, &timed.path, plan),
                cwd: None,
            },
            vec!["mkvmerge assigns the validated rational cadence to the standalone video intermediate, including HEVC presentation order and VP9 IVF rate normalization, before the common selected-track mux.".into()],
        ));
        Some(timed)
    } else {
        None
    };
    let mux_video = wrapped_video.as_ref().unwrap_or(&video);
    let mut args = mux_args(
        &source.path,
        &mux_video.path,
        &temporary.path,
        selected,
        plan,
        settings,
        external,
    );
    if let Some(trim) = trim {
        trim.apply_mux(&mut args, selected, audio_filters)?;
    }
    subtitles.apply_mux(&mut args, selected);
    let attachment =
        super::encode_settings::SettingsAttachment::prepare(&temporary.path, id, settings)?;
    if let Some(attachment) = &attachment {
        attachment.apply(
            &mut args,
            selected
                .iter()
                .filter(|stream| stream.codec_type.as_deref() == Some("attachment"))
                .count(),
        );
        preview.notes.push("The final Matroska file includes jesses-encode-settings.json. Its complete bytes are verified alongside the selected source attachments.".into());
    }
    preview.stages.push(stage("Selected tracks and metadata: Matroska stage",CommandSpec{executable:ffmpeg.to_owned(),args,cwd:None},vec!["The exact selected stream mappings, converted audio settings, prepared subtitles and chapter mappings are generated by the same final-mux builder used for execution.".into()]));
    let format = output
        .extension()
        .and_then(|value| value.to_str())
        .and_then(media_core::ContainerFormat::from_extension)
        .expect("validated output container");
    if format != media_core::ContainerFormat::Matroska {
        preview.notes.push(format!("The final {:?} conversion is constructed after the validated Matroska stage exists: exact track headers, DTS reconstruction, subtitle conversion and codec VUI requirements depend on its actual packets. Its command is not fabricated in this preview. The final destination is {}.",format,output.display()));
    } else {
        preview.notes.push(format!(
            "After verification, the owned Matroska stage is atomically published to {}.",
            output.display()
        ));
    }
    source.verify()?;
    external.verify()?;
    check_cancel(cancel)
}
