use media_core::{
    AppError, AutoCropRequest, AutoCropResult, BatchEncodePreview, BatchEncodeRequest,
    BitrateRequest, BitrateResult, EncodeRequest, FolderScanRequest, FolderScanResult,
    FramePreviewRequest, FramePreviewResult, JobSnapshot, LoudnessRequest, LoudnessResult,
    MediaFile, RemuxRequest, ToolInfo,
};
use media_runtime::jobs::JobManager;
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::Duration;
use tauri::{Emitter, Manager, State, ipc::Channel};

mod analysis;
mod completion;
mod paths;
mod ui_actions;
use ui_actions::{open_directory, save_activity_log};

#[tauri::command]
fn estimate_av1an_resources(
    request: media_core::Av1anResourceRequest,
) -> media_core::Av1anResourceEstimate {
    media_runtime::estimate_av1an_resources(request)
}

#[tauri::command]
async fn read_av1an_grain_table(path: String) -> Result<String, AppError> {
    tauri::async_runtime::spawn_blocking(move || media_runtime::read_av1an_grain_table(path))
        .await
        .map_err(|e| AppError::new("GRAIN_TABLE_UNREADABLE", e.to_string(), None))?
}

#[tauri::command]
async fn extract_attachment(
    id: String,
    input_path: String,
    stream_index: u32,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<String, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::jobs::extract_attachment(input_path, stream_index, running.cancel.clone()).await
}

