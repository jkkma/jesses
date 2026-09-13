//! Run package tool discovery without launching a desktop window or opening history.
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let require_media = std::env::args_os()
        .skip(2)
        .any(|argument| argument == "--require-media");
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("Pass the absolute package resource directory.");
    media_runtime::configure_bundled_tools(directory.clone()).expect("Valid resource directory");
    let tools = media_runtime::get_capabilities().await;
    println!("{}", serde_json::to_string_pretty(&tools).unwrap());
    let directory = directory
        .canonicalize()
        .expect("Existing package directory");
    if tools
        .iter()
        .filter(|tool| {
            matches!(tool.id.as_str(), "svt-av1-5fish" | "svt-av1-hdr")
                || (require_media && matches!(tool.id.as_str(), "ffmpeg" | "ffprobe"))
        })
        .any(|tool| {
            !tool.available
                || !tool
                    .path
                    .as_ref()
                    .is_some_and(|path| PathBuf::from(path).starts_with(&directory))
        })
    {
        std::process::exit(1);
    }
}
