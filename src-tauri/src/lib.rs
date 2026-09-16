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
use tauri::{Manager, State, ipc::Channel};

mod analysis;
mod completion;
mod paths;

#[tauri::command]
async fn run_utility(
    id: String,
    request: media_core::UtilityRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::UtilityResult, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::run_utility(request, running.cancel.clone()).await
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
async fn inspect_saved_job(path: String) -> Result<media_core::SavedJobInspection, AppError> {
    tauri::async_runtime::spawn_blocking(move || media_runtime::inspect_saved_job(path))
        .await
        .map_err(|e| AppError::new("SAVED_JOB_UNREADABLE", e.to_string(), None))?
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
async fn detect_crop(
    id: String,
    request: AutoCropRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<AutoCropResult, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::detect_crop(request, running.cancel.clone()).await
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
    request: EncodeRequest,
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<media_core::EncodeCommandPlan, AppError> {
    let running = tasks.run(&id)?;
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
    tasks: State<'_, analysis::AnalysisTasks>,
) -> Result<BitrateResult, AppError> {
    let running = tasks.run(&id)?;
    media_runtime::analyze_bitrate(request, running.cancel.clone()).await
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
    request: EncodeRequest,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<JobSnapshot, AppError> {
    let _admission = monitor.admit_work().await?;
    jobs.manager.start_encode(request).await
}

#[tauri::command]
async fn enqueue_encode(
    request: EncodeRequest,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<JobSnapshot, AppError> {
    let _admission = monitor.admit_work().await?;
    jobs.manager.enqueue_encode(request).await
}

#[tauri::command]
async fn scan_media_folder(request: FolderScanRequest) -> Result<FolderScanResult, AppError> {
    media_runtime::scan_media_folder(request).await
}

#[tauri::command]
async fn preview_encode_batch(
    request: BatchEncodeRequest,
    jobs: State<'_, Jobs>,
) -> Result<BatchEncodePreview, AppError> {
    jobs.manager.preview_encode_batch(request).await
}

#[tauri::command]
async fn enqueue_encode_batch(
    requests: Vec<EncodeRequest>,
    jobs: State<'_, Jobs>,
    monitor: State<'_, completion::CompletionMonitor>,
) -> Result<Vec<JobSnapshot>, AppError> {
    let _admission = monitor.admit_work().await?;
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
async fn probe_media(path: String) -> Result<MediaFile, AppError> {
    media_runtime::probe_media(path).await
}

#[tauri::command]
async fn get_capabilities() -> Vec<ToolInfo> {
    media_runtime::get_capabilities().await
}

pub fn run() {
    let exiting = Arc::new(AtomicU8::new(0));
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
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
            completion::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            run_utility,
            inspect_utility_capabilities,
            inspect_saved_job,
            export_saved_job,
            run_image_job,
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
            begin_media_analysis,
            cancel_media_analysis,
            preview_frame,
            detect_crop,
            get_encoder_parameters,
            preview_encode_plan,
            analyze_bitrate,
            analyze_quality,
            set_job_paused,
            measure_loudness,
            probe_media,
            get_capabilities,
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
            list_jobs,
            subscribe_jobs
        ])
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
