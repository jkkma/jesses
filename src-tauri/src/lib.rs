use media_core::{AppError, MediaFile, ToolInfo};

#[tauri::command]
async fn probe_media(path: String) -> Result<MediaFile, AppError> {
    media_runtime::probe_media(path).await
}

#[tauri::command]
async fn get_capabilities() -> Vec<ToolInfo> {
    media_runtime::get_capabilities().await
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![probe_media, get_capabilities])
        .run(tauri::generate_context!())
        .expect("could not start jesses");
}
