use media_core::{
    AppError, BatchEncodePreview, BatchEncodeRequest, EncodeRequest, FolderScanRequest,
    FolderScanResult, JobSnapshot, MediaFile, RemuxRequest, ToolInfo,
};
use media_runtime::jobs::JobManager;
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::Duration;
use tauri::{Manager, State, ipc::Channel};

struct Jobs {
    manager: Arc<JobManager>,
    subscription: Arc<AtomicU64>,
}

#[tauri::command]
async fn start_remux(
    request: RemuxRequest,
    jobs: State<'_, Jobs>,
) -> Result<JobSnapshot, AppError> {
    jobs.manager.start_remux(request).await
}

#[tauri::command]
async fn start_encode(
    request: EncodeRequest,
    jobs: State<'_, Jobs>,
) -> Result<JobSnapshot, AppError> {
    jobs.manager.start_encode(request).await
}

#[tauri::command]
async fn enqueue_encode(
    request: EncodeRequest,
    jobs: State<'_, Jobs>,
) -> Result<JobSnapshot, AppError> {
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
) -> Result<Vec<JobSnapshot>, AppError> {
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
        .setup(|app| {
            let log_dir = app.path().app_log_dir()?.join("jobs");
            let history_dir = app.path().app_data_dir()?.join("jobs");
            app.manage(Jobs {
                manager: Arc::new(tauri::async_runtime::block_on(JobManager::open(
                    log_dir,
                    history_dir,
                ))),
                subscription: Arc::new(AtomicU64::new(0)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            probe_media,
            get_capabilities,
            start_remux,
            start_encode,
            enqueue_encode,
            scan_media_folder,
            preview_encode_batch,
            enqueue_encode_batch,
            cancel_all_jobs,
            cancel_job,
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
                        manager.shutdown().await;
                        exiting.store(2, Ordering::SeqCst);
                        app.exit(code.unwrap_or(0));
                    });
                }
            }
        });
}