#[tauri::command]
async fn resolve_deinterlace(
    id: String,
    request: media_core::DeinterlaceExportRequest,
    automatic: bool,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::DeinterlaceResolution, AppError> {
    let running = tasks.run(&id)?;
    Box::pin(media_runtime::resolve_deinterlace(
        request,
        automatic,
        running.cancel.clone(),
    ))
    .await
}

#[tauri::command]
async fn make_av1an_grain_preset(
    id: String,
    request: String,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<String, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::make_av1an_grain_preset(request, running.cancel.clone()).await
}

#[tauri::command]
async fn run_utility(
    id: String,
    mut request: media_core::UtilityRequest,
    app: tauri::AppHandle,
    tasks: State<'_, analysis::AnalysisTasks>,
    store: State<'_, Preferences>,
) -> Result<media_core::UtilityResult, AppError> {
    let running = tasks.run(&id)?;
    let _outputs = media_runtime::prepare_utility_outputs(
        std::slice::from_mut(&mut request),
        &store.get()?.general.default_output_directory,
    )?;
    media_runtime::supervisor::with_pause(
        running.pause.clone(),
        media_runtime::with_utility_progress(
            move |progress| {
                let _ = app.emit(
                    "utility-progress",
                    serde_json::json!({"id": &id, "progress": progress}),
                );
            },
            media_runtime::run_utility(request, running.cancel.clone()),
        ),
    )
    .await
}
#[tauri::command]
async fn run_utility_batch(
    id: String,
    mut request: media_core::UtilityBatchRequest,
    app: tauri::AppHandle,
    tasks: State<'_, analysis::AnalysisTasks>,
    store: State<'_, Preferences>,
) -> Result<media_core::UtilityBatchResult, AppError> {
    let running = tasks.run(&id)?;
    let _outputs = media_runtime::prepare_utility_outputs(
        &mut request.requests,
        &store.get()?.general.default_output_directory,
    )?;
    media_runtime::supervisor::with_pause(
        running.pause.clone(),
        media_runtime::with_utility_progress(
            move |progress| {
                let _ = app.emit(
                    "utility-progress",
                    serde_json::json!({"id": &id, "progress": progress}),
                );
            },
            media_runtime::run_utility_batch(request, running.cancel.clone()),
        ),
    )
    .await
}
#[tauri::command]
fn set_media_analysis_paused(
    id: String,
    paused: bool,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<(), AppError> {
    tasks.set_paused(&id, paused)
}
#[tauri::command]
async fn inspect_utility_capabilities(
    id: String,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::UtilityCapabilities, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::inspect_utility_capabilities(running.cancel.clone()).await
}
#[tauri::command]
async fn inspect_saved_job(
    path: String,
    svt_build: Option<media_core::VideoEncoder>,
) -> Result<media_core::SavedJobInspection, AppError> {
    media_runtime::inspect_saved_job_with_encoder(path, svt_build).await
}
#[tauri::command]
async fn export_saved_job(
    path: String,
    request: EncodeRequest,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<String, AppError> {
    let _admission = monitor.admit_work().await?;
    tauri::async_runtime::spawn_blocking(move || media_runtime::export_saved_job(path, request))
        .await
        .map_err(|e| AppError::new("SAVED_JOB_WRITE_FAILED", e.to_string(), None))?
}

#[tauri::command]
async fn run_image_job(
    id: String,
    request: media_core::ImageRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::ImageResult, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::jobs::run_image_job(request, running.cancel.clone()).await
}
#[tauri::command]
async fn resolve_output_path(
    request: media_core::OutputPathRequest,
    store: State<'_, Preferences>,
) -> Result<media_core::OutputPathResolution, AppError> {
    let mut request = request;
    if request.default_output_directory.is_none() {
        request.default_output_directory = Some(store.get()?.general.default_output_directory);
    }
    media_runtime::resolve_output_path(request).await
}
#[tauri::command]
async fn resolve_batch_output_paths(
    mut request: media_core::BatchOutputPathRequest,
    id: Option<String>,
    store: State<'_, Preferences>,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::BatchOutputPaths, AppError> {
    if request.default_output_directory.is_none() {
        request.default_output_directory = Some(store.get()?.general.default_output_directory);
    }
    if let Some(id) = id {
        let running = tasks.run(&id)?;
        media_runtime::resolve_batch_output_paths_cancellable(request, running.cancel.clone()).await
    } else {
        media_runtime::resolve_batch_output_paths(request).await
    }
}
#[tauri::command]
async fn prepare_command_edit(
    id: String,
    mut request: media_core::CommandEditRequest,
    jobs: State<'_, Jobs>,
    store: State<'_, Preferences>,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::CommandEditReview, AppError> {
    let running = tasks.run(&id)?;
    match &mut request {
        media_core::CommandEditRequest::Encode { request } => {
            apply_encoding_preferences(request, &store)?
        }
        media_core::CommandEditRequest::ResumeNew { settings, .. }
            if settings.encoding_preferences.is_none() =>
        {
            settings.encoding_preferences = Some((&store.get()?.general).into())
        }
        _ => {}
    }
    jobs.manager
        .prepare_command_edit(request, running.cancel.clone())
        .await
}
#[tauri::command]
async fn confirm_command_edit(
    id: String,
    request: media_core::CommandEditConfirmation,
    jobs: State<'_, Jobs>,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::CommandEditResult, AppError> {
    let running = tasks.run(&id)?;
    jobs.manager
        .confirm_command_edit(request, running.cancel.clone())
        .await
}
#[tauri::command]
async fn cancel_command_edit(id: String, jobs: State<'_, Jobs>) -> Result<(), AppError> {
    jobs.manager.cancel_command_edit(&id).await
}
#[tauri::command]
async fn set_command_edit_paused(
    id: String,
    paused: bool,
    jobs: State<'_, Jobs>,
) -> Result<(), AppError> {
    jobs.manager.set_command_edit_paused(&id, paused).await
}
#[tauri::command]
async fn set_completion_options(
    options: media_core::CompletionOptions,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<media_core::CompletionStatus, AppError> {
    let _admission = monitor.admit_work().await?;
    monitor.configure(options, &jobs.manager.list_jobs().await)
}
#[tauri::command]
fn get_completion_status(
    monitor: State<'_, completion::CompletionMonitor>,
) -> media_core::CompletionStatus {
    monitor.status()
}
#[tauri::command]
fn cancel_finish_action(
    monitor: State<'_, completion::CompletionMonitor>,
) -> media_core::CompletionStatus {
    monitor.cancel();
    monitor.status()
}

#[tauri::command]
async fn export_analysis(
    request: media_core::AnalysisExportRequest,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<String, AppError> {
    let _admission = monitor.admit_work().await?;
    tauri::async_runtime::spawn_blocking(move || media_runtime::jobs::export_analysis(request))
        .await
        .map_err(|e| AppError::new("ANALYSIS_EXPORT_FAILED", e.to_string(), None))?
}

type Preferences = Arc<media_runtime::preferences::PreferencesStore>;
fn apply_encoding_preferences(
    request: &mut EncodeRequest,
    store: &Preferences,
) -> Result<(), AppError> {
    if request.settings.encoding_preferences.is_none() {
        request.settings.encoding_preferences = Some((&store.get()?.general).into());
    }
    Ok(())
}
async fn with_preferences<T: Send + 'static>(
    store: Preferences,
    action: impl FnOnce(&media_runtime::preferences::PreferencesStore) -> Result<T, AppError>
    + Send
    + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(move || action(&store))
        .await
        .map_err(|e| AppError::new("PREFERENCES_FAILED", e.to_string(), None))?
}
#[tauri::command]
async fn get_preferences(
    store: State<'_, Preferences>,
) -> Result<media_core::UserPreferences, AppError> {
    with_preferences(store.inner().clone(), |store| store.get()).await
}
#[tauri::command]
async fn save_preferences(
    request: media_core::SavePreferencesRequest,
    store: State<'_, Preferences>,
) -> Result<media_core::UserPreferences, AppError> {
    with_preferences(store.inner().clone(), move |store| store.save(request)).await
}
#[tauri::command]
async fn get_parameter_presets(
    store: State<'_, Preferences>,
) -> Result<Vec<media_core::EncoderParameterPreset>, AppError> {
    with_preferences(store.inner().clone(), |store| store.parameter_presets()).await
}
#[tauri::command]
async fn save_parameter_preset(
    request: media_core::EncoderParameterPreset,
    store: State<'_, Preferences>,
) -> Result<Vec<media_core::EncoderParameterPreset>, AppError> {
    with_preferences(store.inner().clone(), move |store| {
        store.save_parameter_preset(request)
    })
    .await
}
#[tauri::command]
async fn remove_parameter_preset(
    request: media_core::EncoderParameterPresetKey,
    store: State<'_, Preferences>,
) -> Result<Vec<media_core::EncoderParameterPreset>, AppError> {
    with_preferences(store.inner().clone(), move |store| {
        store.remove_parameter_preset(request)
    })
    .await
}
#[tauri::command]
async fn remember_recent_media(
    paths: Vec<String>,
    store: State<'_, Preferences>,
) -> Result<media_core::UserPreferences, AppError> {
    with_preferences(store.inner().clone(), move |store| store.remember(paths)).await
}
#[tauri::command]
async fn preview_preference_import(
    path: String,
    store: State<'_, Preferences>,
) -> Result<media_core::PreferenceImportPreview, AppError> {
    with_preferences(store.inner().clone(), move |store| {
        store.preview_import(path.into())
    })
    .await
}
#[tauri::command]
async fn recent_path_is_folder(path: String) -> Result<bool, AppError> {
    tauri::async_runtime::spawn_blocking(move || {
        media_runtime::preferences::recent_path_is_folder(path)
    })
    .await
    .map_err(|e| AppError::new("RECENT_MEDIA_UNAVAILABLE", e.to_string(), None))?
}

#[tauri::command]
fn get_storage_locations(paths: State<'_, paths::AppPaths>) -> Vec<(String, String)> {
    vec![
        ("Storage mode".into(), paths.mode.as_str().into()),
        (
            "Application resources".into(),
            paths.resource_dir.display().to_string(),
        ),
        ("Preferences".into(), paths.config_dir.display().to_string()),
        (
            "Job history".into(),
            paths.history_dir().display().to_string(),
        ),
        ("Cache".into(), paths.cache_dir.display().to_string()),
        ("Job logs".into(), paths.job_log_dir().display().to_string()),
    ]
}

#[tauri::command]
async fn begin_media_analysis(
    tasks: State<'_, analysis::AnalysisTasks>,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<String, AppError> {
    let _admission = monitor.admit_work().await?;
    tasks.begin()
}

#[tauri::command]
fn cancel_media_analysis(id: String, tasks: State<'_, analysis::AnalysisTasks>) {
    tasks.cancel(&id);
}

#[tauri::command]
async fn preview_frame(
    id: String,
    request: FramePreviewRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<FramePreviewResult, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::preview_frame(request, running.cancel.clone()).await
}

#[tauri::command]
async fn snap_keyframe(
    id: String,
    request: media_core::KeyframeSnapRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::KeyframeSnapResult, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::snap_keyframe(request, running.cancel.clone()).await
}

#[tauri::command]
async fn detect_fields(
    id: String,
    request: media_core::FieldDetectionRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::FieldDetectionResult, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::detect_fields(request, running.cancel.clone()).await
}

#[tauri::command]
async fn read_color_metadata(
    id: String,
    request: media_core::ColorMetadataInspectionRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::ColorMetadataReport, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::read_color_metadata(request, running.cancel.clone()).await
}

#[tauri::command]
async fn detect_crop(
    id: String,
    request: AutoCropRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
    store: State<'_, Preferences>,
) -> Result<AutoCropResult, AppError> {
    let running = tasks.run(&id)?;
    let count = store.get()?.general.auto_crop_samples;
    media_runtime::detect_crop_with_samples(request, running.cancel.clone(), count).await
}

#[tauri::command]
async fn get_encoder_parameters(
    id: String,
    request: media_core::EncoderParameterQuery,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::EncoderParameterCatalog, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::get_encoder_parameters(request.encoder, request.backend, running.cancel.clone())
        .await
}

#[tauri::command]
async fn preview_encode_plan(
    id: String,
    mut request: EncodeRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
    store: State<'_, Preferences>,
) -> Result<media_core::EncodeCommandPlan, AppError> {
    let running = tasks.run(&id)?;
    apply_encoding_preferences(&mut request, &store)?;
    media_runtime::preview_encode_plan(request, running.cancel.clone()).await
}

struct Jobs {
    manager: Arc<JobManager>,
    subscription: Arc<AtomicU64>,
}

#[tauri::command]
async fn analyze_quality(
    id: String,
    request: media_core::QualityRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::QualityResult, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::analyze_quality(request, running.cancel.clone()).await
}

#[tauri::command]
async fn analyze_bitrate(
    id: String,
    request: BitrateRequest,
    source_chart: Option<bool>,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<BitrateResult, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::supervisor::with_console_profile(
        if source_chart == Some(true) {
            media_runtime::supervisor::ConsoleProfile::General
        } else {
            media_runtime::supervisor::ConsoleProfile::Hidden
        },
        || media_runtime::analyze_bitrate(request, running.cancel.clone()),
    )
    .await
}

#[tauri::command]
async fn start_remux(
    request: RemuxRequest,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<JobSnapshot, AppError> {
    let _admission = monitor.admit_work().await?;
    jobs.manager.start_remux(request).await
}

#[tauri::command]
async fn set_job_paused(
    id: String,
    paused: bool,
    jobs: State<'_, Jobs>,
) -> Result<JobSnapshot, AppError> {
    jobs.manager.set_job_paused(id, paused).await
}

#[tauri::command]
async fn measure_loudness(
    id: String,
    request: LoudnessRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<LoudnessResult, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::measure_loudness(request, running.cancel.clone()).await
}

#[tauri::command]
async fn start_mux(
    request: media_core::MuxRequest,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<JobSnapshot, AppError> {
    let _admission = monitor.admit_work().await?;
    jobs.manager.start_mux(request).await
}

#[tauri::command]
async fn start_encode(
    mut request: EncodeRequest,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
    store: State<'_, Preferences>,
) -> Result<JobSnapshot, AppError> {
    let _admission = monitor.admit_work().await?;
    apply_encoding_preferences(&mut request, &store)?;
    jobs.manager.start_encode(request).await
}

#[tauri::command]
async fn enqueue_encode(
    mut request: EncodeRequest,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
    store: State<'_, Preferences>,
) -> Result<JobSnapshot, AppError> {
    let _admission = monitor.admit_work().await?;
    apply_encoding_preferences(&mut request, &store)?;
    jobs.manager.enqueue_encode(request).await
}

#[tauri::command]
async fn scan_media_folder(request: FolderScanRequest) -> Result<FolderScanResult, AppError> {
    media_runtime::scan_media_folder(request).await
}

#[tauri::command]
async fn preview_encode_batch(
    mut request: BatchEncodeRequest,
    id: Option<String>,
    jobs: State<'_, Jobs>,
    store: State<'_, Preferences>,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<BatchEncodePreview, AppError> {
    if request.encoding_preferences.is_none() {
        request.encoding_preferences = Some((&store.get()?.general).into());
    }
    if let Some(id) = id {
        let running = tasks.run(&id)?;
        jobs.manager
            .preview_encode_batch_cancellable(request, running.cancel.clone())
            .await
    } else {
        jobs.manager.preview_encode_batch(request).await
    }
}

#[tauri::command]
async fn enqueue_encode_batch(
    mut requests: Vec<EncodeRequest>,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
    store: State<'_, Preferences>,
) -> Result<Vec<JobSnapshot>, AppError> {
    let _admission = monitor.admit_work().await?;
    for request in &mut requests {
        apply_encoding_preferences(request, &store)?;
    }
    jobs.manager.enqueue_encode_batch(requests).await
}

#[tauri::command]
async fn cancel_all_jobs(jobs: State<'_, Jobs>) -> Result<Vec<JobSnapshot>, AppError> {
    jobs.manager.cancel_all_jobs().await
}

#[tauri::command]
async fn cancel_job(id: String, jobs: State<'_, Jobs>) -> Result<JobSnapshot, AppError> {
    jobs.manager.cancel_job(id).await
}

#[tauri::command]
async fn stop_job(id: String, jobs: State<'_, Jobs>) -> Result<JobSnapshot, AppError> {
    jobs.manager.stop_job(id).await
}

#[tauri::command]
async fn resume_job(
    id: String,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<JobSnapshot, AppError> {
    let _admission = monitor.admit_work().await?;
    jobs.manager.resume_job(id).await
}

#[tauri::command]
async fn list_av1an_recoveries(
    jobs: State<'_, Jobs>,
) -> Result<Vec<media_core::Av1anRecoveryEntry>, AppError> {
    jobs.manager.list_av1an_recoveries().await
}

#[tauri::command]
async fn av1an_recovery_action(
    request: media_core::Av1anRecoveryAction,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<media_core::Av1anRecoveryActionResult, AppError> {
    let _admission = if matches!(
        &request,
        media_core::Av1anRecoveryAction::ResumeSaved { .. }
            | media_core::Av1anRecoveryAction::ResumeNew { .. }
    ) {
        Some(monitor.admit_work().await?)
    } else {
        None
    };
    jobs.manager.av1an_recovery_action(request).await
}

#[tauri::command]
async fn discard_av1an_recovery(
    id: String,
    jobs: State<'_, Jobs>,
) -> Result<JobSnapshot, AppError> {
    jobs.manager.discard_av1an_recovery(id).await
}

#[tauri::command]
async fn list_jobs(jobs: State<'_, Jobs>) -> Result<Vec<JobSnapshot>, AppError> {
    jobs.manager.ready().await?;
    Ok(jobs.manager.list_jobs().await)
}

#[tauri::command]
async fn subscribe_jobs(
    channel: Channel<Vec<JobSnapshot>>,
    jobs: State<'_, Jobs>,
) -> Result<(), AppError> {
    jobs.manager.ready().await?;
    let manager = Arc::clone(&jobs.manager);
    let generation = Arc::clone(&jobs.subscription);
    let current = generation.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn(async move {
        let mut previous = None;
        while generation.load(Ordering::SeqCst) == current {
            let snapshot = manager.list_jobs().await;
            if previous.as_ref() != Some(&snapshot) {
                if channel.send(snapshot.clone()).is_err() {
                    break;
                }
                previous = Some(snapshot);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
    Ok(())
}

#[tauri::command]
async fn import_image_sequence(
    id: String,
    request: media_core::ImageSequenceImportRequest,
    paths: State<'_, paths::AppPaths>,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<MediaFile, AppError> {
    let running = tasks.run(&id)?;
    Box::pin(media_runtime::import_image_sequence(
        request,
        paths.cache_dir.join("image-sequences"),
        running.cancel.clone(),
    ))
    .await
}
#[tauri::command]
async fn release_image_sequence(
    request: media_core::ImageSequenceReleaseRequest,
    paths: State<'_, paths::AppPaths>,
    jobs: State<'_, JobManager>,
) -> Result<(), AppError> {
    jobs.release_image_sequence(request, paths.cache_dir.join("image-sequences"))
        .await
}
#[tauri::command]
async fn probe_media(path: String) -> Result<MediaFile, AppError> {
    media_runtime::probe_media(path).await
}

#[tauri::command]
async fn get_capabilities() -> Vec<ToolInfo> {
    // Discovery checks several tools concurrently. Keep that combined future
    // off the IPC dispatcher's small main-thread stack.
    Box::pin(media_runtime::get_capabilities()).await
}

// Keep each generated command dispatcher in its own stack frame. A single
// generate_handler! containing every command exceeded the default Windows
// main-thread stack once capability discovery was invoked from the WebView.
#[inline(never)]
fn invoke_utilities(invoke: tauri::ipc::Invoke) -> bool {
    let handler: fn(tauri::ipc::Invoke) -> bool = tauri::generate_handler![
        estimate_av1an_resources,
        read_av1an_grain_table,
        make_av1an_grain_preset,
        run_utility,
        run_utility_batch,
        set_media_analysis_paused,
        inspect_utility_capabilities,
        inspect_saved_job,
        export_saved_job,
        run_image_job,
        extract_attachment,
        resolve_deinterlace
    ];
    handler(invoke)
}

#[inline(never)]
fn invoke_settings(invoke: tauri::ipc::Invoke) -> bool {
    let handler: fn(tauri::ipc::Invoke) -> bool = tauri::generate_handler![
        set_completion_options,
        get_completion_status,
        cancel_finish_action,
        export_analysis,
        get_preferences,
        get_parameter_presets,
        save_parameter_preset,
        remove_parameter_preset,
        save_preferences,
        remember_recent_media,
        preview_preference_import,
        recent_path_is_folder,
        get_storage_locations,
        save_activity_log,
        open_directory
    ];
    handler(invoke)
}

#[inline(never)]
fn invoke_analysis(invoke: tauri::ipc::Invoke) -> bool {
    let handler: fn(tauri::ipc::Invoke) -> bool = tauri::generate_handler![
        begin_media_analysis,
        cancel_media_analysis,
        preview_frame,
        snap_keyframe,
        detect_fields,
        read_color_metadata,
        detect_crop,
        get_encoder_parameters,
        preview_encode_plan,
        analyze_bitrate,
        analyze_quality,
        set_job_paused,
        measure_loudness,
        probe_media,
        resolve_output_path,
        resolve_batch_output_paths,
        prepare_command_edit,
        confirm_command_edit,
        cancel_command_edit,
        set_command_edit_paused,
        import_image_sequence,
        release_image_sequence,
        get_capabilities
    ];
    handler(invoke)
}

#[inline(never)]
fn invoke_jobs(invoke: tauri::ipc::Invoke) -> bool {
    let handler: fn(tauri::ipc::Invoke) -> bool = tauri::generate_handler![
        start_remux,
        start_mux,
        start_encode,
        enqueue_encode,
        scan_media_folder,
        preview_encode_batch,
        enqueue_encode_batch,
        cancel_all_jobs,
        cancel_job,
        stop_job,
        resume_job,
        discard_av1an_recovery,
        list_av1an_recoveries,
        av1an_recovery_action,
        list_jobs,
        subscribe_jobs
    ];
    handler(invoke)
}

fn invoke_command(invoke: tauri::ipc::Invoke) -> bool {
    match invoke.message.command() {
        "estimate_av1an_resources"
        | "read_av1an_grain_table"
        | "make_av1an_grain_preset"
        | "run_utility"
        | "run_utility_batch"
        | "set_media_analysis_paused"
        | "inspect_utility_capabilities"
        | "inspect_saved_job"
        | "export_saved_job"
        | "run_image_job"
        | "extract_attachment" => invoke_utilities(invoke),
        "resolve_deinterlace" => invoke_utilities(invoke),
        "set_completion_options"
        | "get_completion_status"
        | "cancel_finish_action"
        | "export_analysis"
        | "get_preferences"
        | "get_parameter_presets"
        | "save_parameter_preset"
        | "remove_parameter_preset"
        | "save_preferences"
        | "remember_recent_media"
        | "preview_preference_import"
        | "recent_path_is_folder"
        | "get_storage_locations"
        | "save_activity_log"
        | "open_directory" => invoke_settings(invoke),
        "begin_media_analysis"
        | "cancel_media_analysis"
        | "preview_frame"
        | "snap_keyframe"
        | "detect_fields"
        | "read_color_metadata"
        | "detect_crop"
        | "get_encoder_parameters"
        | "preview_encode_plan"
        | "analyze_bitrate"
        | "analyze_quality"
        | "set_job_paused"
        | "measure_loudness"
        | "probe_media"
        | "resolve_output_path"
        | "resolve_batch_output_paths"
        | "prepare_command_edit"
        | "confirm_command_edit"
        | "cancel_command_edit"
        | "set_command_edit_paused"
        | "import_image_sequence"
        | "release_image_sequence"
        | "get_capabilities" => invoke_analysis(invoke),
        "start_remux"
        | "start_mux"
        | "start_encode"
        | "enqueue_encode"
        | "scan_media_folder"
        | "preview_encode_batch"
        | "enqueue_encode_batch"
        | "cancel_all_jobs"
        | "cancel_job"
        | "stop_job"
        | "resume_job"
        | "discard_av1an_recovery"
        | "list_av1an_recoveries"
        | "av1an_recovery_action"
        | "list_jobs"
        | "subscribe_jobs" => invoke_jobs(invoke),
        _ => false,
    }
}

pub fn run() {
    let exiting = Arc::new(AtomicU8::new(0));
    tauri::Builder::default()
        // This must run before setup opens the persisted history and WebView.
        // Scoop launches and version directories all share the same app ID.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let main_window = app
                .config()
                .app
                .windows
                .iter()
                .find(|window| window.label == "main")
                .cloned()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "the main window configuration is missing",
                    )
                })?;
            let executable = std::env::current_exe()?;
            #[cfg(target_os = "linux")]
            let executable = {
                let appimage = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from);
                let appdir = std::env::var_os("APPDIR").map(std::path::PathBuf::from);
                paths::launcher_path(&executable, appimage.as_deref(), appdir.as_deref())?
            };
            let paths = paths::AppPaths::resolve(
                &executable,
                paths::InstalledPaths {
                    resource_dir: app.path().resource_dir()?,
                    config_dir: app.path().app_config_dir()?,
                    data_dir: app.path().app_data_dir()?,
                    cache_dir: app.path().app_cache_dir()?,
                    log_dir: app.path().app_log_dir()?,
                },
            )?;
            paths.prepare()?;
            media_runtime::configure_bundled_tools(paths.resource_dir.clone())
                .map_err(std::io::Error::other)?;
            let log_dir = paths.job_log_dir();
            let history_dir = paths.history_dir();
            let webview_data_dir = paths.webview_data_dir();
            app.manage(Arc::new(
                media_runtime::preferences::PreferencesStore::open(paths.config_dir.clone()),
            ));
            app.manage(paths);
            app.manage(analysis::AnalysisTasks::default());
            app.manage(completion::CompletionMonitor::default());
            app.manage(Jobs {
                manager: Arc::new(tauri::async_runtime::block_on(JobManager::open(
                    log_dir,
                    history_dir,
                ))),
                subscription: Arc::new(AtomicU64::new(0)),
            });
            let mut main_window =
                tauri::WebviewWindowBuilder::from_config(app.handle(), &main_window)?;
            if let Some(data_directory) = webview_data_dir {
                main_window = main_window.data_directory(data_directory);
            }
            main_window
                .on_page_load(|window, payload| {
                    if payload.event() == tauri::webview::PageLoadEvent::Started {
                        let jobs = Arc::clone(&window.state::<Jobs>().manager);
                        tauri::async_runtime::spawn(async move {
                            let _ = jobs.cancel_command_reviews().await;
                        });
                    }
                })
                .build()?;
            completion::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(invoke_command)
        .build(tauri::generate_context!())
        .expect("could not start jesses")
        .run(move |app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if exiting.load(Ordering::SeqCst) == 2 {
                    return;
                }
                api.prevent_exit();
                if exiting
                    .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let app = app.clone();
                    let manager = Arc::clone(&app.state::<Jobs>().manager);
                    let exiting = Arc::clone(&exiting);
                    tauri::async_runtime::spawn(async move {
                        let analyses = app.state::<analysis::AnalysisTasks>();
                        tokio::join!(manager.shutdown(), analyses.shutdown());
                        exiting.store(2, Ordering::SeqCst);
                        app.exit(code.unwrap_or(0));
                    });
                }
            }
        });
}
