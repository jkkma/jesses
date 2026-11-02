#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = media_runtime::supervisor::console_helper_main() {
        std::process::exit(code);
    }
    media_runtime::supervisor::initialize_console_host()
        .expect("Native console host initialization failed");
    jesses_lib::run();
}
