fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "probe_media",
            "get_capabilities",
            "start_remux",
            "cancel_job",
            "list_jobs",
            "subscribe_jobs",
        ]),
    ))
    .expect("failed to build jesses desktop resources");
}
