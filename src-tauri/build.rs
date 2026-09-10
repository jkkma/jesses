fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "probe_media",
            "get_capabilities",
            "start_remux",
            "start_encode",
            "enqueue_encode",
            "scan_media_folder",
            "preview_encode_batch",
            "enqueue_encode_batch",
            "cancel_all_jobs",
            "cancel_job",
            "list_jobs",
            "subscribe_jobs",
        ]),
    ))
    .expect("failed to build jesses desktop resources");
}
