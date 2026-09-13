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
    trim: Option<&super::super::trim::Prepared>,
    subtitles: &super::super::subtitles::Prepared,
    audio_filters: &std::collections::BTreeMap<u32, String>,
    rate: Option<&super::super::rate_control::Rate>,
    frames: usize,
    id: &str,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    check_cancel(cancel)?;
    let temporary = Temporary::create(output, &format!("{id}-plan-mux"))?;
    let video = if settings.encoder.is_svt() {
        Temporary::create_ivf(output, &format!("{id}-plan-video"))?
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
    preview.stages.push(stage("Source frame validation",CommandSpec{executable:ffprobe.to_owned(),args:frame_scan_args(&source.path,plan.video_index,std::thread::available_parallelism().map_or(1,usize::from).min(8)),cwd:None},vec!["Already completed for this preview, including source cadence and per-frame color/field validation.".into()]));
    if settings.backend == media_core::EncodeBackend::Av1an {
        let executable = discover("av1an", cancel).await?;
        let work = super::super::rate_control::Stats::create(output, &format!("{id}-plan-av1an"))?;
        let log = work.path.join("av1an.log");
        let arguments = super::super::av1an::arguments(
            &source.path,
            &video.path,
            &work.path,
            &log,
            plan,
            settings,
            false,
        );
        preview.stages.push(stage("Av1an scene/chunk encoding",CommandSpec{executable,args:arguments,cwd:Some(work.path.clone())},vec![format!("The selected child encoder is {}. Execution stages its managed launcher and compatible dependency environment before starting av1an.",encoder.display()),"Chunk commands and quality probes are generated by av1an from this same validated video-parameter vector. Installed plugin/engine checks run again at execution.".into()]));
    } else {
        let two_pass = rate.is_some_and(|rate| rate.two_pass);
        let stats = two_pass
            .then(|| super::super::rate_control::Stats::create(output, &format!("{id}-plan-stats")))
            .transpose()?;
        for pass in 1..=if two_pass { 2 } else { 1 } {
            let producer = CommandSpec {
                executable: ffmpeg.to_owned(),
                args: decoder_args_with_subtitles(
                    &source.path,
                    plan,
                    subtitles.text_filter(),
                    subtitles.bitmap_index(),
                ),
                cwd: subtitles.decoder_cwd().map(Path::to_path_buf),
            };
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
                    "A fresh producer sends Y4M to the following encoder through an OS pipe."
                        .into(),
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
    let mut args = mux_args(
        &source.path,
        &video.path,
        &temporary.path,
        selected,
        plan,
        settings,
    );
    if let Some(trim) = trim {
        trim.apply_mux(&mut args, selected, audio_filters)?;
    }
    subtitles.apply_mux(&mut args, selected);
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
    check_cancel(cancel)
}
