use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use media_core::{AppError, EncodeJob, EncodeRequest, MediaFile, ToolInfo};
use media_runtime::JobManager;
use tauri::Manager;

#[tauri::command]
async fn probe_media(path: String) -> Result<MediaFile, AppError> {
    media_runtime::probe_media(path).await
}

#[tauri::command]
async fn get_capabilities() -> Vec<ToolInfo> {
    media_runtime::get_capabilities().await
}

#[tauri::command]
async fn start_encode(
    request: EncodeRequest,
    jobs: tauri::State<'_, Arc<JobManager>>,
) -> Result<EncodeJob, AppError> {
    jobs.inner().start(request).await
}

#[tauri::command]
async fn list_jobs(jobs: tauri::State<'_, Arc<JobManager>>) -> Result<Vec<EncodeJob>, AppError> {
    let jobs = Arc::clone(jobs.inner());
    tokio::task::spawn_blocking(move || jobs.list())
        .await
        .map_err(|error| AppError::new("JOB_STORE_FAILED", error.to_string(), None))?
}

#[tauri::command]
async fn cancel_encode(
    id: String,
    jobs: tauri::State<'_, Arc<JobManager>>,
) -> Result<(), AppError> {
    jobs.cancel(&id)
}

pub fn run() {
    // 0: running, 1: cancelling/draining, 2: safe to exit.
    let exit_state = Arc::new(AtomicU8::new(0));
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let directory = app.path().app_local_data_dir()?.join("jobs");
            app.manage(Arc::new(JobManager::open(directory)?));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            probe_media,
            get_capabilities,
            start_encode,
            list_jobs,
            cancel_encode
        ])
        .build(tauri::generate_context!())
        .expect("could not start jesses")
        .run(move |app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                if exit_state.load(Ordering::Acquire) == 2 {
                    return;
                }
                api.prevent_exit();
                if exit_state
                    .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    let jobs = Arc::clone(app.state::<Arc<JobManager>>().inner());
                    jobs.shutdown();
                    let app = app.clone();
                    let exit_state = Arc::clone(&exit_state);
                    tauri::async_runtime::spawn(async move {
                        jobs.wait_idle().await;
                        exit_state.store(2, Ordering::Release);
                        app.exit(0);
                    });
                }
            }
        });
}
