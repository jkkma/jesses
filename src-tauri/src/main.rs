#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = media_runtime::av1an_encoder_helper_main() {
        std::process::exit(code);
    }
    if let Some(code) = media_runtime::supervisor::console_helper_main() {
        std::process::exit(code);
    }
    media_runtime::configure_av1an_encoder_helper(
        std::env::current_exe().expect("Cannot resolve the Jesses application executable"),
    )
    .expect("AV1AN encoder helper initialization failed");
    media_runtime::supervisor::initialize_console_host()
        .expect("Native console host initialization failed");
    jesses_lib::run();
}
